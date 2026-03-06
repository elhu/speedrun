//! Integration tests for edge case recordings.
//!
//! Exercises the full pipeline: parse → index → player → seek/step for
//! edge case inputs including empty recordings, input-only recordings,
//! sub-second recordings, resize events, malformed inputs, and Unicode.

use speedrun_core::{EventType, Player};
use std::io::Write;
use tempfile::NamedTempFile;

// ---------------------------------------------------------------------------
// Helper utilities
// ---------------------------------------------------------------------------

fn testdata_path(name: &str) -> std::path::PathBuf {
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("../../testdata");
    p.push(name);
    p
}

fn load_file(name: &str) -> Player {
    let file = std::fs::File::open(testdata_path(name))
        .unwrap_or_else(|e| panic!("failed to open {name}: {e}"));
    Player::load(file).unwrap_or_else(|e| panic!("failed to load {name}: {e}"))
}

const EPSILON: f64 = 1e-6;

fn assert_f64_approx(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < EPSILON,
        "expected ~{expected}, got {actual}"
    );
}

fn screen_is_blank(player: &Player) -> bool {
    player
        .screen()
        .iter()
        .all(|line| line.text().trim().is_empty())
}

// ---------------------------------------------------------------------------
// empty.cast — no events
// ---------------------------------------------------------------------------

#[test]
fn empty_cast_loads_without_error() {
    load_file("empty.cast");
}

#[test]
fn empty_cast_zero_events() {
    let player = load_file("empty.cast");
    // The recording has 0 events — duration is 0
    assert_f64_approx(player.duration(), 0.0);
}

#[test]
fn empty_cast_seek_does_not_panic() {
    let mut player = load_file("empty.cast");
    player.seek(0.0);
    assert_f64_approx(player.current_time(), 0.0);
}

#[test]
fn empty_cast_step_forward_returns_false() {
    let mut player = load_file("empty.cast");
    assert!(
        !player.step_forward(),
        "step_forward on empty recording should return false"
    );
}

#[test]
fn empty_cast_step_backward_returns_false() {
    let mut player = load_file("empty.cast");
    assert!(
        !player.step_backward(),
        "step_backward on empty recording should return false"
    );
}

// ---------------------------------------------------------------------------
// with_resize.cast — loads and seeks without panic
// ---------------------------------------------------------------------------

#[test]
fn with_resize_loads_without_error() {
    load_file("with_resize.cast");
}

#[test]
fn with_resize_seek_to_zero_does_not_panic() {
    let mut player = load_file("with_resize.cast");
    player.seek(0.0);
}

#[test]
fn with_resize_seek_to_midpoint_does_not_panic() {
    let mut player = load_file("with_resize.cast");
    let mid = player.duration() / 2.0;
    player.seek(mid);
}

#[test]
fn with_resize_seek_to_duration_does_not_panic() {
    let mut player = load_file("with_resize.cast");
    let dur = player.duration();
    player.seek(dur);
}

#[test]
fn with_resize_dimensions_change_after_resize_event() {
    let mut player = load_file("with_resize.cast");
    // Seeking past the resize event at t=2.0 should change dimensions
    player.seek(player.duration());
    let (cols, rows) = player.size();
    // with_resize.cast resizes to 120x40
    assert_eq!(cols, 120, "expected 120 cols after resize");
    assert_eq!(rows, 40, "expected 40 rows after resize");
}

// ---------------------------------------------------------------------------
// input_only.cast (v3) — all input events, no output
// ---------------------------------------------------------------------------

#[test]
fn input_only_loads_without_error() {
    load_file("input_only.cast");
}

#[test]
fn input_only_three_events() {
    // input_only.cast has 3 input events
    // We verify by checking the recording parses 3 events and they are all Input
    // (Player doesn't expose events directly, so we parse directly)
    let file = std::fs::File::open(testdata_path("input_only.cast")).unwrap();
    let recording = speedrun_core::parse(file).unwrap();
    assert_eq!(recording.events.len(), 3, "expected 3 events");
    for event in &recording.events {
        assert_eq!(
            event.event_type,
            EventType::Input,
            "expected all Input events"
        );
    }
}

