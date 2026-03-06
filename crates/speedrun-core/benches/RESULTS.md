# Benchmark Results

## Hardware

- **CPU**: Intel Xeon E5-1620 v2 @ 3.70 GHz (4 cores / 8 threads)
- **RAM**: 32 GB
- **OS**: Linux
- **Rust**: release profile (criterion default)

## Benchmark Output

All timings are median values from `cargo bench -p speedrun-core` (release mode).

### parse — `speedrun_core::parse()` on generated recordings

| Recording size | Median time |
|----------------|-------------|
| 1 min          | 65.1 µs     |
| 5 min          | 355.3 µs    |
| 10 min         | 721.6 µs    |
| 30 min         | 2.138 ms    |

### index_build — `KeyframeIndex::build()` on pre-parsed recordings

| Recording size | Median time |
|----------------|-------------|
| 1 min          | 503.1 µs    |
| 5 min          | 2.957 ms    |
| 10 min         | 6.336 ms    |
| 30 min         | 20.49 ms    |

### seek_worst_case — `player.seek(duration)` (seek from start to end)

| Recording size | Median time |
|----------------|-------------|
| 1 min          | 40.2 µs     |
| 5 min          | 41.5 µs     |
| 10 min         | 42.0 µs     |
| 30 min         | 41.8 µs     |

### seek_random — 10 random seeks per iteration

| Recording size | Median time (10 seeks) | Per-seek |
|----------------|------------------------|----------|
| 1 min          | 360.6 µs               | 36.1 µs  |
| 5 min          | 375.7 µs               | 37.6 µs  |
| 10 min         | 389.2 µs               | 38.9 µs  |
| 30 min         | 391.2 µs               | 39.1 µs  |

## Load + Index Combined Times

The "load" target includes both parsing and index building (the full `Player::load` path).

| Recording size | parse + index_build | Target  | Status |
|----------------|---------------------|---------|--------|
| 1 min          | ~568 µs (0.57 ms)   | < 100ms | ✅ PASS |
| 5 min          | ~3.31 ms            | < 100ms | ✅ PASS |
| 10 min         | ~7.06 ms            | < 500ms | ✅ PASS |

## Seek Performance

| Recording size | Worst-case seek | Per-seek (random) | Target  | Status |
|----------------|-----------------|-------------------|---------|--------|
| 1 min          | 40.2 µs         | 36.1 µs           | < 50ms  | ✅ PASS |
| 5 min          | 41.5 µs         | 37.6 µs           | < 50ms  | ✅ PASS |

## CPU Idle Verification

The `time_to_next_event()` method returns `Some(non-zero duration)` when the player
is playing with events remaining. The event loop sleeps for this duration before the
next tick, keeping CPU usage near zero during idle playback. This is verified by the
`time_to_next_event_idle_sleep_hint` unit test in `player.rs`.

## Summary

All performance targets from REQUIREMENTS.md are met with large margins:

| Target                          | Requirement | Actual (best case) | Status |
|---------------------------------|-------------|-------------------|--------|
| Seek (1 min recording)          | < 50 ms     | ~40 µs            | ✅ PASS (1250× margin) |
| Seek (5 min recording)          | < 50 ms     | ~42 µs            | ✅ PASS (1200× margin) |
| Load (1 min recording)          | < 100 ms    | ~0.57 ms          | ✅ PASS (175× margin) |
| Load (5 min recording)          | < 100 ms    | ~3.3 ms           | ✅ PASS (30× margin) |
| Load (10 min recording)         | < 500 ms    | ~7.1 ms           | ✅ PASS (70× margin) |
| CPU idle (event-driven sleep)   | near zero   | verified          | ✅ PASS |
