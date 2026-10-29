//! Frames between the app and `codeyard-exthost` (docs/plugin-architecture.md §3, §6.2), over the exthost's stdin and
//! stdout. Frame: 4-byte little-endian length + a message encoded with postcard, like the agent protocol (`proto`).
//!
//! The app never trusts the exthost: every frame from it is size-capped (`limits::FRAME_BYTES`) and counted against a
//! byte-rate budget, and either overrun is an error after which the app kills the process. Ids and tickets are the
//! exthost's own numbers; the app checks them against what it has outstanding.
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

#[cfg(windows)]
pub mod appcontainer;

use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

use ext_api::value::{Completion, EnvInfo, Error as ApiError, Notify, Request, Response, Start, StreamEvent, Ticket};
use ext_manifest::limits;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Compared in the handshake. The app and the exthost ship together, so they must match exactly; on a mismatch (the
/// app updated in place) the exthost exits and the app starts the new one.
pub const VERSION: u32 = 1;

/// App → exthost.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToHost {
    /// First frame. The exthost answers `Ready`, or `Mismatch` and exits.
    Hello(Box<Hello>),
    /// Call `guest.handle(id, request)`. `deadline_ms` is how long the app waits for `Respond`; 0 means no deadline.
    Handle { id: u64, deadline_ms: u32, request: Request },
    /// The app no longer waits for request `id`: call `guest.cancel(id)`.
    Cancel { id: u64 },
    /// A ticket finished: call `guest.complete`.
    Complete { ticket: Ticket, completion: Completion },
    /// Streamed data for a ticket: call `guest.event`.
    Event { ticket: Ticket, event: StreamEvent },
    /// New values of the extension's settings (JSON by key; none when unset). The app follows it with a `Handle` of
    /// `request.settings-changed`.
    Settings(Vec<(String, Option<String>)>),
    /// `env.info` changed (theme, language, trust).
    Env(EnvInfo),
    /// Call `guest.deactivate` and exit.
    Shutdown,
}

/// What the exthost needs to start one extension.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub version: u32,
    /// The app's build id; the exthost compares it with its own.
    pub build: String,
    pub extension: String,
    /// The extension's unpacked package (read-only) and its compiled code inside the cache.
    pub package_dir: String,
    pub compiled: String,
    /// The expected sha256 of `compiled`, from its sidecar: checked before loading.
    pub compiled_sha256: [u8; 32],
    /// The extension's data folder, for `storage`.
    pub data_dir: String,
    /// `codeyard:api` interfaces whose permission is granted now. The rest link to stubs that fail with
    /// `denied { why: revoked-by-user }` (or `untrusted-workspace`).
    pub granted: Vec<String>,
    pub untrusted: bool,
    pub env: EnvInfo,
    pub settings: Vec<(String, Option<String>)>,
    pub folders: Vec<String>,
    /// Why it starts, passed to `guest.activate`.
    pub reason: String,
    /// Enter the OS sandbox before loading the code. True for everything installed; false only for `cyx dev`, which
    /// runs the author's own folder behind Wasm and the broker alone (docs/plugin-architecture.md §3).
    pub sandbox: bool,
}

/// Exthost → app.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToApp {
    Ready {
        version: u32,
        build: String,
    },
    /// The handshake failed: the exthost exits after sending this.
    Mismatch {
        version: u32,
        build: String,
    },
    /// `guest.activate` returned.
    Activated(Result<(), String>),
    /// A host call that returns `ticket`.
    Start {
        ticket: Ticket,
        call: Start,
    },
    Notify(Notify),
    /// The guest trapped or ran out of memory; the exthost exits after sending this.
    Trap(Trap),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trap {
    pub message: String,
    /// `function (file:line)` frames, innermost first, as far as the module's debug info allows.
    pub frames: Vec<String>,
}

impl ToApp {
    /// `requests.respond`, the answer to `ToHost::Handle`.
    pub fn respond(id: u64, result: Result<Response, ApiError>) -> ToApp {
        ToApp::Notify(Notify::Respond { id, result })
    }
}

/// The id both ends of one build share: the version plus `CODEYARD_BUILD_ID` when the release build sets it.
pub const BUILD: &str = match option_env!("CODEYARD_BUILD_ID") {
    Some(id) => id,
    None => env!("CARGO_PKG_VERSION"),
};

