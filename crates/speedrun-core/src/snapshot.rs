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
    /// Zero-based column position.
    pub col: usize,
    /// Zero-based row position.
    pub row: usize,
    /// Whether the cursor is visible.
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

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn basic_round_trip() {
        let mut vt = create_vt(10, 4);
        vt.feed_str("hello\r\nworld");

        let original_cursor = CursorState::from_vt(&vt);
        let snapshot = TerminalSnapshot::capture(&vt);
        let restored = snapshot.restore();

        // Verify line text matches line-by-line
        let orig_view = vt.view();
        let rest_view = restored.view();
        for row in 0..4 {
            assert_eq!(
                orig_view[row].text(),
                rest_view[row].text(),
                "line {row} text mismatch"
            );
        }

        // Verify cursor position matches
        let restored_cursor = CursorState::from_vt(&restored);
        assert_eq!(original_cursor, restored_cursor);
    }

    #[test]
    fn colors_and_attributes() {
        let mut vt = create_vt(40, 4);
        // bold + red fg + green bg + inverse, then some text
        vt.feed_str("\x1b[1m\x1b[31m\x1b[42m\x1b[7mStyled Text");

        let snapshot = TerminalSnapshot::capture(&vt);
        let restored = snapshot.restore();

        // Verify text content
        let rest_view = restored.view();
        let line = &rest_view[0];
        assert!(line.text().starts_with("Styled Text"));

        // Verify attributes on the restored cells
        let cells: Vec<(char, avt::Pen)> = line.cells().collect();
        // Check the first character 'S'
        let (ch, ref pen) = cells[0];
        assert_eq!(ch, 'S');
        assert!(pen.is_bold(), "expected bold");
        assert_eq!(
            pen.foreground(),
            Some(avt::Color::Indexed(1)),
            "expected red foreground (indexed 1)"
        );
        assert_eq!(
            pen.background(),
            Some(avt::Color::Indexed(2)),
            "expected green background (indexed 2)"
        );
        assert!(pen.is_inverse(), "expected inverse");
    }

    #[test]
    fn cursor_state() {
        // Default position on a fresh Vt
        let vt = create_vt(80, 24);
        let cursor = CursorState::from_vt(&vt);
        assert_eq!(cursor.col, 0);
        assert_eq!(cursor.row, 0);
        assert!(cursor.visible);

        // After feeding text, cursor moves
        let mut vt = create_vt(80, 24);
        vt.feed_str("abcde\r\nfg");
        let cursor = CursorState::from_vt(&vt);
        assert_eq!(cursor.col, 2);
        assert_eq!(cursor.row, 1);
        assert!(cursor.visible);

        // Hide cursor
        let mut vt = create_vt(80, 24);
        vt.feed_str("\x1b[?25l");
        let snapshot = TerminalSnapshot::capture(&vt);
        let restored = snapshot.restore();
        let cursor = CursorState::from_vt(&restored);
        assert!(!cursor.visible, "cursor should be hidden after \\x1b[?25l");

        // Show cursor
        let mut vt = create_vt(80, 24);
        vt.feed_str("\x1b[?25l\x1b[?25h");
        let snapshot = TerminalSnapshot::capture(&vt);
        let restored = snapshot.restore();
        let cursor = CursorState::from_vt(&restored);
        assert!(cursor.visible, "cursor should be visible after \\x1b[?25h");
    }

    #[test]
    fn alternate_buffer() {
        // Enter alternate buffer and write text
        let mut vt = create_vt(40, 10);
        vt.feed_str("\x1b[?1049h");
        vt.feed_str("alt buffer content");
        let snapshot = TerminalSnapshot::capture(&vt);
        let restored = snapshot.restore();

        let rest_view = restored.view();
        assert!(
            rest_view[0].text().starts_with("alt buffer content"),
            "alternate buffer content should be preserved after capture/restore"
        );

        // Exit alternate buffer to primary, write text there
        let mut vt = create_vt(40, 10);
        vt.feed_str("\x1b[?1049h");
        vt.feed_str("alt text");
        vt.feed_str("\x1b[?1049l");
        vt.feed_str("primary content");

        let snapshot = TerminalSnapshot::capture(&vt);
        let restored = snapshot.restore();

        let rest_view = restored.view();
        // When primary is active, we should see primary content
        assert!(
            rest_view[0].text().starts_with("primary content"),
            "primary buffer content should be preserved"
        );

        // Note: Alternate buffer content is intentionally not captured when
        // primary is active — entering alternate screen mode always starts
        // with a fresh buffer. This is correct behavior per the VT protocol:
        // switching to the alternate screen saves the primary state and
        // presents a blank screen, so there is no need to persist alternate
        // content across snapshots taken from the primary buffer.
    }

    #[test]
    fn resize() {
        let mut vt = create_vt(80, 24);
        // xtwinops: \x1b[8;rows;cols t — note rows;cols order
        vt.feed_str("\x1b[8;40;120t");
        vt.feed_str("after resize");

        let snapshot = TerminalSnapshot::capture(&vt);
        assert_eq!(
            snapshot.width(),
            120,
            "snapshot width should be 120 after resize"
        );
        assert_eq!(
            snapshot.height(),
            40,
            "snapshot height should be 40 after resize"
        );

        let restored = snapshot.restore();
        assert_eq!(
            restored.size(),
            (120, 40),
            "restored Vt size should be (120, 40)"
        );

        let rest_view = restored.view();
        assert!(
            rest_view[0].text().starts_with("after resize"),
            "text content should be preserved after resize round-trip"
        );
    }

    #[test]
    fn fresh_vt_empty_recording() {
        let vt = create_vt(80, 24);
        let original_cursor = CursorState::from_vt(&vt);

        let snapshot = TerminalSnapshot::capture(&vt);
        let restored = snapshot.restore();

        // Verify all lines are blank
        let orig_view = vt.view();
        let rest_view = restored.view();
        for row in 0..24 {
            assert_eq!(
                orig_view[row].text().trim(),
                rest_view[row].text().trim(),
                "line {row} should be blank"
            );
        }

        // Verify cursor at default position
        let restored_cursor = CursorState::from_vt(&restored);
        assert_eq!(original_cursor, restored_cursor);
        assert_eq!(restored_cursor.col, 0);
        assert_eq!(restored_cursor.row, 0);
        assert!(restored_cursor.visible);
    }
}
