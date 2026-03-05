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
    let _ = reader;
    todo!()
}
