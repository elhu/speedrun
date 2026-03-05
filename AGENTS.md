# Agent Instructions

## Project Overview

**speedrun** is a modern terminal session player for asciicast (`.cast`) recordings with instant seeking, scrubbing, and full playback control. Think of it as a video player for terminal recordings — you can jump to any point, control playback speed, and navigate with a visual timeline.

This is a two-crate Cargo workspace:
- **Core engine** — Pure Rust library for parsing, indexing, seeking, and playback control
- **TUI player** — Terminal interface using ratatui framework for keyboard-driven playback

## Build and Test Commands

```bash
# Build everything
cargo build --workspace

# Run all tests
cargo test --workspace

# Lint (must pass with zero warnings)
cargo clippy --workspace -- -D warnings

# Format check (must pass)
cargo fmt --check

# Test a single crate
cargo test -p speedrun-core
cargo test -p speedrun
```

## Project Structure

```
crates/
├── speedrun-core/src/
│   ├── lib.rs          # Public API re-exports
│   ├── parser.rs       # Asciicast v2/v3 parsing
│   ├── timemap.rs      # Raw → effective time mapping (idle limit)
│   ├── index.rs        # Keyframe index building
│   ├── player.rs       # Playback controller + seek engine
│   └── snapshot.rs     # Terminal state snapshots
└── speedrun/src/
    ├── main.rs         # Entry point, CLI parsing
    ├── app.rs          # Event loop + application state
    ├── ui.rs           # Ratatui rendering (TerminalView + controls)
    └── input.rs        # Keymap handling
```

## Coding Conventions

- **Rust edition 2024** — All code must pass strict compiler checks
- **Clippy + rustfmt** — Zero warnings required, auto-formatting enforced  
- **Cargo workspace** — Use `cargo <command> --workspace` from root, crate-specific commands as needed
- **No `unwrap()` in library code** — Use proper error types with `Result<T, E>`
- **Error handling** — Use typed errors with proper Error traits, no panicking on recoverable errors
- **Testing** — Use Rust's built-in test framework with `insta` for snapshot tests

## Key Dependencies

- **`avt`** — Virtual terminal emulation library for parsing escape sequences
- **`ratatui`** + **`crossterm`** — TUI framework for the terminal player interface  
- **`clap`** — CLI argument parsing (with derive feature)
- **`serde`** + **`serde_json`** — Runtime type validation for asciicast parsing
- **`insta`**, **`tempfile`**, **`pretty_assertions`** — Testing utilities and frameworks

This project uses **bd** (beads) for issue tracking. Run `bd onboard` to get started.

## Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --status in_progress  # Claim work
bd close <id>         # Complete work
bd sync               # Sync with git
```

## Landing the Plane (Session Completion)

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   bd sync
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches after merging, etc...
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session when closing the bead

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds

<!-- pit:orchestrator:start -->
## Parallel Agent Orchestration with pit

You have access to `pit`, a tmux-based CLI that manages parallel AI coding agent sessions.
Each agent works in its own git worktree and tmux window, autonomously looping through
tickets in a beads epic.

### Prerequisites

Before using pit, ensure the project has epics and tickets set up with beads (`bd`).

### Workflow

1. **Start agents**: `pit start --epics <epic-id>` (comma-separated for multiple)
   - Creates one git worktree + tmux window + agent TUI per epic
   - Each agent begins working through tickets autonomously
2. **Monitor**: `pit status` — returns JSON with state of all epics
3. **Read agent output**: `pit log <epic>` — captures the agent's terminal output
4. **Pause an agent**: `pit pause <epic>` — pauses the agent's autonomous loop
5. **Resume with guidance**: `pit resume <epic> --message "your instructions"`
   — resumes a paused agent, injecting your message into its context

### CLI Reference

| Command | Description |
|---|---|
| `pit start --epics <ids>` | Start agent sessions for given epic IDs |
| `pit status` | Show status of all running epics (JSON) |
| `pit log <epic>` | Capture agent's recent terminal output |
| `pit log <epic> --lines 100` | Capture more lines of output |
| `pit pause <epic>` | Pause a running epic's autonomous loop |
| `pit resume <epic> --message "..."` | Resume a paused epic with guidance |
| `pit teardown [<epic-id>]` | Tear down all epics (or a single epic), removes worktrees by default |
| `pit teardown --keep-worktrees` | Tear down without removing git worktrees |
| `pit teardown --force` | Tear down all epics even if some are still running |

### Start Options

| Flag | Default | Description |
|---|---|---|
| `--epics <ids>` | (required) | Comma-separated beads epic IDs |
| `--agent <type>` | `auto` | Agent type: `auto`, `Claude`, or `claude-code` |
| `--base-branch <branch>` | `main` | Base branch for worktrees |
| `--tmux-session <name>` | `pit` | tmux session name |
| `--worktree-dir <path>` | `.worktrees` | Directory for git worktrees |
| `--prompt-template <path>` | (built-in) | Path to a custom prompt template file |
| `--instructions-template <path>` | (built-in) | Path to a custom agent instructions template file |
| `--ticket-timeout <minutes>` | (none) | Auto-pause agents when a ticket exceeds this duration |
| `--model <value>` | (none) | Model to pass to the agent CLI (passed through as `--model <value>`) |
| `--epic-model <mapping>` | (none) | Per-epic model override, repeatable: `--epic-model <epic>=<model>` |

> **Model resolution order:** `--epic-model <epic>=<model>` overrides `--model` / `.pit.json` model, which overrides the agent default. The model string is passed through to the agent CLI as-is.

### Custom Templates

Use `--prompt-template` and `--instructions-template` to override the built-in agent
prompt and instructions with your own files. Templates support variable substitution:

- `{{EPIC_ID}}` — replaced with the epic ID being worked on
- `{{EPIC_NAME}}` — replaced with the epic name

### Configuration

pit reads `.pit.json` from the project root. CLI flags override file values; missing
fields fall back to defaults.

```json
{
  "agent": "auto",
  "model": null,
  "worktreeDir": ".worktrees",
  "baseBranch": "main",
  "tmuxSession": "pit",
  "clearDelay": 2000,
  "initDelay": 5000,
  "ticketTimeout": null
}
```

| Field | Type | Default | Description |
|---|---|---|---|
| `agent` | string | `"auto"` | Agent type: `auto`, `opencode`, or `claude-code` |
| `model` | string|null | `null` | Model to pass to the agent CLI; null = agent default |
| `worktreeDir` | string | `".worktrees"` | Directory for git worktrees |
| `baseBranch` | string | `"main"` | Base branch for worktrees |
| `tmuxSession` | string | `"pit"` | tmux session name |
| `clearDelay` | number | `2000` | Ms to wait before clearing agent context after ticket completion |
| `initDelay` | number | `5000` | Ms to wait before starting the agent after setup |
| `ticketTimeout` | number|null | `null` | Minutes before a ticket auto-pauses the agent (null = disabled) |

### Output Format

All pit commands output JSON by default (for LLM consumption). Add `--pretty` for
human-readable output. Parse the JSON to understand the current state:

```json
// pit status example
{
  "epics": {
    "auth": { "state": "running", "progress": "3/7" },
    "payments": { "state": "paused", "progress": "1/4" }
  }
}
```

### When to Intervene

- **`pit status` shows an epic as "paused"**: An agent hit `NEEDS_HUMAN_INPUT`.
  Read `pit log <epic>` to understand what the agent needs, then
  `pit resume <epic> --message "your answer"`.
- **Pause reason starts with `TIMEOUT:`**: The agent was auto-paused because the
  ticket exceeded the configured `ticketTimeout`. Read `pit log <epic>` to assess
  progress, then `pit resume <epic> --message "your guidance"` or let it continue
  with `pit resume <epic>`.
- **An agent seems stuck**: Use `pit log <epic>` to check progress.
  `pit pause <epic>` if needed, then resume with instructions.
- **Epic completes**: The epic state becomes `"done"`. Merge the worktree branch
  back to the base branch.

### Session Reliability

pit automatically manages long-running sessions reliably. Once started, agent sessions
continue running independently until completion, regardless of how long they take.
No special flags or configuration needed.

### Important Notes

- Agents work independently — they do not communicate with each other.
- Each agent works in its own git worktree, so there are no file conflicts during work.
- Merging worktree branches back is a manual step (done by you after epic completion).
- The tmux session is named `pit` by default. You can attach to it to observe agents directly.
- `pit teardown` removes worktrees by default. Use `--keep-worktrees` if you need to inspect the worktree after teardown.
- `pit teardown` (no epic specified) will fail if any epic is in `running` state, to prevent accidental destruction of active sessions. Use `pit teardown --force` to bypass this guard when you deliberately want to tear everything down.
<!-- pit:orchestrator:end -->
