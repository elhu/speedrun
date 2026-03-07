use avt::{Color, Pen};
use rgb::RGB8;

/// Base configuration for color resolution during export.
pub struct ExportOptions {
    /// Default foreground color when pen has no explicit foreground.
    pub default_fg: RGB8,
    /// Default background color when pen has no explicit background.
    pub default_bg: RGB8,
    /// When true, bold text with standard colors (indices 0-7) resolves
    /// to the bright variant (indices 8-15).
    pub bold_brightens: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            default_fg: RGB8::new(208, 208, 208),
            default_bg: RGB8::new(30, 30, 30),
            bold_brightens: true,
        }
    }
}

/// Resolves `avt::Color` values to concrete `rgb::RGB8` using the xterm-256
/// color palette.
pub struct Palette {
    options: ExportOptions,
}

/// The standard xterm-256 colors (indices 0-15).
const STANDARD_COLORS: [RGB8; 16] = [
    // Standard colors (0-7)
    RGB8::new(0, 0, 0),       // 0: Black
    RGB8::new(205, 0, 0),     // 1: Red
    RGB8::new(0, 205, 0),     // 2: Green
    RGB8::new(205, 205, 0),   // 3: Yellow
    RGB8::new(0, 0, 238),     // 4: Blue
    RGB8::new(205, 0, 205),   // 5: Magenta
    RGB8::new(0, 205, 205),   // 6: Cyan
    RGB8::new(229, 229, 229), // 7: White
    // Bright colors (8-15)
    RGB8::new(127, 127, 127), // 8: Bright Black (Gray)
    RGB8::new(255, 0, 0),     // 9: Bright Red
    RGB8::new(0, 255, 0),     // 10: Bright Green
    RGB8::new(255, 255, 0),   // 11: Bright Yellow
    RGB8::new(92, 92, 255),   // 12: Bright Blue
    RGB8::new(255, 0, 255),   // 13: Bright Magenta
    RGB8::new(0, 255, 255),   // 14: Bright Cyan
    RGB8::new(255, 255, 255), // 15: Bright White
];

impl Palette {
    /// Create a new palette with the given export options.
    pub fn new(options: ExportOptions) -> Self {
        Self { options }
    }

    /// Resolve an `avt::Color` to a concrete `RGB8` value using the xterm-256
    /// color palette.
    pub fn resolve(&self, color: Color) -> RGB8 {
        match color {
            Color::Indexed(n) => self.resolve_indexed(n),
            Color::RGB(rgb) => rgb,
        }
    }

    /// Resolve an indexed color (0-255) to RGB8.
    fn resolve_indexed(&self, n: u8) -> RGB8 {
        match n {
            // Standard and bright colors
            0..=15 => STANDARD_COLORS[n as usize],
            // 6x6x6 color cube (indices 16-231)
            16..=231 => {
                let n = n - 16;
                let r = n / 36;
                let g = (n % 36) / 6;
                let b = n % 6;
                let to_value = |component: u8| -> u8 {
                    if component == 0 {
                        0
                    } else {
                        55 + 40 * component
                    }
                };
                RGB8::new(to_value(r), to_value(g), to_value(b))
            }
            // Grayscale ramp (indices 232-255)
            232..=255 => {
                let value = 8 + 10 * (n - 232);
                RGB8::new(value, value, value)
            }
        }
    }

    /// Resolve the foreground and background colors for a cell, accounting for
    /// bold brightening and inverse attributes.
    ///
    /// Returns `(foreground_rgb, background_rgb)`.
    pub fn resolve_cell_colors(&self, pen: &Pen) -> (RGB8, RGB8) {
        // Resolve foreground
        let fg = match pen.foreground() {
            Some(color) => {
                let color = if self.options.bold_brightens && pen.is_bold() {
                    brighten_if_standard(color)
                } else {
                    color
                };
                self.resolve(color)
            }
            None => self.options.default_fg,
        };

        // Resolve background
        let bg = match pen.background() {
            Some(color) => self.resolve(color),
            None => self.options.default_bg,
        };

        // Apply inverse: swap fg and bg
        if pen.is_inverse() { (bg, fg) } else { (fg, bg) }
    }
}

