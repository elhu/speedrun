//! Animated GIF export for terminal recordings.
//!
//! Each frame of the recording is rasterized by [`ScreenRenderer`] and
//! encoded as a GIF89a frame. Frame diffing is applied to reduce file size:
//! only the first frame of each run writes the full frame; subsequent frames
//! compare against the previous and use transparency for unchanged regions.
//!
//! # GIF timing resolution
//!
//! GIF frame delay is stored in units of 10 ms (centiseconds). The maximum
//! meaningful frame rate is therefore 100 FPS (1 cs delay), but the spec
//! commonly limits to 50 FPS in practice. This implementation enforces a
//! maximum of 50 FPS.
//!
//! # Memory management
//!
//! Only two pixel buffers (current + previous) are kept in memory at once.
//! Encoded frames are streamed directly to the writer.

use std::io::Write;

use image::RgbaImage;
use speedrun_core::Player;

use crate::palette::ExportOptions;
use crate::renderer::{FontError, ScreenRenderer};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during GIF export.
#[derive(Debug)]
pub enum GifError {
    /// An I/O error occurred while writing the output.
    Io(std::io::Error),
    /// The requested FPS exceeds the maximum (50).
    FpsTooHigh(u32),
    /// An error occurred during GIF encoding.
    Encoding(gif::EncodingError),
    /// The provided font could not be parsed as a valid TTF/OTF font.
    Font(FontError),
}

impl std::fmt::Display for GifError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GifError::Io(e) => write!(f, "I/O error: {e}"),
            GifError::FpsTooHigh(fps) => write!(
                f,
                "Maximum FPS for GIF is 50 (due to GIF timing resolution of 10ms). \
                 Use --fps 50 or lower (got {fps})."
            ),
            GifError::Encoding(e) => write!(f, "GIF encoding error: {e}"),
            GifError::Font(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for GifError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            GifError::Io(e) => Some(e),
            GifError::FpsTooHigh(_) => None,
            GifError::Encoding(e) => Some(e),
            GifError::Font(e) => Some(e),
        }
    }
}

impl From<FontError> for GifError {
    fn from(e: FontError) -> Self {
        GifError::Font(e)
    }
}

impl From<std::io::Error> for GifError {
    fn from(e: std::io::Error) -> Self {
        GifError::Io(e)
    }
}

impl From<gif::EncodingError> for GifError {
    fn from(e: gif::EncodingError) -> Self {
        GifError::Encoding(e)
    }
}

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

/// Options for GIF export.
pub struct GifOptions {
    /// Frames per second (default: 10, max: 50).
    pub fps: u32,
    /// Scale factor (default: 1).
    pub scale: u32,
    /// Loop count (0 = infinite).
    pub loop_count: u16,
    /// Color palette configuration.
    pub export: ExportOptions,
    /// Custom font bytes (TTF/OTF). When `None`, the embedded JetBrains Mono
    /// is used. Bold text uses the embedded JetBrains Mono Bold regardless.
    /// The font must be monospace; non-monospace fonts will produce misaligned
    /// output.
    pub font_data: Option<Vec<u8>>,
}

impl Default for GifOptions {
    fn default() -> Self {
        Self {
            fps: 10,
            scale: 1,
            loop_count: 0,
            export: ExportOptions::default(),
            font_data: None,
        }
    }
}

// ---------------------------------------------------------------------------
// GIF export
// ---------------------------------------------------------------------------

