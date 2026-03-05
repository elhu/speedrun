use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::widgets::{Block, Borders, Paragraph};
use speedrun_core::Player;
use std::time::Instant;

type Tui = Terminal<CrosstermBackend<std::io::Stdout>>;

pub struct App {
    pub player: Player,
    pub show_controls: bool,
    pub last_interaction: Instant,
    pub should_quit: bool,
}

impl App {
    pub fn new(player: Player, show_controls: bool) -> Self {
        Self {
            player,
            show_controls,
            last_interaction: Instant::now(),
            should_quit: false,
        }
    }

    pub fn run(&mut self, terminal: &mut Tui) -> std::io::Result<()> {
        let mut last_tick = Instant::now();

        loop {
            // Render
            terminal.draw(|frame| self.render(frame))?;

            // Compute timeout
            let timeout = self
                .player
                .time_to_next_event()
                .unwrap_or(std::time::Duration::from_millis(100));

            // Poll for input
            if event::poll(timeout)? {
                match event::read()? {
                    Event::Key(key) => self.handle_key(key),
                    // Resize triggers automatic re-render on next loop iteration
                    Event::Resize(_, _) => {}
                    _ => {}
                }
            }

            // Advance playback
            let now = Instant::now();
            let dt = now.duration_since(last_tick).as_secs_f64();
            last_tick = now;

            if self.player.is_playing() {
                self.player.tick(dt);
            }

            if self.should_quit {
                break;
            }
        }
        Ok(())
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

    fn render(&self, frame: &mut Frame) {
        let area = frame.size();

        // Placeholder: show recording info text until TerminalView widget is ready
        let info = format!(
            "speedrun — {}x{} | {:.1}s / {:.1}s | {} | {:.1}x",
            self.player.size().0,
            self.player.size().1,
            self.player.current_time(),
            self.player.duration(),
            if self.player.is_playing() {
                "playing"
            } else {
                "paused"
            },
            self.player.speed(),
        );

        let paragraph =
            Paragraph::new(info).block(Block::default().borders(Borders::ALL).title("speedrun"));
        frame.render_widget(paragraph, area);
    }
}
