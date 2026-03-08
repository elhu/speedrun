//! Asciicast v2/v3 parser.
//!
//! Parses `.cast` files in [asciicast v2](https://docs.asciinema.org/manual/asciicast/v2/)
//! and v3 formats into a structured [`Recording`]. The parser is lenient:
//! malformed event lines are collected as [`ParseWarning`]s rather than
//! aborting the entire parse, so partially corrupted files can still be played.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A fully parsed asciicast recording.
///
/// Contains the file header, all successfully parsed events, any markers
/// extracted from marker events, and warnings for lines that could not be
/// parsed.
#[derive(Debug, Serialize)]
pub struct Recording {
    /// File header with metadata (version, dimensions, etc.).
    pub header: Header,
    /// Parsed events in chronological order.
    pub events: Vec<Event>,
    /// Markers extracted from marker events for convenient access.
    pub markers: Vec<Marker>,
    /// Warnings produced for malformed event lines.
    pub warnings: Vec<ParseWarning>,
}

/// Header metadata from an asciicast file.
#[derive(Debug, Clone, Serialize)]
pub struct Header {
    /// Asciicast format version (2 or 3).
    pub version: u8,
    /// Initial terminal width in columns.
    pub width: u16,
    /// Initial terminal height in rows.
    pub height: u16,
    /// Unix timestamp of the recording start, if present.
    pub timestamp: Option<u64>,
    /// Maximum idle time between events (seconds), if specified in the header.
    pub idle_time_limit: Option<f64>,
    /// Recording title, if present.
    pub title: Option<String>,
    /// Environment variables captured at recording time, if present.
    pub env: Option<serde_json::Value>,
}

/// The type of a recorded terminal event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum EventType {
    /// Terminal output (`"o"`) — data written to stdout.
    Output,
    /// Terminal input (`"i"`) — data read from stdin.
    Input,
    /// Named marker (`"m"`) — a user-defined label at a point in time.
    Marker,
    /// Terminal resize (`"r"`) — a change in terminal dimensions.
    Resize,
}

/// A single recorded terminal event.
#[derive(Debug, Clone, Serialize)]
pub struct Event {
    /// Raw absolute timestamp in seconds (for v3 files, converted from
    /// relative intervals during parsing).
    pub time: f64,
    /// The kind of event (output, input, marker, or resize).
    pub event_type: EventType,
    /// The event payload.
    pub data: EventData,
}

/// Feed a single event into the terminal emulator.
///
/// Returns `true` if the event changed terminal state (Output or Resize).
/// Input and Marker events return `false`.
///
/// # Examples
///
/// ```
/// use speedrun_core::parser::{Event, EventData, EventType, feed_event};
///
/// let mut vt = speedrun_core::create_vt(80, 24);
/// let event = Event {
///     time: 0.5,
///     event_type: EventType::Output,
///     data: EventData::Text("hello".into()),
/// };
/// assert!(feed_event(&mut vt, &event));
/// ```
pub fn feed_event(vt: &mut avt::Vt, event: &Event) -> bool {
    match (&event.event_type, &event.data) {
        (EventType::Output, EventData::Text(data)) => {
            let _ = vt.feed_str(data);
            true
        }
        (EventType::Resize, EventData::Resize { cols, rows }) => {
            let _ = vt.resize(*cols as usize, *rows as usize);
            true
        }
        _ => false,
    }
}

/// Typed event data. Resize is eagerly parsed into structured dimensions
/// so downstream consumers never parse "COLSxROWS" strings.
#[derive(Debug, Clone, Serialize)]
pub enum EventData {
    /// Text payload for output, input, and marker events.
    Text(String),
    /// Structured resize dimensions.
    Resize {
        /// New terminal width in columns.
        cols: u16,
        /// New terminal height in rows.
        rows: u16,
    },
}

/// A named marker at a specific point in the recording.
#[derive(Debug, Clone, Serialize)]
pub struct Marker {
    /// Timestamp in seconds (raw during parsing; effective after
    /// [`Player`](crate::Player) converts it via the time map).
    pub time: f64,
    /// User-defined label for this marker.
    pub label: String,
}

/// A non-fatal warning produced during lenient parsing.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParseWarning {
    /// 1-based line number in the source file where the issue occurred.
    pub line_number: usize,
    /// Human-readable description of the problem.
    pub message: String,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur when parsing an asciicast file.
#[derive(Debug)]
pub enum ParseError {
    /// The input contains no data (not even a header line).
    EmptyFile,
    /// The first line is not valid asciicast header JSON.
    InvalidHeader {
        /// The raw header line that failed to parse.
        line: String,
        /// The underlying JSON deserialization error.
        source: serde_json::Error,
    },
    /// The header specifies an asciicast version other than 2 or 3.
    UnsupportedVersion {
        /// The unsupported version number.
        version: u64,
    },
    /// A required header field (e.g. `width` or `height`) is missing.
    MissingField {
        /// Name of the missing field.
        field: &'static str,
    },
    /// An event line could not be parsed.
    InvalidEvent {
        /// 1-based line number.
        line_number: usize,
        /// The raw line content.
        content: String,
        /// Human-readable reason the line is invalid.
        reason: String,
    },
    /// A resize event has malformed dimension data.
    InvalidResize {
        /// 1-based line number.
        line_number: usize,
        /// The raw resize data string.
        data: String,
    },
    /// The input is not valid UTF-8.
    NotUtf8 {
        /// The underlying I/O error.
        source: std::io::Error,
    },
    /// An I/O error occurred while reading the input.
    Io {
        /// The underlying I/O error.
        source: std::io::Error,
    },
    /// The input appears to be a binary file (contains null bytes).
    BinaryFile,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::EmptyFile => write!(f, "empty file: no header line found"),
            ParseError::InvalidHeader { line, source } => {
                write!(
                    f,
                    "invalid header JSON: {source} (line: {line}). Expected asciicast v2 or v3 format — see https://docs.asciinema.org/manual/asciicast/v2/"
                )
            }
            ParseError::UnsupportedVersion { version } => {
                write!(f, "unsupported asciicast version: {version}")
            }
            ParseError::MissingField { field } => {
                write!(f, "missing required field: {field}")
            }
            ParseError::InvalidEvent {
                line_number,
                content,
                reason,
            } => {
                write!(
                    f,
                    "invalid event at line {line_number}: {reason} (content: {content})"
                )
            }
            ParseError::InvalidResize { line_number, data } => {
                write!(f, "invalid resize data at line {line_number}: {data}")
            }
            ParseError::NotUtf8 { source } => {
                write!(f, "input is not valid UTF-8: {source}")
            }
            ParseError::Io { source } => {
                write!(f, "I/O error: {source}")
            }
            ParseError::BinaryFile => {
                write!(f, "file appears to be binary, not an asciicast recording")
            }
        }
    }
}

