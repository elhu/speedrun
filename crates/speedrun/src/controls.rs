//! Controls bar widget for the terminal session player.
//!
//! A single-row display showing playback state icon, current/total time,
//! a progress bar with marker ticks, and a speed indicator.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Widget;

// ---------------------------------------------------------------------------
// Time formatting
// ---------------------------------------------------------------------------

/// Format a duration in seconds as a human-readable time string.
///
/// - Under 1 hour: `M:SS` (e.g., `0:00`, `1:23`, `59:59`)
/// - 1 hour+: `H:MM:SS` (e.g., `1:00:00`, `12:34:56`)
/// - Negative, NaN, or infinity values clamp to `0:00`.
pub fn format_time(seconds: f64) -> String {
    if seconds.is_nan() || seconds.is_infinite() || seconds < 0.0 {
        return "0:00".to_string();
    }

    let total_secs = seconds as u64;
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;

    if hours > 0 {
        format!("{hours}:{mins:02}:{secs:02}")
    } else {
        format!("{mins}:{secs:02}")
    }
}

// ---------------------------------------------------------------------------
// Progress bar calculation
// ---------------------------------------------------------------------------

/// Calculate progress bar segments for rendering.
///
/// Returns `(filled_width, empty_width, marker_columns)`.
///
/// - `width`: total width of the progress bar in columns
/// - `fraction`: playback position as a fraction of duration, clamped to \[0.0, 1.0\]
/// - `marker_positions`: fractional positions (0.0–1.0) of markers
pub fn progress_bar_segments(
    width: u16,
    fraction: f64,
    marker_positions: &[f64],
) -> (u16, u16, Vec<u16>) {
    if width == 0 {
        return (0, 0, vec![]);
    }

    let fraction = fraction.clamp(0.0, 1.0);
    let filled = (width as f64 * fraction).round() as u16;
    let filled = filled.min(width);
    let empty = width - filled;

    let marker_cols: Vec<u16> = marker_positions
        .iter()
        .filter(|&&p| (0.0..=1.0).contains(&p))
        .map(|&p| {
            let col = (p * width as f64).round() as u16;
            col.min(width.saturating_sub(1))
        })
        .collect();

    (filled, empty, marker_cols)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Format playback speed as a display string (e.g., `1.0×`, `0.25×`).
fn format_speed(speed: f64) -> String {
    if speed.fract() == 0.0 {
        format!("{speed:.1}×")
    } else {
        format!("{speed}×")
    }
}

/// Write a string into the buffer at position `(*x, y)`, advancing `*x`.
fn write_str_at(buf: &mut Buffer, x: &mut u16, y: u16, s: &str, max_x: u16) {
    for ch in s.chars() {
        if *x < max_x {
            buf.get_mut(*x, y).set_char(ch);
            *x += 1;
        }
    }
}

/// Count display width of a string (character count, assuming single-width chars).
fn display_width(s: &str) -> u16 {
    s.chars().count() as u16
}

// ---------------------------------------------------------------------------
// ControlsBar widget
// ---------------------------------------------------------------------------

/// Minimum useful width for the progress bar segment.
const MIN_PROGRESS_WIDTH: u16 = 10;

/// Controls bar widget — a single-row display showing playback state icon,
/// current/total time, a progress bar with marker ticks, and speed.
///
/// Takes a snapshot of player state (not a reference to `Player`) so it's
/// decoupled and testable. The struct is cheap to construct and consumed
/// on render (ratatui 0.26 `Widget` trait).
#[derive(Clone, Debug)]
pub struct ControlsBar {
    /// Whether playback is currently active.
    pub is_playing: bool,
    /// Whether playback has reached the end of the recording.
    pub is_at_end: bool,
    /// Current playback position in effective seconds.
    pub current_time: f64,
    /// Total effective duration in seconds.
    pub duration: f64,
    /// Playback speed multiplier.
    pub speed: f64,
    /// Effective times of markers in the recording.
    pub marker_times: Vec<f64>,
}

impl ControlsBar {
    /// Return the state icon string for the current playback state.
    fn state_icon(&self) -> &'static str {
        if self.is_at_end {
            "■ "
        } else if self.is_playing {
            "▶ "
        } else {
            "▮▮"
        }
    }
}

