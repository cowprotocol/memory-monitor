use std::collections::VecDeque;

pub struct History {
    samples: VecDeque<u64>,
    capacity: usize,
}

impl History {
    pub fn new(capacity: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Add a sample, dropping the oldest if at capacity.
    pub fn push(&mut self, value: u64) {
        if self.samples.len() >= self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back(value);
    }

    /// Current number of samples.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Whether the window is full.
    pub fn is_full(&self) -> bool {
        self.samples.len() >= self.capacity
    }

    /// Calculate the p-th percentile using nearest-rank ceiling method.
    /// Same formula as the shell script: `ceil(count * p / 100)`.
    pub fn percentile(&self, p: u64) -> u64 {
        if self.samples.is_empty() {
            return 0;
        }

        let mut sorted: Vec<u64> = self.samples.iter().copied().collect();
        sorted.sort_unstable();

        let count = sorted.len() as u64;
        // ceiling: (count * p + 99) / 100, clamped to [1, count]
        let index = (count * p).div_ceil(100).max(1).min(count);
        sorted[(index - 1) as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_and_len() {
        let mut h = History::new(3);
        assert_eq!(h.len(), 0);

        h.push(100);
        assert_eq!(h.len(), 1);

        h.push(200);
        h.push(300);
        assert_eq!(h.len(), 3);
        assert!(h.is_full());

        // Exceeding capacity drops oldest
        h.push(400);
        assert_eq!(h.len(), 3);
    }

    #[test]
    fn test_oldest_dropped() {
        let mut h = History::new(3);
        h.push(10);
        h.push(20);
        h.push(30);
        h.push(40); // drops 10

        // P0 (min) should now be 20
        assert_eq!(h.percentile(1), 20);
    }

    #[test]
    fn test_percentile_empty() {
        let h = History::new(5);
        assert_eq!(h.percentile(50), 0);
    }

    #[test]
    fn test_percentile_single() {
        let mut h = History::new(5);
        h.push(42);
        assert_eq!(h.percentile(50), 42);
        assert_eq!(h.percentile(95), 42);
    }

    #[test]
    fn test_percentile_p50() {
        let mut h = History::new(10);
        for v in 1..=10 {
            h.push(v);
        }
        // ceil(10 * 50 / 100) = ceil(5.0) = 5
        assert_eq!(h.percentile(50), 5);
    }

    #[test]
    fn test_percentile_p95() {
        let mut h = History::new(60);
        for v in 1..=60 {
            h.push(v);
        }
        // ceil(60 * 95 / 100) = ceil(57.0) = 57
        assert_eq!(h.percentile(95), 57);
    }

    #[test]
    fn test_percentile_p95_non_exact() {
        let mut h = History::new(10);
        for v in 1..=10 {
            h.push(v);
        }
        // ceil(10 * 95 / 100) = ceil(9.5) = 10
        // (10*95 + 99) / 100 = (950 + 99)/100 = 1049/100 = 10
        assert_eq!(h.percentile(95), 10);
    }

    #[test]
    fn test_percentile_unsorted_input() {
        let mut h = History::new(5);
        h.push(50);
        h.push(10);
        h.push(40);
        h.push(20);
        h.push(30);
        // sorted: [10, 20, 30, 40, 50]
        // P50: ceil(5*50/100)=ceil(2.5)=3 → index 2 → 30
        assert_eq!(h.percentile(50), 30);
    }

    #[test]
    fn test_percentile_matches_shell_formula() {
        // Replicate the shell's ceil(count * percentile / 100) = (count*p + 99)/100
        let mut h = History::new(60);
        for v in 1..=60 {
            h.push(v * 1024 * 1024); // simulate MB values in bytes
        }

        let p50 = h.percentile(50);
        // ceil(60*50/100) = 30 → 30 MB
        assert_eq!(p50, 30 * 1024 * 1024);

        let p95 = h.percentile(95);
        // ceil(60*95/100) = 57 → 57 MB
        assert_eq!(p95, 57 * 1024 * 1024);
    }
}
