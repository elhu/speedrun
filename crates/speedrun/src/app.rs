use crate::input::{Action, map_key_event};
use crate::ui::{TerminalView, ViewportState};
use crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use speedrun_core::Player;
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

pub struct App {
    pub player: Player,
    pub show_controls: bool,
    pub last_interaction: Instant,
    pub should_quit: bool,
    viewport: ViewportState,
}

impl App {
    pub fn new(player: Player, show_controls: bool) -> Self {
        Self {
            player,
            show_controls,
            last_interaction: Instant::now(),
            should_quit: false,
            viewport: ViewportState::default(),
        }
    }

    pub fn run(&mut self, terminal: &mut Tui) -> std::io::Result<()> {
        let mut last_tick = Instant::now();
        let mut needs_redraw = true; // render first frame immediately

        loop {
            // Render (conditional)
            if needs_redraw {
                terminal.draw(|frame| self.render(frame))?;
                needs_redraw = false;
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
                let changed = self.player.tick(dt);
                if changed {
                    needs_redraw = true;
                }
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
    pub fn compute_timeout(&self) -> Duration {
        self.player
            .time_to_next_event()
            .unwrap_or(Duration::from_millis(100))
    }

    fn handle_key(&mut self, key: KeyEvent) {
        // Ignore key release events (crossterm 0.27+ sends both press and release)
        if key.kind != KeyEventKind::Press {
            return;
        }

        self.last_interaction = Instant::now();

        if let Some(action) = map_key_event(key) {
            self.handle_action(action);
        }
    }

    fn handle_action(&mut self, action: Action) {
        match action {
            Action::Quit => {
                self.should_quit = true;
            }
            Action::TogglePlayback => {
                self.player.toggle();
            }
            Action::SeekForward => {
                self.player.seek_relative(5.0);
                self.show_controls = true;
            }
            Action::SeekBackward => {
                self.player.seek_relative(-5.0);
                self.show_controls = true;
            }
            Action::SeekForward30s => {
                self.player.seek_relative(30.0);
                self.show_controls = true;
                self.last_interaction = Instant::now();
            }
            Action::SeekBackward30s => {
                self.player.seek_relative(-30.0);
                self.show_controls = true;
                self.last_interaction = Instant::now();
            }
            Action::StepForward => {
                self.player.step_forward();
                self.show_controls = true;
                self.last_interaction = Instant::now();
            }
            Action::StepBackward => {
                self.player.step_backward();
                self.show_controls = true;
                self.last_interaction = Instant::now();
            }
            Action::SpeedUp => {
                let new_speed = next_speed(self.player.speed(), 1);
                self.player.set_speed(new_speed);
                self.show_controls = true;
                self.last_interaction = Instant::now();
            }
            Action::SpeedDown => {
                let new_speed = next_speed(self.player.speed(), -1);
                self.player.set_speed(new_speed);
                self.show_controls = true;
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
                }
                self.show_controls = true;
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
                }
                self.show_controls = true;
                self.last_interaction = Instant::now();
            }
            Action::JumpToPercent(n) => {
                let target = self.player.duration() * (n as f64) / 10.0;
                self.player.seek(target);
                self.show_controls = true;
                self.last_interaction = Instant::now();
            }
            Action::JumpToStart => {
                self.player.seek(0.0);
                self.show_controls = true;
                self.last_interaction = Instant::now();
            }
            Action::JumpToEnd => {
                self.player.seek(self.player.duration());
                if self.player.is_playing() {
                    self.player.pause();
                }
                self.show_controls = true;
                self.last_interaction = Instant::now();
            }
            Action::ToggleControls => {
                // TODO: implemented in Phase 3 epic
            }
            Action::ToggleHelp => {
                // TODO: implemented in Phase 3 epic
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

        let view = TerminalView::new(self.player.screen(), cursor, (rec_cols, rec_rows))
            .with_scroll(self.viewport.scroll_x, self.viewport.scroll_y);

        frame.render_widget(view, area);
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
        let mut app = App::new(player, true);

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
        let app = App::new(player, true);

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
        let app = App::new(player, true);

        assert_eq!(app.compute_timeout(), expected);
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

        let app = App::new(player, true);
        // time_to_next_event() returns None when paused → fallback 100ms
        assert_eq!(app.compute_timeout(), Duration::from_millis(100));
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
        let mut app = App::new(player, false);

        app.handle_action(Action::SpeedUp);
        assert!((app.player.speed() - 1.5).abs() < 1e-9);
        assert!(app.show_controls);
    }

    #[test]
    fn speed_down_action_changes_player_speed() {
        let mut player = load_player("minimal_v2.cast");
        player.set_speed(1.0);
        let mut app = App::new(player, false);

        app.handle_action(Action::SpeedDown);
        assert!((app.player.speed() - 0.5).abs() < 1e-9);
        assert!(app.show_controls);
    }

    // ── 30s seek tests ───────────────────────────────────────────────────────

    #[test]
    fn seek_forward_30s_from_start() {
        // minimal_v2.cast has duration 2.1s — seeking 30s forward should clamp to duration
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, false);

        app.handle_action(Action::SeekForward30s);
        assert!((app.player.current_time() - app.player.duration()).abs() < 1e-9);
        assert!(app.show_controls);
    }

    #[test]
    fn seek_backward_30s_clamps_to_zero() {
        let mut player = load_player("minimal_v2.cast");
        player.seek(1.5);
        let mut app = App::new(player, false);

        app.handle_action(Action::SeekBackward30s);
        assert!((app.player.current_time() - 0.0).abs() < 1e-9);
        assert!(app.show_controls);
    }

    // ── Stepping tests ───────────────────────────────────────────────────────

    #[test]
    fn step_forward_while_paused_advances() {
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, false);
        assert!(!app.player.is_playing());

        app.handle_action(Action::StepForward);
        // First output event in minimal_v2.cast is at 0.5
        assert!((app.player.current_time() - 0.5).abs() < 1e-9);
        assert!(app.show_controls);
    }

    #[test]
    fn step_backward_while_paused_goes_back() {
        let mut player = load_player("minimal_v2.cast");
        player.seek(1.5); // between events at 1.2 and 2.0
        let mut app = App::new(player, false);
        assert!(!app.player.is_playing());

        app.handle_action(Action::StepBackward);
        assert!((app.player.current_time() - 1.2).abs() < 1e-9);
        assert!(app.show_controls);
    }

    #[test]
    fn step_forward_while_playing_does_nothing() {
        let mut player = load_player("minimal_v2.cast");
        player.play();
        let mut app = App::new(player, false);
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
        let mut app = App::new(player, false);

        app.handle_action(Action::NextMarker);
        assert!((app.player.current_time() - 3.0).abs() < 1e-9);
        assert!(app.show_controls);
    }

    #[test]
    fn next_marker_from_first_marker() {
        let mut player = load_player("with_markers.cast");
        player.seek(3.0);
        let mut app = App::new(player, false);

        app.handle_action(Action::NextMarker);
        assert!((app.player.current_time() - 7.0).abs() < 1e-9);
    }

    #[test]
    fn next_marker_from_last_marker_does_nothing() {
        let mut player = load_player("with_markers.cast");
        player.seek(7.0);
        let mut app = App::new(player, false);

        app.handle_action(Action::NextMarker);
        // Should stay at 7.0 since no marker after
        assert!((app.player.current_time() - 7.0).abs() < 1e-9);
    }

    #[test]
    fn prev_marker_from_last_marker() {
        let mut player = load_player("with_markers.cast");
        player.seek(7.0);
        let mut app = App::new(player, false);

        app.handle_action(Action::PrevMarker);
        assert!((app.player.current_time() - 3.0).abs() < 1e-9);
        assert!(app.show_controls);
    }

    #[test]
    fn prev_marker_from_first_marker_does_nothing() {
        let mut player = load_player("with_markers.cast");
        player.seek(3.0);
        let mut app = App::new(player, false);

        app.handle_action(Action::PrevMarker);
        // Should stay at 3.0 since no marker before
        assert!((app.player.current_time() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn prev_marker_from_mid_recording() {
        let mut player = load_player("with_markers.cast");
        player.seek(5.0); // between markers at 3.0 and 7.0
        let mut app = App::new(player, false);

        app.handle_action(Action::PrevMarker);
        assert!((app.player.current_time() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn marker_nav_no_markers_does_nothing() {
        // minimal_v2.cast has no markers
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, false);

        app.handle_action(Action::NextMarker);
        assert!((app.player.current_time() - 0.0).abs() < 1e-9);

        app.handle_action(Action::PrevMarker);
        assert!((app.player.current_time() - 0.0).abs() < 1e-9);
    }

    // ── Percentage jump tests ────────────────────────────────────────────────

    #[test]
    fn jump_to_percent_0() {
        let mut player = load_player("with_markers.cast");
        player.seek(4.0);
        let mut app = App::new(player, false);

        app.handle_action(Action::JumpToPercent(0));
        assert!((app.player.current_time() - 0.0).abs() < 1e-9);
        assert!(app.show_controls);
    }

    #[test]
    fn jump_to_percent_5() {
        // with_markers.cast has duration 8.0, so 50% = 4.0
        let player = load_player("with_markers.cast");
        let mut app = App::new(player, false);
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
        let mut app = App::new(player, false);
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
        let mut app = App::new(player, false);

        app.handle_action(Action::JumpToStart);
        assert!((app.player.current_time() - 0.0).abs() < 1e-9);
        assert!(app.show_controls);
    }

    #[test]
    fn jump_to_end_pauses_player() {
        let mut player = load_player("minimal_v2.cast");
        player.play();
        let mut app = App::new(player, false);
        assert!(app.player.is_playing());

        app.handle_action(Action::JumpToEnd);
        assert!(
            (app.player.current_time() - app.player.duration()).abs() < 1e-9,
            "expected current_time == duration"
        );
        assert!(!app.player.is_playing(), "player should be paused at end");
        assert!(app.show_controls);
    }

    #[test]
    fn jump_to_end_already_paused() {
        let player = load_player("minimal_v2.cast");
        let mut app = App::new(player, false);
        assert!(!app.player.is_playing());

        app.handle_action(Action::JumpToEnd);
        assert!((app.player.current_time() - app.player.duration()).abs() < 1e-9);
        assert!(!app.player.is_playing());
    }

    // ── Edge case: empty recording ───────────────────────────────────────────

    #[test]
    fn navigation_on_empty_recording_no_panic() {
        let player = load_player("empty.cast");
        let mut app = App::new(player, false);

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
}