#[test]
fn input_only_duration_approx_1_5() {
    let player = load_file("input_only.cast");
    // V3 relative intervals: 0.5, 0.5, 0.5 → absolute 0.5, 1.0, 1.5
    // Duration = last event time = 1.5
    assert_f64_approx(player.duration(), 1.5);
}

#[test]
fn input_only_step_forward_returns_false() {
    let mut player = load_file("input_only.cast");
    // No output events → step_forward should return false
    assert!(
        !player.step_forward(),
        "step_forward should return false when there are no output events"
    );
}

#[test]
fn input_only_screen_is_blank() {
    let player = load_file("input_only.cast");
    // Input-only recording → screen stays blank
    assert!(
        screen_is_blank(&player),
        "screen should be blank for input-only recording"
    );
}

// ---------------------------------------------------------------------------
// sub_second.cast (v2) — three events under 0.5s
// ---------------------------------------------------------------------------

#[test]
fn sub_second_loads_without_error() {
    load_file("sub_second.cast");
}

#[test]
fn sub_second_three_events() {
    let file = std::fs::File::open(testdata_path("sub_second.cast")).unwrap();
    let recording = speedrun_core::parse(file).unwrap();
    assert_eq!(recording.events.len(), 3, "expected 3 events");
}

#[test]
fn sub_second_duration_approx_0_5() {
    let player = load_file("sub_second.cast");
    // Events at 0.1, 0.3, 0.5 → duration = 0.5
    assert_f64_approx(player.duration(), 0.5);
}

#[test]
fn sub_second_seek_sets_current_time() {
    let mut player = load_file("sub_second.cast");
    player.seek(0.3);
    assert_f64_approx(player.current_time(), 0.3);
}

#[test]
fn sub_second_step_forward_returns_true() {
    let mut player = load_file("sub_second.cast");
    // sub_second.cast has output events → step_forward should return true
    assert!(
        player.step_forward(),
        "step_forward should return true when output events exist"
    );
}

// ---------------------------------------------------------------------------
// Malformed inputs (generated at runtime using tempfile)
// ---------------------------------------------------------------------------

#[test]
fn truncated_file_parses_one_event_one_warning() {
    let mut tmp = NamedTempFile::new().unwrap();
    write!(
        tmp,
        "{}\n{}\n{}",
        r#"{"version": 2, "width": 80, "height": 24}"#,
        r#"[1.0, "o", "hello"]"#,
        r#"[2.0, "o", "he"# // truncated mid-JSON
    )
    .unwrap();
    tmp.flush().unwrap();

    let file = std::fs::File::open(tmp.path()).unwrap();
    let recording = speedrun_core::parse(file).unwrap();
    assert_eq!(recording.events.len(), 1, "expected 1 valid event");
    assert_eq!(
        recording.warnings.len(),
        1,
        "expected 1 warning for truncated line"
    );
}

