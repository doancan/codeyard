//! The session's connection endpoint (`Link`): writes to the current connection, buffers terminal output, exits and
//! file changes while none is attached, and counts each terminal's output so a client that attaches to it again can
//! tell what it missed.

use std::collections::HashMap;
use std::io::{self, BufWriter, Write};
use std::sync::{Arc, Mutex};

use proto::{Error, Frame, Response};

/// Maximum output kept per terminal while disconnected (the tail is kept).
const PTY_BACKLOG: usize = 256 * 1024;

/// The session's connection endpoint. While disconnected (daemon mode, ssh dropped) terminal output, shell exits
/// and file changes are buffered and replayed in order on reconnect. Other responses are dropped (the client's
/// pending requests already got "no connection").
#[derive(Default)]
pub(crate) struct Link {
    inner: Mutex<LinkState>,
}

#[derive(Default)]
struct LinkState {
    writer: Option<BufWriter<Box<dyn Write + Send>>>,
    /// Increments on every connect: so a stale connection's drop can't tear down the new one.
    generation: u64,
    backlog: Vec<(u32, Vec<u8>)>,
    /// Per terminal: bytes it has written so far (its output sequence number) and bytes cut from its backlog since a
    /// client last attached to it (`take_counts`).
    counts: HashMap<u32, (u64, u64)>,
    exits: Vec<(u32, Option<i32>)>,
    changes: Vec<(String, proto::ChangeKind)>,
}

pub(crate) type Out = Arc<Link>;

impl Link {
    /// A response (id ≠ 0) or an event (id 0). Events are buffered while disconnected.
    /// Borrowed response frames share the codec used for replay and daemon handshakes.
    pub(crate) fn send(&self, id: u32, body: Response) -> io::Result<()> {
        let mut st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Response::PtyOutput { pty, data } = &body {
            st.counts.entry(*pty).or_default().0 += data.len() as u64;
        }
        if let Some(w) = st.writer.as_mut() {
            match proto::write_frame(w, &Frame { id, body: &body }) {
                Ok(()) => return Ok(()),
                // Frame too large (a huge directory listing, an `Exec` with both streams full): the connection is fine, only this response
                // could not be sent. It is reported as an error so the client doesn't wait forever; events (id 0) are dropped.
                Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                    if id != 0 {
                        let note = Response::Error(Error::Other(format!("the answer is too large to send ({e})")));
                        if proto::write_frame(w, &Frame { id, body: &note }).is_err() {
                            st.writer = None;
                        }
                    }
                    return Ok(());
                }
                Err(_) => st.writer = None, // connection dropped
            }
        }
        match body {
            Response::PtyOutput { pty, data } => {
                match st.backlog.iter_mut().find(|(p, _)| *p == pty) {
                    Some((_, buf)) => buf.extend_from_slice(&data),
                    None => st.backlog.push((pty, data)),
                }
                let mut cut = 0;
                for (_, buf) in st.backlog.iter_mut().filter(|(p, _)| *p == pty) {
                    if buf.len() > PTY_BACKLOG {
                        cut = buf.len() - PTY_BACKLOG;
                        buf.drain(..cut);
                    }
                }
                st.counts.entry(pty).or_default().1 += cut as u64;
            }
            Response::PtyExit { pty, code } => {
                st.counts.remove(&pty);
                st.exits.push((pty, code));
            }
            Response::FsChanged(c) => {
                for ch in c {
                    if !st.changes.contains(&ch) {
                        st.changes.push(ch);
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// New connection: handshake response first, then the buffered events.
    pub(crate) fn attach(&self, writer: Box<dyn Write + Send>, hello: Option<Frame<Response>>) -> io::Result<u64> {
        let mut st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut w = BufWriter::new(writer);
        if let Some(hello) = hello {
            proto::write_frame(&mut w, &Frame { id: hello.id, body: &hello.body })?;
        }
        // Clear only after a successful write: one that breaks midway must not lose buffered events (resent on next connect).
        for (pty, data) in &st.backlog {
            proto::write_frame(&mut w, &Frame { id: 0, body: &Response::PtyOutput { pty: *pty, data: data.clone() } })?;
        }
        for (pty, code) in &st.exits {
            proto::write_frame(&mut w, &Frame { id: 0, body: &Response::PtyExit { pty: *pty, code: *code } })?;
        }
        if !st.changes.is_empty() {
            proto::write_frame(&mut w, &Frame { id: 0, body: &Response::FsChanged(st.changes.clone()) })?;
        }
        st.backlog.clear();
        st.exits.clear();
        st.changes.clear();
        st.writer = Some(w);
        st.generation += 1;
        Ok(st.generation)
    }

    /// Mark disconnected, but only if this is still the current connection.
    pub(crate) fn detach(&self, generation: u64) {
        let mut st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if st.generation == generation {
            st.writer = None;
        }
    }

    pub(crate) fn generation(&self) -> u64 {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).generation
    }

    pub(crate) fn is_attached(&self) -> bool {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).writer.is_some()
    }

    /// A terminal's output sequence number (bytes written so far) and the bytes cut from its backlog since the last
    /// call: a client that attaches to the terminal again shows that gap.
    pub(crate) fn take_counts(&self, pty: u32) -> (u64, u64) {
        let mut st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let entry = st.counts.entry(pty).or_default();
        let counts = *entry;
        entry.1 = 0;
        counts
    }
}

pub(crate) fn send(out: &Out, id: u32, body: Response) -> io::Result<()> {
    out.send(id, body)
}
