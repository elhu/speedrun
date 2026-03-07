mod app;
pub mod controls;
pub mod help;
pub mod input;
pub mod ui;

use std::io::{IsTerminal, Read};

use clap::{Parser, Subcommand};
use crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

// ---------------------------------------------------------------------------
// CLI structure
// ---------------------------------------------------------------------------

/// A modern terminal session player with instant seeking.
#[derive(Parser, Debug)]
#[command(name = "speedrun", version, about)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    /// Path to an asciicast v2 or v3 recording. Use "-" to read from stdin.
    /// (Positional argument for direct playback — use this when no subcommand
    /// is given.)
    file: Option<std::path::PathBuf>,

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
    #[arg(short = 'k', long, default_value_t = 5.0)]
    keyframe_interval: f64,

    /// Start with the controls bar hidden.
    #[arg(long)]
    no_controls: bool,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Export a recording to various formats.
    Export {
        #[command(subcommand)]
        format: ExportFormat,
    },
}

#[derive(Subcommand, Debug)]
enum ExportFormat {
    /// Export a recording as an SVG image.
    Svg {
        /// Path to the asciicast recording file.
        file: std::path::PathBuf,

        /// Output SVG file path (required).
        #[arg(short, long)]
        output: std::path::PathBuf,

        /// Capture the terminal state at this effective time (seconds).
        /// Cannot be used with --animated.
        #[arg(long, default_value_t = 0.0, conflicts_with = "animated")]
        at: f64,

        /// Font size in pixels.
        #[arg(long, default_value_t = 14.0)]
        font_size: f64,

        /// Overwrite output file if it already exists.
        #[arg(long)]
        force: bool,

        /// Produce an animated SVG (CSS keyframes). Mutually exclusive with --at.
        #[arg(long)]
        animated: bool,

        /// Override the 120-second duration limit for animated SVG.
        #[arg(long)]
        force_long: bool,
    },
    /// Export a recording as an animated GIF.
    Gif {
        /// Path to the asciicast recording file.
        file: std::path::PathBuf,

        /// Output GIF file path (required).
        #[arg(short, long)]
        output: std::path::PathBuf,

        /// Frames per second (max 50).
        #[arg(long, default_value_t = 10)]
        fps: u32,

        /// Scale factor for pixel dimensions.
        #[arg(long, default_value_t = 1)]
        scale: u32,

        /// Loop count (0 = infinite).
        #[arg(long = "loop", default_value_t = 0)]
        loop_count: u16,

        /// Overwrite output file if it already exists.
        #[arg(long)]
        force: bool,
    },
}

// ---------------------------------------------------------------------------
// Terminal setup / teardown
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Export helpers
// ---------------------------------------------------------------------------

/// Open a recording file or stdin into a [`speedrun_core::Player`].
fn load_player(file: &std::path::Path, opts: speedrun_core::LoadOptions) -> speedrun_core::Player {
    let reader: Box<dyn Read> = if file.as_os_str() == "-" {
        if std::io::stdin().is_terminal() {
            eprintln!("No input piped — did you mean to specify a file?");
            std::process::exit(1);
        }
        let mut buf = Vec::new();
        std::io::stdin().read_to_end(&mut buf).unwrap_or_else(|e| {
            eprintln!("Cannot read from stdin: {e}");
            std::process::exit(1);
        });
        Box::new(std::io::Cursor::new(buf))
    } else {
        Box::new(std::fs::File::open(file).unwrap_or_else(|e| {
            match e.kind() {
                std::io::ErrorKind::NotFound => {
                    eprintln!("File not found: {}", file.display());
                }
                _ => {
                    eprintln!("Cannot read file: {}: {e}", file.display());
                }
            }
            std::process::exit(1);
        }))
    };

    speedrun_core::Player::load_with(reader, opts).unwrap_or_else(|e| {
        eprintln!("Invalid recording: {e}");
        std::process::exit(1);
    })
}

/// Open the output file for writing, respecting the `--force` flag.
fn open_output(path: &std::path::Path, force: bool) -> std::fs::File {
    if path.exists() && !force {
        eprintln!(
            "Output file already exists: {}. Use --force to overwrite.",
            path.display()
        );
        std::process::exit(1);
    }
    std::fs::File::create(path).unwrap_or_else(|e| {
        eprintln!("Cannot create output file {}: {e}", path.display());
        std::process::exit(1);
    })
}

#[cfg(feature = "export")]
fn run_export_svg(
    file: std::path::PathBuf,
    output: std::path::PathBuf,
    at: f64,
    font_size: f64,
    force: bool,
    animated: bool,
    force_long: bool,
) {
    use speedrun_export::svg::{
        AnimatedSvgOptions, ExportError, SvgOptions, export_animated_svg, export_svg,
    };

    let mut player = load_player(
        &file,
        speedrun_core::LoadOptions {
            idle_limit: None,
            keyframe_interval: 5.0,
        },
    );
    for warning in player.warnings() {
        eprintln!("warning: line {}: {}", warning.line_number, warning.message);
    }

    let mut out_file = open_output(&output, force);

    if animated {
        let opts = AnimatedSvgOptions {
            font_size,
            force_long,
            ..Default::default()
        };
        export_animated_svg(&mut player, &opts, &mut out_file).unwrap_or_else(|e| {
            match &e {
                ExportError::TooLong(_) => {
                    eprintln!("Error: {e}");
                    eprintln!("Tip: use --force-long to override, or export as GIF instead.");
                }
                _ => eprintln!("Export error: {e}"),
            }
            std::process::exit(1);
        });
    } else {
        let opts = SvgOptions {
            at_time: at,
            font_size,
            ..Default::default()
        };
        export_svg(&mut player, &opts, &mut out_file).unwrap_or_else(|e| {
            eprintln!("Export error: {e}");
            std::process::exit(1);
        });
    }

    eprintln!("Wrote {}", output.display());
}

