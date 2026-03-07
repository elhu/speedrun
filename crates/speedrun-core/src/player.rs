//! Playback controller and seek engine.
//!
//! The [`Player`] struct owns the parsed recording, time map, keyframe index,
//! and a virtual terminal. It exposes load, seek, and state-accessor methods
//! that form the foundation for all playback control.

use std::fmt;
use std::io::Read;

use crate::index::{KEYFRAME_INTERVAL, KeyframeIndex};
use crate::parser::{
    EventData, EventType, Marker, ParseError, ParseWarning, Recording, feed_event,
};
use crate::snapshot::{CursorState, create_vt};
use crate::timemap::{TimeMap, TimeMapError};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur when loading a recording into a [`Player`].
#[derive(Debug)]
pub enum PlayerError {
    /// The recording could not be parsed.
    Parse(ParseError),
    /// The time map could not be built (e.g. invalid idle limit).
    TimeMap(TimeMapError),
    /// The keyframe index could not be built.
    Index(String),
}

impl fmt::Display for PlayerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlayerError::Parse(e) => write!(f, "parse error: {e}"),
            PlayerError::TimeMap(e) => write!(f, "time map error: {e}"),
            PlayerError::Index(msg) => write!(f, "index error: {msg}"),
        }
    }
}

impl std::error::Error for PlayerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PlayerError::Parse(e) => Some(e),
            PlayerError::TimeMap(e) => Some(e),
            PlayerError::Index(_) => None,
        }
    }
}

impl From<ParseError> for PlayerError {
    fn from(e: ParseError) -> Self {
        PlayerError::Parse(e)
    }
}

impl From<TimeMapError> for PlayerError {
    fn from(e: TimeMapError) -> Self {
        PlayerError::TimeMap(e)
    }
}

// ---------------------------------------------------------------------------
// Load options
// ---------------------------------------------------------------------------

/// Options for loading a recording into a [`Player`].
pub struct LoadOptions {
    /// Cap idle time between events (seconds).
    /// `Some(limit)` overrides the header's `idle_time_limit`.
    /// `None` means use the header value, or no limit if the header
    /// doesn't specify one either.
    pub idle_limit: Option<f64>,
    /// Interval between keyframe snapshots in effective time (seconds).
    /// Lower values increase memory usage but make seeks faster.
    pub keyframe_interval: f64,
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self {
            idle_limit: None,
            keyframe_interval: KEYFRAME_INTERVAL,
        }
    }
}

// ---------------------------------------------------------------------------
// Player
// ---------------------------------------------------------------------------

/// Playback controller that owns a parsed recording and virtual terminal.
///
/// Provides load, seek, and state-accessor methods. The terminal state is
/// always consistent with `current_time` — seeking restores the nearest
/// keyframe and replays events forward.
pub struct Player {
    recording: Recording,
    time_map: TimeMap,
    index: KeyframeIndex,
    // avt::Vt does not implement Debug, so Player uses a manual Debug impl.
    vt: avt::Vt,
    /// Current position in effective time.
    current_time: f64,
    /// Index of the next event to process (into recording.events).
    current_event_index: usize,
    /// Markers with effective times (converted at load).
    markers: Vec<Marker>,
    /// Whether playback is active.
    playing: bool,
    /// Playback speed multiplier.
    speed: f64,
    /// Warnings produced during parsing.
    warnings: Vec<ParseWarning>,
}

impl fmt::Debug for Player {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Player")
            .field("current_time", &self.current_time)
            .field("current_event_index", &self.current_event_index)
            .field("playing", &self.playing)
            .field("speed", &self.speed)
            .field("markers", &self.markers)
            .finish_non_exhaustive()
    }
}

impl Player {
    /// Load a recording from a reader using default options.
    ///
    /// # Examples
    ///
    /// ```
    /// use speedrun_core::Player;
    ///
    /// let data = b"{\"version\":2,\"width\":80,\"height\":24}\n[0.5,\"o\",\"hello\"]\n[1.0,\"o\",\" world\"]";
    /// let mut player = Player::load(&data[..]).unwrap();
    ///
    /// assert_eq!(player.size(), (80, 24));
    /// assert!(player.duration() > 0.0);
    ///
    /// // Seek to a point and inspect screen content
    /// player.seek(1.0);
    /// let screen = player.screen();
    /// assert!(!screen.is_empty());
    /// ```
    pub fn load(reader: impl Read) -> Result<Self, PlayerError> {
        Self::load_with(reader, LoadOptions::default())
    }

