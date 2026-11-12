//! Looking at the toolchain (is `cargo nextest` there, which targets are installed) must never leave the UI waiting on a
//! tool that hangs: the question runs on a thread of its own and is given up on after a limit.

use std::sync::mpsc;
use std::time::Duration;

/// How long a question to a toolchain tool may take.
pub const LIMIT: Duration = Duration::from_secs(5);

/// The result of `question`, or `None` when it has not finished within `limit` (it is left to finish on its own thread).
pub fn within<T: Send + 'static>(limit: Duration, question: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(question());
    });
    rx.recv_timeout(limit).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_in_time_is_returned_and_a_hang_is_given_up_on() {
        assert_eq!(within(Duration::from_secs(5), || 7), Some(7));
        let started = std::time::Instant::now();
        assert_eq!(within(Duration::from_millis(50), || std::thread::sleep(Duration::from_secs(3))), None);
        assert!(started.elapsed() < Duration::from_secs(2), "the caller did not wait for the hung tool");
    }
}
