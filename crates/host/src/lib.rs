//! `Host`: the UI's single entry to the file system, search and terminals. Local
//! and remote speak the same framed protocol: the local agent runs on a thread
//! (over a pipe), the remote one over `ssh` stdio. One code path.
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod askpass;
mod outbox;
pub mod ssh;
pub mod ssh_config;
pub mod transfer;

use std::collections::HashMap;
use std::io::{BufReader, BufWriter, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use proto::{ChangeKind, DirEntry, Frame, HostInfo, Meta, Request, Response, SearchQuery};

pub type FsChanges = Vec<(String, ChangeKind)>;

/// `Mutex::lock` that ignores poisoning. The guarded state (tables of sessions, buffers) stays
/// consistent across a panicking holder, and a poison panic would take down every other thread.
pub(crate) trait Locked<T> {
    fn locked(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> Locked<T> for Mutex<T> {
    fn locked(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Fold `new` into the changes a slow consumer has not read yet: one entry per path (the agent only reports the few paths it
/// was asked to watch, so the result is bounded by that set however long the consumer stalls). The kinds combine the way the
/// path's history reads: created then modified is created, removed then created is modified, anything then removed is removed.
pub fn merge_fs_changes(pending: &mut FsChanges, new: FsChanges) {
    let mut index: std::collections::HashMap<String, usize> = pending.iter().enumerate().map(|(i, (p, _))| (p.clone(), i)).collect();
    for (path, kind) in new {
        match index.get(&path) {
            Some(&i) => {
                pending[i].1 = match (pending[i].1, kind) {
                    (ChangeKind::Created, ChangeKind::Modified) => ChangeKind::Created,
                    (ChangeKind::Removed, ChangeKind::Created) => ChangeKind::Modified,
                    (_, k) => k,
                }
            }
            None => {
                index.insert(path.clone(), pending.len());
                pending.push((path, kind));
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum HostError {
    /// The agent refused the operation (not found, permission denied, conflict...).
    Op(proto::Error),
    /// The connection dropped or could never be established.
    Disconnected(String),
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostError::Op(e) => e.fmt(f),
            HostError::Disconnected(m) => write!(f, "not connected: {m}"),
        }
    }
}

impl std::error::Error for HostError {}

pub type Result<T> = std::result::Result<T, HostError>;

pub struct FileList {
    pub paths: Vec<String>,
    pub truncated: bool,
}

/// A running search: `rx` yields `SearchChunk`s first, then `SearchDone` (or `Error`).
pub struct SearchStream {
    pub id: u32,
    pub rx: mpsc::Receiver<Response>,
}

/// A running `HashTree`: records arrive in chunks, then one `Done` (or `Failed`). `next` returns `None` when the connection
/// dropped before the end, so a caller keeps what it got and can say the result is partial.
pub struct HashStream {
    pub id: u32,
    pub rx: mpsc::Receiver<Response>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HashEvent {
    Records(Vec<proto::HashRecord>),
    Done { entries: u32, truncated: bool, cancelled: bool },
    Failed(proto::Error),
}

impl HashStream {
    pub fn next(&self) -> Option<HashEvent> {
        loop {
            return Some(match self.rx.recv().ok()? {
                Response::HashChunk(records) => HashEvent::Records(records),
                Response::HashDone { entries, truncated, cancelled } => HashEvent::Done { entries, truncated, cancelled },
                Response::Error(e) => HashEvent::Failed(e),
                _ => continue,
            });
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PtyEvent {
    Output(Vec<u8>),
    /// The shell exited (or the connection dropped: `None`).
    Exit(Option<i32>),
}

pub struct Pty {
    pub id: u32,
    pub events: mpsc::Receiver<PtyEvent>,
}

/// Piped process (language server): events have the same shape as a terminal's.
pub type ProcEvent = PtyEvent;

pub struct Proc {
    pub id: u32,
    pub events: mpsc::Receiver<ProcEvent>,
}

/// Result of a finished command.
#[derive(Debug, Clone)]
pub struct ExecOutput {
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl ExecOutput {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }

    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr_str(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim().to_string()
    }
}

/// Blocking calls (the UI runs them on a background executor); streams return channels.
pub trait Host: Send + Sync + 'static {
    fn info(&self) -> &HostInfo;
    /// Name shown in the status bar: "Local", "ssh://node-dev".
    fn label(&self) -> String;
    fn is_remote(&self) -> bool;
    fn call(&self, req: Request) -> Result<Response>;
    fn search(&self, query: SearchQuery) -> Result<SearchStream>;
    /// Folder comparison (see `Request::HashTree`). Stop it with `cancel(stream.id)`.
    fn hash_tree(&self, root: &str, ignore: Vec<String>, gitignore: bool, limit: u32, only: Option<Vec<String>>) -> Result<HashStream>;
    fn cancel(&self, id: u32);
    fn pty_spawn(&self, cols: u16, rows: u16, cwd: Option<String>, shell: Option<String>) -> Result<Pty>;
    /// No reply expected (so a keystroke doesn't wait for a round trip).
    fn pty_input(&self, pty: u32, data: Vec<u8>);
    fn pty_resize(&self, pty: u32, cols: u16, rows: u16);
    fn pty_kill(&self, pty: u32);
    /// Change the watched paths (resends the whole set).
    fn watch(&self, paths: Vec<String>);
    /// Change events; a new call replaces the previous one.
    fn fs_events(&self) -> mpsc::Receiver<FsChanges>;
    /// Connection state (reconnection).
    fn status_events(&self) -> mpsc::Receiver<ConnStatus>;
    /// Start a long-running process (language server). `Exit(None)` arrives if the connection drops.
    fn proc_spawn(&self, program: &str, args: Vec<String>, cwd: Option<String>) -> Result<Proc>;
    /// No reply expected.
    fn proc_input(&self, proc: u32, data: Vec<u8>);
    fn proc_kill(&self, proc: u32);

    /// Large file: (line count, size). The agent scans the file on the first call.
    fn line_index(&self, path: &str) -> Result<(u64, u64)> {
        match self.call(Request::LineIndex { path: path.into() })? {
            Response::LineIndex { lines, size } => Ok((lines, size)),
            other => Err(unexpected(other)),
        }
    }

    fn read_lines(&self, path: &str, start: u64, count: u32) -> Result<Vec<String>> {
        match self.call(Request::ReadLines { path: path.into(), start, count })? {
            Response::Lines(l) => Ok(l),
            other => Err(unexpected(other)),
        }
    }

    /// Run the command and wait (git).
    fn exec(&self, program: &str, args: &[&str], cwd: Option<&str>, stdin: Option<Vec<u8>>) -> Result<ExecOutput> {
        let req = Request::Exec { program: program.into(), args: args.iter().map(|a| a.to_string()).collect(), cwd: cwd.map(str::to_string), stdin };
        match self.call(req)? {
            Response::Exited { code, stdout, stderr } => Ok(ExecOutput { code, stdout, stderr }),
            other => Err(unexpected(other)),
        }
    }

    fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>> {
        match self.call(Request::ReadDir { path: path.into() })? {
            Response::Entries(e) => Ok(e),
            other => Err(unexpected(other)),
        }
    }

    fn stat(&self, path: &str) -> Result<Meta> {
        match self.call(Request::Stat { path: path.into() })? {
            Response::Stat(m) => Ok(m),
            other => Err(unexpected(other)),
        }
    }

    /// (content, hash)
    fn read(&self, path: &str) -> Result<(Vec<u8>, u64)> {
        match self.call(Request::Read { path: path.into() })? {
            Response::File { data, hash } => Ok((data, hash)),
            other => Err(unexpected(other)),
        }
    }

    /// Files under the root (relative), honouring .gitignore.
    fn list_files(&self, root: &str, limit: u32) -> Result<FileList> {
        match self.call(Request::ListFiles { root: root.into(), limit })? {
            Response::Files { paths, truncated } => Ok(FileList { paths, truncated }),
            other => Err(unexpected(other)),
        }
    }

    fn create_file(&self, path: &str) -> Result<()> {
        self.done(Request::CreateFile { path: path.into() })
    }

    fn create_dir(&self, path: &str) -> Result<()> {
        self.done(Request::CreateDir { path: path.into() })
    }

    fn rename(&self, from: &str, to: &str) -> Result<()> {
        self.done(Request::Rename { from: from.into(), to: to.into() })
    }

    fn delete(&self, path: &str) -> Result<()> {
        self.done(Request::Delete { path: path.into() })
    }

    fn done(&self, req: Request) -> Result<()> {
        match self.call(req)? {
            Response::Done => Ok(()),
            other => Err(unexpected(other)),
        }
    }

    /// Returns the new hash. `Conflict` if the disk differs from `expect_hash`.
    fn write(&self, path: &str, data: Vec<u8>, expect_hash: Option<u64>) -> Result<u64> {
        match self.call(Request::Write { path: path.into(), data, expect_hash })? {
            Response::Written { hash } => Ok(hash),
            other => Err(unexpected(other)),
        }
    }
}

fn unexpected(r: Response) -> HostError {
    match r {
        Response::Error(e) => HostError::Op(e),
        other => HostError::Disconnected(format!("unexpected response: {other:?}")),
    }
}

/// The party waiting for a request's reply.
enum Waiter {
    Once(mpsc::Sender<Response>),
    /// Until `SearchDone`, `HashDone` or `Error` arrives.
    Stream(mpsc::Sender<Response>),
    /// If the reply is `PtySpawned`, the event channel is registered on the reader thread, so
    /// later output frames can't be lost.
    PtySpawn(mpsc::Sender<Response>, mpsc::Sender<PtyEvent>),
    ProcSpawn(mpsc::Sender<Response>, mpsc::Sender<ProcEvent>),
}

/// Connection state (shown in the UI).
#[derive(Debug, Clone, PartialEq)]
pub enum ConnStatus {
    /// `resumed`: returned to the previous session (terminals are alive).
    Connected {
        resumed: bool,
    },
    Reconnecting {
        attempt: u32,
        error: String,
    },
    /// The reconnection window ran out.
    Lost(String),
}

/// The two ends of an agent connection (ssh process or pipe).
pub struct Transport {
    pub r: Box<dyn Read + Send>,
    pub w: Box<dyn Write + Send>,
    pub child: Option<Child>,
    pub stderr: Arc<Mutex<String>>,
}

impl Transport {
    pub fn pipes(r: Box<dyn Read + Send>, w: Box<dyn Write + Send>) -> Self {
        Transport { r, w, child: None, stderr: Arc::default() }
    }
}

/// Establishes a new connection (called again on reconnect).
pub type Connector = Arc<dyn Fn() -> Result<Transport> + Send + Sync>;

/// Reconnect attempts (the server-side session waits as long).
pub const RECONNECT_WINDOW: std::time::Duration = std::time::Duration::from_secs(300);

/// How long reconnecting is tried: 5 minutes by default; tests shorten it with `CODEYARD_RECONNECT_WINDOW_SECS`
/// (otherwise "connection permanently lost" can't be exercised).
fn reconnect_window() -> std::time::Duration {
    std::env::var("CODEYARD_RECONNECT_WINDOW_SECS").ok().and_then(|v| v.parse::<u64>().ok()).map_or(RECONNECT_WINDOW, std::time::Duration::from_secs)
}

#[derive(Default)]
struct Shared {
    waiters: Mutex<HashMap<u32, Waiter>>,
    ptys: Mutex<HashMap<u32, mpsc::Sender<PtyEvent>>>,
    procs: Mutex<HashMap<u32, mpsc::Sender<ProcEvent>>>,
    fs: Mutex<Option<mpsc::Sender<FsChanges>>>,
    broken: Mutex<Option<String>>,
    writer: Mutex<Option<Arc<outbox::Writer>>>,
    child: Mutex<Option<Child>>,
    stderr: Mutex<Arc<Mutex<String>>>,
    status: Mutex<Vec<mpsc::Sender<ConnStatus>>>,
    /// Watch set to resend on reconnect.
    last_watch: Mutex<Option<Vec<String>>>,
    closing: std::sync::atomic::AtomicBool,
    next_id: AtomicU32,
    /// Pin the call timeout (seconds; 0 = default table). For tests and diagnostics.
    call_limit_secs: AtomicU32,
}

/// Heartbeat interval and how long a single probe waits.
const HEARTBEAT_EVERY: std::time::Duration = std::time::Duration::from_secs(15);
const HEARTBEAT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(20);

impl Shared {
    /// Send `Ping` to the agent; `false` if `Done` doesn't arrive within `limit`.
    fn ping(&self, limit: std::time::Duration) -> bool {
        let id = loop {
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            if id != 0 {
                break id;
            }
        };
        let (tx, rx) = mpsc::channel();
        self.waiters.locked().insert(id, Waiter::Once(tx));
        // Queued, not awaited: a write stuck behind a dead link is what the missed reply below reports.
        if self.post(id, &Request::Ping).is_err() {
            self.waiters.locked().remove(&id);
            return false;
        }
        let ok = rx.recv_timeout(limit).is_ok();
        if !ok {
            self.waiters.locked().remove(&id);
        }
        ok
    }

    /// Drop an unresponsive connection: kill ssh, the reader sees EOF and reconnecting starts. ssh's own
    /// `ServerAlive` catches network loss; this catches a healthy network with a frozen agent.
    fn drop_unresponsive(&self) -> bool {
        let Some(mut c) = self.child.locked().take() else { return false };
        let _ = c.kill();
        let _ = c.wait();
        true
    }

    fn route(&self, frame: Frame<Response>) {
        if frame.id == 0 {
            match frame.body {
                Response::PtyOutput { pty, data } => {
                    if let Some(tx) = self.ptys.locked().get(&pty) {
                        let _ = tx.send(PtyEvent::Output(data));
                    }
                }
                Response::PtyExit { pty, code } => {
                    if let Some(tx) = self.ptys.locked().remove(&pty) {
                        let _ = tx.send(PtyEvent::Exit(code));
                    }
                }
                Response::FsChanged(changes) => {
                    if let Some(tx) = self.fs.locked().as_ref() {
                        let _ = tx.send(changes);
                    }
                }
                Response::ProcOutput { proc, data } => {
                    if let Some(tx) = self.procs.locked().get(&proc) {
                        let _ = tx.send(ProcEvent::Output(data));
                    }
                }
                Response::ProcExit { proc, code } => {
                    if let Some(tx) = self.procs.locked().remove(&proc) {
                        let _ = tx.send(ProcEvent::Exit(code));
                    }
                }
                _ => {}
            }
            return;
        }
        let mut waiters = self.waiters.locked();
        match waiters.remove(&frame.id) {
            Some(Waiter::Once(tx)) => {
                let _ = tx.send(frame.body);
            }
            Some(Waiter::Stream(tx)) => {
                let last = matches!(frame.body, Response::SearchDone { .. } | Response::HashDone { .. } | Response::Error(_));
                // Receiver is gone (search abandoned): silently drop the rest of the stream.
                if tx.send(frame.body).is_ok() && !last {
                    waiters.insert(frame.id, Waiter::Stream(tx));
                }
            }
            Some(Waiter::PtySpawn(tx, events)) => {
                if let Response::PtySpawned { pty } = frame.body {
                    self.ptys.locked().insert(pty, events);
                }
                let _ = tx.send(frame.body);
            }
            Some(Waiter::ProcSpawn(tx, events)) => {
                if let Response::ProcSpawned { proc } = frame.body {
                    self.procs.locked().insert(proc, events);
                }
                let _ = tx.send(frame.body);
            }
            None => {}
        }
    }

    /// Connection is permanently gone: wake all waiters and terminals.
    fn fail(&self, why: String) {
        self.broken.locked().get_or_insert(why);
        self.clear_writer();
        self.waiters.locked().clear();
        for (_, tx) in self.ptys.locked().drain() {
            let _ = tx.send(PtyEvent::Exit(None));
        }
        self.end_procs();
    }

    /// Processes end with the connection (the agent terminates them too).
    fn end_procs(&self) {
        for (_, tx) in self.procs.locked().drain() {
            let _ = tx.send(ProcEvent::Exit(None));
        }
    }

    fn broken(&self) -> Option<String> {
        self.broken.locked().clone()
    }

    fn emit(&self, status: ConnStatus) {
        self.status.locked().retain(|tx| tx.send(status.clone()).is_ok());
    }

    fn stderr_tail(&self) -> String {
        self.stderr.locked().locked().trim().to_string()
    }

    fn writer(&self) -> std::io::Result<Arc<outbox::Writer>> {
        self.writer.locked().clone().ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotConnected))
    }

    fn set_writer(&self, w: BufWriter<Box<dyn Write + Send>>) -> std::io::Result<()> {
        let old = self.writer.locked().replace(Arc::new(outbox::Writer::start(w)?));
        old.inspect(|o| o.close());
        Ok(())
    }

    fn clear_writer(&self) {
        if let Some(w) = self.writer.locked().take() {
            w.close();
        }
    }

    fn encode(id: u32, req: &Request) -> std::io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        proto::write_frame(&mut bytes, &Frame { id, body: req })?;
        Ok(bytes)
    }

    /// Write a frame and wait until it is out. Only for callers that need the outcome and are not the UI thread.
    fn send(&self, id: u32, req: &Request) -> std::io::Result<()> {
        self.writer()?.send(Self::encode(id, req)?)
    }

    /// Queue a frame and return; never waits for the socket.
    fn post(&self, id: u32, req: &Request) -> std::io::Result<()> {
        self.writer()?.post(Self::encode(id, req)?)
    }

    /// The peer stopped reading for good: give the connection up so reconnecting (or failing) takes over.
    fn stalled(&self) {
        self.broken.locked().get_or_insert_with(|| "the connection stopped accepting data".into());
        self.clear_writer();
        if !self.drop_unresponsive() {
            self.fail("the connection stopped accepting data".into());
        }
    }
}

/// Handshake result: info, reader, writer, child process and stderr log.
type Handshake = (HostInfo, BufReader<Box<dyn Read + Send>>, BufWriter<Box<dyn Write + Send>>, Option<Child>, Arc<Mutex<String>>);

/// Longest wait for the handshake reply, so a reconnect loop can't hang forever when ssh connects but the agent
/// never answers.
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Handshake on a new connection: (info, reader, writer). On failure the child is killed and reaped so failed
/// attempts don't leave zombie ssh processes.
fn handshake(t: Transport, session: Option<&str>) -> Result<Handshake> {
    let Transport { r, w, mut child, stderr } = t;
    let disconnected = |m: String| {
        let tail = stderr.locked().trim().to_string();
        HostError::Disconnected(if tail.is_empty() { m } else { format!("{m}\n{tail}") })
    };
    let mut w = BufWriter::new(w);
    let hello = Request::Hello { client_version: proto::VERSION, session: session.map(str::to_string) };
    let outcome = (|| {
        proto::write_frame(&mut w, &Frame { id: 1, body: hello }).map_err(|e| disconnected(e.to_string()))?;
        // Read on its own thread: on timeout the child is killed and the read ends with EOF.
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut r = BufReader::new(r);
            let frame = proto::read_frame::<Response>(&mut r);
            let _ = tx.send((r, frame));
        });
        let (r, frame) = match rx.recv_timeout(HANDSHAKE_TIMEOUT) {
            Ok(v) => v,
            Err(_) => return Err(disconnected(format!("the agent did not answer within {} seconds", HANDSHAKE_TIMEOUT.as_secs()))),
        };
        match frame {
            Ok(Some(Frame { body: Response::Hello(info), .. })) => Ok((info, r)),
            Ok(Some(Frame { body: Response::Error(e), .. })) => Err(HostError::Op(e)),
            Ok(Some(other)) => Err(disconnected(format!("unexpected response: {:?}", other.body))),
            Ok(None) => Err(disconnected("the agent closed the connection".into())),
            Err(e) => Err(disconnected(e.to_string())),
        }
    })();
    match outcome {
        Ok((info, r)) => Ok((info, r, w, child, stderr)),
        Err(e) => {
            if let Some(c) = child.as_mut() {
                let _ = c.kill();
                let _ = c.wait();
            }
            Err(e)
        }
    }
}

/// Framed, multiplexed connection to the agent. With a connector and a
/// session id it reconnects by itself after a drop.
pub struct RemoteHost {
    shared: Arc<Shared>,
    info: HostInfo,
    label: String,
    remote: bool,
}

/// This machine: the agent is a thread in this process, same protocol over a pipe.
// Infallible by signature: a failure here is fd or thread exhaustion at startup, which nothing can recover from.
#[allow(clippy::expect_used)]
pub fn local() -> RemoteHost {
    let (req_r, req_w) = std::io::pipe().expect("pipe");
    let (resp_r, resp_w) = std::io::pipe().expect("pipe");
    std::thread::Builder::new().name("agent".into()).spawn(move || agent::serve(req_r, resp_w)).expect("agent thread");
    let mut host = RemoteHost::from_streams(Box::new(resp_r), Box::new(req_w), "Local").expect("local agent handshake");
    host.remote = false;
    host
}

/// Start a child process as the connection (stdio + stderr capture).
pub fn spawn_transport(mut cmd: Command) -> Result<Transport> {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| HostError::Disconnected(format!("could not start: {e}")))?;
    let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.kill();
        return Err(HostError::Disconnected("could not start: stdio was not piped".into()));
    };
    let stderr: Arc<Mutex<String>> = Arc::default();
    if let Some(mut err) = child.stderr.take() {
        let buf = stderr.clone();
        std::thread::spawn(move || {
            let mut chunk = [0u8; 1024];
            while let Ok(n) = err.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                let mut b = buf.locked();
                b.push_str(&String::from_utf8_lossy(&chunk[..n]));
                // Only the last 4 KB: enough for the error message.
                if b.len() > 4096 {
                    let cut = b.len() - 4096;
                    let cut = (cut..b.len()).find(|&i| b.is_char_boundary(i)).unwrap_or(b.len());
                    b.drain(..cut);
                }
            }
        });
    }
    Ok(Transport { r: Box::new(stdout), w: Box::new(stdin), child: Some(child), stderr })
}

impl RemoteHost {
    /// Connect over any reader/writer pair (no reconnect).
    pub fn from_streams(r: Box<dyn Read + Send>, w: Box<dyn Write + Send>, label: impl Into<String>) -> Result<Self> {
        Self::open(Transport::pipes(r, w), None, None, label)
    }

    /// Start a child process (local agent binary; no reconnection).
    pub fn spawn(cmd: Command, label: impl Into<String>) -> Result<Self> {
        Self::open(spawn_transport(cmd)?, None, None, label)
    }

    /// Reconnectable connection: `connector` sets up a fresh transport per attempt,
    /// `session` finds the session (terminals) on the server.
    pub fn connect(connector: Connector, session: String, label: impl Into<String>) -> Result<Self> {
        let t = connector()?;
        Self::open(t, Some(connector), Some(session), label)
    }

    fn open(t: Transport, connector: Option<Connector>, session: Option<String>, label: impl Into<String>) -> Result<Self> {
        let (info, reader, writer, child, stderr) = handshake(t, session.as_deref())?;
        let reconnects = connector.is_some();
        let shared = Arc::new(Shared { next_id: AtomicU32::new(2), ..Default::default() });
        shared.set_writer(writer).map_err(|e| HostError::Disconnected(e.to_string()))?;
        *shared.child.locked() = child;
        *shared.stderr.locked() = stderr;
        let reader_shared = shared.clone();
        std::thread::Builder::new()
            .name("host-reader".into())
            .spawn(move || reader_loop(reader_shared, reader, connector, session))
            .map_err(|e| HostError::Disconnected(e.to_string()))?;
        // Liveness probe on reconnectable (i.e. real, remote) connections: two misses in a row drop it
        // and reconnecting starts. Skipped while already down or reconnecting.
        if reconnects {
            let weak = Arc::downgrade(&shared);
            std::thread::Builder::new()
                .name("host-heartbeat".into())
                .spawn(move || {
                    let mut misses = 0;
                    loop {
                        std::thread::sleep(HEARTBEAT_EVERY);
                        let Some(shared) = weak.upgrade() else { return };
                        if shared.closing.load(Ordering::Relaxed) {
                            return;
                        }
                        if shared.broken().is_some() {
                            misses = 0;
                            continue;
                        }
                        if shared.ping(HEARTBEAT_DEADLINE) {
                            misses = 0;
                        } else {
                            misses += 1;
                            if misses >= 2 {
                                shared.drop_unresponsive();
                                misses = 0;
                            }
                        }
                    }
                })
                .ok();
        }
        Ok(RemoteHost { shared, info, label: label.into(), remote: true })
    }

    fn disconnected(&self) -> HostError {
        let why = self.shared.broken().unwrap_or_else(|| "connection lost".into());
        let tail = self.shared.stderr_tail();
        HostError::Disconnected(if tail.is_empty() { why } else { format!("{why}\n{tail}") })
    }

    fn send(&self, id: u32, req: Request) -> Result<()> {
        if self.shared.broken().is_some() {
            return Err(self.disconnected());
        }
        self.shared.send(id, &req).map_err(|_| self.disconnected())
    }

    /// Send without waiting for a reply (id 0). Only queues, so it is safe on the UI thread; the writer thread does the
    /// blocking. A peer that never drains its queue ends the connection rather than losing input.
    fn notify(&self, req: Request) {
        if self.shared.broken().is_some() {
            return;
        }
        if let Err(e) = self.shared.post(0, &req)
            && e.kind() == std::io::ErrorKind::WouldBlock
        {
            self.shared.stalled();
        }
    }

    fn start(&self, req: Request, waiter: Waiter) -> Result<u32> {
        let id = loop {
            let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
            if id != 0 {
                break id; // 0 is reserved for events
            }
        };
        self.shared.waiters.locked().insert(id, waiter);
        if let Err(e) = self.send(id, req) {
            self.shared.waiters.locked().remove(&id);
            return Err(e);
        }
        Ok(id)
    }

    /// Pin the timeout of all calls (seconds; 0: per-request-type default, 2 min for file operations).
    pub fn set_call_timeout_secs(&self, secs: u32) {
        self.shared.call_limit_secs.store(secs, Ordering::Relaxed);
    }

    /// Connection state changes.
    pub fn status_events(&self) -> mpsc::Receiver<ConnStatus> {
        let (tx, rx) = mpsc::channel();
        self.shared.status.locked().push(tx);
        rx
    }
}

/// Reader thread: dispatch frames; after a drop reconnect (if there is a connector)
/// and keep reading on the same thread.
fn reader_loop(shared: Arc<Shared>, mut reader: BufReader<Box<dyn Read + Send>>, connector: Option<Connector>, session: Option<String>) {
    loop {
        let why = loop {
            match proto::read_frame::<Response>(&mut reader) {
                Ok(Some(frame)) => shared.route(frame),
                Ok(None) => break "the agent closed the connection".to_string(),
                Err(e) => break e.to_string(),
            }
        };
        let (Some(connector), false) = (&connector, shared.closing.load(Ordering::Relaxed)) else {
            shared.fail(why);
            return;
        };
        // Pending calls get "no connection" right away; terminals stay open.
        *shared.broken.locked() = Some(format!("reconnecting ({why})"));
        shared.clear_writer();
        shared.waiters.locked().clear();
        if let Some(mut c) = shared.child.locked().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        let deadline = std::time::Instant::now() + reconnect_window();
        let mut delay = std::time::Duration::from_secs(1);
        let mut attempt = 0;
        let mut error = why;
        loop {
            attempt += 1;
            shared.emit(ConnStatus::Reconnecting { attempt, error: error.clone() });
            std::thread::sleep(delay);
            if shared.closing.load(Ordering::Relaxed) {
                shared.fail("closed".into());
                return;
            }
            match connector().and_then(|t| handshake(t, session.as_deref())) {
                Ok((info, r, w, mut child, stderr)) => {
                    // If the window closed during reconnect nobody owns the new connection: don't leak ssh or the session.
                    if shared.closing.load(Ordering::Relaxed) {
                        if let Some(c) = child.as_mut() {
                            let _ = c.kill();
                            let _ = c.wait();
                        }
                        shared.fail("closed".into());
                        return;
                    }
                    if let Err(e) = shared.set_writer(w) {
                        if let Some(c) = child.as_mut() {
                            let _ = c.kill();
                            let _ = c.wait();
                        }
                        error = e.to_string();
                        continue;
                    }
                    *shared.child.locked() = child;
                    *shared.stderr.locked() = stderr;
                    if !info.resumed {
                        // The server lost the session: terminals are gone.
                        for (_, tx) in shared.ptys.locked().drain() {
                            let _ = tx.send(PtyEvent::Exit(None));
                        }
                    }
                    shared.end_procs();
                    *shared.broken.locked() = None;
                    if let Some(paths) = shared.last_watch.locked().clone() {
                        let _ = shared.post(0, &Request::Watch { paths });
                    }
                    shared.emit(ConnStatus::Connected { resumed: info.resumed });
                    reader = r;
                    break;
                }
                Err(e) => {
                    error = e.to_string();
                    if std::time::Instant::now() >= deadline {
                        shared.fail(format!("gave up reconnecting: {error}"));
                        shared.emit(ConnStatus::Lost(error));
                        return;
                    }
                    delay = (delay * 2).min(std::time::Duration::from_secs(15));
                }
            }
        }
    }
}

impl Host for RemoteHost {
    fn info(&self) -> &HostInfo {
        &self.info
    }
    fn label(&self) -> String {
        self.label.clone()
    }
    fn is_remote(&self) -> bool {
        self.remote
    }

    fn call(&self, req: Request) -> Result<Response> {
        let limit = call_timeout(&req, self.shared.call_limit_secs.load(Ordering::Relaxed));
        let (tx, rx) = mpsc::channel();
        let id = self.start(req, Waiter::Once(tx))?;
        match rx.recv_timeout(limit) {
            Ok(r) => Ok(r),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(self.disconnected()),
            // No reply: hung agent or very slow disk. Drop the pending entry (a late reply is discarded);
            // the connection stays up and the caller gets an error it can retry.
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.shared.waiters.locked().remove(&id);
                Err(HostError::Op(proto::Error::Other(format!("the agent did not answer within {} seconds", limit.as_secs()))))
            }
        }
    }

    fn search(&self, query: SearchQuery) -> Result<SearchStream> {
        let (tx, rx) = mpsc::channel();
        let id = self.start(Request::Search(query), Waiter::Stream(tx))?;
        Ok(SearchStream { id, rx })
    }

    fn hash_tree(&self, root: &str, ignore: Vec<String>, gitignore: bool, limit: u32, only: Option<Vec<String>>) -> Result<HashStream> {
        let (tx, rx) = mpsc::channel();
        let id = self.start(Request::HashTree { root: root.into(), ignore, gitignore, limit, only }, Waiter::Stream(tx))?;
        Ok(HashStream { id, rx })
    }

    fn cancel(&self, id: u32) {
        self.notify(Request::Cancel { id });
    }

    fn pty_spawn(&self, cols: u16, rows: u16, cwd: Option<String>, shell: Option<String>) -> Result<Pty> {
        let (tx, rx) = mpsc::channel();
        let (ev_tx, ev_rx) = mpsc::channel();
        self.start(Request::PtySpawn { cols, rows, cwd, shell }, Waiter::PtySpawn(tx, ev_tx))?;
        match rx.recv().map_err(|_| self.disconnected())? {
            Response::PtySpawned { pty } => Ok(Pty { id: pty, events: ev_rx }),
            other => Err(unexpected(other)),
        }
    }

    fn pty_input(&self, pty: u32, data: Vec<u8>) {
        self.notify(Request::PtyInput { pty, data });
    }

    fn pty_resize(&self, pty: u32, cols: u16, rows: u16) {
        self.notify(Request::PtyResize { pty, cols, rows });
    }

    fn pty_kill(&self, pty: u32) {
        self.notify(Request::PtyKill { pty });
    }

    fn proc_spawn(&self, program: &str, args: Vec<String>, cwd: Option<String>) -> Result<Proc> {
        let (tx, rx) = mpsc::channel();
        let (ev_tx, ev_rx) = mpsc::channel();
        self.start(Request::ProcSpawn { program: program.into(), args, cwd }, Waiter::ProcSpawn(tx, ev_tx))?;
        match rx.recv().map_err(|_| self.disconnected())? {
            Response::ProcSpawned { proc } => Ok(Proc { id: proc, events: ev_rx }),
            other => Err(unexpected(other)),
        }
    }

    fn proc_input(&self, proc: u32, data: Vec<u8>) {
        self.notify(Request::ProcInput { proc, data });
    }

    fn proc_kill(&self, proc: u32) {
        self.notify(Request::ProcKill { proc });
    }

    fn watch(&self, paths: Vec<String>) {
        *self.shared.last_watch.locked() = Some(paths.clone());
        self.notify(Request::Watch { paths });
    }

    fn fs_events(&self) -> mpsc::Receiver<FsChanges> {
        let (tx, rx) = mpsc::channel();
        *self.shared.fs.locked() = Some(tx);
        rx
    }

    fn status_events(&self) -> mpsc::Receiver<ConnStatus> {
        RemoteHost::status_events(self)
    }
}

/// Longest wait per request: ordinary file ops get 2 minutes; large-output or long jobs (commands,
/// big file scans) get more, in line with the agent's own limits (10 minutes).
fn call_timeout(req: &Request, override_secs: u32) -> std::time::Duration {
    if override_secs > 0 {
        return std::time::Duration::from_secs(u64::from(override_secs));
    }
    match req {
        Request::Exec { .. } | Request::LineIndex { .. } | Request::ReadLines { .. } => std::time::Duration::from_secs(15 * 60),
        _ => std::time::Duration::from_secs(120),
    }
}

impl Drop for RemoteHost {
    fn drop(&mut self) {
        // Closing on purpose: tell the agent so the daemon closes shells and processes now instead of holding the session "detached" for 5 minutes.
        // (Best effort: ignored if the connection is already down.)
        self.shared.closing.store(true, Ordering::Relaxed); // first, so the reader doesn't try to reconnect when the agent closes
        // Waited for (briefly), because the child is killed right below and would take an unsent Bye with it.
        if let Ok(w) = self.shared.writer()
            && let Ok(bytes) = Shared::encode(0, &Request::Bye)
        {
            let _ = w.send_within(bytes, std::time::Duration::from_secs(1));
        }
        self.shared.clear_writer();
        if let Some(mut child) = self.shared.child.locked().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The heartbeat primitive: a live agent answers `Ping`, a silent one does not (and its waiter is removed again).
    #[test]
    fn ping_tells_a_live_agent_from_a_silent_one() {
        let live = piped_agent();
        assert!(live.shared.ping(std::time::Duration::from_secs(5)));
        assert!(live.shared.waiters.lock().unwrap().is_empty());

        let (req_r, req_w) = std::io::pipe().unwrap();
        let (resp_r, resp_w) = std::io::pipe().unwrap();
        std::thread::spawn(move || {
            let mut r = BufReader::new(req_r);
            let mut w = resp_w;
            while let Ok(Some(Frame { id, body })) = proto::read_frame::<Request>(&mut r) {
                if let Request::Hello { client_version, .. } = body {
                    let info =
                        proto::HostInfo { version: client_version, os: "t".into(), arch: "t".into(), home: "/".into(), hostname: "t".into(), resumed: false };
                    let _ = proto::write_frame(&mut w, &Frame { id, body: Response::Hello(info) });
                }
            }
        });
        let silent = RemoteHost::from_streams(Box::new(resp_r), Box::new(req_w), "silent").unwrap();
        let started = std::time::Instant::now();
        assert!(!silent.shared.ping(std::time::Duration::from_millis(300)));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert!(silent.shared.waiters.lock().unwrap().is_empty(), "no waiter is leaked by a missed ping");
    }

    /// An agent that answers the handshake and then goes silent: the call returns an error instead of hanging forever,
    /// and the connection is still usable afterwards.
    #[test]
    fn a_silent_agent_makes_a_call_fail_instead_of_hanging() {
        let (req_r, req_w) = std::io::pipe().unwrap();
        let (resp_r, resp_w) = std::io::pipe().unwrap();
        std::thread::spawn(move || {
            let mut r = BufReader::new(req_r);
            let mut w = resp_w;
            while let Ok(Some(Frame { id, body })) = proto::read_frame::<Request>(&mut r) {
                // Answers Hello and Stat of "/ok"; swallows everything else.
                match body {
                    Request::Hello { client_version, .. } => {
                        let _ = proto::write_frame(
                            &mut w,
                            &Frame {
                                id,
                                body: Response::Hello(proto::HostInfo {
                                    version: client_version,
                                    os: "test".into(),
                                    arch: "test".into(),
                                    home: "/".into(),
                                    hostname: "t".into(),
                                    resumed: false,
                                }),
                            },
                        );
                    }
                    Request::Stat { path } if path == "/ok" => {
                        let meta = proto::Meta { kind: proto::Kind::File, size: 1, mtime_ms: 0, readonly: false };
                        let _ = proto::write_frame(&mut w, &Frame { id, body: Response::Stat(meta) });
                    }
                    _ => {}
                }
            }
        });
        let host = RemoteHost::from_streams(Box::new(resp_r), Box::new(req_w), "silent").unwrap();
        host.set_call_timeout_secs(1);
        let started = std::time::Instant::now();
        let err = host.call(Request::ReadDir { path: "/hang".into() }).unwrap_err();
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "{:?}", started.elapsed());
        assert!(err.to_string().contains("did not answer"), "{err}");
        assert!(host.shared.waiters.lock().unwrap().is_empty(), "the abandoned waiter was removed");
        assert!(matches!(host.call(Request::Stat { path: "/ok".into() }), Ok(Response::Stat(_))), "the connection still works");
    }

    /// Closing a host tells the agent (`Bye`): its session ends at once.
    // Unix only: it opens a pty running `/bin/sh`. The ConPTY side has its own tests below.
    #[cfg(unix)]
    #[test]
    fn dropping_a_host_ends_the_agent_session_immediately() {
        let (req_r, req_w) = std::io::pipe().unwrap();
        let (resp_r, resp_w) = std::io::pipe().unwrap();
        let ended = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = ended.clone();
        std::thread::spawn(move || {
            let _ = agent::serve(req_r, resp_w);
            flag.store(true, Ordering::Relaxed);
        });
        let host = RemoteHost::from_streams(Box::new(resp_r), Box::new(req_w), "bye").unwrap();
        let pty = host.pty_spawn(80, 24, None, Some("/bin/sh".into())).unwrap();
        drop(pty);
        drop(host);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !ended.load(Ordering::Relaxed) {
            assert!(std::time::Instant::now() < deadline, "the agent session did not end after the host was dropped");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// Two pipes and a thread instead of a real process: end-to-end protocol test.
    fn piped_agent() -> RemoteHost {
        let (req_r, req_w) = std::io::pipe().unwrap();
        let (resp_r, resp_w) = std::io::pipe().unwrap();
        std::thread::spawn(move || agent::serve(req_r, resp_w));
        RemoteHost::from_streams(Box::new(resp_r), Box::new(req_w), "test").unwrap()
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("codeyard-host-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn remote_and_local_behave_the_same() {
        let dir = tmp("same");
        let path = dir.join("a.rs").to_string_lossy().into_owned();

        let remote = piped_agent();
        let local = local();
        assert_eq!(remote.info().version, proto::VERSION);
        assert!(!local.is_remote());

        let h1 = remote.write(&path, b"fn main() {}\n".to_vec(), None).unwrap();
        let (data, h2) = local.read(&path).unwrap();
        assert_eq!((data.as_slice(), h1), (&b"fn main() {}\n"[..], h2));

        // Modify locally, then try writing remotely with the old hash: conflict.
        local.write(&path, b"// degisti\n".to_vec(), Some(h2)).unwrap();
        let err = remote.write(&path, b"x".to_vec(), Some(h1)).unwrap_err();
        assert!(matches!(err, HostError::Op(proto::Error::Conflict { .. })), "{err}");

        let names: Vec<_> = remote.read_dir(&dir.to_string_lossy()).unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["a.rs"]);
    }

    #[test]
    fn concurrent_calls_are_matched_by_id() {
        let dir = tmp("concurrent");
        for i in 0..20 {
            std::fs::write(dir.join(format!("{i}.txt")), i.to_string()).unwrap();
        }
        let host = Arc::new(piped_agent());
        let handles: Vec<_> = (0..20)
            .map(|i| {
                let host = host.clone();
                let p = dir.join(format!("{i}.txt")).to_string_lossy().into_owned();
                std::thread::spawn(move || host.read(&p).unwrap().0)
            })
            .collect();
        for (i, h) in handles.into_iter().enumerate() {
            assert_eq!(h.join().unwrap(), i.to_string().into_bytes(), "replies must not get mixed up");
        }
    }

    #[test]
    fn search_streams_while_other_calls_proceed() {
        let dir = tmp("search");
        std::fs::write(dir.join("a.txt"), "iğne\n").unwrap();
        std::fs::write(dir.join("b.txt"), "saman İĞNE\n").unwrap();
        let host = local();
        let stream = host
            .search(SearchQuery {
                root: dir.to_string_lossy().into_owned(),
                pattern: "iğne".into(),
                regex: false,
                case_sensitive: false,
                whole_word: false,
                include: vec![],
                exclude: vec![],
                max_results: 100,
            })
            .unwrap();
        // Another call runs while a search is in progress.
        assert!(host.stat(&dir.to_string_lossy()).is_ok());
        let mut files = Vec::new();
        for r in stream.rx.iter() {
            match r {
                Response::SearchChunk(fm) => files.extend(fm.into_iter().map(|f| f.path)),
                Response::SearchDone { matches, .. } => {
                    assert_eq!(matches, 2);
                    break;
                }
                other => panic!("{other:?}"),
            }
        }
        files.sort();
        assert_eq!(files, ["a.txt", "b.txt"]);
    }

    #[test]
    fn hash_tree_streams_records_then_done_and_hashes_on_request() {
        let dir = tmp("hashtree");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/a.txt"), "iğne").unwrap();
        let root = dir.to_string_lossy().into_owned();
        let host = local();
        let collect = |only: Option<Vec<String>>| {
            let stream = host.hash_tree(&root, vec![], true, 100, only).unwrap();
            let mut records = Vec::new();
            loop {
                match stream.next().expect("the stream must end with Done") {
                    HashEvent::Records(r) => records.extend(r),
                    HashEvent::Done { entries, .. } => return (records, entries),
                    HashEvent::Failed(e) => panic!("{e}"),
                }
            }
        };
        let (listing, entries) = collect(None);
        assert_eq!((listing.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(), entries), (vec!["sub", "sub/a.txt"], 2));
        assert!(listing.iter().all(|r| r.hash.is_none()));
        let (hashed, _) = collect(Some(vec!["sub/a.txt".into()]));
        assert_eq!(hashed[0].hash, Some(proto::content_hash("iğne".as_bytes())));
        let stream = host.hash_tree("/definitely/not/here", vec![], true, 10, None).unwrap();
        assert!(matches!(stream.next(), Some(HashEvent::Failed(proto::Error::NotFound(_)))));
    }

    #[cfg(unix)]
    #[test]
    fn pty_roundtrip_through_host() {
        let host = local();
        let pty = host.pty_spawn(80, 24, None, Some("/bin/sh".into())).unwrap();
        host.pty_input(pty.id, b"printf 'merhaba\\n'; exit 0\n".to_vec());
        let mut out = Vec::new();
        loop {
            match pty.events.recv_timeout(std::time::Duration::from_secs(10)).expect("output expected") {
                PtyEvent::Output(d) => out.extend(d),
                PtyEvent::Exit(code) => {
                    assert_eq!(code, Some(0));
                    break;
                }
            }
        }
        assert!(String::from_utf8_lossy(&out).contains("merhaba"));
    }

    #[test]
    fn watch_reports_external_changes() {
        let dir = tmp("watch");
        let f = dir.join("a.txt");
        std::fs::write(&f, "1").unwrap();
        let host = local();
        let events = host.fs_events();
        let key = f.to_string_lossy().into_owned();
        host.watch(vec![key.clone()]);
        std::thread::sleep(std::time::Duration::from_millis(100));
        std::fs::write(&f, "dışarıdan").unwrap();
        let batch = events.recv_timeout(std::time::Duration::from_secs(5)).expect("change event");
        assert_eq!(batch, vec![(key, ChangeKind::Modified)]);
    }

    /// Server-side writer: writes fail once the switch is off (network drop).
    #[cfg(unix)]
    struct Killable {
        inner: std::io::PipeWriter,
        dead: Arc<std::sync::atomic::AtomicBool>,
    }

    #[cfg(unix)]
    impl Write for Killable {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            if self.dead.load(Ordering::Relaxed) {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            self.inner.write(b)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.inner.flush()
        }
    }

    #[cfg(unix)]
    #[test]
    fn reconnects_and_terminal_continues() {
        let registry = Arc::new(agent::daemon::Registry::default());
        let switch: Arc<Mutex<Arc<std::sync::atomic::AtomicBool>>> = Arc::default();
        let attempts = Arc::new(AtomicU32::new(0));
        let (reg, sw, att) = (registry.clone(), switch.clone(), attempts.clone());
        let connector: Connector = Arc::new(move || {
            att.fetch_add(1, Ordering::Relaxed);
            let (req_r, req_w) = std::io::pipe().unwrap();
            let (resp_r, resp_w) = std::io::pipe().unwrap();
            let dead = Arc::new(std::sync::atomic::AtomicBool::new(false));
            *sw.lock().unwrap() = dead.clone();
            let reg = reg.clone();
            std::thread::spawn(move || reg.serve_connection(req_r, Killable { inner: resp_w, dead }));
            Ok(Transport::pipes(Box::new(resp_r), Box::new(req_w)))
        });
        let host = RemoteHost::connect(connector, "pencere-1".into(), "test").unwrap();
        let status = host.status_events();
        let pty = host.pty_spawn(80, 24, None, Some("/bin/sh".into())).unwrap();
        // Piped process like a language server: can't survive a drop, gets terminated.
        let lsp = host.proc_spawn("cat", Vec::new(), None).unwrap();

        // Network drops; then a command that produces output (the write error triggers the drop).
        switch.lock().unwrap().store(true, Ordering::Relaxed);
        host.pty_input(pty.id, b"echo koptuktan-sonra\n".to_vec());

        let wait = std::time::Duration::from_secs(15);
        assert!(matches!(status.recv_timeout(wait).unwrap(), ConnStatus::Reconnecting { .. }));
        assert!(matches!(host.stat("/"), Err(HostError::Disconnected(_))), "call must not hang while disconnected");
        assert_eq!(status.recv_timeout(wait).unwrap(), ConnStatus::Connected { resumed: true });
        assert!(attempts.load(Ordering::Relaxed) >= 2);

        // Same terminal channel: output produced while disconnected and the new command arrive.
        host.pty_input(pty.id, b"echo yeniden-bagli\n".to_vec());
        let mut out = String::new();
        let deadline = std::time::Instant::now() + wait;
        while !(out.contains("koptuktan-sonra") && out.contains("yeniden-bagli")) {
            assert!(std::time::Instant::now() < deadline, "terminal output: {out}");
            if let Ok(PtyEvent::Output(d)) = pty.events.recv_timeout(std::time::Duration::from_millis(200)) {
                out.push_str(&String::from_utf8_lossy(&d));
            }
        }
        assert!(host.stat("/").is_ok(), "connection works again");

        // The piped process was reported as exited (the client restarts it); the new one runs.
        let mut exited = false;
        while let Ok(ev) = lsp.events.recv_timeout(std::time::Duration::from_millis(500)) {
            if ev == PtyEvent::Exit(None) {
                exited = true;
            }
        }
        assert!(exited, "dropped process must get Exit(None)");
        let again = host.proc_spawn("cat", Vec::new(), None).unwrap();
        host.proc_input(again.id, b"tekrar\n".to_vec());
        assert_eq!(again.events.recv_timeout(wait).unwrap(), PtyEvent::Output(b"tekrar\n".to_vec()));
    }

    #[test]
    fn dead_agent_reports_disconnected() {
        let (req_r, req_w) = std::io::pipe().unwrap();
        let (resp_r, resp_w) = std::io::pipe().unwrap();
        let t = std::thread::spawn(move || {
            // Only answer the handshake, then die.
            let mut r = BufReader::new(req_r);
            let mut w = resp_w;
            let f: Frame<Request> = proto::read_frame(&mut r).unwrap().unwrap();
            proto::write_frame(&mut w, &Frame { id: f.id, body: agent::handle(f.body) }).unwrap();
        });
        let host = RemoteHost::from_streams(Box::new(resp_r), Box::new(req_w), "test").unwrap();
        t.join().unwrap();
        let err = host.read_dir("/").unwrap_err();
        assert!(matches!(err, HostError::Disconnected(_)), "{err}");
        // Later calls must fail the same way right away, not hang.
        assert!(matches!(host.stat("/"), Err(HostError::Disconnected(_))));
    }

    #[test]
    #[cfg(unix)]
    fn exec_and_piped_process() {
        let h = piped_agent();
        let dir = tmp("exec");
        // stdin + cwd; the program name is looked up with the login shell.
        let out = h.exec("cat", &[], Some(&dir.to_string_lossy()), Some("merhaba\n".into())).unwrap();
        assert!(out.ok());
        assert_eq!(out.stdout_str(), "merhaba\n");
        let out = h.exec("pwd", &[], Some(&dir.to_string_lossy()), None).unwrap();
        assert!(out.stdout_str().trim().ends_with(dir.file_name().unwrap().to_str().unwrap()));
        let out = h.exec("codeyard-boyle-bir-program-yok", &[], None, None).unwrap();
        assert_eq!(out.code, Some(127), "shell could not find it: {}", out.stderr_str());
        let out = h.exec("/yok/program", &[], None, None);
        assert!(matches!(out, Err(HostError::Op(proto::Error::NotFound(_)))));

        // Long-running process: write, read, close.
        let p = h.proc_spawn("cat", Vec::new(), None).unwrap();
        h.proc_input(p.id, b"ping\n".to_vec());
        match p.events.recv_timeout(std::time::Duration::from_secs(5)).unwrap() {
            ProcEvent::Output(d) => assert_eq!(d, b"ping\n"),
            other => panic!("{other:?}"),
        }
        h.proc_kill(p.id);
        loop {
            match p.events.recv_timeout(std::time::Duration::from_secs(5)).unwrap() {
                ProcEvent::Exit(_) => break,
                ProcEvent::Output(_) => {}
            }
        }
    }

    /// Fault injection, slow consumer of a process (this is what a language server or debug adapter is): nothing may be
    /// dropped and nothing may be reordered, and the connection reader must keep serving every other call, because it is
    /// shared by all calls, terminals, searches and the heartbeat. Blocking it to bound the queue would stall all of those.
    #[test]
    #[cfg(unix)]
    fn a_stalled_process_consumer_loses_nothing_and_does_not_block_other_calls() {
        const TOTAL: usize = 4 * 1024 * 1024;
        let h = piped_agent();
        let p = h.proc_spawn("cat", Vec::new(), None).unwrap();
        let pattern = |i: usize| (i % 251) as u8;
        let mut sent = 0;
        while sent < TOTAL {
            let n = (64 * 1024).min(TOTAL - sent);
            h.proc_input(p.id, (sent..sent + n).map(pattern).collect());
            sent += n;
        }
        // Give the echo time to pile up unread, then prove other calls are served meanwhile.
        std::thread::sleep(std::time::Duration::from_millis(800));
        let t = std::time::Instant::now();
        assert!(h.stat("/").is_ok());
        assert!(t.elapsed() < std::time::Duration::from_secs(2), "a stat took {:?} behind a stalled process consumer", t.elapsed());
        // The slow consumer finally reads: every byte, in order.
        let mut got = 0usize;
        while got < TOTAL {
            match p.events.recv_timeout(std::time::Duration::from_secs(20)).expect("output keeps coming") {
                ProcEvent::Output(d) => {
                    assert!(d.iter().enumerate().all(|(i, b)| *b == pattern(got + i)), "bytes reordered or damaged at {got}");
                    got += d.len();
                }
                ProcEvent::Exit(c) => panic!("cat exited early: {c:?}"),
            }
        }
        assert_eq!(got, TOTAL);
        h.proc_kill(p.id);
    }

    #[cfg(windows)]
    #[test]
    fn windows_local_exec_and_piped_process() {
        let h = piped_agent();
        let dir = tmp("exec-şablon boşluk");
        let out = h.exec("git", &["--version"], Some(&dir.to_string_lossy()), None).unwrap();
        assert!(out.ok(), "{}", out.stderr_str());
        assert!(out.stdout_str().starts_with("git version"));
        assert!(matches!(h.exec("codeyard-no-such-program", &[], None, None), Err(HostError::Op(proto::Error::NotFound(_)))));
        let p = h.proc_spawn("cmd.exe", vec!["/d".into(), "/q".into(), "/c".into(), "more".into()], Some(dir.to_string_lossy().into_owned())).unwrap();
        h.proc_input(p.id, b"pipe-marker\r\n".to_vec());
        let mut output = Vec::new();
        while !String::from_utf8_lossy(&output).contains("pipe-marker") {
            match p.events.recv_timeout(std::time::Duration::from_secs(5)).expect("piped output") {
                ProcEvent::Output(data) => output.extend(data),
                other => panic!("{other:?}"),
            }
        }
        h.proc_kill(p.id);
        while !matches!(p.events.recv_timeout(std::time::Duration::from_secs(5)).expect("process exit"), ProcEvent::Exit(_)) {}
    }

    #[cfg(windows)]
    #[test]
    fn windows_conpty_roundtrip_resize_and_exit() {
        let h = piped_agent();
        let dir = tmp("terminal-şablon boşluk");
        let p = h.pty_spawn(80, 24, Some(dir.to_string_lossy().into_owned()), Some("cmd.exe".into())).unwrap();
        h.pty_resize(p.id, 120, 35);
        let mut output = Vec::new();
        let mut sent = false;
        loop {
            match p
                .events
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap_or_else(|e| panic!("ConPTY output/exit: {e}: {}", String::from_utf8_lossy(&output)))
            {
                PtyEvent::Output(data) => {
                    output.extend(data);
                    // ConPTY asks the terminal emulator for its initial cursor position.
                    if !sent && output.windows(4).any(|s| s == b"\x1b[6n") {
                        h.pty_input(p.id, b"\x1b[1;1R".to_vec());
                        h.pty_input(
                            p.id,
                            "chcp 65001 >nul\r\necho __CY_%TERM_PROGRAM%__\r\necho Türkçe: ıİşğüöç\r\necho %CD%\r\nexit 7\r\n".as_bytes().to_vec(),
                        );
                        sent = true;
                    }
                }
                PtyEvent::Exit(code) => {
                    assert_eq!(code, Some(7), "{}", String::from_utf8_lossy(&output));
                    break;
                }
            }
        }
        let text = String::from_utf8_lossy(&output);
        assert!(text.contains("__CY_codeyard__"), "{text}");
        assert!(text.contains("Türkçe: ıİşğüöç"), "{text}");
        assert!(text.contains(&*dir.to_string_lossy()), "{text}");
    }

    #[test]
    fn fs_changes_of_a_stalled_consumer_merge_per_path() {
        use ChangeKind::*;
        let mut pending: FsChanges = Vec::new();
        merge_fs_changes(&mut pending, vec![("a".into(), Created), ("b".into(), Modified), ("c".into(), Removed)]);
        merge_fs_changes(&mut pending, vec![("a".into(), Modified), ("b".into(), Removed), ("c".into(), Created), ("d".into(), Created)]);
        assert_eq!(pending, vec![("a".into(), Created), ("b".into(), Removed), ("c".into(), Modified), ("d".into(), Created)]);

        // Fault injection: the consumer never reads while the agent keeps reporting its 2048 watched paths every second
        // for hours. The backlog is the watched set, not the number of reports.
        let mut stalled: FsChanges = Vec::new();
        for round in 0..20_000u32 {
            let kind = if round % 2 == 0 { Modified } else { Removed };
            merge_fs_changes(&mut stalled, (0..64).map(|i| (format!("/w/{i}"), kind)).collect());
        }
        assert_eq!(stalled.len(), 64);
        assert!(stalled.iter().all(|(_, k)| *k == Removed), "the last report wins");
    }

    /// A writer that blocks in `write` until the gate opens, then forwards to `inner`; `fail` makes later writes error.
    struct Gated {
        inner: Box<dyn Write + Send>,
        gate: Arc<(Mutex<bool>, std::sync::Condvar)>,
        fail: Arc<std::sync::atomic::AtomicBool>,
    }

    impl Write for Gated {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            let (open, cv) = &*self.gate;
            let mut open = open.locked();
            while !*open {
                open = cv.wait(open).unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            drop(open);
            if self.fail.load(Ordering::Relaxed) {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            self.inner.write(b)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.inner.flush()
        }
    }

    type Gate = Arc<(Mutex<bool>, std::sync::Condvar)>;

    fn open_gate(gate: &Gate) {
        *gate.0.locked() = true;
        gate.1.notify_all();
    }

    /// A host over a real agent whose request pipe goes through a gate (open for the handshake, closed afterwards).
    fn gated_agent() -> (RemoteHost, Gate, Arc<std::sync::atomic::AtomicBool>) {
        let (req_r, req_w) = std::io::pipe().unwrap();
        let (resp_r, resp_w) = std::io::pipe().unwrap();
        std::thread::spawn(move || agent::serve(req_r, resp_w));
        let gate: Gate = Arc::new((Mutex::new(true), std::sync::Condvar::new()));
        let fail = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let w = Gated { inner: Box::new(req_w), gate: gate.clone(), fail: fail.clone() };
        let host = RemoteHost::from_streams(Box::new(resp_r), Box::new(w), "gated").unwrap();
        *gate.0.locked() = false;
        (host, gate, fail)
    }

    /// The UI thread only queues: with a 30 MB write stuck in the pipe, notifications return at once, and everything goes out
    /// in order once the peer reads again.
    #[test]
    fn notify_does_not_wait_for_a_stuck_write() {
        let dir = tmp("notify-stuck");
        let path = dir.join("big.bin").to_string_lossy().into_owned();
        let (host, gate, _) = gated_agent();
        let host = Arc::new(host);
        let big = {
            let host = host.clone();
            std::thread::spawn(move || host.write(&path, vec![7u8; 30 * 1024 * 1024], None))
        };
        std::thread::sleep(std::time::Duration::from_millis(200));

        let started = std::time::Instant::now();
        host.pty_resize(1, 80, 24);
        host.proc_input(1, b"didChange".to_vec());
        host.cancel(99);
        assert!(started.elapsed() < std::time::Duration::from_millis(50), "{:?}", started.elapsed());
        // The heartbeat probe must not wait on the stuck write either.
        let started = std::time::Instant::now();
        assert!(!host.shared.ping(std::time::Duration::from_millis(100)));
        assert!(started.elapsed() < std::time::Duration::from_secs(2));

        open_gate(&gate);
        assert!(big.join().unwrap().is_ok());
        assert!(host.stat(&dir.to_string_lossy()).is_ok(), "the connection works after the stall");
    }

    /// Frames reach the peer in the order they were queued, notifications and requests alike.
    #[test]
    fn queued_frames_keep_their_order() {
        let (req_r, req_w) = std::io::pipe().unwrap();
        let (resp_r, resp_w) = std::io::pipe().unwrap();
        let (seen_tx, seen_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut r = BufReader::new(req_r);
            let mut w = resp_w;
            while let Ok(Some(Frame { id, body })) = proto::read_frame::<Request>(&mut r) {
                match body {
                    Request::Hello { client_version, .. } => {
                        let info = proto::HostInfo {
                            version: client_version,
                            os: "t".into(),
                            arch: "t".into(),
                            home: "/".into(),
                            hostname: "t".into(),
                            resumed: false,
                        };
                        let _ = proto::write_frame(&mut w, &Frame { id, body: Response::Hello(info) });
                    }
                    Request::PtyInput { data, .. } => {
                        let _ = seen_tx.send(data);
                    }
                    Request::Stat { .. } => {
                        let _ = seen_tx.send(b"stat".to_vec());
                        let meta = proto::Meta { kind: proto::Kind::File, size: 0, mtime_ms: 0, readonly: false };
                        let _ = proto::write_frame(&mut w, &Frame { id, body: Response::Stat(meta) });
                    }
                    _ => {}
                }
            }
        });
        let host = RemoteHost::from_streams(Box::new(resp_r), Box::new(req_w), "order").unwrap();
        for i in 0..200u32 {
            host.pty_input(1, i.to_le_bytes().to_vec());
        }
        host.stat("/x").unwrap();
        for i in 200..210u32 {
            host.pty_input(1, i.to_le_bytes().to_vec());
        }
        let wait = std::time::Duration::from_secs(5);
        for i in 0..200u32 {
            assert_eq!(seen_rx.recv_timeout(wait).unwrap(), i.to_le_bytes().to_vec());
        }
        assert_eq!(seen_rx.recv_timeout(wait).unwrap(), b"stat".to_vec());
        for i in 200..210u32 {
            assert_eq!(seen_rx.recv_timeout(wait).unwrap(), i.to_le_bytes().to_vec());
        }
    }

    /// A write error reaches the caller that waited for it as `Disconnected`, and later calls fail the same way.
    #[test]
    fn a_write_error_surfaces_as_disconnected() {
        let (host, gate, fail) = gated_agent();
        fail.store(true, Ordering::Relaxed);
        open_gate(&gate);
        let err = host.stat("/").unwrap_err();
        assert!(matches!(err, HostError::Disconnected(_)), "{err}");
        assert!(matches!(host.stat("/"), Err(HostError::Disconnected(_))));
        host.pty_input(1, b"x".to_vec()); // does not panic or block
    }

    /// Input is never dropped quietly: when the peer has not read for far longer than the queue holds, the connection is
    /// given up on and callers see `Disconnected`.
    #[test]
    fn a_peer_that_never_reads_fails_the_host_instead_of_dropping_input() {
        let (host, gate, fail) = gated_agent();
        for _ in 0..40 {
            host.proc_input(1, vec![0u8; 2 * 1024 * 1024]);
        }
        assert!(host.shared.broken().is_some());
        assert!(matches!(host.stat("/"), Err(HostError::Disconnected(_))));
        // Let the stuck writer thread go (and fail) instead of leaving it parked.
        fail.store(true, Ordering::Relaxed);
        open_gate(&gate);
    }

    /// Dropping the host ends the writer thread: the stream it owns is dropped with it.
    #[test]
    fn the_writer_thread_ends_with_the_host() {
        struct Sink(Arc<std::sync::atomic::AtomicBool>);
        impl Write for Sink {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl Drop for Sink {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Relaxed);
            }
        }
        let released = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let w = outbox::Writer::start(BufWriter::new(Box::new(Sink(released.clone())))).unwrap();
        w.send(vec![1, 2, 3]).unwrap();
        assert!(!released.load(Ordering::Relaxed));
        drop(w);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !released.load(Ordering::Relaxed) {
            assert!(std::time::Instant::now() < deadline, "writer thread still alive");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
