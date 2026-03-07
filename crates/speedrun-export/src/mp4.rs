//! MP4 video export for terminal recordings.
//!
//! Frames are rasterized by [`ScreenRenderer`] to raw RGBA pixel buffers and
//! piped to an `ffmpeg` subprocess for H.264/MP4 encoding.
//!
//! # ffmpeg requirement
//!
//! `ffmpeg` must be on `PATH`. Returns [`Mp4Error::FfmpegNotFound`] if not
//! available. The `-vf pad` filter handles even-dimension enforcement required
//! by yuv420p, so no Rust-side padding is needed.
//!
//! # Deadlock prevention
//!
//! ffmpeg's stderr is drained in a dedicated thread concurrently with frame
//! writes to stdin. Without this, filling ffmpeg's 64 KB stderr pipe buffer
//! while still writing frames to stdin would cause both processes to deadlock.

use std::io::{BufReader, Read, Write};
use std::path::Path;

use speedrun_core::Player;

use crate::palette::ExportOptions;
use crate::renderer::ScreenRenderer;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during MP4 export.
#[derive(Debug)]
pub enum Mp4Error {
    /// An I/O error occurred (pipe, file, etc.).
    Io(std::io::Error),
    /// `ffmpeg` was not found on PATH.
    FfmpegNotFound,
    /// `ffmpeg` exited with a non-zero status.
    FfmpegFailed {
        /// The exit code, if available.
        exit_code: Option<i32>,
        /// Captured stderr output from ffmpeg.
        stderr: String,
    },
}

impl std::fmt::Display for Mp4Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Mp4Error::Io(e) => write!(f, "I/O error: {e}"),
            Mp4Error::FfmpegNotFound => write!(
                f,
                "ffmpeg not found on PATH. Please install ffmpeg to export MP4 files."
            ),
            Mp4Error::FfmpegFailed { exit_code, stderr } => {
                let code_str = exit_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                write!(
                    f,
                    "ffmpeg failed with exit code {code_str}. stderr:\n{stderr}"
                )
            }
        }
    }
}

impl std::error::Error for Mp4Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Mp4Error::Io(e) => Some(e),
            Mp4Error::FfmpegNotFound => None,
            Mp4Error::FfmpegFailed { .. } => None,
        }
    }
}

impl From<std::io::Error> for Mp4Error {
    fn from(e: std::io::Error) -> Self {
        Mp4Error::Io(e)
    }
}

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

/// Options for MP4 export.
pub struct Mp4Options {
    /// Frames per second (default: 30).
    pub fps: u32,
    /// Scale factor (default: 1).
    pub scale: u32,
    /// Constant Rate Factor, 0–51 (default: 23; lower = better quality).
    pub crf: u8,
    /// Color palette configuration.
    pub export: ExportOptions,
}