impl std::error::Error for ParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ParseError::InvalidHeader { source, .. } => Some(source),
            ParseError::NotUtf8 { source } => Some(source),
            ParseError::Io { source } => Some(source),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Private serde deserialization helpers
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[allow(dead_code)]
struct RawHeader {
    version: u64, // u64 to detect unsupported versions like 99
    // V2 fields (top-level)
    width: Option<u16>,
    height: Option<u16>,
    // V3 fields (nested under term)
    term: Option<RawTerm>,
    // Shared
    timestamp: Option<u64>,
    idle_time_limit: Option<f64>,
    title: Option<String>,
    env: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct RawTerm {
    cols: Option<u16>,
    rows: Option<u16>,
    #[serde(rename = "type")]
    #[allow(dead_code)]
    term_type: Option<String>,
    #[allow(dead_code)]
    version: Option<String>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse an asciicast v2 or v3 recording from a reader.
///
/// The parser is lenient: malformed event lines are recorded as
/// [`ParseWarning`]s and skipped rather than failing the entire parse.
/// Only header-level errors (missing header, unsupported version, etc.)
/// produce a hard [`ParseError`].
///
/// # Examples
///
/// ```
/// let data = b"{\"version\":2,\"width\":80,\"height\":24}\n[0.5,\"o\",\"hello\"]\n[1.0,\"o\",\" world\"]";
/// let recording = speedrun_core::parser::parse(&data[..]).unwrap();
/// assert_eq!(recording.header.version, 2);
/// assert_eq!(recording.header.width, 80);
/// assert_eq!(recording.events.len(), 2);
/// assert!(recording.warnings.is_empty());
/// ```
pub fn parse(reader: impl std::io::Read) -> Result<Recording, ParseError> {
    use std::io::{BufRead, BufReader, ErrorKind};

    let mut reader = BufReader::new(reader);

    // -----------------------------------------------------------------------
    // 0. Binary sniff check: peek at first 512 bytes for null bytes
    // -----------------------------------------------------------------------
    {
        let buf = reader.fill_buf().map_err(|e| {
            if e.kind() == ErrorKind::InvalidData {
                ParseError::NotUtf8 { source: e }
            } else {
                ParseError::Io { source: e }
            }
        })?;
        if buf.contains(&0u8) {
            return Err(ParseError::BinaryFile);
        }
    }

    let mut lines = reader.lines();

    // -----------------------------------------------------------------------
    // 1. Header parsing
    // -----------------------------------------------------------------------
    let first_line = match lines.next() {
        None => return Err(ParseError::EmptyFile),
        Some(Err(e)) if e.kind() == ErrorKind::InvalidData => {
            return Err(ParseError::NotUtf8 { source: e });
        }
        Some(Err(e)) => return Err(ParseError::Io { source: e }),
        Some(Ok(line)) => line,
    };

    let raw: RawHeader =
        serde_json::from_str(&first_line).map_err(|e| ParseError::InvalidHeader {
            line: first_line.clone(),
            source: e,
        })?;

    if raw.version != 2 && raw.version != 3 {
        return Err(ParseError::UnsupportedVersion {
            version: raw.version,
        });
    }

    let version = raw.version as u8;

    // Extract width/height based on version
    let (width, height) = if version == 3 {
        let term = raw.term.as_ref();
        let cols = term
            .and_then(|t| t.cols)
            .ok_or(ParseError::MissingField { field: "width" })?;
        let rows = term
            .and_then(|t| t.rows)
            .ok_or(ParseError::MissingField { field: "height" })?;
        (cols, rows)
    } else {
        let w = raw
            .width
            .ok_or(ParseError::MissingField { field: "width" })?;
        let h = raw
            .height
            .ok_or(ParseError::MissingField { field: "height" })?;
        (w, h)
    };

    let header = Header {
        version,
        width,
        height,
        timestamp: raw.timestamp,
        idle_time_limit: raw.idle_time_limit,
        title: raw.title,
        env: raw.env,
    };

    // -----------------------------------------------------------------------
    // 2. Event parsing (lenient — malformed lines become warnings)
    // -----------------------------------------------------------------------
    let mut events = Vec::new();
    let mut warnings = Vec::new();
    let mut line_number: usize = 1; // header is line 1
    // prev_time is updated only when a V2 event is successfully parsed.
    let mut prev_time: f64 = 0.0;
    // absolute_time tracks the running sum for V3 relative timestamps.
    // It is NOT updated when a line is skipped (we cannot parse the delta
    // from a malformed line); the next valid line's delta is added to the
    // last valid event's absolute time.
    let mut absolute_time: f64 = 0.0;

    for line_result in lines {
        line_number += 1;

        let line = match line_result {
            Ok(l) => l,
            Err(e) if e.kind() == ErrorKind::InvalidData => {
                // Mid-stream UTF-8 / IO error: stop reading, record a warning
                warnings.push(ParseWarning {
                    line_number,
                    message: format!("I/O error reading line: {e}"),
                });
                break;
            }
            Err(e) => {
                // Other mid-stream IO error: stop reading, record a warning
                warnings.push(ParseWarning {
                    line_number,
                    message: format!("I/O error reading line: {e}"),
                });
                break;
            }
        };

        // Skip empty/whitespace-only lines
        if line.trim().is_empty() {
            continue;
        }

        // --- Try to parse this event line; any failure → warning + continue ---

        let val: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => {
                warnings.push(ParseWarning {
                    line_number,
                    message: format!("invalid JSON: {line}"),
                });
                continue;
            }
        };

        let arr = match val.as_array() {
            Some(a) => a,
            None => {
                warnings.push(ParseWarning {
                    line_number,
                    message: format!("expected array of 3 elements: {line}"),
                });
                continue;
            }
        };

        if arr.len() != 3 {
            warnings.push(ParseWarning {
                line_number,
                message: format!("expected array of 3 elements, got {}: {line}", arr.len()),
            });
            continue;
        }

        // Timestamp
        let raw_time = match arr[0].as_f64() {
            Some(t) => t,
            None => {
                warnings.push(ParseWarning {
                    line_number,
                    message: format!("invalid timestamp: {line}"),
                });
                continue;
            }
        };

        // Version-specific time handling
        let time = if version == 3 {
            if raw_time < 0.0 {
                warnings.push(ParseWarning {
                    line_number,
                    message: format!("negative interval {raw_time}: {line}"),
                });
                continue;
            }
            // V3: timestamps are relative intervals; accumulate into absolute time.
            // absolute_time is not updated for skipped lines.
            absolute_time += raw_time;
            absolute_time
        } else {
            // V2: timestamps are absolute. Emit a warning for out-of-order
            // events but still accept them — they will be sorted after the
            // loop so appended markers (whose raw times may be earlier than
            // the last event) are not silently dropped.
            if raw_time < prev_time {
                warnings.push(ParseWarning {
                    line_number,
                    message: format!(
                        "timestamps must be non-decreasing (got {raw_time} after {prev_time}): {line}"
                    ),
                });
            } else {
                // Only advance prev_time for in-order events so the warning
                // accurately identifies which events are out of order.
                prev_time = raw_time;
            }
            raw_time
        };

        // Event type
        let type_str = match arr[1].as_str() {
            Some(s) => s,
            None => {
                warnings.push(ParseWarning {
                    line_number,
                    message: format!("invalid event type: {line}"),
                });
                continue;
            }
        };

        let event_type = match type_str {
            "o" => EventType::Output,
            "i" => EventType::Input,
            "m" => EventType::Marker,
            "r" => EventType::Resize,
            _ => {
                // Unknown event type → skip silently
                continue;
            }
        };

        // Data
        let data_str = match arr[2].as_str() {
            Some(s) => s,
            None => {
                warnings.push(ParseWarning {
                    line_number,
                    message: format!("invalid event data: {line}"),
                });
                continue;
            }
        };

        // Build EventData based on type
        let data = match event_type {
            EventType::Resize => {
                let parts: Vec<&str> = data_str.split('x').collect();
                if parts.len() != 2 {
                    warnings.push(ParseWarning {
                        line_number,
                        message: format!("invalid resize format (expected COLSxROWS): {data_str}"),
                    });
                    continue;
                }
                let cols: u16 = match parts[0].parse() {
                    Ok(v) => v,
                    Err(_) => {
                        warnings.push(ParseWarning {
                            line_number,
                            message: format!(
                                "invalid resize format (non-numeric cols): {data_str}"
                            ),
                        });
                        continue;
                    }
                };
                let rows: u16 = match parts[1].parse() {
                    Ok(v) => v,
                    Err(_) => {
                        warnings.push(ParseWarning {
                            line_number,
                            message: format!(
                                "invalid resize format (non-numeric rows): {data_str}"
                            ),
                        });
                        continue;
                    }
                };
                EventData::Resize { cols, rows }
            }
            _ => EventData::Text(data_str.to_string()),
        };

        events.push(Event {
            time,
            event_type,
            data,
        });
    }

    // Sort events by timestamp (stable sort preserves file order for ties).
    // This is a no-op for properly-ordered v2 files and for v3 files (whose
    // accumulated absolute times are monotonically increasing by construction).
    // For v2 files with appended out-of-order events (e.g. appended markers),
    // this places every event at its correct chronological position.
    events.sort_by(|a, b| {
        a.time
            .partial_cmp(&b.time)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Re-derive markers from the now-sorted events so their order matches.
    let markers: Vec<Marker> = events
        .iter()
        .filter(|e| e.event_type == EventType::Marker)
        .map(|e| Marker {
            time: e.time,
            label: match &e.data {
                EventData::Text(s) => s.clone(),
                _ => String::new(),
            },
        })
        .collect();

    Ok(Recording {
        header,
        events,
        markers,
        warnings,
    })
}

/// Serialize a marker event as an asciicast v2 NDJSON line.
///
/// Returns a string like `[3.0,"m","chapter-1"]`. The caller is responsible
/// for appending a newline and writing to the file.
///
/// # Examples
///
/// ```
/// use speedrun_core::serialize_marker_event;
///
/// let line = serialize_marker_event(3.0, "chapter-1");
/// assert_eq!(line, r#"[3.0,"m","chapter-1"]"#);
/// ```
pub fn serialize_marker_event(raw_time: f64, label: &str) -> String {
    // Round to 6 decimal places to avoid floating-point noise
    let rounded = (raw_time * 1_000_000.0).round() / 1_000_000.0;
    serde_json::to_string(&(rounded, "m", label)).expect("marker tuple serialization cannot fail")
}

// ---------------------------------------------------------------------------
// V3 rewrite support
// ---------------------------------------------------------------------------

/// Errors that can occur when rewriting a v3 asciicast file to insert a marker.
#[derive(Debug)]
pub enum RewriteError {
    /// The input is not a v3 asciicast file.
    NotV3,
    /// The input has no header line.
    EmptyFile,
}

impl std::fmt::Display for RewriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RewriteError::NotV3 => write!(f, "input is not a v3 asciicast file"),
            RewriteError::EmptyFile => write!(f, "input has no header line"),
        }
    }
}

