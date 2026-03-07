//! Core engine for parsing, indexing, and playing back asciicast terminal recordings.
//!
//! This crate provides the foundational components for a terminal session player:
//!
//! - **Parsing** ([`parser`]) — reads asciicast v2 and v3 `.cast` files into structured
//!   [`Recording`] data.
//! - **Time mapping** ([`timemap`]) — transforms raw timestamps into effective (idle-capped)
//!   timestamps so long pauses can be compressed.
//! - **Keyframe indexing** ([`index`]) — builds a snapshot index at regular intervals,
//!   enabling O(log n) seeking to any point in a recording.
//! - **Playback** ([`player`]) — ties everything together into a [`Player`] that supports
//!   seek, tick-based advancement, speed control, and single-event stepping.
//! - **Snapshots** ([`snapshot`]) — captures and restores terminal state via the `avt`
//!   virtual terminal emulator.
//!
//! # Dependencies
//!
//! Some public APIs expose types from the [`avt`](https://docs.rs/avt) crate
//! (e.g., [`Player::screen()`] returns `&[avt::Line]`). Callers that need to
//! work with these return values should add `avt` as a direct dependency.

#![warn(missing_docs)]

/// Asciicast v2/v3 parser.
pub mod parser;

/// Keyframe index for O(log n) seeking.
pub mod index;

/// Playback controller and seek engine.
pub mod player;

/// Terminal state snapshot capture and restore.
pub mod snapshot;

/// Raw-to-effective time mapping with idle capping.
pub mod timemap;

pub use index::{KEYFRAME_INTERVAL, Keyframe, KeyframeIndex};
pub use parser::{
    Event, EventData, EventType, Header, Marker, ParseError, ParseWarning, Recording, parse,
};
pub use player::{LoadOptions, Player, PlayerError};
pub use snapshot::{CursorState, TerminalSnapshot, create_vt};
pub use timemap::{TimeMap, TimeMapError};
