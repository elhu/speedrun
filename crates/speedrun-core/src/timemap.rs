//! Raw-to-effective time mapping with idle capping.
//!
//! When a recording contains long idle pauses, the [`TimeMap`] compresses
//! them by capping inter-event gaps to a configurable limit. This produces
//! a parallel array of "effective" timestamps that preserves event ordering
//! while making playback duration manageable.

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

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    const EPSILON: f64 = 1e-10;

    fn assert_times_eq(actual: &[f64], expected: &[f64]) {
        assert_eq!(actual.len(), expected.len(), "length mismatch");
        for (i, (a, e)) in actual.iter().zip(expected).enumerate() {
            assert!((a - e).abs() < EPSILON, "index {i}: got {a}, expected {e}");
        }
    }

    fn assert_f64_eq(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < EPSILON,
            "got {actual}, expected {expected}"
        );
    }

    // Test 1: Idle limit capping — three scenarios with same raw times
    #[test]
    fn idle_limit_capping() {
        let raw = &[1.0, 1.1, 35.0, 35.1, 36.0];

        // limit = 2.0
        let tm = TimeMap::build(raw, Some(2.0)).unwrap();
        let effective: Vec<f64> = (0..tm.len())
            .map(|i| tm.effective_time(i).unwrap())
            .collect();
        assert_times_eq(&effective, &[1.0, 1.1, 3.1, 3.2, 4.1]);
        assert_f64_eq(tm.duration(), 4.1);

        // limit = 1.0
        let tm = TimeMap::build(raw, Some(1.0)).unwrap();
        let effective: Vec<f64> = (0..tm.len())
            .map(|i| tm.effective_time(i).unwrap())
            .collect();
        assert_times_eq(&effective, &[1.0, 1.1, 2.1, 2.2, 3.1]);
        assert_f64_eq(tm.duration(), 3.1);

        // no limit
        let tm = TimeMap::build(raw, None).unwrap();
        let effective: Vec<f64> = (0..tm.len())
            .map(|i| tm.effective_time(i).unwrap())
            .collect();
        assert_eq!(effective, vec![1.0, 1.1, 35.0, 35.1, 36.0]);
        assert_eq!(tm.duration(), 36.0);
    }

    // Test 2: Empty event list
    #[test]
    fn empty_event_list() {
        let tm = TimeMap::build(&[], Some(2.0)).unwrap();
        assert_eq!(tm.duration(), 0.0);
        assert_eq!(tm.len(), 0);
        assert!(tm.is_empty());
    }

    // Test 3: Single event
    #[test]
    fn single_event() {
        let tm = TimeMap::build(&[5.0], Some(2.0)).unwrap();
        assert_eq!(tm.effective_time(0), Some(5.0));
        assert_eq!(tm.duration(), 5.0);
    }

    // Test 4: Multiple consecutive idle gaps
    #[test]
    fn multiple_consecutive_idle_gaps() {
        let raw = &[0.0, 1.0, 20.0, 21.0, 40.0];
        let tm = TimeMap::build(raw, Some(2.0)).unwrap();
        let effective: Vec<f64> = (0..tm.len())
            .map(|i| tm.effective_time(i).unwrap())
            .collect();
        assert_eq!(effective, vec![0.0, 1.0, 3.0, 4.0, 6.0]);
        assert_eq!(tm.duration(), 6.0);
    }

    // Test 5: First event at t>0
    #[test]
    fn first_event_at_nonzero_time() {
        let raw = &[5.0, 6.0, 7.0];
        let tm = TimeMap::build(raw, Some(2.0)).unwrap();
        let effective: Vec<f64> = (0..tm.len())
            .map(|i| tm.effective_time(i).unwrap())
            .collect();
        assert_eq!(effective, vec![5.0, 6.0, 7.0]);
        assert_eq!(tm.duration(), 7.0);
    }

    // Test 6: Invalid idle limits
    #[test]
    fn invalid_idle_limits() {
        let result = TimeMap::build(&[1.0], Some(0.0));
        assert!(matches!(result, Err(TimeMapError::InvalidIdleLimit(v)) if v == 0.0));

        let result = TimeMap::build(&[1.0], Some(-1.0));
        assert!(matches!(result, Err(TimeMapError::InvalidIdleLimit(v)) if v == -1.0));
    }

    // Test 7: event_index_at binary search correctness
    #[test]
    fn event_index_at_binary_search() {
        let raw = &[1.0, 1.1, 35.0, 35.1, 36.0];
        let tm = TimeMap::build(raw, Some(2.0)).unwrap();

        // Before first event
        assert_eq!(tm.event_index_at(0.5), None);
        // Exact first event
        assert_eq!(tm.event_index_at(1.0), Some(0));
        // Between first and second
        assert_eq!(tm.event_index_at(1.05), Some(0));
        // Between second and third
        assert_eq!(tm.event_index_at(2.0), Some(1));
        // Exact last event
        assert_eq!(tm.event_index_at(4.1), Some(4));
        // Past end
        assert_eq!(tm.event_index_at(100.0), Some(4));

        // Empty map
        let empty = TimeMap::build(&[], Some(2.0)).unwrap();
        assert_eq!(empty.event_index_at(0.0), None);

        // Single-event map
        let single = TimeMap::build(&[5.0], Some(2.0)).unwrap();
        assert_eq!(single.event_index_at(4.0), None);
        assert_eq!(single.event_index_at(5.0), Some(0));
    }

    // Test 8: Insta snapshot for effective times (long_idle, limit 2.0)
    #[test]
    fn snapshot_effective_times() {
        let raw = &[1.0, 1.1, 35.0, 35.1, 36.0];
        let tm = TimeMap::build(raw, Some(2.0)).unwrap();
        let times: Vec<f64> = (0..tm.len())
            .map(|i| tm.effective_time(i).unwrap())
            .collect();
        insta::assert_debug_snapshot!(times);
    }
}
