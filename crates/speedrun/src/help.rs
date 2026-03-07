//! Help overlay widget displaying keybindings grouped by category.

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

/// A group of related keybindings.
struct KeybindingGroup {
    title: &'static str,
    bindings: &'static [(&'static str, &'static str)],
}

/// Key column width for aligned formatting.
const KEY_COL_WIDTH: usize = 12;

/// All keybinding groups displayed in the help overlay.
const KEYBINDING_GROUPS: &[KeybindingGroup] = &[
    KeybindingGroup {
        title: "Playback",
        bindings: &[
            ("Space", "Play / pause"),
            ("+ / =", "Speed up"),
            ("-", "Speed down"),
        ],
    },
    KeybindingGroup {
        title: "Navigation",
        bindings: &[
            ("← →", "Seek ±5s"),
            ("Shift+← →", "Seek ±30s"),
            (". ,", "Step forward / back"),
            ("0-9", "Jump to 0%-90%"),
            ("Home / g", "Jump to start"),
            ("End / G", "Jump to end"),
        ],
    },
    KeybindingGroup {
        title: "Markers",
        bindings: &[
            ("] [", "Next / prev marker"),
            ("m", "Add marker"),
            ("M", "Add labeled marker"),
            ("-m", "Auto-pause at markers (CLI)"),
        ],
    },
    KeybindingGroup {
        title: "Search",
        bindings: &[
            ("/", "Search"),
            ("n", "Next match"),
            ("N", "Previous match"),
            ("Esc", "Clear search"),
        ],
    },
    KeybindingGroup {
        title: "Display",
        bindings: &[("Tab", "Toggle controls"), ("?", "Toggle help")],
    },
    KeybindingGroup {
        title: "Application",
        bindings: &[("q / Esc", "Quit")],
    },
];

/// Minimum terminal width to render the overlay.
const MIN_WIDTH: u16 = 10;
/// Minimum terminal height to render the overlay.
const MIN_HEIGHT: u16 = 5;

/// A purely presentational help overlay widget.
///
/// Renders a centered bordered panel with all keybindings grouped
/// by category. No state needed — always renders the same content.
pub struct HelpOverlay;

impl HelpOverlay {
    /// Calculate the width of the widest content line.
    fn content_width() -> u16 {
        let mut max_width: usize = 0;
        for group in KEYBINDING_GROUPS {
            // Group title (no indent — flush left inside padded area)
            max_width = max_width.max(group.title.len());
            for &(_key, desc) in group.bindings {
                // Format: "  {key:<12}  {desc}"
                let w = 2 + KEY_COL_WIDTH + 2 + desc.len();
                max_width = max_width.max(w);
            }
        }
        max_width as u16
    }

    /// Calculate the total number of content lines.
    fn content_height() -> u16 {
        let mut height: u16 = 0;
        for (i, group) in KEYBINDING_GROUPS.iter().enumerate() {
            height += 1; // group title
            height += group.bindings.len() as u16;
            if i < KEYBINDING_GROUPS.len() - 1 {
                height += 1; // blank line between groups
            }
        }
        height
    }

    /// Calculate the overlay outer dimensions (width, height).
    pub fn overlay_size() -> (u16, u16) {
        // +2 for border, +2 for 1-char padding on each side
        let w = Self::content_width() + 4;
        // +2 for border (top/bottom)
        let h = Self::content_height() + 2;
        (w, h)
    }

    /// Calculate the centered overlay `Rect` within the given area.
    ///
    /// If the overlay is larger than the area, it is clamped to fit.
    pub fn centered_rect(area: Rect) -> Rect {
        let (ov_w, ov_h) = Self::overlay_size();
        let w = ov_w.min(area.width);
        let h = ov_h.min(area.height);
        let x = area.x + (area.width.saturating_sub(w)) / 2;
        let y = area.y + (area.height.saturating_sub(h)) / 2;
        Rect::new(x, y, w, h)
    }

    /// Build the styled content lines for the overlay.
    fn build_lines() -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        for (i, group) in KEYBINDING_GROUPS.iter().enumerate() {
            // Group title (bold, no indent)
            lines.push(Line::from(Span::styled(
                group.title,
                Style::default().add_modifier(Modifier::BOLD),
            )));
            // Bindings: "  {key:<12}  {desc}"
            for &(key, desc) in group.bindings {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {key:<KEY_COL_WIDTH$}"),
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::styled(format!("  {desc}"), Style::default()),
                ]));
            }
            // Blank line between groups (except after last)
            if i < KEYBINDING_GROUPS.len() - 1 {
                lines.push(Line::from(""));
            }
        }
        lines
    }
}