impl std::error::Error for RewriteError {}

/// Attempt to parse a line as a timestamped v3 event, returning the delta if
/// successful.
///
/// A line is considered timestamped if it is a valid 3-element JSON array with
/// a non-negative numeric first element. This matches the parser's accumulation
/// semantics (lines 468-480) where `absolute_time` advances before event type
/// matching, so unknown event types with valid timestamps also count.
fn try_parse_delta(line: &str) -> Option<f64> {
    let val: serde_json::Value = serde_json::from_str(line).ok()?;
    let arr = val.as_array()?;
    if arr.len() != 3 {
        return None;
    }
    let delta = arr[0].as_f64()?;
    if delta < 0.0 {
        return None;
    }
    Some(delta)
}

/// Replace the timestamp in a JSON array line (content between `[` and first
/// `,`) with a new delta value, preserving the rest of the line byte-for-byte.
///
/// Handles integers (`[1,`), decimals (`[0.5,`), and leading whitespace
/// (`[ 0.5,`).
fn replace_first_timestamp(line: &str, new_delta: f64) -> String {
    let rounded = (new_delta * 1_000_000.0).round() / 1_000_000.0;
    let formatted = serde_json::to_string(&rounded).expect("f64 serialization cannot fail");

    let bracket_pos = line.find('[').expect("timestamped line must contain '['");
    let search_start = bracket_pos + 1;
    let comma_pos = line[search_start..]
        .find(',')
        .expect("timestamped line must contain ','")
        + search_start;

    format!(
        "{}{}{}",
        &line[..bracket_pos + 1],
        formatted,
        &line[comma_pos..]
    )
}