impl Default for Mp4Options {
    fn default() -> Self {
        Self {
            fps: 30,
            scale: 1,
            crf: 23,
            export: ExportOptions::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// ffmpeg helper
// ---------------------------------------------------------------------------

/// Check that `ffmpeg` is available on PATH.
///
/// Returns `Ok(())` if found, `Err(Mp4Error::FfmpegNotFound)` otherwise.
fn check_ffmpeg() -> Result<(), Mp4Error> {
    match std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
    {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(Mp4Error::FfmpegNotFound),
        Err(e) => Err(Mp4Error::Io(e)),
    }
}

// ---------------------------------------------------------------------------
// MP4 export
// ---------------------------------------------------------------------------

/// Export a recording as an MP4 video file via `ffmpeg`.
///
/// Frames are rasterized at `options.fps` frames per second, then piped as
/// raw RGBA bytes to `ffmpeg` which encodes them as H.264/yuv420p in an MP4
/// container. The output file is written directly by `ffmpeg`.
///
/// # Arguments
///
/// * `player` — Mutable reference to the player (will be seeked during export).
/// * `options` — MP4 export options.
/// * `output_path` — Path to the output `.mp4` file. The file is always
///   overwritten (the caller is responsible for --force checks).
/// * `progress` — Optional callback `(current_frame, total_frames)`.
///
/// # Errors
///
/// Returns [`Mp4Error::FfmpegNotFound`] if `ffmpeg` is not on PATH.
/// Returns [`Mp4Error::FfmpegFailed`] if `ffmpeg` exits with a non-zero status.
pub fn export_mp4(
    player: &mut Player,
    options: &Mp4Options,
    output_path: &Path,
    progress: Option<&dyn Fn(usize, usize)>,
) -> Result<(), Mp4Error> {
    // 1. Check ffmpeg availability upfront.
    check_ffmpeg()?;

    let fps = options.fps.max(1);
    let duration = player.duration();

    // Warn for very long recordings.
    if duration > 300.0 {
        eprintln!(
            "Warning: MP4 export for a {duration:.0}s recording at {fps} FPS may take a while."
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
    );

    // 2. Compute frame dimensions.
    let img_w = w as u32 * renderer.cell_width;
    let img_h = h as u32 * renderer.cell_height;

    // For an empty recording (0 duration), render a single frame.
    let total_frames = if duration <= 0.0 {
        1usize
    } else {
        (duration * fps as f64).ceil() as usize
    };

    // 3. Spawn ffmpeg with stdin/stderr piped.
    //    -y overwrites output; -an disables audio.
    //    -vf pad enforces even dimensions required by yuv420p.
    let mut child = std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-s",
            &format!("{img_w}x{img_h}"),
            "-r",
            &fps.to_string(),
            "-i",
            "pipe:0",
            "-vf",
            "pad=ceil(iw/2)*2:ceil(ih/2)*2",
            "-c:v",
            "libx264",
            "-preset",
            "medium",
            "-crf",
            &options.crf.to_string(),
            "-pix_fmt",
            "yuv420p",
            "-an",
        ])
        .arg(output_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    // 4. Drain stderr in a separate thread to prevent deadlock.
    let stderr = child.stderr.take().expect("stderr was piped");
    let stderr_thread = std::thread::spawn(move || -> String {
        let mut buf = String::new();
        BufReader::new(stderr).read_to_string(&mut buf).unwrap_or(0);
        buf
    });

    // 5. Write frames to ffmpeg's stdin.
    {
        let mut stdin = child.stdin.take().expect("stdin was piped");

        for frame_idx in 0..total_frames {
            let time = if duration <= 0.0 {
                0.0
            } else {
                frame_idx as f64 / fps as f64
            };

            player.seek(time);
            let img = renderer.render_frame(player.screen(), &player.cursor(), w, h);
            stdin.write_all(img.as_raw())?;

            if let Some(cb) = progress {
                cb(frame_idx + 1, total_frames);
            }
        }
        // stdin is dropped here, closing the pipe and signalling EOF to ffmpeg.
    }

    // 6. Join stderr thread and wait for ffmpeg to finish.
    let stderr_output = stderr_thread.join().unwrap_or_default();
    let status = child.wait()?;

    if !status.success() {
        return Err(Mp4Error::FfmpegFailed {
            exit_code: status.code(),
            stderr: stderr_output,
        });
    }

    Ok(())
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

    // -----------------------------------------------------------------------
    // Always-run tests (no ffmpeg required)
    // -----------------------------------------------------------------------

    #[test]
    fn test_mp4_options_default() {
        let opts = Mp4Options::default();
        assert_eq!(opts.fps, 30, "default fps should be 30");
        assert_eq!(opts.scale, 1, "default scale should be 1");
        assert_eq!(opts.crf, 23, "default crf should be 23");
    }

    #[test]
    fn test_mp4_error_display() {
        let not_found = Mp4Error::FfmpegNotFound;
        let msg = not_found.to_string();
        assert!(
            msg.contains("ffmpeg"),
            "FfmpegNotFound message should mention ffmpeg: {msg}"
        );

        let failed = Mp4Error::FfmpegFailed {
            exit_code: Some(1),
            stderr: "some error".to_string(),
        };
        let msg = failed.to_string();
        assert!(
            msg.contains("1"),
            "FfmpegFailed message should contain exit code: {msg}"
        );
        assert!(
            msg.contains("some error"),
            "FfmpegFailed message should contain stderr: {msg}"
        );
    }

    #[test]
    fn test_mp4_ffmpeg_not_found() {
        // Use a helper that checks ffmpeg with a custom PATH so we don't modify
        // the global environment. We replicate check_ffmpeg() logic inline with
        // an overridden PATH that contains no ffmpeg binary.
        let result = std::process::Command::new("ffmpeg")
            .arg("-version")
            .env("PATH", "/nonexistent")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();

        let err = match result {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Mp4Error::FfmpegNotFound,
            Err(e) => Mp4Error::Io(e),
            Ok(_) => {
                // If somehow ffmpeg exists at /nonexistent, skip the assertion.
                return;
            }
        };

        assert!(
            matches!(err, Mp4Error::FfmpegNotFound),
            "Expected FfmpegNotFound when ffmpeg is not on PATH"
        );
    }

    // -----------------------------------------------------------------------
    // ffmpeg-gated tests (require ffmpeg + ffprobe; skipped by default)
    // -----------------------------------------------------------------------

    #[test]
    #[ignore]
    fn test_mp4_export_produces_valid_file() {
        let cast = "{\"version\":2,\"width\":10,\"height\":3}\n[0.5,\"o\",\"hello\"]";
        let mut player = make_player(cast);
        let opts = Mp4Options::default();

        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let path = tmp.path().with_extension("mp4");

        export_mp4(&mut player, &opts, &path, None).expect("export_mp4 failed");

        assert!(path.exists(), "output file should exist");

        let data = std::fs::read(&path).expect("read output");
        // MP4 files start with an ftyp box: bytes 4..8 == b"ftyp"
        assert!(
            data.len() > 8,
            "output should be at least 8 bytes, got {}",
            data.len()
        );
        assert_eq!(
            &data[4..8],
            b"ftyp",
            "bytes 4..8 should be 'ftyp' MP4 box marker"
        );
    }

    #[test]
    #[ignore]
    fn test_mp4_frame_count() {
        // 2-second recording at 10 FPS → 20 frames
        let cast = concat!(
            "{\"version\":2,\"width\":10,\"height\":3}\n",
            "[0.5,\"o\",\"hello\"]\n",
            "[2.0,\"o\",\"world\"]",
        );
        let mut player = make_player(cast);
        let opts = Mp4Options {
            fps: 10,
            ..Default::default()
        };

        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let path = tmp.path().with_extension("mp4");

        export_mp4(&mut player, &opts, &path, None).expect("export_mp4 failed");

        // Use ffprobe to count frames.
        let output = std::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-count_frames",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=nb_read_frames",
                "-of",
                "default=nokey=1:noprint_wrappers=1",
            ])
            .arg(&path)
            .output()
            .expect("ffprobe failed");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let frame_count: usize = stdout.trim().parse().expect("parse frame count");

        // 2s * 10fps = 20, allow ±1 for rounding
        assert!(
            (19..=21).contains(&frame_count),
            "Expected ~20 frames, got {frame_count}"
        );
    }

    #[test]
    #[ignore]
    fn test_mp4_dimensions_are_even() {
        // 81 cols → img_w = 81 * 8 = 648 (even, but cell_height may vary)
        // Use a narrow terminal to exercise the pad filter
        let cast = "{\"version\":2,\"width\":81,\"height\":5}\n[0.5,\"o\",\"hi\"]";
        let mut player = make_player(cast);
        let opts = Mp4Options::default();

        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let path = tmp.path().with_extension("mp4");

        export_mp4(&mut player, &opts, &path, None).expect("export_mp4 failed");

        let output = std::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=width,height",
                "-of",
                "default=nokey=1:noprint_wrappers=1",
            ])
            .arg(&path)
            .output()
            .expect("ffprobe failed");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut parts = stdout.split_whitespace();
        let width: u32 = parts.next().unwrap_or("0").parse().expect("parse width");
        let height: u32 = parts.next().unwrap_or("0").parse().expect("parse height");

