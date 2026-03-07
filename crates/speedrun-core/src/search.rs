//! Text search across the timeline of a recording.
//!
//! Provides [`SearchHit`] and search functions that scan terminal screen
//! content at each output event, using keyframe acceleration to avoid
//! full replay from the beginning.
//!
//! ## Limitations
//!
//! - Words that wrap across physical screen lines will NOT be found.
//!   Each physical line is searched independently via `line.text()`.
//! - Only substring matching is supported (no regex).
//! - Search is case-insensitive.

use crate::index::KeyframeIndex;
use crate::parser::{EventData, EventType, Recording};
use crate::snapshot::create_vt;
use crate::timemap::TimeMap;

/// A single search match at a specific point in time.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    /// Effective time when this text is visible on screen.
    pub time: f64,
    /// Physical screen row (0-indexed).
    pub row: usize,
    /// Physical screen column (0-indexed, in terminal columns not char indices).
    pub col: usize,
    /// Match length in terminal columns.
    pub length: usize,
}

/// Feed a single event into a throwaway `Vt`, mirroring `Player::replay_to` logic.
fn feed_event(vt: &mut avt::Vt, event: &crate::parser::Event) {
    match (&event.event_type, &event.data) {
        (EventType::Output, EventData::Text(data)) => {
            let _ = vt.feed_str(data);
        }
        (EventType::Resize, EventData::Resize { cols, rows }) => {
            let _ = vt.feed_str(&format!("\x1b[8;{rows};{cols}t"));
        }
        // Input and Marker events don't affect terminal state
        (EventType::Input, _) | (EventType::Marker, _) => {}
        _ => {}
    }
}

/// Scan all lines of the virtual terminal for a case-insensitive substring match.
/// Returns the first match found (top-to-bottom, left-to-right).
fn scan_screen(vt: &avt::Vt, query_lower: &str, time: f64) -> Option<SearchHit> {
    let view = vt.view();
    for (row_idx, line) in view.iter().enumerate() {
        let text = line.text();
        let text_lower = text.to_lowercase();
        if let Some(char_idx) = text_lower.find(query_lower) {
            // Map char byte index to column position.
            // For ASCII this is 1:1. For wide chars we need to walk cells.
            let col = char_index_to_col(line, char_idx);
            let match_end_char_idx = char_idx + query_lower.len();
            let col_end = char_index_to_col(line, match_end_char_idx);
            return Some(SearchHit {
                time,
                row: row_idx,
                col,
                length: col_end - col,
            });
        }
    }
    None
}

/// Scan all lines for the LAST match (bottom-to-top, right-to-left).
/// Used for backward search to find the latest match in a keyframe interval.
fn scan_screen_last(vt: &avt::Vt, query_lower: &str, time: f64) -> Option<SearchHit> {
    let view = vt.view();
    let mut last_hit: Option<SearchHit> = None;
    for (row_idx, line) in view.iter().enumerate() {
        let text = line.text();
        let text_lower = text.to_lowercase();
        // Find all occurrences, keep the last one on this line
        let mut search_start = 0;
        while let Some(pos) = text_lower[search_start..].find(query_lower) {
            let char_idx = search_start + pos;
            let col = char_index_to_col(line, char_idx);
            let match_end_char_idx = char_idx + query_lower.len();
            let col_end = char_index_to_col(line, match_end_char_idx);
            last_hit = Some(SearchHit {
                time,
                row: row_idx,
                col,
                length: col_end - col,
            });
            search_start = char_idx + 1;
            if search_start >= text_lower.len() {
                break;
            }
        }
    }
    last_hit
}