/// If the color is a standard indexed color (0-7), shift it to the bright
/// variant (8-15). All other colors are returned unchanged.
fn brighten_if_standard(color: Color) -> Color {
    match color {
        Color::Indexed(n @ 0..=7) => Color::Indexed(n + 8),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_palette() -> Palette {
        Palette::new(ExportOptions::default())
    }

    /// Helper: create a bold Pen with a specific foreground color.
    fn bold_pen_with_fg(index: u8) -> Pen {
        let mut vt = avt::Vt::new(10, 1);
        vt.feed_str(&format!("\x1b[1;38;5;{index}m "));
        let line = &vt.view()[0];
        line.cells().next().unwrap().1
    }

    /// Helper: create an inverse Pen with foreground and background.
    fn inverse_pen_with_fg_bg(fg_index: u8, bg_index: u8) -> Pen {
        let mut vt = avt::Vt::new(10, 1);
        vt.feed_str(&format!("\x1b[7;38;5;{fg_index};48;5;{bg_index}m "));
        let line = &vt.view()[0];
        line.cells().next().unwrap().1
    }

    /// Helper: create a default inverse Pen (no explicit fg/bg).
    fn inverse_pen_default() -> Pen {
        let mut vt = avt::Vt::new(10, 1);
        vt.feed_str("\x1b[7m ");
        let line = &vt.view()[0];
        line.cells().next().unwrap().1
    }

    // ---- Standard color table tests ----

    #[test]
    fn test_indexed_color_0_black() {
        let palette = default_palette();
        assert_eq!(palette.resolve(Color::Indexed(0)), RGB8::new(0, 0, 0));
    }

    #[test]
    fn test_indexed_color_1_red() {
        let palette = default_palette();
        assert_eq!(palette.resolve(Color::Indexed(1)), RGB8::new(205, 0, 0));
    }

    #[test]
    fn test_indexed_color_9_bright_red() {
        let palette = default_palette();
        assert_eq!(palette.resolve(Color::Indexed(9)), RGB8::new(255, 0, 0));
    }

    #[test]
    fn test_indexed_color_15_bright_white() {
        let palette = default_palette();
        assert_eq!(
            palette.resolve(Color::Indexed(15)),
            RGB8::new(255, 255, 255)
        );
    }

    // ---- Color cube tests ----

    #[test]
    fn test_color_cube_index_16() {
        let palette = default_palette();
        assert_eq!(palette.resolve(Color::Indexed(16)), RGB8::new(0, 0, 0));
    }

    #[test]
    fn test_color_cube_index_196() {
        let palette = default_palette();
        // n=196: r=(196-16)/36=5, g=((196-16)%36)/6=0, b=(196-16)%6=0
        // r=255, g=0, b=0
        assert_eq!(palette.resolve(Color::Indexed(196)), RGB8::new(255, 0, 0));
    }

    #[test]
    fn test_color_cube_index_231() {
        let palette = default_palette();
        // n=231: r=(231-16)/36=5, g=((231-16)%36)/6=5, b=(231-16)%6=5
        // All components 5 -> 255
        assert_eq!(
            palette.resolve(Color::Indexed(231)),
            RGB8::new(255, 255, 255)
        );
    }

    // ---- Grayscale ramp tests ----

    #[test]
    fn test_grayscale_index_232() {
        let palette = default_palette();
        // 8 + 10 * 0 = 8
        assert_eq!(palette.resolve(Color::Indexed(232)), RGB8::new(8, 8, 8));
    }

    #[test]
    fn test_grayscale_index_255() {
        let palette = default_palette();
        // 8 + 10 * 23 = 238
        assert_eq!(
            palette.resolve(Color::Indexed(255)),
            RGB8::new(238, 238, 238)
        );
    }

    // ---- Edge indexed value test ----

    // Test 10 is skipped: avt::Color::Indexed uses u8 (0-255), so index 256
    // is impossible at the type level. All 256 values are handled by the
    // match arms in resolve_indexed, so there is no out-of-range case.

    // ---- RGB passthrough test ----

    #[test]
    fn test_rgb_passthrough() {
        let palette = default_palette();
        assert_eq!(
            palette.resolve(Color::RGB(RGB8::new(42, 128, 255))),
            RGB8::new(42, 128, 255)
        );
    }

    // ---- Bold brightening tests ----

    #[test]
    fn test_bold_brightens_standard_color() {
        let palette = default_palette();
        let pen = bold_pen_with_fg(1);
        assert!(pen.is_bold());
        let (fg, _bg) = palette.resolve_cell_colors(&pen);
        // Bold + Indexed(1) should resolve as Indexed(9) = bright red
        assert_eq!(fg, RGB8::new(255, 0, 0));
    }

    #[test]
    fn test_bold_brightens_disabled() {
        let palette = Palette::new(ExportOptions {
            bold_brightens: false,
            ..ExportOptions::default()
        });
        let pen = bold_pen_with_fg(1);
        assert!(pen.is_bold());
        let (fg, _bg) = palette.resolve_cell_colors(&pen);
        // With bold_brightens disabled, should stay as Indexed(1) = standard red
        assert_eq!(fg, RGB8::new(205, 0, 0));
    }

    #[test]
    fn test_bold_does_not_brighten_high_index() {
        let palette = default_palette();
        let pen = bold_pen_with_fg(100);
        assert!(pen.is_bold());
        let (fg, _bg) = palette.resolve_cell_colors(&pen);
        // Indexed(100) is in the color cube, should NOT be shifted
        assert_eq!(fg, palette.resolve(Color::Indexed(100)));
    }

    // ---- Inverse handling tests ----

    #[test]
    fn test_inverse_swaps_fg_bg() {
        let palette = default_palette();
        let pen = inverse_pen_with_fg_bg(1, 4);
        assert!(pen.is_inverse());
        let (fg, bg) = palette.resolve_cell_colors(&pen);
        // Foreground should be blue (originally bg), background should be red (originally fg)
        let red = palette.resolve(Color::Indexed(1));
        let blue = palette.resolve(Color::Indexed(4));
        assert_eq!(fg, blue);
        assert_eq!(bg, red);
    }

    #[test]
    fn test_inverse_with_defaults() {
        let palette = default_palette();
        let pen = inverse_pen_default();
        assert!(pen.is_inverse());
        let (fg, bg) = palette.resolve_cell_colors(&pen);
        // Swapped: fg becomes default_bg, bg becomes default_fg
        assert_eq!(fg, RGB8::new(30, 30, 30)); // default_bg
        assert_eq!(bg, RGB8::new(208, 208, 208)); // default_fg
    }

    // ---- Default color tests ----

    #[test]
    fn test_default_fg_bg_configurable() {
        let palette = Palette::new(ExportOptions {
            default_fg: RGB8::new(0, 255, 0),
            ..ExportOptions::default()
        });
        let pen = Pen::default();
        let (fg, _bg) = palette.resolve_cell_colors(&pen);
        assert_eq!(fg, RGB8::new(0, 255, 0));
    }
}
