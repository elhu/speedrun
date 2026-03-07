use crate::help::HelpOverlay;
use crate::input::{Action, map_key_event};
use crate::ui::{ControlsBar, TerminalView, ViewportState, find_on_screen_matches};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use speedrun_core::Player;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

type Tui = Terminal<CrosstermBackend<std::io::Stdout>>;

const SPEED_STEPS: &[f64] = &[0.25, 0.5, 1.0, 1.5, 2.0, 4.0];

/// Find the next speed step in the given direction.
///
/// `direction = 1` (up): returns the smallest step strictly greater than `current`.
/// `direction = -1` (down): returns the largest step strictly less than `current`.
/// If no such step exists, clamps to the boundary (max or min).
fn next_speed(current: f64, direction: i8) -> f64 {
    const EPSILON: f64 = 1e-9;

    if direction > 0 {
        // Find smallest step strictly greater than current
        SPEED_STEPS
            .iter()
            .copied()
            .find(|&s| s > current + EPSILON)
            .unwrap_or(*SPEED_STEPS.last().unwrap())
    } else {
        // Find largest step strictly less than current
        SPEED_STEPS
            .iter()
            .rev()
            .copied()
            .find(|&s| s < current - EPSILON)
            .unwrap_or(*SPEED_STEPS.first().unwrap())
    }
}

/// Find the first marker whose time falls in the half-open interval (prev_time, new_time].
///
/// Returns the time of the earliest such marker, or `None` if no marker falls in that
/// interval. The lower bound is exclusive to prevent re-triggering on resume after
/// auto-pause at a marker. The upper bound is inclusive so that a marker exactly at
/// `new_time` is detected.
fn find_crossed_marker(
    markers: &[speedrun_core::Marker],
    prev_time: f64,
    new_time: f64,
) -> Option<f64> {
    markers
        .iter()
        .filter(|m| m.time > prev_time && m.time <= new_time)
        .map(|m| m.time)
        .reduce(f64::min) // first marker if multiple
}

/// Modal input mode for the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    /// All keys go through `map_key_event`.
    Normal,
    /// Keys are captured for the search query text field.
    SearchInput,
    /// Confirmation prompt for file modification.
    ConfirmFileModify,
}

/// Tracks the pending marker action type for the confirmation dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingMarkerAction {
    None,
    Unlabeled,
}

pub struct App {
    pub player: Player,
    /// Whether the user has manually hidden the controls bar via Tab.
    /// When true, controls are hidden regardless of other state.
    pub controls_manually_hidden: bool,
    /// Whether controls should be force-shown (e.g., just interacted, paused, at end).
    /// Overrides the auto-hide timeout.
    pub controls_force_show: bool,
    /// Timestamp of the last user interaction (keypress).
    pub last_interaction: Instant,
    pub should_quit: bool,
    /// Whether the help overlay is currently visible.
    pub help_visible: bool,
    /// Whether playback was active before help was shown (for resume on dismiss).
    was_playing_before_help: bool,
    viewport: ViewportState,
    /// Current input mode (Normal or SearchInput).
    pub input_mode: InputMode,
    /// Buffer for text being typed in the search bar.
    pub search_input: String,
    /// Committed search query (set on Enter).
    pub search_query: Option<String>,
    /// Index of the current match for n/N navigation.
    pub current_match_index: Option<usize>,
    /// Transient "No matches" feedback message, with expiry time.
    pub no_match_feedback: Option<Instant>,
    /// Whether to auto-pause when a marker boundary is crossed during tick().
    pub pause_at_markers: bool,
    /// Path to the source .cast file (None for stdin).
    pub file_path: Option<PathBuf>,
    /// Whether the user has confirmed that file modification is OK.
    pub file_modify_confirmed: bool,
    /// The effective time captured when the user pressed `m`.
    pending_marker_time: Option<f64>,
    /// What kind of marker action is pending (unlabeled or none).
    pending_marker_action: PendingMarkerAction,
    /// Whether playback was active before the marker dialog paused it.
    was_playing_before_marker: bool,
    /// Instant when a marker was successfully created (for transient feedback).
    pub marker_feedback: Option<Instant>,
    /// Transient error message for marker operations.
    pub marker_error_feedback: Option<(String, Instant)>,
}

impl App {
    pub fn new(
        player: Player,
        show_controls: bool,
        pause_at_markers: bool,
        file_path: Option<PathBuf>,
    ) -> Self {
        // If show_controls is false (--no-controls), start manually hidden.
        // Otherwise, force-show for the initial 2s window.
        Self {
            player,
            controls_manually_hidden: !show_controls,
            controls_force_show: show_controls,
            last_interaction: Instant::now(),
            should_quit: false,
            help_visible: false,
            was_playing_before_help: false,
            viewport: ViewportState::default(),
            input_mode: InputMode::Normal,
            search_input: String::new(),
            search_query: None,
            current_match_index: None,
            no_match_feedback: None,
            pause_at_markers,
            file_path,
            file_modify_confirmed: false,
            pending_marker_time: None,
            pending_marker_action: PendingMarkerAction::None,
            was_playing_before_marker: false,
            marker_feedback: None,
            marker_error_feedback: None,
        }
    }

    /// Returns true if the controls bar should currently be visible.
    pub fn controls_visible(&self) -> bool {
        if self.controls_manually_hidden {
            return false;
        }
        if self.controls_force_show {
            return true;
        }
        // Auto-hide during playback after 2s of no interaction.
        if self.player.is_playing()
            && Instant::now().duration_since(self.last_interaction) > Duration::from_secs(2)
        {
            return false;
        }
        true
    }

    /// Adjust viewport scroll_y to center the given row vertically.
    ///
    /// This is called after n/N navigation to ensure the match row is visible
    /// when the recording is taller than the host terminal.
    fn scroll_to_match_row(&mut self, match_row: u16) {
        let (_, rec_rows) = self.player.size();
        // We don't have the current terminal height here, but we can estimate
        // a reasonable viewport height. The viewport will be corrected on next
        // render by follow_cursor, but we set scroll_y here for the match row.
        // Use a conservative estimate — the viewport state will be refined on render.
        let max_scroll = rec_rows.saturating_sub(1);
        // Center the match row — try to put it in the middle of screen
        // We'll use scroll_y directly; follow_cursor will refine on next render
        let half_screen = 12u16; // reasonable estimate for half screen
        self.viewport.scroll_y = match_row.saturating_sub(half_screen).min(max_scroll);
    }

    fn show_help(&mut self) {
        self.was_playing_before_help = self.player.is_playing();
        self.player.pause();
        self.help_visible = true;
        // Show controls underneath the overlay (paused state, per controls rules)
        self.controls_force_show = true;
        self.controls_manually_hidden = false;
    }

    fn dismiss_help(&mut self) {
        self.help_visible = false;
        if self.was_playing_before_help {
            self.player.play();
            // Resume auto-hide: clear force_show so the 2s interaction timer takes over.
            // last_interaction is already fresh from the keypress that dismissed help.
            self.controls_force_show = false;
        }
        // If was paused, keep controls_force_show = true (already set by show_help)
    }

