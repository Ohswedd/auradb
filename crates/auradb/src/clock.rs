//! A wall-clock time source, abstracted so transaction-timeout and abandoned
//! transaction reaping can be driven deterministically in tests without sleeping.
//!
//! Production code uses [`WallClock::System`], which reads the system clock in
//! whole seconds since the Unix epoch. Tests use [`WallClock::manual`], a clock
//! that only advances when [`WallClock::advance`] (or [`WallClock::set`]) is
//! called, so a timeout can be exercised instantly and reproducibly.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// A source of coarse wall-clock time (whole seconds).
#[derive(Clone, Default)]
pub enum WallClock {
    /// The real system clock.
    #[default]
    System,
    /// A manually-advanced clock for deterministic tests.
    Manual(Arc<AtomicU64>),
}

impl WallClock {
    /// Create a manual clock starting at second zero.
    pub fn manual() -> WallClock {
        WallClock::Manual(Arc::new(AtomicU64::new(0)))
    }

    /// The current time in whole seconds since the Unix epoch (or since the
    /// manual clock's origin).
    pub fn now_secs(&self) -> u64 {
        match self {
            WallClock::System => SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            WallClock::Manual(t) => t.load(Ordering::SeqCst),
        }
    }

    /// Advance a manual clock by `secs`. A no-op on the system clock.
    pub fn advance(&self, secs: u64) {
        if let WallClock::Manual(t) = self {
            t.fetch_add(secs, Ordering::SeqCst);
        }
    }

    /// Set a manual clock to an absolute value. A no-op on the system clock.
    pub fn set(&self, secs: u64) {
        if let WallClock::Manual(t) = self {
            t.store(secs, Ordering::SeqCst);
        }
    }
}

impl std::fmt::Debug for WallClock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WallClock::System => write!(f, "WallClock::System"),
            WallClock::Manual(t) => write!(f, "WallClock::Manual({})", t.load(Ordering::SeqCst)),
        }
    }
}
