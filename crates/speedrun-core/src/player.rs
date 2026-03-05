//! Playback controller and seek engine.
//!
//! The [`Player`] struct owns the parsed recording, time map, keyframe index,
//! and a virtual terminal. It exposes load, seek, and state-accessor methods
//! that form the foundation for all playback control.

use std::fmt;
use std::io::Read;

use crate::index::KeyframeIndex;
use crate::parser::{EventData, EventType, Marker, ParseError, Recording};
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
}

impl fmt::Display for PlayerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlayerError::Parse(e) => write!(f, "parse error: {e}"),
            PlayerError::TimeMap(e) => write!(f, "time map error: {e}"),
        }
    }
}

impl std::error::Error for PlayerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PlayerError::Parse(e) => Some(e),
            PlayerError::TimeMap(e) => Some(e),
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
#[derive(Default)]
pub struct LoadOptions {
    /// Cap idle time between events (seconds).
    /// `Some(limit)` overrides the header's `idle_time_limit`.
    /// `None` means use the header value, or no limit if the header
    /// doesn't specify one either.
    pub idle_limit: Option<f64>,
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
    pub fn load(reader: impl Read) -> Result<Self, PlayerError> {
        Self::load_with(reader, LoadOptions::default())
    }

    /// Load a recording from a reader with the given options.
    ///
    /// Idle limit resolution order: `opts.idle_limit` > header `idle_time_limit` > no limit.
    pub fn load_with(reader: impl Read, opts: LoadOptions) -> Result<Self, PlayerError> {
        let recording = crate::parse(reader)?;

        // Resolve idle limit: CLI override > header > None (no limit)
        let resolved_limit = opts.idle_limit.or(recording.header.idle_time_limit);

        // Extract raw timestamps
        let raw_times: Vec<f64> = recording.events.iter().map(|e| e.time).collect();

        // Build time map
        let time_map = TimeMap::build(&raw_times, resolved_limit)?;

        // Build keyframe index
        let index = KeyframeIndex::build(&recording, &time_map);

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
                let keyframe = self.index.get(kf_idx).expect("keyframe index in bounds");
                self.vt = keyframe.snapshot.restore();
                self.current_event_index = keyframe.event_index;
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
            match (&event.event_type, &event.data) {
                (EventType::Output, EventData::Text(data)) => {
                    let _ = self.vt.feed_str(data);
                }
                (EventType::Resize, EventData::Resize { cols, rows }) => {
                    // xtwinops: rows;cols order (opposite of EventData field order)
                    let _ = self.vt.feed_str(&format!("\x1b[8;{rows};{cols}t"));
                }
                // Input and Marker events don't affect terminal state
                (EventType::Input, _) | (EventType::Marker, _) => {}
                // Ignore mismatched type/data combinations
                _ => {}
            }

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
    pub fn set_speed(&mut self, speed: f64) {
        self.speed = speed;
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
            match (&event.event_type, &event.data) {
                (EventType::Output, EventData::Text(data)) => {
                    let _ = self.vt.feed_str(data);
                    state_changed = true;
                }
                (EventType::Resize, EventData::Resize { cols, rows }) => {
                    let _ = self.vt.feed_str(&format!("\x1b[8;{rows};{cols}t"));
                    state_changed = true;
                }
                // Input and Marker events don't affect terminal state
                (EventType::Input, _) | (EventType::Marker, _) => {}
                _ => {}
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
            match (&event.event_type, &event.data) {
                (EventType::Output, EventData::Text(data)) => {
                    let _ = self.vt.feed_str(data);
                    if let Some(t) = self.time_map.effective_time(idx) {
                        self.current_time = t;
                    }
                    self.current_event_index = idx + 1;
                    return true;
                }
                (EventType::Resize, EventData::Resize { cols, rows }) => {
                    let _ = self.vt.feed_str(&format!("\x1b[8;{rows};{cols}t"));
                }
                // Input / Marker: skip
                _ => {}
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

        // Guard against underflow
        if self.current_event_index == 0 {
            return false;
        }

        // Scan backward from current_event_index - 1
        let mut idx = self.current_event_index - 1;
        loop {
            if self.recording.events[idx].event_type == EventType::Output
                && let Some(t) = self.time_map.effective_time(idx)
            {
                self.seek(t);
                return true;
            }
            if idx == 0 {
                break;
            }
            idx -= 1;
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn testdata_path(name: &str) -> std::path::PathBuf {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.push("../../testdata");
        p.push(name);
        p
    }

    fn load_file(name: &str) -> Player {
        let file = std::fs::File::open(testdata_path(name)).unwrap();
        Player::load(file).unwrap()
    }

    fn load_file_with(name: &str, opts: LoadOptions) -> Player {
        let file = std::fs::File::open(testdata_path(name)).unwrap();
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
            let file = std::fs::File::open(testdata_path(name)).unwrap();
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
}
