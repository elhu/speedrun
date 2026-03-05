// speedrun-core library - Terminal session player core engine

pub mod index;
pub mod parser;
pub mod player;
pub mod snapshot;
pub mod timemap;

pub use index::{KEYFRAME_INTERVAL, Keyframe, KeyframeIndex};
pub use parser::{Event, EventData, EventType, Header, Marker, ParseError, Recording, parse};
pub use player::{LoadOptions, Player, PlayerError};
pub use snapshot::{CursorState, TerminalSnapshot, create_vt};
pub use timemap::{TimeMap, TimeMapError};
