//! Taking terminals back after the client restarts (daemon mode). A terminal is named to a new connection by the
//! session it lives in (the client's own secret session id) and a token the client asked for and saved: this agent
//! process's random instance id plus a random secret of that terminal. A process id is never part of it, so a token
//! cannot reach a process that merely reuses a pid, a terminal of another daemon or one whose secret was not given out.
//! Only one connection owns a session at a time (`Session::run` stops reading from a connection a newer one replaced).

use std::io;
use std::sync::OnceLock;

use proto::{Error, Request, Response};

use crate::{Locked, Session, send};

/// 128 random bits as hex.
fn random_hex() -> String {
    let mut bytes = [0u8; 16];
    #[cfg(unix)]
    {
        use std::io::Read;
        if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes)).is_ok() {
            return bytes.iter().map(|b| format!("{b:02x}")).collect();
        }
    }
    // No random device (or not Unix, where terminals are not taken back): unique, not secret.
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    for (i, chunk) in bytes.chunks_mut(8).enumerate() {
        chunk.copy_from_slice(&proto::content_hash(format!("{nanos}-{}-{n}-{i}", std::process::id()).as_bytes()).to_le_bytes());
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// This agent process's instance id: tokens of another (earlier or later) daemon never match.
pub(crate) fn instance() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(random_hex)
}

/// Compare without stopping at the first difference.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl Session {
    /// `PtyToken`, `PtyAttach` and `PtyList`.
    pub(crate) fn reattach_request(&self, id: u32, body: Request) -> io::Result<()> {
        let body = match body {
            Request::PtyToken { pty } if self.ptys.locked().contains_key(&pty) => {
                let secret = self.secrets.locked().entry(pty).or_insert_with(random_hex).clone();
                Response::PtyToken { instance: instance().to_string(), secret }
            }
            Request::PtyToken { pty } | Request::PtyAttach { pty, .. } if !self.ptys.locked().contains_key(&pty) => {
                self.secrets.locked().remove(&pty);
                Response::Error(Error::NotFound(format!("terminal {pty}")))
            }
            Request::PtyAttach { pty, instance: given, secret } => {
                let known = self.secrets.locked().get(&pty).cloned();
                if same(&given, instance()) && known.is_some_and(|k| same(&k, &secret)) {
                    let (sequence, dropped) = self.out.take_counts(pty);
                    Response::PtyAttached { pty, sequence, dropped }
                } else {
                    Response::Error(Error::PermissionDenied(format!("terminal {pty}: the token is not this session's")))
                }
            }
            Request::PtyList => {
                let mut ids: Vec<u32> = self.ptys.locked().keys().copied().collect();
                ids.sort_unstable();
                Response::PtyList(ids)
            }
            _ => Response::Error(Error::Unsupported("not a terminal request".into())),
        };
        if id == 0 { Ok(()) } else { send(&self.out, id, body) }
    }
}
