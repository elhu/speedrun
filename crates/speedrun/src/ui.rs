//! Ratatui widgets for rendering terminal session content.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use ratatui::widgets::Widget;
use speedrun_core::CursorState;

/// Tracks the visible portion of the recording when the host terminal
/// is smaller than the recording dimensions.
#[derive(Debug, Default, Clone)]
pub struct ViewportState {
    /// Horizontal scroll offset (column of the recording at the left edge).
    pub scroll_x: u16,
    /// Vertical scroll offset (row of the recording at the top edge).
    pub scroll_y: u16,
}

impl ViewportState {
    /// Update scroll offsets to keep the cursor visible within the viewport.
    ///
    /// The cursor should stay within the visible area, with a small margin
    /// when possible. If the recording fits entirely within the viewport
    /// (in either dimension), the offset for that dimension stays at 0.
    pub fn follow_cursor(
        &mut self,
        cursor_col: u16,
        cursor_row: u16,
        recording_cols: u16,
        recording_rows: u16,
        viewport_width: u16,
        viewport_height: u16,
    ) {
        // Horizontal: no scrolling needed if recording fits
        if recording_cols <= viewport_width {
            self.scroll_x = 0;
        } else {
            // Keep cursor visible — if cursor is outside current viewport, adjust
            let margin = 2u16.min(viewport_width / 4); // small margin, at least 2 cols
            if cursor_col < self.scroll_x + margin {
                self.scroll_x = cursor_col.saturating_sub(margin);
            } else if cursor_col >= self.scroll_x + viewport_width - margin {
                self.scroll_x = (cursor_col + margin + 1).saturating_sub(viewport_width);
            }
            // Clamp: don't scroll past the end of the recording
            let max_scroll = recording_cols.saturating_sub(viewport_width);
            self.scroll_x = self.scroll_x.min(max_scroll);
        }

        // Vertical: same logic
        if recording_rows <= viewport_height {
            self.scroll_y = 0;
        } else {
            let margin = 1u16.min(viewport_height / 4);
            if cursor_row < self.scroll_y + margin {
                self.scroll_y = cursor_row.saturating_sub(margin);
            } else if cursor_row >= self.scroll_y + viewport_height - margin {
                self.scroll_y = (cursor_row + margin + 1).saturating_sub(viewport_height);
            }
            let max_scroll = recording_rows.saturating_sub(viewport_height);
            self.scroll_y = self.scroll_y.min(max_scroll);
        }
    }
}

/// Renders an avt terminal cell grid to a ratatui buffer.
pub struct TerminalView<'a> {
    /// The terminal line grid from `player.screen()`.
    lines: &'a [avt::Line],
    /// Cursor state from `player.cursor()`.
    cursor: CursorState,
    /// Recording dimensions (cols, rows).
    size: (u16, u16),
    /// Horizontal scroll offset.
    scroll_x: u16,
    /// Vertical scroll offset.
    scroll_y: u16,
}

impl<'a> TerminalView<'a> {
    /// Create a new `TerminalView` from the given lines, cursor state, and recording size.
    pub fn new(lines: &'a [avt::Line], cursor: CursorState, size: (u16, u16)) -> Self {
        Self {
            lines,
            cursor,
            size,
            scroll_x: 0,
            scroll_y: 0,
        }
    }

    /// Set the scroll offsets for viewport-aware rendering.
    pub fn with_scroll(mut self, scroll_x: u16, scroll_y: u16) -> Self {
        self.scroll_x = scroll_x;
        self.scroll_y = scroll_y;
        self
    }
}

/// Map an avt color to a ratatui color.
fn map_color(color: Option<avt::Color>) -> Color {
    match color {
        None => Color::Reset,
        Some(avt::Color::Indexed(i)) => Color::Indexed(i),
        Some(avt::Color::RGB(rgb)) => Color::Rgb(rgb.r, rgb.g, rgb.b),
    }
}

/// Map avt Pen attributes to ratatui Modifier flags.
fn map_modifiers(pen: &avt::Pen) -> Modifier {
    let mut mods = Modifier::empty();
    if pen.is_bold() {
        mods |= Modifier::BOLD;
    }
    if pen.is_faint() {
        mods |= Modifier::DIM;
    }
    if pen.is_italic() {
        mods |= Modifier::ITALIC;
    }
    if pen.is_underline() {
        mods |= Modifier::UNDERLINED;
    }
    if pen.is_strikethrough() {
        mods |= Modifier::CROSSED_OUT;
    }
    if pen.is_blink() {
        mods |= Modifier::SLOW_BLINK;
    }
    if pen.is_inverse() {
        mods |= Modifier::REVERSED;
    }
    mods
}

