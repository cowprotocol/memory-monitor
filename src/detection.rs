use std::time::Instant;

/// The type of memory anomaly detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectionMode {
    Spike,
    SlowLeak,
}

impl DetectionMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            DetectionMode::Spike => "spike",
            DetectionMode::SlowLeak => "slow-leak",
        }
    }
}

/// Result of a detection check: which mode triggered, and the baseline value
/// to include in notifications.
pub struct Detection {
    pub mode: DetectionMode,
    /// The reference memory value for the notification (P95 for spike, baseline P50 for slow-leak).
    pub baseline_for_notification: u64,
}

/// Tracks cooldown state and baseline for the dual detection system.
pub struct Detector {
    pub baseline_p50: u64,
    last_dump_time: Option<Instant>,
    last_spike_dump_time: Option<Instant>,
    dump_cooldown_secs: u64,
    spike_cooldown_secs: u64,
    spike_multiplier: u64,
    memory_change_threshold: u64,
}

impl Detector {
    pub fn new(
        dump_cooldown_secs: u64,
        spike_cooldown_secs: u64,
        spike_multiplier: u64,
        memory_change_threshold: u64,
    ) -> Self {
        Self {
            baseline_p50: 0,
            last_dump_time: None,
            last_spike_dump_time: None,
            dump_cooldown_secs,
            spike_cooldown_secs,
            spike_multiplier,
            memory_change_threshold,
        }
    }

    /// Check whether a spike or slow leak is detected.
    /// Returns `None` if neither condition is met.
    pub fn check(
        &self,
        current_usage: u64,
        current_p50: u64,
        current_p95: u64,
    ) -> Option<Detection> {
        // Spike detection: instantaneous memory > current P95 * multiplier
        if current_usage > current_p95.saturating_mul(self.spike_multiplier) {
            return Some(Detection {
                mode: DetectionMode::Spike,
                baseline_for_notification: current_p95,
            });
        }

        // Slow leak detection: current P50 > baseline P50 + threshold
        if current_p50
            > self
                .baseline_p50
                .saturating_add(self.memory_change_threshold)
        {
            return Some(Detection {
                mode: DetectionMode::SlowLeak,
                baseline_for_notification: self.baseline_p50,
            });
        }

        None
    }

    /// Whether the cooldown for the given mode has elapsed.
    pub fn cooldown_passed(&self, mode: DetectionMode) -> bool {
        let (last_time, cooldown_secs) = match mode {
            DetectionMode::Spike => (self.last_spike_dump_time, self.spike_cooldown_secs),
            DetectionMode::SlowLeak => (self.last_dump_time, self.dump_cooldown_secs),
        };

        match last_time {
            None => true,
            Some(t) => t.elapsed().as_secs() >= cooldown_secs,
        }
    }

    /// Record that a successful dump was made. Resets baseline P50.
    pub fn record_dump(&mut self, mode: DetectionMode, new_baseline_p50: u64) {
        let now = Instant::now();
        self.last_dump_time = Some(now);
        if mode == DetectionMode::Spike {
            self.last_spike_dump_time = Some(now);
        }
        self.baseline_p50 = new_baseline_p50;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_detector() -> Detector {
        Detector::new(
            60,       // dump_cooldown_secs
            600,      // spike_cooldown_secs
            3,        // spike_multiplier
            200 * MB, // memory_change_threshold
        )
    }

    const MB: u64 = 1024 * 1024;

    #[test]
    fn test_no_detection() {
        let mut det = make_detector();
        det.baseline_p50 = 500 * MB;

        // Usage within normal range
        let result = det.check(510 * MB, 505 * MB, 520 * MB);
        assert!(result.is_none());
    }

    #[test]
    fn test_spike_detection() {
        let mut det = make_detector();
        det.baseline_p50 = 500 * MB;

        let current_p95 = 520 * MB;
        let spike_usage = current_p95 * 3 + 1; // just above threshold

        let result = det.check(spike_usage, 505 * MB, current_p95);
        assert!(result.is_some());
        let detection = result.unwrap();
        assert_eq!(detection.mode, DetectionMode::Spike);
        assert_eq!(detection.baseline_for_notification, current_p95);
    }

    #[test]
    fn test_spike_at_boundary() {
        let mut det = make_detector();
        det.baseline_p50 = 500 * MB;

        let current_p95 = 520 * MB;
        let at_boundary = current_p95 * 3; // exactly at threshold, not above

        let result = det.check(at_boundary, 505 * MB, current_p95);
        assert!(result.is_none());
    }

    #[test]
    fn test_slow_leak_detection() {
        let mut det = make_detector();
        det.baseline_p50 = 500 * MB;

        // P50 exceeds baseline + threshold
        let new_p50 = 500 * MB + 200 * MB + 1;
        let result = det.check(510 * MB, new_p50, 520 * MB);
        assert!(result.is_some());
        let detection = result.unwrap();
        assert_eq!(detection.mode, DetectionMode::SlowLeak);
        assert_eq!(detection.baseline_for_notification, 500 * MB);
    }

    #[test]
    fn test_slow_leak_at_boundary() {
        let mut det = make_detector();
        det.baseline_p50 = 500 * MB;

        // P50 exactly at threshold, not above
        let new_p50 = 500 * MB + 200 * MB;
        let result = det.check(510 * MB, new_p50, 520 * MB);
        assert!(result.is_none());
    }

    #[test]
    fn test_spike_takes_priority_over_slow_leak() {
        let mut det = make_detector();
        det.baseline_p50 = 100 * MB;

        let current_p95 = 120 * MB;
        // Both conditions met: spike and slow leak
        let spike_usage = current_p95 * 3 + 1;
        let new_p50 = 100 * MB + 200 * MB + 1;

        let result = det.check(spike_usage, new_p50, current_p95);
        assert!(result.is_some());
        assert_eq!(result.unwrap().mode, DetectionMode::Spike);
    }

    #[test]
    fn test_cooldown_initially_passed() {
        let det = make_detector();
        assert!(det.cooldown_passed(DetectionMode::Spike));
        assert!(det.cooldown_passed(DetectionMode::SlowLeak));
    }

    #[test]
    fn test_record_dump_updates_baseline() {
        let mut det = make_detector();
        det.baseline_p50 = 500 * MB;

        det.record_dump(DetectionMode::SlowLeak, 700 * MB);
        assert_eq!(det.baseline_p50, 700 * MB);

        // Cooldown should not have passed yet (just recorded)
        assert!(!det.cooldown_passed(DetectionMode::SlowLeak));
    }

    #[test]
    fn test_spike_dump_sets_both_timestamps() {
        let mut det = make_detector();
        det.record_dump(DetectionMode::Spike, 500 * MB);

        // Both cooldowns should be active
        assert!(!det.cooldown_passed(DetectionMode::Spike));
        assert!(!det.cooldown_passed(DetectionMode::SlowLeak));
    }

    #[test]
    fn test_slow_leak_dump_does_not_set_spike_timestamp() {
        let mut det = make_detector();
        det.record_dump(DetectionMode::SlowLeak, 500 * MB);

        // Spike cooldown should still be passed (no spike recorded)
        assert!(det.cooldown_passed(DetectionMode::Spike));
        // Slow-leak cooldown should not have passed
        assert!(!det.cooldown_passed(DetectionMode::SlowLeak));
    }
}
