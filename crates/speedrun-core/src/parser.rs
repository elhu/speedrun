use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct Recording {
    pub header: Header,
    pub events: Vec<Event>,
    pub markers: Vec<Marker>, // Extracted from events for convenience
    pub warnings: Vec<ParseWarning>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Header {
    pub version: u8, // 2 or 3
    pub width: u16,
    pub height: u16,
    pub timestamp: Option<u64>,
    pub idle_time_limit: Option<f64>,
    pub title: Option<String>,
    pub env: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum EventType {
    Output,
    Input,
    Marker,
    Resize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub time: f64, // Raw absolute timestamp (seconds)
    pub event_type: EventType,
    pub data: EventData,
}

/// Typed event data. Resize is eagerly parsed into structured dimensions
/// so downstream consumers never parse "COLSxROWS" strings.
#[derive(Debug, Clone, Serialize)]
pub enum EventData {
    Text(String),
    Resize { cols: u16, rows: u16 },
}

#[derive(Debug, Clone, Serialize)]
pub struct Marker {
    pub time: f64, // Raw absolute timestamp (effective time from TimeMap)
    pub label: String,
}

/// A non-fatal warning produced during lenient parsing.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParseWarning {
    pub line_number: usize,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ParseError {
    EmptyFile,
    InvalidHeader {
        line: String,
        source: serde_json::Error,
    },
    UnsupportedVersion {
        version: u64,
    },
    MissingField {
        field: &'static str,
    },
    InvalidEvent {
        line_number: usize,
        content: String,
        reason: String,
    },
    InvalidResize {
        line_number: usize,
        data: String,
    },
    NotUtf8 {
        source: std::io::Error,
    },
    Io {
        source: std::io::Error,
    },
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
    let mut markers = Vec::new();
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
            // V2: timestamps are absolute; check monotonicity against the last
            // *valid* event's timestamp.
            if raw_time < prev_time {
                warnings.push(ParseWarning {
                    line_number,
                    message: format!(
                        "timestamps must be non-decreasing (got {raw_time} after {prev_time}): {line}"
                    ),
                });
                continue;
            }
            prev_time = raw_time;
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
            EventType::Marker => {
                markers.push(Marker {
                    time,
                    label: data_str.to_string(),
                });
                EventData::Text(data_str.to_string())
            }
            _ => EventData::Text(data_str.to_string()),
        };

        events.push(Event {
            time,
            event_type,
            data,
        });
    }

    Ok(Recording {
        header,
        events,
        markers,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn testdata_path(name: &str) -> std::path::PathBuf {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.push("../../testdata");
        p.push(name);
        p
    }

    // -----------------------------------------------------------------------
    // Valid file tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_minimal_v2() {
        let file = std::fs::File::open(testdata_path("minimal_v2.cast")).unwrap();
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
        let file = std::fs::File::open(testdata_path("minimal_v3.cast")).unwrap();
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
        let file = std::fs::File::open(testdata_path("empty.cast")).unwrap();
        let recording = parse(file).unwrap();

        assert_eq!(recording.header.version, 2);
        assert_eq!(recording.header.width, 80);
        assert_eq!(recording.header.height, 24);
        assert_eq!(recording.events.len(), 0);
        assert_eq!(recording.markers.len(), 0);
    }

    #[test]
    fn test_parse_with_markers() {
        let file = std::fs::File::open(testdata_path("with_markers.cast")).unwrap();
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
        let file = std::fs::File::open(testdata_path("with_resize.cast")).unwrap();
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
        let file = std::fs::File::open(testdata_path("long_idle.cast")).unwrap();
        let recording = parse(file).unwrap();
        assert_eq!(recording.header.idle_time_limit, Some(2.0));
        assert_eq!(recording.events.len(), 5);

        // alternate_buffer.cast
        let file = std::fs::File::open(testdata_path("alternate_buffer.cast")).unwrap();
        let recording = parse(file).unwrap();
        assert_eq!(recording.events.len(), 9);

        // real_session.cast — v3, 188x50, at least 100 events, timestamp 1772729753
        let file = std::fs::File::open(testdata_path("real_session.cast")).unwrap();
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
        let file = std::fs::File::open(testdata_path("invalid/empty_file.cast")).unwrap();
        let err = parse(file).unwrap_err();
        assert!(matches!(err, ParseError::EmptyFile));
    }

    #[test]
    fn test_invalid_no_header() {
        let file = std::fs::File::open(testdata_path("invalid/no_header.cast")).unwrap();
        let err = parse(file).unwrap_err();
        assert!(
            matches!(err, ParseError::InvalidHeader { .. }),
            "expected InvalidHeader, got {err:?}"
        );
    }

    #[test]
    fn test_invalid_bad_json() {
        let file = std::fs::File::open(testdata_path("invalid/bad_json.cast")).unwrap();
        let err = parse(file).unwrap_err();
        assert!(
            matches!(err, ParseError::InvalidHeader { .. }),
            "expected InvalidHeader, got {err:?}"
        );
    }

    #[test]
    fn test_invalid_bad_version() {
        let file = std::fs::File::open(testdata_path("invalid/bad_version.cast")).unwrap();
        let err = parse(file).unwrap_err();
        assert!(
            matches!(err, ParseError::UnsupportedVersion { version: 99 }),
            "expected UnsupportedVersion(99), got {err:?}"
        );
    }

    #[test]
    fn test_invalid_missing_fields() {
        let file = std::fs::File::open(testdata_path("invalid/missing_fields.cast")).unwrap();
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
        let file = std::fs::File::open(testdata_path("invalid/bad_event.cast")).unwrap();
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
        let file = std::fs::File::open(testdata_path("invalid/binary_garbage.cast")).unwrap();
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
        let file = std::fs::File::open(testdata_path("invalid/truncated.cast")).unwrap();
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
        // Under lenient parsing: Ok with 1 warning (line 4 skipped), 2 events
        let recording = parse(Cursor::new(input)).unwrap();
        assert_eq!(
            recording.events.len(),
            2,
            "expected 2 events (t=1.0 and t=3.0 kept, t=2.0 skipped)"
        );
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
        let file = std::fs::File::open(testdata_path("minimal_v2.cast")).unwrap();
        let recording = parse(file).unwrap();
        assert_eq!(recording.events.len(), 4, "expected 4 events");
        assert!(recording.warnings.is_empty(), "expected no warnings");
    }

    #[test]
    fn test_happy_path_regression_real_session() {
        let file = std::fs::File::open(testdata_path("real_session.cast")).unwrap();
        let recording = parse(file).unwrap();
        assert_eq!(recording.events.len(), 114, "expected 114 events");
        assert!(recording.warnings.is_empty(), "expected no warnings");
    }

    // -----------------------------------------------------------------------
    // New fixture tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_input_only() {
        let file = std::fs::File::open(testdata_path("input_only.cast")).unwrap();
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
        let file = std::fs::File::open(testdata_path("sub_second.cast")).unwrap();
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
        let file = std::fs::File::open(testdata_path("minimal_v2.cast")).unwrap();
        let recording = parse(file).unwrap();
        insta::assert_debug_snapshot!(recording);
    }
}