    /// Load a recording from a reader with the given options.
    ///
    /// Idle limit resolution order: `opts.idle_limit` > header `idle_time_limit` > no limit.
    pub fn load_with(reader: impl Read, opts: LoadOptions) -> Result<Self, PlayerError> {
        let mut recording = crate::parse(reader)?;

        // Resolve idle limit: CLI override > header > None (no limit)
        let resolved_limit = opts.idle_limit.or(recording.header.idle_time_limit);

        // Extract raw timestamps
        let raw_times: Vec<f64> = recording.events.iter().map(|e| e.time).collect();

        // Build time map
        let time_map = TimeMap::build(&raw_times, resolved_limit)?;

        // Build keyframe index
        let index = KeyframeIndex::build(&recording, &time_map, opts.keyframe_interval)?;

        // Create initial virtual terminal
        let vt = create_vt(
            recording.header.width as usize,
            recording.header.height as usize,
        );

        // Convert markers to effective time by finding Marker events in the
        // events vec and looking up their effective time from the time map.
        let markers: Vec<Marker> = recording
            .events
            .iter()
            .enumerate()
            .filter(|(_, event)| event.event_type == EventType::Marker)
            .filter_map(|(i, event)| {
                let effective_time = time_map.effective_time(i)?;
                let label = match &event.data {
                    EventData::Text(s) => s.clone(),
                    EventData::Resize { .. } => return None,
                };
                Some(Marker {
                    time: effective_time,
                    label,
                })
            })
            .collect();

        let warnings = std::mem::take(&mut recording.warnings);

        Ok(Player {
            recording,
            time_map,
            index,
            vt,
            current_time: 0.0,
            current_event_index: 0,
            markers,
            playing: false,
            speed: 1.0,
            warnings,
        })
    }

    // -----------------------------------------------------------------------
    // Seek engine
    // -----------------------------------------------------------------------

    /// Internal: replay terminal state to the given effective time.
    ///
    /// Finds the nearest keyframe at or before `target_time`, restores it,
    /// then feeds events forward up to and including `target_time`.
    fn replay_to(&mut self, target_time: f64) {
        // Find the nearest keyframe at or before target_time
        match self.index.keyframe_at(target_time) {
            Some(kf_idx) => {
                if let Some(keyframe) = self.index.get(kf_idx) {
                    self.vt = keyframe.snapshot.restore();
                    self.current_event_index = keyframe.event_index;
                } else {
                    // Fallback: reset to initial state
                    self.vt = create_vt(
                        self.recording.header.width as usize,
                        self.recording.header.height as usize,
                    );
                    self.current_event_index = 0;
                }
            }
            None => {
                // Target is before first keyframe or index is empty
                self.vt = create_vt(
                    self.recording.header.width as usize,
                    self.recording.header.height as usize,
                );
                self.current_event_index = 0;
            }
        }

        // Feed events forward from current_event_index
        while self.current_event_index < self.recording.events.len() {
            let Some(t) = self.time_map.effective_time(self.current_event_index) else {
                break;
            };
            if t > target_time {
                break;
            }

            let event = &self.recording.events[self.current_event_index];
            feed_event(&mut self.vt, event);
            self.current_event_index += 1;
        }

        self.current_time = target_time;
    }

    /// Seek to an absolute effective time, clamped to `[0.0, duration]`.
    pub fn seek(&mut self, time: f64) {
        let clamped = time.clamp(0.0, self.duration());
        self.replay_to(clamped);
    }

