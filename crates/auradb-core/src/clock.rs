//! A monotonic logical clock used to allocate transaction ids and versions.

use std::sync::atomic::{AtomicU64, Ordering};

/// A thread-safe monotonically increasing counter.
///
/// Used to allocate transaction ids and record versions. It is seeded from the
/// highest value recovered on startup so ids never regress across restarts.
#[derive(Debug)]
pub struct LogicalClock {
    next: AtomicU64,
}

impl LogicalClock {
    /// Create a clock whose next value will be `start`.
    pub fn new(start: u64) -> Self {
        LogicalClock {
            next: AtomicU64::new(start),
        }
    }

    /// Allocate and return the next value.
    pub fn tick(&self) -> u64 {
        self.next.fetch_add(1, Ordering::SeqCst)
    }

    /// The value that would be returned by the next [`tick`](Self::tick),
    /// without consuming it.
    pub fn peek(&self) -> u64 {
        self.next.load(Ordering::SeqCst)
    }

    /// Ensure the clock will not return a value `<= observed`.
    pub fn observe(&self, observed: u64) {
        let mut current = self.next.load(Ordering::SeqCst);
        while current <= observed {
            match self.next.compare_exchange(
                current,
                observed + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }
}

impl Default for LogicalClock {
    fn default() -> Self {
        LogicalClock::new(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_increase() {
        let c = LogicalClock::new(1);
        assert_eq!(c.tick(), 1);
        assert_eq!(c.tick(), 2);
        assert_eq!(c.peek(), 3);
    }

    #[test]
    fn observe_advances_past_recovered_value() {
        let c = LogicalClock::new(1);
        c.observe(100);
        assert_eq!(c.tick(), 101);
    }

    #[test]
    fn observe_never_regresses() {
        let c = LogicalClock::new(50);
        c.observe(10);
        assert_eq!(c.tick(), 50);
    }
}
