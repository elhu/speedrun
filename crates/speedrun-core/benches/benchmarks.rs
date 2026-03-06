mod recording_gen;

use criterion::{Criterion, criterion_group, criterion_main};
use speedrun_core::{KeyframeIndex, Player, TimeMap, parse};
use std::io::Cursor;

/// Recording sizes in seconds for each benchmark group.
const SIZES: &[(&str, u64)] = &[("1min", 60), ("5min", 300), ("10min", 600), ("30min", 1800)];

/// Seed used for all generators (for determinism).
const SEED: u64 = 42;

// ---------------------------------------------------------------------------
// parse benchmark: measure parse() throughput
// ---------------------------------------------------------------------------

fn bench_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse");

    for &(name, duration_secs) in SIZES {
        // Generate recording in setup (outside b.iter)
        let data = recording_gen::generate_recording(duration_secs, SEED);

        group.bench_function(name, |b| {
            b.iter(|| parse(Cursor::new(data.clone())).expect("parse failed"));
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// index_build benchmark: measure KeyframeIndex::build() throughput
// ---------------------------------------------------------------------------

fn bench_index_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("index_build");

    for &(name, duration_secs) in SIZES {
        // Parse once in setup
        let data = recording_gen::generate_recording(duration_secs, SEED);
        let recording = parse(Cursor::new(data)).expect("parse failed");
        let raw_times: Vec<f64> = recording.events.iter().map(|e| e.time).collect();
        let time_map = TimeMap::build(&raw_times, None).expect("time map failed");

        group.bench_function(name, |b| {
            b.iter(|| KeyframeIndex::build(&recording, &time_map, 5.0));
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// seek_worst_case benchmark: seek from 0 to duration (worst-case seek)
// ---------------------------------------------------------------------------

fn bench_seek_worst_case(c: &mut Criterion) {
    let mut group = c.benchmark_group("seek_worst_case");

    for &(name, duration_secs) in SIZES {
        // Build a full Player in setup
        let data = recording_gen::generate_recording(duration_secs, SEED);
        let mut player = Player::load(Cursor::new(data)).expect("player load failed");
        let duration = player.duration();

        group.bench_function(name, |b| {
            b.iter(|| {
                // Seek from implicit start to end (worst-case: traverses all events)
                player.seek(duration);
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// seek_random benchmark: seek to 10 random positions
// ---------------------------------------------------------------------------

fn bench_seek_random(c: &mut Criterion) {
    use rand::Rng;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    let mut group = c.benchmark_group("seek_random");

    for &(name, duration_secs) in SIZES {
        // Build a full Player in setup
        let data = recording_gen::generate_recording(duration_secs, SEED);
        let mut player = Player::load(Cursor::new(data)).expect("player load failed");
        let duration = player.duration();

        // Generate 10 seek targets from a seeded RNG in setup
        let mut rng = StdRng::seed_from_u64(SEED);
        let targets: Vec<f64> = (0..10).map(|_| rng.random::<f64>() * duration).collect();

        group.bench_function(name, |b| {
            b.iter(|| {
                // Loop over all 10 targets
                for &t in &targets {
                    player.seek(t);
                }
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_parse,
    bench_index_build,
    bench_seek_worst_case,
    bench_seek_random
);
criterion_main!(benches);

// ---------------------------------------------------------------------------
// Unit test: generator validation
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::recording_gen;
    use speedrun_core::parse;
    use std::io::Cursor;

    #[test]
    fn generator_produces_parseable_recording() {
        let data = recording_gen::generate_recording(10, 42);
        let recording = parse(Cursor::new(data)).expect("generated recording should parse");

        let event_count = recording.events.len();
        // Expected: ~10 * 3 = 30 events, allow ±50% → [15, 45]
        assert!(
            event_count >= 15 && event_count <= 45,
            "expected ~30 events (10s * 3/s ±50%), got {event_count}"
        );
    }

    #[test]
    fn generator_is_deterministic() {
        let data1 = recording_gen::generate_recording(10, 42);
        let data2 = recording_gen::generate_recording(10, 42);
        assert_eq!(data1, data2, "generate_recording must be deterministic");
    }

    #[test]
    fn generator_includes_ansi_sequences() {
        let data = recording_gen::generate_recording(30, 42);
        let text = std::str::from_utf8(&data).expect("output should be valid UTF-8");
        // Check for SGR sequences (ESC[...m) or cursor movement (ESC[...H)
        assert!(
            text.contains("\x1b["),
            "generated recording should contain ANSI escape sequences"
        );
    }
}