/// Export a recording as an animated GIF.
///
/// Frames are rendered at `options.fps` frames per second using
/// [`ScreenRenderer`]. Each frame is quantized to 256 colors and encoded
/// with frame diffing (transparent pixels for unchanged regions).
///
/// # Arguments
///
/// * `player` — Mutable reference to the player (will be seeked during export).
/// * `options` — GIF export options.
/// * `writer` — Output writer. Frames are streamed; no full-file buffering.
/// * `progress` — Optional callback `(current_frame, total_frames)`.
///
/// # Errors
///
/// Returns [`GifError::FpsTooHigh`] if `options.fps > 50`.
pub fn export_gif(
    player: &mut Player,
    options: &GifOptions,
    mut writer: impl Write,
    progress: Option<&dyn Fn(usize, usize)>,
) -> Result<(), GifError> {
    if options.fps > 50 {
        return Err(GifError::FpsTooHigh(options.fps));
    }

    let fps = options.fps.max(1);
    let duration = player.duration();

    // Warn for very long recordings
    if duration > 300.0 {
        eprintln!(
            "Warning: GIF export for a {duration:.0}s recording at {fps} FPS may produce a very large file."
        );
    }

    let (w, h) = player.size();
    let renderer = ScreenRenderer::new(
        ExportOptions {
            default_fg: options.export.default_fg,
            default_bg: options.export.default_bg,
            bold_brightens: options.export.bold_brightens,
        },
        options.scale,
        options.font_data.as_deref(),
    )?;

    let cell_w = renderer.cell_width;
    let cell_h = renderer.cell_height;
    let img_w = w as u32 * cell_w;
    let img_h = h as u32 * cell_h;

    // For an empty recording (0 duration), render a single frame
    let total_frames = if duration <= 0.0 {
        1usize
    } else {
        (duration * fps as f64).ceil() as usize
    };

    // GIF frame delay in centiseconds (1/100 s)
    let frame_delay_cs = (100u32 / fps) as u16;

    // Encode GIF header (no global palette — each frame has its own)
    let mut encoder = gif::Encoder::new(&mut writer, img_w as u16, img_h as u16, &[])?;

    // Loop count
    let repeat = match options.loop_count {
        0 => gif::Repeat::Infinite,
        n => gif::Repeat::Finite(n),
    };
    encoder.set_repeat(repeat)?;

    let mut prev_frame: Option<RgbaImage> = None;

    for frame_idx in 0..total_frames {
        let time = if duration <= 0.0 {
            0.0
        } else {
            frame_idx as f64 / fps as f64
        };

        player.seek(time);
        let current = renderer.render_frame(player.screen(), &player.cursor(), w, h);

        // Apply frame diffing: transparent color index = 0 in a palette of
        // [transparent_marker, ...actual_colors...]
        let mut gif_frame = encode_frame_with_diffing(&current, prev_frame.as_ref(), img_w, img_h);
        gif_frame.delay = frame_delay_cs;

        encoder.write_frame(&gif_frame)?;

        if let Some(cb) = progress {
            cb(frame_idx + 1, total_frames);
        }

        prev_frame = Some(current);
    }

    Ok(())
}

