use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct Recording {
    pub header: Header,
    pub events: Vec<Event>,
    pub markers: Vec<Marker>, // Extracted from events for convenience
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
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::EmptyFile => write!(f, "empty file: no header line found"),
            ParseError::InvalidHeader { line, source } => {
                write!(f, "invalid header JSON: {source} (line: {line})")
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

    let reader = BufReader::new(reader);
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
    // 2. Event parsing
    // -----------------------------------------------------------------------
    let mut events = Vec::new();
    let mut markers = Vec::new();
    let mut line_number: usize = 1; // header is line 1
    let mut prev_time: f64 = 0.0;
    let mut absolute_time: f64 = 0.0;

    for line_result in lines {
        line_number += 1;

        let line = match line_result {
            Ok(l) => l,
            Err(e) if e.kind() == ErrorKind::InvalidData => {
                return Err(ParseError::NotUtf8 { source: e });
            }
            Err(e) => return Err(ParseError::Io { source: e }),
        };

        // Skip empty/whitespace-only lines
        if line.trim().is_empty() {
            continue;
        }

        let val: serde_json::Value =
            serde_json::from_str(&line).map_err(|_| ParseError::InvalidEvent {
                line_number,
                content: line.clone(),
                reason: "invalid JSON".to_string(),
            })?;

        let arr = val.as_array().ok_or_else(|| ParseError::InvalidEvent {
            line_number,
            content: line.clone(),
            reason: "expected array of 3 elements".to_string(),
        })?;

        if arr.len() != 3 {
            return Err(ParseError::InvalidEvent {
                line_number,
                content: line.clone(),
                reason: "expected array of 3 elements".to_string(),
            });
        }

        // Timestamp
        let raw_time = arr[0].as_f64().ok_or_else(|| ParseError::InvalidEvent {
            line_number,
            content: line.clone(),
            reason: "invalid timestamp".to_string(),
        })?;

        if raw_time < 0.0 {
            return Err(ParseError::InvalidEvent {
                line_number,
                content: line.clone(),
                reason: "negative timestamp".to_string(),
            });
        }

        // Compute absolute time
        let time = if version == 3 {
            // V3: timestamps are relative intervals
            absolute_time += raw_time;
            absolute_time
        } else {
            // V2: timestamps are absolute; check monotonicity
            if raw_time < prev_time {
                return Err(ParseError::InvalidEvent {
                    line_number,
                    content: line.clone(),
                    reason: "timestamps must be non-decreasing".to_string(),
                });
            }
            prev_time = raw_time;
            raw_time
        };

        // Event type
        let type_str = arr[1].as_str().ok_or_else(|| ParseError::InvalidEvent {
            line_number,
            content: line.clone(),
            reason: "invalid event type".to_string(),
        })?;

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
        let data_str = arr[2].as_str().ok_or_else(|| ParseError::InvalidEvent {
            line_number,
            content: line.clone(),
            reason: "invalid event data".to_string(),
        })?;

        // Build EventData based on type
        let data = match event_type {
            EventType::Resize => {
                let parts: Vec<&str> = data_str.split('x').collect();
                if parts.len() != 2 {
                    return Err(ParseError::InvalidResize {
                        line_number,
                        data: data_str.to_string(),
                    });
                }
                let cols: u16 = parts[0].parse().map_err(|_| ParseError::InvalidResize {
                    line_number,
                    data: data_str.to_string(),
                })?;
                let rows: u16 = parts[1].parse().map_err(|_| ParseError::InvalidResize {
                    line_number,
                    data: data_str.to_string(),
                })?;
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
        let file = std::fs::File::open(testdata_path("invalid/bad_event.cast")).unwrap();
        let err = parse(file).unwrap_err();
        match &err {
            ParseError::InvalidEvent {
                line_number,
                reason,
                ..
            } => {
                assert_eq!(
                    *line_number, 2,
                    "expected error on line 2, got {line_number}"
                );
                // Line 2 is [0.5, "o"] — only 2 elements
                assert!(
                    reason.contains("3 elements"),
                    "expected reason about 3 elements, got: {reason}"
                );
            }
            _ => panic!("expected InvalidEvent, got {err:?}"),
        }
    }

    #[test]
    fn test_invalid_binary_garbage() {
        let file = std::fs::File::open(testdata_path("invalid/binary_garbage.cast")).unwrap();
        let err = parse(file).unwrap_err();
        assert!(
            matches!(
                err,
                ParseError::NotUtf8 { .. } | ParseError::InvalidHeader { .. }
            ),
            "expected NotUtf8 or InvalidHeader, got {err:?}"
        );
    }

    #[test]
    fn test_invalid_truncated() {
        let file = std::fs::File::open(testdata_path("invalid/truncated.cast")).unwrap();
        let err = parse(file).unwrap_err();
        assert!(
            matches!(err, ParseError::InvalidEvent { .. }),
            "expected InvalidEvent, got {err:?}"
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
        let err = parse(Cursor::new(input)).unwrap_err();
        match &err {
            ParseError::InvalidEvent { reason, .. } => {
                assert!(
                    reason.contains("non-decreasing"),
                    "expected reason containing 'non-decreasing', got: {reason}"
                );
            }
            _ => panic!("expected InvalidEvent, got {err:?}"),
        }
    }

    #[test]
    fn test_v3_negative_interval() {
        let input = r#"{"version": 3, "term": {"cols": 80, "rows": 24}}
[0.5, "o", "a"]
[-0.1, "o", "b"]
[0.3, "o", "c"]
"#;
        let err = parse(Cursor::new(input)).unwrap_err();
        match &err {
            ParseError::InvalidEvent { reason, .. } => {
                assert!(
                    reason.contains("negative"),
                    "expected reason containing 'negative', got: {reason}"
                );
            }
            _ => panic!("expected InvalidEvent, got {err:?}"),
        }
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
        let err = parse(Cursor::new(input)).unwrap_err();
        assert!(
            matches!(err, ParseError::InvalidResize { .. }),
            "expected InvalidResize, got {err:?}"
        );
    }

    #[test]
    fn test_resize_bad_format_non_numeric() {
        let input = r#"{"version": 2, "width": 80, "height": 24}
[1.0, "r", "abcxdef"]
"#;
        let err = parse(Cursor::new(input)).unwrap_err();
        assert!(
            matches!(err, ParseError::InvalidResize { .. }),
            "expected InvalidResize, got {err:?}"
        );
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