impl Widget for TerminalView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let render_rows = self.size.1.min(area.height);
        let render_cols = self.size.0.min(area.width);

        for row in 0..render_rows {
            let line_idx = (row + self.scroll_y) as usize;
            if line_idx >= self.lines.len() {
                break;
            }

            let line = &self.lines[line_idx];

            for (col_idx, (ch, pen)) in line.cells().enumerate() {
                let src_col = col_idx as u16;
                if src_col < self.scroll_x {
                    continue;
                }
                let dst_col = src_col - self.scroll_x;
                if dst_col >= render_cols {
                    break;
                }

                let cell = buf.get_mut(area.x + dst_col, area.y + row);
                cell.set_char(ch);
                cell.set_fg(map_color(pen.foreground()));
                cell.set_bg(map_color(pen.background()));
                cell.modifier = map_modifiers(&pen);
            }
        }

        // Render cursor (adjusted for scroll offset)
        if self.cursor.visible {
            let cx = (self.cursor.col as u16).checked_sub(self.scroll_x);
            let cy = (self.cursor.row as u16).checked_sub(self.scroll_y);
            if let (Some(cx), Some(cy)) = (cx, cy)
                && cx < render_cols
                && cy < render_rows
            {
                let cell = buf.get_mut(area.x + cx, area.y + cy);
                cell.modifier |= Modifier::REVERSED;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Modifier;
    use rgb::RGB8;

    // ── Color mapping tests ──────────────────────────────────────────────────

    #[test]
    fn map_color_none_is_reset() {
        assert_eq!(map_color(None), Color::Reset);
    }

    #[test]
    fn map_color_indexed() {
        assert_eq!(map_color(Some(avt::Color::Indexed(1))), Color::Indexed(1));
        assert_eq!(map_color(Some(avt::Color::Indexed(0))), Color::Indexed(0));
        assert_eq!(
            map_color(Some(avt::Color::Indexed(255))),
            Color::Indexed(255)
        );
    }

    #[test]
    fn map_color_rgb() {
        let rgb = RGB8 {
            r: 255,
            g: 128,
            b: 0,
        };
        assert_eq!(
            map_color(Some(avt::Color::RGB(rgb))),
            Color::Rgb(255, 128, 0)
        );
    }

    // ── Attribute mapping tests ──────────────────────────────────────────────

    #[test]
    fn map_modifiers_all_attributes() {
        let mut vt = speedrun_core::create_vt(10, 1);
        // Italic(3), Underline(4), Blink(5), Inverse(7), Strikethrough(9)
        vt.feed_str("\x1b[3;4;5;7;9mX");
        let line = &vt.view()[0];
        let (_, pen) = line.cells().next().unwrap();
        let mods = map_modifiers(&pen);
        assert!(mods.contains(Modifier::ITALIC));
        assert!(mods.contains(Modifier::UNDERLINED));
        assert!(mods.contains(Modifier::SLOW_BLINK));
        assert!(mods.contains(Modifier::REVERSED));
        assert!(mods.contains(Modifier::CROSSED_OUT));

        // Test bold separately (bold and faint are mutually exclusive in avt's intensity enum)
        let mut vt2 = speedrun_core::create_vt(10, 1);
        vt2.feed_str("\x1b[1mX");
        let (_, pen2) = vt2.view()[0].cells().next().unwrap();
        assert!(map_modifiers(&pen2).contains(Modifier::BOLD));

        // Test faint
        let mut vt3 = speedrun_core::create_vt(10, 1);
        vt3.feed_str("\x1b[2mX");
        let (_, pen3) = vt3.view()[0].cells().next().unwrap();
        assert!(map_modifiers(&pen3).contains(Modifier::DIM));
    }

    // ── Cursor rendering tests ───────────────────────────────────────────────

    #[test]
    fn cursor_visible_renders_reversed() {
        let mut vt = speedrun_core::create_vt(5, 1);
        vt.feed_str("AB");
        let cursor = speedrun_core::CursorState {
            col: 2,
            row: 0,
            visible: true,
        };
        let view = TerminalView::new(vt.view(), cursor, (5, 1));

        let area = Rect::new(0, 0, 5, 1);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);

        assert!(buf.get(2, 0).modifier.contains(Modifier::REVERSED));
        assert!(!buf.get(0, 0).modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn cursor_hidden_not_rendered() {
        let mut vt = speedrun_core::create_vt(5, 1);
        vt.feed_str("AB");
        let cursor = speedrun_core::CursorState {
            col: 2,
            row: 0,
            visible: false,
        };
        let view = TerminalView::new(vt.view(), cursor, (5, 1));

        let area = Rect::new(0, 0, 5, 1);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);

        assert!(!buf.get(2, 0).modifier.contains(Modifier::REVERSED));
    }

    // ── Viewport scroll offset tests ─────────────────────────────────────────

    #[test]
    fn viewport_no_scroll_when_fits() {
        let mut vp = ViewportState::default();
        vp.follow_cursor(5, 5, 80, 24, 100, 30);
        assert_eq!(vp.scroll_x, 0);
        assert_eq!(vp.scroll_y, 0);
    }

    #[test]
    fn viewport_scrolls_to_follow_cursor_right() {
        let mut vp = ViewportState::default();
        vp.follow_cursor(60, 0, 80, 24, 40, 24);
        assert!(vp.scroll_x > 0);
        assert!(vp.scroll_x + 40 > 60);
    }

    #[test]
    fn viewport_scrolls_to_follow_cursor_down() {
        let mut vp = ViewportState::default();
        vp.follow_cursor(0, 40, 80, 50, 80, 24);
        assert!(vp.scroll_y > 0);
        assert!(vp.scroll_y + 24 > 40);
    }

    #[test]
    fn viewport_clamps_scroll_at_end() {
        let mut vp = ViewportState::default();
        vp.follow_cursor(79, 49, 80, 50, 40, 24);
        assert!(vp.scroll_x <= 40);
        assert!(vp.scroll_y <= 26);
    }

    // ── End-to-end snapshot test ─────────────────────────────────────────────

    #[test]
    fn snapshot_render_minimal_v2() {
        let file = std::fs::File::open("../../testdata/minimal_v2.cast").unwrap();
        let mut player = speedrun_core::Player::load(file).unwrap();
        player.seek(player.duration());

        let (cols, rows) = player.size();
        let view = TerminalView::new(player.screen(), player.cursor(), (cols, rows));

        let area = Rect::new(0, 0, cols, rows);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);

        let text_lines: Vec<String> = (0..rows)
            .map(|y| {
                (0..cols)
                    .map(|x| buf.get(x, y).symbol().to_string())
                    .collect()
            })
            .collect();

        insta::assert_debug_snapshot!(text_lines);
    }
}