impl Widget for ControlsBar {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let width = area.width;
        let y = area.y;
        let base_x = area.x;
        let max_x = base_x + width;

        // Build text pieces
        let icon = self.state_icon();
        let current_str = format_time(self.current_time);
        let total_str = format_time(self.duration);
        let speed_str = format_speed(self.speed);
        let time_full = format!("{current_str} / {total_str}");

        // Calculate display widths
        let icon_w: u16 = 2;
        let time_full_w = display_width(&time_full);
        let time_short_w = display_width(&current_str);
        let speed_w = display_width(&speed_str);

        // Fixed widths for each layout level:
        //   Full:        icon + time_full + gap + progress + gap + speed
        //   NoSpeed:     icon + time_full + gap + progress
        //   NoDuration:  icon + time_short + gap + progress
        //   Minimal:     icon + time_short
        let full_fixed = icon_w + time_full_w + 1 + 1 + speed_w;
        let no_speed_fixed = icon_w + time_full_w + 1;
        let no_duration_fixed = icon_w + time_short_w + 1;
        let minimal_fixed = icon_w + time_short_w;

        // Determine layout: which elements fit?
        let (time_str, progress_w, show_speed) = if width >= full_fixed + MIN_PROGRESS_WIDTH {
            (time_full.as_str(), Some(width - full_fixed), true)
        } else if width >= no_speed_fixed + MIN_PROGRESS_WIDTH {
            (time_full.as_str(), Some(width - no_speed_fixed), false)
        } else if width >= no_duration_fixed + MIN_PROGRESS_WIDTH {
            (current_str.as_str(), Some(width - no_duration_fixed), false)
        } else if width >= minimal_fixed {
            (current_str.as_str(), None, false)
        } else {
            return; // Too small — render nothing
        };

        // Fill background for entire row
        let bg_style = Style::default().bg(Color::DarkGray).fg(Color::White);
        for x in base_x..max_x {
            let cell = buf.get_mut(x, y);
            cell.set_style(bg_style);
            cell.set_char(' ');
        }

        let mut x = base_x;

        // 1. State icon (2 chars)
        write_str_at(buf, &mut x, y, icon, max_x);

        // 2. Time display
        write_str_at(buf, &mut x, y, time_str, max_x);