#[cfg(feature = "export")]
fn run_export_gif(
    file: std::path::PathBuf,
    output: std::path::PathBuf,
    fps: u32,
    scale: u32,
    loop_count: u16,
    force: bool,
) {
    use speedrun_export::gif::{GifOptions, export_gif};

    let mut player = load_player(
        &file,
        speedrun_core::LoadOptions {
            idle_limit: None,
            keyframe_interval: 5.0,
        },
    );
    for warning in player.warnings() {
        eprintln!("warning: line {}: {}", warning.line_number, warning.message);
    }

    let mut out_file = open_output(&output, force);

    let opts = GifOptions {
        fps,
        scale,
        loop_count,
        ..Default::default()
    };

    export_gif(
        &mut player,
        &opts,
        &mut out_file,
        Some(&|current, total| {
            if current % 100 == 0 || current == total {
                eprintln!("Rendering frame {current}/{total}...");
            }
        }),
    )
    .unwrap_or_else(|e| {
        eprintln!("Export error: {e}");
        std::process::exit(1);
    });

    eprintln!("Wrote {}", output.display());
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() {
    install_panic_hook();

    let args = Args::parse();

    // Handle export subcommands
    if let Some(Command::Export { format }) = args.command {
        match format {
            ExportFormat::Svg {
                file,
                output,
                at,
                font_size,
                force,
                animated,
                force_long,
            } => {
                #[cfg(feature = "export")]
                run_export_svg(file, output, at, font_size, force, animated, force_long);

                #[cfg(not(feature = "export"))]
                {
                    let _ = (file, output, at, font_size, force, animated, force_long);
                    eprintln!("Export feature not enabled. Rebuild with --features export.");
                    std::process::exit(1);
                }
            }
            ExportFormat::Gif {
                file,
                output,
                fps,
                scale,
                loop_count,
                force,
            } => {
                #[cfg(feature = "export")]
                run_export_gif(file, output, fps, scale, loop_count, force);

                #[cfg(not(feature = "export"))]
                {
                    let _ = (file, output, fps, scale, loop_count, force);
                    eprintln!("Export feature not enabled. Rebuild with --features export.");
                    std::process::exit(1);
                }
            }
        }
        return;
    }

    // Playback mode
    let file = args.file.unwrap_or_else(|| {
        eprintln!("Usage: speedrun <file.cast>");
        std::process::exit(1);
    });

    let reader: Box<dyn Read> = if file.as_os_str() == "-" {
        // Must check is_terminal BEFORE reading
        if std::io::stdin().is_terminal() {
            eprintln!("No input piped — did you mean to specify a file?");
            std::process::exit(1);
        }
        let mut buf = Vec::new();
        std::io::stdin().read_to_end(&mut buf).unwrap_or_else(|e| {
            eprintln!("Cannot read from stdin: {e}");
            std::process::exit(1);
        });
        Box::new(std::io::Cursor::new(buf))
    } else {
        Box::new(std::fs::File::open(&file).unwrap_or_else(|e| {
            match e.kind() {
                std::io::ErrorKind::NotFound => {
                    eprintln!("File not found: {}", file.display());
                }
                _ => {
                    eprintln!("Cannot read file: {}: {e}", file.display());
                }
            }
            std::process::exit(1);
        }))
    };

    let opts = speedrun_core::LoadOptions {
        idle_limit: args.idle_limit,
        keyframe_interval: args.keyframe_interval,
    };

    let mut player = speedrun_core::Player::load_with(reader, opts).unwrap_or_else(|e| {
        eprintln!("Invalid recording: {e}");
        std::process::exit(1);
    });

    for warning in player.warnings() {
        eprintln!("warning: line {}: {}", warning.line_number, warning.message);
    }

    player.set_speed(args.speed);

    if let Some(t) = args.start_at {
        player.seek(t);
    }

    player.play();

    let mut terminal = setup_terminal().unwrap_or_else(|e| {
        eprintln!("Failed to initialize terminal: {e}");
        std::process::exit(1);
    });

    let mut app = app::App::new(player, !args.no_controls);
    let result = app.run(&mut terminal);

    // Always restore terminal, even if run() returned an error
    let _ = restore_terminal(&mut terminal);

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn args_accepts_dash_as_file() {
        let args = Args::try_parse_from(["speedrun", "-"]).expect("should accept '-' as file arg");
        assert_eq!(
            args.file.as_deref().map(|p| p.as_os_str()),
            Some(std::ffi::OsStr::new("-"))
        );
    }
}