impl Widget for HelpOverlay {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Extremely small terminal: skip rendering entirely
        if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
            return;
        }

        let overlay_rect = Self::centered_rect(area);

        // Clear the overlay area to obscure content beneath
        Clear.render(overlay_rect, buf);

        // Fill background with solid color
        let bg_style = Style::default().bg(Color::Black).fg(Color::White);
        for y in overlay_rect.y..overlay_rect.y + overlay_rect.height {
            for x in overlay_rect.x..overlay_rect.x + overlay_rect.width {
                buf.get_mut(x, y).set_style(bg_style);
            }
        }

        // Bordered block with centered title
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Keybindings ")
            .title_alignment(Alignment::Center)
            .style(bg_style);

        let inner = block.inner(overlay_rect);
        block.render(overlay_rect, buf);

        // Add 1-char horizontal padding inside the border
        if inner.width < 3 || inner.height == 0 {
            return;
        }
        let padded = Rect {
            x: inner.x.saturating_add(1),
            width: inner.width.saturating_sub(2),
            ..inner
        };

        // Render keybinding content
        let lines = Self::build_lines();
        let paragraph = Paragraph::new(lines);
        paragraph.render(padded, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helper ───────────────────────────────────────────────────────────────

    fn render_to_string(width: u16, height: u16) -> String {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        HelpOverlay.render(area, &mut buf);

        let mut output = String::new();
        for y in 0..height {
            for x in 0..width {
                output.push_str(buf.get(x, y).symbol());
            }
            if y < height - 1 {
                output.push('\n');
            }
        }
        output
    }

    // ── Snapshot tests ───────────────────────────────────────────────────────

    #[test]
    fn snapshot_standard_60x25() {
        let output = render_to_string(60, 25);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn snapshot_small_30x15() {
        let output = render_to_string(30, 15);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn snapshot_minimum_15x5() {
        let output = render_to_string(15, 5);
        insta::assert_snapshot!(output);
    }

    // ── Dimension tests ──────────────────────────────────────────────────────

    #[test]
    fn overlay_dimensions() {
        let (w, h) = HelpOverlay::overlay_size();
        // 6 groups + 20 bindings + 5 blank lines = 31 content lines + 2 border = 33
        assert_eq!(h, 33);
        // Widest line: 2 + 12 + 2 + 27 = 43 content + 4 (border+padding) = 47
        assert_eq!(w, 47);
    }

    #[test]
    fn content_width_matches_widest_binding() {
        let w = HelpOverlay::content_width();
        // "  {-m:<12}  Auto-pause at markers (CLI)" = 2 + 12 + 2 + 27 = 43
        assert_eq!(w, 43);
    }

    #[test]
    fn content_height_sums_correctly() {
        let h = HelpOverlay::content_height();
        // 6 titles + 20 bindings + 5 blanks = 31
        assert_eq!(h, 31);
    }

    // ── Centering tests ──────────────────────────────────────────────────────

    #[test]
    fn centering_standard_terminal() {
        let area = Rect::new(0, 0, 80, 40);
        let rect = HelpOverlay::centered_rect(area);
        assert_eq!(rect.width, 47);
        assert_eq!(rect.height, 33);
        // Horizontal: (80 - 47) / 2 = 16
        assert_eq!(rect.x, 16);
        // Vertical: (40 - 33) / 2 = 3
        assert_eq!(rect.y, 3);
    }

    #[test]
    fn centering_large_terminal() {
        let area = Rect::new(0, 0, 120, 50);
        let rect = HelpOverlay::centered_rect(area);
        assert_eq!(rect.width, 47);
        assert_eq!(rect.height, 33);
        assert_eq!(rect.x, 36); // (120 - 47) / 2 = 36
        assert_eq!(rect.y, 8); // (50 - 33) / 2 = 8
    }

    #[test]
    fn centering_clamps_to_terminal_size() {
        let area = Rect::new(0, 0, 30, 15);
        let rect = HelpOverlay::centered_rect(area);
        assert_eq!(rect.width, 30);
        assert_eq!(rect.height, 15);
        assert_eq!(rect.x, 0);
        assert_eq!(rect.y, 0);
    }

    #[test]
    fn centering_with_offset_area() {
        let area = Rect::new(5, 3, 80, 40);
        let rect = HelpOverlay::centered_rect(area);
        assert_eq!(rect.width, 47);
        assert_eq!(rect.height, 33);
        assert_eq!(rect.x, 21); // 5 + (80 - 47) / 2
        assert_eq!(rect.y, 6); // 3 + (40 - 33) / 2
    }

    // ── Graceful degradation tests ───────────────────────────────────────────

    #[test]
    fn no_render_when_too_small() {
        let area = Rect::new(0, 0, 8, 4);
        let mut buf = Buffer::empty(area);
        // Fill with 'X' to verify nothing is overwritten
        for y in 0..4 {
            for x in 0..8 {
                buf.get_mut(x, y).set_char('X');
            }
        }
        HelpOverlay.render(area, &mut buf);
        // Buffer should be unchanged
        for y in 0..4u16 {
            for x in 0..8u16 {
                assert_eq!(
                    buf.get(x, y).symbol(),
                    "X",
                    "cell ({x}, {y}) should be unchanged"
                );
            }
        }
    }

    #[test]
    fn no_render_width_below_minimum() {
        let area = Rect::new(0, 0, 9, 20);
        let mut buf = Buffer::empty(area);
        for y in 0..20 {
            for x in 0..9 {
                buf.get_mut(x, y).set_char('X');
            }
        }
        HelpOverlay.render(area, &mut buf);
        assert_eq!(buf.get(0, 0).symbol(), "X");
    }

    #[test]
    fn no_render_height_below_minimum() {
        let area = Rect::new(0, 0, 60, 4);
        let mut buf = Buffer::empty(area);
        for y in 0..4 {
            for x in 0..60 {
                buf.get_mut(x, y).set_char('X');
            }
        }
        HelpOverlay.render(area, &mut buf);
        assert_eq!(buf.get(0, 0).symbol(), "X");
    }

    // ── Search category test ─────────────────────────────────────────────────

    #[test]
    fn test_help_overlay_search_category() {
        let output = render_to_string(60, 30);
        insta::assert_snapshot!(output);
    }
}
