//! The write half of a connection, owned by its own thread. Callers serialize a frame, queue it and (when they need
//! to know it went out) wait for the thread's answer, so a stalled peer (a full ssh pipe, a dead link) can block a
//! caller that asked to wait but never the UI thread, which only queues. The queue is bounded in bytes.

use std::collections::VecDeque;
use std::io::{self, BufWriter, Write};
use std::sync::Arc;
use std::sync::mpsc;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::Locked;

/// Bytes queued (including the frame being written) before a caller that waits has to wait for room. One frame is
/// always accepted into an empty queue, so a maximal frame still goes through.
const QUEUE_LIMIT: usize = proto::MAX_FRAME;
/// What fire-and-forget frames may add on top of the limit: keystrokes and edits are small, and they must not be lost or
/// block the caller. Past this the peer has not read for tens of megabytes and the connection is given up on.
const POST_SLACK: usize = 8 * 1024 * 1024;
/// How long a caller that waits for room in the queue keeps waiting.
const ROOM_TIMEOUT: Duration = Duration::from_secs(60);

type Done = mpsc::Sender<io::Result<()>>;

struct Item {
    bytes: Vec<u8>,
    done: Option<Done>,
}

#[derive(Default)]
struct State {
    queue: VecDeque<Item>,
    /// Queued bytes plus the frame the thread is writing right now.
    bytes: usize,
    closed: bool,
    /// The first write error: everything queued behind it is failed with it and nothing new is accepted.
    failed: Option<io::ErrorKind>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    /// Wakes the thread (work or close) and callers waiting for room.
    changed: Condvar,
}

pub(crate) struct Writer {
    shared: Arc<Shared>,
}

impl Writer {
    pub(crate) fn start(w: BufWriter<Box<dyn Write + Send>>) -> io::Result<Writer> {
        let shared = Arc::new(Shared::default());
        let thread_shared = shared.clone();
        std::thread::Builder::new().name("host-writer".into()).spawn(move || run(&thread_shared, w))?;
        Ok(Writer { shared })
    }

    /// Queue `bytes` and wait until they were written (and flushed). A write error comes back as the error.
    pub(crate) fn send(&self, bytes: Vec<u8>) -> io::Result<()> {
        let rx = self.enqueue(bytes, Some(Instant::now() + ROOM_TIMEOUT))?;
        rx.recv().unwrap_or_else(|_| Err(io::ErrorKind::NotConnected.into()))
    }

    /// Wait at most `limit` for queue room and the write together. A frame already queued stays queued.
    pub(crate) fn send_within(&self, bytes: Vec<u8>, limit: Duration) -> io::Result<()> {
        let deadline = Instant::now() + limit;
        let rx = self.enqueue(bytes, Some(deadline))?;
        rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).unwrap_or_else(|_| Err(io::ErrorKind::TimedOut.into()))
    }

    /// Queue `bytes` without waiting for anything. `WouldBlock` means the peer has stopped reading and the queue is
    /// full even with the slack: the caller has to treat the connection as lost, the frame is not queued.
    pub(crate) fn post(&self, bytes: Vec<u8>) -> io::Result<()> {
        self.enqueue(bytes, None).map(drop)
    }

    /// Stop accepting frames; the thread writes what is queued and exits.
    pub(crate) fn close(&self) {
        self.shared.state.locked().closed = true;
        self.shared.changed.notify_all();
    }

    fn enqueue(&self, bytes: Vec<u8>, deadline: Option<Instant>) -> io::Result<mpsc::Receiver<io::Result<()>>> {
        let len = bytes.len();
        let mut st = self.shared.state.locked();
        loop {
            if let Some(kind) = st.failed {
                return Err(kind.into());
            }
            if st.closed {
                return Err(io::ErrorKind::NotConnected.into());
            }
            let cap = if deadline.is_some() { QUEUE_LIMIT } else { QUEUE_LIMIT + POST_SLACK };
            if st.bytes == 0 || st.bytes + len <= cap {
                break;
            }
            let Some(deadline) = deadline else {
                return Err(io::Error::new(io::ErrorKind::WouldBlock, "the connection is not accepting data"));
            };
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(io::ErrorKind::TimedOut.into());
            }
            st = self.shared.changed.wait_timeout(st, left).unwrap_or_else(std::sync::PoisonError::into_inner).0;
        }
        let (tx, rx) = mpsc::channel();
        st.bytes += len;
        st.queue.push_back(Item { bytes, done: deadline.map(|_| tx) });
        drop(st);
        self.shared.changed.notify_all();
        Ok(rx)
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.close();
    }
}

fn run(shared: &Shared, mut w: BufWriter<Box<dyn Write + Send>>) {
    loop {
        let item = {
            let mut st = shared.state.locked();
            loop {
                if let Some(item) = st.queue.pop_front() {
                    break item;
                }
                if st.closed {
                    return;
                }
                st = shared.changed.wait(st).unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        };
        let written = w.write_all(&item.bytes).and_then(|()| w.flush());
        let mut st = shared.state.locked();
        st.bytes -= item.bytes.len();
        match written {
            Ok(()) => {
                if let Some(done) = item.done {
                    let _ = done.send(Ok(()));
                }
                drop(st);
                shared.changed.notify_all();
            }
            Err(e) => {
                st.failed = Some(e.kind());
                for item in std::iter::once(item).chain(st.queue.drain(..)) {
                    if let Some(done) = item.done {
                        let _ = done.send(Err(e.kind().into()));
                    }
                }
                st.bytes = 0;
                drop(st);
                shared.changed.notify_all();
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_within_bounds_the_wait_for_queue_room() {
        struct Stalled {
            entered: mpsc::Sender<()>,
            release: mpsc::Receiver<()>,
        }
        impl Write for Stalled {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                let _ = self.entered.send(());
                let _ = self.release.recv_timeout(Duration::from_secs(2));
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let (entered, ready) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let writer = Writer::start(BufWriter::new(Box::new(Stalled { entered, release: gate }))).unwrap();
        writer.post(vec![0; QUEUE_LIMIT]).unwrap();
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        let start = Instant::now();
        let result = writer.send_within(vec![1], Duration::from_millis(30));
        let elapsed = start.elapsed();
        drop(release);
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert!(elapsed < Duration::from_secs(1), "30 ms queue timeout took {elapsed:?}");
    }
}
