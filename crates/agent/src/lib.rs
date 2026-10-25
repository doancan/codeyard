//! Agent: file system, search, terminal. Locally, `host::local()` runs this session
//! on a thread over a pipe; remotely, as `codeyard-agent --stdio`.
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod bigfile;
pub mod daemon;
pub mod proc;
pub mod pty;
pub mod search;
pub mod watch;

use std::collections::HashMap;
use std::fs;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use proto::{DirEntry, Error, Frame, HostInfo, Kind, Meta, Request, Response, content_hash};

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

pub fn handle(req: Request) -> Response {
    let result = match req {
        Request::Hello { client_version, .. } => hello(client_version, false),
        Request::ReadDir { path } => read_dir(&expand(&path)).map(Response::Entries),
        Request::Stat { path } => stat(&expand(&path)).map(Response::Stat),
        Request::Read { path } => read(&expand(&path)),
        Request::Write { path, data, expect_hash } => write(&expand(&path), &data, expect_hash).map(|hash| Response::Written { hash }),
        Request::CreateFile { path } => create_file(&expand(&path)).map(|_| Response::Done),
        Request::CreateDir { path } => {
            let p = expand(&path);
            // An existing directory is an error; missing parent directories are created ("a/b/c").
            if fs::symlink_metadata(&p).is_ok() {
                return Response::Error(Error::AlreadyExists(p.display().to_string()));
            }
            fs::create_dir_all(&p).map(|_| Response::Done).map_err(|e| map_err(&p, e))
        }
        Request::Rename { from, to } => rename(&expand(&from), &expand(&to)).map(|_| Response::Done),
        Request::Delete { path } => delete(&expand(&path)).map(|_| Response::Done),
        // Streaming requests are handled inside `serve`; here only one-shot responses.
        Request::Search(_)
        | Request::Cancel { .. }
        | Request::PtySpawn { .. }
        | Request::PtyInput { .. }
        | Request::PtyResize { .. }
        | Request::PtyKill { .. }
        | Request::Watch { .. }
        | Request::Bye
        | Request::Ping
        | Request::Exec { .. }
        | Request::ProcSpawn { .. }
        | Request::ProcInput { .. }
        | Request::ProcKill { .. }
        | Request::LineIndex { .. }
        | Request::ReadLines { .. } => Err(Error::Unsupported("streaming request outside a session".into())),
        Request::ListFiles { root, limit } => list_files(&expand(&root), limit as usize).map(|(paths, truncated)| Response::Files { paths, truncated }),
    };
    result.unwrap_or_else(Response::Error)
}

/// Maximum output kept per terminal while disconnected (the tail is kept).
const PTY_BACKLOG: usize = 256 * 1024;

/// The session's connection endpoint. While disconnected (daemon mode, ssh dropped) terminal output, shell exits
/// and file changes are buffered and replayed in order on reconnect. Other responses are dropped (the client's
/// pending requests already got "no connection").
#[derive(Default)]
struct Link {
    inner: Mutex<LinkState>,
}

#[derive(Default)]
struct LinkState {
    writer: Option<BufWriter<Box<dyn Write + Send>>>,
    /// Increments on every connect: so a stale connection's drop can't tear down the new one.
    generation: u64,
    backlog: Vec<(u32, Vec<u8>)>,
    exits: Vec<(u32, Option<i32>)>,
    changes: Vec<(String, proto::ChangeKind)>,
}

type Out = Arc<Link>;

impl Link {
    /// A response (id ≠ 0) or an event (id 0). Events are buffered while disconnected.
    fn send(&self, id: u32, body: Response) -> io::Result<()> {
        let mut st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(w) = st.writer.as_mut() {
            match proto::write_frame(w, &Frame { id, body: &body }) {
                Ok(()) => return Ok(()),
                // Frame too large (a huge directory listing, an `Exec` with both streams full): the connection is fine, only this response
                // could not be sent. It is reported as an error so the client doesn't wait forever; events (id 0) are dropped.
                Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                    if id != 0 {
                        let note = Response::Error(Error::Other(format!("the answer is too large to send ({e})")));
                        if proto::write_frame(w, &Frame { id, body: note }).is_err() {
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
                for (_, buf) in st.backlog.iter_mut().filter(|(p, _)| *p == pty) {
                    if buf.len() > PTY_BACKLOG {
                        let cut = buf.len() - PTY_BACKLOG;
                        buf.drain(..cut);
                    }
                }
            }
            Response::PtyExit { pty, code } => st.exits.push((pty, code)),
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
    fn attach(&self, writer: Box<dyn Write + Send>, hello: Option<Frame<Response>>) -> io::Result<u64> {
        let mut st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut w = BufWriter::new(writer);
        if let Some(hello) = hello {
            proto::write_frame(&mut w, &hello)?;
        }
        // Clear only after a successful write: one that breaks midway must not lose buffered events (resent on next connect).
        for (pty, data) in &st.backlog {
            proto::write_frame(&mut w, &Frame { id: 0, body: Response::PtyOutput { pty: *pty, data: data.clone() } })?;
        }
        for (pty, code) in &st.exits {
            proto::write_frame(&mut w, &Frame { id: 0, body: Response::PtyExit { pty: *pty, code: *code } })?;
        }
        if !st.changes.is_empty() {
            proto::write_frame(&mut w, &Frame { id: 0, body: Response::FsChanged(st.changes.clone()) })?;
        }
        st.backlog.clear();
        st.exits.clear();
        st.changes.clear();
        st.writer = Some(w);
        st.generation += 1;
        Ok(st.generation)
    }

    /// Mark disconnected, but only if this is still the current connection.
    fn detach(&self, generation: u64) {
        let mut st = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if st.generation == generation {
            st.writer = None;
        }
    }

    fn generation(&self) -> u64 {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).generation
    }

    fn is_attached(&self) -> bool {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).writer.is_some()
    }
}

fn send(out: &Out, id: u32, body: Response) -> io::Result<()> {
    out.send(id, body)
}

/// A session: running searches, open terminals, watching. Locally (stdio) it
/// ends together with the connection; in daemon mode it lives on after the connection drops.
pub struct Session {
    out: Out,
    cancels: Arc<Mutex<HashMap<u32, Arc<AtomicBool>>>>,
    ptys: Arc<Mutex<HashMap<u32, pty::Pty>>>,
    next_pty: AtomicU32,
    /// Piped processes and the generation of the connection they were opened on.
    procs: Arc<Mutex<HashMap<u32, (u64, proc::Proc)>>>,
    /// Large-file indexes (path → index).
    big: Arc<Mutex<HashMap<PathBuf, bigfile::Index>>>,
    watcher: Arc<watch::Watcher>,
    stopped: Arc<AtomicBool>,
    /// Small pool for file operations: a slow NFS walk or a big delete must not hold up `Cancel`, `Watch` and terminal input.
    pool: Pool,
}

type Job = Box<dyn FnOnce() + Send>;

/// Fixed-size thread pool (bounded queue: when full, the sender waits).
struct Pool {
    tx: std::sync::mpsc::SyncSender<Job>,
}

impl Pool {
    fn new(workers: usize) -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Job>(1024);
        let rx = Arc::new(Mutex::new(rx));
        for _ in 0..workers {
            let rx = rx.clone();
            std::thread::spawn(move || {
                // The lock is held only while waiting for a job; released while a job runs.
                loop {
                    let job = rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
                    match job {
                        Ok(job) => job(),
                        Err(_) => break,
                    }
                }
            });
        }
        Pool { tx }
    }

    fn run(&self, job: impl FnOnce() + Send + 'static) {
        let _ = self.tx.send(Box::new(job));
    }
}

impl Default for Session {
    fn default() -> Self {
        Session {
            out: Arc::default(),
            cancels: Arc::default(),
            ptys: Arc::default(),
            next_pty: AtomicU32::new(1),
            procs: Arc::default(),
            big: Arc::default(),
            watcher: Arc::default(),
            stopped: Arc::default(),
            pool: Pool::new(4),
        }
    }
}

impl Session {
    /// Closed by `Bye` or by `shutdown` (the daemon drops such a session immediately).
    pub(crate) fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }

