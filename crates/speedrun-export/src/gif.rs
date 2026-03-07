//! GIF encoding and export for terminal recordings.
//!
//! This module will be implemented in ticket 7nx.6 (GIF encoding and CLI).
//! It depends on ticket 7nx.5 (font infrastructure and pixel renderer).

use speedrun_core::Player;

use crate::palette::ExportOptions;

/// Errors that can occur during GIF export.
#[derive(Debug)]
pub enum GifError {
    /// An I/O error occurred while writing the output.
    Io(std::io::Error),
    /// The requested FPS exceeds the maximum (50).
    FpsTooHigh(u32),
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
        }
    }
}

impl std::error::Error for GifError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            GifError::Io(e) => Some(e),
            GifError::FpsTooHigh(_) => None,
        }
    }
}

impl From<std::io::Error> for GifError {
    fn from(e: std::io::Error) -> Self {
        GifError::Io(e)
    }
}

/// Options for GIF export.
pub struct GifOptions {
    /// Frames per second (default: 10, max: 50).
    pub fps: u32,
    /// Scale factor (default: 1).
    pub scale: u32,
    /// Loop count (default: 0 = infinite).
    pub loop_count: u16,
    /// Color palette configuration.
    pub export: ExportOptions,
}

impl Default for GifOptions {
    fn default() -> Self {
        Self {
            fps: 10,
            scale: 1,
            loop_count: 0,
            export: ExportOptions::default(),
        }
    }
}

/// Export a recording as an animated GIF.
///
/// # Arguments
///
/// * `player` — Mutable reference to the player (will be seeked).
/// * `options` — GIF export options.
/// * `writer` — Output writer.
/// * `progress` — Optional progress callback `(current_frame, total_frames)`.
///
/// # Errors
///
/// Returns [`GifError::FpsTooHigh`] if `options.fps > 50`.
pub fn export_gif(
    _player: &mut Player,
    options: &GifOptions,
    _writer: impl std::io::Write,
    _progress: Option<&dyn Fn(usize, usize)>,
) -> Result<(), GifError> {
    if options.fps > 50 {
        return Err(GifError::FpsTooHigh(options.fps));
    }
    // TODO: Implement in ticket 7nx.6
    Err(GifError::Io(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "GIF export not yet implemented (ticket 7nx.6)",
    )))
}