#[test]
fn huge_single_event_parses_without_panic() {
    let huge_data = "x".repeat(50 * 1024); // 50 KB
    let mut tmp = NamedTempFile::new().unwrap();
    writeln!(tmp, r#"{{"version": 2, "width": 80, "height": 24}}"#).unwrap();
    writeln!(tmp, r#"[1.0, "o", "{huge_data}"]"#).unwrap();
    tmp.flush().unwrap();

    let file = std::fs::File::open(tmp.path()).unwrap();
    let recording = speedrun_core::parse(file).unwrap();
    assert_eq!(recording.events.len(), 1, "expected 1 event with huge data");
}

#[test]
fn empty_string_event_data_parses_successfully() {
    let mut tmp = NamedTempFile::new().unwrap();
    writeln!(tmp, r#"{{"version": 2, "width": 80, "height": 24}}"#).unwrap();
    writeln!(tmp, r#"[1.0, "o", ""]"#).unwrap();
    tmp.flush().unwrap();

    let file = std::fs::File::open(tmp.path()).unwrap();
    let recording = speedrun_core::parse(file).unwrap();
    assert_eq!(recording.events.len(), 1, "expected 1 event");
    match &recording.events[0].data {
        speedrun_core::EventData::Text(s) => {
            assert_eq!(s, "", "expected empty string data");
        }
        other => panic!("expected Text data, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Lenient parser warnings through full pipeline
// ---------------------------------------------------------------------------

#[test]
fn five_valid_two_invalid_events_counts() {
    let input = concat!(
        "{\"version\": 2, \"width\": 80, \"height\": 24}\n",
        "[1.0, \"o\", \"a\"]\n",
        "[2.0, \"o\", \"b\"]\n",
        "[3.0, \"o\", \"c\"]\n",
        "[4.0, \"o\", \"d\"]\n",
        "[5.0, \"o\", \"e\"]\n",
        "[6.0, \"o\"]\n", // invalid — missing data
        "[7.0, \"o\"]\n", // invalid — missing data
    );
    let recording = speedrun_core::parse(std::io::Cursor::new(input)).unwrap();
    assert_eq!(recording.events.len(), 5, "expected 5 valid events");
    assert_eq!(recording.warnings.len(), 2, "expected 2 warnings");
}

#[test]
fn player_load_with_warnings_succeeds_and_exposes_warnings() {
    let input = concat!(
        "{\"version\": 2, \"width\": 80, \"height\": 24}\n",
        "[1.0, \"o\", \"hello\"]\n",
        "[2.0, \"o\", \"world\"]\n",
        "[3.0, \"o\"]\n", // invalid — 2 elements
        "[4.0, \"o\", \"final\"]\n",
        "[5.0, \"o\"]\n", // invalid — 2 elements
    );
    let player = Player::load(std::io::Cursor::new(input)).unwrap();
    assert_eq!(
        player.warnings().len(),
        2,
        "expected 2 warnings from player, got {}",
        player.warnings().len()
    );
    // Seek and step should still work on the 3 valid events
    let mut player = player;
    player.seek(2.0);
    assert_f64_approx(player.current_time(), 2.0);
    assert!(
        player.step_forward(),
        "step_forward should find remaining output event"
    );
}

// ---------------------------------------------------------------------------
// Unicode output data — load/index/seek do not panic
// ---------------------------------------------------------------------------

#[test]
fn unicode_output_loads_and_seeks_without_panic() {
    let unicode_data = "hello 🌍 こんにちは мир café naïve résumé";
    let input = format!(
        "{}\n{}\n",
        r#"{"version": 2, "width": 80, "height": 24}"#,
        format!(r#"[1.0, "o", "{unicode_data}"]"#)
    );
    let mut player = Player::load(std::io::Cursor::new(input)).unwrap();
    // Load succeeded; verify seek positions don't panic
    player.seek(0.0);
    player.seek(player.duration());
    player.seek(0.5);
}

#[test]
fn unicode_step_forward_does_not_panic() {
    let unicode_data = "emoji: 🎉🚀💻 cjk: 日本語 arabic: مرحبا";
    let input = format!(
        "{}\n{}\n{}\n",
        r#"{"version": 2, "width": 80, "height": 24}"#,
        format!(r#"[0.5, "o", "{unicode_data}"]"#),
        format!(r#"[1.0, "o", "more: ñoño"]"#),
    );
    let mut player = Player::load(std::io::Cursor::new(input)).unwrap();
    // step_forward should not panic on Unicode content
    let stepped1 = player.step_forward();
    assert!(
        stepped1,
        "expected step_forward to find first Unicode event"
    );
    let stepped2 = player.step_forward();
    assert!(stepped2, "expected step_forward to find second event");
    let stepped3 = player.step_forward();
    assert!(!stepped3, "expected step_forward to return false at end");
}