/// Map a byte index in `line.text()` to a terminal column position.
///
/// For ASCII text, byte index == column. For wide characters (e.g. CJK),
/// one character may occupy 2 columns.
fn char_index_to_col(line: &avt::Line, byte_idx: usize) -> usize {
    // line.text() returns a string. We need to map byte_idx in that string
    // to a column position. We walk the cells to build a mapping.
    let text = line.text();
    if byte_idx == 0 {
        return 0;
    }
    if byte_idx >= text.len() {
        // Count total columns from cells
        let mut total_cols = 0;
        for (ch, _pen) in line.cells() {
            total_cols += if ch == ' ' || ch.is_ascii() {
                1
            } else {
                unicode_width(ch)
            };
        }
        return total_cols;
    }

    // Walk through cells, accumulating both byte offset and column offset
    let mut current_byte = 0;
    let mut current_col = 0;

    for (ch, _pen) in line.cells() {
        if current_byte >= byte_idx {
            break;
        }
        let char_len = ch.len_utf8();
        let col_width = if ch == ' ' || ch.is_ascii() {
            1
        } else {
            unicode_width(ch)
        };
        current_byte += char_len;
        current_col += col_width;
    }

    current_col
}

/// Estimate the terminal column width of a character.
fn unicode_width(ch: char) -> usize {
    // CJK Unified Ideographs and related blocks are typically double-width
    let cp = ch as u32;
    if (0x1100..=0x115F).contains(&cp) // Hangul Jamo
        || (0x2E80..=0x9FFF).contains(&cp) // CJK
        || (0xAC00..=0xD7AF).contains(&cp) // Hangul Syllables
        || (0xF900..=0xFAFF).contains(&cp) // CJK Compatibility
        || (0xFE10..=0xFE6F).contains(&cp) // CJK forms
        || (0xFF01..=0xFF60).contains(&cp) // Fullwidth forms
        || (0xFFE0..=0xFFE6).contains(&cp) // Fullwidth signs
        || (0x20000..=0x2FA1F).contains(&cp)
    // CJK Extension B+
    {
        2
    } else {
        1
    }
}

/// Search forward through the recording for the first screen containing `query`
/// after `from_time`. Wraps around to the beginning if no match is found.
///
/// This function does NOT modify any player state — it uses throwaway `Vt` instances.
///
/// Words that wrap across physical screen lines will not be found.
pub fn search_forward(
    recording: &Recording,
    time_map: &TimeMap,
    index: &KeyframeIndex,
    query: &str,
    from_time: f64,
) -> Option<SearchHit> {
    if query.is_empty() {
        return None;
    }
    if recording.events.is_empty() {
        return None;
    }

    let query_lower = query.to_lowercase();

    // Phase 1: Search from from_time to end
    if let Some(hit) =
        search_forward_range(recording, time_map, index, &query_lower, from_time, None)
    {
        return Some(hit);
    }

    // Phase 2: Wrap around — search from beginning to from_time
    search_forward_range(
        recording,
        time_map,
        index,
        &query_lower,
        0.0,
        Some(from_time),
    )
}

/// Search forward in a time range [after from_time, up to end_time).
/// If end_time is None, search to the end of the recording.
fn search_forward_range(
    recording: &Recording,
    time_map: &TimeMap,
    index: &KeyframeIndex,
    query_lower: &str,
    from_time: f64,
    end_time: Option<f64>,
) -> Option<SearchHit> {
    // Find keyframe at or before from_time
    let mut vt = match index.keyframe_at(from_time) {
        Some(kf_idx) => {
            let kf = index.get(kf_idx).expect("keyframe index in bounds");
            let vt = kf.snapshot.restore();
            // Replay events from keyframe's event_index up to from_time
            let mut vt = vt;
            for i in kf.event_index..recording.events.len() {
                let Some(t) = time_map.effective_time(i) else {
                    break;
                };
                if t > from_time {
                    // Now scan from this event index forward
                    return search_forward_from_event(
                        recording,
                        time_map,
                        &mut vt,
                        i,
                        from_time,
                        end_time,
                        query_lower,
                    );
                }
                feed_event(&mut vt, &recording.events[i]);
            }
            // All events were at or before from_time, nothing to search
            return None;
        }
        None => {
            // No keyframe before from_time — start from scratch
            create_vt(
                recording.header.width as usize,
                recording.header.height as usize,
            )
        }
    };

    // If we get here, index was empty or from_time is before first keyframe
    search_forward_from_event(
        recording,
        time_map,
        &mut vt,
        0,
        from_time,
        end_time,
        query_lower,
    )
}