        assert_eq!(width % 2, 0, "output width {width} must be even");
        assert_eq!(height % 2, 0, "output height {height} must be even");
    }

    #[test]
    #[ignore]
    fn test_mp4_progress_callback() {
        let cast = concat!(
            "{\"version\":2,\"width\":10,\"height\":3}\n",
            "[0.5,\"o\",\"hello\"]\n",
            "[1.0,\"o\",\"world\"]",
        );
        let mut player = make_player(cast);
        let opts = Mp4Options {
            fps: 10,
            ..Default::default()
        };

        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let path = tmp.path().with_extension("mp4");

        let calls: RefCell<Vec<(usize, usize)>> = RefCell::new(Vec::new());
        let callback = |current, total| {
            calls.borrow_mut().push((current, total));
        };

        export_mp4(&mut player, &opts, &path, Some(&callback)).expect("export_mp4 failed");

        let recorded = calls.borrow();
        assert!(
            !recorded.is_empty(),
            "progress callback should have been called"
        );

        let total = recorded[0].1;
        assert!(
            recorded.iter().all(|(_, t)| *t == total),
            "total should be consistent across progress calls"
        );

        let last = recorded.last().unwrap();
        assert_eq!(
            last.0, last.1,
            "last progress call should have current == total"
        );

        assert_eq!(
            recorded.len(),
            total,
            "callback should be called exactly total_frames times"
        );
    }

    #[test]
    #[ignore]
    fn test_mp4_empty_recording() {
        // Header only, no events, 0 duration.
        let cast = "{\"version\":2,\"width\":10,\"height\":3}";
        let mut player = make_player(cast);
        let opts = Mp4Options::default();

        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let path = tmp.path().with_extension("mp4");

        // Should neither error nor hang.
        export_mp4(&mut player, &opts, &path, None).expect("export_mp4 failed on empty recording");

        assert!(
            path.exists(),
            "output file should exist even for empty recording"
        );
        let data = std::fs::read(&path).expect("read output");
        assert!(data.len() > 8, "output should be at least 8 bytes");
        assert_eq!(&data[4..8], b"ftyp", "should be valid MP4");
    }
}
