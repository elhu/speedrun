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
