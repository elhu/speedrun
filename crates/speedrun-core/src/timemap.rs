use std::fmt;

/// Mapping from raw timestamps to effective (idle-capped) timestamps.
/// Computed once at load time. One entry per event, parallel to the events vec.
#[derive(Debug)]
pub struct TimeMap {
    effective_times: Vec<f64>,
    duration: f64,
}

/// Errors that can occur when building a `TimeMap`.
#[derive(Debug)]
pub enum TimeMapError {
    /// Idle limit must be positive.
    InvalidIdleLimit(f64),
}

impl fmt::Display for TimeMapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TimeMapError::InvalidIdleLimit(limit) => {
                write!(f, "idle limit must be positive, got {limit}")
            }
        }
    }
}

impl std::error::Error for TimeMapError {}

impl TimeMap {
    /// Build a time map from raw event timestamps.
    ///
    /// `idle_limit`: if `Some(limit)`, inter-event gaps exceeding `limit`
    /// seconds are capped to `limit`. If `None`, effective = raw.
    pub fn build(raw_times: &[f64], idle_limit: Option<f64>) -> Result<Self, TimeMapError> {
        if let Some(limit) = idle_limit
            && limit <= 0.0
        {
            return Err(TimeMapError::InvalidIdleLimit(limit));
        }

        if raw_times.is_empty() {
            return Ok(TimeMap {
                effective_times: vec![],
                duration: 0.0,
            });
        }

        let mut effective_times = Vec::with_capacity(raw_times.len());
        effective_times.push(raw_times[0]);

        for i in 1..raw_times.len() {
            let gap = raw_times[i] - raw_times[i - 1];
            let capped_gap = match idle_limit {
                Some(limit) => gap.min(limit),
                None => gap,
            };
            effective_times.push(effective_times[i - 1] + capped_gap);
        }

        let duration = *effective_times.last().unwrap_or(&0.0);

        Ok(TimeMap {
            effective_times,
            duration,
        })
    }

    /// Effective timestamp for event at index. None if out of bounds.
    pub fn effective_time(&self, index: usize) -> Option<f64> {
        self.effective_times.get(index).copied()
    }

    /// Total effective duration (effective time of last event, or 0.0 if empty).
    pub fn duration(&self) -> f64 {
        self.duration
    }

    /// Number of entries (same as number of events).
    pub fn len(&self) -> usize {
        self.effective_times.len()
    }

    /// Returns true if the time map has no entries.
    pub fn is_empty(&self) -> bool {
        self.effective_times.is_empty()
    }

    /// Find the last event at or before `time` in effective time.
    /// Returns None if the time map is empty or time is before the first event.
    pub fn event_index_at(&self, time: f64) -> Option<usize> {
        if self.effective_times.is_empty() {
            return None;
        }

        let count = self.effective_times.partition_point(|&t| t <= time);
        if count == 0 { None } else { Some(count - 1) }
    }
}
