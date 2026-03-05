// speedrun-core library - Terminal session player core engine

pub mod parser;

pub use parser::{Event, EventData, EventType, Header, Marker, ParseError, Recording, parse};
