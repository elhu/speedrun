//! Keyframe index for O(log n) seek operations.
//!
//! The index replays all events through an `avt::Vt` terminal emulator,
//! capturing [`TerminalSnapshot`]s at regular effective-time intervals.
//! This allows the player to seek to any point by restoring the nearest
//! keyframe and replaying only a small number of events forward.

use crate::parser::{EventData, EventType, Recording};
use crate::snapshot::{TerminalSnapshot, create_vt};
use crate::timemap::TimeMap;

/// Interval between keyframes in effective time (seconds).
pub const KEYFRAME_INTERVAL: f64 = 5.0;

/// A snapshot of terminal state at a specific effective time.
#[derive(Debug, Clone)]
pub struct Keyframe {
    /// Effective timestamp (with idle limit applied).
    pub time: f64,
    /// Index of the first event NOT included in this snapshot.
    /// To resume playback from this keyframe, start replaying from
    /// `events[event_index]`.
    pub event_index: usize,
    /// Serialized terminal state.
    pub snapshot: TerminalSnapshot,
}

/// Index of keyframes for O(log n) seek lookup.
#[derive(Debug)]
pub struct KeyframeIndex {
    keyframes: Vec<Keyframe>,
}

impl KeyframeIndex {
    /// Build a keyframe index by replaying all events through a virtual terminal.
    ///
    /// Captures snapshots at [`KEYFRAME_INTERVAL`] intervals in effective time.
    /// Empty recordings produce an empty index.
    pub fn build(recording: &Recording, time_map: &TimeMap) -> Self {
        if recording.events.is_empty() {
            return KeyframeIndex {
                keyframes: Vec::new(),
            };
        }

        let mut vt = create_vt(
            recording.header.width as usize,
            recording.header.height as usize,
        );

        let mut keyframes = Vec::new();

        // Capture initial keyframe (time 0.0, before any events)
        keyframes.push(Keyframe {
            time: 0.0,
            event_index: 0,
            snapshot: TerminalSnapshot::capture(&vt),
        });

        let mut next_keyframe_time = KEYFRAME_INTERVAL;

        for i in 0..recording.events.len() {
            let effective_time = time_map
                .effective_time(i)
                .expect("event index must be within time_map bounds");

            // Before processing the event, check if we've crossed a keyframe boundary
            if effective_time >= next_keyframe_time {
                keyframes.push(Keyframe {
                    time: effective_time,
                    event_index: i,
                    snapshot: TerminalSnapshot::capture(&vt),
                });

                // Advance next_keyframe_time past the current event to avoid
                // double-captures. Handles large gaps spanning multiple intervals.
                while next_keyframe_time <= effective_time {
                    next_keyframe_time += KEYFRAME_INTERVAL;
                }
            }

            // Process the event through the terminal
            let event = &recording.events[i];
            match (&event.event_type, &event.data) {
                (EventType::Output, EventData::Text(data)) => {
                    let _ = vt.feed_str(data);
                }
                (EventType::Resize, EventData::Resize { cols, rows }) => {
                    // Critical: xtwinops takes rows;cols (opposite of EventData order)
                    let _ = vt.feed_str(&format!("\x1b[8;{rows};{cols}t"));
                }
                // Input and Marker events don't change terminal state
                (EventType::Input, _) | (EventType::Marker, _) => {}
                // Ignore mismatched type/data combinations
                _ => {}
            }
        }

        KeyframeIndex { keyframes }
    }

    /// Find the index of the last keyframe at or before `time`.
    ///
    /// Uses binary search for O(log n) lookup. Returns `None` if the index
    /// is empty or `time` is before the first keyframe.
    pub fn keyframe_at(&self, time: f64) -> Option<usize> {
        if self.keyframes.is_empty() {
            return None;
        }

        let count = self.keyframes.partition_point(|kf| kf.time <= time);
        if count == 0 { None } else { Some(count - 1) }
    }

    /// Get a keyframe by index.
    pub fn get(&self, index: usize) -> Option<&Keyframe> {
        self.keyframes.get(index)
    }

    /// Number of keyframes in the index.
    pub fn len(&self) -> usize {
        self.keyframes.len()
    }

    /// Returns true if the index contains no keyframes.
    pub fn is_empty(&self) -> bool {
        self.keyframes.is_empty()
    }
}
