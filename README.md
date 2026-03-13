# speedrun

A modern terminal session player for [asciicast](https://docs.asciinema.org/manual/asciicast/v2/) recordings. Think of it as a video player for your terminal -- jump to any point, scrub through the timeline, adjust playback speed, and step through output frame by frame.

## Acknowledgements

speedrun would not exist without [asciinema](https://asciinema.org/). The asciinema project pioneered terminal session recording and sharing, and its asciicast format is the foundation that speedrun builds on. If you haven't already, check out asciinema -- it's a wonderful tool for recording and sharing terminal sessions.

## Features

- **Instant seeking** -- jump to any point in a recording with fast keyframe-based seeking
- **Scrubbing** -- arrow keys and percent jumps for fluid navigation
- **Frame stepping** -- advance or rewind one output event at a time
- **Speed control** -- adjustable playback speed from 0.25x to 30x
- **Idle compression** -- cap long pauses with configurable idle time limits
- **Text search** -- search through terminal output and jump between matches
- **Marker navigation** -- jump between named markers embedded in the recording
- **Export** -- render recordings to SVG (static or animated), GIF, or MP4
- **Asciicast v2 and v3** -- supports both format versions, with lenient parsing for corrupted files
- **Stdin support** -- pipe recordings directly into speedrun

## Building from source

Requires [Rust](https://www.rust-lang.org/tools/install) (edition 2024).

```sh
# Clone and build
git clone https://github.com/anomalyco/speedrun.git
cd speedrun
cargo build --release

# The binary is at target/release/speedrun
```

To install it into your Cargo bin directory:

```sh
cargo install --path crates/speedrun
```

### Export support

Export functionality (SVG, GIF, MP4) is enabled by default. MP4 export requires `ffmpeg` to be installed on your system. To build without export support:

```sh
cargo build --release --no-default-features
```

## Usage

```sh
# Play a recording
speedrun recording.cast

# Play at double speed
speedrun recording.cast --speed 2.0

# Start at a specific timestamp
speedrun recording.cast --start-at 30.0

# Cap idle pauses to 2 seconds
speedrun recording.cast --idle-limit 2.0

# Read from stdin
curl https://example.com/demo.cast | speedrun -
```

### Keyboard controls

| Key | Action |
|-----|--------|
| `Space` | Play / pause |
| `Left` / `Right` | Seek backward / forward 5s |
| `Shift+Left` / `Shift+Right` | Seek backward / forward 30s |
| `.` / `,` | Step forward / backward one frame |
| `+` / `-` | Speed up / slow down |
| `]` / `[` | Next / previous marker |
| `0`-`9` | Jump to 0%-90% of the recording |
| `g` / `G` | Jump to start / end |
| `/` | Search terminal output |
| `n` / `N` | Next / previous search match |
| `Tab` | Toggle controls bar |
| `?` | Show keybinding help |
| `q` / `Esc` | Quit |

### Exporting recordings

speedrun can render recordings to shareable formats:

```sh
# Animated SVG
speedrun export svg recording.cast -o output.svg

# Static SVG screenshot at a specific time
speedrun export svg recording.cast -o output.svg --at 5.0

# Animated GIF
speedrun export gif recording.cast -o output.gif --fps 10 --scale 2

# MP4 video (requires ffmpeg)
speedrun export mp4 recording.cast -o output.mp4 --fps 30

# Use --force to overwrite existing files
speedrun export gif recording.cast -o output.gif --force
```

## Library usage

The `speedrun-core` crate can be used independently for programmatic access to asciicast recordings:

```rust
use speedrun_core::Player;

// Load a recording from any Read impl
let data = std::fs::File::open("recording.cast").unwrap();
let mut player = Player::load(data).unwrap();

// Seek and read terminal state
player.seek(5.0);
let first_line = player.screen()[0].text();

// Recording metadata
println!("Duration: {:.1}s", player.duration());
println!("Terminal size: {:?}", player.size());
```

## Project structure

```
crates/
├── speedrun-core/     Core engine: parsing, indexing, seeking, playback
├── speedrun-export/   SVG, GIF, and MP4 export
└── speedrun/          TUI player (ratatui + crossterm)
```

## License

MIT
