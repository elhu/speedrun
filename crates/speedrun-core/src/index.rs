//! Keyframe index for O(log n) seek operations.
//!
//! The index replays all events through an `avt::Vt` terminal emulator,
//! capturing [`TerminalSnapshot`]s at regular effective-time intervals.
//! This allows the player to seek to any point by restoring the nearest
//! keyframe and replaying only a small number of events forward.

use crate::parser::{EventData, EventType, Recording};
use crate::snapshot::{TerminalSnapshot, create_vt};
use crate::timemap::TimeMap;

/// Interval between keyframes in effective time (seconds).
pub const KEYFRAME_INTERVAL: f64 = 5.0;

/// A snapshot of terminal state at a specific effective time.
#[derive(Debug, Clone)]
pub struct Keyframe {
    /// Effective timestamp (with idle limit applied).
    pub time: f64,
    /// Index of the first event NOT included in this snapshot.
    /// To resume playback from this keyframe, start replaying from
    /// `events[event_index]`.
    pub event_index: usize,
    /// Serialized terminal state.
    pub snapshot: TerminalSnapshot,
}

/// Index of keyframes for O(log n) seek lookup.
#[derive(Debug)]
pub struct KeyframeIndex {
    keyframes: Vec<Keyframe>,
}

impl KeyframeIndex {
    /// Build a keyframe index by replaying all events through a virtual terminal.
    ///
    /// Captures snapshots at `interval` seconds in effective time.
    /// Empty recordings produce an empty index.
    pub fn build(recording: &Recording, time_map: &TimeMap, interval: f64) -> Self {
        if recording.events.is_empty() {
            return KeyframeIndex {
                keyframes: Vec::new(),
            };
        }

        let mut vt = create_vt(
            recording.header.width as usize,
            recording.header.height as usize,
        );

        let mut keyframes = Vec::new();

        // Capture initial keyframe (time 0.0, before any events)
        keyframes.push(Keyframe {
            time: 0.0,
            event_index: 0,
            snapshot: TerminalSnapshot::capture(&vt),
        });

        let mut next_keyframe_time = interval;

        for i in 0..recording.events.len() {
            let effective_time = time_map
                .effective_time(i)
                .expect("event index must be within time_map bounds");

            // Before processing the event, check if we've crossed a keyframe boundary
            if effective_time >= next_keyframe_time {
                keyframes.push(Keyframe {
                    time: effective_time,
                    event_index: i,
                    snapshot: TerminalSnapshot::capture(&vt),
                });

                // Advance next_keyframe_time past the current event to avoid
                // double-captures. Handles large gaps spanning multiple intervals.
                while next_keyframe_time <= effective_time {
                    next_keyframe_time += interval;
                }
            }

            // Process the event through the terminal
            let event = &recording.events[i];
            match (&event.event_type, &event.data) {
                (EventType::Output, EventData::Text(data)) => {
                    let _ = vt.feed_str(data);
                }
                (EventType::Resize, EventData::Resize { cols, rows }) => {
                    // Critical: xtwinops takes rows;cols (opposite of EventData order)
                    let _ = vt.feed_str(&format!("\x1b[8;{rows};{cols}t"));
                }
                // Input and Marker events don't change terminal state
                (EventType::Input, _) | (EventType::Marker, _) => {}
                // Ignore mismatched type/data combinations
                _ => {}
            }
        }

        KeyframeIndex { keyframes }
    }

    /// Find the index of the last keyframe at or before `time`.
    ///
    /// Uses binary search for O(log n) lookup. Returns `None` if the index
    /// is empty or `time` is before the first keyframe.
    pub fn keyframe_at(&self, time: f64) -> Option<usize> {
        if self.keyframes.is_empty() {
            return None;
        }

        let count = self.keyframes.partition_point(|kf| kf.time <= time);
        if count == 0 { None } else { Some(count - 1) }
    }

    /// Get a keyframe by index.
    pub fn get(&self, index: usize) -> Option<&Keyframe> {
        self.keyframes.get(index)
    }

    /// Number of keyframes in the index.
    pub fn len(&self) -> usize {
        self.keyframes.len()
    }

