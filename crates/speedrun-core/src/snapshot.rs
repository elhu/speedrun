//! Terminal state snapshot capture and restore.
//!
//! ## Strategy
//!
//! `avt::Vt` does not implement `Clone`, so we cannot directly copy terminal
//! state. Instead, we use `Vt::dump()` which serializes the full terminal state
//! — both screen buffers, cursor position, pen attributes, modes, charsets,
//! tab stops, and scroll margins — as a string of escape sequences.
//!
//! Feeding a dump string into a fresh `Vt` of the **same dimensions** reproduces
//! the exact terminal state.
//!
//! ### Caveats
//!
//! - When the primary buffer is active, alternate buffer *content* is not
//!   captured (only saved cursor context). This is correct behavior — entering
//!   alternate screen mode always starts with a fresh buffer.
//! - `dump()` does NOT capture terminal dimensions. The snapshot must store and
//!   restore the correct width and height separately.

/// Snapshot of terminal state, captured via `avt::Vt::dump()`.
#[derive(Debug, Clone)]
pub struct TerminalSnapshot {
    /// Opaque escape sequence blob from `Vt::dump()`.
    dump: String,
    /// Terminal width in columns.
    width: u16,
    /// Terminal height in rows.
    height: u16,
}

/// Cursor state extracted from avt.
///
/// We define our own type because `avt::Cursor` is not publicly importable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CursorState {
    pub col: usize,
    pub row: usize,
    pub visible: bool,
}

/// Create a `Vt` configured for asciicast playback.
///
/// The terminal is resizable (recordings contain resize events) and has zero
/// scrollback (we don't need scroll history for playback).
pub fn create_vt(cols: usize, rows: usize) -> avt::Vt {
    avt::Vt::builder()
        .size(cols, rows)
        .resizable(true)
        .scrollback_limit(0)
        .build()
}

impl TerminalSnapshot {
    /// Capture the current state of a `Vt`.
    pub fn capture(vt: &avt::Vt) -> Self {
        let dump = vt.dump();
        let (cols, rows) = vt.size();
        Self {
            dump,
            width: cols as u16,
            height: rows as u16,
        }
    }

    /// Restore state into a fresh `Vt` with matching dimensions.
    ///
    /// The `Vt` is created with the snapshot's width/height, NOT the header
    /// dimensions.
    pub fn restore(&self) -> avt::Vt {
        let mut vt = create_vt(self.width as usize, self.height as usize);
        vt.feed_str(&self.dump);
        vt
    }

    /// Terminal width in columns.
    pub fn width(&self) -> u16 {
        self.width
    }

    /// Terminal height in rows.
    pub fn height(&self) -> u16 {
        self.height
    }
}

impl CursorState {
    /// Extract cursor state from an avt `Vt`.
    pub fn from_vt(vt: &avt::Vt) -> Self {
        let cursor = vt.cursor();
        Self {
            col: cursor.col,
            row: cursor.row,
            visible: cursor.visible,
        }
    }
}