/// Replay events from `start_event_idx` forward, scanning screen after each output
/// event whose effective time is > from_time (and optionally < end_time).
fn search_forward_from_event(
    recording: &Recording,
    time_map: &TimeMap,
    vt: &mut avt::Vt,
    start_event_idx: usize,
    from_time: f64,
    end_time: Option<f64>,
    query_lower: &str,
) -> Option<SearchHit> {
    for i in start_event_idx..recording.events.len() {
        let Some(t) = time_map.effective_time(i) else {
            break;
        };
        if let Some(end) = end_time
            && t >= end
        {
            break;
        }

        feed_event(vt, &recording.events[i]);

        // Only scan after output events whose time is > from_time
        if t > from_time
            && recording.events[i].event_type == EventType::Output
            && let Some(hit) = scan_screen(vt, query_lower, t)
        {
            return Some(hit);
        }
    }
    None
}

/// Search backward through the recording for the last screen containing `query`
/// before `from_time`. Wraps around to the end if no match is found.
///
/// This function does NOT modify any player state — it uses throwaway `Vt` instances.
///
/// Words that wrap across physical screen lines will not be found.
pub fn search_backward(
    recording: &Recording,
    time_map: &TimeMap,
    index: &KeyframeIndex,
    query: &str,
    from_time: f64,
) -> Option<SearchHit> {
    if query.is_empty() {
        return None;
    }
    if recording.events.is_empty() {
        return None;
    }

    let query_lower = query.to_lowercase();

    // Phase 1: Search backward from from_time to beginning
    if let Some(hit) = search_backward_range(recording, time_map, index, &query_lower, from_time) {
        return Some(hit);
    }

    // Phase 2: Wrap around — search backward from end to from_time
    // We search the entire recording and find the last match at time >= from_time
    let duration = time_map.duration();
    if duration <= 0.0 {
        return None;
    }

    search_backward_wrap(
        recording,
        time_map,
        index,
        &query_lower,
        from_time,
        duration,
    )
}

/// Search backward from `from_time` to the beginning of the recording.
/// Works backward through keyframe intervals.
fn search_backward_range(
    recording: &Recording,
    time_map: &TimeMap,
    index: &KeyframeIndex,
    query_lower: &str,
    from_time: f64,
) -> Option<SearchHit> {
    if index.is_empty() {
        // No keyframes — replay from start, collect last match before from_time
        let mut vt = create_vt(
            recording.header.width as usize,
            recording.header.height as usize,
        );
        return replay_collect_last_match(
            recording,
            time_map,
            &mut vt,
            0,
            query_lower,
            None,
            Some(from_time),
        );
    }

    // Find the keyframe at or before from_time
    let start_kf = index.keyframe_at(from_time)?;

    // Search backward through keyframe intervals
    let mut kf_idx = start_kf;
    loop {
        let kf = index.get(kf_idx).expect("keyframe index in bounds");
        let mut vt = kf.snapshot.restore();

        // Determine the end boundary for this interval
        let upper_bound = if kf_idx == start_kf {
            Some(from_time)
        } else {
            // For previous keyframe intervals, search up to the next keyframe time
            index.get(kf_idx + 1).map(|next_kf| next_kf.time)
        };

        let hit = replay_collect_last_match(
            recording,
            time_map,
            &mut vt,
            kf.event_index,
            query_lower,
            None,
            upper_bound,
        );

        if hit.is_some() {
            return hit;
        }

        if kf_idx == 0 {
            break;
        }
        kf_idx -= 1;
    }

    None
}

