//! The clock the reconnect loop waits on. Production sleeps for real; a test that drops a connection and expects it back would
//! otherwise wait through the backoff (1, 2, 4, 8, 15 s ...) and, when the first attempts fail, for tens of seconds more than it
//! does on a quiet run. `SkippingClock` returns from `sleep` at once and counts the time as passed, so the backoff and the
//! give-up deadline behave as they would but no test waits for them.

use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use crate::Locked as _;

pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
    fn sleep(&self, duration: Duration);
}

/// Real time.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// Time that moves forward by what was slept instead of waiting for it.
#[derive(Default)]
pub struct SkippingClock {
    skipped: Mutex<Duration>,
}

impl Clock for SkippingClock {
    fn now(&self) -> Instant {
        Instant::now() + *self.skipped.locked()
    }

    fn sleep(&self, duration: Duration) {
        *self.skipped.locked() += duration;
        std::thread::yield_now();
    }
}

static ACTIVE: RwLock<Option<Arc<dyn Clock>>> = RwLock::new(None);

/// The clock reconnecting uses: real time unless [`use_skipping_clock`] was called.
pub(crate) fn current() -> Arc<dyn Clock> {
    ACTIVE.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone().unwrap_or_else(|| Arc::new(SystemClock))
}

/// For tests of the whole application: every reconnecting connection of this process stops waiting for its backoff.
pub fn use_skipping_clock() {
    *ACTIVE.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(SkippingClock::default()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_skipping_clock_counts_a_sleep_as_passed_without_waiting() {
        let clock = SkippingClock::default();
        let (before, started) = (clock.now(), Instant::now());
        clock.sleep(Duration::from_secs(3600));
        assert!(clock.now() - before >= Duration::from_secs(3600));
        assert!(started.elapsed() < Duration::from_secs(3600), "it returned at once");
    }
}
