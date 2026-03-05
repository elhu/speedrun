use crate::ui::{TerminalView, ViewportState};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use speedrun_core::Player;
use std::time::{Duration, Instant};

type Tui = Terminal<CrosstermBackend<std::io::Stdout>>;

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

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                self.should_quit = true;
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::Char(' ') => {
                self.player.toggle();
            }
            KeyCode::Right => {
                self.player.seek_relative(5.0);
                self.show_controls = true;
            }
            KeyCode::Left => {
                self.player.seek_relative(-5.0);
                self.show_controls = true;
            }
            _ => {}
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
}