/// The two ends compare versions and build ids.
pub fn handshake_ok(hello: &Hello, build: &str) -> bool {
    hello.version == VERSION && hello.build == build
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// The other end closed the pipe between frames.
    Closed,
    TooLarge {
        len: usize,
    },
    /// The exthost sent more than its byte-rate budget allows.
    RateExceeded,
    Decode(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "extension host pipe: {e}"),
            Error::Closed => f.write_str("the extension host closed its pipe"),
            Error::TooLarge { len } => {
                write!(f, "the extension host sent a {len}-byte frame; the limit is {}", limits::FRAME_BYTES)
            }
            Error::RateExceeded => f.write_str("the extension host sent more data than its rate limit allows"),
            Error::Decode(e) => write!(f, "the extension host sent a malformed frame: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

/// Write one frame. A message over `limits::FRAME_BYTES` is refused here, so a call that is too large fails at its
/// sender with `TooLarge` instead of killing the connection.
pub fn write<T: Serialize>(w: &mut impl Write, msg: &T) -> Result<(), Error> {
    let body = postcard::to_stdvec(msg).map_err(|e| Error::Decode(e.to_string()))?;
    if body.len() > limits::FRAME_BYTES {
        return Err(Error::TooLarge { len: body.len() });
    }
    let len = u32::try_from(body.len()).map_err(|_| Error::TooLarge { len: body.len() })?;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(&body)?;
    w.flush()?;
    Ok(())
}

/// Reads frames, optionally against a byte-rate budget (the app's side, reading an exthost).
pub struct Reader<R> {
    inner: R,
    budget: Option<RateBudget>,
}

impl<R: Read> Reader<R> {
    /// For the exthost reading the app, which is trusted.
    pub fn new(inner: R) -> Self {
        Reader { inner, budget: None }
    }

    /// For the app reading an exthost.
    pub fn limited(inner: R, now: Instant) -> Self {
        Reader { inner, budget: Some(RateBudget::new(now)) }
    }

    /// The next frame. Checks the length before reading the body, and charges the budget at `now`.
    pub fn read<T: DeserializeOwned>(&mut self, now: Instant) -> Result<T, Error> {
        let mut len = [0u8; 4];
        match self.inner.read_exact(&mut len) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Err(Error::Closed),
            Err(e) => return Err(e.into()),
        }
        let len = u32::from_le_bytes(len) as usize;
        if len > limits::FRAME_BYTES {
            return Err(Error::TooLarge { len });
        }
        if let Some(b) = &mut self.budget {
            b.take(len as u64 + 4, now)?;
        }
        let mut body = vec![0u8; len];
        self.inner.read_exact(&mut body)?;
        postcard::from_bytes(&body).map_err(|e| Error::Decode(e.to_string()))
    }
}

/// A token bucket: `limits::IPC_BYTES_PER_SECOND` sustained, `limits::IPC_BURST_BYTES` at once.
#[derive(Clone, Debug)]
pub struct RateBudget {
    available: f64,
    at: Instant,
}

impl RateBudget {
    pub fn new(now: Instant) -> Self {
        RateBudget { available: limits::IPC_BURST_BYTES as f64, at: now }
    }

    pub fn take(&mut self, bytes: u64, now: Instant) -> Result<(), Error> {
        let elapsed = now.saturating_duration_since(self.at);
        self.at = now;
        let refill = elapsed.as_secs_f64() * limits::IPC_BYTES_PER_SECOND as f64;
        self.available = (self.available + refill).min(limits::IPC_BURST_BYTES as f64);
        if bytes as f64 > self.available {
            return Err(Error::RateExceeded);
        }
        self.available -= bytes as f64;
        Ok(())
    }

    /// How long until `bytes` would fit again; for tests and the Inspector.
    pub fn wait_for(&self, bytes: u64) -> Duration {
        let missing = bytes as f64 - self.available;
        if missing <= 0.0 { Duration::ZERO } else { Duration::from_secs_f64(missing / limits::IPC_BYTES_PER_SECOND as f64) }
    }
}

#[cfg(test)]
mod tests;
