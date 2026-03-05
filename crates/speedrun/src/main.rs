use clap::Parser;
use crossterm::{
    event::KeyModifiers,
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

/// A modern terminal session player with instant seeking.
#[derive(Parser, Debug)]
#[command(name = "speedrun", version, about)]
struct Args {
    /// Path to an asciicast v2 or v3 recording.
    file: std::path::PathBuf,

    /// Playback speed multiplier.
    #[arg(short, long, default_value_t = 1.0)]
    speed: f64,

    /// Start playback at this timestamp (seconds).
    #[arg(short = 't', long)]
    start_at: Option<f64>,

    /// Cap idle time between events (seconds).
    /// Overrides the recording header's idle_time_limit.
    #[arg(short, long)]
    idle_limit: Option<f64>,

    /// Keyframe snapshot interval in seconds.
    /// Lower values increase memory but make seeks faster.
    #[arg(long, default_value_t = 5.0)]
    keyframe_interval: f64,

    /// Start with the controls bar hidden.
    #[arg(long)]
    no_controls: bool,
}

type Tui = Terminal<CrosstermBackend<std::io::Stdout>>;

fn setup_terminal() -> std::io::Result<Tui> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut Tui) -> std::io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn install_panic_hook() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        // Best-effort terminal restoration
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
        original_hook(panic_info);
    }));
}

fn run(terminal: &mut Tui, _player: speedrun_core::Player, _args: &Args) -> std::io::Result<()> {
    loop {
        terminal.draw(|frame| {
            // Placeholder: just clear the screen
            let area = frame.size();
            frame.render_widget(ratatui::widgets::Clear, area);
        })?;

        // Wait for input, quit on 'q', Esc, or Ctrl-C
        if crossterm::event::poll(std::time::Duration::from_millis(100))?
            && let crossterm::event::Event::Key(key) = crossterm::event::read()?
            && (matches!(
                key.code,
                crossterm::event::KeyCode::Char('q') | crossterm::event::KeyCode::Esc
            ) || (key.code == crossterm::event::KeyCode::Char('c')
                && key.modifiers.contains(KeyModifiers::CONTROL)))
        {
            break;
        }
    }
    Ok(())
}

fn main() {
    install_panic_hook();

    let args = Args::parse();

    let file = std::fs::File::open(&args.file).unwrap_or_else(|e| {
        match e.kind() {
            std::io::ErrorKind::NotFound => {
                eprintln!("File not found: {}", args.file.display());
            }
            _ => {
                eprintln!("Cannot read file: {}: {e}", args.file.display());
            }
        }
        std::process::exit(1);
    });

    let opts = speedrun_core::LoadOptions {
        idle_limit: args.idle_limit,
    };

    let mut player = speedrun_core::Player::load_with(file, opts).unwrap_or_else(|e| {
        eprintln!("Invalid recording: {e}");
        std::process::exit(1);
    });

    player.set_speed(args.speed);

    if let Some(t) = args.start_at {
        player.seek(t);
    }

    player.play();

    let mut terminal = setup_terminal().unwrap_or_else(|e| {
        eprintln!("Failed to initialize terminal: {e}");
        std::process::exit(1);
    });

    // Run the app (placeholder — just clear screen and wait for 'q')
    let result = run(&mut terminal, player, &args);

    // Always restore terminal, even if run() returned an error
    let _ = restore_terminal(&mut terminal);

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