    /// Returns true if the index contains no keyframes.
    pub fn is_empty(&self) -> bool {
        self.keyframes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{Event, EventData, EventType, Header, Recording};
    use crate::timemap::TimeMap;

    fn testdata_path(name: &str) -> std::path::PathBuf {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.push("../../testdata");
        p.push(name);
        p
    }

    fn load_test_recording(name: &str) -> Recording {
        let path = testdata_path(name);
        let file = std::fs::File::open(&path).unwrap();
        crate::parse(file).unwrap()
    }

    fn make_header() -> Header {
        Header {
            version: 2,
            width: 80,
            height: 24,
            timestamp: None,
            idle_time_limit: None,
            title: None,
            env: None,
        }
    }

    fn make_recording(events: Vec<Event>) -> Recording {
        Recording {
            header: make_header(),
            events,
            markers: vec![],
        }
    }

    fn make_output_event(time: f64, text: &str) -> Event {
        Event {
            time,
            event_type: EventType::Output,
            data: EventData::Text(text.into()),
        }
    }

    // -----------------------------------------------------------------------
    // Test 1: Empty recording (header only, no events)
    // -----------------------------------------------------------------------
    #[test]
    fn empty_recording_produces_empty_index() {
        let recording = load_test_recording("empty.cast");
        let raw_times: Vec<f64> = recording.events.iter().map(|e| e.time).collect();
        let time_map = TimeMap::build(&raw_times, None).unwrap();
        let index = KeyframeIndex::build(&recording, &time_map, KEYFRAME_INTERVAL);

        assert_eq!(index.len(), 0);
        assert!(index.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 2: Short recording (< 5s effective) — only initial keyframe
    // -----------------------------------------------------------------------
    #[test]
    fn short_recording_only_initial_keyframe() {
        let recording = load_test_recording("minimal_v2.cast");
        let raw_times: Vec<f64> = recording.events.iter().map(|e| e.time).collect();
        let time_map = TimeMap::build(&raw_times, None).unwrap();
        let index = KeyframeIndex::build(&recording, &time_map, KEYFRAME_INTERVAL);

        assert_eq!(
            index.len(),
            1,
            "short recording should have only 1 keyframe"
        );
        assert!(!index.is_empty());

        let kf = index.get(0).unwrap();
        assert_eq!(kf.time, 0.0);
        assert_eq!(kf.event_index, 0);

        // Snapshot should be an empty terminal (no events replayed yet)
        let restored = kf.snapshot.restore();
        let view = restored.view();
        for row in 0..24 {
            assert_eq!(
                view[row].text().trim(),
                "",
                "row {row} should be blank in initial keyframe"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Test 3: Multiple keyframe intervals (synthetic >15s)
    // -----------------------------------------------------------------------
    #[test]
    fn multiple_keyframe_intervals() {
        let events = vec![
            make_output_event(0.0, "t0\r\n"),
            make_output_event(3.0, "t3\r\n"),
            make_output_event(6.0, "t6\r\n"),
            make_output_event(9.0, "t9\r\n"),
            make_output_event(12.0, "t12\r\n"),
            make_output_event(15.0, "t15\r\n"),
        ];
        let raw_times: Vec<f64> = events.iter().map(|e| e.time).collect();
        let time_map = TimeMap::build(&raw_times, None).unwrap();
        let recording = make_recording(events);
        let index = KeyframeIndex::build(&recording, &time_map, KEYFRAME_INTERVAL);

        // KEYFRAME_INTERVAL = 5.0
        // Initial keyframe at t=0.0 (event_index=0)
        // Next at effective >= 5.0 → event at t=6.0, event_index=2
        // Next at effective >= 10.0 → event at t=12.0, event_index=4
        // Next at effective >= 15.0 → event at t=15.0, event_index=5
        assert_eq!(index.len(), 4, "expected 4 keyframes");

        let kf0 = index.get(0).unwrap();
        assert_eq!(kf0.time, 0.0);
        assert_eq!(kf0.event_index, 0);

        let kf1 = index.get(1).unwrap();
        assert_eq!(kf1.time, 6.0);
        assert_eq!(kf1.event_index, 2);

        let kf2 = index.get(2).unwrap();
        assert_eq!(kf2.time, 12.0);
        assert_eq!(kf2.event_index, 4);

        let kf3 = index.get(3).unwrap();
        assert_eq!(kf3.time, 15.0);
        assert_eq!(kf3.event_index, 5);
    }

    // -----------------------------------------------------------------------
    // Test 4: Idle-capped recording — keyframes align to effective time
    // -----------------------------------------------------------------------
    #[test]
    fn idle_capped_recording_keyframes_align_to_effective_time() {
        let recording = load_test_recording("long_idle.cast");
        // long_idle.cast has idle_time_limit: 2, raw events at ~[1, 1.1, 35, 35.1, 36]
        // Effective times with idle_limit=2: [1.0, 1.1, 3.1, 3.2, 4.1]
        // Effective duration is ~4.1s — less than one KEYFRAME_INTERVAL (5.0s)
        let raw_times: Vec<f64> = recording.events.iter().map(|e| e.time).collect();
        let time_map = TimeMap::build(&raw_times, recording.header.idle_time_limit).unwrap();
        let index = KeyframeIndex::build(&recording, &time_map, KEYFRAME_INTERVAL);

        // Only the initial keyframe should exist since effective duration < 5.0s
        assert_eq!(
            index.len(),
            1,
            "idle-capped recording with <5s effective duration should have only 1 keyframe"
        );
        let kf = index.get(0).unwrap();
        assert_eq!(kf.time, 0.0);
        assert_eq!(kf.event_index, 0);
    }

    // -----------------------------------------------------------------------
    // Test 5: Smoke test with all test data files
    // -----------------------------------------------------------------------
    #[test]
    fn smoke_test_all_test_data_files() {
        let files = [
            "minimal_v2.cast",
            "minimal_v3.cast",
            "empty.cast",
            "long_idle.cast",
            "with_markers.cast",
            "with_resize.cast",
            "alternate_buffer.cast",
            "real_session.cast",
        ];

        for name in &files {
            let recording = load_test_recording(name);
            let raw_times: Vec<f64> = recording.events.iter().map(|e| e.time).collect();
            let time_map = TimeMap::build(&raw_times, recording.header.idle_time_limit).unwrap();
            let index = KeyframeIndex::build(&recording, &time_map, KEYFRAME_INTERVAL);

            // Non-empty recordings should have at least 1 keyframe
            if !recording.events.is_empty() {
                assert!(
                    !index.is_empty(),
                    "{name}: non-empty recording should produce at least one keyframe"
                );
                // First keyframe should always be at time 0.0
                let kf0 = index.get(0).unwrap();
                assert_eq!(kf0.time, 0.0, "{name}: first keyframe should be at t=0");
                assert_eq!(
                    kf0.event_index, 0,
                    "{name}: first keyframe event_index should be 0"
                );
            } else {
                assert!(
                    index.is_empty(),
                    "{name}: empty recording should produce empty index"
                );
            }

            // Keyframe count should be reasonable (at least 1 per 5s of effective time)
            assert!(
                index.len() <= recording.events.len() + 1,
                "{name}: too many keyframes ({} keyframes for {} events)",
                index.len(),
                recording.events.len()
            );
        }
    }

    // -----------------------------------------------------------------------
    // Test 6: keyframe_at() lookup correctness
    // -----------------------------------------------------------------------
    #[test]
    fn keyframe_at_exact_keyframe_time() {
        let events = vec![
            make_output_event(0.0, "t0\r\n"),
            make_output_event(3.0, "t3\r\n"),
            make_output_event(6.0, "t6\r\n"),
            make_output_event(9.0, "t9\r\n"),
            make_output_event(12.0, "t12\r\n"),
            make_output_event(15.0, "t15\r\n"),
        ];
        let raw_times: Vec<f64> = events.iter().map(|e| e.time).collect();
        let time_map = TimeMap::build(&raw_times, None).unwrap();
        let recording = make_recording(events);
        let index = KeyframeIndex::build(&recording, &time_map, KEYFRAME_INTERVAL);

        // Keyframes are at: t=0.0 (idx 0), t=6.0 (idx 1), t=12.0 (idx 2), t=15.0 (idx 3)

        // Exact keyframe time → returns that keyframe's index
        assert_eq!(index.keyframe_at(0.0), Some(0));
        assert_eq!(index.keyframe_at(6.0), Some(1));
        assert_eq!(index.keyframe_at(12.0), Some(2));
        assert_eq!(index.keyframe_at(15.0), Some(3));
    }

    #[test]
    fn keyframe_at_between_keyframes() {
        let events = vec![
            make_output_event(0.0, "t0\r\n"),
            make_output_event(3.0, "t3\r\n"),
            make_output_event(6.0, "t6\r\n"),
            make_output_event(9.0, "t9\r\n"),
            make_output_event(12.0, "t12\r\n"),
            make_output_event(15.0, "t15\r\n"),
        ];
        let raw_times: Vec<f64> = events.iter().map(|e| e.time).collect();
        let time_map = TimeMap::build(&raw_times, None).unwrap();
        let recording = make_recording(events);
        let index = KeyframeIndex::build(&recording, &time_map, KEYFRAME_INTERVAL);

        // Time between two keyframes → returns the earlier keyframe's index
        assert_eq!(index.keyframe_at(3.0), Some(0));
        assert_eq!(index.keyframe_at(5.9), Some(0));
        assert_eq!(index.keyframe_at(7.5), Some(1));
        assert_eq!(index.keyframe_at(11.0), Some(1));
        assert_eq!(index.keyframe_at(14.0), Some(2));
    }

    #[test]
    fn keyframe_at_before_first_keyframe() {
        let events = vec![
            make_output_event(0.0, "t0\r\n"),
            make_output_event(6.0, "t6\r\n"),
        ];
        let raw_times: Vec<f64> = events.iter().map(|e| e.time).collect();
        let time_map = TimeMap::build(&raw_times, None).unwrap();
        let recording = make_recording(events);
        let index = KeyframeIndex::build(&recording, &time_map, KEYFRAME_INTERVAL);

        // Time before first keyframe → returns None
        // First keyframe is at t=0.0, so negative time returns None
        assert_eq!(index.keyframe_at(-1.0), None);
        assert_eq!(index.keyframe_at(-0.001), None);
    }

    #[test]
    fn keyframe_at_past_last_keyframe() {
        let events = vec![
            make_output_event(0.0, "t0\r\n"),
            make_output_event(3.0, "t3\r\n"),
            make_output_event(6.0, "t6\r\n"),
            make_output_event(9.0, "t9\r\n"),
            make_output_event(12.0, "t12\r\n"),
            make_output_event(15.0, "t15\r\n"),
        ];
        let raw_times: Vec<f64> = events.iter().map(|e| e.time).collect();
        let time_map = TimeMap::build(&raw_times, None).unwrap();
        let recording = make_recording(events);
        let index = KeyframeIndex::build(&recording, &time_map, KEYFRAME_INTERVAL);

        // Time at or past last keyframe → returns last keyframe's index
        assert_eq!(index.keyframe_at(15.0), Some(3));
        assert_eq!(index.keyframe_at(100.0), Some(3));
        assert_eq!(index.keyframe_at(999.0), Some(3));
    }

    #[test]
    fn keyframe_at_empty_index() {
        let recording = load_test_recording("empty.cast");
        let raw_times: Vec<f64> = recording.events.iter().map(|e| e.time).collect();
        let time_map = TimeMap::build(&raw_times, None).unwrap();
        let index = KeyframeIndex::build(&recording, &time_map, KEYFRAME_INTERVAL);

        // Empty index → returns None
        assert_eq!(index.keyframe_at(0.0), None);
        assert_eq!(index.keyframe_at(5.0), None);
        assert_eq!(index.keyframe_at(-1.0), None);
    }

    // -----------------------------------------------------------------------
    // Test 7: Keyframe count for real_session.cast matches expected formula
    // -----------------------------------------------------------------------
    #[test]
    fn real_session_keyframe_count_matches_formula() {
        let recording = load_test_recording("real_session.cast");
        let raw_times: Vec<f64> = recording.events.iter().map(|e| e.time).collect();
        let time_map = TimeMap::build(&raw_times, recording.header.idle_time_limit).unwrap();
        let interval = 5.0_f64;
        let index = KeyframeIndex::build(&recording, &time_map, interval);

        let duration = time_map.duration();
        // Expected: 1 initial keyframe at t=0 plus one for each full interval crossed.
        // Formula: floor(duration / interval) + 1
        let expected_count = (duration / interval).floor() as usize + 1;

        assert_eq!(
            index.len(),
            expected_count,
            "expected {expected_count} keyframes for duration={duration:.3}s with interval={interval}s, got {}",
            index.len()
        );
    }

    // -----------------------------------------------------------------------
    // Test 8: Insta snapshot test for keyframe placement
    // -----------------------------------------------------------------------
    #[test]
    fn snapshot_keyframe_placement() {
        let events = vec![
            make_output_event(0.0, "t0\r\n"),
            make_output_event(3.0, "t3\r\n"),
            make_output_event(6.0, "t6\r\n"),
            make_output_event(9.0, "t9\r\n"),
            make_output_event(12.0, "t12\r\n"),
            make_output_event(15.0, "t15\r\n"),
        ];
        let raw_times: Vec<f64> = events.iter().map(|e| e.time).collect();
        let time_map = TimeMap::build(&raw_times, None).unwrap();
        let recording = make_recording(events);
        let index = KeyframeIndex::build(&recording, &time_map, KEYFRAME_INTERVAL);

        let keyframe_tuples: Vec<(f64, usize)> = (0..index.len())
            .map(|i| {
                let kf = index.get(i).unwrap();
                (kf.time, kf.event_index)
            })
            .collect();

        insta::assert_debug_snapshot!(keyframe_tuples);
    }
}
