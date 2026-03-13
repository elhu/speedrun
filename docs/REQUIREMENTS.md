# speedrun — Requirements

> Derived from [SPEC.md](./SPEC.md). This document covers architecture, phasing, setup, and acceptance criteria. Each phase is designed to be refined into one or more epics for parallel development.

---

## Table of contents

- [Overview](#overview)
- [Architecture](#architecture)
- [Technology stack](#technology-stack)
- [Project structure](#project-structure)
- [Phase 0 — Project setup](#phase-0--project-setup)
- [Phase 1 — Core library (`speedrun-core`)](#phase-1--core-library-speedrun-core)
- [Phase 2 — TUI shell (`speedrun`)](#phase-2--tui-shell-speedrun)
- [Phase 3 — Full TUI features](#phase-3--full-tui-features)
- [Phase 4 — Polish and hardening](#phase-4--polish-and-hardening)
- [Out of scope (v1)](#out-of-scope-v1)
- [Later phases (post-v1)](#later-phases-post-v1)

---

## Overview

speedrun is a modern terminal session player that provides instant seeking, scrubbing, and full playback control for asciicast recordings (`.cast` files). It treats terminal recordings the way a video player treats video files — with random access, a visual timeline, and speed control.

**What we're building for v1:**
- `speedrun-core`: A Rust library crate providing parsing, indexing, seeking, and playback control for asciicast v2/v3 recordings.
- `speedrun`: A TUI binary crate providing a keyboard-driven terminal player with controls overlay, help screen, and full navigation.

**Design principles:**
1. Zero-config for existing recordings — any `.cast` file (v2 or v3) just works.
2. Seek-first architecture — seeking to any point feels instant (<50ms).
3. Cross-platform playback — record on macOS, play on Linux.
4. Composable — the core engine is a library; the TUI is one consumer.

---

## Architecture

### High-level data flow

```
.cast file → Parser → Event list (raw timestamps)
                          │
                     Time mapper (idle limit)
                          │
                     Event list (effective timestamps)
                          │
                     Indexer (replay through avt, snapshot every 5s)
                          │
                     Keyframe index + Event list
                          │
                     Player (seek, tick, playback control)
                          │
                     Screen state (avt cell grid)
                          │
                     TUI renderer (ratatui TerminalView widget)
```

### Core concepts

**Event list:** All events from the `.cast` file, parsed into structs with both raw and effective timestamps. Events are output (`o`), input (`i`), marker (`m`), and resize (`r`).

**Time model:** Recordings have raw timestamps (as recorded) and effective timestamps (with idle gaps capped). The idle limit is resolved once at load time:
1. `--idle-limit` CLI flag (highest priority)
2. `idle_time_limit` in `.cast` header
3. No limit (raw = effective)

All public API surfaces use effective time. Raw timestamps are internal.

**Keyframe index:** Built at load time by replaying all output events through `avt`. At a configurable interval (default: 5 seconds) of effective time, a full terminal state snapshot is captured. Seeking to time T = restore nearest keyframe + replay events from keyframe to T. The keyframe interval is configurable via `LoadOptions` in the API and `--keyframe-interval` on the CLI, allowing users to tune the memory/seek-latency tradeoff.

**Keyframe structure:**
```rust
struct Keyframe {
    time: f64,                          // Effective timestamp
    event_index: usize,                 // First event after this keyframe
    terminal_state: TerminalSnapshot,   // Full terminal state
}

struct TerminalSnapshot {
    cells: Vec<Cell>,       // width × height cell grid
    cursor: CursorState,    // Position + visibility
    active_buffer: Buffer,  // Primary vs alternate
    width: u16,
    height: u16,
}
```

**Event loop:** Event-driven with deadline. No fixed tick rate. The loop sleeps until `min(next_event_timestamp, input_poll_timeout)`. On wake: process input, advance playback, re-render if changed. `crossterm::event::poll(timeout)` is the timer mechanism.

### avt snapshot strategy

The keyframe strategy depends on `avt::Vt` cloneability:

- **Preferred path:** If `Vt: Clone`, clone the entire `Vt` instance at each keyframe interval. This captures all state (both screen buffers, cursor, parser state) with zero risk of missing something. Snapshots store the cloned `Vt` directly.
- **Fallback path:** If `Vt` does not implement `Clone`, build our own `TerminalSnapshot` type that reads every cell from avt's screen API into an owned cell grid, cursor state, and active buffer. On restore, create a fresh `Vt` and replay events from the keyframe's event index. This abstraction layer decouples our snapshot format from avt, which also makes a future VT backend migration (e.g., libghostty-vt) cheaper.

This must be verified as the first task of Phase 1 (see Epic 1.1). The public API and architecture are identical either way. The chosen approach is documented in `snapshot.rs`.

---

## Technology stack

### Language and toolchain

| Item | Choice |
|---|---|
| Language | Rust |
| Edition | 2024 |
| MSRV | Latest stable (no pinned minimum) |
| Build system | Cargo workspace |

### Dependencies

| Crate | Used by | Purpose | License |
|---|---|---|---|
| `avt` | speedrun-core | Virtual terminal emulation | Apache 2.0 |
| `serde` | speedrun-core | JSON deserialization | MIT/Apache 2.0 |
| `serde_json` | speedrun-core | NDJSON parsing | MIT/Apache 2.0 |
| `ratatui` | speedrun | Terminal UI framework | MIT |
| `crossterm` | speedrun | Terminal I/O backend | MIT |
| `clap` | speedrun | CLI argument parsing (derive) | MIT/Apache 2.0 |

### Dev dependencies

| Crate | Purpose |
|---|---|
| `insta` | Snapshot testing for terminal state |
| `tempfile` | Temporary files in tests |
| `pretty_assertions` | Readable test diffs |

### Tooling

| Tool | Purpose |
|---|---|
| `rustfmt` | Code formatting (default config) |
| `clippy` | Linting (deny warnings) |
| Pre-commit hooks | `cargo fmt --check`, `cargo clippy`, `cargo test` |

---

## Project structure

```
speedrun/
├── Cargo.toml                  # Workspace root
├── docs/
│   ├── SPEC.md                 # Original specification
│   ├── REQUIREMENTS.md         # This document
│   └── agents.md               # Agent onboarding guide (provided separately)
├── crates/
│   ├── speedrun-core/
│   │   ├── Cargo.toml          # deps: avt, serde, serde_json
│   │   └── src/
│   │       ├── lib.rs          # Public API re-exports
│   │       ├── parser.rs       # Asciicast v2/v3 parsing
│   │       ├── timemap.rs      # Raw → effective time mapping
│   │       ├── index.rs        # Keyframe index building
│   │       ├── player.rs       # Playback controller + seek engine
│   │       └── snapshot.rs     # Terminal state snapshots
│   └── speedrun/
│       ├── Cargo.toml          # deps: speedrun-core, ratatui, crossterm, clap
│       └── src/
│           ├── main.rs         # Entry point, CLI parsing
│           ├── app.rs          # Event loop + application state
│           ├── ui.rs           # Ratatui rendering (TerminalView + controls)
│           └── input.rs        # Keymap handling
├── testdata/                   # Sample .cast files
│   ├── minimal_v2.cast         # Minimal v2 recording (few events)
│   ├── minimal_v3.cast         # Minimal v3 recording
│   ├── empty.cast              # Header only, no events
│   ├── with_markers.cast       # Recording containing marker events
│   ├── with_resize.cast        # Recording containing resize events
│   ├── long_idle.cast          # Recording with long idle gaps (tests idle limit)
│   ├── alternate_buffer.cast   # Recording that uses alternate screen buffer
│   ├── real_session.cast       # A real-world recording for integration tests
│   └── invalid/                # Invalid files for error handling tests
│       ├── empty_file.cast     # Zero bytes
│       ├── no_header.cast      # Event lines but no header
│       ├── bad_json.cast       # Invalid JSON on header line
│       ├── bad_version.cast    # Valid JSON header but unknown version (e.g. 99)
│       ├── missing_fields.cast # Header missing required fields (width/height)
│       ├── bad_event.cast      # Valid header + malformed event lines
│       ├── binary_garbage.cast # Binary/non-text content
│       └── truncated.cast      # Valid header + partial/truncated event line
└── README.md
```

---

## Phase 0 — Project setup

**Goal:** A buildable, testable, linted workspace with CI-equivalent local checks and test data ready for development.

### Epic 0.1 — Workspace scaffold

Create the Cargo workspace with both crates, minimal `lib.rs` / `main.rs` stubs that compile, and workspace-level configuration.

**Requirements:**
- Workspace `Cargo.toml` at root with `crates/speedrun-core` and `crates/speedrun` as members.
- `speedrun-core` has placeholder `lib.rs` exporting nothing. `Cargo.toml` declares dependencies: `avt`, `serde`, `serde_json`.
- `speedrun` has placeholder `main.rs` that prints a message and exits. `Cargo.toml` declares dependencies: `speedrun-core` (path), `ratatui`, `crossterm`, `clap` (with derive feature).
- Dev dependencies added to workspace: `insta`, `tempfile`, `pretty_assertions`.
- `cargo build` and `cargo test` pass with zero warnings.
- Rust edition 2024 in both crates.

### Epic 0.2 — Tooling and pre-commit hooks

Set up formatting, linting, and pre-commit hooks.

**Requirements:**
- `rustfmt.toml` at workspace root (default config or minimal customization).
- `clippy.toml` or workspace-level `[lints]` configuration denying warnings.
- Pre-commit hook that runs: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`.
- Pre-commit hook is installed via a setup script or documented setup step (not git-managed hooks that need manual symlinking).
- `cargo fmt` and `cargo clippy` pass on the scaffold.

### Epic 0.3 — Test data

Create synthetic `.cast` files for testing, including both valid and invalid files.

**Requirements:**

**Valid test files** (hand-crafted and minimal, human-readable, few events):
- `minimal_v2.cast`: Valid v2 header + 3-5 output events with varying timestamps.
- `minimal_v3.cast`: Valid v3 header + 3-5 output events with relative (interval) timestamps.
- `empty.cast`: Valid header, zero events.
- `with_markers.cast`: V2 file with at least 2 marker (`m`) events interspersed with output.
- `with_resize.cast`: V2 file with at least 1 resize (`r`) event.
- `long_idle.cast`: V2 file with a 30+ second gap between two events. Header includes `idle_time_limit: 2`.
- `alternate_buffer.cast`: V2 file with escape sequences that switch to alternate screen buffer and back.
- `real_session.cast`: A real asciinema recording (can be generated or sourced). At least 30 seconds long.

**Invalid test files** (in `testdata/invalid/`, for error handling tests):
- `empty_file.cast`: Zero bytes.
- `no_header.cast`: Event lines but no JSON header on the first line.
- `bad_json.cast`: Invalid JSON on the header line (e.g., missing closing brace).
- `bad_version.cast`: Valid JSON header but with an unknown version number (e.g., `"version": 99`).
- `missing_fields.cast`: Header missing required fields (e.g., no `width` or `height`).
- `bad_event.cast`: Valid header followed by malformed event lines (bad JSON, wrong array length, etc.).
- `binary_garbage.cast`: Binary/non-text content (random bytes).
- `truncated.cast`: Valid header followed by a partial/truncated event line (simulates incomplete write).

### Phase 0 acceptance criteria

- [ ] `cargo build --workspace` succeeds with zero warnings.
- [ ] `cargo test --workspace` succeeds (even if no tests exist yet).
- [ ] `cargo fmt --check` passes.
- [ ] `cargo clippy -- -D warnings` passes.
- [ ] Pre-commit hook is functional and blocks commits that fail fmt/clippy/test.
- [ ] All valid test data files exist and contain valid JSON (parseable headers).
- [ ] All invalid test data files exist in `testdata/invalid/` and are intentionally malformed as described.
- [ ] `agents.md` placeholder exists in `docs/`.

---

## Phase 1 — Core library (`speedrun-core`)

**Goal:** A fully functional library that can load any asciicast v2/v3 file, build a keyframe index, and provide instant seeking and playback control — with no UI dependency.

### Epic 1.1 — avt exploration and snapshot strategy

Verify `avt` API surface and decide on the snapshot approach.

**Requirements:**
- Run `cargo doc -p avt --open` and examine the `Vt` type.
- Determine whether `Vt` implements `Clone`.
- Document findings in a brief comment at the top of `snapshot.rs` (which approach is used and why).

**If `Vt: Clone` (preferred path):**
- Snapshots store a cloned `Vt` directly. `TerminalSnapshot` wraps the cloned `Vt`.
- Restoration is trivial: clone the snapshot's `Vt` back.
- This captures all state (both screen buffers, cursor, parser/decoder state) with zero risk of missing anything.

**If `Vt: !Clone` (fallback path):**
- Build our own `TerminalSnapshot` type that owns the terminal state independently of avt.
- Read every cell from avt's screen API into an owned cell grid (`Vec<Cell>` with char, fg, bg, attributes).
- Capture cursor position/visibility, active buffer (primary vs alternate), and terminal dimensions.
- Restoration: create a fresh `Vt` and replay events from the keyframe's event index forward.
- This abstraction decouples snapshot format from avt internals, making a future VT backend migration (e.g., libghostty-vt) cheaper.

**In either case:**
- Implement snapshot restoration: given a `TerminalSnapshot`, produce a usable terminal state for rendering and continued replay.
- Unit tests: snapshot a `Vt` after feeding it known escape sequences, restore it, verify cell contents match.

**Error handling:**
- Clearly typed errors for snapshot failures (e.g., if avt API doesn't expose expected state).

### Epic 1.2 — Asciicast parser

Parse asciicast v2 and v3 files into an in-memory event list.

**Requirements:**
- Parse from any `impl Read` (no filesystem dependency).
- V2 format: first line is JSON header object, subsequent lines are `[timestamp, event_type, data]` arrays. Timestamps are absolute (seconds as f64).
- V3 format: first line is JSON header object (with `"version": 3`), subsequent lines use relative timestamps (intervals). Normalize to absolute timestamps during parsing.
- Header fields parsed: `version`, `width`, `height`, `timestamp` (optional), `idle_time_limit` (optional), `title` (optional), `env` (optional).
- Event types supported: `o` (output), `i` (input), `m` (marker), `r` (resize).
- Events stored in order with raw absolute timestamps.
- Markers extracted and stored separately with their labels.

**Error handling:**
- Invalid JSON → descriptive error with line number.
- Unknown version → error with version found.
- Missing required header fields (`version`, `width`, `height`) → error naming the missing field.
- Malformed event lines → error with line number and content.
- Graceful handling of trailing newlines, empty lines.

**Tests:**
- Unit: Parse each valid test data file, verify event counts, timestamps, dimensions.
- Unit: Parse v3 file, verify timestamps are normalized to absolute.
- Unit: Error cases using `testdata/invalid/` files:
  - `empty_file.cast` → appropriate empty/EOF error.
  - `no_header.cast` → error about missing/invalid header.
  - `bad_json.cast` → JSON parse error with line number.
  - `bad_version.cast` → unknown version error.
  - `missing_fields.cast` → error naming the missing field.
  - `bad_event.cast` → error with line number for malformed events.
  - `binary_garbage.cast` → detected early, clear error.
  - `truncated.cast` → graceful handling of incomplete data.
- Snapshot: Parse `minimal_v2.cast`, snapshot the resulting event list.

### Epic 1.3 — Time mapper

Apply idle time limits to produce effective timestamps.

**Requirements:**
- Takes an event list (with raw timestamps) and an idle limit configuration.
- Idle limit resolution: explicit override > header value > no limit.
- Produces effective timestamps for each event: walk events in order, if gap between consecutive events exceeds the idle limit, cap it.
- Effective duration = effective timestamp of last event.
- The mapping is computed once at load time and stored alongside events.
- Bidirectional lookups: effective → raw (for seeking to a raw event index), raw → effective (less common but needed for resize handling).

**Error handling:**
- Idle limit of 0 or negative → error.
- Empty event list → effective duration of 0, no error.

**Tests:**
- Unit: `long_idle.cast` with header idle limit → verify effective timestamps, duration compression.
- Unit: Same file with explicit override of 1s → verify different effective timestamps.
- Unit: No idle limit → effective = raw.
- Unit: Multiple consecutive idle gaps.
- Snapshot: Effective timestamp list for `long_idle.cast`.

### Epic 1.4 — Keyframe indexer

Build the keyframe index by replaying events through `avt`.

**Requirements:**
- Takes the event list (with effective timestamps) and a keyframe interval (default: 5 seconds) and builds keyframes.
- Replay all output events through `avt::Vt` in order.
- At every `keyframe_interval` seconds of effective time, capture a `TerminalSnapshot` (using the strategy from Epic 1.1).
- Always capture a keyframe at effective time 0 (initial state).
- Store keyframes in a `Vec<Keyframe>` sorted by effective time.
- Each keyframe records: effective time, event index (first event after the keyframe), terminal snapshot.
- The keyframe interval is configurable via `LoadOptions::keyframe_interval` (default: 5.0 seconds). Lower values trade memory for faster seeks; higher values trade seek latency for lower memory.
- Handle resize events: `avt` should be told about new dimensions. The snapshot captures current dimensions.
- Handle alternate buffer switches (avt manages this internally).
- Performance target: indexing a 10-minute recording should complete in <500ms.

**Error handling:**
- avt errors during replay → propagate with context (event index, timestamp).

**Tests:**
- Unit: Index `minimal_v2.cast` → verify keyframe count (expected: 1 at t=0 for short recordings).
- Unit: Index a longer recording → verify keyframes at 0s, 5s, 10s, etc. (default interval).
- Unit: Index with custom keyframe interval (e.g., 2s) → verify keyframe count changes accordingly.
- Unit: Verify keyframe event indices point to correct events.
- Snapshot: Keyframe metadata (times, event indices) for a test recording.

### Epic 1.5 — Seek engine and playback controller

Implement the `Player` struct with seeking, playback, and the full public API.

**Requirements:**

**Loading:**
- `Player::load(reader)` and `Player::load_with(reader, opts)` parse, map time, and build index.
- `LoadOptions` includes: `idle_limit: Option<f64>` (overrides header), `keyframe_interval: f64` (default: 5.0).
- Load is synchronous and fast (<100ms for typical recordings, <500ms for 10-minute recordings).

**Playback control:**
- `play()`, `pause()`, `toggle()`, `is_playing()` — standard state machine.
- `set_speed(speed)`, `speed()` — speed multiplier. Valid range: 0.25 to 30.0. Values outside range are clamped.
- `tick(dt)` — advance playback by `dt` wall-clock seconds (scaled by speed). Returns `true` if terminal state changed. Auto-pauses at end of recording.
- `time_to_next_event()` — effective-time gap to next event, scaled by speed. Returns `None` if paused or at end.

**Seeking:**
- `seek(time)` — absolute seek in effective time. Clamped to [0, duration].
- `seek_relative(delta)` — relative seek. Negative values seek backward.
- Implementation: binary search keyframe index, restore snapshot, replay events from keyframe to target time.
- Seeking while playing continues playback from new position. Seeking while paused stays paused.

**Stepping:**
- `step_forward()` — advance to next output event. Only when paused. Returns `false` at end.
- `step_backward()` — step to previous output event. Internally a seek. Returns `false` at start.

**Frame access:**
- `screen()` — returns avt's current screen (cell grid).
- `cursor()` — cursor position and visibility.

**Metadata:**
- `current_time()` — effective time position.
- `duration()` — total effective duration.
- `size()` — current terminal dimensions.
- `markers()` — list of markers with effective times and labels.

**Error handling:**
- All load errors are typed and descriptive.
- Seeking/stepping on an unloaded player is not possible (Player is only constructed via load).
- Speed values outside [0.25, 30.0] are clamped, not errored.

**Tests:**
- Unit: Load `minimal_v2.cast`, verify duration, size, event traversal via tick.
- Unit: Seek to various positions, verify `current_time()` and screen contents.
- Unit: Seek backward (requires keyframe restore + replay), verify correctness.
- Unit: Speed changes affect tick advancement correctly.
- Unit: Auto-pause at end of recording.
- Unit: Step forward/backward, verify screen state at each step.
- Unit: `time_to_next_event()` returns correct durations at various playback positions and speeds.
- Integration: Load `real_session.cast`, seek to 50%, verify non-empty screen.
- Snapshot: Screen state at t=0, t=duration/2, t=duration for test recordings.

### Phase 1 acceptance criteria

- [ ] `Player::load()` successfully loads all test data files (v2, v3, with markers, with resizes, with idle gaps).
- [ ] Seeking to any point in a recording completes in <50ms.
- [ ] `tick()` correctly advances playback and reports state changes.
- [ ] Stepping forward/backward produces correct terminal states.
- [ ] Idle limit (header and override) correctly compresses effective time.
- [ ] All error cases produce descriptive, typed errors.
- [ ] `cargo test -p speedrun-core` passes with all tests green.
- [ ] No UI or filesystem dependencies in `speedrun-core` (only `Read` trait).

---

## Phase 2 — TUI shell (`speedrun`)

**Goal:** A working terminal player that can load a `.cast` file, render it to the terminal, and support basic playback controls. Not fully featured — just enough to play, pause, seek, and see the recording.

### Epic 2.1 — CLI and application bootstrap

Set up the CLI interface and application lifecycle.

**Requirements:**
- CLI parsed with `clap` (derive API).
- Arguments and options as specified in the spec:
  - `<file.cast>` — required positional argument.
  - `-s, --speed <SPEED>` — default 1.0.
  - `-t, --start-at <SECONDS>` — optional.
  - `-i, --idle-limit <SECS>` — optional.
  - `--keyframe-interval <SECS>` — keyframe snapshot interval in seconds (default: 5.0). Lower values increase memory but make seeks faster.
  - `--no-controls` — flag.
  - `-h, --help` and `-V, --version`.
- Application lifecycle: enter raw mode → enter alternate screen → run event loop → restore terminal on exit (including on panic).
- Panic hook that restores terminal state before printing panic info.
- `--start-at` seeks to the specified position after load. Playback starts immediately.
- File loading errors produce a clear message on stderr and exit with non-zero code.

**Error handling:**
- File not found → "File not found: <path>".
- File not readable → "Cannot read file: <path>: <os error>".
- Parse error → "Invalid recording: <parse error details>".
- Invalid CLI arguments → clap's default error messages.

**Tests:**
- Unit: CLI parsing with various argument combinations.
- Integration: Run binary with `--help`, verify exit code 0.
- Integration: Run binary with nonexistent file, verify exit code != 0 and error message.

### Epic 2.2 — Terminal view widget

Implement the ratatui `TerminalView` widget that renders avt's cell grid.

**Requirements:**
- Custom ratatui `Widget` implementation.
- Maps each `avt` cell to a `ratatui::buffer::Cell`:
  - Character (including wide characters / multi-cell characters).
  - Foreground color: avt indexed (0-255) → ratatui `Color::Indexed`. avt RGB → ratatui `Color::Rgb`.
  - Background color: same mapping.
  - Attributes: bold, dim, italic, underline, blink, reverse, strikethrough → ratatui `Modifier` flags.
- Cursor rendering: if cursor is visible, render it at the correct position (using reverse video or a distinct style).
- Viewport handling when host terminal is larger than recording: recording renders at its original size, positioned at top-left.
- Viewport handling when host terminal is smaller than recording: viewport follows cursor position. Scroll offsets track which portion of the recording grid is visible.

**Error handling:**
- Recording larger than available area → viewport scrolling, no error.
- Zero-size terminal area → skip rendering, no crash.

**Tests:**
- Snapshot: Render a known avt state to a ratatui `Buffer`, compare cell contents.
- Unit: Color mapping correctness (indexed, RGB, default).
- Unit: Attribute mapping (bold, italic, etc.).
- Unit: Viewport offset calculation for various host/recording size combinations.

### Epic 2.3 — Event loop and basic playback

Wire up the event loop with keyboard input and playback.

**Requirements:**
- Event-driven loop as described in the spec: `poll(timeout)` → handle input → tick → render.
- Timeout computed from `player.time_to_next_event()`, fallback 100ms.
- Playback starts immediately on launch.
- Keybindings (basic subset for this phase):
  - `Space` — toggle play/pause.
  - `q` or `Esc` — quit.
  - `→` — seek forward 5s.
  - `←` — seek backward 5s.
- Re-render only when terminal state changes (tick returns true) or on user input.
- On terminal resize: re-render with new layout. Recording dimensions stay the same (avt not resized).

**Error handling:**
- Crossterm event read error → log and continue (transient terminal errors shouldn't crash).
- Render error → exit gracefully with error message.

**Tests:**
- Integration: Launch with a test file, verify it doesn't crash (smoke test).
- Unit: Event loop timeout calculation for various player states (playing, paused, at end).

### Phase 2 acceptance criteria

- [ ] `speedrun demo.cast` launches, shows the recording, and plays it to completion.
- [ ] Space pauses/resumes. Arrow keys seek. q quits.
- [ ] Terminal is properly restored on exit (including Ctrl-C and panic).
- [ ] Colors and text attributes from the recording render correctly.
- [ ] Viewport scrolls when host terminal is smaller than recording.
- [ ] `--speed`, `--start-at`, `--idle-limit`, `--keyframe-interval` CLI options work correctly.
- [ ] No visual artifacts or flickering during playback.

---

## Phase 3 — Full TUI features

**Goal:** Complete the TUI with the controls bar, help overlay, all keybindings, and the full UX described in the spec.

### Epic 3.1 — Controls bar widget

Implement the controls bar overlay.

**Requirements:**
- Single-row widget showing: state icon, current time / total duration, progress bar, speed indicator.
- State icons: `▶` (playing), `▮▮` (paused), `■` (stopped/at end).
- Progress bar: filled (`█`) and empty (`░`) segments proportional to current_time/duration.
- Marker ticks: small `│` marks on the progress bar at marker positions.
- Speed display: e.g. `1.0×`, `2.0×`.
- Time format: `M:SS` for times under an hour, `H:MM:SS` for longer.
- Responsive layout: progress bar shrinks first when space is tight. Below minimum width, speed indicator drops, then duration.
- Overlay positioning: rendered over the bottom row of the recording area. When host terminal is larger than recording, rendered below the recording (no overlay needed).
- Styling: distinct background color to visually separate from recording content.

**Error handling:**
- Terminal too narrow for even minimal controls → hide controls entirely, no crash.

**Tests:**
- Snapshot: Controls bar rendering at various widths with known player state.
- Unit: Time formatting (seconds → M:SS, H:MM:SS).
- Unit: Progress bar width calculation and marker positioning.
- Unit: Responsive layout breakpoints.

### Epic 3.2 — Controls bar behavior and auto-hide

Implement the visibility logic for the controls bar.

**Requirements:**
- **Initial state:** Controls visible for 2 seconds, then auto-hide. If `--no-controls`, start hidden.
- **Auto-hide during playback:** Controls hide after 2 seconds of no user input.
- **Show on interaction:** Any navigation keypress (seek, step, speed change) shows controls. Controls auto-hide timer resets.
- **Show on pause:** Pausing shows controls. They remain visible while paused.
- **Show at end:** When recording ends (auto-pause), controls appear.
- **Manual toggle:** `Tab` toggles visibility. When manually hidden, stays hidden until next interaction or Tab.
- Timer implementation: track `last_interaction_time`. On each render, if playing and `now - last_interaction > 2s`, hide controls.

**Tests:**
- Unit: Visibility state machine transitions (play → idle → hide, pause → show, tab → toggle, etc.).

### Epic 3.3 — Complete keybindings

Implement all remaining keybindings from the spec.

**Requirements:**

**Speed control:**
- `+` or `=` — speed up (next in: 0.25×, 0.5×, 1×, 1.5×, 2×, 4×, 10×, 20×, 30×). Clamp at 30×.
- `-` — slow down (previous in same set). Clamp at 0.25×.

**Navigation:**
- `Shift+→` — seek forward 30s.
- `Shift+←` — seek backward 30s.
- `.` — step forward one output event (when paused).
- `,` — step backward one output event (when paused).
- `]` — jump to next marker.
- `[` — jump to previous marker.
- `0`–`9` — jump to 0%–90% of duration.
- `Home` or `g` — jump to start.
- `End` or `G` — jump to end (auto-pause).

**Display:**
- `Tab` — toggle controls bar (handled in Epic 3.2).
- `?` — show/dismiss help overlay (handled in Epic 3.4).

**Implementation:**
- Key handling in `input.rs`: match crossterm `KeyEvent` to actions. Actions are an enum dispatched by the app.
- Stepping only works when paused — ignore `.` and `,` when playing.
- Marker navigation: find next/previous marker relative to current effective time.

**Error handling:**
- Key events that don't match any binding → ignored silently.
- Modifier combinations not listed → ignored.

**Tests:**
- Unit: Key event → action mapping for all bindings.
- Unit: Speed cycling (up from 1× → 1.5×, down from 1× → 0.5×, clamp at boundaries).
- Unit: Marker navigation with various marker positions.
- Unit: Percentage jump calculation (key `5` → seek to 50% of duration).

### Epic 3.4 — Help overlay

Implement the `?` help overlay.

**Requirements:**
- Centered overlay listing all keybindings, grouped by category (Playback, Navigation, Display, Application).
- Styled with a border and title ("Keybindings").
- `?` toggles the overlay. `Esc` also dismisses it.
- Playback pauses while help is visible. Resumes (if it was playing) on dismiss.
- Help overlay renders on top of everything (recording + controls).

**Error handling:**
- Terminal too small for help overlay → render as much as fits, or show a "terminal too small" message.

**Tests:**
- Snapshot: Help overlay rendering at a standard terminal size.
- Unit: Playback state preservation (was playing → pause → dismiss → resume).

### Phase 3 acceptance criteria

- [ ] Controls bar displays correctly with all elements (icon, time, progress, speed, markers).
- [ ] Controls auto-hide after 2s during playback, show on pause/interaction/end.
- [ ] `Tab` toggles controls. `--no-controls` starts hidden.
- [ ] All keybindings from the spec are functional.
- [ ] Speed cycling works through the full set (0.25× to 30×).
- [ ] Marker navigation jumps correctly.
- [ ] Percentage jumps (0-9 keys) land at correct positions.
- [ ] Help overlay shows/dismisses correctly, pauses/resumes playback.
- [ ] Controls bar adapts responsively to narrow terminals.

---

## Phase 4 — Polish and hardening

**Goal:** Edge case handling, performance verification, final UX polish, and comprehensive test coverage.

### Epic 4.1 — Edge cases and error UX

**Requirements:**
- Empty recording (header only, no events): load succeeds, duration is 0, controls show `■ 0:00 / 0:00`. No crash.
- Recording with only input events (no output): load succeeds, blank screen, duration reflects event span.
- Very long recordings (1+ hour): indexing completes in reasonable time. Memory usage proportional to keyframe count.
- Very short recordings (<1 second): single keyframe at t=0. Playback and seeking work correctly.
- Recordings with many resize events: viewport handles dimension changes. Keyframe snapshots capture each size.
- Malformed `.cast` files: partial parse succeeds if header is valid and at least some events parse. Malformed lines are skipped with a warning on stderr.
- Binary / garbage files: detected early, clear error message.
- Piped input: `cat demo.cast | speedrun -` reads from stdin (stretch goal — not required for v1, but design should not preclude it).

**Tests:**
- Unit/integration test for each edge case listed above.
- Fuzz testing with malformed inputs (stretch goal).

### Epic 4.2 — Performance verification

**Requirements:**
- Benchmark: load + index time for recordings of 1min, 5min, 10min, 30min.
- Benchmark: seek time (worst case: seek from start to end).
- Verify <50ms seek for typical recordings.
- Verify <100ms load for typical recordings, <500ms for 10-minute recordings.
- Memory profiling: keyframe index memory usage is reasonable (~58KB per keyframe for 80×24 terminal).
- CPU during idle playback: near zero (event-driven loop, no spinning).
- If performance targets are not met, optimize (profile-guided).

**Tests:**
- Benchmarks using `criterion` or similar.
- Performance regression tests (track load/seek times).

### Epic 4.3 — Final UX polish

**Requirements:**
- Clean startup: no flicker on launch. First frame renders immediately after load.
- Clean shutdown: terminal fully restored. No leftover raw mode or alternate screen.
- Ctrl-C handling: graceful exit with terminal restoration.
- SIGWINCH (terminal resize): re-render with new layout. No crash, no artifacts.
- Color accuracy: verify rendering matches `asciinema play` for the same recording on the same terminal.
- Cursor blink: match avt's cursor state. Don't show cursor when recording doesn't.
- Progress bar accuracy: bar position matches current_time / duration precisely.
- Smooth speed transitions: changing speed mid-playback doesn't cause jumps.

**Tests:**
- Manual test checklist (documented).
- Automated smoke tests where possible.

### Phase 4 acceptance criteria

- [ ] All edge case recordings load and play without crashes.
- [ ] Performance targets met: <50ms seek, <100ms load (typical), <500ms load (10-min).
- [ ] Memory usage is proportional and reasonable.
- [ ] Terminal is always properly restored on exit.
- [ ] No visual artifacts during playback, seeking, speed changes, or resize.
- [ ] All tests pass, including snapshot tests and integration tests.
- [ ] `cargo clippy -- -D warnings` passes.

---

## Out of scope (v1)

These are explicitly **not** included in v1:

- **Recording** — use `asciinema rec`.
- **Re-executing commands** — we replay display output only.
- **Hosting/sharing** — asciinema.org exists.
- **Web player** — WASM build is a future phase.
- **Mouse support** — keyboard only. No click-to-seek, no scroll.
- **Text search** — searching recording content by string.
- **Live streaming** — real-time stream playback.
- **Export** — GIF, MP4, or other format export.
- **Keyframe sidecar cache** — pre-computed `.cast.idx` files.

---

## Later phases (post-v1)

These are natural extensions that the architecture supports but are deferred:

### Phase 5 — Marker authoring and auto-pause

Three epics covering marker playback behavior, core authoring infrastructure, and TUI integration.

#### Epic 5.1 — Marker playback behavior

Enhances how markers affect navigation and playback. No file I/O, no new input modes.

**Ticket 5.1.1 — Virtual marker navigation boundaries**

Modify `[`/`]` so they always have a destination, even with zero markers.

Changes:
- `app.rs` `Action::NextMarker` handler: if no real marker found after current time, `seek(duration)` and pause (same as `JumpToEnd` behavior).
- `app.rs` `Action::PrevMarker` handler: if no real marker found before current time, `seek(0.0)`.
- `help.rs`: update `] [` description to mention start/end boundaries.

Acceptance criteria:
- [ ] `]` when no markers exist seeks to recording end.
- [ ] `[` when no markers exist seeks to recording start.
- [ ] `]` when past the last marker seeks to recording end.
- [ ] `[` when before the first marker seeks to recording start.
- [ ] `]` when between markers still jumps to next real marker (existing behavior preserved).
- [ ] `[` when between markers still jumps to previous real marker (existing behavior preserved).
- [ ] Help overlay text updated.
- [ ] Existing marker navigation tests still pass.
- [ ] New unit tests cover all six cases above.

**Ticket 5.1.2 — Auto-pause at markers (CLI flag + detection)**

Add `--pause-at-markers` flag and marker crossing detection in the event loop.

Changes:
- `main.rs`: add `-m, --pause-at-markers` to `Args` struct, pass to `App::new()`.
- `app.rs`: add `pause_at_markers: bool` field to `App`, update constructor.
- `app.rs` event loop (`run()`): before `tick()`, capture `prev_time`; after `tick()`, check if any marker `m` satisfies `prev_time < m.time <= current_time`; if so, `player.pause()` and seek to the marker's exact time.
- Detection only fires when `pause_at_markers` is `true` and `player.is_playing()` is `true`.
- Note: the `seek()` after detection is intentional — `tick()` may have processed events slightly past the marker, and `seek()` restores the exact terminal state at that point. Add a code comment explaining this.

Acceptance criteria:
- [ ] `speedrun --pause-at-markers file.cast` is accepted by CLI parser.
- [ ] `speedrun -m file.cast` short form works.
- [ ] During `tick()` playback, crossing a marker pauses playback at the marker's time.
- [ ] When multiple markers exist between prev_time and new_time, pauses at the first one.
- [ ] Seek (`←`/`→`), step (`.`/`,`), and `[`/`]` do NOT trigger auto-pause.
- [ ] `JumpToPercent`, `JumpToStart`, `JumpToEnd` do NOT trigger auto-pause.
- [ ] Without `--pause-at-markers`, markers are crossed silently (existing behavior).
- [ ] With flag but no markers in recording, playback runs normally.
- [ ] Unit test: marker crossing detected between two tick times.
- [ ] Unit test: no false positive when tick window doesn't cross a marker.

**Ticket 5.1.3 — Auto-pause UX and help integration**

Polish the auto-pause experience and document everything.

Changes:
- `app.rs`: on auto-pause, set `controls_force_show = true` and `controls_manually_hidden = false`.
- `app.rs`: after auto-pause, `Space` resumes normally and continues to next marker.
- `help.rs`: add `"Markers"` keybinding group between Navigation and Search. Include documentation of `--pause-at-markers` behavior.
- Update `insta` snapshot tests for the help overlay.
- Verify: auto-pause at last marker, then `Space` resumes, playback continues to end and auto-pauses at duration (existing end-of-recording behavior).

Acceptance criteria:
- [ ] Controls bar becomes visible immediately on auto-pause.
- [ ] `Space` after auto-pause resumes playback normally.
- [ ] Playback that resumes after auto-pause will pause again at the next marker.
- [ ] Auto-pause at last marker followed by `Space` plays to end, then auto-pauses at duration.
- [ ] Help overlay contains a "Markers" group.
- [ ] Help overlay `insta` snapshots updated and passing.
- [ ] `cargo clippy --workspace -- -D warnings` passes.
- [ ] `cargo fmt --check` passes.
- [ ] All existing tests pass.

#### Epic 5.2 — Marker authoring core

Core library support for creating markers and converting them to asciicast format. No file I/O (stays WASM-compatible).

**Ticket 5.2.1 — Parser: accept out-of-order events**

The v2 parser currently skips events with timestamps earlier than the preceding event (`parser.rs:483-496`), producing a warning. Marker authoring appends marker events to the end of the `.cast` file with raw times that may be earlier than the last event. Without this fix, appended markers are silently dropped on reload.

Changes:
- `parser.rs`: remove the v2 monotonicity skip. Instead of `continue` on out-of-order timestamps, accept the event (still emit a warning for diagnostics).
- `parser.rs`: after the event parsing loop, sort `events` by `time` (stable sort to preserve file order for same-timestamp events).
- `parser.rs`: re-derive `markers` from the sorted events list (or sort `markers` separately).
- `timemap.rs`: no changes needed — `TimeMap::build()` receives sorted raw times.

Acceptance criteria:
- [ ] A `.cast` file with an appended out-of-order marker event parses successfully.
- [ ] The marker appears in `recording.markers` at the correct sorted position.
- [ ] The marker appears in `recording.events` at the correct sorted position.
- [ ] A warning is still emitted for out-of-order timestamps (non-breaking diagnostic).
- [ ] Existing test files with properly ordered events produce identical parse results.
- [ ] New test: file with marker appended after last event but with earlier timestamp.
- [ ] `cargo clippy --workspace -- -D warnings` passes.

**Ticket 5.2.2 — Reverse time mapping (effective → raw)**

Add the ability to convert effective times back to raw times in `TimeMap`, using the "uncapped delta" strategy.

Changes:
- `timemap.rs`: store `raw_times: Vec<f64>` alongside `effective_times` in the `TimeMap` struct (populated during `build()`).
- `timemap.rs`: add `pub fn raw_time(&self, effective: f64) -> Option<f64>`.
- Algorithm: binary search `effective_times` for surrounding events. Compute `raw = prev_raw + (effective - prev_eff)`. This preserves the effective-time position on reload because the delta from the preceding event is always ≤ the idle limit (it came from an already-capped effective gap), so it will never be re-capped.
- Edge cases: before first event → `None`, at/after last event → `Some(last_raw)`, exact event match → exact raw time (via the same formula, no special case needed — avoids f64 equality issues).

Acceptance criteria:
- [ ] `raw_time(eff)` returns `Some(raw)` for any effective time within the recording range.
- [ ] `raw_time()` returns `None` for effective times before the first event.
- [ ] For recordings with no idle limit, `raw_time(t) == Some(t)` for all event times.
- [ ] For recordings with idle limit, round-trip holds: `effective_time(i)` → `raw_time()` returns the original raw time for every event index.
- [ ] Within an idle-capped gap: `raw_time(prev_eff + delta)` returns `prev_raw + delta` (NOT linear interpolation across the full raw gap).
- [ ] `TimeMap` memory increase is exactly `size_of::<f64>() * num_events`.
- [ ] All existing `TimeMap` tests still pass.
- [ ] New tests: no-limit round-trip, idle-limited round-trip, uncapped-delta in gap, before-first, at-last, empty map.

**Ticket 5.2.3 — Marker serialization and Player::add_marker()**

Add a `Player` method to create a marker at the current position, returning the serialized NDJSON line for the caller to persist.

Changes:
- `parser.rs`: add `pub fn serialize_marker_event(raw_time: f64, label: &str) -> String`. Returns `[<raw_time>,"m","<label>"]` using `serde_json::to_string` on a `(f64, &str, &str)` tuple for correct JSON escaping. Round `raw_time` to 6 decimal places to avoid floating-point noise (e.g., `3.5000000000000004`).
- `player.rs`: add `pub fn add_marker(&mut self, label: String) -> Option<String>`:
  1. Gets current effective time via `self.current_time`.
  2. Converts to raw via `self.time_map.raw_time()`.
  3. Creates `Marker { time: effective_time, label }` and inserts into `self.markers` in sorted order (binary search insertion).
  4. Returns `Some(serialized_ndjson_line)` on success, `None` if time conversion fails.
- `player.rs`: add `pub fn version(&self) -> u8` accessor (returns `self.recording.header.version`), used by TUI to gate v3 rejection.
- `lib.rs`: re-export `serialize_marker_event`.

Acceptance criteria:
- [ ] `serialize_marker_event(3.0, "chapter-1")` produces valid JSON: `[3.0,"m","chapter-1"]`.
- [ ] `serialize_marker_event(1.5, "")` produces `[1.5,"m",""]` (empty label is valid).
- [ ] Labels with quotes, backslashes, and unicode are properly JSON-escaped.
- [ ] Raw times are rounded to 6 decimal places (no floating-point noise).
- [ ] `player.add_marker("test".into())` inserts a marker at the current effective time.
- [ ] After `add_marker()`, `player.markers()` returns the new marker in sorted position.
- [ ] `add_marker()` returns `Some(line)` where `line` is valid NDJSON.
- [ ] Multiple `add_marker()` calls maintain sorted order.
- [ ] After `add_marker()`, `[`/`]` navigation finds the new marker immediately.
- [ ] Tests with idle-limited recordings verify correct raw time (uncapped delta, not interpolation).

#### Epic 5.3 — Marker authoring TUI

Wire marker creation into the TUI with file persistence, confirmation safety, and labeled marker input.

Depends on: Epic 5.2.

**Ticket 5.3.1 — Unlabeled marker authoring (`m` key)**

Add the `m` key to create unlabeled markers, with file modification confirmation and append-to-file persistence.

Changes:
- `input.rs`: add `Action::AddMarker` variant, map `KeyCode::Char('m')`.
- `app.rs`: add fields: `file_path: Option<PathBuf>`, `file_modify_confirmed: bool`, `marker_feedback: Option<Instant>`, `pending_marker_time: Option<f64>`.
- `main.rs`: pass `Some(file.clone())` to `App::new()` (or `None` for stdin).
- `app.rs`: add `InputMode::ConfirmFileModify` with pending action tracking.
- On `m` press: capture `pending_marker_time = Some(player.current_time())`. If playing, pause (store `was_playing_before_confirm`). If `file_path.is_none()` (stdin), show transient error and return. If not yet confirmed, enter `ConfirmFileModify` mode. If already confirmed, create marker immediately.
- Confirmation renders `"Append marker to <filename>? [y/N] "` on bottom row. `y` confirms and proceeds. `n`/`N`/`Esc` cancels. `q` and `Ctrl+C` still quit the application.
- On confirmed: call `player.add_marker("".into())` using the captured `pending_marker_time` (seek to it first if current time has drifted). Open file with `OpenOptions::new().append(true).open()`. Write NDJSON line followed by `\n`. Handle write errors as transient feedback, not panics.
- On cancel: resume playback if was playing.
- V3 files: check `player.version() == 3`, show transient error "Marker authoring not supported for v3 recordings" and return.

Acceptance criteria:
- [ ] Pressing `m` in Normal mode when not yet confirmed shows `[y/N]` confirmation prompt.
- [ ] Playback pauses during confirmation dialog.
- [ ] `y` at confirmation creates the marker and appends to file.
- [ ] `n`, `N`, or `Esc` at confirmation cancels without modifying the file.
- [ ] `q` and `Ctrl+C` during confirmation still quit the application.
- [ ] On cancel, playback resumes if it was playing before.
- [ ] After confirming once, subsequent `m` presses create markers immediately.
- [ ] The `.cast` file gains a new NDJSON line with correct format.
- [ ] No double-newline: file append handles trailing newline correctly.
- [ ] Marker appears on the progress bar immediately after creation.
- [ ] `[`/`]` navigation finds newly created markers.
- [ ] Marker is placed at the time when `m` was pressed, not when `y` confirmed.
- [ ] When reading from stdin, `m` shows transient error.
- [ ] V3 recordings show transient error, no file modification.
- [ ] File write errors shown as transient feedback, not panics.
- [ ] Tests: confirmation flow (accept/reject), file append (`tempfile`), stdin rejection, v3 rejection, marker-time-at-keypress.

**Ticket 5.3.2 — Labeled marker authoring (`M` key)**

Add the `M` key to create labeled markers with an inline text prompt.

Changes:
- `input.rs`: add `Action::AddLabeledMarker` variant, map `KeyCode::Char('M')`.
- `app.rs`: add `InputMode::MarkerLabelInput` mode and `marker_label_input: String` buffer.
- `Action::AddLabeledMarker` handler: capture `pending_marker_time`, pause playback. If stdin → error. If v3 → error. If not confirmed → `ConfirmFileModify` (on confirm, transition to `MarkerLabelInput`). If confirmed → enter `MarkerLabelInput` directly.
- `handle_marker_label_input()`: same pattern as `handle_search_input()`. `Enter` → create marker with label, append to file, return to Normal, resume if was playing. `Esc` → cancel, resume. `Backspace` / `Char(c)` as expected.
- Render: `"Marker label: <text>"` with cursor on bottom row.

Acceptance criteria:
- [ ] `M` (when confirmed) enters MarkerLabelInput mode.
- [ ] `M` (when not confirmed) shows `[y/N]`, then enters MarkerLabelInput on `y`.
- [ ] Playback pauses during label input.
- [ ] Text input renders on bottom row as `"Marker label: <text>"`.
- [ ] `Enter` with non-empty text creates a labeled marker and appends to file.
- [ ] `Enter` with empty text creates an unlabeled marker (empty string label).
- [ ] `Esc` cancels input, no marker created, resumes playback if was playing.
- [ ] `Backspace` removes the last character.
- [ ] Special characters in label are properly JSON-escaped in the file.
- [ ] Marker is placed at the time when `M` was pressed, not when `Enter` committed.
- [ ] MarkerLabelInput mode does not process playback keys.
- [ ] Tests: label input flow, labeled marker file output, cancel via Esc.

**Ticket 5.3.3 — Help overlay and edge case hardening**

Update documentation and handle edge cases across the marker authoring system.

Changes:
- `help.rs`: add `m` and `M` to the "Markers" group (created in 5.1.3): `("m", "Add marker")`, `("M", "Add labeled marker")`.
- Update `insta` snapshot tests for new overlay dimensions.
- Edge cases to verify:
  - `m`/`M` while help overlay is visible → ignored (existing pattern).
  - `m`/`M` while in SearchInput → ignored (only Normal mode dispatches actions).
  - File becomes read-only after initial confirmation → graceful transient error.
  - `--no-controls` + marker creation → controls force-show briefly for feedback.
  - Markers added while paused work correctly.
  - Markers added while playing work correctly (time captured at keypress).
  - Round-trip test: add marker → reload file → marker at correct position.

Acceptance criteria:
- [ ] Help overlay displays `m` / `M` keybindings in the Markers group.
- [ ] Help overlay `insta` snapshot tests updated and passing.
- [ ] `m`/`M` ignored during help overlay, search input, and marker label input.
- [ ] Read-only file error shown as transient message, not panic.
- [ ] Markers can be added while paused and while playing.
- [ ] Controls become visible after marker creation even with `--no-controls`.
- [ ] Round-trip integration test: create marker, re-parse file, marker at correct effective time.
- [ ] `cargo test --workspace` passes.
- [ ] `cargo clippy --workspace -- -D warnings` passes.
- [ ] `cargo fmt --check` passes.
- [ ] No `unwrap()` in any new library code.

### Phase 5 — Known limitations

- **No undo.** Markers are appended to the file immediately. Users can manually edit the `.cast` file (NDJSON) to remove unwanted markers.
- **V3 not supported for authoring.** Marker authoring is only supported for asciicast v2 files. V3 uses relative timestamps which require different serialization logic. V3 support can be added in a follow-up.
- **Duration may change on reload.** Adding a marker inside an idle-capped gap introduces a new event that splits the gap, potentially increasing the effective duration. The increase equals the marker's offset into the gap. This is inherent to the asciicast format + idle limiting.

### Phase 5 — Dependency graph

```
Phase 5:  [5.1.1 Virtual boundaries]
          [5.1.2 Auto-pause detection]──→ [5.1.3 Auto-pause UX]
          (5.1.1 and 5.1.2 can run in parallel)
          (5.1.3 depends on 5.1.2)

          [5.2.1 Parser out-of-order]─┐
          [5.2.2 Reverse time map]────┼→ [5.2.3 Serialization + add_marker]
                                      │
          (5.2.1 and 5.2.2 can run in parallel)
          (5.2.3 depends on 5.2.1 + 5.2.2)

          [5.3.1 Unlabeled `m`]──→ [5.3.2 Labeled `M`]──→ [5.3.3 Help + hardening]
          (5.3 depends on Epic 5.2 completion)

          Epic 5.1 and Epic 5.2 can run in parallel.
```

### Phase 5 — Acceptance criteria

- [ ] `]` past last marker jumps to end; `[` before first marker jumps to start.
- [ ] `[`/`]` work sensibly with zero markers (jump to start/end).
- [ ] `--pause-at-markers` auto-pauses on marker crossing during `tick()` only.
- [ ] Auto-pause does not trigger on seek, step, or marker navigation.
- [ ] `m` adds an unlabeled marker at current time, persisted to `.cast` file.
- [ ] `M` opens label input, then adds a labeled marker on Enter.
- [ ] First marker write shows `[y/N]` confirmation; subsequent writes skip it.
- [ ] Markers added during playback appear immediately on the progress bar and in `[`/`]` navigation.
- [ ] Marker time is captured at keypress, not at confirmation/commit.
- [ ] Playback pauses during confirmation and label input dialogs.
- [ ] Out-of-order marker events in `.cast` files are accepted on reload.
- [ ] V3 recordings show clear error when marker authoring is attempted.
- [ ] Stdin input shows clear error when attempting to add markers.
- [ ] All tests pass, clippy clean, fmt check passes.
- [ ] Help overlay documents new keybindings.

### Phase 6 — Mouse support
Click-to-seek on the progress bar, scroll wheel for speed control. Requires careful handling of tmux/screen mouse passthrough and terminal compatibility detection.

### Phase 7 — Web player (WASM)
Compile `speedrun-core` to WASM via `wasm-bindgen`. Build a JS/TS rendering layer using `<canvas>` with a web-native controls UI. Ship as an npm package and/or a `<speedrun-player>` web component. The core API is already WASM-friendly (no filesystem, no threads, caller provides `Read`).

### Phase 8 — Text search
`/` to search for a string across the recording, jumping to timestamps where it appears on screen. Implementation: scan keyframe snapshots for matches first (fast), then narrow down between keyframes for precise timing.

### Phase 9 — Export
`speedrun export --format gif demo.cast` — render keyframe snapshots to animated GIF or MP4. Leverage the existing keyframe infrastructure for frame generation.

### Phase 10 — Keyframe sidecar cache
Write pre-computed keyframes to `demo.cast.idx` for instant load on very long recordings. Versioned format so cache invalidation is automatic when the recording or speedrun version changes.

---

## Appendix: Epic dependency graph

```
Phase 0:  [0.1 Scaffold] → [0.2 Tooling] → [0.3 Test data]

Phase 1:  [1.1 avt exploration]─┐
          [1.2 Parser]──────────┤
          [1.3 Time mapper]─────┼→ [1.4 Indexer] → [1.5 Player]
                                │
          (1.1, 1.2, 1.3 can run in parallel)
          (1.4 depends on 1.1 + 1.2 + 1.3)
          (1.5 depends on 1.4)

Phase 2:  [2.1 CLI bootstrap]──┐
          [2.2 TerminalView]───┼→ [2.3 Event loop]
                               │
          (2.1 and 2.2 can run in parallel)
          (2.3 depends on 2.1 + 2.2)
          (Phase 2 depends on Phase 1 completion)

Phase 3:  [3.1 Controls widget]──→ [3.2 Controls behavior]
          [3.3 Keybindings]
          [3.4 Help overlay]
          (3.1 and 3.3 and 3.4 can run in parallel)
          (3.2 depends on 3.1)
          (Phase 3 depends on Phase 2 completion)

Phase 4:  [4.1 Edge cases]
          [4.2 Performance]
          [4.3 UX polish]
          (All can run in parallel)
          (Phase 4 depends on Phase 3 completion)

Phase 5:  [5.1.1 Virtual boundaries]─────────────────┐
          [5.1.2 Auto-pause detection]→[5.1.3 UX]    │
                                                      │
          [5.2.1 Parser out-of-order]─┐               │
          [5.2.2 Reverse time map]────┼→[5.2.3 Ser.]──┼→[5.3.1 `m`]→[5.3.2 `M`]→[5.3.3 Polish]
                                                      │
          (5.1 and 5.2 can run in parallel)
          (5.3 depends on 5.2 completion)
```