    /// Seek relative to the current position by `delta` seconds.
    pub fn seek_relative(&mut self, delta: f64) {
        self.seek(self.current_time + delta);
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    /// Current terminal screen content.
    pub fn screen(&self) -> &[avt::Line] {
        self.vt.view()
    }

    /// Current cursor state.
    pub fn cursor(&self) -> CursorState {
        CursorState::from_vt(&self.vt)
    }

    /// Current terminal dimensions as `(cols, rows)`.
    pub fn size(&self) -> (u16, u16) {
        let (c, r) = self.vt.size();
        (c as u16, r as u16)
    }

    /// Current playback position in effective time (seconds).
    pub fn current_time(&self) -> f64 {
        self.current_time
    }

    /// Total effective duration of the recording (seconds).
    pub fn duration(&self) -> f64 {
        self.time_map.duration()
    }

    /// Markers with effective (not raw) timestamps.
    pub fn markers(&self) -> &[Marker] {
        &self.markers
    }

    /// Warnings produced during parsing.
    pub fn warnings(&self) -> &[ParseWarning] {
        &self.warnings
    }

    // -----------------------------------------------------------------------
    // Playback state control
    // -----------------------------------------------------------------------

    /// Start playback.
    pub fn play(&mut self) {
        self.playing = true;
    }

    /// Pause playback.
    pub fn pause(&mut self) {
        self.playing = false;
    }

    /// Toggle between playing and paused.
    pub fn toggle(&mut self) {
        self.playing = !self.playing;
    }

    /// Whether playback is currently active.
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Set the playback speed multiplier.
    ///
    /// Valid range: 0.25 to 4.0. Values outside the range are clamped.
    /// NaN and negative-infinite values reset to 1.0.
    /// Positive-infinite values are clamped to the maximum (4.0).
    pub fn set_speed(&mut self, speed: f64) {
        if speed.is_nan() || speed == f64::NEG_INFINITY {
            self.speed = 1.0;
        } else {
            self.speed = speed.clamp(0.25, 4.0);
        }
    }

    /// Current playback speed multiplier.
    pub fn speed(&self) -> f64 {
        self.speed
    }

    // -----------------------------------------------------------------------
    // Tick (continuous playback advancement)
    // -----------------------------------------------------------------------

    /// Advance playback by `dt` wall-clock seconds.
    ///
    /// Returns `true` if any terminal state changed (events were processed
    /// or playback auto-paused at the end).
    pub fn tick(&mut self, dt: f64) -> bool {
        if !self.playing {
            return false;
        }

        let new_time = (self.current_time + dt * self.speed).min(self.duration());
        let mut state_changed = false;

        // Feed events in the [current_time, new_time] window
        while self.current_event_index < self.recording.events.len() {
            let Some(t) = self.time_map.effective_time(self.current_event_index) else {
                break;
            };
            if t > new_time {
                break;
            }

            let event = &self.recording.events[self.current_event_index];
            if feed_event(&mut self.vt, event) {
                state_changed = true;
            }
            self.current_event_index += 1;
        }

        self.current_time = new_time;

        // Auto-pause at end
        if new_time >= self.duration() {
            self.playing = false;
            state_changed = true;
        }

        state_changed
    }

    // -----------------------------------------------------------------------
    // Time to next event (for TUI event loop sleep)
    // -----------------------------------------------------------------------

    /// Wall-clock duration until the next event, accounting for speed.
    ///
    /// Returns `None` when paused or at the end of the recording.
    /// Returns `Some(Duration::ZERO)` if an event is overdue.
    pub fn time_to_next_event(&self) -> Option<std::time::Duration> {
        if !self.playing {
            return None;
        }

        if self.current_event_index >= self.recording.events.len() {
            return None;
        }

        let next_effective_time = self.time_map.effective_time(self.current_event_index)?;
        let delta = (next_effective_time - self.current_time) / self.speed;

        if delta <= 0.0 {
            Some(std::time::Duration::ZERO)
        } else {
            Some(std::time::Duration::from_secs_f64(delta))
        }
    }

    // -----------------------------------------------------------------------
    // Search
    // -----------------------------------------------------------------------

    /// Find the next occurrence of `query` after `from_time`.
    /// Wraps around to the beginning if no match is found before the end.
    /// Returns `None` if the query is not found anywhere in the recording.
    ///
    /// Takes `&self` — search uses throwaway `Vt` instances and does not
    /// modify the player's current state.
    pub fn search_forward(&self, query: &str, from_time: f64) -> Option<crate::search::SearchHit> {
        crate::search::search_forward(
            &self.recording,
            &self.time_map,
            &self.index,
            query,
            from_time,
        )
    }

    /// Find the previous occurrence of `query` before `from_time`.
    /// Wraps around to the end if no match is found before the start.
    /// Returns `None` if the query is not found anywhere in the recording.
    ///
    /// Takes `&self` — search uses throwaway `Vt` instances and does not
    /// modify the player's current state.
    pub fn search_backward(&self, query: &str, from_time: f64) -> Option<crate::search::SearchHit> {
        crate::search::search_backward(
            &self.recording,
            &self.time_map,
            &self.index,
            query,
            from_time,
        )
    }

    // -----------------------------------------------------------------------
    // Single-event stepping
    // -----------------------------------------------------------------------

    /// Advance to the next output event (only when paused).
    ///
    /// Processes intervening resize events to keep terminal dimensions
    /// correct. Skips input and marker events.
    /// Returns `true` if an output event was found and processed.
    pub fn step_forward(&mut self) -> bool {
        if self.playing {
            return false;
        }

        let mut idx = self.current_event_index;
        while idx < self.recording.events.len() {
            let event = &self.recording.events[idx];
            let is_output = event.event_type == EventType::Output;
            feed_event(&mut self.vt, event);
            if is_output {
                if let Some(t) = self.time_map.effective_time(idx) {
                    self.current_time = t;
                }
                self.current_event_index = idx + 1;
                return true;
            }
            idx += 1;
        }

        false
    }

    /// Seek to the previous output event (only when paused).
    ///
    /// Internally calls `seek()` to rebuild terminal state from a keyframe,
    /// since the virtual terminal is forward-only.
    /// Returns `true` if a previous output event was found.
    pub fn step_backward(&mut self) -> bool {
        if self.playing {
            return false;
        }

        // Scan backward. The first Output event we find is the one currently
        // on screen — skip it. The second Output event is our target.
        let mut skipped_current = false;
        let mut idx = self.current_event_index;
        while idx > 0 {
            idx -= 1;
            if self.recording.events[idx].event_type == EventType::Output {
                if !skipped_current {
                    // This is the event currently displayed — skip it
                    skipped_current = true;
                    continue;
                }
                if let Some(t) = self.time_map.effective_time(idx) {
                    self.seek(t);
                    return true;
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn load_file(name: &str) -> Player {
        let file = std::fs::File::open(crate::testdata_path(name)).unwrap();
        Player::load(file).unwrap()
    }

    fn load_file_with(name: &str, opts: LoadOptions) -> Player {
        let file = std::fs::File::open(crate::testdata_path(name)).unwrap();
        Player::load_with(file, opts).unwrap()
    }

    const EPSILON: f64 = 1e-9;

    fn assert_f64_eq(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < EPSILON,
            "got {actual}, expected {expected}"
        );
    }

    // -----------------------------------------------------------------------
    // Load tests
    // -----------------------------------------------------------------------

    #[test]
    fn load_all_valid_test_files() {
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
            let file = std::fs::File::open(crate::testdata_path(name)).unwrap();
            Player::load(file).unwrap_or_else(|e| panic!("failed to load {name}: {e}"));
        }
    }

    #[test]
    fn initial_state() {
        let player = load_file("minimal_v2.cast");
        assert_f64_eq(player.current_time(), 0.0);
        assert_eq!(player.size(), (80, 24));
        assert!(!player.playing);
        assert_f64_eq(player.speed, 1.0);
    }

    // -----------------------------------------------------------------------
    // Idle limit resolution
    // -----------------------------------------------------------------------

    #[test]
    fn idle_limit_from_header() {
        // long_idle.cast has idle_time_limit: 2.0 in the header
        let player = load_file("long_idle.cast");
        // With idle limit 2.0, effective times: [1.0, 1.1, 3.1, 3.2, 4.1]
        assert_f64_eq(player.duration(), 4.1);
    }

    #[test]
    fn idle_limit_cli_override() {
        // Override the header's idle_time_limit (2.0) with 1.0
        let player = load_file_with(
            "long_idle.cast",
            LoadOptions {
                idle_limit: Some(1.0),
                ..LoadOptions::default()
            },
        );
        // With idle limit 1.0, effective times: [1.0, 1.1, 2.1, 2.2, 3.1]
        assert_f64_eq(player.duration(), 3.1);
    }

    #[test]
    fn idle_limit_no_limit() {
        // minimal_v2.cast has no idle_time_limit in header, no CLI override
        let player = load_file("minimal_v2.cast");
        // Raw times: [0.5, 1.2, 2.0, 2.1] — no capping
        assert_f64_eq(player.duration(), 2.1);
    }

    // -----------------------------------------------------------------------
    // Seek tests
    // -----------------------------------------------------------------------

    #[test]
    fn seek_clamps_to_bounds() {
        let mut player = load_file("minimal_v2.cast");

        player.seek(-10.0);
        assert_f64_eq(player.current_time(), 0.0);

        player.seek(9999.0);
        assert_f64_eq(player.current_time(), player.duration());
    }

    #[test]
    fn seek_to_specific_time() {
        let mut player = load_file("minimal_v2.cast");
        player.seek(1.0);
        assert_f64_eq(player.current_time(), 1.0);
    }

    #[test]
    fn seek_restores_terminal_state() {
        let mut player = load_file("minimal_v2.cast");
        // Seek to end — all output events should have been replayed
        player.seek(player.duration());

        let view = player.screen();
        // Verify there's content on screen (the test file has output events)
        let has_content = view.iter().any(|line| !line.text().trim().is_empty());
        assert!(
            has_content,
            "screen should have content after seeking to end"
        );
    }

    #[test]
    fn seek_to_zero_gives_empty_screen() {
        let mut player = load_file("minimal_v2.cast");
        // First seek to end to populate terminal
        player.seek(player.duration());
        // Then seek back to 0
        player.seek(0.0);
        assert_f64_eq(player.current_time(), 0.0);
    }

    // -----------------------------------------------------------------------
    // Seek relative
    // -----------------------------------------------------------------------

    #[test]
    fn seek_relative_forward() {
        let mut player = load_file("minimal_v2.cast");
        player.seek(0.5);
        player.seek_relative(0.5);
        assert_f64_eq(player.current_time(), 1.0);
    }

    #[test]
    fn seek_relative_backward() {
        let mut player = load_file("minimal_v2.cast");
        player.seek(1.5);
        player.seek_relative(-0.5);
        assert_f64_eq(player.current_time(), 1.0);
    }

    #[test]
    fn seek_relative_clamps() {
        let mut player = load_file("minimal_v2.cast");
        player.seek_relative(-100.0);
        assert_f64_eq(player.current_time(), 0.0);

        player.seek_relative(9999.0);
        assert_f64_eq(player.current_time(), player.duration());
    }

    // -----------------------------------------------------------------------
    // Accessor tests
    // -----------------------------------------------------------------------

    #[test]
    fn screen_returns_lines() {
        let player = load_file("minimal_v2.cast");
        let view = player.screen();
        // 80x24 terminal should have 24 lines
        assert_eq!(view.len(), 24);
    }

    #[test]
    fn cursor_returns_state() {
        let player = load_file("minimal_v2.cast");
        let cursor = player.cursor();
        // At time 0.0, cursor should be at origin
        assert_eq!(cursor.col, 0);
        assert_eq!(cursor.row, 0);
        assert!(cursor.visible);
    }

    #[test]
    fn size_returns_dimensions() {
        let player = load_file("minimal_v2.cast");
        assert_eq!(player.size(), (80, 24));
    }

    #[test]
    fn duration_and_current_time() {
        let player = load_file("minimal_v2.cast");
        assert_f64_eq(player.current_time(), 0.0);
        assert!(player.duration() > 0.0);
    }

    // -----------------------------------------------------------------------
    // Marker tests
    // -----------------------------------------------------------------------

    #[test]
    fn markers_have_effective_times() {
        let player = load_file("with_markers.cast");
        let markers = player.markers();
        assert_eq!(markers.len(), 2);
        assert_eq!(markers[0].label, "chapter-1");
        assert_eq!(markers[1].label, "chapter-2");
        // These should be effective times (no idle capping in this file)
        assert_f64_eq(markers[0].time, 3.0);
        assert_f64_eq(markers[1].time, 7.0);
    }

    #[test]
    fn markers_empty_when_no_markers() {
        let player = load_file("minimal_v2.cast");
        assert!(player.markers().is_empty());
    }

    // -----------------------------------------------------------------------
    // Error handling
    // -----------------------------------------------------------------------

    #[test]
    fn load_invalid_file_returns_parse_error() {
        let input = b"not valid json at all";
        let result = Player::load(Cursor::new(input));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, PlayerError::Parse(_)),
            "expected Parse error, got {err}"
        );
    }

    #[test]
    fn load_with_invalid_idle_limit_returns_timemap_error() {
        let input = r#"{"version": 2, "width": 80, "height": 24}
[0.5, "o", "hello"]
"#;
        let result = Player::load_with(
            Cursor::new(input),
            LoadOptions {
                idle_limit: Some(-1.0),
                ..LoadOptions::default()
            },
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, PlayerError::TimeMap(_)),
            "expected TimeMap error, got {err}"
        );
    }

