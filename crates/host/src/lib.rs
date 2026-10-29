//! `Host`: the UI's single entry to the file system, search and terminals. Local
//! and remote speak the same framed protocol: the local agent runs on a thread
//! (over a pipe), the remote one over `ssh` stdio. One code path.
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod askpass;
pub mod forward;
mod outbox;
pub mod ssh;
pub mod ssh_config;
pub mod ssh_hosts;
pub mod transfer;

/// Resolve tools without requiring a terminal launch of the editor. Container tools
/// commonly install outside the GUI process's PATH.
pub fn find_program(program: &str) -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    #[cfg(windows)]
    {
        if let Some(p) = std::env::var_os("ProgramFiles") {
            let root = PathBuf::from(p);
            dirs.extend([root.join("Docker/Docker/resources/bin"), root.join("RedHat/Podman"), root.join("WSL")]);
        }
        if let Some(p) = std::env::var_os("SystemRoot") {
            dirs.push(PathBuf::from(p).join("System32"));
        }
        if let Some(p) = std::env::var_os("APPDATA") {
            dirs.push(PathBuf::from(p).join("npm"));
        }
    }
    #[cfg(unix)]
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/Applications/Docker.app/Contents/Resources/bin"].map(PathBuf::from));
    let extensions: &[&str] = if cfg!(windows) { &["", "exe", "com", "cmd", "bat"] } else { &[""] };
    dirs.into_iter()
        .flat_map(|dir| {
            extensions.iter().map(move |ext| {
                let path = dir.join(program);
                if ext.is_empty() || path.extension().is_some() { path } else { path.with_extension(ext) }
            })
        })
        .find(|p| {
            if !p.is_file() {
                return false;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                p.metadata().is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
            }
            #[cfg(not(unix))]
            {
                true
            }
        })
}

/// Platform-aware construction (login PATH, npm launchers, hidden Windows consoles).
pub fn process_command(program: &str, args: &[String], cwd: Option<&str>) -> std::io::Result<std::process::Command> {
    let path = find_program(program);
    agent::proc::command(path.as_ref().and_then(|p| p.to_str()).unwrap_or(program), args, cwd)
}

