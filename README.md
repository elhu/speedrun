# speedrun

A modern terminal session player for asciicast recordings with instant seeking and full playback control.

## Features

- **Instant seeking** -- jump to any point in a recording with O(log n) keyframe-based seeking
- **Scrubbing** -- arrow keys and percent jumps for fluid navigation
- **Idle compression** -- cap long pauses with configurable idle time limits
- **Speed control** -- adjustable playback speed from 0.25x to 8x
- **Frame stepping** -- advance or rewind one output event at a time
- **Marker navigation** -- jump between named markers in the recording
- **Asciicast v2 and v3** -- supports both format versions, with lenient parsing for corrupted files

## Installation

```sh
cargo install speedrun
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
cat recording.cast | speedrun -
```

### Keyboard Controls

| Key              | Action                  |
|------------------|-------------------------|
| `Space`          | Toggle play/pause       |
| `Left` / `Right` | Seek backward/forward 5s |
| `Shift+Left/Right` | Seek backward/forward 30s |
| `.` / `,`        | Step forward/backward one frame |
| `+` / `-`        | Speed up / slow down    |
| `[` / `]`        | Previous / next marker  |
| `0`-`9`          | Jump to 0%-90%          |
| `g` / `G`        | Jump to start / end     |
| `Tab`            | Toggle controls bar     |
| `?`              | Toggle help overlay     |
| `q` / `Esc`      | Quit                    |

## Library Usage

The `speedrun-core` crate can be used as a library for programmatic access to asciicast recordings:

```rust
use speedrun_core::Player;

// Load a recording from any reader
let data = b"{\"version\":2,\"width\":80,\"height\":24}\n[0.5,\"o\",\"$ hello\\r\\n\"]";
let mut player = Player::load(&data[..]).unwrap();

// Seek to a point in the recording
player.seek(0.5);

// Read terminal screen content
let first_line = player.screen()[0].text();
assert!(first_line.starts_with("$ hello"));

// Check recording metadata
println!("Duration: {:.1}s", player.duration());
println!("Terminal size: {:?}", player.size());
```

## License

MIT
