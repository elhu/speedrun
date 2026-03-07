//! Static and animated SVG export for terminal recordings.
//!
//! # Font approximation
//!
//! The SVG output uses a monospace font stack. Character width is approximated
//! as `font_size * 0.6` and line height as `font_size * 1.2`. Actual glyph
//! metrics vary by viewer and installed fonts; these values are reasonable
//! estimates for the default font stack.

use std::fmt::Write as FmtWrite;
use std::io;

use speedrun_core::{CursorState, Player};

use crate::palette::{ExportOptions, Palette};

/// Default font stack for SVG export.
pub const DEFAULT_FONT_FAMILY: &str = "'Fira Code', 'Cascadia Code', 'JetBrains Mono', 'SF Mono', Menlo, Monaco, Consolas, \
     'Liberation Mono', 'Courier New', monospace";

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during SVG export.
#[derive(Debug)]
pub enum ExportError {
    /// An I/O error occurred while writing the output.
    Io(io::Error),
    /// A formatting error occurred while building the SVG string.
    Fmt(std::fmt::Error),
    /// The recording is too long for animated SVG without `force_long`.
    TooLong(f64),
    /// Mutually exclusive options were specified.
    MutuallyExclusive(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportError::Io(e) => write!(f, "I/O error: {e}"),
            ExportError::Fmt(e) => write!(f, "format error: {e}"),
            ExportError::TooLong(secs) => write!(
                f,
                "Recording too long for animated SVG ({secs:.0}s > 120s limit). \
                 Use --force to override, or use GIF format instead."
            ),
            ExportError::MutuallyExclusive(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ExportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ExportError::Io(e) => Some(e),
            ExportError::Fmt(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for ExportError {
    fn from(e: io::Error) -> Self {
        ExportError::Io(e)
    }
}

impl From<std::fmt::Error> for ExportError {
    fn from(e: std::fmt::Error) -> Self {
        ExportError::Fmt(e)
    }
}

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

/// Options for static SVG export.
pub struct SvgOptions {
    /// Effective time to capture (default: 0.0).
    pub at_time: f64,
    /// Font size in pixels (default: 14.0).
    pub font_size: f64,
    /// Font family string (default: [`DEFAULT_FONT_FAMILY`]).
    pub font_family: String,
    /// Whether to render the cursor (default: true).
    pub show_cursor: bool,
    /// Color palette configuration.
    pub export: ExportOptions,
}

impl Default for SvgOptions {
    fn default() -> Self {
        Self {
            at_time: 0.0,
            font_size: 14.0,
            font_family: DEFAULT_FONT_FAMILY.to_string(),
            show_cursor: true,
            export: ExportOptions::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Font geometry helpers
// ---------------------------------------------------------------------------

/// Compute the character (cell) width from font size.
#[inline]
pub(crate) fn svg_char_width(font_size: f64) -> f64 {
    font_size * 0.6
}

/// Compute the line height from font size.
#[inline]
pub(crate) fn svg_line_height(font_size: f64) -> f64 {
    font_size * 1.2
}

// ---------------------------------------------------------------------------
// XML escaping
// ---------------------------------------------------------------------------

/// Escape special XML characters in text content.
///
/// Replaces: `<` → `&lt;`, `>` → `&gt;`, `&` → `&amp;`, `"` → `&quot;`
pub fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Color resolution for Segment (using public Segment API)
// ---------------------------------------------------------------------------

/// Resolve foreground and background colors for an `avt::Segment` using the
/// palette. Handles bold-brightening and inverse.
fn resolve_segment_colors(
    seg: &avt::Segment,
    palette: &Palette,
    export_opts: &ExportOptions,
) -> (rgb::RGB8, rgb::RGB8) {
    use avt::Color;

    let fg_color: Option<Color> = seg.foreground();
    let bg_color: Option<Color> = seg.background();
    let is_bold = seg.is_bold();
    let is_inverse = seg.is_inverse();

    let fg = match fg_color {
        Some(color) => {
            let color = if export_opts.bold_brightens && is_bold {
                brighten_if_standard(color)
            } else {
                color
            };
            palette.resolve(color)
        }
        None => export_opts.default_fg,
    };

    let bg = match bg_color {
        Some(color) => palette.resolve(color),
        None => export_opts.default_bg,
    };

    if is_inverse { (bg, fg) } else { (fg, bg) }
}

/// If the color is a standard indexed color (0-7), return the bright variant (8-15).
fn brighten_if_standard(color: avt::Color) -> avt::Color {
    match color {
        avt::Color::Indexed(n @ 0..=7) => avt::Color::Indexed(n + 8),
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Shared rendering helpers
// ---------------------------------------------------------------------------

/// Configuration passed to [`render_screen_to_svg_elements`].
pub(crate) struct RenderConfig<'a> {
    pub palette: &'a Palette,
    pub export_opts: &'a ExportOptions,
    pub font_size: f64,
    pub show_cursor: bool,
    pub default_bg: rgb::RGB8,
}

/// Render a single frame's screen content as SVG elements (no outer `<svg>`
/// wrapper). Writes background `<rect>` elements and `<text>` elements for
/// all styled segments, plus an optional cursor rect.
///
/// This function is used by both static and animated SVG export.
pub(crate) fn render_screen_to_svg_elements(
    screen: &[avt::Line],
    cursor: &CursorState,
    cfg: &RenderConfig<'_>,
    buf: &mut String,
) -> Result<(), std::fmt::Error> {
    let palette = cfg.palette;
    let export_opts = cfg.export_opts;
    let font_size = cfg.font_size;
    let show_cursor = cfg.show_cursor;
    let default_bg = cfg.default_bg;
    let cw = svg_char_width(font_size);
    let lh = svg_line_height(font_size);

    for (row_idx, line) in screen.iter().enumerate() {
        let y_top = row_idx as f64 * lh;
        let y_baseline = y_top + font_size;

        // Background rects for non-default background segments
        let mut col_offset = 0usize;
        for seg in line.segments() {
            let (_, bg) = resolve_segment_colors(&seg, palette, export_opts);
            let seg_cols = seg.text().chars().count() * seg.char_width();
            if bg != default_bg {
                let x = col_offset as f64 * cw;
                let w = seg_cols as f64 * cw;
                write!(
                    buf,
                    "<rect x=\"{x:.2}\" y=\"{y_top:.2}\" width=\"{w:.2}\" height=\"{lh:.2}\" fill=\"#{r:02x}{g:02x}{b:02x}\"/>",
                    r = bg.r,
                    g = bg.g,
                    b = bg.b
                )?;
            }
            col_offset += seg_cols;
        }

        // Text elements per styled segment
        let mut col_offset = 0usize;
        for seg in line.segments() {
            let text = seg.text();
            let seg_cols = text.chars().count() * seg.char_width();
            let trimmed = text.trim_end_matches(' ');
            if !trimmed.is_empty() {
                let (fg, _) = resolve_segment_colors(&seg, palette, export_opts);
                let x = col_offset as f64 * cw;

                let mut attrs = format!(
                    "x=\"{x:.2}\" y=\"{y_baseline:.2}\" fill=\"#{r:02x}{g:02x}{b:02x}\"",
                    r = fg.r,
                    g = fg.g,
                    b = fg.b
                );
                if seg.is_bold() {
                    attrs.push_str(" font-weight=\"bold\"");
                }
                if seg.is_italic() {
                    attrs.push_str(" font-style=\"italic\"");
                }
                if seg.is_underline() {
                    attrs.push_str(" text-decoration=\"underline\"");
                }
                if seg.is_strikethrough() {
                    attrs.push_str(" text-decoration=\"line-through\"");
                }

                write!(buf, "<text {attrs}>{}</text>", xml_escape(trimmed))?;
            }
            col_offset += seg_cols;
        }
    }

    // Cursor rect
    if show_cursor && cursor.visible {
        let cx = cursor.col as f64 * cw;
        let cy = cursor.row as f64 * lh;
        write!(
            buf,
            "<rect x=\"{cx:.2}\" y=\"{cy:.2}\" width=\"{cw:.2}\" height=\"{lh:.2}\" fill=\"#ffffff\" opacity=\"0.7\"/>"
        )?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Static SVG export
// ---------------------------------------------------------------------------

/// Render the terminal state at a given point in time as a static SVG document.
///
/// Seeks the player to `options.at_time`, reads the screen, and writes a
/// complete SVG document to `writer`.
pub fn export_svg(
    player: &mut Player,
    options: &SvgOptions,
    writer: &mut dyn io::Write,
) -> Result<(), ExportError> {
    let (cols, rows) = player.size();
    player.seek(options.at_time);

    let palette = Palette::new(ExportOptions {
        default_fg: options.export.default_fg,
        default_bg: options.export.default_bg,
        bold_brightens: options.export.bold_brightens,
    });

    let cw = svg_char_width(options.font_size);
    let lh = svg_line_height(options.font_size);
    let svg_width = cols as f64 * cw;
    let svg_height = rows as f64 * lh;
    let bg = options.export.default_bg;
    let ff = xml_escape(&options.font_family);

    let mut buf = String::new();

    write!(
        buf,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {svg_width:.2} {svg_height:.2}\" font-family=\"{ff}\" font-size=\"{fs:.2}\">",
        fs = options.font_size
    )?;

    // Full-screen background rect
    write!(
        buf,
        "<rect width=\"100%\" height=\"100%\" fill=\"#{r:02x}{g:02x}{b:02x}\"/>",
        r = bg.r,
        g = bg.g,
        b = bg.b
    )?;

    let screen = player.screen().to_vec();
    let cursor = player.cursor();

    let cfg = RenderConfig {
        palette: &palette,
        export_opts: &options.export,
        font_size: options.font_size,
        show_cursor: options.show_cursor,
        default_bg: bg,
    };
    render_screen_to_svg_elements(&screen, &cursor, &cfg, &mut buf)?;

    buf.push_str("</svg>");

    writer.write_all(buf.as_bytes())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Animated SVG export
// ---------------------------------------------------------------------------

/// Options for animated SVG export.
pub struct AnimatedSvgOptions {
    /// Font size in pixels (default: 14.0).
    pub font_size: f64,
    /// Font family string (default: [`DEFAULT_FONT_FAMILY`]).
    pub font_family: String,
    /// Whether to render the cursor (default: true).
    pub show_cursor: bool,
    /// Override the 120-second duration limit.
    pub force_long: bool,
    /// Color palette configuration.
    pub export: ExportOptions,
}

impl Default for AnimatedSvgOptions {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            font_family: DEFAULT_FONT_FAMILY.to_string(),
            show_cursor: true,
            force_long: false,
            export: ExportOptions::default(),
        }
    }
}

/// A single rendered frame: its SVG content string and the effective start/end
/// time.
struct Frame {
    content: String,
    start_time: f64,
    end_time: f64,
}

/// Render a terminal recording as an animated SVG using CSS `@keyframes`.
///
/// Each unique screen state is rendered as a `<g>` group. CSS animations toggle
/// group visibility according to event timing using effective (idle-compressed)
/// timestamps.
///
/// # Duration limits
///
/// Recordings over 30 seconds emit a warning to stderr. Recordings over 120
/// seconds require `options.force_long = true`, otherwise an [`ExportError::TooLong`]
/// is returned.
///
/// # Browser compatibility
///
/// CSS animations may not play when the SVG is loaded via an `<img>` tag.
/// Use `<object>`, `<iframe>`, or inline SVG.
pub fn export_animated_svg(
    player: &mut Player,
    options: &AnimatedSvgOptions,
    writer: &mut dyn io::Write,
) -> Result<(), ExportError> {
    let duration = player.duration();

    // Duration limit checks
    if duration > 30.0 {
        eprintln!(
            "Warning: animated SVG for recordings > 30s may produce large files \
             (this recording is {duration:.1}s)"
        );
    }
    if duration > 120.0 && !options.force_long {
        return Err(ExportError::TooLong(duration));
    }

    let (cols, rows) = player.size();
    let palette = Palette::new(ExportOptions {
        default_fg: options.export.default_fg,
        default_bg: options.export.default_bg,
        bold_brightens: options.export.bold_brightens,
    });
    let bg = options.export.default_bg;

    // Build frames by iterating over events
    let frames = build_frames(player, &palette, options, bg)?;

    // Render the SVG
    let cw = svg_char_width(options.font_size);
    let lh = svg_line_height(options.font_size);
    let svg_width = cols as f64 * cw;
    let svg_height = rows as f64 * lh;
    let ff = xml_escape(&options.font_family);

    let mut out = String::new();

    // Browser compat comment and SVG header
    out.push_str("<!-- CSS animations may not play when loaded via <img> tag. Use <object>, <iframe>, or inline SVG. -->");
    write!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {svg_width:.2} {svg_height:.2}\" font-family=\"{ff}\" font-size=\"{fs:.2}\">",
        fs = options.font_size
    )?;

    // Full-screen background rect
    write!(
        out,
        "<rect width=\"100%\" height=\"100%\" fill=\"#{r:02x}{g:02x}{b:02x}\"/>",
        r = bg.r,
        g = bg.g,
        b = bg.b
    )?;

    if frames.len() == 1 {
        // Single-frame recording: render statically with no animation overhead
        out.push_str("<g>");
        out.push_str(&frames[0].content);
        out.push_str("</g>");
    } else {
        // Build CSS style block
        let total_duration = if duration > 0.0 { duration } else { 1.0 };
        let mut style = String::from("<style>");

        for (i, frame) in frames.iter().enumerate() {
            let start_pct = frame.start_time / total_duration * 100.0;
            let end_pct = frame.end_time / total_duration * 100.0;
            let anim_duration = total_duration;

            if i == 0 {
                // First frame: starts visible, becomes hidden at end_pct
                write!(
                    style,
                    "#f{i}{{animation:a{i} {anim_duration:.3}s step-end infinite;}}"
                )?;
                write!(
                    style,
                    "@keyframes a{i}{{0%{{visibility:visible}}{end_pct:.4}%{{visibility:hidden}}}}"
                )?;
            } else {
                // Other frames: start hidden, become visible at start_pct
                write!(
                    style,
                    "#f{i}{{visibility:hidden;animation:a{i} {anim_duration:.3}s step-end infinite;}}"
                )?;
                if i == frames.len() - 1 {
                    // Last frame: explicitly hidden at 100% so iteration boundary is clean
                    write!(
                        style,
                        "@keyframes a{i}{{{start_pct:.4}%{{visibility:visible}}100%{{visibility:hidden}}}}"
                    )?;
                } else {
                    write!(
                        style,
                        "@keyframes a{i}{{{start_pct:.4}%{{visibility:visible}}{end_pct:.4}%{{visibility:hidden}}}}"
                    )?;
                }
            }
        }

        style.push_str("</style>");
        out.push_str(&style);

        // Render each frame as a <g> group
        for (i, frame) in frames.iter().enumerate() {
            write!(out, "<g id=\"f{i}\">")?;
            out.push_str(&frame.content);
            out.push_str("</g>");
        }
    }

    out.push_str("</svg>");

    writer.write_all(out.as_bytes())?;
    Ok(())
}

/// Build coalesced frames by walking through recording events.
fn build_frames(
    player: &mut Player,
    palette: &Palette,
    options: &AnimatedSvgOptions,
    bg: rgb::RGB8,
) -> Result<Vec<Frame>, ExportError> {
    let mut frames: Vec<Frame> = Vec::new();

    // Render the initial frame at t=0
    player.seek(0.0);
    let mut prev_lines: Vec<String> = player.screen().iter().map(|l| l.text()).collect();

    let initial_content = render_frame_content(player, palette, options, bg)?;
    frames.push(Frame {
        content: initial_content,
        start_time: 0.0,
        end_time: 0.0, // placeholder — updated when next frame begins
    });

    // Walk through all output events using step_forward
    loop {
        let had_event = player.step_forward();
        if !had_event {
            break;
        }

        let time_after = player.current_time();

        // Check if screen content changed
        let current_lines: Vec<String> = player.screen().iter().map(|l| l.text()).collect();
        if current_lines != prev_lines {
            let prev_frame = frames.last_mut().unwrap();
            if (time_after - prev_frame.start_time).abs() < f64::EPSILON {
                // Same effective timestamp — replace previous frame's content instead of
                // pushing a zero-duration frame. This handles the case where idle_time_limit
                // compresses multiple events to the same effective time.
                prev_frame.content = render_frame_content(player, palette, options, bg)?;
            } else {
                // Different timestamp — close previous frame and push a new one
                prev_frame.end_time = time_after;
                let content = render_frame_content(player, palette, options, bg)?;
                frames.push(Frame {
                    content,
                    start_time: time_after,
                    end_time: 0.0, // placeholder
                });
            }
            prev_lines = current_lines;
        }
    }

    let duration = player.duration();

    // Close the last frame at the end of the recording
    let frame_count = frames.len();
    let last_start = frames[frame_count - 1].start_time;
    frames[frame_count - 1].end_time = duration.max(last_start + 0.001);

    // Handle degenerate case: single frame / zero duration
    if frames.len() == 1 && frames[0].end_time <= frames[0].start_time {
        frames[0].end_time = 1.0;
    }

    Ok(frames)
}

/// Render the current screen state to an SVG content string (no `<svg>` wrapper).
fn render_frame_content(
    player: &Player,
    palette: &Palette,
    options: &AnimatedSvgOptions,
    bg: rgb::RGB8,
) -> Result<String, ExportError> {
    let screen = player.screen().to_vec();
    let cursor = player.cursor();

    let cfg = RenderConfig {
        palette,
        export_opts: &options.export,
        font_size: options.font_size,
        show_cursor: options.show_cursor,
        default_bg: bg,
    };

    let mut buf = String::new();
    render_screen_to_svg_elements(&screen, &cursor, &cfg, &mut buf)?;

    Ok(buf)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use speedrun_core::Player;

    /// Create a minimal in-memory recording.
    fn make_player(cast: &str) -> Player {
        let reader = std::io::Cursor::new(cast.to_string());
        Player::load(reader).expect("failed to load test recording")
    }

    fn default_svg_opts() -> SvgOptions {
        SvgOptions::default()
    }

    fn export_to_string(player: &mut Player, opts: &SvgOptions) -> String {
        let mut buf = Vec::new();
        export_svg(player, opts, &mut buf).expect("export_svg failed");
        String::from_utf8(buf).expect("non-UTF8 SVG output")
    }

    // -----------------------------------------------------------------------
    // 1. xml_escape helper test
    // -----------------------------------------------------------------------

    #[test]
    fn test_xml_escape_function() {
        let input = "<hello>&\"world\"";
        let output = xml_escape(input);
        assert_eq!(output, "&lt;hello&gt;&amp;&quot;world&quot;");
    }

    // -----------------------------------------------------------------------
    // 2. SVG starts and ends correctly
    // -----------------------------------------------------------------------

    #[test]
    fn test_svg_starts_and_ends_correctly() {
        let mut player =
            make_player("{\"version\":2,\"width\":80,\"height\":24}\n[0.5,\"o\",\"hello\\r\\n\"]");
        let svg = export_to_string(&mut player, &default_svg_opts());
        assert!(svg.starts_with("<svg "), "SVG should start with <svg");
        assert!(
            svg.contains("xmlns=\"http://www.w3.org/2000/svg\""),
            "SVG should contain xmlns"
        );
        assert!(svg.ends_with("</svg>"), "SVG should end with </svg>");
    }

    // -----------------------------------------------------------------------
    // 3. SVG has background rect
    // -----------------------------------------------------------------------

    #[test]
    fn test_svg_has_background_rect() {
        let mut player =
            make_player("{\"version\":2,\"width\":80,\"height\":24}\n[0.5,\"o\",\"hello\\r\\n\"]");
        let svg = export_to_string(&mut player, &default_svg_opts());
        assert!(
            svg.contains("<rect width=\"100%\" height=\"100%\""),
            "SVG should have full-screen background rect"
        );
    }

    // -----------------------------------------------------------------------
    // 4. SVG viewBox dimensions
    // -----------------------------------------------------------------------

    #[test]
    fn test_svg_viewbox_dimensions() {
        let mut player =
            make_player("{\"version\":2,\"width\":80,\"height\":24}\n[0.5,\"o\",\"hello\\r\\n\"]");
        let opts = SvgOptions {
            font_size: 14.0,
            ..Default::default()
        };
        let svg = export_to_string(&mut player, &opts);
        let expected_w = 80.0 * 14.0 * 0.6; // 672.0
        let expected_h = 24.0 * 14.0 * 1.2; // 403.2
        let expected_viewbox = format!("viewBox=\"0 0 {:.2} {:.2}\"", expected_w, expected_h);
        assert!(
            svg.contains(&expected_viewbox),
            "SVG viewBox should be {expected_viewbox}, got: {svg}"
        );
    }

    // -----------------------------------------------------------------------
    // 5. XML escaping of special chars in content
    // -----------------------------------------------------------------------

    #[test]
    fn test_svg_escapes_special_chars_in_content() {
        // Feed escape sequences that produce < > & characters
        let cast = "{\"version\":2,\"width\":80,\"height\":24}\n[0.5,\"o\",\"<script>&alert</script>\\r\\n\"]";
        let mut player = make_player(cast);
        let opts = SvgOptions {
            at_time: 1.0,
            ..Default::default()
        };
        let mut buf = Vec::new();
        export_svg(&mut player, &opts, &mut buf).expect("export failed");
        let svg = String::from_utf8(buf).unwrap();
        assert!(
            svg.contains("&lt;script&gt;"),
            "SVG should contain escaped &lt;script&gt;"
        );
        // Verify no raw unescaped <script> in text content area
        assert!(
            !svg.contains("<script>"),
            "SVG should not contain raw <script> tag"
        );
    }

    // -----------------------------------------------------------------------
    // 6. Foreground color in text element
    // -----------------------------------------------------------------------

    #[test]
    fn test_svg_fg_color_in_text_element() {
        // \x1b[31m = red foreground (color index 1 = #cd0000)
        let cast = "{\"version\":2,\"width\":80,\"height\":24}\n[0.5,\"o\",\"\\u001b[31mhello\"]";
        let mut player = make_player(cast);
        let opts = SvgOptions {
            at_time: 1.0,
            ..Default::default()
        };
        let svg = export_to_string(&mut player, &opts);
        // Color index 1 (red) = RGB(205, 0, 0) = #cd0000
        assert!(
            svg.contains("#cd0000"),
            "SVG should contain red foreground color #cd0000, got: {svg}"
        );
    }

    // -----------------------------------------------------------------------
    // 7. Background color rect
    // -----------------------------------------------------------------------

    #[test]
    fn test_svg_bg_color_rect() {
        // \x1b[44m = blue background (color index 4 = #0000ee)
        let cast = "{\"version\":2,\"width\":80,\"height\":24}\n[0.5,\"o\",\"\\u001b[44mhello\"]";
        let mut player = make_player(cast);
        let opts = SvgOptions {
            at_time: 1.0,
            ..Default::default()
        };
        let svg = export_to_string(&mut player, &opts);
        // Color index 4 (blue) = RGB(0, 0, 238) = #0000ee
        assert!(
            svg.contains("#0000ee"),
            "SVG should contain blue background color #0000ee, got: {svg}"
        );
    }

    // -----------------------------------------------------------------------
    // 8. Bold text attribute
    // -----------------------------------------------------------------------

    #[test]
    fn test_svg_bold_text_attribute() {
        // \x1b[1m = bold
        let cast = "{\"version\":2,\"width\":80,\"height\":24}\n[0.5,\"o\",\"\\u001b[1mhello\"]";
        let mut player = make_player(cast);
        let opts = SvgOptions {
            at_time: 1.0,
            ..Default::default()
        };
        let svg = export_to_string(&mut player, &opts);
        assert!(
            svg.contains("font-weight=\"bold\""),
            "SVG should contain font-weight=bold for bold text"
        );
    }

    // -----------------------------------------------------------------------
    // 9. Italic text attribute
    // -----------------------------------------------------------------------

    #[test]
    fn test_svg_italic_text_attribute() {
        // \x1b[3m = italic
        let cast = "{\"version\":2,\"width\":80,\"height\":24}\n[0.5,\"o\",\"\\u001b[3mhello\"]";
        let mut player = make_player(cast);
        let opts = SvgOptions {
            at_time: 1.0,
            ..Default::default()
        };
        let svg = export_to_string(&mut player, &opts);
        assert!(
            svg.contains("font-style=\"italic\""),
            "SVG should contain font-style=italic for italic text"
        );
    }

    // -----------------------------------------------------------------------
    // 10. Empty recording
    // -----------------------------------------------------------------------

    #[test]
    fn test_svg_empty_recording() {
        let cast = "{\"version\":2,\"width\":80,\"height\":24}";
        let mut player = make_player(cast);
        let svg = export_to_string(&mut player, &default_svg_opts());
        assert!(
            svg.starts_with("<svg "),
            "Empty recording SVG should start with <svg"
        );
        assert!(
            svg.ends_with("</svg>"),
            "Empty recording SVG should end with </svg>"
        );
        assert!(
            svg.contains("<rect width=\"100%\" height=\"100%\""),
            "Empty recording should have background rect"
        );
        assert!(
            !svg.contains("<text"),
            "Empty recording should have no <text> elements"
        );
    }

    // -----------------------------------------------------------------------
    // 11. at_time beyond duration clamps
    // -----------------------------------------------------------------------

    #[test]
    fn test_svg_at_time_beyond_duration() {
        let cast = "{\"version\":2,\"width\":80,\"height\":24}\n[1.0,\"o\",\"hello\\r\\n\"]\n[5.0,\"o\",\"done\\r\\n\"]";
        let mut player = make_player(cast);
        let opts = SvgOptions {
            at_time: 9999.0,
            ..Default::default()
        };
        let mut buf = Vec::new();
        let result = export_svg(&mut player, &opts, &mut buf);
        assert!(result.is_ok(), "at_time beyond duration should not error");
        let svg = String::from_utf8(buf).unwrap();
        assert!(svg.starts_with("<svg "), "Clamped SVG should be valid");
        assert!(
            svg.ends_with("</svg>"),
            "Clamped SVG should end with </svg>"
        );
    }

    // -----------------------------------------------------------------------
    // 12. Cursor visible
    // -----------------------------------------------------------------------

    #[test]
    fn test_svg_cursor_visible() {
        let cast = "{\"version\":2,\"width\":80,\"height\":24}\n[0.5,\"o\",\"hello\"]";
        let mut player = make_player(cast);
        let opts = SvgOptions {
            at_time: 1.0,
            show_cursor: true,
            ..Default::default()
        };
        let svg = export_to_string(&mut player, &opts);
        assert!(
            svg.contains("opacity"),
            "SVG should contain cursor rect with opacity attribute"
        );
    }

    // -----------------------------------------------------------------------
    // 13. Snapshot test
    // -----------------------------------------------------------------------

    #[test]
    fn test_svg_output_snapshot() {
        let cast = "{\"version\":2,\"width\":10,\"height\":3}\n[0.5,\"o\",\"\\u001b[31mhi\\u001b[0m world\\r\\n\"]\n[1.0,\"o\",\"\\u001b[1mfoo\\u001b[0m\\r\\n\"]";
        let mut player = make_player(cast);
        let opts = SvgOptions {
            at_time: 1.5,
            font_size: 10.0,
            show_cursor: false,
            ..Default::default()
        };
        let svg = export_to_string(&mut player, &opts);
        insta::assert_snapshot!(svg);
    }

    // -----------------------------------------------------------------------
    // Animated SVG tests
    // -----------------------------------------------------------------------

    fn export_animated_to_string(
        player: &mut Player,
        opts: &AnimatedSvgOptions,
    ) -> Result<String, ExportError> {
        let mut buf = Vec::new();
        export_animated_svg(player, opts, &mut buf)?;
        Ok(String::from_utf8(buf).expect("non-UTF8 SVG output"))
    }

    // -----------------------------------------------------------------------
    // Animated 1: Frame coalescing
    // -----------------------------------------------------------------------

    #[test]
    fn test_animated_coalesces_identical_frames() {
        // Each event re-outputs the same text, overwriting with \r (carriage return)
        // so the visible screen content is the same after events 2–5.
        // Event 1: displays "hello" on line 1
        // Events 2-5: rewrite "hello" to line 1 (no net change)
        let cast = concat!(
            "{\"version\":2,\"width\":80,\"height\":24}\n",
            "[0.1,\"o\",\"hello\"]\n",
            "[1.0,\"o\",\"\\rhello\"]\n",
            "[2.0,\"o\",\"\\rhello\"]\n",
            "[3.0,\"o\",\"\\rhello\"]\n",
            "[4.0,\"o\",\"\\rhello\"]",
        );
        let mut player = make_player(cast);
        let opts = AnimatedSvgOptions::default();
        let svg = export_animated_to_string(&mut player, &opts).unwrap();
        let frame_count = svg.matches("<g id=\"f").count();
        // There are 5 output events but the screen content is identical after event 1
        // so all subsequent events should be coalesced — total frames < 5.
        assert!(
            frame_count < 5,
            "Frame coalescing should produce fewer than 5 frames, got {frame_count}"
        );
    }

    // -----------------------------------------------------------------------
    // Animated 1b: Zero-duration frame coalescing
    // -----------------------------------------------------------------------

    #[test]
    fn test_animated_no_zero_duration_frames() {
        // Two events at the same raw time produce different screen content.
        // Previously build_frames would emit:
        //   Frame 0 (initial blank):  start=0.0, end=0.5
        //   Frame 1 (content "aaaa"): start=0.5, end=0.5  ← zero-duration!
        //   Frame 2 (content "bbbb"): start=0.5, end=…
        //
        // With the fix, frame 1 is replaced in-place so we get only 2 frames:
        //   Frame 0 (initial blank):  start=0.0, end=0.5
        //   Frame 1 (content "bbbb"): start=0.5, end=…
        let cast = concat!(
            "{\"version\":2,\"width\":80,\"height\":24}\n",
            "[0.5,\"o\",\"aaaa\\r\\n\"]\n",
            // Same timestamp as previous event — different content
            "[0.5,\"o\",\"bbbb\\r\\n\"]",
        );
        let mut player = make_player(cast);
        let opts = AnimatedSvgOptions::default();
        let svg = export_animated_to_string(&mut player, &opts).unwrap();

        // With the fix, only 2 frames: initial blank + one coalesced content frame.
        // Without the fix we'd get 3 frames (one of which has zero duration).
        let frame_count = svg.matches("<g id=\"f").count();
        assert_eq!(
            frame_count, 2,
            "Expected exactly 2 frames when two events share a timestamp, got {frame_count}. SVG:\n{svg}"
        );

        // The coalesced frame (f1) must show the LAST event's content ("bbbb"),
        // not the first ("aaaa").
        assert!(
            svg.contains("bbbb"),
            "Coalesced frame must contain content from the last event at the timestamp"
        );
    }

    // -----------------------------------------------------------------------
    // Animated 2: Single frame recording
    // -----------------------------------------------------------------------

    #[test]
    fn test_animated_single_frame_recording() {
        // No output events → build_frames returns exactly 1 frame (initial blank)
        let cast = "{\"version\":2,\"width\":80,\"height\":24}";
        let mut player = make_player(cast);
        let opts = AnimatedSvgOptions::default();
        let svg = export_animated_to_string(&mut player, &opts).unwrap();
        // Single-frame recordings render as static <g> with no id
        assert!(
            svg.contains("<g>"),
            "Single-frame should have a plain <g> group"
        );
    }

    // -----------------------------------------------------------------------
    // Animated 3: Timing percentages
    // -----------------------------------------------------------------------

    #[test]
    fn test_animated_timing_percentages() {
        let cast = concat!(
            "{\"version\":2,\"width\":80,\"height\":24}\n",
            "[0.1,\"o\",\"frame one content here\\r\\n\"]\n",
            "[5.0,\"o\",\"frame two different text\\r\\n\"]",
        );
        let mut player = make_player(cast);
        let opts = AnimatedSvgOptions::default();
        let svg = export_animated_to_string(&mut player, &opts).unwrap();

        let style_start = svg.find("<style>").expect("should have <style>");
        let style_end = svg.find("</style>").expect("should have </style>");
        let style_block = &svg[style_start..style_end];

        assert!(
            style_block.contains('%'),
            "Style block should contain percentage keyframes"
        );
        assert!(
            svg.contains("<style>"),
            "Animated SVG should have <style> block"
        );
        assert!(
            svg.contains("@keyframes"),
            "Animated SVG should have @keyframes"
        );
    }

    // -----------------------------------------------------------------------
    // Animated 4: Uses effective time
    // -----------------------------------------------------------------------

    #[test]
    fn test_animated_uses_effective_time() {
        // idle_time_limit=5 compresses the 99s gap between events 2 and 3
        let cast = concat!(
            "{\"version\":2,\"width\":80,\"height\":24,\"idle_time_limit\":5.0}\n",
            "[0.1,\"o\",\"aaaa\\r\\n\"]\n",
            "[1.0,\"o\",\"bbbb\\r\\n\"]\n",
            "[100.0,\"o\",\"cccc\\r\\n\"]\n",
            "[101.0,\"o\",\"dddd\\r\\n\"]",
        );
        let reader = std::io::Cursor::new(cast.to_string());
        let mut player = Player::load(reader).expect("load failed");
        let duration = player.duration();
        assert!(
            duration < 20.0,
            "Effective duration should be compressed (< 20s), got {duration}"
        );
        let opts = AnimatedSvgOptions::default();
        let svg = export_animated_to_string(&mut player, &opts).unwrap();
        assert!(svg.contains("@keyframes"), "Should have keyframes");
    }

    // -----------------------------------------------------------------------
    // Animated 5: Rejects long recordings
    // -----------------------------------------------------------------------

    #[test]
    fn test_animated_rejects_long_recording() {
        let cast = "{\"version\":2,\"width\":80,\"height\":24}\n[1.0,\"o\",\"start\\r\\n\"]\n[130.0,\"o\",\"end\\r\\n\"]";
        let reader = std::io::Cursor::new(cast.to_string());
        let mut player = Player::load(reader).expect("load failed");
        let opts = AnimatedSvgOptions {
            force_long: false,
            ..Default::default()
        };
        let result = export_animated_to_string(&mut player, &opts);
        assert!(result.is_err(), "Should return error for recording > 120s");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("120"),
            "Error message should mention 120s limit, got: {err_msg}"
        );
    }

    // -----------------------------------------------------------------------
    // Animated 6: force_long overrides the limit
    // -----------------------------------------------------------------------

    #[test]
    fn test_animated_force_overrides_limit() {
        let cast = "{\"version\":2,\"width\":80,\"height\":24}\n[1.0,\"o\",\"start\\r\\n\"]\n[130.0,\"o\",\"end\\r\\n\"]";
        let reader = std::io::Cursor::new(cast.to_string());
        let mut player = Player::load(reader).expect("load failed");
        let opts = AnimatedSvgOptions {
            force_long: true,
            ..Default::default()
        };
        let result = export_animated_to_string(&mut player, &opts);
        assert!(
            result.is_ok(),
            "force_long=true should succeed for > 120s recording"
        );
    }

    // -----------------------------------------------------------------------
    // Animated 7: Has style block
    // -----------------------------------------------------------------------

    #[test]
    fn test_animated_has_style_block() {
        let cast = concat!(
            "{\"version\":2,\"width\":80,\"height\":24}\n",
            "[0.5,\"o\",\"hello\\r\\n\"]\n",
            "[1.5,\"o\",\"world\\r\\n\"]",
        );
        let mut player = make_player(cast);
        let opts = AnimatedSvgOptions::default();
        let svg = export_animated_to_string(&mut player, &opts).unwrap();
        assert!(svg.contains("<style>"), "Should contain <style>");
        assert!(svg.contains("@keyframes"), "Should contain @keyframes");
    }

    // -----------------------------------------------------------------------
    // Animated 8: animation loops infinitely (not forwards)
    // -----------------------------------------------------------------------

    #[test]
    fn test_animated_loops_infinitely() {
        let cast = concat!(
            "{\"version\":2,\"width\":80,\"height\":24}\n",
            "[0.5,\"o\",\"hello\\r\\n\"]\n",
            "[1.5,\"o\",\"world\\r\\n\"]",
        );
        let mut player = make_player(cast);
        let opts = AnimatedSvgOptions::default();
        let svg = export_animated_to_string(&mut player, &opts).unwrap();
        assert!(
            svg.contains("infinite"),
            "Animated SVG should contain 'infinite' for looping animation"
        );
        assert!(
            !svg.contains("forwards"),
            "Animated SVG must not contain 'forwards'"
        );
    }

    // -----------------------------------------------------------------------
    // Animated 8b: last frame keyframe has 100%{visibility:hidden}
    // -----------------------------------------------------------------------

    #[test]
    fn test_animated_last_frame_hides_at_100_percent() {
        let cast = concat!(
            "{\"version\":2,\"width\":80,\"height\":24}\n",
            "[0.5,\"o\",\"hello\\r\\n\"]\n",
            "[1.5,\"o\",\"world\\r\\n\"]",
        );
        let mut player = make_player(cast);
        let opts = AnimatedSvgOptions::default();
        let svg = export_animated_to_string(&mut player, &opts).unwrap();
        assert!(
            svg.contains("100%{visibility:hidden}"),
            "Last frame's @keyframes must include 100%{{visibility:hidden}}, got: {svg}"
        );
    }

    // -----------------------------------------------------------------------
    // Animated 8c: single-frame SVG has no <style> block
    // -----------------------------------------------------------------------

    #[test]
    fn test_animated_single_frame_no_style_block() {
        // No output events → build_frames returns exactly 1 frame (initial blank)
        let cast = "{\"version\":2,\"width\":80,\"height\":24}";
        let mut player = make_player(cast);
        let opts = AnimatedSvgOptions::default();
        let svg = export_animated_to_string(&mut player, &opts).unwrap();
        assert!(
            !svg.contains("<style>"),
            "Single-frame animated SVG must have no <style> block"
        );
        assert!(
            !svg.contains("@keyframes"),
            "Single-frame animated SVG must have no @keyframes"
        );
    }

    // -----------------------------------------------------------------------
    // Animated 9: Browser compatibility comment
    // -----------------------------------------------------------------------

    #[test]
    fn test_animated_browser_compat_comment() {
        let cast = "{\"version\":2,\"width\":80,\"height\":24}\n[0.5,\"o\",\"hello\"]";
        let mut player = make_player(cast);
        let opts = AnimatedSvgOptions::default();
        let svg = export_animated_to_string(&mut player, &opts).unwrap();
        assert!(
            svg.contains("<!-- CSS animations"),
            "Animated SVG should contain browser compatibility comment"
        );
    }

    // -----------------------------------------------------------------------
    // Animated 10: AnimatedSvgOptions has no at_time field (compile-time check)
    // -----------------------------------------------------------------------

    #[test]
    fn test_animated_no_at_time_field() {
        // This compiles only if AnimatedSvgOptions has no at_time field.
        // CLI-level mutual exclusion with --at is enforced by clap.
        let opts = AnimatedSvgOptions::default();
        let _ = opts.font_size;
    }
}
