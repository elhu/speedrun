//! Ratatui widgets for rendering terminal session content.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use ratatui::widgets::Widget;
use speedrun_core::CursorState;

/// Renders an avt terminal cell grid to a ratatui buffer.
pub struct TerminalView<'a> {
    /// The terminal line grid from `player.screen()`.
    lines: &'a [avt::Line],
    /// Cursor state from `player.cursor()`.
    cursor: CursorState,
    /// Recording dimensions (cols, rows).
    size: (u16, u16),
}

impl<'a> TerminalView<'a> {
    /// Create a new `TerminalView` from the given lines, cursor state, and recording size.
    pub fn new(lines: &'a [avt::Line], cursor: CursorState, size: (u16, u16)) -> Self {
        Self {
            lines,
            cursor,
            size,
        }
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
            let line_idx = row as usize;
            if line_idx >= self.lines.len() {
                break;
            }

            let line = &self.lines[line_idx];

            for (col, (ch, pen)) in line.cells().enumerate() {
                if col as u16 >= render_cols {
                    break;
                }

                let cell = buf.get_mut(area.x + col as u16, area.y + row);
                cell.set_char(ch);
                cell.set_fg(map_color(pen.foreground()));
                cell.set_bg(map_color(pen.background()));
                cell.modifier = map_modifiers(&pen);
            }
        }

        // Render cursor
        if self.cursor.visible {
            let cx = self.cursor.col as u16;
            let cy = self.cursor.row as u16;
            if cx < render_cols && cy < render_rows {
                let cell = buf.get_mut(area.x + cx, area.y + cy);
                cell.modifier |= Modifier::REVERSED;
            }
        }
    }
}