/// Encode a frame as a GIF `Frame`, applying simple frame diffing.
///
/// For pixels that are identical to the previous frame, the transparent color
/// index is used. This significantly reduces file size for recordings where
/// most of the screen is static.
///
/// The frame uses a local palette with transparent index 0. Non-transparent
/// pixels are quantized using the gif crate's built-in NeuQuant algorithm.
fn encode_frame_with_diffing(
    current: &RgbaImage,
    prev: Option<&RgbaImage>,
    width: u32,
    height: u32,
) -> gif::Frame<'static> {
    if prev.is_none() {
        // First frame: encode the full image as-is
        let mut rgba_data: Vec<u8> = current.pixels().flat_map(|p| p.0).collect();
        let mut frame =
            gif::Frame::from_rgba_speed(width as u16, height as u16, &mut rgba_data, 10);
        frame.dispose = gif::DisposalMethod::Keep;
        return frame;
    }

    let prev = prev.unwrap();

    // Build a pixel list where changed pixels keep their RGBA value and
    // unchanged pixels are made fully transparent (0,0,0,0).
    let mut diffed_rgba: Vec<u8> = Vec::with_capacity((width * height * 4) as usize);
    let mut has_diff = false;

    for (cur_px, prev_px) in current.pixels().zip(prev.pixels()) {
        if cur_px == prev_px {
            // Mark as transparent
            diffed_rgba.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            diffed_rgba.extend_from_slice(&[cur_px[0], cur_px[1], cur_px[2], 255]);
            has_diff = true;
        }
    }

    if !has_diff {
        // Identical frame — encode a minimal transparent frame
        let transparent_idx = 0u8;
        let pixels = vec![transparent_idx; (width * height) as usize];
        // Minimal palette: [black, ...]
        let palette = vec![0u8; 3];
        let mut frame = gif::Frame::from_palette_pixels(
            width as u16,
            height as u16,
            pixels,
            palette,
            Some(transparent_idx),
        );
        frame.dispose = gif::DisposalMethod::Keep;
        return frame;
    }

    let mut frame = gif::Frame::from_rgba_speed(width as u16, height as u16, &mut diffed_rgba, 10);
    frame.dispose = gif::DisposalMethod::Keep;

    // Find the transparent color index (the color that corresponds to alpha=0)
    // from_rgba_speed will have mapped transparent pixels to some palette entry
    // We need to tell the GIF encoder which index is transparent.
    // The gif crate sets transparent automatically when alpha < 128.
    // frame.transparent is already set by from_rgba_speed.

    frame
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use speedrun_core::Player;

    fn make_player(cast: &str) -> Player {
        let reader = std::io::Cursor::new(cast.to_string());
        Player::load(reader).expect("failed to load test recording")
    }

    fn export_to_vec(player: &mut Player, opts: &GifOptions) -> Result<Vec<u8>, GifError> {
        let mut buf = Vec::new();
        export_gif(player, opts, &mut buf, None)?;
        Ok(buf)
    }

    /// Count GIF image descriptor (frame) separators in output.
    fn count_gif_frames(data: &[u8]) -> usize {
        // GIF frames start with Image Separator byte 0x2C
        data.iter().filter(|&&b| b == 0x2C).count()
    }

    /// Parse GIF Logical Screen Descriptor dimensions.
    fn gif_dimensions(data: &[u8]) -> (u16, u16) {
        // Bytes 6-7: width (LE), bytes 8-9: height (LE)
        let width = u16::from_le_bytes([data[6], data[7]]);
        let height = u16::from_le_bytes([data[8], data[9]]);
        (width, height)
    }

    // -----------------------------------------------------------------------
    // 1. GIF magic bytes
    // -----------------------------------------------------------------------

    #[test]
    fn test_gif_magic_bytes() {
        let cast = "{\"version\":2,\"width\":10,\"height\":3}\n[0.5,\"o\",\"hello\"]";
        let mut player = make_player(cast);
        let opts = GifOptions {
            fps: 10,
            ..Default::default()
        };
        let data = export_to_vec(&mut player, &opts).unwrap();
        assert_eq!(
            &data[..6],
            b"GIF89a",
            "Output should start with GIF89a magic"
        );
    }

    // -----------------------------------------------------------------------
    // 2. Non-empty output
    // -----------------------------------------------------------------------

    #[test]
    fn test_gif_non_empty_output() {
        let cast = "{\"version\":2,\"width\":10,\"height\":3}\n[0.5,\"o\",\"hello\"]";
        let mut player = make_player(cast);
        let opts = GifOptions::default();
        let data = export_to_vec(&mut player, &opts).unwrap();
        assert!(
            data.len() > 100,
            "GIF output should be > 100 bytes, got {}",
            data.len()
        );
    }

    // -----------------------------------------------------------------------
    // 3. Frame count matches FPS × duration
    // -----------------------------------------------------------------------

    #[test]
    fn test_gif_frame_count_matches_fps() {
        // 3 seconds of recording at 10 FPS → ~30 frames
        let cast = concat!(
            "{\"version\":2,\"width\":10,\"height\":3}\n",
            "[0.5,\"o\",\"hello\"]\n",
            "[3.0,\"o\",\"world\"]",
        );
        let mut player = make_player(cast);
        let opts = GifOptions {
            fps: 10,
            ..Default::default()
        };
        let data = export_to_vec(&mut player, &opts).unwrap();
        let frame_count = count_gif_frames(&data);
        // Expected: ceil(3.0 * 10) = 30, allow ±1 for rounding
        assert!(
            (29..=31).contains(&frame_count),
            "Expected ~30 frames, got {frame_count}"
        );
    }

    // -----------------------------------------------------------------------
    // 4. Single frame for empty recording
    // -----------------------------------------------------------------------

    #[test]
    fn test_gif_single_frame_empty_recording() {
        let cast = "{\"version\":2,\"width\":10,\"height\":3}";
        let mut player = make_player(cast);
        let opts = GifOptions::default();
        let data = export_to_vec(&mut player, &opts).unwrap();
        let frame_count = count_gif_frames(&data);
        // Empty recording (0 duration) should produce exactly 1 frame
        assert_eq!(frame_count, 1, "Empty recording should produce 1 frame");
    }

    // -----------------------------------------------------------------------
    // 5. Rejects FPS > 50
    // -----------------------------------------------------------------------

    #[test]
    fn test_gif_rejects_fps_over_50() {
        let cast = "{\"version\":2,\"width\":10,\"height\":3}\n[0.5,\"o\",\"hello\"]";
        let mut player = make_player(cast);
        let opts = GifOptions {
            fps: 60,
            ..Default::default()
        };
        let result = export_to_vec(&mut player, &opts);
        assert!(result.is_err(), "fps=60 should return an error");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("50"),
            "Error message should mention the 50 FPS limit, got: {err}"
        );
    }

    // -----------------------------------------------------------------------
    // 6. Accepts FPS = 50
    // -----------------------------------------------------------------------

    #[test]
    fn test_gif_accepts_fps_50() {
        let cast = "{\"version\":2,\"width\":10,\"height\":3}\n[0.5,\"o\",\"hello\"]";
        let mut player = make_player(cast);
        let opts = GifOptions {
            fps: 50,
            ..Default::default()
        };
        let result = export_to_vec(&mut player, &opts);
        assert!(
            result.is_ok(),
            "fps=50 should succeed (maximum valid value)"
        );
    }

    // -----------------------------------------------------------------------
    // 7. Frame diffing reduces size
    // -----------------------------------------------------------------------

    #[test]
    fn test_gif_frame_diffing_reduces_size() {
        // Static recording (same content every frame) — diffing should significantly
        // reduce size vs a naive upper bound
        let cast = concat!(
            "{\"version\":2,\"width\":10,\"height\":3}\n",
            "[0.5,\"o\",\"hello\"]\n",
            "[2.0,\"o\",\"hello\\r\"]", // cursor moves but same text
        );
        let mut player = make_player(cast);
        let opts = GifOptions {
            fps: 10,
            scale: 1,
            ..Default::default()
        };
        let data = export_to_vec(&mut player, &opts).unwrap();

        // Naive upper bound: frames * (10*8) * (3*16) * 3 bytes
        let renderer = ScreenRenderer::new(ExportOptions::default(), 1, None)
            .expect("embedded font should parse");
        let frame_count = count_gif_frames(&data);
        let naive_upper =
            frame_count * (10 * renderer.cell_width * 3 * renderer.cell_height * 3) as usize;

        assert!(
            data.len() < naive_upper,
            "GIF with diffing should be smaller than naive upper bound ({} < {naive_upper})",
            data.len()
        );
    }

    // -----------------------------------------------------------------------
    // 8. Scale doubles dimensions
    // -----------------------------------------------------------------------

    #[test]
    fn test_gif_scale_doubles_dimensions() {
        let cast = "{\"version\":2,\"width\":10,\"height\":3}\n[0.5,\"o\",\"hello\"]";

        let mut player1 = make_player(cast);
        let opts1 = GifOptions {
            fps: 1,
            scale: 1,
            ..Default::default()
        };
        let data1 = export_to_vec(&mut player1, &opts1).unwrap();

        let mut player2 = make_player(cast);
        let opts2 = GifOptions {
            fps: 1,
            scale: 2,
            ..Default::default()
        };
        let data2 = export_to_vec(&mut player2, &opts2).unwrap();

        let (w1, h1) = gif_dimensions(&data1);
        let (w2, h2) = gif_dimensions(&data2);

        assert_eq!(w2, w1 * 2, "scale=2 width should be 2x scale=1 width");
        assert_eq!(h2, h1 * 2, "scale=2 height should be 2x scale=1 height");
    }

    // -----------------------------------------------------------------------
    // 9. Progress callback is called
    // -----------------------------------------------------------------------

    #[test]
    fn test_gif_progress_callback_called() {
        let cast = concat!(
            "{\"version\":2,\"width\":10,\"height\":3}\n",
            "[0.5,\"o\",\"hello\"]\n",
            "[1.0,\"o\",\"world\"]",
        );
        let mut player = make_player(cast);
        let opts = GifOptions {
            fps: 10,
            ..Default::default()
        };

        let calls: RefCell<Vec<(usize, usize)>> = RefCell::new(Vec::new());
        let callback = |current, total| {
            calls.borrow_mut().push((current, total));
        };

        let mut buf = Vec::new();
        export_gif(&mut player, &opts, &mut buf, Some(&callback)).unwrap();

        let recorded = calls.borrow();
        assert!(
            !recorded.is_empty(),
            "Progress callback should have been called"
        );

        // Total should be consistent across all calls
        let total = recorded[0].1;
        assert!(
            recorded.iter().all(|(_, t)| *t == total),
            "Total should be consistent across progress calls"
        );

        // Last call's current should equal total
        let last = recorded.last().unwrap();
        assert_eq!(
            last.0, last.1,
            "Last progress call should have current == total"
        );
    }

    // -----------------------------------------------------------------------
    // 10. Export returns Ok, not panic
    // -----------------------------------------------------------------------

    #[test]
    fn test_gif_export_returns_result_not_panic() {
        let cast = "{\"version\":2,\"width\":10,\"height\":3}\n[0.5,\"o\",\"hello\"]";
        let mut player = make_player(cast);
        let opts = GifOptions::default();
        let result = export_to_vec(&mut player, &opts);
        assert!(
            result.is_ok(),
            "export_gif should return Ok for a valid recording"
        );
    }
}