/// Search for the last match at time >= from_time (wrap-around for backward search).
fn search_backward_wrap(
    recording: &Recording,
    time_map: &TimeMap,
    index: &KeyframeIndex,
    query_lower: &str,
    from_time: f64,
    _duration: f64,
) -> Option<SearchHit> {
    if index.is_empty() {
        let mut vt = create_vt(
            recording.header.width as usize,
            recording.header.height as usize,
        );
        return replay_collect_last_match(
            recording,
            time_map,
            &mut vt,
            0,
            query_lower,
            Some(from_time),
            None,
        );
    }

    // Search backward from the last keyframe to the one at from_time
    let last_kf_idx = index.len() - 1;
    let start_kf = index.keyframe_at(from_time).unwrap_or(0);

    let mut kf_idx = last_kf_idx;
    loop {
        if kf_idx < start_kf {
            break;
        }

        let kf = index.get(kf_idx).expect("keyframe index in bounds");
        let mut vt = kf.snapshot.restore();

        let lower_bound = if kf_idx == start_kf {
            Some(from_time)
        } else {
            None
        };

        let upper_bound = if kf_idx < last_kf_idx {
            index.get(kf_idx + 1).map(|next_kf| next_kf.time)
        } else {
            None
        };

        let hit = replay_collect_last_match(
            recording,
            time_map,
            &mut vt,
            kf.event_index,
            query_lower,
            lower_bound,
            upper_bound,
        );

        if hit.is_some() {
            return hit;
        }

        if kf_idx == start_kf || kf_idx == 0 {
            break;
        }
        kf_idx -= 1;
    }

    None
}