    #[test]
    fn player_error_display() {
        let parse_err = PlayerError::Parse(ParseError::EmptyFile);
        assert!(parse_err.to_string().contains("parse error"));

        let tm_err = PlayerError::TimeMap(TimeMapError::InvalidIdleLimit(-1.0));
        assert!(tm_err.to_string().contains("time map error"));
    }

    // -----------------------------------------------------------------------
    // Resize handling
    // -----------------------------------------------------------------------

    #[test]
    fn seek_past_resize_changes_terminal_size() {
        let mut player = load_file("with_resize.cast");
        // with_resize.cast has a resize event at index 2: 120x40
        // Seek past it
        player.seek(player.duration());
        let (cols, rows) = player.size();
        assert_eq!(cols, 120);
        assert_eq!(rows, 40);
    }

    // -----------------------------------------------------------------------
    // Load error cases
    // -----------------------------------------------------------------------

    #[test]
    fn load_empty_bytes_returns_error() {
        let result = Player::load(Cursor::new(b""));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, PlayerError::Parse(_)),
            "expected Parse error for empty bytes, got {err}"
        );
    }

    #[test]
    fn load_binary_garbage_returns_error() {
        let garbage: Vec<u8> = (0..256).map(|i| (i % 256) as u8).collect();
        let result = Player::load(Cursor::new(garbage));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, PlayerError::Parse(_)),
            "expected Parse error for binary garbage, got {err}"
        );
    }

    #[test]
    fn player_warnings_accessor() {
        // Parse a file with malformed events → warnings should be surfaced via player.warnings()
        let input = r#"{"version": 2, "width": 80, "height": 24}
[1.0, "o", "valid"]
[2.0, "o"]
[3.0, "o", "also valid"]
"#;
        let player = Player::load(Cursor::new(input)).unwrap();
        assert_eq!(
            player.warnings().len(),
            1,
            "expected 1 warning, got {}",
            player.warnings().len()
        );
        assert!(
            player.warnings()[0].message.contains("3 elements"),
            "expected warning about 3 elements"
        );
    }

    // -----------------------------------------------------------------------
    // Seek tests — screen content verification
    // -----------------------------------------------------------------------

    #[test]
    fn seek_zero_cursor_at_origin_screen_blank() {
        let mut player = load_file("minimal_v2.cast");
        // Seek to end first, then back to 0
        player.seek(player.duration());
        player.seek(0.0);

        assert_f64_eq(player.current_time(), 0.0);

        let cursor = player.cursor();
        assert_eq!(cursor.col, 0);
        assert_eq!(cursor.row, 0);

        // Screen should be blank at time 0
        let view = player.screen();
        for line in view {
            assert!(
                line.text().trim().is_empty(),
                "expected blank screen at t=0, got: {:?}",
                line.text()
            );
        }
    }

    #[test]
    fn seek_to_last_event_shows_expected_output() {
        let mut player = load_file("minimal_v2.cast");
        // Last event at t=2.1 outputs "logout\r\n"
        // After all events: line 0 = "$ hello", line 1 = "world",
        // line 2 = "$ exit", line 3 = "logout"
        player.seek(player.duration());

        let view = player.screen();
        assert!(
            view[0].text().starts_with("$ hello"),
            "expected line 0 to start with '$ hello', got: {:?}",
            view[0].text()
        );
        assert!(
            view[1].text().starts_with("world"),
            "expected line 1 to start with 'world', got: {:?}",
            view[1].text()
        );
        assert!(
            view[2].text().starts_with("$ exit"),
            "expected line 2 to start with '$ exit', got: {:?}",
            view[2].text()
        );
        assert!(
            view[3].text().starts_with("logout"),
            "expected line 3 to start with 'logout', got: {:?}",
            view[3].text()
        );
    }

    #[test]
    fn seek_between_events_reflects_prior_state() {
        let mut player = load_file("minimal_v2.cast");
        // Events: [0.5, "$ hello\r\n"], [1.2, "world\r\n"], [2.0, "$ exit\r\n"], [2.1, "logout\r\n"]
        // Seek to 1.5 — should include events at 0.5 and 1.2 but NOT 2.0
        player.seek(1.5);

        let view = player.screen();
        assert!(
            view[0].text().starts_with("$ hello"),
            "expected '$ hello' on line 0 at t=1.5, got: {:?}",
            view[0].text()
        );
        assert!(
            view[1].text().starts_with("world"),
            "expected 'world' on line 1 at t=1.5, got: {:?}",
            view[1].text()
        );
        // Line 2 should be blank (event at 2.0 not yet processed)
        assert!(
            view[2].text().trim().is_empty(),
            "expected blank line 2 at t=1.5, got: {:?}",
            view[2].text()
        );
    }

    #[test]
    fn seek_forward_then_backward_consistency() {
        let mut player = load_file("minimal_v2.cast");

        // Seek to end, capture screen
        player.seek(player.duration());
        let end_screen: Vec<String> = player
            .screen()
            .iter()
            .map(|l| l.text().to_string())
            .collect();

        // Seek to middle
        player.seek(1.5);
        let mid_screen: Vec<String> = player
            .screen()
            .iter()
            .map(|l| l.text().to_string())
            .collect();

        // Seek back to end — should match original end screen
        player.seek(player.duration());
        let end_screen2: Vec<String> = player
            .screen()
            .iter()
            .map(|l| l.text().to_string())
            .collect();
        assert_eq!(
            end_screen, end_screen2,
            "seek forward→backward should produce same state"
        );

        // Seek back to middle — should match original mid screen
        player.seek(1.5);
        let mid_screen2: Vec<String> = player
            .screen()
            .iter()
            .map(|l| l.text().to_string())
            .collect();
        assert_eq!(
            mid_screen, mid_screen2,
            "seek backward should produce same state as first seek"
        );
    }

    // -----------------------------------------------------------------------
    // Tick tests
    // -----------------------------------------------------------------------

    #[test]
    fn tick_advances_time_and_feeds_events() {
        let mut player = load_file("minimal_v2.cast");
        player.play();

        // First event at effective time 0.5. Tick 0.6s to pass it.
        let changed = player.tick(0.6);
        assert!(changed, "tick should return true when events are processed");
        assert_f64_eq(player.current_time(), 0.6);

        // Screen should show output from the first event
        let view = player.screen();
        assert!(
            view[0].text().starts_with("$ hello"),
            "expected '$ hello' after tick past t=0.5, got: {:?}",
            view[0].text()
        );
    }

    #[test]
    fn tick_at_double_speed() {
        let mut player = load_file("minimal_v2.cast");
        player.play();
        player.set_speed(2.0);

        // Tick 0.3s wall-clock at 2× speed → 0.6s effective time
        player.tick(0.3);
        assert_f64_eq(player.current_time(), 0.6);
    }

    #[test]
    fn tick_when_paused_returns_false() {
        let mut player = load_file("minimal_v2.cast");
        // Player starts paused
        assert!(!player.is_playing());

        let before = player.current_time();
        let changed = player.tick(1.0);
        assert!(!changed, "tick when paused should return false");
        assert_f64_eq(player.current_time(), before);
    }

    #[test]
    fn tick_reaching_end_auto_pauses() {
        let mut player = load_file("minimal_v2.cast");
        player.play();

        // Tick past the entire duration
        let changed = player.tick(player.duration() + 10.0);
        assert!(changed, "tick reaching end should return true");
        assert!(!player.is_playing(), "player should auto-pause at end");
        assert_f64_eq(player.current_time(), player.duration());

        // Subsequent tick should return false
        let changed2 = player.tick(1.0);
        assert!(!changed2, "tick after auto-pause should return false");
    }

    // -----------------------------------------------------------------------
    // time_to_next_event tests
    // -----------------------------------------------------------------------

    #[test]
    fn time_to_next_event_at_start_playing() {
        let mut player = load_file("minimal_v2.cast");
        player.play();

        // At time 0.0, playing at 1×, first event at effective time 0.5
        let ttne = player.time_to_next_event();
        assert!(ttne.is_some(), "should return Some when playing");
        let dur = ttne.unwrap();
        assert!(
            (dur.as_secs_f64() - 0.5).abs() < EPSILON,
            "expected ~0.5s to next event, got {:?}",
            dur
        );
    }

    #[test]
    fn time_to_next_event_at_double_speed() {
        let mut player = load_file("minimal_v2.cast");
        player.play();
        player.set_speed(2.0);

        // At time 0.0, 2× speed, first event at 0.5 → wall-clock 0.25
        let ttne = player.time_to_next_event();
        assert!(ttne.is_some());
        let dur = ttne.unwrap();
        assert!(
            (dur.as_secs_f64() - 0.25).abs() < EPSILON,
            "expected ~0.25s at 2× speed, got {:?}",
            dur
        );
    }

    #[test]
    fn time_to_next_event_when_paused() {
        let player = load_file("minimal_v2.cast");
        // Player starts paused
        assert!(player.time_to_next_event().is_none());
    }

    #[test]
    fn time_to_next_event_idle_sleep_hint() {
        // Confirm that when playing with events remaining, time_to_next_event()
        // returns Some(non-zero duration). This verifies the event loop can sleep
        // rather than busy-loop, keeping CPU usage near zero during idle playback.
        let mut player = load_file("minimal_v2.cast");
        player.play();

        // At t=0.0 playing, first event is at t=0.5 — there is a non-trivial gap.
        let hint = player.time_to_next_event();
        assert!(
            hint.is_some(),
            "expected Some sleep hint when playing with events remaining"
        );
        let dur = hint.unwrap();
        assert!(
            dur > std::time::Duration::ZERO,
            "expected non-zero sleep duration, got {dur:?} — a zero duration would cause busy-looping"
        );
    }

    #[test]
    fn time_to_next_event_at_end() {
        let mut player = load_file("minimal_v2.cast");
        player.play();
        // Advance to end
        player.tick(player.duration() + 10.0);
        // Now at end and auto-paused
        assert!(player.time_to_next_event().is_none());
    }

    // -----------------------------------------------------------------------
    // Stepping tests
    // -----------------------------------------------------------------------

    #[test]
    fn step_forward_from_start_while_paused() {
        let mut player = load_file("minimal_v2.cast");
        assert!(!player.is_playing());

        let stepped = player.step_forward();
        assert!(
            stepped,
            "step_forward should return true when there are output events"
        );

        // Should advance to first output event at effective time 0.5
        assert_f64_eq(player.current_time(), 0.5);

        // Screen should show the first event's output
        let view = player.screen();
        assert!(
            view[0].text().starts_with("$ hello"),
            "expected '$ hello' after step_forward, got: {:?}",
            view[0].text()
        );
    }

    #[test]
    fn step_forward_skips_marker_events() {
        let mut player = load_file("with_markers.cast");
        assert!(!player.is_playing());

        // with_markers.cast events:
        // [0] t=0.5 output "Starting application...\r\n"
        // [1] t=1.0 output "Loading configuration\r\n"
        // [2] t=3.0 marker "chapter-1"
        // [3] t=3.5 output "Chapter 1: Basic setup\r\n"
        // ...

        // Step 1: first output event
        player.step_forward();
        assert_f64_eq(player.current_time(), 0.5);

        // Step 2: second output event
        player.step_forward();
        assert_f64_eq(player.current_time(), 1.0);

        // Step 3: should skip marker at 3.0, land on output at 3.5
        player.step_forward();
        assert_f64_eq(player.current_time(), 3.5);
    }

    #[test]
    fn step_forward_at_end_returns_false() {
        let mut player = load_file("minimal_v2.cast");

        // Seek to end
        player.seek(player.duration());

        let stepped = player.step_forward();
        assert!(!stepped, "step_forward at end should return false");
    }

    #[test]
    fn step_backward_from_mid_recording() {
        let mut player = load_file("minimal_v2.cast");

        // Seek to between events 1 (t=1.2) and 2 (t=2.0).
        // This positions current_event_index past events 0 and 1.
        player.seek(1.5);
        assert_f64_eq(player.current_time(), 1.5);

        let screen_before = player
            .screen()
            .iter()
            .map(|line| line.text().trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n");

        // step_backward should skip the currently displayed output event (index 1, t=1.2)
        // and seek to the previous output event (index 0, t=0.5).
        let stepped = player.step_backward();
        assert!(stepped, "step_backward should return true");
        assert_f64_eq(player.current_time(), 0.5);

        let screen_after = player
            .screen()
            .iter()
            .map(|line| line.text().trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert_ne!(
            screen_before, screen_after,
            "step_backward should change screen content"
        );
    }

    #[test]
    fn step_backward_at_start_returns_false() {
        let player = load_file("minimal_v2.cast");
        // current_event_index is 0, so step_backward should return false
        let mut player = player;
        let stepped = player.step_backward();
        assert!(!stepped, "step_backward at start should return false");
    }

    #[test]
    fn step_forward_while_playing_returns_false() {
        let mut player = load_file("minimal_v2.cast");
        player.play();
        assert!(player.is_playing());

        let stepped = player.step_forward();
        assert!(!stepped, "step_forward while playing should return false");
    }

    #[test]
    fn step_backward_while_playing_returns_false() {
        let mut player = load_file("minimal_v2.cast");
        player.play();
        assert!(player.is_playing());

        let stepped = player.step_backward();
        assert!(!stepped, "step_backward while playing should return false");
    }

    fn screen_text(player: &Player) -> String {
        player
            .screen()
            .iter()
            .map(|line| line.text().trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn step_forward_then_backward_round_trips() {
        let mut player = load_file("minimal_v2.cast");

        // Advance to event 0
        let stepped = player.step_forward();
        assert!(stepped, "first step_forward should succeed");
        let screen_after_first_step = screen_text(&player);

        // Advance to event 1
        let stepped = player.step_forward();
        assert!(stepped, "second step_forward should succeed");

        // Step backward should go back to event 0 screen content
        let stepped = player.step_backward();
        assert!(stepped, "step_backward should succeed");
        let screen_after_backward = screen_text(&player);

        assert_eq!(
            screen_after_first_step, screen_after_backward,
            "step forward then backward should round-trip to the same screen content"
        );
    }

    #[test]
    fn step_backward_repeatedly_walks_back() {
        let mut player = load_file("minimal_v2.cast");

        // Seek to the end
        player.seek(player.duration());

        let mut times = Vec::new();
        for _ in 0..3 {
            let stepped = player.step_backward();
            assert!(stepped, "step_backward should succeed");
            times.push(player.current_time());
        }

        // Each time should be strictly less than the previous
        assert!(
            times[0] > times[1],
            "first step_backward time {} should be greater than second {}",
            times[0],
            times[1]
        );
        assert!(
            times[1] > times[2],
            "second step_backward time {} should be greater than third {}",
            times[1],
            times[2]
        );
    }

    #[test]
    fn step_backward_after_tick_changes_screen() {
        let mut player = load_file("minimal_v2.cast");

        player.play();
        player.tick(2.0);
        player.pause();

        let screen_before = screen_text(&player);

        let stepped = player.step_backward();
        assert!(stepped, "step_backward should succeed after tick");

        let screen_after = screen_text(&player);
        assert_ne!(
            screen_before, screen_after,
            "step_backward after tick should change screen content"
        );
    }

    #[test]
    fn step_backward_with_one_event_processed_returns_false() {
        let mut player = load_file("minimal_v2.cast");

        // Advance to event 0 only
        let stepped = player.step_forward();
        assert!(stepped, "step_forward should succeed");

        // Only one output event has been processed; no earlier one to step to
        let stepped = player.step_backward();
        assert!(
            !stepped,
            "step_backward with one event processed should return false"
        );
    }

    // -----------------------------------------------------------------------
    // set_speed() clamping tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_set_speed_clamp_low() {
        let mut player = load_file("minimal_v2.cast");
        player.set_speed(0.0);
        assert_f64_eq(player.speed(), 0.25);
    }

    #[test]
    fn test_set_speed_clamp_high() {
        let mut player = load_file("minimal_v2.cast");
        player.set_speed(10.0);
        assert_f64_eq(player.speed(), 4.0);
    }

    #[test]
    fn test_set_speed_clamp_negative() {
        let mut player = load_file("minimal_v2.cast");
        player.set_speed(-1.0);
        assert_f64_eq(player.speed(), 0.25);
    }

    #[test]
    fn test_set_speed_nan() {
        let mut player = load_file("minimal_v2.cast");
        player.set_speed(f64::NAN);
        assert_f64_eq(player.speed(), 1.0);
    }

    #[test]
    fn test_set_speed_infinity() {
        let mut player = load_file("minimal_v2.cast");
        player.set_speed(f64::INFINITY);
        assert_f64_eq(player.speed(), 4.0);
    }

    #[test]
    fn test_set_speed_neg_infinity() {
        let mut player = load_file("minimal_v2.cast");
        player.set_speed(f64::NEG_INFINITY);
        assert_f64_eq(player.speed(), 1.0);
    }

    #[test]
    fn test_set_speed_in_range() {
        let mut player = load_file("minimal_v2.cast");
        player.set_speed(2.0);
        assert_f64_eq(player.speed(), 2.0);
    }

    #[test]
    fn test_set_speed_boundary_low() {
        let mut player = load_file("minimal_v2.cast");
        player.set_speed(0.25);
        assert_f64_eq(player.speed(), 0.25);
    }

    #[test]
    fn test_set_speed_boundary_high() {
        let mut player = load_file("minimal_v2.cast");
        player.set_speed(4.0);
        assert_f64_eq(player.speed(), 4.0);
    }

    // -----------------------------------------------------------------------
    // Insta snapshot test — full pipeline
    // -----------------------------------------------------------------------

    #[test]
    fn snapshot_screen_at_end_of_minimal_v2() {
        let mut player = load_file("minimal_v2.cast");
        player.seek(player.duration());

        // Collect all non-empty screen lines as text
        let screen_text: Vec<String> = player
            .screen()
            .iter()
            .map(|line| line.text().trim_end().to_string())
            .filter(|text| !text.is_empty())
            .collect();

        // This locks down the full pipeline: parse → time map → index → seek → render
        insta::assert_debug_snapshot!(screen_text);
    }
}