        // 3–5. Progress bar and speed (if room)
        if let Some(pw) = progress_w {
            // Gap before progress bar
            x += 1;

            // Compute playback fraction
            let fraction = if self.duration > 0.0 {
                self.current_time / self.duration
            } else {
                0.0
            };

            // Convert marker times to fractional positions
            let marker_fractions: Vec<f64> = if self.duration > 0.0 {
                self.marker_times
                    .iter()
                    .map(|&t| t / self.duration)
                    .collect()
            } else {
                vec![]
            };

            let (filled, _, marker_cols) = progress_bar_segments(pw, fraction, &marker_fractions);

            let progress_start = x;

            // Filled portion (█)
            for _ in 0..filled {
                if x < max_x {
                    buf.get_mut(x, y).set_char('█');
                    x += 1;
                }
            }

            // Empty portion (░)
            for _ in filled..pw {
                if x < max_x {
                    buf.get_mut(x, y).set_char('░');
                    x += 1;
                }
            }

            // Overlay marker ticks (│)
            for col in marker_cols {
                let mx = progress_start + col;
                if mx < max_x {
                    buf.get_mut(mx, y).set_char('│');
                }
            }

            // Speed indicator (if full layout)
            if show_speed {
                x += 1; // gap after progress bar
                write_str_at(buf, &mut x, y, &speed_str, max_x);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Test helpers ─────────────────────────────────────────────────────────

    fn render_to_string(controls: ControlsBar, width: u16) -> String {
        let area = Rect::new(0, 0, width, 1);
        let mut buf = Buffer::empty(area);
        controls.render(area, &mut buf);
        (0..width)
            .map(|x| buf.get(x, 0).symbol().to_string())
            .collect()
    }

    fn default_controls() -> ControlsBar {
        ControlsBar {
            is_playing: false,
            is_at_end: false,
            current_time: 30.0,
            duration: 120.0,
            speed: 1.0,
            marker_times: vec![],
        }
    }

    // ── format_time unit tests ──────────────────────────────────────────────

    #[test]
    fn format_time_zero() {
        assert_eq!(format_time(0.0), "0:00");
    }

    #[test]
    fn format_time_sixty_two_point_five() {
        assert_eq!(format_time(62.5), "1:02");
    }

    #[test]
    fn format_time_one_hour_one_minute_one_second() {
        assert_eq!(format_time(3661.0), "1:01:01");
    }

    #[test]
    fn format_time_negative_clamps() {
        assert_eq!(format_time(-1.0), "0:00");
    }

    #[test]
    fn format_time_nan_clamps() {
        assert_eq!(format_time(f64::NAN), "0:00");
    }

    #[test]
    fn format_time_infinity_clamps() {
        assert_eq!(format_time(f64::INFINITY), "0:00");
    }

    #[test]
    fn format_time_neg_infinity_clamps() {
        assert_eq!(format_time(f64::NEG_INFINITY), "0:00");
    }

    // ── progress_bar_segments unit tests ─────────────────────────────────────

    #[test]
    fn progress_zero_width() {
        let (f, e, m) = progress_bar_segments(0, 0.5, &[]);
        assert_eq!((f, e), (0, 0));
        assert!(m.is_empty());
    }

    #[test]
    fn progress_zero_percent() {
        let (f, e, _) = progress_bar_segments(20, 0.0, &[]);
        assert_eq!((f, e), (0, 20));
    }

    #[test]
    fn progress_fifty_percent() {
        let (f, e, _) = progress_bar_segments(20, 0.5, &[]);
        assert_eq!((f, e), (10, 10));
    }

    #[test]
    fn progress_hundred_percent() {
        let (f, e, _) = progress_bar_segments(20, 1.0, &[]);
        assert_eq!((f, e), (20, 0));
    }

    #[test]
    fn progress_with_markers() {
        let (_, _, markers) = progress_bar_segments(20, 0.5, &[0.25, 0.75]);
        assert_eq!(markers, vec![5, 15]);
    }

    #[test]
    fn progress_fraction_clamped() {
        let (f, e, _) = progress_bar_segments(20, -0.5, &[]);
        assert_eq!((f, e), (0, 20));
        let (f, e, _) = progress_bar_segments(20, 1.5, &[]);
        assert_eq!((f, e), (20, 0));
    }

    // ── Snapshot tests ───────────────────────────────────────────────────────

    #[test]
    fn snapshot_controls_wide_80() {
        let controls = default_controls();
        insta::assert_snapshot!(render_to_string(controls, 80));
    }

    #[test]
    fn snapshot_controls_medium_40() {
        let controls = default_controls();
        insta::assert_snapshot!(render_to_string(controls, 40));
    }

    #[test]
    fn snapshot_controls_narrow_25() {
        let controls = default_controls();
        insta::assert_snapshot!(render_to_string(controls, 25));
    }

    #[test]
    fn snapshot_controls_very_narrow_15() {
        let controls = default_controls();
        insta::assert_snapshot!(render_to_string(controls, 15));
    }

    #[test]
    fn snapshot_controls_too_small_5() {
        let controls = default_controls();
        insta::assert_snapshot!(render_to_string(controls, 5));
    }
}