    /// Stop everything: cancel searches, close shells, end watching.
    pub fn shutdown(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        for flag in self.cancels.locked().values() {
            flag.store(true, Ordering::Relaxed);
        }
        self.ptys.locked().clear(); // Drop: kills the shells
        self.procs.locked().clear();
    }

    /// Attach a connection to this session and run the request loop. When it returns
    /// the connection is over (not the session itself).
    fn run(&self, input: &mut impl Read, output: Box<dyn Write + Send>, hello: Option<Frame<Response>>, first: Option<Frame<Request>>) -> io::Result<()> {
        let generation = self.out.attach(output, hello)?;
        let result = (|| -> io::Result<()> {
            if let Some(Frame { id, body }) = first {
                self.dispatch(id, body)?;
            }
            while let Some(Frame { id, body }) = proto::read_frame::<Request>(input)? {
                self.dispatch(id, body)?;
            }
            Ok(())
        })();
        self.out.detach(generation);
        // Language servers can't survive a disconnect (lost messages corrupt the stream): terminate
        // those opened on this connection (leave the new connection's alone);
        // the client restarts them. Terminals keep living.
        self.procs.locked().retain(|_, (g, _)| *g != generation);
        result
    }
}

/// Framed request loop (stdin/stdout or a pipe): a single-connection session.
/// When the stream closes, shells are closed, searches stopped, and it returns.
pub fn serve(input: impl Read, output: impl Write + Send + 'static) -> io::Result<()> {
    let mut input = BufReader::new(input);
    let session = Session::default();
    // The first frame must be the handshake.
    let result = match proto::read_frame::<Request>(&mut input)? {
        None => Ok(()),
        Some(Frame { id, body: Request::Hello { client_version, .. } }) => {
            let hello = Frame { id, body: hello(client_version, false).unwrap_or_else(Response::Error) };
            session.run(&mut input, Box::new(output), Some(hello), None)
        }
        // A client without a handshake: the first request goes through the normal path too.
        Some(first) => session.run(&mut input, Box::new(output), None, Some(first)),
    };
    session.shutdown();
    result
}

