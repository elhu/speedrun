# speedrun

> A terminal session player with instant seeking, scrubbing, and cross-device playback.

## Problem

Terminal session recordings (`.cast` files produced by asciinema) are widely used for demos, documentation, and tutorials. But playback is stuck in the VHS era: no seeking, no scrubbing, no progress bar, no speed control beyond a fixed multiplier. If you want to jump to the 2-minute mark of a 5-minute recording, you wait.

`speedrun` is a modern player for terminal recordings that treats `.cast` files the way a video player treats `.mp4` files — with full random access, a visual timeline, and instant seeking.

## Non-goals

- **Recording.** Use `asciinema rec`. It works. The asciicast format is an open standard with wide ecosystem support. We don't replace the recorder.
- **Re-executing commands.** We replay _what was displayed_, not what was run. Recordings are portable across devices and operating systems by design.
- **Hosting/sharing platform.** asciinema.org exists. We're a player, not a service.
- **Web player (v1).** A WASM-based web player is a natural future extension, but v1 ships as a TUI only.

## Design principles

1. **Zero-config for existing recordings.** Any `.cast` file (asciicast v2 or v3) should just work.
2. **Seek-first architecture.** Seeking to an arbitrary point in a recording should feel instant (<50ms for typical recordings).
3. **Cross-platform playback.** Record on macOS, play on Linux. The recording is the portable artifact.
4. **Composable.** The core engine is a library; the TUI player is one consumer of it.

## Technical decisions

| Decision | Choice | Rationale |
|---|---|---|
| Language | **Rust** | `avt` is native Rust; `wasm-bindgen` gives us a web path later; single binary distribution. |
| Terminal emulator | **`avt`** (committed) | Apache 2.0, battle-tested across the asciinema ecosystem, purpose-built for exactly this use case (parsing + virtual buffer, no rendering). No abstraction layer — we depend on `avt` directly. If libghostty-vt becomes compelling later, the migration surface is internal to `speedrun-core`. |
| TUI framework | **`ratatui` + `crossterm`** | `avt` cell grid maps naturally to ratatui cells. Ratatui gives us the overlay/layout system for the controls bar for free. `crossterm` is the backend (pure Rust, no ncurses dependency, works on macOS/Linux/Windows). |
| Rendering approach | **ratatui custom widget** | A `TerminalView` widget that maps `avt`'s cell grid (chars + attributes) to ratatui `Cell` values. The controls bar is a separate ratatui widget rendered conditionally as an overlay. This is cleaner than raw ANSI and lets ratatui handle diffing (only redraw changed cells). |
| Event loop | **Event-driven with deadline** | No fixed tick rate. The loop sleeps until `min(next_event_timestamp, input_poll_timeout)`. On wake: process any user input, advance playback to now, re-render if the frame changed. This avoids both wasted CPU (no spinning at 60fps when nothing changes) and missed events (no 16ms quantization). `crossterm::event::poll` with a timeout handles this naturally. |
| Keyframe granularity | **Fixed 5-second interval** | Simple, predictable, good enough. For 80×24 terminals at ~30 bytes/cell, each keyframe is ~58KB. A 10-minute recording = 120 keyframes ≈ 7MB. Acceptable. No adaptive logic, no memory cap — keep it simple for v1. |
| Mouse support | **None (v1)** | Keyboard only. No mouse capture, no click-to-seek on the progress bar. Avoids complexity around tmux mouse passthrough, terminal compatibility, and the interaction model for the overlay controls. Can be added later without architectural changes. |
| v1 scope | **`speedrun-core` library + `speedrun` TUI player** | Two crates in a Cargo workspace. No web player, no export tools, no sidecar cache. |

### Open verification: avt `Vt` cloneability

The keyframe strategy depends on how we snapshot `avt`'s terminal state. The ideal path is `Vt: Clone` — cloning the entire `Vt` instance at each keyframe interval captures all state (both screen buffers, cursor, parser state) with zero risk of missing something. This needs to be verified by running `cargo doc` on avt at implementation start.

If `Vt` does **not** implement `Clone`, the fallback is: at each keyframe, read every cell from avt's screen API (which we know exists — agg uses it to render GIFs) into our own `TerminalSnapshot` struct. On restore, create a fresh `Vt` and replay events from the keyframe's event index. This is slightly more code but architecturally identical. The public API doesn't change either way.

## Architecture

### Input format

speedrun reads standard asciicast v2 and v3 files (`.cast`). These are newline-delimited JSON:

```
{"version": 2, "width": 80, "height": 24, "timestamp": 1504467315}
[0.248, "o", "\u001b[1;31mHello \u001b[32mWorld!\u001b[0m\n"]
[1.001, "o", "That was ok\rThis is better."]
[2.143, "o", "Now... "]
[6.541, "o", "Bye!"]
```

Line 1 is a header with terminal dimensions and metadata. Each subsequent line is `[timestamp, event_type, data]`. Event type `"o"` is output (the vast majority), `"i"` is input, `"m"` is a marker, `"r"` is a resize.

asciicast v3 is similar but uses relative timestamps (intervals between events) instead of absolute times. speedrun normalizes both to absolute timestamps internally.

No custom format. No conversion step. Standard `.cast` files in, enhanced playback out.

### Time model: raw vs effective time

A recording's raw timestamps may include long idle gaps (e.g. the user pausing to think for 30 seconds between commands). Both asciinema's header (`idle_time_limit`) and speedrun's `--idle-limit` flag cap these gaps during playback. This means the timeline the user sees ("effective time") is shorter than the raw recorded time.

This isn't just a cosmetic concern — it affects the entire system. The progress bar, the duration display, seek targets, and keyframe timestamps must all operate on effective time. Otherwise, seeking to "50%" of a 10-minute recording that compresses to 3 minutes would land at completely the wrong content.

The solution is a **time mapping layer** built during indexing:

```
Raw events:     [0.0s] [1.0s] [31.0s] [32.0s] [62.0s]
                  │       │       │        │        │
                  │       │   30s gap → capped to 2s │
                  │       │       │        │        │
Effective time: [0.0s] [1.0s]  [3.0s]  [4.0s]  [6.0s]
```

Each event gets both a raw timestamp and an effective timestamp. The idle limit is resolved once at load time with the following precedence:

1. `--idle-limit` CLI flag (explicit override, highest priority)
2. `idle_time_limit` in the `.cast` header (author's intent)
3. No limit (raw timestamps used as-is)

All public API surfaces — `current_time()`, `duration()`, `seek()`, keyframe timestamps, the progress bar — use effective time. Raw timestamps are internal to the event list and only used when replaying through `avt` (which doesn't care about timing, just byte order).

### Core: virtual terminal + keyframe index

The key insight enabling instant seeking: we don't need keyframes baked into the recording file. Instead, we build them at load time.

On load, speedrun:

1. Parses the `.cast` file into an in-memory event list (normalizing v3 relative timestamps to absolute).
2. Computes effective timestamps by applying the idle limit to each event's raw timestamp.
3. Replays all output events through `avt`, which maintains a virtual terminal grid.
4. Every 5 seconds of **effective time**, snapshots the full terminal grid state as a **keyframe**.
5. Stores keyframes in a seekable index alongside the event list.

To seek to effective time T:
- Binary search the keyframe index for the nearest keyframe at or before T.
- Restore `avt`'s terminal state from that keyframe's snapshot.
- Replay output events from the keyframe's event index to T through `avt`.

For a typical 5-minute recording with keyframes every 5s, seeking requires replaying at most 5 seconds of events — which `avt` processes in microseconds.

### Keyframe structure

```rust
struct Keyframe {
    /// Effective timestamp (with idle limit applied)
    time: f64,
    /// Index into the event list (first event after this keyframe)
    event_index: usize,
    /// Serialized terminal state from avt
    terminal_state: TerminalSnapshot,
}

struct TerminalSnapshot {
    /// Full cell grid: (width × height) cells, each with char + fg/bg color + attributes
    cells: Vec<Cell>,
    /// Cursor position and visibility
    cursor: CursorState,
    /// Which buffer is active (primary vs alternate)
    active_buffer: Buffer,
    /// Terminal dimensions at this point (may differ from header if resizes occurred)
    width: u16,
    height: u16,
}
```

### Event loop (TUI)

```
loop {
    let timeout = player.time_to_next_event()
        .unwrap_or(Duration::from_millis(100));  // 100ms idle poll when paused or at end

    if crossterm::event::poll(timeout) {
        handle_input(crossterm::event::read());
    }

    if player.is_playing() {
        let changed = player.tick(elapsed_since_last_tick());
        if changed {
            render_frame();
        }
    }
}
```

`crossterm::event::poll(timeout)` is the timer mechanism: it returns when either user input arrives or the timeout elapses — whichever comes first. No async runtime needed. `time_to_next_event()` computes the effective-time gap to the next event, scaled by playback speed, giving us precise wakeup timing without a fixed tick rate.

## Components

### 1. `speedrun-core` (library crate)

The engine. No UI, no terminal I/O, no filesystem access beyond what the caller provides.

Responsibilities:
- **Parser**: reads asciicast v2/v3 from a `Read` impl into an event list.
- **Time mapper**: applies idle limit to produce effective timestamps for each event.
- **Indexer**: replays events through `avt`, builds keyframe index at 5-second effective-time intervals.
- **Seek engine**: given an effective time, restores terminal state from nearest keyframe + replay.
- **Playback controller**: manages playback state (play/pause, speed, current time), advances on `tick()`.

Public API:

```rust
/// Load and play asciicast recordings with instant seeking.
pub struct Player { /* ... */ }

pub struct LoadOptions {
    /// Cap idle time between events (seconds). Overrides the header's idle_time_limit.
    /// None means use the header value, or no limit if the header doesn't specify one.
    pub idle_limit: Option<f64>,
}

impl Default for LoadOptions {
    fn default() -> Self { Self { idle_limit: None } }
}

impl Player {
    /// Parse a recording and build the keyframe index.
    /// This replays the entire recording through avt (fast — typically <100ms).
    pub fn load(reader: impl Read) -> Result<Self>;
    /// Load with explicit options (e.g. idle limit override).
    pub fn load_with(reader: impl Read, opts: LoadOptions) -> Result<Self>;

    // -- Playback control --

    pub fn play(&mut self);
    pub fn pause(&mut self);
    pub fn toggle(&mut self);
    pub fn is_playing(&self) -> bool;

    /// Seek to an absolute effective time (seconds). Clamped to [0, duration].
    pub fn seek(&mut self, time: f64);
    /// Seek relative to current position in effective time. Negative values seek backward.
    pub fn seek_relative(&mut self, delta: f64);

    /// Set playback speed multiplier (e.g. 0.5, 1.0, 2.0).
    pub fn set_speed(&mut self, speed: f64);
    pub fn speed(&self) -> f64;

    // -- Frame access --

    /// Advance playback by `dt` seconds of wall-clock time (scaled by speed).
    /// Returns true if the terminal state changed (caller should re-render).
    /// When the end of the recording is reached, auto-pauses and returns true
    /// (for the final frame). Subsequent calls return false.
    pub fn tick(&mut self, dt: f64) -> bool;

    /// Effective time until the next output event, accounting for speed.
    /// Returns None if paused or at end of recording.
    /// This is the value the TUI should pass to `crossterm::event::poll()`.
    pub fn time_to_next_event(&self) -> Option<Duration>;

    /// Current terminal cell grid for rendering.
    pub fn screen(&self) -> &avt::Screen;
    /// Current cursor position and visibility.
    pub fn cursor(&self) -> CursorState;

    // -- Metadata --

    /// Current position in effective time (with idle limit applied).
    pub fn current_time(&self) -> f64;
    /// Total duration in effective time.
    pub fn duration(&self) -> f64;
    /// Recorded terminal dimensions (current — may change on resize events).
    pub fn size(&self) -> (u16, u16);
    /// Markers embedded in the recording (for chapter navigation).
    pub fn markers(&self) -> &[Marker];

    // -- Stepping (when paused) --

    /// Advance to the next output event. Returns false if at end.
    pub fn step_forward(&mut self) -> bool;
    /// Step back to the previous output event. Returns false if at start.
    /// Note: this is internally a seek (avt is forward-only), so it replays
    /// from the nearest keyframe. Still fast — bounded by keyframe interval.
    pub fn step_backward(&mut self) -> bool;
}

pub struct Marker {
    pub time: f64,
    pub label: String,
}
```

Dependencies: `avt`, `serde` + `serde_json` (for parsing NDJSON). Nothing else.

### 2. `speedrun` (TUI binary crate)

A terminal-based player. Consumes `speedrun-core`.

```
speedrun demo.cast
speedrun demo.cast --speed 2.0
speedrun demo.cast --start-at 30
```

#### Viewport sizing

The recorded terminal dimensions (e.g. 80×24) must be preserved exactly. The output contains absolute cursor positions and full-screen apps (vim, htop, etc.) that assume the original geometry. `avt` always runs at the recorded dimensions — we never resize the virtual terminal.

The controls bar is rendered as an **overlay**, not as a region that shrinks the viewport:

- **During playback**: controls auto-hide after 2 seconds of inactivity. Full undistorted recording visible.
- **On pause, end of recording, or any navigation keypress**: controls appear, overlaying the bottom row.
- **`Tab`**: manually toggle controls visibility.
- See **Controls behavior** below for full details on auto-hide, initial state, and end-of-recording.

If the user's terminal is **larger** than the recording, the recording is rendered at its original size and the controls sit in the extra space below — no overlay needed.

If the user's terminal is **smaller** than the recording, the viewport scrolls to follow the cursor position within the recorded grid. Controls overlay as above.

```
 During playback (controls hidden):          On pause (controls shown):
┌──────────────────────────────────┐   ┌──────────────────────────────────┐
│ $ cargo build                    │   │ $ cargo build                    │
│    Compiling speedrun v0.1.0     │   │    Compiling speedrun v0.1.0     │
│    Finished dev [unoptimized]    │   │    Finished dev [unoptimized]    │
│ $ _                              │   │ $ _                              │
│                                  │   │                                  │
│                                  │   │                                  │
│                                  │   ├──────────────────────────────────┤
│                                  │   │ ▮▮ 1:23 / 5:00  ████░░░░  1.0× │
└──────────────────────────────────┘   └──────────────────────────────────┘

 At end of recording:                       Help overlay (? key):
┌──────────────────────────────────┐   ┌──────────────────────────────────┐
│ $ cargo build                    │   │                                  │
│    Compiling speedrun v0.1.0     │   │  ┌────── Keybindings ────────┐  │
│    Finished dev [unoptimized]    │   │  │ Space   play / pause      │  │
│ $ _                              │   │  │ ← →     seek ±5s         │  │
│                                  │   │  │ . ,     step fwd / back  │  │
│                                  │   │  │ + -     speed up / down  │  │
├──────────────────────────────────┤   │  │ 0-9     jump to 0%-90%   │  │
│ ■  5:00 / 5:00  ████████  1.0×  │   │  │ q       quit             │  │
└──────────────────────────────────┘   │  └────────────────────────────┘  │
                                       └──────────────────────────────────┘
```

#### Rendering

The TUI player implements a custom ratatui `Widget` (`TerminalView`) that:

1. Reads the cell grid from `player.screen()`.
2. Maps each `avt` cell to a `ratatui::buffer::Cell` (character, foreground, background, modifiers).
3. Handles the viewport offset when the host terminal is smaller than the recording.

The controls bar is a second widget rendered conditionally on top. ratatui's `Frame::render_widget` with a calculated `Rect` handles the overlay positioning.

Color mapping: `avt` provides colors as indexed (0–255) or RGB. ratatui's `Color` type supports both. The mapping is direct — no theme translation needed. The host terminal's color scheme determines how indexed colors actually look, which is the correct behavior (recordings adapt to the viewer's theme, just like `asciinema play` does).

#### Controls bar

The controls bar is a single-row overlay showing playback state at a glance.

```
 ▶ 1:23 / 5:00   advancement advancement 1.0×   advancement advancement marker

 ║    ║       ║                            ║                ║
 ║    ║       ║                            ║                ╚══ marker indicator
 ║    ║       ║                            ╚══ speed
 ║    ║       ╚══ progress bar (filled █ / empty ░)
 ║    ╚══ current time / total duration
 ╚══ state icon
```

State icon values:
- `▶` — playing
- `║` — paused (stylized double bar, or `▮▮` if unicode support is limited)
- `■` — stopped (at end of recording)

When there are markers in the recording, small tick marks (`│`) appear on the progress bar at the corresponding positions.

When the user's terminal is wider than the controls need, the bar is centered within the recording's width. When narrower, the progress bar shrinks first; below a minimum width, the speed indicator is dropped, then the duration.

#### Controls behavior

**Initial state:** playback starts immediately on launch (like `asciinema play`). The controls bar is visible for 2 seconds, then auto-hides. If `--start-at` is specified, playback begins from that position. If the user passes `--no-controls`, the controls bar starts hidden and only appears on explicit `Tab`.

**End of recording:** playback auto-pauses. The state icon changes to `■`. The controls bar appears (if hidden). The user can seek backward, restart with `0`, or quit.

**Speed cycling:** speeds follow a fixed set: `0.25×`, `0.5×`, `1×`, `1.5×`, `2×`, `4×`. `+`/`=` moves up the list, `-` moves down. Wrapping: hitting `+` at `4×` stays at `4×`; hitting `-` at `0.25×` stays at `0.25×`.

**Seeking boundary behavior:** seeking before `0` clamps to `0`. Seeking past the end clamps to the final event and auto-pauses.

**Stepping:** only available when paused. `step_forward` at the end of the recording does nothing. `step_backward` at the start does nothing. When stepping, the controls bar shows and remains visible.

#### Keybindings

**Playback**

| Key | Action |
|---|---|
| `Space` | Toggle play / pause |
| `+` or `=` | Speed up (next in 0.25× → 0.5× → 1× → 1.5× → 2× → 4×) |
| `-` | Slow down (previous in the same set) |

**Navigation**

| Key | Action |
|---|---|
| `→` | Seek forward 5s |
| `←` | Seek backward 5s |
| `Shift+→` | Seek forward 30s |
| `Shift+←` | Seek backward 30s |
| `.` | Step forward one output event (when paused) |
| `,` | Step backward one output event (when paused) |
| `]` | Jump to next marker |
| `[` | Jump to previous marker |
| `0`–`9` | Jump to 0%–90% of recording duration |
| `Home` or `g` | Jump to start (0:00) |
| `End` or `G` | Jump to end (final frame, auto-pause) |

**Display**

| Key | Action |
|---|---|
| `Tab` | Toggle controls bar visibility |
| `?` | Show/dismiss help overlay (keybinding reference) |

**Application**

| Key | Action |
|---|---|
| `q` or `Esc` | Quit |

#### Help overlay

Pressing `?` shows a centered overlay listing all keybindings, grouped as above. Pressing `?` again or `Esc` dismisses it. Playback pauses while the help overlay is visible and resumes (if it was playing before) on dismiss.

#### CLI interface

```
speedrun <file.cast> [OPTIONS]

Arguments:
    <file.cast>    Path to an asciicast v2 or v3 recording

Options:
    -s, --speed <SPEED>       Playback speed multiplier [default: 1.0]
    -t, --start-at <SECONDS>  Start playback at this timestamp
    -i, --idle-limit <SECS>   Cap idle time between events [default: from .cast header]
    --no-controls              Start with controls hidden
    -h, --help                 Print help
    -V, --version              Print version
```

## Project structure

```
speedrun/
├── Cargo.toml              # workspace
├── crates/
│   ├── speedrun-core/
│   │   ├── Cargo.toml      # deps: avt, serde, serde_json
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── parser.rs   # asciicast v2/v3 parsing
│   │       ├── timemap.rs  # raw → effective time mapping (idle limit)
│   │       ├── index.rs    # keyframe index building
│   │       ├── player.rs   # playback controller + seek engine
│   │       └── snapshot.rs # terminal state snapshots
│   └── speedrun/
│       ├── Cargo.toml      # deps: speedrun-core, ratatui, crossterm, clap
│       └── src/
│           ├── main.rs
│           ├── app.rs      # event loop + state
│           ├── ui.rs       # ratatui rendering (TerminalView + controls)
│           └── input.rs    # keymap handling
├── testdata/               # sample .cast files for testing
└── README.md
```

## Dependencies

| Crate | Used by | Purpose | License |
|---|---|---|---|
| `avt` | core | Virtual terminal emulation | Apache 2.0 |
| `serde` | core | JSON deserialization | MIT/Apache 2.0 |
| `serde_json` | core | NDJSON parsing | MIT/Apache 2.0 |
| `ratatui` | tui | Terminal UI framework | MIT |
| `crossterm` | tui | Terminal I/O backend | MIT |
| `clap` | tui | CLI argument parsing | MIT/Apache 2.0 |

## Recording workflow (unchanged)

```bash
# Record with asciinema (user's existing tool)
asciinema rec demo.cast

# Play with speedrun (new)
speedrun demo.cast

# Still works with asciinema too — same file, same format
asciinema play demo.cast
```

## Future directions (out of scope for v1)

- **Web player.** Compile `speedrun-core` to WASM via `wasm-bindgen`. Render to `<canvas>` with a JS controls UI. Ship as an npm package and/or a `<speedrun-player>` web component. The core API is already designed to be WASM-friendly (no filesystem, no threads, caller provides `Read`).
- **Mouse support.** Click-to-seek on the progress bar, scroll to change speed. Needs careful handling of tmux/screen mouse passthrough.
- **Text search.** `/` to search for a string across the recording, jumping to the timestamp where it appears on screen. Scan keyframe snapshots first, then narrow down between keyframes.
- **Thumbnail strip.** Generate a visual preview strip from keyframe snapshots for the web player.
- **Recording trimming.** `speedrun cut --from 10s --to 30s demo.cast` — simple NDJSON event filtering.
- **Live streaming playback.** Connect to an asciinema stream endpoint and play in real-time.
- **GIF / video export.** Render to animated GIF or MP4 with speed/region control.
- **Keyframe sidecar cache.** Write pre-computed keyframes to `demo.cast.idx` for instant load on very long recordings.