    /// Handle key events while in ConfirmFileModify mode.
    fn handle_confirm_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') => {
                self.file_modify_confirmed = true;
                self.input_mode = InputMode::Normal;
                match self.pending_marker_action {
                    PendingMarkerAction::Unlabeled => self.execute_marker_creation(),
                    PendingMarkerAction::None => {}
                }
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
                self.cancel_pending_marker();
            }
            KeyCode::Char('q') => {
                self.should_quit = true;
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            _ => {} // ignore other keys
        }
    }

    /// Create the marker and append it to the file.
    fn execute_marker_creation(&mut self) {
        if let Some(line) = self.player.add_marker(String::new()) {
            self.append_marker_to_file(&line);
        }
        self.controls_force_show = true;
        self.controls_manually_hidden = false;
        if self.was_playing_before_marker {
            self.player.play();
            self.controls_force_show = false;
        }
        self.pending_marker_action = PendingMarkerAction::None;
        self.pending_marker_time = None;
    }

    /// Cancel the pending marker action and resume playback if needed.
    fn cancel_pending_marker(&mut self) {
        if self.was_playing_before_marker {
            self.player.play();
            self.controls_force_show = false;
        }
        self.pending_marker_action = PendingMarkerAction::None;
        self.pending_marker_time = None;
    }

    /// Append a marker line to the .cast file.
    fn append_marker_to_file(&mut self, line: &str) {
        let Some(ref path) = self.file_path else {
            return;
        };
        match self.append_line_to_file(path, line) {
            Ok(()) => {
                self.marker_feedback = Some(Instant::now());
            }
            Err(e) => {
                self.marker_error_feedback =
                    Some((format!("Failed to write marker: {e}"), Instant::now()));
            }
        }
    }

    /// Append a line to a file, ensuring proper newline handling.
    fn append_line_to_file(&self, path: &Path, line: &str) -> std::io::Result<()> {
        use std::fs::OpenOptions;
        use std::io::{Read, Seek, SeekFrom, Write};
        let mut file = OpenOptions::new().read(true).append(true).open(path)?;
        // Check if file ends with newline
        let needs_newline = if file.seek(SeekFrom::End(-1)).is_ok() {
            let mut byte = [0u8; 1];
            file.read_exact(&mut byte)?;
            byte[0] != b'\n'
        } else {
            false // empty file
        };
        if needs_newline {
            writeln!(file, "\n{line}")
        } else {
            writeln!(file, "{line}")
        }
    }

    pub fn run(&mut self, terminal: &mut Tui) -> std::io::Result<()> {
        let mut last_tick = Instant::now();
        let mut needs_redraw = true; // render first frame immediately
        let mut last_controls_visible = self.controls_visible();

        loop {
            // Render (conditional)
            if needs_redraw {
                terminal.draw(|frame| self.render(frame))?;
                needs_redraw = false;
                last_controls_visible = self.controls_visible();
            }

            // Compute timeout
            let timeout = self.compute_timeout();

            // Poll for input
            if event::poll(timeout)? {
                match event::read() {
                    Ok(Event::Key(key)) => {
                        self.handle_key(key);
                        needs_redraw = true;
                    }
                    Ok(Event::Resize(_, _)) => {
                        needs_redraw = true;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        eprintln!("crossterm read error: {e}");
                    }
                }
            }

            // Advance playback
            let now = Instant::now();
            let dt = now.duration_since(last_tick).as_secs_f64();
            last_tick = now;

            if self.player.is_playing() {
                let prev_time = self.player.current_time(); // capture before tick
                self.player.tick(dt);
                // Auto-pause at markers when --pause-at-markers is active.
                if self.pause_at_markers
                    && let Some(marker_time) = find_crossed_marker(
                        self.player.markers(),
                        prev_time,
                        self.player.current_time(),
                    )
                {
                    self.player.pause();
                    // seek() restores exact terminal state at marker time, since
                    // tick() may have processed events slightly past the marker.
                    self.player.seek(marker_time);
                    self.controls_force_show = true;
                    self.controls_manually_hidden = false;
                }
                // Always redraw while playing so time display and progress bar
                // update smoothly (~30fps), not just when events fire.
                needs_redraw = true;
            }

            // Trigger re-render if controls visibility changed (e.g., auto-hide fired)
            let current_controls_visible = self.controls_visible();
            if current_controls_visible != last_controls_visible {
                needs_redraw = true;
                last_controls_visible = current_controls_visible;
            }

            if self.should_quit {
                break;
            }
        }
        Ok(())
    }

    /// Compute the poll timeout based on time to next playback event.
    ///
    /// Falls back to 100ms when paused or at the end of the recording.
    /// When controls are visible during playback, also accounts for the
    /// auto-hide deadline to ensure a timely re-render.
    pub fn compute_timeout(&self) -> Duration {
        let playback = self
            .player
            .time_to_next_event()
            .unwrap_or(Duration::from_millis(100));

        let base = if self.player.is_playing() && self.controls_visible() {
            let elapsed = self.last_interaction.elapsed();
            let hide_deadline = Duration::from_secs(2).saturating_sub(elapsed);
            playback.min(hide_deadline.max(Duration::from_millis(10)))
        } else {
            playback
        };

        if self.player.is_playing() {
            base.min(Duration::from_millis(33))
        } else {
            base
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        // Ignore key release events (crossterm 0.27+ sends both press and release)
        if key.kind != KeyEventKind::Press {
            return;
        }

        self.last_interaction = Instant::now();

        match self.input_mode {
            InputMode::SearchInput => self.handle_search_input(key),
            InputMode::ConfirmFileModify => self.handle_confirm_input(key),
            InputMode::Normal => {
                if let Some(action) = map_key_event(key) {
                    self.handle_action(action);
                }
            }
        }
    }

    /// Handle key events while in SearchInput mode.
    fn handle_search_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                // Cancel search input, return to Normal mode, keep previous results
                self.input_mode = InputMode::Normal;
                self.search_input.clear();
            }
            KeyCode::Enter => {
                // Execute search
                let query = self.search_input.drain(..).collect::<String>();
                if query.is_empty() {
                    // Empty query: cancel
                    self.input_mode = InputMode::Normal;
                } else {
                    self.input_mode = InputMode::Normal;
                    self.search_query = Some(query.clone());
                    self.current_match_index = None;
                    // Seek to first match from current time
                    let current = self.player.current_time();
                    if self.player.search_forward(&query, current).is_none() {
                        // No matches at all
                        self.no_match_feedback = Some(Instant::now());
                    }
                }
            }
            KeyCode::Backspace => {
                self.search_input.pop();
            }
            KeyCode::Char(c) => {
                self.search_input.push(c);
            }
            _ => {} // Ignore other keys (arrows, etc.)
        }
    }

    fn handle_action(&mut self, action: Action) {
        // While help is visible, only allow toggle, quit, and escape (all dismiss help)
        if self.help_visible {
            match action {
                Action::ToggleHelp | Action::Quit | Action::Escape => {
                    self.dismiss_help();
                }
                _ => {} // ignore all other actions (including StartSearch)
            }
            return;
        }

        match action {
            Action::Quit => {
                self.should_quit = true;
            }
            Action::Escape => {
                // Esc chain: search input > help overlay > search active > quit
                // (search input is handled in handle_search_input, help handled above)
                if self.search_query.is_some() {
                    // Clear search state
                    self.search_query = None;
                    self.current_match_index = None;
                    self.no_match_feedback = None;
                } else {
                    self.should_quit = true;
                }
            }
            Action::StartSearch => {
                self.input_mode = InputMode::SearchInput;
                self.search_input.clear();
            }
            Action::NextMatch => {
                if let Some(ref query) = self.search_query.clone() {
                    let current = self.player.current_time();
                    let epsilon = 0.001;
                    if let Some(hit) = self.player.search_forward(query, current + epsilon) {
                        self.player.seek(hit.time);
                        // Scroll viewport to center the match row
                        self.scroll_to_match_row(hit.row as u16);
                    } else {
                        self.no_match_feedback = Some(Instant::now());
                    }
                }
                // No-op if search_query is None
            }
            Action::PrevMatch => {
                if let Some(ref query) = self.search_query.clone() {
                    let current = self.player.current_time();
                    let epsilon = 0.001;
                    if let Some(hit) = self.player.search_backward(query, current - epsilon) {
                        self.player.seek(hit.time);
                        // Scroll viewport to center the match row
                        self.scroll_to_match_row(hit.row as u16);
                    } else {
                        self.no_match_feedback = Some(Instant::now());
                    }
                }
                // No-op if search_query is None
            }
            Action::TogglePlayback => {
                // If paused at the end of a non-empty recording, seek to beginning
                // so Space restarts playback instead of immediately auto-pausing again.
                // The !is_playing() guard prevents seeking when pressing Space to pause.
                if !self.player.is_playing()
                    && self.player.current_time() >= self.player.duration()
                    && self.player.duration() > 0.0
                {
                    self.player.seek(0.0);
                }
                self.player.toggle();
                if self.player.is_playing() {
                    // Resumed: let auto-hide take over
                    self.controls_force_show = false;
                } else {
                    // Paused: show controls
                    self.controls_force_show = true;
                    self.controls_manually_hidden = false;
                }
            }
            Action::SeekForward => {
                self.player.seek_relative(5.0);
                self.controls_force_show = true;
                self.controls_manually_hidden = false;
                self.last_interaction = Instant::now();
            }
            Action::SeekBackward => {
                self.player.seek_relative(-5.0);
                self.controls_force_show = true;
                self.controls_manually_hidden = false;
                self.last_interaction = Instant::now();
            }
            Action::SeekForward30s => {
                self.player.seek_relative(30.0);
                self.controls_force_show = true;
                self.controls_manually_hidden = false;
                self.last_interaction = Instant::now();
            }
            Action::SeekBackward30s => {
                self.player.seek_relative(-30.0);
                self.controls_force_show = true;
                self.controls_manually_hidden = false;
                self.last_interaction = Instant::now();
            }
            Action::StepForward => {
                self.player.step_forward();
                self.controls_force_show = true;
                self.controls_manually_hidden = false;
                self.last_interaction = Instant::now();
            }
            Action::StepBackward => {
                self.player.step_backward();
                self.controls_force_show = true;
                self.controls_manually_hidden = false;
                self.last_interaction = Instant::now();
            }
            Action::SpeedUp => {
                let new_speed = next_speed(self.player.speed(), 1);
                self.player.set_speed(new_speed);
                self.controls_force_show = true;
                self.controls_manually_hidden = false;
                self.last_interaction = Instant::now();
            }
            Action::SpeedDown => {
                let new_speed = next_speed(self.player.speed(), -1);
                self.player.set_speed(new_speed);
                self.controls_force_show = true;
                self.controls_manually_hidden = false;
                self.last_interaction = Instant::now();
            }
            Action::NextMarker => {
                let current = self.player.current_time();
                if let Some(marker) = self
                    .player
                    .markers()
                    .iter()
                    .find(|m| m.time > current + 0.001)
                {
                    self.player.seek(marker.time);
                } else {
                    // No marker after current position: fall through to end of recording
                    self.player.seek(self.player.duration());
                    if self.player.is_playing() {
                        self.player.pause();
                    }
                }
                self.controls_force_show = true;
                self.controls_manually_hidden = false;
                self.last_interaction = Instant::now();
            }
            Action::PrevMarker => {
                let current = self.player.current_time();
                if let Some(marker) = self
                    .player
                    .markers()
                    .iter()
                    .rev()
                    .find(|m| m.time < current - 0.001)
                {
                    self.player.seek(marker.time);
                } else {
                    // No marker before current position: fall through to start of recording
                    self.player.seek(0.0);
                }
                self.controls_force_show = true;
                self.controls_manually_hidden = false;
                self.last_interaction = Instant::now();
            }
            Action::JumpToPercent(n) => {
                let target = self.player.duration() * (n as f64) / 10.0;
                self.player.seek(target);
                self.controls_force_show = true;
                self.controls_manually_hidden = false;
                self.last_interaction = Instant::now();
            }
            Action::JumpToStart => {
                self.player.seek(0.0);
                self.controls_force_show = true;
                self.controls_manually_hidden = false;
                self.last_interaction = Instant::now();
            }
            Action::JumpToEnd => {
                self.player.seek(self.player.duration());
                if self.player.is_playing() {
                    self.player.pause();
                }
                self.controls_force_show = true;
                self.controls_manually_hidden = false;
                self.last_interaction = Instant::now();
            }
            Action::ToggleControls => {
                self.controls_manually_hidden = !self.controls_manually_hidden;
                if self.controls_manually_hidden {
                    self.controls_force_show = false;
                }
            }
            Action::ToggleHelp => {
                self.show_help();
            }
            Action::AddMarker => {
                // No file path → transient error
                if self.file_path.is_none() {
                    self.marker_error_feedback = Some((
                        "Cannot add marker: reading from stdin".into(),
                        Instant::now(),
                    ));
                    return;
                }
                // V3 recordings not supported
                if self.player.version() == 3 {
                    self.marker_error_feedback = Some((
                        "Cannot add marker: v3 recordings not supported".into(),
                        Instant::now(),
                    ));
                    return;
                }
                // Capture time and pause
                self.pending_marker_time = Some(self.player.current_time());
                self.was_playing_before_marker = self.player.is_playing();
                self.player.pause();
                self.pending_marker_action = PendingMarkerAction::Unlabeled;
                // Need confirmation?
                if !self.file_modify_confirmed {
                    self.input_mode = InputMode::ConfirmFileModify;
                } else {
                    self.execute_marker_creation();
                }
            }
        }
    }

    /// Calculate the Rect for the controls bar based on host and recording dimensions.
    ///
    /// - When host is taller than recording: place controls immediately below recording.
    /// - When host is same height or shorter: overlay the controls on the bottom row.
    pub fn controls_rect(area: Rect, rec_cols: u16, rec_rows: u16) -> Rect {
        if area.height > rec_rows {
            // Host terminal taller than recording: place below recording content
            Rect {
                x: area.x,
                y: area.y + rec_rows,
                width: rec_cols.min(area.width),
                height: 1,
            }
        } else {
            // Host terminal same height or shorter: overlay bottom row
            Rect {
                x: area.x,
                y: area.y + area.height.saturating_sub(1),
                width: area.width,
                height: 1,
            }
        }
    }

    fn render(&mut self, frame: &mut Frame) {
        let area = frame.size();

        let cursor = self.player.cursor();
        let (rec_cols, rec_rows) = self.player.size();

        self.viewport.follow_cursor(
            cursor.col as u16,
            cursor.row as u16,
            rec_cols,
            rec_rows,
            area.width,
            area.height,
        );

        // Compute on-screen matches for highlighting
        let (screen_matches, current_match_idx) = if let Some(ref query) = self.search_query {
            let lines: Vec<String> = self
                .player
                .screen()
                .iter()
                .map(|line| line.text())
                .collect();
            let matches = find_on_screen_matches(&lines, query);
            (matches, self.current_match_index)
        } else {
            (Vec::new(), None)
        };

        let view = TerminalView::new(self.player.screen(), cursor, (rec_cols, rec_rows))
            .with_scroll(self.viewport.scroll_x, self.viewport.scroll_y)
            .with_matches(screen_matches, current_match_idx);

        frame.render_widget(view, area);

        // Conditionally render controls bar
        if self.controls_visible() {
            let is_at_end =
                !self.player.is_playing() && self.player.current_time() >= self.player.duration();
            let controls = ControlsBar {
                is_playing: self.player.is_playing(),
                is_at_end,
                current_time: self.player.current_time(),
                duration: self.player.duration(),
                speed: self.player.speed(),
                marker_times: self.player.markers().iter().map(|m| m.time).collect(),
                search_query: self.search_query.clone(),
            };
            let controls_rect = Self::controls_rect(area, rec_cols, rec_rows);
            frame.render_widget(controls, controls_rect);
        }

        // Render search input bar when in SearchInput mode
        if self.input_mode == InputMode::SearchInput {
            let search_bar_rect = Rect {
                x: area.x,
                y: area.y + area.height.saturating_sub(1),
                width: area.width,
                height: 1,
            };
            let available = search_bar_rect.width.saturating_sub(3) as usize; // "/ " prefix + cursor
            let display_text = if self.search_input.len() > available {
                &self.search_input[self.search_input.len() - available..]
            } else {
                &self.search_input
            };
            let line = Line::from(vec![
                Span::styled("/ ", Style::default().fg(Color::Yellow)),
                Span::raw(display_text),
                Span::styled("█", Style::default().fg(Color::White)),
            ]);
            let bar =
                Paragraph::new(line).style(Style::default().bg(Color::DarkGray).fg(Color::White));
            frame.render_widget(bar, search_bar_rect);
        }

        // Render confirmation dialog for file modification
        if self.input_mode == InputMode::ConfirmFileModify {
            let bar_rect = Rect {
                x: area.x,
                y: area.y + area.height.saturating_sub(1),
                width: area.width,
                height: 1,
            };
            let filename = self
                .file_path
                .as_ref()
                .map(|p| {
                    p.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string()
                })
                .unwrap_or_default();
            let line = Line::from(vec![
                Span::styled(
                    format!("  Append marker to {filename}? "),
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw("[y/N] "),
            ]);
            let bar =
                Paragraph::new(line).style(Style::default().bg(Color::DarkGray).fg(Color::White));
            frame.render_widget(bar, bar_rect);
        }

        // Render marker error feedback
        if let Some((ref msg, when)) = self.marker_error_feedback {
            if when.elapsed() < Duration::from_secs(2) {
                let feedback_rect = Rect {
                    x: area.x,
                    y: area.y + area.height.saturating_sub(1),
                    width: area.width,
                    height: 1,
                };
                let bar = Paragraph::new(format!("  {msg}"))
                    .style(Style::default().bg(Color::DarkGray).fg(Color::Red));
                frame.render_widget(bar, feedback_rect);
            } else {
                self.marker_error_feedback = None;
            }
        }

        // Render "No matches" feedback
        if let Some(when) = self.no_match_feedback {
            if when.elapsed() < Duration::from_secs(2) {
                let feedback_rect = Rect {
                    x: area.x,
                    y: area.y + area.height.saturating_sub(1),
                    width: area.width,
                    height: 1,
                };
                let bar = Paragraph::new("  No matches")
                    .style(Style::default().bg(Color::DarkGray).fg(Color::Red));
                frame.render_widget(bar, feedback_rect);
            } else {
                self.no_match_feedback = None;
            }
        }

        // Render help overlay on top of everything when visible
        if self.help_visible {
            frame.render_widget(HelpOverlay, area);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn testdata_path(name: &str) -> std::path::PathBuf {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.push("../../testdata");
        p.push(name);
        p
    }

    fn load_player(name: &str) -> speedrun_core::Player {
        let file = std::fs::File::open(testdata_path(name)).unwrap();
        speedrun_core::Player::load(file).unwrap()
    }

    // ── Render path integration test ─────────────────────────────────────────

    #[test]
    fn render_path_shows_terminal_content() {
        // Load the recording and seek to the end (known screen state)
        let mut player = load_player("minimal_v2.cast");
        player.seek(player.duration());

        // Get recording dimensions for the backend
        let (cols, rows) = player.size();

        // Construct the App
        let mut app = App::new(player, true, false, None);

        // Create a TestBackend terminal sized to the recording dimensions
        let backend = TestBackend::new(cols, rows);
        let mut terminal = Terminal::new(backend).unwrap();

        // Render via terminal.draw() - exercises the full chain:
        // player screen state → viewport follow_cursor → TerminalView → buffer
        terminal.draw(|f| app.render(f)).unwrap();

        // Verify row 0 starts with "$ hello"
        let buf = terminal.backend().buffer();
        let cell_text: String = (0..7).map(|x| buf.get(x, 0).symbol().to_string()).collect();
        assert_eq!(cell_text, "$ hello");
    }

    // ── compute_timeout tests ─────────────────────────────────────────────────

    #[test]
    fn compute_timeout_paused_returns_100ms() {
        // Load a file, don't call play() — player starts paused
        let player = load_player("minimal_v2.cast");
        let app = App::new(player, true, false, None);

        // When paused, time_to_next_event() returns None → fallback 100ms
        assert_eq!(app.compute_timeout(), Duration::from_millis(100));
    }

    #[test]
    fn compute_timeout_playing_returns_event_driven_duration() {
        // Load a file and start playing
        let mut player = load_player("minimal_v2.cast");
        player.play();

        // At t=0 playing at 1×, first event is at 0.5s → time_to_next_event ≈ 0.5s
        let expected = player.time_to_next_event().unwrap();
        let app = App::new(player, true, false, None);

        // Controls are visible initially (force_show=true), so compute_timeout
        // returns min(playback_timeout, hide_deadline). Since last_interaction is
        // just set, hide_deadline ≈ 2s, so playback timeout wins.
        assert!(app.compute_timeout() <= expected);
    }

    #[test]
    fn compute_timeout_at_end_returns_100ms() {
        // Load a file, play, tick past the end — player auto-pauses
        let mut player = load_player("minimal_v2.cast");
        player.play();
        // Advance past the entire duration
        player.tick(player.duration() + 10.0);
        // Player should now be auto-paused
        assert!(!player.is_playing());

        let app = App::new(player, true, false, None);
        // time_to_next_event() returns None when paused → fallback 100ms
        assert_eq!(app.compute_timeout(), Duration::from_millis(100));
    }

    #[test]
    fn compute_timeout_capped_while_playing() {
        // Load a file and start playing; time_to_next_event returns ~0.5s (500ms).
        // With the 33ms cap applied while playing, timeout must be <= 33ms.
        let mut player = load_player("minimal_v2.cast");
        player.play();
        let app = App::new(player, true, false, None);
        assert!(
            app.compute_timeout() <= Duration::from_millis(33),
            "compute_timeout while playing should be capped at 33ms, got {:?}",
            app.compute_timeout()
        );
    }

    #[test]
    fn compute_timeout_uncapped_while_paused() {
        // Load a file without playing; time_to_next_event returns None → fallback 100ms.
        // When paused, the 33ms cap is NOT applied, so result should be > 33ms.
        let player = load_player("minimal_v2.cast");
        let app = App::new(player, true, false, None);
        assert!(
            app.compute_timeout() > Duration::from_millis(33),
            "compute_timeout while paused should exceed 33ms, got {:?}",
            app.compute_timeout()
        );
    }

    #[test]
    fn space_at_end_restarts() {
        // Seek to end so player is auto-paused at duration, then press Space.
        // Expect: player is_playing() and current_time() == 0.0.
        let mut player = load_player("minimal_v2.cast");
        player.play();
        player.tick(player.duration() + 10.0);
        assert!(!player.is_playing()); // auto-paused at end
        let duration = player.duration();
        assert!(player.current_time() >= duration);

        let mut app = App::new(player, false, false, None);
        app.handle_action(Action::TogglePlayback);

        assert!(
            app.player.is_playing(),
            "player should be playing after Space at end"
        );
        assert!(
            (app.player.current_time() - 0.0).abs() < 1e-9,
            "current_time should be 0.0 after restart, got {}",
            app.player.current_time()
        );
    }

    #[test]
    fn space_on_empty_recording() {
        // On a zero-duration recording, pressing Space should toggle without seeking or panicking.
        let player = load_player("empty.cast");
        assert_eq!(player.duration(), 0.0);
        let mut app = App::new(player, false, false, None);

        // Should not panic
        app.handle_action(Action::TogglePlayback);
        // Player should have toggled (started playing, then may auto-pause — just no panic)
        // The main assertion is no panic; we also check no seek-to-0 infinite loop behavior.
    }

    // ── next_speed unit tests ─────────────────────────────────────────────────

    #[test]
    fn next_speed_up_from_1x() {
        assert!((next_speed(1.0, 1) - 1.5).abs() < 1e-9);
    }

    #[test]
    fn next_speed_down_from_1x() {
        assert!((next_speed(1.0, -1) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn next_speed_up_clamps_at_max() {
        assert!((next_speed(4.0, 1) - 4.0).abs() < 1e-9);
    }

    #[test]
    fn next_speed_down_clamps_at_min() {
        assert!((next_speed(0.25, -1) - 0.25).abs() < 1e-9);
    }

    #[test]
    fn next_speed_up_snaps_from_non_step() {
        // 0.75 is between 0.5 and 1.0, snapping up → 1.0
        assert!((next_speed(0.75, 1) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn next_speed_down_snaps_from_non_step() {
        // 0.75 is between 0.5 and 1.0, snapping down → 0.5
        assert!((next_speed(0.75, -1) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn next_speed_up_from_min() {
        assert!((next_speed(0.25, 1) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn next_speed_down_from_max() {
        assert!((next_speed(4.0, -1) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn next_speed_full_cycle_up() {
        let expected = [0.25, 0.5, 1.0, 1.5, 2.0, 4.0, 4.0];
        let mut current = 0.25;
        for &exp in &expected[1..] {
            current = next_speed(current, 1);
            assert!(
                (current - exp).abs() < 1e-9,
                "expected {exp}, got {current}"
            );
        }
    }

    #[test]
    fn next_speed_full_cycle_down() {
        let expected = [4.0, 2.0, 1.5, 1.0, 0.5, 0.25, 0.25];
        let mut current = 4.0;
        for &exp in &expected[1..] {
            current = next_speed(current, -1);
            assert!(
                (current - exp).abs() < 1e-9,
                "expected {exp}, got {current}"
            );
        }
    }

    #[test]
    fn speed_up_action_changes_player_speed() {
        let mut player = load_player("minimal_v2.cast");
        // Ensure starting speed is 1.0
        player.set_speed(1.0);
        let mut app = App::new(player, false, false, None);

        app.handle_action(Action::SpeedUp);
        assert!((app.player.speed() - 1.5).abs() < 1e-9);
        assert!(app.controls_force_show);
    }

    #[test]
    fn speed_down_action_changes_player_speed() {
        let mut player = load_player("minimal_v2.cast");
        player.set_speed(1.0);
        let mut app = App::new(player, false, false, None);

        app.handle_action(Action::SpeedDown);
        assert!((app.player.speed() - 0.5).abs() < 1e-9);
        assert!(app.controls_force_show);
    }

    // ── 30s seek tests ───────────────────────────────────────────────────────

    #[test]
    fn seek_forward_30s_from_start() {
        // minimal_v2.cast has duration 2.1s — seeking 30s forward should clamp to duration
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, false, false, None);

        app.handle_action(Action::SeekForward30s);
        assert!((app.player.current_time() - app.player.duration()).abs() < 1e-9);
        assert!(app.controls_force_show);
    }

    #[test]
    fn seek_backward_30s_clamps_to_zero() {
        let mut player = load_player("minimal_v2.cast");
        player.seek(1.5);
        let mut app = App::new(player, false, false, None);

        app.handle_action(Action::SeekBackward30s);
        assert!((app.player.current_time() - 0.0).abs() < 1e-9);
        assert!(app.controls_force_show);
    }

    // ── Stepping tests ───────────────────────────────────────────────────────

    #[test]
    fn step_forward_while_paused_advances() {
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, false, false, None);
        assert!(!app.player.is_playing());

        app.handle_action(Action::StepForward);
        // First output event in minimal_v2.cast is at 0.5
        assert!((app.player.current_time() - 0.5).abs() < 1e-9);
        assert!(app.controls_force_show);
    }

    #[test]
    fn step_backward_while_paused_goes_back() {
        let mut player = load_player("minimal_v2.cast");
        player.seek(1.5); // between events at 1.2 and 2.0
        let mut app = App::new(player, false, false, None);
        assert!(!app.player.is_playing());

        // With the fix, step_backward skips the currently displayed event (index 1, t=1.2)
        // and seeks to the previous output event (index 0, t=0.5).
        app.handle_action(Action::StepBackward);
        assert!((app.player.current_time() - 0.5).abs() < 1e-9);
        assert!(app.controls_force_show);
    }

    #[test]
    fn step_forward_while_playing_does_nothing() {
        let mut player = load_player("minimal_v2.cast");
        player.play();
        let mut app = App::new(player, false, false, None);
        assert!(app.player.is_playing());

        app.handle_action(Action::StepForward);
        // Should remain at 0.0 since step_forward guards against playing
        assert!((app.player.current_time() - 0.0).abs() < 1e-9);
    }

    // ── Marker navigation tests ──────────────────────────────────────────────

    #[test]
    fn next_marker_from_start() {
        // with_markers.cast has markers at 3.0 and 7.0
        let player = load_player("with_markers.cast");
        let mut app = App::new(player, false, false, None);

        app.handle_action(Action::NextMarker);
        assert!((app.player.current_time() - 3.0).abs() < 1e-9);
        assert!(app.controls_force_show);
    }

    #[test]
    fn next_marker_from_first_marker() {
        let mut player = load_player("with_markers.cast");
        player.seek(3.0);
        let mut app = App::new(player, false, false, None);

        app.handle_action(Action::NextMarker);
        assert!((app.player.current_time() - 7.0).abs() < 1e-9);
    }

    #[test]
    fn next_marker_past_last_seeks_to_end() {
        let mut player = load_player("with_markers.cast");
        player.seek(7.0);
        let mut app = App::new(player, false, false, None);
        let duration = app.player.duration();

        app.handle_action(Action::NextMarker);
        // Should seek to end of recording when no marker exists after current position
        assert!((app.player.current_time() - duration).abs() < 1e-9);
    }

    #[test]
    fn prev_marker_from_last_marker() {
        let mut player = load_player("with_markers.cast");
        player.seek(7.0);
        let mut app = App::new(player, false, false, None);

        app.handle_action(Action::PrevMarker);
        assert!((app.player.current_time() - 3.0).abs() < 1e-9);
        assert!(app.controls_force_show);
    }

    #[test]
    fn prev_marker_before_first_seeks_to_start() {
        let mut player = load_player("with_markers.cast");
        player.seek(3.0);
        let mut app = App::new(player, false, false, None);

        app.handle_action(Action::PrevMarker);
        // Should seek to start of recording when no marker exists before current position
        assert!((app.player.current_time() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn prev_marker_from_mid_recording() {
        let mut player = load_player("with_markers.cast");
        player.seek(5.0); // between markers at 3.0 and 7.0
        let mut app = App::new(player, false, false, None);

        app.handle_action(Action::PrevMarker);
        assert!((app.player.current_time() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn next_marker_no_markers_seeks_to_end() {
        // minimal_v2.cast has no markers
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, false, false, None);
        let duration = app.player.duration();

        app.handle_action(Action::NextMarker);
        // Should seek to end of recording when there are no markers
        assert!((app.player.current_time() - duration).abs() < 1e-9);
    }

    #[test]
    fn prev_marker_no_markers_seeks_to_start() {
        // minimal_v2.cast has no markers; PrevMarker from mid → seeks to start
        let mut player = load_player("minimal_v2.cast");
        player.seek(1.0);
        let mut app = App::new(player, false, false, None);

        app.handle_action(Action::PrevMarker);
        // No markers in recording → seeks to start (0.0)
        assert!((app.player.current_time() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn test_space_resumes_after_auto_pause() {
        // Load with_markers.cast, create App with pause_at_markers: true.
        // Seek to a marker time (paused state), press Space. Assert is_playing().
        let mut player = load_player("with_markers.cast");
        player.seek(3.0); // seek to first marker
        // Player is paused after seek
        assert!(!player.is_playing());

        let mut app = App::new(player, false, true, None); // pause_at_markers = true

        // Press Space to resume playback
        app.handle_action(Action::TogglePlayback);
        assert!(
            app.player.is_playing(),
            "player should be playing after Space at marker"
        );
    }

    #[test]
    fn test_no_retrigger_at_same_marker() {
        // Create two markers at 3.0 and 7.0. Call find_crossed_marker with
        // prev_time=3.0, new_time=4.0. Assert None (marker at 3.0 not re-triggered
        // since 3.0 is not strictly greater than prev_time=3.0).
        let markers = vec![
            speedrun_core::Marker {
                time: 3.0,
                label: String::new(),
            },
            speedrun_core::Marker {
                time: 7.0,
                label: String::new(),
            },
        ];

        // prev_time=3.0, new_time=4.0: marker at 3.0 has time <= 3.0 (not > 3.0), so None
        let result = find_crossed_marker(&markers, 3.0, 4.0);
        assert!(
            result.is_none(),
            "marker at 3.0 should not re-trigger when prev_time=3.0"
        );

        // Sanity check: marker at 7.0 is detected when crossing from 6.0 to 7.0
        let result2 = find_crossed_marker(&markers, 6.0, 7.0);
        assert!(
            result2.is_some(),
            "marker at 7.0 should be detected when crossing (6.0, 7.0]"
        );
        assert!((result2.unwrap() - 7.0).abs() < 1e-9);
    }

    // ── Percentage jump tests ────────────────────────────────────────────────

    #[test]
    fn jump_to_percent_0() {
        let mut player = load_player("with_markers.cast");
        player.seek(4.0);
        let mut app = App::new(player, false, false, None);

        app.handle_action(Action::JumpToPercent(0));
        assert!((app.player.current_time() - 0.0).abs() < 1e-9);
        assert!(app.controls_force_show);
    }

    #[test]
    fn jump_to_percent_5() {
        // with_markers.cast has duration 8.0, so 50% = 4.0
        let player = load_player("with_markers.cast");
        let mut app = App::new(player, false, false, None);
        let duration = app.player.duration();

        app.handle_action(Action::JumpToPercent(5));
        let expected = duration * 0.5;
        assert!(
            (app.player.current_time() - expected).abs() < 1e-9,
            "expected {expected}, got {}",
            app.player.current_time()
        );
    }

    #[test]
    fn jump_to_percent_9() {
        // with_markers.cast has duration 8.0, so 90% = 7.2
        let player = load_player("with_markers.cast");
        let mut app = App::new(player, false, false, None);
        let duration = app.player.duration();

        app.handle_action(Action::JumpToPercent(9));
        let expected = duration * 0.9;
        assert!(
            (app.player.current_time() - expected).abs() < 1e-9,
            "expected {expected}, got {}",
            app.player.current_time()
        );
    }

    // ── Jump to start/end tests ──────────────────────────────────────────────

    #[test]
    fn jump_to_start() {
        let mut player = load_player("minimal_v2.cast");
        player.seek(1.5);
        let mut app = App::new(player, false, false, None);

        app.handle_action(Action::JumpToStart);
        assert!((app.player.current_time() - 0.0).abs() < 1e-9);
        assert!(app.controls_force_show);
    }

    #[test]
    fn jump_to_end_pauses_player() {
        let mut player = load_player("minimal_v2.cast");
        player.play();
        let mut app = App::new(player, false, false, None);
        assert!(app.player.is_playing());

        app.handle_action(Action::JumpToEnd);
        assert!(
            (app.player.current_time() - app.player.duration()).abs() < 1e-9,
            "expected current_time == duration"
        );
        assert!(!app.player.is_playing(), "player should be paused at end");
        assert!(app.controls_force_show);
    }

    #[test]
    fn jump_to_end_already_paused() {
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, false, false, None);
        assert!(!app.player.is_playing());

        app.handle_action(Action::JumpToEnd);
        assert!((app.player.current_time() - app.player.duration()).abs() < 1e-9);
        assert!(!app.player.is_playing());
    }

    // ── Edge case: empty recording ───────────────────────────────────────────

    #[test]
    fn navigation_on_empty_recording_no_panic() {
        let player = load_player("empty.cast");
        let mut app = App::new(player, false, false, None);

        // None of these should panic
        app.handle_action(Action::SeekForward30s);
        app.handle_action(Action::SeekBackward30s);
        app.handle_action(Action::StepForward);
        app.handle_action(Action::StepBackward);
        app.handle_action(Action::NextMarker);
        app.handle_action(Action::PrevMarker);
        app.handle_action(Action::JumpToPercent(5));
        app.handle_action(Action::JumpToStart);
        app.handle_action(Action::JumpToEnd);
    }

    // ── Visibility state machine tests ────────────────────────────────────────

    #[test]
    fn controls_visible_initial_show_controls_true() {
        let player = load_player("minimal_v2.cast");
        let app = App::new(player, true, false, None);
        assert!(app.controls_visible());
    }

    #[test]
    fn controls_visible_initial_no_controls() {
        let player = load_player("minimal_v2.cast");
        let app = App::new(player, false, false, None);
        assert!(!app.controls_visible());
    }

    #[test]
    fn controls_auto_hide_when_playing_and_idle() {
        let mut player = load_player("minimal_v2.cast");
        player.play();
        let mut app = App::new(player, true, false, None);
        // Simulate 3 seconds of no interaction
        app.last_interaction = Instant::now() - Duration::from_secs(3);
        // force_show must be false for auto-hide to kick in
        app.controls_force_show = false;
        assert!(!app.controls_visible());
    }

    #[test]
    fn controls_visible_within_2s_window() {
        let mut player = load_player("minimal_v2.cast");
        player.play();
        let mut app = App::new(player, true, false, None);
        // Simulate 1 second of no interaction (within 2s window)
        app.last_interaction = Instant::now() - Duration::from_secs(1);
        app.controls_force_show = false;
        assert!(app.controls_visible());
    }

    #[test]
    fn controls_visible_after_seek_forward() {
        let mut player = load_player("minimal_v2.cast");
        player.play();
        let mut app = App::new(player, true, false, None);
        // Simulate old interaction time
        app.last_interaction = Instant::now() - Duration::from_secs(10);
        app.controls_force_show = false;

        app.handle_action(Action::SeekForward);

        assert!(app.controls_visible());
        // last_interaction should be recent
        assert!(app.last_interaction.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn controls_visible_when_paused() {
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, true, false, None);
        // Simulate old interaction, but player is paused
        app.last_interaction = Instant::now() - Duration::from_secs(10);
        app.controls_force_show = true; // force_show is set when paused
        // Player is already paused by default (not playing)
        assert!(app.controls_visible());
    }

    #[test]
    fn controls_tab_toggle_hides_and_shows() {
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, true, false, None);
        assert!(app.controls_visible());

        // Tab hides
        app.handle_action(Action::ToggleControls);
        assert!(!app.controls_visible());

        // Tab shows again
        app.handle_action(Action::ToggleControls);
        assert!(app.controls_visible());
    }

    #[test]
    fn controls_manual_hide_persists_over_recent_interaction() {
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, true, false, None);
        // Tab to hide
        app.handle_action(Action::ToggleControls);
        // Even with recent interaction, manually hidden stays hidden
        app.last_interaction = Instant::now();
        assert!(!app.controls_visible());
    }

    #[test]
    fn controls_manual_hide_cleared_by_seek() {
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, true, false, None);
        // Tab to hide
        app.handle_action(Action::ToggleControls);
        assert!(!app.controls_visible());

        // Seek clears manual hide
        app.handle_action(Action::SeekForward);
        assert!(app.controls_visible());
    }

    #[test]
    fn controls_visible_at_end_force_shown() {
        let mut player = load_player("minimal_v2.cast");
        player.play();
        // Advance to end
        player.tick(player.duration() + 10.0);
        assert!(!player.is_playing()); // auto-paused

        let mut app = App::new(player, true, false, None);
        // On auto-pause at end, force_show should be true; simulate the state
        // that would be set when TogglePlayback is called during pause detection.
        // Here we directly set force_show to test the is_at_end case.
        app.controls_force_show = true;
        assert!(app.controls_visible());
    }

    // ── Overlay positioning tests ─────────────────────────────────────────────

    #[test]
    fn controls_rect_below_recording_when_host_taller() {
        // Host 80x30, recording 80x24 → controls at y=24
        let area = Rect::new(0, 0, 80, 30);
        let rect = App::controls_rect(area, 80, 24);
        assert_eq!(rect.y, 24);
        assert_eq!(rect.height, 1);
        assert_eq!(rect.width, 80);
    }

    #[test]
    fn controls_rect_overlay_when_host_same_height() {
        // Host 80x24, recording 80x24 → controls at y=23 (overlay bottom row)
        let area = Rect::new(0, 0, 80, 24);
        let rect = App::controls_rect(area, 80, 24);
        assert_eq!(rect.y, 23);
        assert_eq!(rect.height, 1);
        assert_eq!(rect.width, 80);
    }

    #[test]
    fn controls_rect_overlay_when_host_shorter() {
        // Host 60x20, recording 80x24 → controls at y=19 (overlay)
        let area = Rect::new(0, 0, 60, 20);
        let rect = App::controls_rect(area, 80, 24);
        assert_eq!(rect.y, 19);
        assert_eq!(rect.height, 1);
        assert_eq!(rect.width, 60);
    }

    // ── Render integration test with controls ─────────────────────────────────

    #[test]
    fn render_with_controls_bar_visible() {
        let mut player = load_player("minimal_v2.cast");
        // Pause the player (controls should be visible)
        // Player starts paused by default
        let (cols, rows) = player.size();
        player.seek(player.duration());

        let mut app = App::new(player, true, false, None);

        // Use a backend bigger than recording so controls go below
        let backend = TestBackend::new(cols, rows + 1);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|f| app.render(f)).unwrap();

        // Controls bar should be on the row after the recording (y = rows)
        let buf = terminal.backend().buffer();
        // The controls bar fills the bottom row with a dark gray background
        // At minimum, the state icon should appear — check it's non-empty
        let bottom_row: String = (0..cols)
            .map(|x| buf.get(x, rows).symbol().to_string())
            .collect();
        // Controls bar was rendered — it contains content (not all spaces from a blank terminal)
        // The paused icon is "▮▮" or end icon "■ ". Check it's not all spaces.
        assert!(
            bottom_row.chars().any(|c| c != ' '),
            "Controls bar should render non-space content in bottom row, got: {bottom_row:?}"
        );
    }

    // ── Help overlay state preservation tests ────────────────────────────────

    #[test]
    fn toggle_help_while_playing_pauses_and_shows_overlay() {
        let mut player = load_player("minimal_v2.cast");
        player.play();
        let mut app = App::new(player, true, false, None);

        app.handle_action(Action::ToggleHelp);

        assert!(app.help_visible);
        assert!(!app.player.is_playing()); // paused while help shown
    }

    #[test]
    fn dismiss_help_resumes_if_was_playing() {
        let mut player = load_player("minimal_v2.cast");
        player.play();
        let mut app = App::new(player, true, false, None);

        app.handle_action(Action::ToggleHelp); // show help (was playing)
        app.handle_action(Action::ToggleHelp); // dismiss help

        assert!(!app.help_visible);
        assert!(app.player.is_playing()); // resumed
    }

    #[test]
    fn toggle_help_while_paused_stays_paused_on_dismiss() {
        let player = load_player("minimal_v2.cast");
        // player starts paused — do NOT call play()
        let mut app = App::new(player, true, false, None);

        app.handle_action(Action::ToggleHelp); // show help (was paused)
        assert!(app.help_visible);
        assert!(!app.player.is_playing());

        app.handle_action(Action::ToggleHelp); // dismiss help
        assert!(!app.help_visible);
        assert!(!app.player.is_playing()); // stays paused
    }

    // ── Esc / Quit behavior tests ─────────────────────────────────────────────

    #[test]
    fn esc_dismisses_help_when_visible() {
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, true, false, None);

        app.handle_action(Action::ToggleHelp); // show help
        assert!(app.help_visible);

        app.handle_action(Action::Escape); // Esc → dismiss, not quit
        assert!(!app.help_visible);
        assert!(!app.should_quit);
    }

    #[test]
    fn esc_quits_when_nothing_active() {
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, true, false, None);

        app.handle_action(Action::Escape);
        assert!(app.should_quit);
    }

    #[test]
    fn q_dismisses_help_when_visible() {
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, true, false, None);

        app.handle_action(Action::ToggleHelp); // show help
        app.handle_action(Action::Quit); // q → dismiss help

        assert!(!app.help_visible);
        assert!(!app.should_quit);
    }

    #[test]
    fn q_quits_after_help_dismissed() {
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, true, false, None);

        app.handle_action(Action::ToggleHelp); // show help
        app.handle_action(Action::Quit); // q → dismiss help
        app.handle_action(Action::Quit); // q again → now quits

        assert!(app.should_quit);
    }

    // ── Action blocking tests ─────────────────────────────────────────────────

    #[test]
    fn seek_forward_blocked_while_help_visible() {
        let mut player = load_player("minimal_v2.cast");
        player.play();
        let mut app = App::new(player, true, false, None);

        let pos_before = app.player.current_time();
        app.handle_action(Action::ToggleHelp); // show help
        app.handle_action(Action::SeekForward); // should be blocked

        assert_eq!(app.player.current_time(), pos_before);
    }

    #[test]
    fn speed_up_blocked_while_help_visible() {
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, true, false, None);

        let speed_before = app.player.speed();
        app.handle_action(Action::ToggleHelp); // show help
        app.handle_action(Action::SpeedUp); // should be blocked

        assert_eq!(app.player.speed(), speed_before);
    }

    #[test]
    fn toggle_help_dismisses_overlay_while_visible() {
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, true, false, None);

        app.handle_action(Action::ToggleHelp); // show
        assert!(app.help_visible);
        app.handle_action(Action::ToggleHelp); // dismiss
        assert!(!app.help_visible);
    }

    // ── Help overlay + controls interaction tests ─────────────────────────────

    #[test]
    fn show_help_forces_controls_visible() {
        // Start with auto-hidden controls (playing, manually hidden cleared)
        let mut player = load_player("minimal_v2.cast");
        player.play();
        let mut app = App::new(player, true, false, None);
        // Simulate controls having been auto-hidden
        app.controls_force_show = false;
        app.controls_manually_hidden = false;
        app.last_interaction = Instant::now() - Duration::from_secs(10);
        assert!(!app.controls_visible()); // sanity: controls are hidden

        // Opening help should force controls visible
        app.handle_action(Action::ToggleHelp);
        assert!(app.help_visible);
        assert!(app.controls_visible());
        assert!(app.controls_force_show);
    }

    #[test]
    fn dismiss_help_resumes_autohide_when_was_playing() {
        // Playing, then open help, then dismiss — controls_force_show should be cleared
        let mut player = load_player("minimal_v2.cast");
        player.play();
        let mut app = App::new(player, true, false, None);

        app.handle_action(Action::ToggleHelp); // pauses, sets force_show=true
        assert!(app.controls_force_show);

        app.handle_action(Action::ToggleHelp); // dismisses, resumes play, clears force_show
        assert!(!app.help_visible);
        assert!(app.player.is_playing());
        // force_show cleared so auto-hide timer governs visibility
        assert!(!app.controls_force_show);
        // last_interaction is fresh from keypress, so controls are still visible within 2s
        assert!(app.controls_visible());
    }

    #[test]
    fn dismiss_help_keeps_force_show_when_was_paused() {
        // Paused, then open help, then dismiss — controls should stay force-shown
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, true, false, None);
        assert!(!app.player.is_playing());

        app.handle_action(Action::ToggleHelp);
        app.handle_action(Action::ToggleHelp); // dismiss

        assert!(!app.help_visible);
        assert!(!app.player.is_playing());
        // Paused state: force_show should still be true
        assert!(app.controls_force_show);
        assert!(app.controls_visible());
    }

    // ── Toggle cycle test ─────────────────────────────────────────────────────

    #[test]
    fn toggle_cycle_playing() {
        let mut player = load_player("minimal_v2.cast");
        player.play();
        let mut app = App::new(player, true, false, None);

        // ? → help shown, paused
        app.handle_action(Action::ToggleHelp);
        assert!(app.help_visible);
        assert!(!app.player.is_playing());

        // ? → help dismissed, resumed
        app.handle_action(Action::ToggleHelp);
        assert!(!app.help_visible);
        assert!(app.player.is_playing());

        // ? → help shown again, paused
        app.handle_action(Action::ToggleHelp);
        assert!(app.help_visible);
        assert!(!app.player.is_playing());
    }

    // ── Search input and navigation tests ─────────────────────────────────────

    #[test]
    fn test_start_search_sets_input_mode() {
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, true, false, None);

        app.handle_action(Action::StartSearch);
        assert_eq!(app.input_mode, InputMode::SearchInput);
    }

    #[test]
    fn test_esc_cancels_search_input() {
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, true, false, None);

        app.handle_action(Action::StartSearch);
        assert_eq!(app.input_mode, InputMode::SearchInput);

        // Type something into search input
        app.search_input.push_str("hello");

        // Simulate Esc in search input mode
        app.handle_search_input(KeyEvent::new(
            KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));

        assert_eq!(app.input_mode, InputMode::Normal);
        assert!(app.search_input.is_empty());
    }

    #[test]
    fn test_esc_dismisses_help() {
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, true, false, None);

        app.handle_action(Action::ToggleHelp);
        assert!(app.help_visible);

        app.handle_action(Action::Escape);
        assert!(!app.help_visible);
        assert!(!app.should_quit);
    }

    #[test]
    fn test_esc_clears_search() {
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, true, false, None);

        app.search_query = Some("test".to_string());

        app.handle_action(Action::Escape);

        assert_eq!(app.search_query, None);
        assert!(!app.should_quit);
    }

    #[test]
    fn test_esc_quits_when_nothing_active() {
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, true, false, None);

        // No search, no help
        app.handle_action(Action::Escape);
        assert!(app.should_quit);
    }

    #[test]
    fn test_esc_chain_order() {
        // Verify priority: search input mode > help overlay > search active > quit
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, true, false, None);

        // Set up all states: search query active, help visible, and search input mode
        app.search_query = Some("test".to_string());

        // Stage 1: Esc clears search query first
        app.handle_action(Action::Escape);
        assert_eq!(app.search_query, None);
        assert!(!app.should_quit);

        // Stage 2: Esc with nothing active -> quit
        app.handle_action(Action::Escape);
        assert!(app.should_quit);
    }

    #[test]
    fn test_esc_chain_with_search_input_mode() {
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, true, false, None);

        // Enter search input mode
        app.handle_action(Action::StartSearch);
        assert_eq!(app.input_mode, InputMode::SearchInput);

        // Esc in search input mode cancels search input (handled by handle_search_input)
        app.handle_search_input(KeyEvent::new(
            KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(app.input_mode, InputMode::Normal);
        assert!(!app.should_quit);
    }

    #[test]
    fn test_esc_chain_help_before_search() {
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, true, false, None);

        // Set up help visible AND search query active
        app.search_query = Some("test".to_string());
        app.handle_action(Action::ToggleHelp);
        assert!(app.help_visible);

        // Esc should dismiss help first (help is checked before search in handle_action)
        app.handle_action(Action::Escape);
        assert!(!app.help_visible);
        assert_eq!(app.search_query, Some("test".to_string())); // search still active
        assert!(!app.should_quit);

        // Next Esc clears search
        app.handle_action(Action::Escape);
        assert_eq!(app.search_query, None);
        assert!(!app.should_quit);

        // Final Esc quits
        app.handle_action(Action::Escape);
        assert!(app.should_quit);
    }

    #[test]
    fn test_next_match_no_search_is_noop() {
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, true, false, None);

        let time_before = app.player.current_time();
        app.handle_action(Action::NextMatch);
        assert_eq!(app.player.current_time(), time_before);
        assert!(!app.should_quit);
    }

    #[test]
    fn test_prev_match_no_search_is_noop() {
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, true, false, None);

        let time_before = app.player.current_time();
        app.handle_action(Action::PrevMatch);
        assert_eq!(app.player.current_time(), time_before);
        assert!(!app.should_quit);
    }

    #[test]
    fn test_start_search_ignored_during_help() {
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, true, false, None);

        app.handle_action(Action::ToggleHelp);
        assert!(app.help_visible);

        app.handle_action(Action::StartSearch);
        // Should still be in Normal mode with help showing
        assert_eq!(app.input_mode, InputMode::Normal);
        assert!(app.help_visible);
    }

    #[test]
    fn test_search_input_inserts_characters() {
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, true, false, None);

        app.handle_action(Action::StartSearch);
        assert_eq!(app.input_mode, InputMode::SearchInput);

        // Type characters including space and q
        app.handle_search_input(KeyEvent::new(
            KeyCode::Char('h'),
            crossterm::event::KeyModifiers::NONE,
        ));
        app.handle_search_input(KeyEvent::new(
            KeyCode::Char('e'),
            crossterm::event::KeyModifiers::NONE,
        ));
        app.handle_search_input(KeyEvent::new(
            KeyCode::Char(' '),
            crossterm::event::KeyModifiers::NONE,
        ));
        app.handle_search_input(KeyEvent::new(
            KeyCode::Char('q'),
            crossterm::event::KeyModifiers::NONE,
        ));

        assert_eq!(app.search_input, "he q");
        // Still in search input mode (space didn't toggle playback, q didn't quit)
        assert_eq!(app.input_mode, InputMode::SearchInput);
        assert!(!app.should_quit);
    }

    #[test]
    fn test_search_input_backspace() {
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, true, false, None);

        app.handle_action(Action::StartSearch);
        app.handle_search_input(KeyEvent::new(
            KeyCode::Char('a'),
            crossterm::event::KeyModifiers::NONE,
        ));
        app.handle_search_input(KeyEvent::new(
            KeyCode::Char('b'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(app.search_input, "ab");

        app.handle_search_input(KeyEvent::new(
            KeyCode::Backspace,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(app.search_input, "a");
    }

    #[test]
    fn test_search_enter_commits_query() {
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, true, false, None);

        app.handle_action(Action::StartSearch);
        app.handle_search_input(KeyEvent::new(
            KeyCode::Char('h'),
            crossterm::event::KeyModifiers::NONE,
        ));
        app.handle_search_input(KeyEvent::new(
            KeyCode::Char('i'),
            crossterm::event::KeyModifiers::NONE,
        ));
        app.handle_search_input(KeyEvent::new(
            KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));

        assert_eq!(app.input_mode, InputMode::Normal);
        assert_eq!(app.search_query, Some("hi".to_string()));
    }

    #[test]
    fn test_search_empty_enter_cancels() {
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, true, false, None);

        app.handle_action(Action::StartSearch);
        // Enter with empty query
        app.handle_search_input(KeyEvent::new(
            KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));

        assert_eq!(app.input_mode, InputMode::Normal);
        assert_eq!(app.search_query, None); // no query committed
    }

    // ── find_crossed_marker unit tests ──────────────────────────────────────

    fn make_markers(times: &[f64]) -> Vec<speedrun_core::Marker> {
        times
            .iter()
            .map(|&t| speedrun_core::Marker {
                time: t,
                label: String::new(),
            })
            .collect()
    }

    #[test]
    fn test_find_crossed_marker_hit() {
        let markers = make_markers(&[3.0, 7.0]);
        let result = find_crossed_marker(&markers, 2.0, 4.0);
        assert_eq!(result, Some(3.0));
    }

    #[test]
    fn test_find_crossed_marker_miss() {
        let markers = make_markers(&[3.0, 7.0]);
        let result = find_crossed_marker(&markers, 4.0, 6.0);
        assert_eq!(result, None);
    }

    #[test]
    fn test_find_crossed_marker_multiple() {
        // Multiple markers in range — should return first (lowest time)
        let markers = make_markers(&[3.0, 5.0, 7.0]);
        let result = find_crossed_marker(&markers, 2.0, 8.0);
        assert_eq!(result, Some(3.0));
    }

    #[test]
    fn test_find_crossed_marker_at_prev_time() {
        // Marker exactly at prev_time is excluded (strict lower bound prevents re-trigger)
        let markers = make_markers(&[3.0]);
        let result = find_crossed_marker(&markers, 3.0, 5.0);
        assert_eq!(result, None);
    }

    #[test]
    fn test_find_crossed_marker_at_new_time() {
        // Marker exactly at new_time is included (inclusive upper bound)
        let markers = make_markers(&[3.0]);
        let result = find_crossed_marker(&markers, 1.0, 3.0);
        assert_eq!(result, Some(3.0));
    }

    #[test]
    fn test_find_crossed_marker_empty() {
        let result = find_crossed_marker(&[], 0.0, 5.0);
        assert_eq!(result, None);
    }

    // ── Marker authoring tests ───────────────────────────────────────────────

    /// Helper: copy a test data file to a temp file and create an App with its path.
    fn make_marker_app(name: &str) -> (App, tempfile::NamedTempFile) {
        let src = testdata_path(name);
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut tmp, &std::fs::read(&src).unwrap()).unwrap();
        let player = {
            let file = std::fs::File::open(tmp.path()).unwrap();
            speedrun_core::Player::load(file).unwrap()
        };
        let app = App::new(player, true, false, Some(tmp.path().to_path_buf()));
        (app, tmp)
    }

    #[test]
    fn test_m_enters_confirm_mode() {
        let (mut app, _tmp) = make_marker_app("with_markers.cast");
        assert!(!app.file_modify_confirmed);

        app.handle_action(Action::AddMarker);

        assert_eq!(app.input_mode, InputMode::ConfirmFileModify);
        assert!(!app.player.is_playing()); // paused
    }

    #[test]
    fn test_confirm_y_creates_marker() {
        let (mut app, tmp) = make_marker_app("with_markers.cast");
        app.player.seek(5.0);
        let markers_before = app.player.markers().len();

        app.handle_action(Action::AddMarker);
        assert_eq!(app.input_mode, InputMode::ConfirmFileModify);

        // Press 'y' to confirm
        app.handle_confirm_input(KeyEvent::new(
            KeyCode::Char('y'),
            crossterm::event::KeyModifiers::NONE,
        ));

        assert_eq!(app.input_mode, InputMode::Normal);
        assert!(app.file_modify_confirmed);
        assert_eq!(app.player.markers().len(), markers_before + 1);
        // File should have a new line
        let contents = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(contents.contains("\"m\""));
    }

    #[test]
    fn test_confirm_n_cancels() {
        let (mut app, tmp) = make_marker_app("with_markers.cast");
        let markers_before = app.player.markers().len();
        let file_before = std::fs::read_to_string(tmp.path()).unwrap();

        app.handle_action(Action::AddMarker);
        assert_eq!(app.input_mode, InputMode::ConfirmFileModify);

        // Press 'n' to cancel
        app.handle_confirm_input(KeyEvent::new(
            KeyCode::Char('n'),
            crossterm::event::KeyModifiers::NONE,
        ));

        assert_eq!(app.input_mode, InputMode::Normal);
        assert!(!app.file_modify_confirmed);
        assert_eq!(app.player.markers().len(), markers_before);
        let file_after = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(file_before, file_after);
    }

    #[test]
    fn test_confirm_esc_cancels() {
        let (mut app, tmp) = make_marker_app("with_markers.cast");
        let markers_before = app.player.markers().len();
        let file_before = std::fs::read_to_string(tmp.path()).unwrap();

        app.handle_action(Action::AddMarker);
        assert_eq!(app.input_mode, InputMode::ConfirmFileModify);

        // Press Esc to cancel
        app.handle_confirm_input(KeyEvent::new(
            KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));

        assert_eq!(app.input_mode, InputMode::Normal);
        assert!(!app.file_modify_confirmed);
        assert_eq!(app.player.markers().len(), markers_before);
        let file_after = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(file_before, file_after);
    }

    #[test]
    fn test_confirm_q_quits() {
        let (mut app, _tmp) = make_marker_app("with_markers.cast");

        app.handle_action(Action::AddMarker);
        assert_eq!(app.input_mode, InputMode::ConfirmFileModify);

        // Press 'q' to quit
        app.handle_confirm_input(KeyEvent::new(
            KeyCode::Char('q'),
            crossterm::event::KeyModifiers::NONE,
        ));

        assert!(app.should_quit);
    }

    #[test]
    fn test_m_after_confirm_skips_dialog() {
        let (mut app, _tmp) = make_marker_app("with_markers.cast");
        app.file_modify_confirmed = true;
        app.player.seek(5.0); // seek to a position to ensure time mapping works
        let markers_before = app.player.markers().len();

        app.handle_action(Action::AddMarker);

        // Should go directly to Normal (no confirmation), marker created
        assert_eq!(app.input_mode, InputMode::Normal);
        assert_eq!(app.player.markers().len(), markers_before + 1);
    }

    #[test]
    fn test_m_stdin_no_file_path() {
        let player = load_player("with_markers.cast");
        let mut app = App::new(player, true, false, None);
        let markers_before = app.player.markers().len();

        app.handle_action(Action::AddMarker);

        // Should stay in Normal mode, no panic, no marker
        assert_eq!(app.input_mode, InputMode::Normal);
        assert_eq!(app.player.markers().len(), markers_before);
        // Error feedback should be set
        assert!(app.marker_error_feedback.is_some());
    }

    #[test]
    fn test_file_append_no_double_newline() {
        // with_markers.cast ends with '\n' — should NOT insert blank line
        let (mut app, tmp) = make_marker_app("with_markers.cast");
        app.file_modify_confirmed = true;
        app.player.seek(5.0);

        app.handle_action(Action::AddMarker);

        let contents = std::fs::read_to_string(tmp.path()).unwrap();
        // Should not have double-newline before the marker line
        assert!(
            !contents.contains("\n\n"),
            "Should not have double newline, got:\n{contents}"
        );
    }

    #[test]
    fn test_file_append_adds_newline_if_missing() {
        // Create a temp file with content NOT ending in newline
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        let src = testdata_path("with_markers.cast");
        let content = std::fs::read_to_string(&src).unwrap();
        let trimmed = content.trim_end_matches('\n');
        std::io::Write::write_all(&mut tmp, trimmed.as_bytes()).unwrap();

        let player = {
            let file = std::fs::File::open(tmp.path()).unwrap();
            speedrun_core::Player::load(file).unwrap()
        };
        let mut app = App::new(player, true, false, Some(tmp.path().to_path_buf()));
        app.file_modify_confirmed = true;
        app.player.seek(5.0);

        app.handle_action(Action::AddMarker);

        let result = std::fs::read_to_string(tmp.path()).unwrap();
        // The marker line should be on its own line (preceded by \n)
        let lines: Vec<&str> = result.lines().collect();
        let last_line = lines.last().unwrap();
        assert!(
            last_line.starts_with('['),
            "Last line should be the marker event, got: {last_line}"
        );
        // The second-to-last line should be the last event, not concatenated
        let second_last = lines[lines.len() - 2];
        assert!(
            second_last.starts_with('['),
            "Second-to-last line should be complete, got: {second_last}"
        );
    }

    #[test]
    fn test_m_on_v3_recording_shows_error() {
        let player = load_player("minimal_v3.cast");
        let mut app = App::new(
            player,
            true,
            false,
            Some(std::path::PathBuf::from("fake.cast")),
        );

        app.handle_action(Action::AddMarker);

        assert_eq!(app.input_mode, InputMode::Normal);
        assert!(app.marker_error_feedback.is_some());
        let (msg, _) = app.marker_error_feedback.as_ref().unwrap();
        assert!(msg.contains("v3"), "Error should mention v3, got: {msg}");
    }

    #[test]
    fn test_cancel_resumes_playback() {
        let (mut app, _tmp) = make_marker_app("with_markers.cast");
        app.player.play();
        assert!(app.player.is_playing());

        app.handle_action(Action::AddMarker);
        // Should be paused during confirmation
        assert!(!app.player.is_playing());

        // Cancel with 'n'
        app.handle_confirm_input(KeyEvent::new(
            KeyCode::Char('n'),
            crossterm::event::KeyModifiers::NONE,
        ));

        // Should resume playing
        assert!(app.player.is_playing());
    }

    #[test]
    fn test_marker_navigable_after_creation() {
        let (mut app, _tmp) = make_marker_app("with_markers.cast");
        app.file_modify_confirmed = true;
        app.player.seek(5.0); // between markers at 3.0 and 7.0

        app.handle_action(Action::AddMarker);

        // New marker at 5.0 should be navigable
        app.player.seek(4.0);
        app.handle_action(Action::NextMarker);
        assert!(
            (app.player.current_time() - 5.0).abs() < 0.01,
            "Expected to navigate to new marker at 5.0, got {}",
            app.player.current_time()
        );
    }
}