impl Session {
    fn dispatch(&self, id: u32, body: Request) -> io::Result<()> {
        // id 0: the client is not waiting for a response.
        let reply = |body: Response| if id == 0 { Ok(()) } else { send(&self.out, id, body) };
        match body {
            Request::Search(q) => {
                let flag = Arc::new(AtomicBool::new(false));
                self.cancels.locked().insert(id, flag.clone());
                let (out, cancels) = (self.out.clone(), self.cancels.clone());
                std::thread::spawn(move || {
                    let root = expand(&q.root);
                    let result = search::run(&q, &root, &flag, |fm| {
                        let _ = send(&out, id, Response::SearchChunk(vec![fm]));
                    });
                    cancels.locked().remove(&id);
                    let _ = send(
                        &out,
                        id,
                        match result {
                            Ok(s) => Response::SearchDone { files: s.files, matches: s.matches, truncated: s.truncated, cancelled: s.cancelled },
                            Err(e) => Response::Error(e),
                        },
                    );
                });
                Ok(())
            }
            Request::Cancel { id: target } => {
                if let Some(flag) = self.cancels.locked().get(&target) {
                    flag.store(true, Ordering::Relaxed);
                }
                reply(Response::Done)
            }
            Request::PtySpawn { cols, rows, cwd, shell } => {
                let cwd = cwd.map(|c| expand(&c).to_string_lossy().into_owned());
                match pty::spawn(cols, rows, cwd.as_deref(), shell.as_deref()) {
                    Err(e) => reply(Response::Error(Error::Other(format!("terminal: {e}")))),
                    Ok((p, mut reader)) => {
                        let pid = self.next_pty.fetch_add(1, Ordering::Relaxed);
                        self.ptys.locked().insert(pid, p);
                        // Response first, then output: so output doesn't arrive before the client has registered the terminal.
                        reply(Response::PtySpawned { pty: pid })?;
                        let (out, ptys) = (self.out.clone(), self.ptys.clone());
                        std::thread::spawn(move || {
                            let mut buf = vec![0u8; 16 * 1024];
                            // Keep reading even if the connection drops: output goes to the buffer, the shell keeps running.
                            loop {
                                match reader.read(&mut buf) {
                                    Ok(0) | Err(_) => break, // shell exited (EIO) or closed
                                    Ok(n) => {
                                        let _ = send(&out, 0, Response::PtyOutput { pty: pid, data: buf[..n].to_vec() });
                                    }
                                }
                            }
                            let code = ptys.locked().remove(&pid).and_then(|mut p| p.wait());
                            let _ = send(&out, 0, Response::PtyExit { pty: pid, code });
                        });
                        Ok(())
                    }
                }
            }
            Request::PtyInput { pty: pid, data } => {
                if let Some(p) = self.ptys.locked().get_mut(&pid) {
                    let _ = p.write(&data);
                }
                reply(Response::Done)
            }
            Request::PtyResize { pty: pid, cols, rows } => {
                if let Some(p) = self.ptys.locked().get(&pid) {
                    let _ = pty::resize(p, cols, rows);
                }
                reply(Response::Done)
            }
            Request::Watch { paths } => {
                let list = paths.into_iter().map(|p| {
                    let full = expand(&p);
                    (p, full)
                });
                self.watcher.set(list.collect());
                let out = self.out.clone();
                self.watcher.start(self.stopped.clone(), move |changes| send(&out, 0, Response::FsChanged(changes)).is_ok());
                reply(Response::Done)
            }
            Request::PtyKill { pty: pid } => {
                if let Some(p) = self.ptys.locked().get_mut(&pid) {
                    p.kill();
                }
                reply(Response::Done)
            }
            Request::Exec { program, args, cwd, stdin } => {
                let out = self.out.clone();
                std::thread::spawn(move || {
                    let cwd = cwd.map(|c| expand(&c).to_string_lossy().into_owned());
                    let body = match proc::exec(&program, &args, cwd.as_deref(), stdin) {
                        Ok(o) => Response::Exited { code: o.code, stdout: o.stdout, stderr: o.stderr },
                        Err(e) => Response::Error(Error::NotFound(format!("{program}: {e}"))),
                    };
                    if id != 0 {
                        let _ = send(&out, id, body);
                    }
                });
                Ok(())
            }
            Request::ProcSpawn { program, args, cwd } => {
                let cwd = cwd.map(|c| expand(&c).to_string_lossy().into_owned());
                match proc::spawn(&program, &args, cwd.as_deref()) {
                    Err(e) => reply(Response::Error(Error::NotFound(format!("{program}: {e}")))),
                    Ok((p, mut reader)) => {
                        let pid = self.next_pty.fetch_add(1, Ordering::Relaxed);
                        self.procs.locked().insert(pid, (self.out.generation(), p));
                        reply(Response::ProcSpawned { proc: pid })?;
                        let (out, procs) = (self.out.clone(), self.procs.clone());
                        std::thread::spawn(move || {
                            let mut buf = vec![0u8; 64 * 1024];
                            while let Ok(n @ 1..) = reader.read(&mut buf) {
                                let _ = send(&out, 0, Response::ProcOutput { proc: pid, data: buf[..n].to_vec() });
                            }
                            let code = procs.locked().remove(&pid).and_then(|(_, mut p)| p.wait());
                            let _ = send(&out, 0, Response::ProcExit { proc: pid, code });
                        });
                        Ok(())
                    }
                }
            }
            Request::LineIndex { path } if id != 0 => {
                self.line_request(id, path, None);
                Ok(())
            }
            Request::ReadLines { path, start, count } if id != 0 => {
                self.line_request(id, path, Some((start, count)));
                Ok(())
            }
            Request::ProcInput { proc: pid, data } => {
                if let Some((_, p)) = self.procs.locked().get_mut(&pid) {
                    let _ = p.write(&data);
                }
                reply(Response::Done)
            }
            Request::ProcKill { proc: pid } => {
                self.procs.locked().remove(&pid);
                reply(Response::Done)
            }
            Request::Hello { client_version, .. } => reply(hello(client_version, false).unwrap_or_else(Response::Error)),
            Request::Ping => reply(Response::Done),
            // Deliberate leave: terminals, processes and watching close right away; the loop ends.
            Request::Bye => {
                self.shutdown();
                Err(io::Error::from(io::ErrorKind::ConnectionAborted))
            }
            // File operations run on the pool: responses are matched by id, so order doesn't matter (the client waits on each call).
            other => {
                let out = self.out.clone();
                self.pool.run(move || {
                    let body = handle(other);
                    if id != 0 {
                        let _ = send(&out, id, body);
                    }
                });
                Ok(())
            }
        }
    }
}

impl Session {
    /// Line index or a line range of a big file, answered from a thread (the scan can take a while).
    fn line_request(&self, id: u32, path: String, range: Option<(u64, u32)>) {
        let (out, big) = (self.out.clone(), self.big.clone());
        std::thread::spawn(move || {
            let p = expand(&path);
            let known = big.locked().get(&p).cloned();
            let index = match known {
                Some(i) if i.fresh(&p) => Ok(i),
                // Growing log: scan only the appended part; if it was rotated, start over.
                other => other.and_then(|old| old.extend(&p).ok().flatten()).map_or_else(|| bigfile::build(&p), Ok).inspect(|i| {
                    big.locked().insert(p.clone(), i.clone());
                }),
            };
            let body = match (index, range) {
                (Err(e), _) => Response::Error(map_err(&p, e)),
                (Ok(i), None) => Response::LineIndex { lines: i.lines, size: i.size },
                (Ok(i), Some((start, count))) => match bigfile::read_lines(&p, &i, start, count.min(4096)) {
                    Ok(lines) => Response::Lines(lines),
                    Err(e) => Response::Error(map_err(&p, e)),
                },
            };
            let _ = send(&out, id, body);
        });
    }
}

pub(crate) fn hello(client_version: u32, resumed: bool) -> Result<Response, Error> {
    if client_version != proto::VERSION {
        return Err(Error::VersionMismatch { agent: proto::VERSION, client: client_version });
    }
    Ok(Response::Hello(HostInfo {
        version: proto::VERSION,
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        home: home().to_string_lossy().into_owned(),
        hostname: hostname(),
        resumed,
    }))
}

fn home() -> PathBuf {
    // No HOME on Windows: USERPROFILE.
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(not(unix))]
fn hostname() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_default()
}