/// Rewrite a v3 asciicast file to insert a marker at the given absolute time.
///
/// V3 files use relative timestamps (deltas), so a marker cannot simply be
/// appended — it must be inserted at the correct chronological position and the
/// following event's delta must be patched. All other lines are preserved
/// byte-for-byte.
///
/// # Arguments
///
/// * `input` — the full content of the v3 `.cast` file
/// * `raw_time` — the absolute time (in seconds) where the marker should appear
/// * `label` — the marker label
///
/// # Errors
///
/// Returns [`RewriteError::EmptyFile`] if the input has no header line, or
/// [`RewriteError::NotV3`] if the header does not specify version 3.
///
/// # Examples
///
/// ```
/// use speedrun_core::parser::rewrite_v3_with_marker;
///
/// let input = "{\"version\":3,\"term\":{\"cols\":80,\"rows\":24}}\n\
///              [0.5,\"o\",\"hello\"]\n\
///              [0.5,\"o\",\"world\"]\n";
/// let output = rewrite_v3_with_marker(input, 0.7, "mid").unwrap();
/// assert!(output.contains("\"m\""));
/// ```
pub fn rewrite_v3_with_marker(
    input: &str,
    raw_time: f64,
    label: &str,
) -> Result<String, RewriteError> {
    let has_trailing_newline = input.ends_with('\n');
    let lines: Vec<&str> = input.lines().collect();

    if lines.is_empty() {
        return Err(RewriteError::EmptyFile);
    }

    // Validate v3 header
    let header_val: serde_json::Value =
        serde_json::from_str(lines[0]).map_err(|_| RewriteError::NotV3)?;
    if header_val.get("version").and_then(|v| v.as_u64()) != Some(3) {
        return Err(RewriteError::NotV3);
    }

    // Scan event lines to find the insertion point: the first timestamped line
    // whose accumulated absolute time is strictly greater than `raw_time`.
    let mut absolute_time: f64 = 0.0;
    let mut insertion_idx: Option<usize> = None;
    let mut prev_abs_at_insertion: f64 = 0.0;
    let mut acc_at_insertion: f64 = 0.0;

    for (i, line) in lines.iter().enumerate().skip(1) {
        if let Some(delta) = try_parse_delta(line) {
            let prev_abs = absolute_time;
            absolute_time += delta;

            if insertion_idx.is_none() && absolute_time > raw_time {
                insertion_idx = Some(i);
                prev_abs_at_insertion = prev_abs;
                acc_at_insertion = absolute_time;
            }
        }
    }

    // Build output
    let mut result_lines: Vec<String> = Vec::with_capacity(lines.len() + 1);

    match insertion_idx {
        Some(ins_idx) => {
            // Insert marker before the line at ins_idx and patch its delta.
            let marker_delta = raw_time - prev_abs_at_insertion;
            let marker_delta_rounded = (marker_delta * 1_000_000.0).round() / 1_000_000.0;
            let marker_line = serde_json::to_string(&(marker_delta_rounded, "m", label))
                .expect("marker tuple serialization cannot fail");

            let patched_delta = acc_at_insertion - raw_time;
            let patched_line = replace_first_timestamp(lines[ins_idx], patched_delta);

            for (i, line) in lines.iter().enumerate() {
                if i == ins_idx {
                    result_lines.push(marker_line.clone());
                    result_lines.push(patched_line.clone());
                } else {
                    result_lines.push((*line).to_string());
                }
            }
        }
        None => {
            // Marker goes after all events.
            let marker_delta = raw_time - absolute_time;
            let marker_delta_rounded = (marker_delta * 1_000_000.0).round() / 1_000_000.0;
            let marker_line = serde_json::to_string(&(marker_delta_rounded, "m", label))
                .expect("marker tuple serialization cannot fail");

            for line in &lines {
                result_lines.push((*line).to_string());
            }
            result_lines.push(marker_line);
        }
    }

    let mut output = result_lines.join("\n");
    if has_trailing_newline {
        output.push('\n');
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // -----------------------------------------------------------------------
    // Valid file tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_minimal_v2() {
        let file = std::fs::File::open(crate::testdata_path("minimal_v2.cast")).unwrap();
        let recording = parse(file).unwrap();

        assert_eq!(recording.header.version, 2);
        assert_eq!(recording.header.width, 80);
        assert_eq!(recording.header.height, 24);
        assert_eq!(recording.events.len(), 4);
        assert_eq!(recording.markers.len(), 0);

        let timestamps: Vec<f64> = recording.events.iter().map(|e| e.time).collect();
        assert_eq!(timestamps, vec![0.5, 1.2, 2.0, 2.1]);

        for event in &recording.events {
            assert_eq!(event.event_type, EventType::Output);
            assert!(matches!(event.data, EventData::Text(_)));
        }
    }

    #[test]
    fn test_parse_minimal_v3() {
        let file = std::fs::File::open(crate::testdata_path("minimal_v3.cast")).unwrap();
        let recording = parse(file).unwrap();

        assert_eq!(recording.header.version, 3);
        assert_eq!(recording.header.width, 80);
        assert_eq!(recording.header.height, 24);
        assert_eq!(recording.events.len(), 4);

        // V3 relative intervals: 0.5, 0.7, 0.8, 0.9 → absolute 0.5, 1.2, 2.0, 2.9
        let timestamps: Vec<f64> = recording.events.iter().map(|e| e.time).collect();
        let expected = vec![0.5, 1.2, 2.0, 2.9];
        for (actual, exp) in timestamps.iter().zip(expected.iter()) {
            assert!((actual - exp).abs() < 1e-9, "expected {exp}, got {actual}");
        }

        for event in &recording.events {
            assert_eq!(event.event_type, EventType::Output);
        }
    }

    #[test]
    fn test_parse_empty() {
        let file = std::fs::File::open(crate::testdata_path("empty.cast")).unwrap();
        let recording = parse(file).unwrap();

        assert_eq!(recording.header.version, 2);
        assert_eq!(recording.header.width, 80);
        assert_eq!(recording.header.height, 24);
        assert_eq!(recording.events.len(), 0);
        assert_eq!(recording.markers.len(), 0);
    }

    #[test]
    fn test_parse_with_markers() {
        let file = std::fs::File::open(crate::testdata_path("with_markers.cast")).unwrap();
        let recording = parse(file).unwrap();

        assert_eq!(recording.events.len(), 8);

        let output_count = recording
            .events
            .iter()
            .filter(|e| e.event_type == EventType::Output)
            .count();
        let marker_count = recording
            .events
            .iter()
            .filter(|e| e.event_type == EventType::Marker)
            .count();
        assert_eq!(output_count, 6);
        assert_eq!(marker_count, 2);

        assert_eq!(recording.markers.len(), 2);
        assert_eq!(recording.markers[0].label, "chapter-1");
        assert!((recording.markers[0].time - 3.0).abs() < 1e-9);
        assert_eq!(recording.markers[1].label, "chapter-2");
        assert!((recording.markers[1].time - 7.0).abs() < 1e-9);
    }

    #[test]
    fn test_parse_with_resize() {
        let file = std::fs::File::open(crate::testdata_path("with_resize.cast")).unwrap();
        let recording = parse(file).unwrap();

        assert_eq!(recording.events.len(), 6);

        let resize_event = &recording.events[2];
        assert_eq!(resize_event.event_type, EventType::Resize);
        match &resize_event.data {
            EventData::Resize { cols, rows } => {
                assert_eq!(*cols, 120);
                assert_eq!(*rows, 40);
            }
            _ => panic!("expected Resize data at index 2"),
        }
    }

    #[test]
    fn test_parse_remaining_valid_files() {
        // long_idle.cast
        let file = std::fs::File::open(crate::testdata_path("long_idle.cast")).unwrap();
        let recording = parse(file).unwrap();
        assert_eq!(recording.header.idle_time_limit, Some(2.0));
        assert_eq!(recording.events.len(), 5);

        // alternate_buffer.cast
        let file = std::fs::File::open(crate::testdata_path("alternate_buffer.cast")).unwrap();
        let recording = parse(file).unwrap();
        assert_eq!(recording.events.len(), 9);

        // real_session.cast — v3, 188x50, at least 100 events, timestamp 1772729753
        let file = std::fs::File::open(crate::testdata_path("real_session.cast")).unwrap();
        let recording = parse(file).unwrap();
        assert_eq!(recording.header.version, 3);
        assert_eq!(recording.header.width, 188);
        assert_eq!(recording.header.height, 50);
        assert!(
            recording.events.len() >= 100,
            "expected at least 100 events, got {}",
            recording.events.len()
        );
        // 114 output events (the "x" event is skipped)
        assert_eq!(recording.events.len(), 114);
        assert_eq!(recording.header.timestamp, Some(1772729753));
    }

    // -----------------------------------------------------------------------
    // Invalid file tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_invalid_empty_file() {
        let file = std::fs::File::open(crate::testdata_path("invalid/empty_file.cast")).unwrap();
        let err = parse(file).unwrap_err();
        assert!(matches!(err, ParseError::EmptyFile));
    }

    #[test]
    fn test_invalid_no_header() {
        let file = std::fs::File::open(crate::testdata_path("invalid/no_header.cast")).unwrap();
        let err = parse(file).unwrap_err();
        assert!(
            matches!(err, ParseError::InvalidHeader { .. }),
            "expected InvalidHeader, got {err:?}"
        );
    }

    #[test]
    fn test_invalid_bad_json() {
        let file = std::fs::File::open(crate::testdata_path("invalid/bad_json.cast")).unwrap();
        let err = parse(file).unwrap_err();
        assert!(
            matches!(err, ParseError::InvalidHeader { .. }),
            "expected InvalidHeader, got {err:?}"
        );
    }

    #[test]
    fn test_invalid_bad_version() {
        let file = std::fs::File::open(crate::testdata_path("invalid/bad_version.cast")).unwrap();
        let err = parse(file).unwrap_err();
        assert!(
            matches!(err, ParseError::UnsupportedVersion { version: 99 }),
            "expected UnsupportedVersion(99), got {err:?}"
        );
    }

    #[test]
    fn test_invalid_missing_fields() {
        let file =
            std::fs::File::open(crate::testdata_path("invalid/missing_fields.cast")).unwrap();
        let err = parse(file).unwrap_err();
        assert!(
            matches!(err, ParseError::MissingField { .. }),
            "expected MissingField, got {err:?}"
        );
    }

    #[test]
    fn test_invalid_bad_event() {
        // bad_event.cast has valid header + malformed event [0.5, "o"] (only 2 elements)
        // Under lenient parsing: Ok with 1 warning, valid events after the bad one are kept
        let file = std::fs::File::open(crate::testdata_path("invalid/bad_event.cast")).unwrap();
        let recording = parse(file).unwrap();
        assert!(
            recording.warnings.len() >= 1,
            "expected at least 1 warning, got {}",
            recording.warnings.len()
        );
        let first_warning = &recording.warnings[0];
        assert_eq!(
            first_warning.line_number, 2,
            "expected warning on line 2, got {}",
            first_warning.line_number
        );
        assert!(
            first_warning.message.contains("3 elements"),
            "expected warning about 3 elements, got: {}",
            first_warning.message
        );
    }

    #[test]
    fn test_invalid_binary_garbage() {
        let file =
            std::fs::File::open(crate::testdata_path("invalid/binary_garbage.cast")).unwrap();
        let err = parse(file).unwrap_err();
        assert!(
            matches!(
                err,
                ParseError::NotUtf8 { .. }
                    | ParseError::InvalidHeader { .. }
                    | ParseError::BinaryFile
            ),
            "expected NotUtf8, InvalidHeader, or BinaryFile, got {err:?}"
        );
    }

    #[test]
    fn test_invalid_truncated() {
        // truncated.cast has a valid header and one valid event, then a truncated line.
        // Under lenient parsing: Ok with at least 1 event and possibly a warning
        let file = std::fs::File::open(crate::testdata_path("invalid/truncated.cast")).unwrap();
        let recording = parse(file).unwrap();
        // Should have at least the first valid event
        assert!(
            recording.events.len() >= 1,
            "expected at least 1 event, got {}",
            recording.events.len()
        );
    }

    // -----------------------------------------------------------------------
    // Synthetic edge case tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_v2_non_monotonic_timestamps() {
        let input = r#"{"version": 2, "width": 80, "height": 24}