use std::collections::HashMap;
use std::io::{BufReader, BufWriter, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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
    /// A mutating request may still be running; the caller must inspect before retrying.
    Uncertain(String),
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostError::Op(e) => e.fmt(f),
            HostError::Disconnected(m) => write!(f, "not connected: {m}"),
            HostError::Uncertain(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for HostError {}

/// A terminal to open with a chosen program.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PtySpec {
    pub cols: u16,
    pub rows: u16,
    pub cwd: Option<String>,
    pub shell: Option<String>,
    /// `None`: a login shell.
    pub args: Option<Vec<String>>,
    pub env: Vec<(String, String)>,
    pub integration: bool,
}

/// What runs in a terminal (`Host::pty_status`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PtyStatus {
    /// A program other than the shell holds the terminal (`None`: unknown).
    pub busy: Option<bool>,
    pub name: Option<String>,
    pub cwd: Option<String>,
}

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

    /// A terminal running `shell` with `args` (instead of a login shell) and extra environment variables.
    fn pty_spawn_ex(&self, spec: PtySpec) -> Result<Pty> {
        self.pty_spawn(spec.cols, spec.rows, spec.cwd, spec.shell)
    }

    /// Whether a program holds the terminal's foreground, and the shell's folder.
    fn pty_status(&self, pty: u32) -> PtyStatus {
        match self.call(Request::PtyStatus { pty }) {
            Ok(Response::PtyStatus { busy, name, cwd }) => PtyStatus { busy, name, cwd },
            _ => PtyStatus::default(),
        }
    }

    /// The shells the machine offers.
    fn list_shells(&self) -> Vec<String> {
        match self.call(Request::ListShells) {
            Ok(Response::Shells(list)) => list,
            _ => Vec::new(),
        }
    }
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
    /// An extension's process: absolute `program`, only `env` on top of a minimal environment (`ProcSpawnEnv`).
    fn proc_spawn_env(&self, program: &str, args: Vec<String>, cwd: Option<String>, env: Vec<(String, String)>) -> Result<Proc> {
        let _ = (program, args, cwd, env);
        Err(HostError::Op(proto::Error::Unsupported("processes with an explicit environment".into())))
    }

    /// Where `program` is on the login shell's PATH (`Which`).
    fn which(&self, program: &str) -> Result<Option<String>> {
        match self.call(Request::Which { program: program.into() })? {
            Response::Path(p) => Ok(p),
            other => Err(unexpected(other)),
        }
    }
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

    /// Read a preview file beneath `root` in one call, without following symlinks.
    ///
    /// # Errors
    /// Returns the agent's path, file type, size or I/O error, or a transport error.
    fn read_preview(&self, root: &str, path: &str) -> Result<(Vec<u8>, u64)> {
        match self.call(Request::ReadPreview { root: root.into(), path: path.into() })? {
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

    fn copy_path(&self, from: &str, to: &str) -> Result<()> {
        self.done(Request::Copy { from: from.into(), to: to.into() })
    }

    fn copy_path_cancellable(&self, from: &str, to: &str, cancel: &AtomicBool) -> Result<()> {
        if cancel.load(Ordering::Acquire) {
            return Err(HostError::Op(proto::Error::Cancelled));
        }
        self.copy_path(from, to)
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
    /// Copies are cancellable mutations: after stopping, wait for the agent to
    /// remove its staging entry instead of reporting a completed cancellation early.
    fn copy_call(&self, req: Request, cancel: Option<&AtomicBool>) -> Result<Response> {
        if cancel.is_some_and(|c| c.load(Ordering::Acquire)) {
            return Err(HostError::Op(proto::Error::Cancelled));
        }
        let limit = call_timeout(&req, self.shared.call_limit_secs.load(Ordering::Relaxed));
        let started = std::time::Instant::now();
        let (tx, rx) = mpsc::channel();
        let id = self.start(req, Waiter::Once(tx))?;
        let mut stopping = None;
        loop {
            if stopping.is_none() && (started.elapsed() >= limit || cancel.is_some_and(|c| c.load(Ordering::Acquire))) {
                self.cancel(id);
                stopping = Some(std::time::Instant::now());
            }
            match rx.recv_timeout(std::time::Duration::from_millis(50)) {
                Ok(response) => return Ok(response),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(HostError::Uncertain("the connection was lost during the copy; inspect the source and destination before retrying".into()));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if stopping.is_some_and(|at| at.elapsed() >= std::time::Duration::from_secs(5)) {
                self.shared.waiters.locked().remove(&id);
                return Err(HostError::Uncertain("the copy has not confirmed cancellation; inspect the source and destination before retrying".into()));
            }
        }
    }

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
        if matches!(req, Request::Copy { .. }) {
            return self.copy_call(req, None);
        }
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

    fn copy_path_cancellable(&self, from: &str, to: &str, cancel: &AtomicBool) -> Result<()> {
        match self.copy_call(Request::Copy { from: from.into(), to: to.into() }, Some(cancel))? {
            Response::Done => Ok(()),
            Response::Error(e) => Err(HostError::Op(e)),
            other => Err(unexpected(other)),
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

    fn pty_spawn_ex(&self, spec: PtySpec) -> Result<Pty> {
        let (tx, rx) = mpsc::channel();
        let (ev_tx, ev_rx) = mpsc::channel();
        let PtySpec { cols, rows, cwd, shell, args, env, integration } = spec;
        self.start(Request::PtySpawnEx { cols, rows, cwd, shell, args, env, integration }, Waiter::PtySpawn(tx, ev_tx))?;
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

    fn proc_spawn_env(&self, program: &str, args: Vec<String>, cwd: Option<String>, env: Vec<(String, String)>) -> Result<Proc> {
        let (tx, rx) = mpsc::channel();
        let (ev_tx, ev_rx) = mpsc::channel();
        self.start(Request::ProcSpawnEnv { program: program.into(), args, cwd, env }, Waiter::ProcSpawn(tx, ev_tx))?;
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
        Request::Exec { .. } | Request::LineIndex { .. } | Request::ReadLines { .. } | Request::Copy { .. } => std::time::Duration::from_secs(15 * 60),
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
mod copy_tests;
#[cfg(test)]
mod tests;
