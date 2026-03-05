// speedrun-core library - Terminal session player core engine

pub mod snapshot;

pub use snapshot::{CursorState, TerminalSnapshot, create_vt};