[1.0, "o", "a"]
[3.0, "o", "b"]
[2.0, "o", "c"]
"#;
        // Out-of-order events are now accepted and sorted; a warning is still
        // emitted for diagnostics.
        let recording = parse(Cursor::new(input)).unwrap();
        assert_eq!(
            recording.events.len(),
            3,
            "expected 3 events (all accepted and sorted)"
        );
        let timestamps: Vec<f64> = recording.events.iter().map(|e| e.time).collect();
        assert_eq!(timestamps, vec![1.0, 2.0, 3.0], "events should be sorted");
        assert_eq!(
            recording.warnings.len(),
            1,
            "expected 1 warning for non-monotonic timestamp"
        );
        assert!(
            recording.warnings[0].message.contains("non-decreasing"),
            "expected warning about non-decreasing, got: {}",
            recording.warnings[0].message
        );
    }

    #[test]
    fn test_v3_negative_interval() {
        let input = r#"{"version": 3, "term": {"cols": 80, "rows": 24}}
[0.5, "o", "a"]
[-0.1, "o", "b"]
[0.3, "o", "c"]
"#;
        // Under lenient parsing: Ok with 1 warning (line 3 skipped), 2 events
        let recording = parse(Cursor::new(input)).unwrap();
        assert_eq!(
            recording.events.len(),
            2,
            "expected 2 events (line 3 skipped)"
        );
        assert_eq!(
            recording.warnings.len(),
            1,
            "expected 1 warning for negative interval"
        );
        assert!(
            recording.warnings[0].message.contains("negative"),
            "expected warning about negative, got: {}",
            recording.warnings[0].message
        );
    }

    #[test]
    fn test_unknown_event_type_skipped() {
        let input = r#"{"version": 2, "width": 80, "height": 24}
[1.0, "o", "hello"]
[2.0, "x", "unknown"]
[3.0, "o", "world"]
"#;
        let recording = parse(Cursor::new(input)).unwrap();
        // Unknown "x" event should be skipped, leaving 2 output events
        assert_eq!(recording.events.len(), 2);
        assert_eq!(recording.events[0].event_type, EventType::Output);
        assert_eq!(recording.events[1].event_type, EventType::Output);
    }

    #[test]
    fn test_resize_bad_format_missing_separator() {
        let input = r#"{"version": 2, "width": 80, "height": 24}
[1.0, "r", "120"]
"#;
        // Under lenient parsing: Ok with 1 warning, 0 events
        let recording = parse(Cursor::new(input)).unwrap();
        assert_eq!(recording.events.len(), 0, "expected 0 events");
        assert_eq!(
            recording.warnings.len(),
            1,
            "expected 1 warning for bad resize"
        );
    }

    #[test]
    fn test_resize_bad_format_non_numeric() {
        let input = r#"{"version": 2, "width": 80, "height": 24}
[1.0, "r", "abcxdef"]
"#;
        // Under lenient parsing: Ok with 1 warning, 0 events
        let recording = parse(Cursor::new(input)).unwrap();
        assert_eq!(recording.events.len(), 0, "expected 0 events");
        assert_eq!(
            recording.warnings.len(),
            1,
            "expected 1 warning for bad resize"
        );
    }

    // -----------------------------------------------------------------------
    // New lenient parsing tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_lenient_parse_mixed_valid_invalid() {
        // 10 valid events + 1 malformed at position 5
        let mut input = String::from("{\"version\": 2, \"width\": 80, \"height\": 24}\n");
        for i in 1..=4 {
            input.push_str(&format!("[{}.0, \"o\", \"event{}\"]\n", i, i));
        }
        // malformed: only 2 elements (position 5 = line 6)
        input.push_str("[5.0, \"o\"]\n");
        for i in 6..=10 {
            // skip i=5 since we used 5.0 for the bad line
            input.push_str(&format!("[{}.0, \"o\", \"event{}\"]\n", i, i));
        }
        let recording = parse(Cursor::new(input)).unwrap();
        assert_eq!(recording.events.len(), 9, "expected 9 valid events");
        assert_eq!(recording.warnings.len(), 1, "expected 1 warning");
        assert_eq!(
            recording.warnings[0].line_number, 6,
            "expected warning on line 6"
        );
        assert!(
            recording.warnings[0].message.contains("3 elements"),
            "expected warning about 3 elements"
        );
    }

    #[test]
    fn test_all_bad_events() {
        // Valid header + 5 malformed event lines
        let input = r#"{"version": 2, "width": 80, "height": 24}
[1.0, "o"]
[2.0, "o"]
[3.0, "o"]
[4.0, "o"]
[5.0, "o"]
"#;
        let recording = parse(Cursor::new(input)).unwrap();
        assert_eq!(recording.events.len(), 0, "expected 0 events");
        assert_eq!(recording.warnings.len(), 5, "expected 5 warnings");
    }

    #[test]
    fn test_v3_skipped_line_timestamps() {
        // 3 V3 events where event 2 is malformed
        // Event 1: delta 0.5 → abs 0.5
        // Event 2: malformed → skipped, absolute_time stays at 0.5
        // Event 3: delta 0.3 → abs 0.5 + 0.3 = 0.8
        let input = r#"{"version": 3, "term": {"cols": 80, "rows": 24}}
[0.5, "o", "a"]
[0.2, "o"]
[0.3, "o", "c"]
"#;
        let recording = parse(Cursor::new(input)).unwrap();
        assert_eq!(recording.events.len(), 2, "expected 2 events");
        assert_eq!(recording.warnings.len(), 1, "expected 1 warning");
        // First event absolute time: 0.5
        assert!(
            (recording.events[0].time - 0.5).abs() < 1e-9,
            "expected first event at t=0.5, got {}",
            recording.events[0].time
        );
        // Third event absolute time: 0.5 + 0.3 = 0.8 (skipped line's delta lost)
        assert!(
            (recording.events[1].time - 0.8).abs() < 1e-9,
            "expected second event at t=0.8, got {}",
            recording.events[1].time
        );
    }

    #[test]
    fn test_binary_sniff() {
        // Input with null bytes should produce BinaryFile error
        let input: Vec<u8> = vec![0x00, 0x01, 0x02, 0x03, b'h', b'e', b'l', b'l', b'o'];
        let err = parse(Cursor::new(input)).unwrap_err();
        assert!(
            matches!(err, ParseError::BinaryFile),
            "expected BinaryFile, got {err:?}"
        );
    }

    #[test]
    fn test_happy_path_regression_minimal_v2() {
        let file = std::fs::File::open(crate::testdata_path("minimal_v2.cast")).unwrap();
        let recording = parse(file).unwrap();
        assert_eq!(recording.events.len(), 4, "expected 4 events");
        assert!(recording.warnings.is_empty(), "expected no warnings");
    }

    #[test]
    fn test_happy_path_regression_real_session() {
        let file = std::fs::File::open(crate::testdata_path("real_session.cast")).unwrap();
        let recording = parse(file).unwrap();
        assert_eq!(recording.events.len(), 114, "expected 114 events");
        assert!(recording.warnings.is_empty(), "expected no warnings");
    }

    // -----------------------------------------------------------------------
    // New fixture tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_input_only() {
        let file = std::fs::File::open(crate::testdata_path("input_only.cast")).unwrap();
        let recording = parse(file).unwrap();
        assert_eq!(recording.header.version, 3);
        assert_eq!(recording.header.width, 80);
        assert_eq!(recording.header.height, 24);
        assert_eq!(recording.events.len(), 3);
        assert!(recording.warnings.is_empty());
        // All events should be Input type
        for event in &recording.events {
            assert_eq!(event.event_type, EventType::Input);
        }
        // V3 relative intervals: 0.5, 0.5, 0.5 → absolute 0.5, 1.0, 1.5
        let timestamps: Vec<f64> = recording.events.iter().map(|e| e.time).collect();
        let expected = vec![0.5, 1.0, 1.5];
        for (actual, exp) in timestamps.iter().zip(expected.iter()) {
            assert!((actual - exp).abs() < 1e-9, "expected {exp}, got {actual}");
        }
    }

    #[test]
    fn test_parse_sub_second() {
        let file = std::fs::File::open(crate::testdata_path("sub_second.cast")).unwrap();
        let recording = parse(file).unwrap();
        assert_eq!(recording.header.version, 2);
        assert_eq!(recording.header.width, 80);
        assert_eq!(recording.header.height, 24);
        assert_eq!(recording.events.len(), 3);
        assert!(recording.warnings.is_empty());
        let timestamps: Vec<f64> = recording.events.iter().map(|e| e.time).collect();
        let expected = vec![0.1, 0.3, 0.5];
        for (actual, exp) in timestamps.iter().zip(expected.iter()) {
            assert!((actual - exp).abs() < 1e-9, "expected {exp}, got {actual}");
        }
    }

    // -----------------------------------------------------------------------
    // Insta snapshot test
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_minimal_v2_snapshot() {
        let file = std::fs::File::open(crate::testdata_path("minimal_v2.cast")).unwrap();
        let recording = parse(file).unwrap();
        insta::assert_debug_snapshot!(recording);
    }

    // -----------------------------------------------------------------------
    // feed_event() unit tests
    // -----------------------------------------------------------------------

    fn make_event(event_type: EventType, data: EventData) -> Event {
        Event {
            time: 0.0,
            event_type,
            data,
        }
    }

    #[test]
    fn test_feed_event_output() {
        let mut vt = crate::create_vt(80, 24);
        let event = make_event(EventType::Output, EventData::Text("hello".into()));
        let changed = feed_event(&mut vt, &event);
        assert!(changed, "Output event should return true");
        assert!(
            vt.line(0).text().contains("hello"),
            "terminal should show 'hello' after Output event"
        );
    }

    #[test]
    fn test_feed_event_resize() {
        let mut vt = crate::create_vt(80, 24);
        let event = make_event(
            EventType::Resize,
            EventData::Resize {
                cols: 120,
                rows: 40,
            },
        );
        let changed = feed_event(&mut vt, &event);
        assert!(changed, "Resize event should return true");
        assert_eq!(
            vt.size(),
            (120, 40),
            "terminal size should change after Resize event"
        );
    }

    #[test]
    fn test_feed_event_input() {
        let mut vt = crate::create_vt(80, 24);
        let event = make_event(EventType::Input, EventData::Text("some input".into()));
        let changed = feed_event(&mut vt, &event);
        assert!(!changed, "Input event should return false");
    }

    #[test]
    fn test_feed_event_marker() {
        let mut vt = crate::create_vt(80, 24);
        let event = make_event(EventType::Marker, EventData::Text("my-marker".into()));
        let changed = feed_event(&mut vt, &event);
        assert!(!changed, "Marker event should return false");
    }

    // -----------------------------------------------------------------------
    // Out-of-order event acceptance tests (speedrun-fr8.1)
    // -----------------------------------------------------------------------

    #[test]
    fn test_out_of_order_marker_accepted() {
        // V2 file: output events at t=1.0, 3.0, 8.0; marker appended after
        // t=8.0 but with a raw time of 5.0 (simulates appended marker).
        let input = r#"{"version": 2, "width": 80, "height": 24}
[1.0, "o", "a"]
[3.0, "o", "b"]
[8.0, "o", "c"]
[5.0, "m", "mid"]
"#;
        let recording = parse(Cursor::new(input)).unwrap();
        // All 4 events accepted and sorted by time.
        assert_eq!(recording.events.len(), 4, "expected 4 events");
        let timestamps: Vec<f64> = recording.events.iter().map(|e| e.time).collect();
        assert_eq!(
            timestamps,
            vec![1.0, 3.0, 5.0, 8.0],
            "events should be sorted by time"
        );
        // Marker appears between t=3.0 and t=8.0.
        assert_eq!(recording.events[2].event_type, EventType::Marker);
        // Markers vec has 1 entry at time 5.0.
        assert_eq!(recording.markers.len(), 1);
        assert!((recording.markers[0].time - 5.0).abs() < 1e-9);
        assert_eq!(recording.markers[0].label, "mid");
    }

    #[test]
    fn test_out_of_order_multiple_events_sorted() {
        // V2 file with all events out of order: 5.0, 1.0, 3.0.
        let input = r#"{"version": 2, "width": 80, "height": 24}
[5.0, "o", "c"]
[1.0, "o", "a"]
[3.0, "o", "b"]
"#;
        let recording = parse(Cursor::new(input)).unwrap();
        assert_eq!(recording.events.len(), 3, "expected 3 events");
        let timestamps: Vec<f64> = recording.events.iter().map(|e| e.time).collect();
        assert_eq!(timestamps, vec![1.0, 3.0, 5.0], "events should be sorted");
    }

    #[test]
    fn test_out_of_order_emits_warning() {
        // Same setup as test_out_of_order_marker_accepted: the out-of-order
        // marker should trigger a warning but still be accepted.
        let input = r#"{"version": 2, "width": 80, "height": 24}
[1.0, "o", "a"]
[3.0, "o", "b"]
[8.0, "o", "c"]
[5.0, "m", "mid"]
"#;
        let recording = parse(Cursor::new(input)).unwrap();
        // A warning must be emitted mentioning "non-decreasing".
        assert!(
            !recording.warnings.is_empty(),
            "expected at least one warning for out-of-order event"
        );
        assert!(
            recording
                .warnings
                .iter()
                .any(|w| w.message.contains("non-decreasing")),
            "expected warning containing 'non-decreasing', got: {:?}",
            recording.warnings
        );
    }

    #[test]
    fn test_stable_sort_preserves_file_order() {
        // Two output events with the same timestamp: file order must be preserved.
        let input = r#"{"version": 2, "width": 80, "height": 24}
[1.0, "o", "first"]
[1.0, "o", "second"]
"#;
        let recording = parse(Cursor::new(input)).unwrap();
        assert_eq!(recording.events.len(), 2, "expected 2 events");
        match &recording.events[0].data {
            EventData::Text(s) => assert_eq!(s, "first", "first event should have data 'first'"),
            _ => panic!("expected Text data"),
        }
        match &recording.events[1].data {
            EventData::Text(s) => assert_eq!(s, "second", "second event should have data 'second'"),
            _ => panic!("expected Text data"),
        }
    }

    #[test]
    fn test_v3_unaffected_by_sort() {
        // V3 file with relative timestamps 0.5, 0.7, 0.8.
        // Accumulated absolute times: 0.5, 1.2, 2.0.
        let input = r#"{"version": 3, "term": {"cols": 80, "rows": 24}}
[0.5, "o", "a"]
[0.7, "o", "b"]
[0.8, "o", "c"]
"#;
        let recording = parse(Cursor::new(input)).unwrap();
        assert_eq!(recording.events.len(), 3, "expected 3 events");
        assert!(
            recording.warnings.is_empty(),
            "expected no warnings for v3 file"
        );
        let expected = [0.5_f64, 1.2, 2.0];
        for (i, &exp) in expected.iter().enumerate() {
            assert!(
                (recording.events[i].time - exp).abs() < 1e-9,
                "event[{i}] time: expected {exp}, got {}",
                recording.events[i].time
            );
        }
    }

    // -----------------------------------------------------------------------
    // serialize_marker_event() tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_serialize_marker_basic() {
        let line = serialize_marker_event(3.0, "chapter-1");
        assert_eq!(line, r#"[3.0,"m","chapter-1"]"#);
    }

    #[test]
    fn test_serialize_marker_empty_label() {
        let line = serialize_marker_event(1.5, "");
        assert_eq!(line, r#"[1.5,"m",""]"#);
    }

    #[test]
    fn test_serialize_marker_special_chars() {
        // Labels with quotes and backslashes should be JSON-escaped.
        let line = serialize_marker_event(2.0, r#"test"quote"#);
        assert_eq!(line, r#"[2.0,"m","test\"quote"]"#);

        let line = serialize_marker_event(2.0, r"back\slash");
        assert_eq!(line, r#"[2.0,"m","back\\slash"]"#);
    }

    #[test]
    fn test_serialize_marker_unicode() {
        let line = serialize_marker_event(1.0, "héllo wörld");
        assert_eq!(line, r#"[1.0,"m","héllo wörld"]"#);
    }

    #[test]
    fn test_serialize_marker_float_noise_rounding() {
        // 3.5000000000000004 should round to 3.5
        let line = serialize_marker_event(3.5000000000000004, "x");
        assert_eq!(line, r#"[3.5,"m","x"]"#);
    }

    // -----------------------------------------------------------------------
    // rewrite_v3_with_marker() tests
    // -----------------------------------------------------------------------

    /// Helper: standard v3 header line.
    const V3_HEADER: &str = r#"{"version":3,"term":{"cols":80,"rows":24}}"#;

    #[test]
    fn test_rewrite_empty_input() {
        let err = rewrite_v3_with_marker("", 1.0, "x").unwrap_err();
        assert!(matches!(err, RewriteError::EmptyFile));
        assert_eq!(err.to_string(), "input has no header line");
    }

    #[test]
    fn test_rewrite_not_v3_version2() {
        let input = "{\"version\":2,\"width\":80,\"height\":24}\n[0.5,\"o\",\"hi\"]\n";
        let err = rewrite_v3_with_marker(input, 0.3, "x").unwrap_err();
        assert!(matches!(err, RewriteError::NotV3));
        assert_eq!(err.to_string(), "input is not a v3 asciicast file");
    }

    #[test]
    fn test_rewrite_not_v3_invalid_json_header() {
        let input = "not json\n[0.5,\"o\",\"hi\"]\n";
        let err = rewrite_v3_with_marker(input, 0.3, "x").unwrap_err();
        assert!(matches!(err, RewriteError::NotV3));
    }

    #[test]
    fn test_rewrite_marker_between_events() {
        // Events at absolute times 0.5 and 1.0. Insert marker at t=0.7.
        let input = format!("{V3_HEADER}\n[0.5,\"o\",\"a\"]\n[0.5,\"o\",\"b\"]\n");
        let output = rewrite_v3_with_marker(&input, 0.7, "mid").unwrap();

        // Verify structure: header, event1, marker, patched_event2
        let out_lines: Vec<&str> = output.lines().collect();
        assert_eq!(out_lines.len(), 4);
        assert_eq!(out_lines[0], V3_HEADER); // header unchanged
        assert_eq!(out_lines[1], "[0.5,\"o\",\"a\"]"); // first event unchanged
        assert_eq!(out_lines[2], "[0.2,\"m\",\"mid\"]"); // marker: 0.7 - 0.5 = 0.2
        assert_eq!(out_lines[3], "[0.3,\"o\",\"b\"]"); // patched: 1.0 - 0.7 = 0.3
    }

    #[test]
    fn test_rewrite_marker_at_t0() {
        // First event at abs time 0.5. Insert marker at t=0.
        let input = format!("{V3_HEADER}\n[0.5,\"o\",\"a\"]\n[0.5,\"o\",\"b\"]\n");
        let output = rewrite_v3_with_marker(&input, 0.0, "start").unwrap();

        let out_lines: Vec<&str> = output.lines().collect();
        assert_eq!(out_lines.len(), 4);
        assert_eq!(out_lines[1], "[0.0,\"m\",\"start\"]"); // marker: 0.0 - 0.0 = 0.0
        assert_eq!(out_lines[2], "[0.5,\"o\",\"a\"]"); // patched: 0.5 - 0.0 = 0.5 (unchanged)
    }

    #[test]
    fn test_rewrite_marker_at_exact_event_time() {
        // Events at absolute times 0.5, 1.2, 2.0.
        // Insert marker at t=1.2 → should be inserted AFTER the event at 1.2
        // (first line with accumulated > 1.2 is the one at 2.0).
        let input =
            format!("{V3_HEADER}\n[0.5,\"o\",\"a\"]\n[0.7,\"o\",\"b\"]\n[0.8,\"o\",\"c\"]\n");
        let output = rewrite_v3_with_marker(&input, 1.2, "exact").unwrap();

        let out_lines: Vec<&str> = output.lines().collect();
        assert_eq!(out_lines.len(), 5);
        assert_eq!(out_lines[1], "[0.5,\"o\",\"a\"]"); // unchanged
        assert_eq!(out_lines[2], "[0.7,\"o\",\"b\"]"); // unchanged (at t=1.2)
        assert_eq!(out_lines[3], "[0.0,\"m\",\"exact\"]"); // marker: 1.2 - 1.2 = 0.0
        assert_eq!(out_lines[4], "[0.8,\"o\",\"c\"]"); // patched: 2.0 - 1.2 = 0.8 (unchanged)
    }

    #[test]
    fn test_rewrite_marker_after_last_event() {
        // Events end at absolute time 1.0. Insert marker at t=5.0 → append.
        let input = format!("{V3_HEADER}\n[0.5,\"o\",\"a\"]\n[0.5,\"o\",\"b\"]\n");
        let output = rewrite_v3_with_marker(&input, 5.0, "end").unwrap();

        let out_lines: Vec<&str> = output.lines().collect();
        assert_eq!(out_lines.len(), 4);
        assert_eq!(out_lines[1], "[0.5,\"o\",\"a\"]"); // unchanged
        assert_eq!(out_lines[2], "[0.5,\"o\",\"b\"]"); // unchanged
        assert_eq!(out_lines[3], "[4.0,\"m\",\"end\"]"); // delta: 5.0 - 1.0 = 4.0
    }

    #[test]
    fn test_rewrite_header_only() {
        // Header with no events. Insert marker at t=2.5 → append.
        let input = format!("{V3_HEADER}\n");
        let output = rewrite_v3_with_marker(&input, 2.5, "solo").unwrap();

        let out_lines: Vec<&str> = output.lines().collect();
        assert_eq!(out_lines.len(), 2);
        assert_eq!(out_lines[0], V3_HEADER);
        assert_eq!(out_lines[1], "[2.5,\"m\",\"solo\"]"); // delta: 2.5 - 0.0 = 2.5
    }

    #[test]
    fn test_rewrite_preserves_blank_and_malformed_lines() {
        // Blank lines and malformed JSON between valid events.
        let input = format!("{V3_HEADER}\n[0.5,\"o\",\"a\"]\n\nnot json\n[0.5,\"o\",\"b\"]\n");
        let output = rewrite_v3_with_marker(&input, 0.7, "m1").unwrap();

        let out_lines: Vec<&str> = output.lines().collect();
        // Header, event1, blank, "not json", marker, patched_event2
        assert_eq!(out_lines.len(), 6);
        assert_eq!(out_lines[0], V3_HEADER);
        assert_eq!(out_lines[1], "[0.5,\"o\",\"a\"]");
        assert_eq!(out_lines[2], ""); // blank line preserved
        assert_eq!(out_lines[3], "not json"); // malformed line preserved
        assert_eq!(out_lines[4], "[0.2,\"m\",\"m1\"]"); // marker before event2
        assert_eq!(out_lines[5], "[0.3,\"o\",\"b\"]"); // patched event2
    }

    #[test]
    fn test_rewrite_unknown_event_types_advance_time() {
        // Unknown type "x" at delta 0.5 → abs 0.5. Output at delta 0.5 → abs 1.0.
        // Insert marker at t=0.7 → between the "x" line and the output line.
        let input = format!("{V3_HEADER}\n[0.5,\"x\",\"unk\"]\n[0.5,\"o\",\"b\"]\n");
        let output = rewrite_v3_with_marker(&input, 0.7, "mid").unwrap();

        let out_lines: Vec<&str> = output.lines().collect();
        assert_eq!(out_lines.len(), 4);
        assert_eq!(out_lines[1], "[0.5,\"x\",\"unk\"]"); // unchanged
        assert_eq!(out_lines[2], "[0.2,\"m\",\"mid\"]"); // marker: 0.7 - 0.5 = 0.2
        assert_eq!(out_lines[3], "[0.3,\"o\",\"b\"]"); // patched: 1.0 - 0.7 = 0.3
    }

    #[test]
    fn test_rewrite_timestamp_integer() {
        // Line with integer timestamp: [1,"o","a"]
        let input = format!("{V3_HEADER}\n[1,\"o\",\"a\"]\n[1,\"o\",\"b\"]\n");
        let output = rewrite_v3_with_marker(&input, 1.5, "mid").unwrap();

        let out_lines: Vec<&str> = output.lines().collect();
        assert_eq!(out_lines[2], "[0.5,\"m\",\"mid\"]"); // marker: 1.5 - 1.0 = 0.5
        assert_eq!(out_lines[3], "[0.5,\"o\",\"b\"]"); // patched: 2.0 - 1.5 = 0.5
    }

    #[test]
    fn test_rewrite_timestamp_leading_whitespace() {
        // Line with leading whitespace in timestamp: [ 0.5,"o","a"]
        let input = format!("{V3_HEADER}\n[ 0.5,\"o\",\"a\"]\n[0.5,\"o\",\"b\"]\n");
        let output = rewrite_v3_with_marker(&input, 0.3, "pre").unwrap();

        let out_lines: Vec<&str> = output.lines().collect();
        assert_eq!(out_lines.len(), 4);
        // Marker inserted before the first event
        assert_eq!(out_lines[1], "[0.3,\"m\",\"pre\"]");
        // Patched first event: original accumulated 0.5, marker at 0.3, new delta = 0.2
        assert_eq!(out_lines[2], "[0.2,\"o\",\"a\"]");
        // Second event unchanged
        assert_eq!(out_lines[3], "[0.5,\"o\",\"b\"]");
    }

    #[test]
    fn test_rewrite_float_precision_six_decimals() {
        // Verify 6-decimal rounding: 1/3 → 0.333333
        let input = format!("{V3_HEADER}\n[0.5,\"o\",\"a\"]\n[0.5,\"o\",\"b\"]\n");
        let output = rewrite_v3_with_marker(&input, 1.0 / 3.0, "third").unwrap();

        let out_lines: Vec<&str> = output.lines().collect();
        // marker delta = 1/3 - 0 ≈ 0.333333
        assert_eq!(out_lines[1], "[0.333333,\"m\",\"third\"]");
        // patched delta = 0.5 - 1/3 ≈ 0.166667
        assert_eq!(out_lines[2], "[0.166667,\"o\",\"a\"]");
    }

    #[test]
    fn test_rewrite_special_chars_in_label() {
        let input = format!("{V3_HEADER}\n[1.0,\"o\",\"a\"]\n");
        // Label with quotes, backslashes, and unicode
        let output = rewrite_v3_with_marker(&input, 2.0, "test\"quote\\slash 🎉").unwrap();

        let out_lines: Vec<&str> = output.lines().collect();
        // serde_json handles escaping: " → \", \ → \\
        assert_eq!(out_lines[2], r#"[1.0,"m","test\"quote\\slash 🎉"]"#);
    }

    #[test]
    fn test_rewrite_around_existing_markers() {
        // Existing markers in the file are just timestamped lines for delta purposes.
        // Event at 0.5, marker at 1.0, event at 1.5. Insert new marker at 0.7.
        let input =
            format!("{V3_HEADER}\n[0.5,\"o\",\"a\"]\n[0.5,\"m\",\"old\"]\n[0.5,\"o\",\"b\"]\n");
        let output = rewrite_v3_with_marker(&input, 0.7, "new").unwrap();

        let out_lines: Vec<&str> = output.lines().collect();
        assert_eq!(out_lines.len(), 5);
        assert_eq!(out_lines[1], "[0.5,\"o\",\"a\"]"); // unchanged
        assert_eq!(out_lines[2], "[0.2,\"m\",\"new\"]"); // new marker: 0.7 - 0.5 = 0.2
        assert_eq!(out_lines[3], "[0.3,\"m\",\"old\"]"); // patched old marker: 1.0 - 0.7 = 0.3
        assert_eq!(out_lines[4], "[0.5,\"o\",\"b\"]"); // unchanged
    }

    #[test]
    fn test_rewrite_no_trailing_newline() {
        // Input without trailing newline should produce output without trailing newline.
        let input = format!("{V3_HEADER}\n[0.5,\"o\",\"a\"]\n[0.5,\"o\",\"b\"]");
        assert!(!input.ends_with('\n'));

        let output = rewrite_v3_with_marker(&input, 0.7, "mid").unwrap();
        assert!(!output.ends_with('\n'));

        let out_lines: Vec<&str> = output.lines().collect();
        assert_eq!(out_lines.len(), 4);
        assert_eq!(out_lines[2], "[0.2,\"m\",\"mid\"]");
    }

    #[test]
    fn test_rewrite_with_trailing_newline() {
        // Input with trailing newline should produce output with trailing newline.
        let input = format!("{V3_HEADER}\n[0.5,\"o\",\"a\"]\n[0.5,\"o\",\"b\"]\n");
        assert!(input.ends_with('\n'));

        let output = rewrite_v3_with_marker(&input, 0.7, "mid").unwrap();
        assert!(output.ends_with('\n'));
    }

    #[test]
    fn test_rewrite_round_trip() {
        // Full round-trip: parse(rewrite(input, t, l)) should produce the same
        // events as parse(input) plus the marker at the correct absolute time.
        let input =
            format!("{V3_HEADER}\n[0.5,\"o\",\"a\"]\n[0.7,\"o\",\"b\"]\n[0.8,\"o\",\"c\"]\n");

        let original = parse(Cursor::new(&input)).unwrap();
        assert_eq!(original.events.len(), 3);
        assert_eq!(original.markers.len(), 0);

        let rewritten = rewrite_v3_with_marker(&input, 1.0, "mark").unwrap();
        let reparsed = parse(Cursor::new(&rewritten)).unwrap();

        // Should have 4 events: 3 original + 1 marker
        assert_eq!(reparsed.events.len(), 4);
        assert_eq!(reparsed.markers.len(), 1);

        // Original events preserved at same absolute times
        let orig_times: Vec<f64> = original.events.iter().map(|e| e.time).collect();
        let new_non_marker: Vec<f64> = reparsed
            .events
            .iter()
            .filter(|e| e.event_type != EventType::Marker)
            .map(|e| e.time)
            .collect();
        for (a, b) in orig_times.iter().zip(new_non_marker.iter()) {
            assert!(
                (a - b).abs() < 1e-9,
                "original event time {a} != reparsed time {b}"
            );
        }

        // Marker at the correct absolute time
        assert!((reparsed.markers[0].time - 1.0).abs() < 1e-9);
        assert_eq!(reparsed.markers[0].label, "mark");
    }

    #[test]
    fn test_rewrite_round_trip_marker_after_last() {
        // Round-trip when marker is appended after the last event.
        let input = format!("{V3_HEADER}\n[0.5,\"o\",\"a\"]\n[0.5,\"o\",\"b\"]\n");

        let original = parse(Cursor::new(&input)).unwrap();
        let rewritten = rewrite_v3_with_marker(&input, 5.0, "end").unwrap();
        let reparsed = parse(Cursor::new(&rewritten)).unwrap();

        assert_eq!(reparsed.events.len(), 3);
        assert_eq!(reparsed.markers.len(), 1);
        assert!((reparsed.markers[0].time - 5.0).abs() < 1e-9);

        // Original events preserved
        for (orig, reparsed_evt) in original.events.iter().zip(
            reparsed
                .events
                .iter()
                .filter(|e| e.event_type != EventType::Marker),
        ) {
            assert!(
                (orig.time - reparsed_evt.time).abs() < 1e-9,
                "times differ: {} vs {}",
                orig.time,
                reparsed_evt.time
            );
        }
    }

    #[test]
    fn test_rewrite_byte_identical_preservation() {
        // Only the marker line and the immediately-following timestamped line
        // should differ; all other lines must be byte-identical.
        let input =
            format!("{V3_HEADER}\n[0.5,\"o\",\"a\"]\n[0.5,\"o\",\"b\"]\n[0.5,\"o\",\"c\"]\n");
        let output = rewrite_v3_with_marker(&input, 0.7, "mid").unwrap();

        let in_lines: Vec<&str> = input.lines().collect();
        let out_lines: Vec<&str> = output.lines().collect();

        // Output has one extra line (the marker)
        assert_eq!(out_lines.len(), in_lines.len() + 1);

        // Header unchanged
        assert_eq!(out_lines[0], in_lines[0]);
        // First event unchanged
        assert_eq!(out_lines[1], in_lines[1]);
        // out_lines[2] = marker (new)
        // out_lines[3] = patched second event (modified)
        // out_lines[4] = third event — must be byte-identical to in_lines[3]
        assert_eq!(out_lines[4], in_lines[3]);
    }

    #[test]
    fn test_rewrite_malformed_between_events_with_unknown_types() {
        // Mix of blank lines, malformed JSON, and unknown event types.
        // Unknown type "z" at delta 0.3 advances time. Malformed/blank do not.
        let input = format!(
            "{V3_HEADER}\n[0.5,\"o\",\"a\"]\n\n{{bad}}\n[0.3,\"z\",\"unk\"]\n[0.2,\"o\",\"b\"]\n"
        );
        // Absolute times: event1=0.5, z=0.8, event2=1.0
        // Insert marker at t=0.9 → between z (0.8) and event2 (1.0)
        let output = rewrite_v3_with_marker(&input, 0.9, "m1").unwrap();

        let out_lines: Vec<&str> = output.lines().collect();
        assert_eq!(out_lines.len(), 7);
        assert_eq!(out_lines[0], V3_HEADER);
        assert_eq!(out_lines[1], "[0.5,\"o\",\"a\"]");
        assert_eq!(out_lines[2], ""); // blank preserved
        assert_eq!(out_lines[3], "{bad}"); // malformed preserved
        assert_eq!(out_lines[4], "[0.3,\"z\",\"unk\"]"); // unknown type preserved
        assert_eq!(out_lines[5], "[0.1,\"m\",\"m1\"]"); // marker: 0.9 - 0.8 = 0.1
        assert_eq!(out_lines[6], "[0.1,\"o\",\"b\"]"); // patched: 1.0 - 0.9 = 0.1
    }

    #[test]
    fn test_rewrite_negative_delta_line_ignored() {
        // A line with a negative delta does not advance absolute_time and is
        // passed through unchanged.
        let input =
            format!("{V3_HEADER}\n[0.5,\"o\",\"a\"]\n[-0.1,\"o\",\"neg\"]\n[0.5,\"o\",\"b\"]\n");
        // abs: event1=0.5, neg line ignored (not timestamped), event2=0.5+0.5=1.0
        // Insert at t=0.7 → before event2
        let output = rewrite_v3_with_marker(&input, 0.7, "mid").unwrap();

        let out_lines: Vec<&str> = output.lines().collect();
        assert_eq!(out_lines.len(), 5);
        assert_eq!(out_lines[1], "[0.5,\"o\",\"a\"]");
        assert_eq!(out_lines[2], "[-0.1,\"o\",\"neg\"]"); // preserved unchanged
        assert_eq!(out_lines[3], "[0.2,\"m\",\"mid\"]"); // marker: 0.7 - 0.5 = 0.2
        assert_eq!(out_lines[4], "[0.3,\"o\",\"b\"]"); // patched: 1.0 - 0.7 = 0.3
    }

    #[test]
    fn test_rewrite_two_element_array_not_timestamped() {
        // A 2-element array is not a valid timestamped line (needs 3 elements).
        let input = format!("{V3_HEADER}\n[0.5,\"o\",\"a\"]\n[0.5,\"o\"]\n[0.5,\"o\",\"b\"]\n");
        // abs: event1=0.5, [0.5,"o"] ignored, event2=0.5+0.5=1.0
        let output = rewrite_v3_with_marker(&input, 0.7, "mid").unwrap();

        let out_lines: Vec<&str> = output.lines().collect();
        assert_eq!(out_lines.len(), 5);
        assert_eq!(out_lines[2], "[0.5,\"o\"]"); // preserved unchanged
        assert_eq!(out_lines[3], "[0.2,\"m\",\"mid\"]");
        assert_eq!(out_lines[4], "[0.3,\"o\",\"b\"]");
    }
}