/// Replay events and collect the last match within [lower_bound, upper_bound).
fn replay_collect_last_match(
    recording: &Recording,
    time_map: &TimeMap,
    vt: &mut avt::Vt,
    start_event_idx: usize,
    query_lower: &str,
    lower_bound: Option<f64>,
    upper_bound: Option<f64>,
) -> Option<SearchHit> {
    let mut last_hit: Option<SearchHit> = None;

    for i in start_event_idx..recording.events.len() {
        let Some(t) = time_map.effective_time(i) else {
            break;
        };
        if let Some(upper) = upper_bound
            && t >= upper
        {
            break;
        }

        feed_event(vt, &recording.events[i]);

        if recording.events[i].event_type == EventType::Output {
            let dominated_by_lower = match lower_bound {
                Some(lower) => t <= lower,
                None => false,
            };

            if !dominated_by_lower && let Some(hit) = scan_screen_last(vt, query_lower, t) {
                last_hit = Some(hit);
            }
        }
    }

    last_hit
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::KeyframeIndex;
    use crate::parser::Recording;
    use crate::player::Player;
    use crate::timemap::TimeMap;

    fn make_recording(events: &str) -> Vec<u8> {
        let header = r#"{"version":2,"width":80,"height":24}"#;
        format!("{header}\n{events}").into_bytes()
    }

    fn load_from_bytes(data: &[u8]) -> (Recording, TimeMap, KeyframeIndex) {
        let recording = crate::parse(std::io::Cursor::new(data)).unwrap();
        let raw_times: Vec<f64> = recording.events.iter().map(|e| e.time).collect();
        let time_map = TimeMap::build(&raw_times, recording.header.idle_time_limit).unwrap();
        let index =
            KeyframeIndex::build(&recording, &time_map, crate::index::KEYFRAME_INTERVAL).unwrap();
        (recording, time_map, index)
    }

    // -----------------------------------------------------------------------
    // Test 1: Basic forward search
    // -----------------------------------------------------------------------
    #[test]
    fn test_search_forward_basic() {
        let data = make_recording(r#"[1.0,"o","hello world"]"#);
        let (recording, time_map, index) = load_from_bytes(&data);

        let hit = search_forward(&recording, &time_map, &index, "hello", 0.0);
        assert_eq!(
            hit,
            Some(SearchHit {
                time: 1.0,
                row: 0,
                col: 0,
                length: 5,
            })
        );
    }

    // -----------------------------------------------------------------------
    // Test 2: No match
    // -----------------------------------------------------------------------
    #[test]
    fn test_search_forward_no_match() {
        let data = make_recording(r#"[1.0,"o","hello world"]"#);
        let (recording, time_map, index) = load_from_bytes(&data);

        let hit = search_forward(&recording, &time_map, &index, "nonexistent", 0.0);
        assert_eq!(hit, None);
    }

    // -----------------------------------------------------------------------
    // Test 3: Case insensitive
    // -----------------------------------------------------------------------
    #[test]
    fn test_search_forward_case_insensitive() {
        let data = make_recording(r#"[1.0,"o","Hello World"]"#);
        let (recording, time_map, index) = load_from_bytes(&data);

        let hit = search_forward(&recording, &time_map, &index, "hello world", 0.0);
        assert!(hit.is_some(), "case-insensitive search should find match");
        let hit = hit.unwrap();
        assert_eq!(hit.time, 1.0);
        assert_eq!(hit.row, 0);
        assert_eq!(hit.col, 0);
        assert_eq!(hit.length, 11);
    }

    // -----------------------------------------------------------------------
    // Test 4: Empty query
    // -----------------------------------------------------------------------
    #[test]
    fn test_search_forward_empty_query() {
        let data = make_recording(r#"[1.0,"o","hello world"]"#);
        let (recording, time_map, index) = load_from_bytes(&data);

        let hit = search_forward(&recording, &time_map, &index, "", 0.0);
        assert_eq!(hit, None);
    }

    // -----------------------------------------------------------------------
    // Test 5: Empty recording
    // -----------------------------------------------------------------------
    #[test]
    fn test_search_forward_empty_recording() {
        let data = make_recording("");
        let (recording, time_map, index) = load_from_bytes(&data);

        let hit = search_forward(&recording, &time_map, &index, "anything", 0.0);
        assert_eq!(hit, None);
    }

    // -----------------------------------------------------------------------
    // Test 6: Wraparound
    // -----------------------------------------------------------------------
    #[test]
    fn test_search_forward_wraparound() {
        // Use clear screen (\x1b[2J\x1b[H) before "bbb" so "aaa" is no longer
        // visible at t=5.0. This forces wraparound to find "aaa" at t=1.0.
        let data = make_recording(
            r#"[1.0,"o","aaa\r\n"]
[5.0,"o","\u001b[2J\u001b[Hbbb"]"#,
        );
        let (recording, time_map, index) = load_from_bytes(&data);

        // Search for "aaa" from t=3.0 — screen is cleared at t=5.0, so
        // no match found going forward. Wraps around and finds at t=1.0.
        let hit = search_forward(&recording, &time_map, &index, "aaa", 3.0);
        assert!(hit.is_some(), "wraparound should find 'aaa'");
        let hit = hit.unwrap();
        assert_eq!(hit.time, 1.0);
    }

    // -----------------------------------------------------------------------
    // Test 7: Mid-recording search
    // -----------------------------------------------------------------------
    #[test]
    fn test_search_forward_mid_recording() {
        let data = make_recording(
            r#"[1.0,"o","aaa\r\n"]
[3.0,"o","bbb\r\n"]
[5.0,"o","ccc"]"#,
        );
        let (recording, time_map, index) = load_from_bytes(&data);

        let hit = search_forward(&recording, &time_map, &index, "bbb", 0.0);
        assert!(hit.is_some());
        let hit = hit.unwrap();
        assert_eq!(hit.time, 3.0);
    }

    // -----------------------------------------------------------------------
    // Test 8: Multiple matches on screen — leftmost wins
    // -----------------------------------------------------------------------
    #[test]
    fn test_search_forward_multiple_on_screen() {
        let data = make_recording(r#"[1.0,"o","hello hello"]"#);
        let (recording, time_map, index) = load_from_bytes(&data);

        let hit = search_forward(&recording, &time_map, &index, "hello", 0.0);
        assert!(hit.is_some());
        let hit = hit.unwrap();
        assert_eq!(hit.col, 0, "leftmost match should win");
    }

    // -----------------------------------------------------------------------
    // Test 9: Search does not modify player state
    // -----------------------------------------------------------------------
    #[test]
    fn test_search_forward_does_not_modify_player() {
        let data = make_recording(
            r#"[1.0,"o","hello\r\n"]
[2.0,"o","world\r\n"]
[3.0,"o","test"]"#,
        );
        let mut player = Player::load(std::io::Cursor::new(&data)).unwrap();
        player.seek(2.0);

        let time_before = player.current_time();
        let screen_before: Vec<String> = player
            .screen()
            .iter()
            .map(|l| l.text().to_string())
            .collect();

        let _hit = player.search_forward("test", 0.0);

        assert!(
            (player.current_time() - time_before).abs() < 1e-9,
            "search should not modify current_time"
        );
        let screen_after: Vec<String> = player
            .screen()
            .iter()
            .map(|l| l.text().to_string())
            .collect();
        assert_eq!(
            screen_before, screen_after,
            "search should not modify screen"
        );
    }

    // -----------------------------------------------------------------------
    // Test 10: Backward search basic
    // -----------------------------------------------------------------------
    #[test]
    fn test_search_backward_basic() {
        // Clear screen before "bbb" so "aaa" is only visible at t=1.0
        let data = make_recording(
            r#"[1.0,"o","aaa\r\n"]
[3.0,"o","\u001b[2J\u001b[Hbbb"]"#,
        );
        let (recording, time_map, index) = load_from_bytes(&data);

        // Search backward for "aaa" from t=5.0 — screen was cleared at t=3.0,
        // so "aaa" is only visible at t=1.0.
        let hit = search_backward(&recording, &time_map, &index, "aaa", 5.0);
        assert!(hit.is_some());
        let hit = hit.unwrap();
        assert_eq!(hit.time, 1.0);
    }

    // -----------------------------------------------------------------------
    // Test 11: Backward search wraparound
    // -----------------------------------------------------------------------
    #[test]
    fn test_search_backward_wraparound() {
        let data = make_recording(
            r#"[1.0,"o","aaa\r\n"]
[3.0,"o","bbb"]"#,
        );
        let (recording, time_map, index) = load_from_bytes(&data);

        // Search backward for "bbb" from t=0.5 — should wrap around to find at t=3.0
        let hit = search_backward(&recording, &time_map, &index, "bbb", 0.5);
        assert!(hit.is_some(), "wraparound should find 'bbb'");
        let hit = hit.unwrap();
        assert_eq!(hit.time, 3.0);
    }

    // -----------------------------------------------------------------------
    // Test 12: Search between keyframes
    // -----------------------------------------------------------------------
    #[test]
    fn test_search_between_keyframes() {
        // Create a recording with events spanning > 5s (keyframe interval).
        // Place text at an event between two keyframes.
        let data = make_recording(
            r#"[0.5,"o","first\r\n"]
[3.0,"o","target_text\r\n"]
[6.0,"o","after keyframe\r\n"]
[9.0,"o","more\r\n"]
[12.0,"o","end"]"#,
        );
        let (recording, time_map, index) = load_from_bytes(&data);

        // "target_text" appears at t=3.0, which is between keyframes (0.0 and 5.0+)
        let hit = search_forward(&recording, &time_map, &index, "target_text", 0.0);
        assert!(hit.is_some(), "text between keyframes should be found");
        let hit = hit.unwrap();
        assert_eq!(hit.time, 3.0);
    }

    // -----------------------------------------------------------------------
    // Test 13: Integration test with real_session.cast — performance
    // -----------------------------------------------------------------------
    #[test]
    fn test_search_real_session_performance() {
        let mut path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("../../testdata/real_session.cast");
        let file = std::fs::File::open(&path).unwrap();
        let mut player = Player::load(file).unwrap();
        player.seek(0.0);

        let start = std::time::Instant::now();
        // Search for something that likely exists
        let _hit = player.search_forward("$", 0.0);
        let elapsed = start.elapsed();

        assert!(
            elapsed.as_secs_f64() < 1.0,
            "search_forward should complete within 1 second, took {elapsed:?}"
        );

        // Also test backward search performance
        let start = std::time::Instant::now();
        let _hit = player.search_backward("$", player.duration());
        let elapsed = start.elapsed();

        assert!(
            elapsed.as_secs_f64() < 1.0,
            "search_backward should complete within 1 second, took {elapsed:?}"
        );
    }
}
