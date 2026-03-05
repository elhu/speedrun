use clap::Parser;

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

fn main() {
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

    println!(
        "Loaded: {}x{}, duration {:.1}s",
        player.size().0,
        player.size().1,
        player.duration()
    );
}
