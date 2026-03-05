// speedrun-core library - Terminal session player core engine

pub mod parser;
pub mod snapshot;
pub mod timemap;

pub use parser::{parse, Event, EventData, EventType, Header, Marker, ParseError, Recording};
pub use snapshot::{create_vt, CursorState, TerminalSnapshot};
pub use timemap::{TimeMap, TimeMapError};