#[cfg(unix)]
fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: `buf` is writable for `buf.len()` bytes, which is the length passed. On truncation the result may lack a NUL;
    // the scan below is bounded by the buffer for that case.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return String::new();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

/// `~` and `~/...` expand to the user's home directory.
fn expand(path: &str) -> PathBuf {
    match path.strip_prefix('~') {
        Some("") => home(),
        Some(rest) if rest.starts_with('/') => home().join(&rest[1..]),
        _ => PathBuf::from(path),
    }
}

fn map_err(path: &Path, e: io::Error) -> Error {
    let p = path.display().to_string();
    match e.kind() {
        io::ErrorKind::NotFound => Error::NotFound(p),
        io::ErrorKind::PermissionDenied => Error::PermissionDenied(p),
        io::ErrorKind::IsADirectory => Error::IsDirectory(p),
        io::ErrorKind::AlreadyExists => Error::AlreadyExists(p),
        _ => Error::Other(format!("{p}: {e}")),
    }
}

fn kind_of(ft: fs::FileType) -> Kind {
    if ft.is_symlink() {
        Kind::Symlink
    } else if ft.is_dir() {
        Kind::Dir
    } else if ft.is_file() {
        Kind::File
    } else {
        Kind::Other
    }
}

fn read_dir(path: &Path) -> Result<Vec<DirEntry>, Error> {
    let mut out = Vec::new();
    for entry in fs::read_dir(path).map_err(|e| map_err(path, e))? {
        let entry = entry.map_err(|e| map_err(path, e))?;
        let Ok(ft) = entry.file_type() else { continue };
        let kind = kind_of(ft);
        let target_kind = if kind == Kind::Symlink { fs::metadata(entry.path()).ok().map(|m| kind_of(m.file_type())) } else { None };
        // Non-UTF-8 file names are converted lossily (known limitation).
        out.push(DirEntry { name: entry.file_name().to_string_lossy().into_owned(), kind, target_kind });
    }
    Ok(out)
}

fn stat(path: &Path) -> Result<Meta, Error> {
    let m = fs::metadata(path).map_err(|e| map_err(path, e))?;
    let mtime_ms = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64);
    Ok(Meta { kind: kind_of(m.file_type()), size: m.len(), mtime_ms, readonly: m.permissions().readonly() })
}

fn read(path: &Path) -> Result<Response, Error> {
    let (data, hash) = read_capped(path)?;
    Ok(Response::File { data, hash })
}

/// The contents of a regular file of at most `MAX_FILE` bytes, and their hash.
fn read_capped(path: &Path) -> Result<(Vec<u8>, u64), Error> {
    let m = fs::metadata(path).map_err(|e| map_err(path, e))?;
    if m.is_dir() {
        return Err(Error::IsDirectory(path.display().to_string()));
    }
    // FIFOs, devices and sockets: length looks like 0 but reads block or never end (would freeze the single dispatch thread).
    if !m.is_file() {
        return Err(Error::Unsupported(format!("{} is not a regular file", path.display())));
    }
    if m.len() > proto::MAX_FILE {
        return Err(Error::TooLarge { path: path.display().to_string(), size: m.len(), limit: proto::MAX_FILE });
    }
    // A file that grows after the check must not exceed the limit: read up to one byte past the limit.
    let mut data = Vec::with_capacity(m.len() as usize);
    let mut f = fs::File::open(path).map_err(|e| map_err(path, e))?;
    io::Read::take(&mut f, proto::MAX_FILE + 1).read_to_end(&mut data).map_err(|e| map_err(path, e))?;
    if data.len() as u64 > proto::MAX_FILE {
        return Err(Error::TooLarge { path: path.display().to_string(), size: data.len() as u64, limit: proto::MAX_FILE });
    }
    let hash = content_hash(&data);
    Ok((data, hash))
}

/// File list for quick open: ripgrep's rules (.gitignore, .ignore,
/// global gitignore), even outside a git repository. Hidden files included; .git and
/// node_modules are never traversed.
fn list_files(root: &Path, limit: usize) -> Result<(Vec<String>, bool), Error> {
    if !root.is_dir() {
        return Err(map_err(root, io::Error::from(io::ErrorKind::NotFound)));
    }
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .sort_by_file_name(|a, b| a.cmp(b))
        .filter_entry(|e| !matches!(e.file_name().to_str(), Some(".git" | "node_modules")))
        .build();
    let mut out = Vec::new();
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if let Ok(rel) = entry.path().strip_prefix(root) {
            out.push(relative_path(rel));
            if out.len() >= limit {
                return Ok((out, true));
            }
        }
    }
    Ok((out, false))
}

/// New file; for paths like "a/b/c.txt" missing directories are created too (like VS Code).
fn create_file(path: &Path) -> Result<(), Error> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| map_err(parent, e))?;
    }
    fs::OpenOptions::new().write(true).create_new(true).open(path).map(|_| ()).map_err(|e| map_err(path, e))
}

fn rename(from: &Path, to: &Path) -> Result<(), Error> {
    // fs::rename overwrites the target; we don't. On case-insensitive disks
    // (macOS) "readme" → "README" finds the target as the same file: allow it in that case.
    if fs::symlink_metadata(to).is_ok() && !same_file(to, from) {
        return Err(Error::AlreadyExists(to.display().to_string()));
    }
    fs::rename(from, to).map_err(|e| map_err(from, e))
}

#[cfg(unix)]
fn same_file(target: &Path, from: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    fs::symlink_metadata(target).is_ok_and(|target| fs::symlink_metadata(from).is_ok_and(|m| m.dev() == target.dev() && m.ino() == target.ino()))
}

#[cfg(windows)]
fn same_file(target: &Path, from: &Path) -> bool {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, GetFileInformationByHandle,
    };
    let identity = |path: &Path| -> Option<(u32, u32, u32)> {
        let file = fs::OpenOptions::new().read(true).custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).open(path).ok()?;
        // SAFETY: BY_HANDLE_FILE_INFORMATION is plain integers (FILETIMEs included), so all-zero is a valid value.
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: `file` keeps the handle open for the call, and `info` is a valid, exclusively borrowed output struct.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return None;
        }
        Some((info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow))
    };
    identity(target).is_some_and(|id| identity(from) == Some(id))
}

