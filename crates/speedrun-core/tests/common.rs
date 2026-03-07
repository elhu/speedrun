//! Shared test utilities for speedrun-core integration tests.
//!
//! This module is included via `mod common;` in each integration test file.
//! The TUI crate (`crates/speedrun/`) has its own copy because `CARGO_MANIFEST_DIR`
//! differs between crates.

use std::path::PathBuf;

/// Returns the path to a test fixture file in the workspace `testdata/` directory.
pub fn testdata_path(name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("../../testdata");
    p.push(name);
    p
}
