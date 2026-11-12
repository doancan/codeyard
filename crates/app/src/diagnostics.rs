//! What helps to find out why something failed, without anything leaving the machine: the log file, the redaction every
//! report goes through, and the diagnostics report the user can copy.

use std::sync::{Mutex, MutexGuard};

#[path = "diagnostics_log.rs"]
pub mod log;
#[path = "diagnostics_redact.rs"]
pub mod redact;
#[path = "diagnostics_report.rs"]
pub mod report;

/// `lock()` that goes on after another thread panicked while holding the lock. The data behind such a lock (a table of
/// waiting requests, a settings tree) stays usable, and one panicked reader thread must not turn every later call into a panic.
pub trait LockExt<T> {
    fn lock_recover(&self) -> MutexGuard<'_, T>;
}

impl<T> LockExt<T> for Mutex<T> {
    fn lock_recover(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lock_poisoned_by_a_panic_is_still_usable() {
        let m = std::sync::Arc::new(Mutex::new(vec![1]));
        let poisoner = m.clone();
        let result = std::thread::spawn(move || {
            let _guard = poisoner.lock().unwrap();
            panic!("poison the lock on purpose");
        })
        .join();
        assert!(result.is_err());
        assert!(m.lock().is_err(), "the lock is poisoned");
        m.lock_recover().push(2);
        assert_eq!(*m.lock_recover(), [1, 2]);
    }
}