/// Relative paths in the protocol use '/' on every host (matching globs and Git).
fn relative_path(path: &Path) -> String {
    let path = path.to_string_lossy().into_owned();
    #[cfg(windows)]
    let path = path.replace('\\', "/");
    path
}

fn delete(path: &Path) -> Result<(), Error> {
    delete_guarded(path, &home())
}

fn delete_guarded(path: &Path, home: &Path) -> Result<(), Error> {
    let canon = fs::canonicalize(path).map_err(|e| map_err(path, e))?;
    // The root, the home directory and its parents (/home, /Users) are never deleted.
    let home_dir = fs::canonicalize(home).unwrap_or_default();
    if canon.parent().is_none() || home_dir.starts_with(&canon) {
        return Err(Error::PermissionDenied(format!("{} (the root, the home directory and its parents are never deleted)", path.display())));
    }
    // The symlink itself is deleted, not what it points to.
    let meta = fs::symlink_metadata(path).map_err(|e| map_err(path, e))?;
    let result = if meta.is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) };
    result.map_err(|e| map_err(path, e))
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Atomic save: temp file + fsync + rename. If ownership or a hard link
/// would be broken by rename, the file is written in place.
fn write(path: &Path, data: &[u8], expect_hash: Option<u64>) -> Result<u64, Error> {
    // Update the file the symlink points to, not the symlink itself.
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let existing = fs::metadata(&target).ok();

    if let Some(m) = &existing {
        if m.is_dir() {
            return Err(Error::IsDirectory(path.display().to_string()));
        }
        if let Some(expected) = expect_hash {
            // Capped like `read`: the target may have grown since the editor loaded it, and the local agent shares the app's memory.
            let (_, current) = read_capped(&target)?;
            if current != expected {
                return Err(Error::Conflict { path: path.display().to_string(), current_hash: current });
            }
        }
    }

    if existing.as_ref().is_some_and(must_write_in_place) {
        // Kept in memory so a failed write can put the original back; there is no room for a sibling copy in a directory we may not own.
        let backup = match existing.as_ref().filter(|m| m.is_file()) {
            Some(_) => Some(read_capped(&target)?.0),
            None => None,
        };
        let mut f = fs::OpenOptions::new().write(true).open(&target).map_err(|e| map_err(path, e))?;
        overwrite(&mut f, data, backup.as_deref(), |f, d| f.write_all(d)).map_err(|e| map_err(path, e))?;
        return Ok(content_hash(data));
    }

    let dir = target.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // Temp name: very long file names must not exceed NAME_MAX and make saving impossible.
    let short: String = name.chars().take(60).collect();
    let mut tmp = temp_path(dir, &short);

    let result = (|| -> io::Result<()> {
        // `create_new`: a pre-planted symlink is not followed and an existing file is not clobbered (the name is also made unpredictable;
        // on a collision a new name is tried). So that the hidden contents (0600) of an existing file can't be read by others while being written,
        // the temp file is opened for the owner only; the real permissions are applied after the data is written.
        let mut attempts = 0;
        let mut f = loop {
            match create_private(&tmp, existing.is_none()) {
                Ok(f) => break f,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists && attempts < 8 => {
                    attempts += 1;
                    tmp = temp_path(dir, &short);
                }
                Err(e) => return Err(e),
            }
        };
        f.write_all(data)?;
        if let Some(m) = &existing {
            f.set_permissions(m.permissions())?;
        }
        f.sync_all()?;
        fs::rename(&tmp, &target)?;
        // Make the rename itself durable too.
        if let Ok(d) = fs::File::open(dir) {
            let _ = d.sync_all();
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(map_err(path, e));
    }
    Ok(content_hash(data))
}

/// Replace the contents of `f` without truncating first, so running out of space or an I/O error part way leaves the original
/// recoverable. On failure `backup` (the previous contents) is written back; if that fails too, both errors are reported.
fn overwrite(f: &mut fs::File, data: &[u8], backup: Option<&[u8]>, put: impl FnOnce(&mut fs::File, &[u8]) -> io::Result<()>) -> io::Result<()> {
    use io::Seek;
    let result = put(f, data).and_then(|()| f.set_len(data.len() as u64)).and_then(|()| f.sync_all());
    let Err(e) = result else { return Ok(()) };
    if let Some(old) = backup {
        let restored = f.rewind().and_then(|()| f.write_all(old)).and_then(|()| f.set_len(old.len() as u64)).and_then(|()| f.sync_all());
        if let Err(r) = restored {
            return Err(io::Error::new(e.kind(), format!("{e}; the original could not be restored: {r}")));
        }
    }
    Err(e)
}

/// `.<name>.codeyard-<pid>-<counter>-<time>.tmp`: the counter prevents collisions within the same process, the timestamp across other processes and against an attacker guessing
/// the name.
fn temp_path(dir: &Path, short: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    dir.join(format!(".{short}.codeyard-{}-{}-{nanos:x}.tmp", std::process::id(), TMP_COUNTER.fetch_add(1, Ordering::Relaxed)))
}

/// Create the temp file with `O_EXCL`. A new file follows the umask (0666); an existing one is opened for the owner only (0600).
#[cfg(unix)]
fn create_private(path: &Path, new_file: bool) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new().write(true).create_new(true).mode(if new_file { 0o666 } else { 0o600 }).open(path)
}

#[cfg(not(unix))]
fn create_private(path: &Path, _new_file: bool) -> io::Result<fs::File> {
    fs::OpenOptions::new().write(true).create_new(true).open(path)
}

#[cfg(unix)]
fn must_write_in_place(m: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: geteuid takes no arguments, touches no memory and cannot fail; it is `unsafe` only as a foreign function.
    let euid = unsafe { libc::geteuid() };
    m.nlink() > 1 || m.uid() != euid
}

#[cfg(not(unix))]
fn must_write_in_place(_: &fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slow_job_does_not_hold_up_the_others() {
        let pool = Pool::new(4);
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<&'static str>();
        let d = done_tx.clone();
        pool.run(move || {
            let _ = release_rx.recv_timeout(std::time::Duration::from_secs(10));
            let _ = d.send("slow");
        });
        for _ in 0..3 {
            let d = done_tx.clone();
            pool.run(move || {
                let _ = d.send("fast");
            });
        }
        for _ in 0..3 {
            assert_eq!(done_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(), "fast", "fast jobs finish while the slow one is stuck");
        }
        release_tx.send(()).unwrap();
        assert_eq!(done_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(), "slow");
    }

    #[test]
    fn requests_are_answered_by_id_even_when_they_run_in_the_pool() {
        let d = tmpdir("pooled");
        for i in 0..20 {
            fs::write(d.join(format!("f{i}.txt")), format!("{i}")).unwrap();
        }
        let (req_r, mut req_w) = io::pipe().unwrap();
        let (resp_r, resp_w) = io::pipe().unwrap();
        std::thread::spawn(move || serve(req_r, resp_w));
        proto::write_frame(&mut req_w, &Frame { id: 1, body: Request::Hello { client_version: proto::VERSION, session: None } }).unwrap();
        for i in 0..20u32 {
            let path = s(&d.join(format!("f{i}.txt")));
            proto::write_frame(&mut req_w, &Frame { id: 100 + i, body: Request::Read { path } }).unwrap();
        }
        let mut r = BufReader::new(resp_r);
        let mut got = std::collections::BTreeMap::new();
        while got.len() < 20 {
            let f: Frame<Response> = proto::read_frame(&mut r).unwrap().unwrap();
            if let Response::File { data, .. } = f.body {
                got.insert(f.id, String::from_utf8(data).unwrap());
            }
        }
        for i in 0..20u32 {
            assert_eq!(got[&(100 + i)], i.to_string(), "answer {i} belongs to request {i}");
        }
        proto::write_frame(&mut req_w, &Frame { id: 0, body: Request::Bye }).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn saving_never_follows_a_planted_symlink_and_handles_long_names() {
        let d = tmpdir("planted");
        let victim = d.join("victim.txt");
        fs::write(&victim, "precious").unwrap();
        // The temp file is opened with O_EXCL: an existing path (here a symlink) is refused, not followed.
        let link = d.join(".planted.tmp");
        std::os::unix::fs::symlink(&victim, &link).unwrap();
        let e = create_private(&link, true).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(&victim).unwrap(), "precious", "the symlink target is untouched");
        // Names are not predictable from pid and counter alone.
        assert_ne!(temp_path(&d, "x"), temp_path(&d, "x"));
        // Saving a normal file next to the planted link works, and a very long name still saves.
        let target = d.join("doc.txt");
        write(&target, b"new content", None).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "new content");
        let long = d.join(format!("{}.txt", "x".repeat(240)));
        write(&long, b"ok", None).expect("long names save");
        assert_eq!(fs::read(&long).unwrap(), b"ok");
    }

    #[cfg(unix)]
    #[test]
    fn saving_over_a_private_file_keeps_it_private() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmpdir("private");
        let p = d.join(".env");
        fs::write(&p, "SECRET=1").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
        write(&p, b"SECRET=2", None).unwrap();
        assert_eq!(fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        let fresh = d.join("new.txt");
        write(&fresh, b"x", None).unwrap();
        assert_ne!(fs::metadata(&fresh).unwrap().permissions().mode() & 0o044, 0, "a new file follows the umask, not 0600");
    }

    #[cfg(unix)]
    #[test]
    fn special_files_are_refused_instead_of_blocking_the_session() {
        let d = tmpdir("fifo");
        let fifo = d.join("pipe");
        let c = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert!(matches!(read(&fifo), Err(Error::Unsupported(_))), "a FIFO would block forever");
        assert!(crate::bigfile::build(&fifo).is_err());
        assert!(matches!(read(Path::new("/dev/zero")), Err(Error::Unsupported(_))));
    }

    #[test]
    fn delete_refuses_home_and_its_parents() {
        let d = tmpdir("delguard");
        let home = d.join("users").join("me");
        fs::create_dir_all(home.join("project")).unwrap();
        assert!(matches!(delete_guarded(&home, &home), Err(Error::PermissionDenied(_))));
        assert!(matches!(delete_guarded(home.parent().unwrap(), &home), Err(Error::PermissionDenied(_))), "/Users itself");
        assert!(matches!(delete_guarded(Path::new("/"), &home), Err(Error::PermissionDenied(_))));
        assert!(home.join("project").exists());
        delete_guarded(&home.join("project"), &home).expect("inside home is fine");
        assert!(!home.join("project").exists());
    }

    /// A writer that remembers what was written.
    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);
    impl Write for Sink {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn an_oversized_answer_becomes_an_error_and_the_link_survives() {
        let link = Link::default();
        let sink = Sink::default();
        link.attach(Box::new(sink.clone()), None).unwrap();
        let huge = Response::File { data: vec![0u8; proto::MAX_FRAME + 1], hash: 0 };
        link.send(7, huge).unwrap();
        // The client got an error for id 7 instead of silence...
        let bytes = sink.0.lock().unwrap().clone();
        let frame: Frame<Response> = proto::read_frame(&mut &bytes[..]).unwrap().expect("a frame");
        assert_eq!(frame.id, 7);
        assert!(matches!(frame.body, Response::Error(Error::Other(ref m)) if m.contains("too large")), "{:?}", frame.body);
        // ...and the connection still works.
        sink.0.lock().unwrap().clear();
        link.send(8, Response::Done).unwrap();
        let bytes = sink.0.lock().unwrap().clone();
        let frame: Frame<Response> = proto::read_frame(&mut &bytes[..]).unwrap().expect("a frame");
        assert_eq!((frame.id, matches!(frame.body, Response::Done)), (8, true));
    }

    #[test]
    fn a_failed_attach_keeps_the_backlog_for_the_next_one() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::BrokenPipe))
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::Error::from(io::ErrorKind::BrokenPipe))
            }
        }
        let link = Link::default();
        link.send(0, Response::PtyOutput { pty: 1, data: b"hello".to_vec() }).unwrap();
        link.send(0, Response::PtyExit { pty: 1, code: Some(3) }).unwrap();
        assert!(link.attach(Box::new(Broken), None).is_err());
        let sink = Sink::default();
        link.attach(Box::new(sink.clone()), None).unwrap();
        let bytes = sink.0.lock().unwrap().clone();
        let mut cur = &bytes[..];
        let a: Frame<Response> = proto::read_frame(&mut cur).unwrap().unwrap();
        let b: Frame<Response> = proto::read_frame(&mut cur).unwrap().unwrap();
        assert!(matches!(a.body, Response::PtyOutput { pty: 1, ref data } if data == b"hello"));
        assert!(matches!(b.body, Response::PtyExit { pty: 1, code: Some(3) }), "the exit was not lost");
    }

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("codeyard-agent-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    #[cfg(windows)]
    #[test]
    fn case_only_rename_preserves_contents_and_refuses_other_files() {
        let dir = tmpdir("case-rename-şablon boşluk");
        let from = dir.join("readme.txt");
        let to = dir.join("README.txt");
        fs::write(&from, "Türkçe: ıİşğüöç").unwrap();
        rename(&from, &to).unwrap();
        assert_eq!(fs::read_to_string(&to).unwrap(), "Türkçe: ıİşğüöç");
        assert!(fs::read_dir(&dir).unwrap().any(|e| e.unwrap().file_name() == "README.txt"));
        fs::write(dir.join("other.txt"), "other").unwrap();
        assert!(matches!(rename(&to, &dir.join("other.txt")), Err(Error::AlreadyExists(_))));
        assert_eq!(fs::read_to_string(dir.join("other.txt")).unwrap(), "other");
    }

    #[test]
    fn write_then_read_roundtrip() {
        let d = tmpdir("rt");
        let p = d.join("a.txt");
        let Response::Written { hash } = handle(Request::Write { path: s(&p), data: b"merhaba\n".to_vec(), expect_hash: None }) else { panic!() };
        let Response::File { data, hash: h2 } = handle(Request::Read { path: s(&p) }) else { panic!() };
        assert_eq!(data, b"merhaba\n");
        assert_eq!(hash, h2);
        // No temp file should remain.
        assert_eq!(fs::read_dir(&d).unwrap().count(), 1);
    }

    #[test]
    fn conflict_when_disk_changed() {
        let d = tmpdir("conflict");
        let p = d.join("a.txt");
        fs::write(&p, "v1").unwrap();
        let base = content_hash(b"v1");
        fs::write(&p, "dışarıdan değişti").unwrap();
        let r = handle(Request::Write { path: s(&p), data: b"benim".to_vec(), expect_hash: Some(base) });
        assert!(matches!(r, Response::Error(Error::Conflict { .. })), "{r:?}");
        assert_eq!(fs::read_to_string(&p).unwrap(), "dışarıdan değişti");
    }

    #[cfg(unix)]
    #[test]
    fn keeps_permissions_and_symlink() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let d = tmpdir("perm");
        let real = d.join("script.sh");
        fs::write(&real, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o750)).unwrap();
        let link = d.join("link.sh");
        symlink(&real, &link).unwrap();

        let r = handle(Request::Write { path: s(&link), data: b"#!/bin/sh\necho 1\n".to_vec(), expect_hash: None });
        assert!(matches!(r, Response::Written { .. }), "{r:?}");
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "the link must be kept");
        assert_eq!(fs::read_to_string(&real).unwrap(), "#!/bin/sh\necho 1\n");
        assert_eq!(fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o750);
    }

    #[cfg(unix)]
    #[test]
    fn hard_link_written_in_place() {
        let d = tmpdir("hardlink");
        let a = d.join("a");
        let b = d.join("b");
        fs::write(&a, "1").unwrap();
        fs::hard_link(&a, &b).unwrap();
        handle(Request::Write { path: s(&a), data: b"2".to_vec(), expect_hash: None });
        assert_eq!(fs::read_to_string(&b).unwrap(), "2", "the hard link must not break");
    }

    /// A write that dies part way (disk full, I/O error) must not leave the file empty or half new: the original comes back,
    /// through every hard link.
    #[cfg(unix)]
    #[test]
    fn failed_in_place_write_restores_the_original() {
        let d = tmpdir("inplace-fail");
        let a = d.join("a");
        let b = d.join("b");
        let original = b"the original, longer than the replacement fragment".to_vec();
        fs::write(&a, &original).unwrap();
        fs::hard_link(&a, &b).unwrap();
        let mut f = fs::OpenOptions::new().write(true).open(&a).unwrap();
        let err = overwrite(&mut f, b"new contents that never finish", Some(&original), |f, d| {
            f.write_all(&d[..10])?;
            Err(io::Error::other("no space left on device"))
        })
        .unwrap_err();
        assert!(err.to_string().contains("no space"), "{err}");
        assert_eq!(fs::read(&a).unwrap(), original);
        assert_eq!(fs::read(&b).unwrap(), original);
    }

    /// In place, a shorter file replaces a longer one completely (the length is cut after the write, not before).
    #[cfg(unix)]
    #[test]
    fn in_place_write_shrinks_the_file() {
        let d = tmpdir("inplace-shrink");
        let a = d.join("a");
        let b = d.join("b");
        fs::write(&a, "a long first version").unwrap();
        fs::hard_link(&a, &b).unwrap();
        let r = handle(Request::Write { path: s(&a), data: b"short".to_vec(), expect_hash: None });
        assert!(matches!(r, Response::Written { .. }), "{r:?}");
        assert_eq!(fs::read(&b).unwrap(), b"short");
    }

    /// The conflict check hashes the file on disk, so it obeys the same size limit as `Read`: a target that grew past it
    /// is refused instead of being read into memory.
    #[test]
    fn conflict_check_refuses_a_target_past_the_size_limit() {
        let d = tmpdir("conflict-big");
        let f = d.join("big.bin");
        fs::write(&f, "small").unwrap();
        let hash = content_hash(b"small");
        fs::OpenOptions::new().write(true).open(&f).unwrap().set_len(proto::MAX_FILE + 1).unwrap();
        let r = handle(Request::Write { path: s(&f), data: b"x".to_vec(), expect_hash: Some(hash) });
        assert!(matches!(r, Response::Error(Error::TooLarge { size, .. }) if size == proto::MAX_FILE + 1), "{r:?}");
        assert_eq!(fs::metadata(&f).unwrap().len(), proto::MAX_FILE + 1, "nothing was written");
    }

    #[test]
    fn read_dir_and_errors() {
        let d = tmpdir("dir");
        fs::create_dir(d.join("src")).unwrap();
        fs::write(d.join("Cargo.toml"), "").unwrap();
        let Response::Entries(mut es) = handle(Request::ReadDir { path: s(&d) }) else { panic!() };
        es.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(es.iter().map(|e| (e.name.as_str(), e.is_dir())).collect::<Vec<_>>(), [("Cargo.toml", false), ("src", true)]);
        assert!(matches!(handle(Request::Read { path: s(&d.join("yok")) }), Response::Error(Error::NotFound(_))));
        assert!(matches!(handle(Request::Read { path: s(&d) }), Response::Error(Error::IsDirectory(_))));
    }

    #[test]
    fn list_files_respects_gitignore_and_skips_git() {
        let d = tmpdir("list");
        fs::create_dir_all(d.join("src/deep")).unwrap();
        fs::create_dir_all(d.join(".git")).unwrap();
        fs::create_dir_all(d.join("target")).unwrap();
        fs::create_dir_all(d.join("node_modules/x")).unwrap();
        fs::write(d.join(".gitignore"), "target/\n*.log\n").unwrap();
        for f in ["src/main.rs", "src/deep/a.rs", ".env", "build.log", "target/out", ".git/HEAD", "node_modules/x/i.js"] {
            fs::write(d.join(f), "").unwrap();
        }
        let Response::Files { paths, truncated } = handle(Request::ListFiles { root: s(&d), limit: 100 }) else { panic!() };
        assert!(!truncated);
        assert_eq!(paths, [".env", ".gitignore", "src/deep/a.rs", "src/main.rs"]);
        let Response::Files { paths, truncated } = handle(Request::ListFiles { root: s(&d), limit: 2 }) else { panic!() };
        assert!(truncated);
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn file_operations() {
        let d = tmpdir("ops");
        let f = d.join("yeni.rs");
        assert!(matches!(handle(Request::CreateFile { path: s(&f) }), Response::Done));
        fs::write(&f, "içerik").unwrap();
        assert!(matches!(handle(Request::CreateFile { path: s(&f) }), Response::Error(Error::AlreadyExists(_))), "must not overwrite an existing file");
        assert_eq!(fs::read_to_string(&f).unwrap(), "içerik");

        let sub = d.join("alt");
        assert!(matches!(handle(Request::CreateDir { path: s(&sub) }), Response::Done));
        let g = sub.join("taşındı.rs");
        assert!(matches!(handle(Request::Rename { from: s(&f), to: s(&g) }), Response::Done));
        assert!(!f.exists() && g.exists());

        fs::write(&f, "başka").unwrap();
        assert!(matches!(handle(Request::Rename { from: s(&f), to: s(&g) }), Response::Error(Error::AlreadyExists(_))));
        assert_eq!(fs::read_to_string(&g).unwrap(), "içerik", "must not overwrite the target");

        assert!(matches!(handle(Request::Delete { path: s(&sub) }), Response::Done));
        assert!(!sub.exists());
    }

    #[test]
    fn refuses_to_delete_home_or_root() {
        assert!(matches!(handle(Request::Delete { path: "~".into() }), Response::Error(Error::PermissionDenied(_))));
        assert!(matches!(handle(Request::Delete { path: "/".into() }), Response::Error(Error::PermissionDenied(_))));
    }

    /// End to end: a framed session over a pipe.
    struct Client {
        w: io::PipeWriter,
        r: BufReader<io::PipeReader>,
    }

    impl Client {
        fn new() -> Self {
            let (req_r, req_w) = io::pipe().unwrap();
            let (resp_r, resp_w) = io::pipe().unwrap();
            std::thread::spawn(move || serve(req_r, resp_w));
            Client { w: req_w, r: BufReader::new(resp_r) }
        }
        fn send(&mut self, id: u32, body: Request) {
            proto::write_frame(&mut self.w, &Frame { id, body }).unwrap();
        }
        fn recv(&mut self) -> Frame<Response> {
            proto::read_frame(&mut self.r).unwrap().unwrap()
        }
    }

    #[test]
    fn session_streams_search_results() {
        let d = tmpdir("session-search");
        fs::write(d.join("a.txt"), "iğne\nsaman\niğne iğne\n").unwrap();
        let mut c = Client::new();
        c.send(
            5,
            Request::Search(proto::SearchQuery {
                root: s(&d),
                pattern: "iğne".into(),
                regex: false,
                case_sensitive: true,
                whole_word: false,
                include: vec![],
                exclude: vec![],
                max_results: 100,
            }),
        );
        let mut chunks = Vec::new();
        loop {
            let f = c.recv();
            assert_eq!(f.id, 5);
            match f.body {
                Response::SearchChunk(fm) => chunks.extend(fm),
                Response::SearchDone { matches, .. } => {
                    assert_eq!(matches, 3);
                    break;
                }
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(chunks[0].matches.iter().map(|m| m.line).collect::<Vec<_>>(), [1, 3]);
    }

    #[cfg(unix)]
    #[test]
    fn session_runs_a_shell_in_a_pty() {
        let mut c = Client::new();
        c.send(1, Request::PtySpawn { cols: 80, rows: 24, cwd: None, shell: Some("/bin/sh".into()) });
        let Response::PtySpawned { pty } = c.recv().body else { panic!("spawn") };
        // id 0: no response expected.
        c.send(0, Request::PtyInput { pty, data: b"echo kod-$((40+2)); exit 3\n".to_vec() });
        let mut output = Vec::new();
        let code = loop {
            let f = c.recv();
            assert_eq!(f.id, 0, "terminal messages are events");
            match f.body {
                Response::PtyOutput { data, .. } => output.extend(data),
                Response::PtyExit { code, .. } => break code,
                other => panic!("{other:?}"),
            }
        };
        let text = String::from_utf8_lossy(&output);
        assert!(text.contains("kod-42"), "{text}");
        assert_eq!(code, Some(3));
    }

    #[test]
    fn hello_rejects_other_version() {
        assert!(matches!(handle(Request::Hello { client_version: 999, session: None }), Response::Error(Error::VersionMismatch { .. })));
        assert!(matches!(handle(Request::Hello { client_version: proto::VERSION, session: None }), Response::Hello(_)));
    }
}
