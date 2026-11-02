//! Agent: file system, search, terminal. Locally, `host::local()` runs this session
//! on a thread over a pipe; remotely, as `codeyard-agent --stdio`.
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod bigfile;
pub mod daemon;
mod file_identity;
mod fileops;
pub mod guard;
pub mod hashtree;
mod new_file;
mod preview;
pub mod proc;
pub mod pty;
mod reattach;
pub mod search;
mod shell_integration;
pub mod watch;
mod write_locks;

#[cfg(test)]
mod write_tests;

use std::collections::HashMap;
use std::fs;
use std::io::{self, BufReader, Read, Write};
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
        Request::Lstat { path } => {
            fs::symlink_metadata(expand(&path)).map(|metadata| Response::Stat(file_meta(metadata))).map_err(|error| map_err(&expand(&path), error))
        }
        Request::Read { path } => read(&expand(&path)),
        Request::ReadPreview { root, path } => preview::read(&expand(&root), &path),
        Request::Write { path, data, expect_hash } => write(&expand(&path), &data, expect_hash).map(|hash| Response::Written { hash }),
        Request::WriteNew { path, data } => new_file::write(&expand(&path), &data).map(|hash| Response::Written { hash }),
        Request::CreateFile { path } => create_file(&expand(&path)).map(|_| Response::Done),
        Request::CreateDir { path } => create_dir(&expand(&path)).map(|_| Response::Done),
        Request::Rename { from, to } => rename(&expand(&from), &expand(&to)).map(|_| Response::Done),
        Request::Copy { from, to } => fileops::copy(&expand(&from), &expand(&to)).map(|_| Response::Done),
        Request::Delete { path } => delete(&expand(&path)).map(|_| Response::Done),
        // Streaming requests are handled inside `serve`; here only one-shot responses.
        Request::Search(_)
        | Request::HashTree { .. }
        | Request::Cancel { .. }
        | Request::PtySpawn { .. }
        | Request::PtyInput { .. }
        | Request::PtyResize { .. }
        | Request::PtyKill { .. }
        | Request::PtySpawnEx { .. }
        | Request::PtyStatus { .. }
        | Request::Watch { .. }
        | Request::Bye
        | Request::Ping
        | Request::Exec { .. }
        | Request::ProcSpawn { .. }
        | Request::ProcInput { .. }
        | Request::ProcKill { .. }
        | Request::TaskSpawn { .. }
        | Request::PtyToken { .. }
        | Request::PtyAttach { .. }
        | Request::PtyList
        | Request::KeepFor { .. }
        | Request::LineIndex { .. }
        | Request::ReadLines { .. } => Err(Error::Unsupported("streaming request outside a session".into())),
        Request::ListFiles { root, limit } => list_files(&expand(&root), limit as usize).map(|(paths, truncated)| Response::Files { paths, truncated }),
        Request::ListShells => Ok(Response::Shells(pty::shells())),
        Request::RealPaths { paths } => Ok(Response::RealPaths(paths.iter().map(|p| real_path(&expand(p))).collect())),
    };
    result.unwrap_or_else(Response::Error)
}

#[path = "link.rs"]
mod link;
#[cfg(test)]
use link::Link;
use link::{Out, send};

/// A session: running searches, open terminals, watching. Locally (stdio) it
/// ends together with the connection; in daemon mode it lives on after the connection drops.
pub struct Session {
    out: Out,
    cancels: Arc<Mutex<HashMap<u32, Arc<AtomicBool>>>>,
    ptys: Arc<Mutex<HashMap<u32, pty::Pty>>>,
    /// Each terminal's secret, once a client asked for its token (`reattach`).
    secrets: Mutex<HashMap<u32, String>>,
    /// Seconds the daemon keeps this session after its connection ends (`KeepFor`; `u64::MAX`: the daemon's default).
    pub(crate) keep_for: AtomicU64,
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
            secrets: Mutex::default(),
            keep_for: AtomicU64::new(u64::MAX),
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
                // A newer connection took the session over: this one may no longer type into its terminals.
                if self.out.generation() != generation {
                    break;
                }
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
    /// Opens a terminal and streams its output until the shell exits.
    fn spawn_pty(&self, id: u32, spec: pty::Spawn) -> io::Result<()> {
        let reply = |body: Response| if id == 0 { Ok(()) } else { send(&self.out, id, body) };
        let spec = pty::Spawn { cwd: spec.cwd.map(|c| expand(&c).to_string_lossy().into_owned()), ..spec };
        match pty::spawn_with(spec) {
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

    fn dispatch(&self, id: u32, body: Request) -> io::Result<()> {
        // id 0: the client is not waiting for a response.
        let reply = |body: Response| if id == 0 { Ok(()) } else { send(&self.out, id, body) };
        match body {
            Request::Copy { from, to } => {
                let flag = Arc::new(AtomicBool::new(false));
                self.cancels.locked().insert(id, flag.clone());
                let (out, cancels) = (self.out.clone(), self.cancels.clone());
                self.pool.run(move || {
                    let result = fileops::copy_cancellable(&expand(&from), &expand(&to), &flag);
                    cancels.locked().remove(&id);
                    let _ = send(&out, id, result.map_or_else(Response::Error, |()| Response::Done));
                });
                Ok(())
            }
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
            Request::HashTree { root, ignore, gitignore, limit, only } => {
                let flag = Arc::new(AtomicBool::new(false));
                self.cancels.locked().insert(id, flag.clone());
                let (out, cancels) = (self.out.clone(), self.cancels.clone());
                std::thread::spawn(move || {
                    let root = expand(&root);
                    let query = hashtree::Query { root: &root, ignore: &ignore, gitignore, limit: limit as usize, only: only.as_deref() };
                    let result = hashtree::run(&query, &flag, |records| {
                        let _ = send(&out, id, Response::HashChunk(records));
                    });
                    cancels.locked().remove(&id);
                    let done = match result {
                        Ok(s) => Response::HashDone { entries: s.entries, truncated: s.truncated, cancelled: s.cancelled },
                        Err(e) => Response::Error(e),
                    };
                    let _ = send(&out, id, done);
                });
                Ok(())
            }
            Request::Cancel { id: target } => {
                if let Some(flag) = self.cancels.locked().get(&target) {
                    flag.store(true, Ordering::Relaxed);
                }
                reply(Response::Done)
            }
            Request::PtySpawn { cols, rows, cwd, shell } => self.spawn_pty(id, pty::Spawn { cols, rows, cwd, shell, ..Default::default() }),
            Request::PtySpawnEx { cols, rows, cwd, shell, args, env, integration } => {
                self.spawn_pty(id, pty::Spawn { cols, rows, cwd, shell, args, env, integration })
            }
            Request::PtyToken { .. } | Request::PtyAttach { .. } | Request::PtyList | Request::KeepFor { .. } => self.reattach_request(id, body),
            Request::PtyStatus { pty: pid } => {
                let status = self.ptys.locked().get(&pid).map(pty::Pty::status).unwrap_or_default();
                reply(Response::PtyStatus { busy: status.busy, name: status.name, cwd: status.cwd })
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
                let spawned = proc::spawn(&program, &args, cwd.as_deref()).map(|(p, r)| (p, Box::new(r) as Box<dyn Read + Send>));
                self.proc_spawned(spawned, &program, reply)
            }
            Request::TaskSpawn { command, cwd, env } => {
                let cwd = cwd.map(|c| expand(&c).to_string_lossy().into_owned());
                let name = match &command {
                    proto::TaskCommand::Process { program, .. } => program.clone(),
                    proto::TaskCommand::Shell { shell, .. } => shell.clone().unwrap_or_else(|| "shell".into()),
                };
                let spawned = proc::spawn_task(&command, cwd.as_deref(), &env).map(|(p, r)| (p, Box::new(r) as Box<dyn Read + Send>));
                self.proc_spawned(spawned, &name, reply)
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
                // Input that can't be queued is lost for good, which would leave a language server working on a stale view of
                // the document. End the process instead: the client sees ProcExit and restarts it with fresh state.
                let mut procs = self.procs.locked();
                if procs.get_mut(&pid).is_some_and(|(_, p)| p.write(&data).is_err()) {
                    procs.remove(&pid);
                }
                drop(procs);
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
    /// Register a spawned process, answer `ProcSpawned`, then stream `ProcOutput` and one `ProcExit`.
    fn proc_spawned(
        &self,
        spawned: io::Result<(proc::Proc, Box<dyn Read + Send>)>,
        name: &str,
        reply: impl FnOnce(Response) -> io::Result<()>,
    ) -> io::Result<()> {
        let (p, mut reader) = match spawned {
            Err(e) => return reply(Response::Error(Error::NotFound(format!("{name}: {e}")))),
            Ok(spawned) => spawned,
        };
        let pid = self.next_pty.fetch_add(1, Ordering::Relaxed);
        self.procs.locked().insert(pid, (self.out.generation(), p));
        reply(Response::ProcSpawned { proc: pid })?;
        let (out, procs) = (self.out.clone(), self.procs.clone());
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            while let Ok(n @ 1..) = reader.read(&mut buf) {
                let _ = send(&out, 0, Response::ProcOutput { proc: pid, data: buf[..n].to_vec() });
            }
            let code = loop {
                let mut processes = procs.locked();
                let Some((_, process)) = processes.get_mut(&pid) else { break None };
                match process.try_wait() {
                    Ok(None) => {
                        drop(processes);
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    result => {
                        processes.remove(&pid);
                        break result.ok().flatten();
                    }
                }
            };
            let _ = send(&out, 0, Response::ProcExit { proc: pid, code });
        });
        Ok(())
    }

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
        // A file where a folder of the path should be: nothing is at that path (ENOTDIR).
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => Error::NotFound(p),
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

/// Symbolic links followed in one path before giving up, as `ELOOP` does.
const MAX_LINKS: u32 = 40;

/// Where `path` really is: every symbolic link followed, also a dangling one at the end (writing through it would create
/// its target), and parts that do not exist yet appended to their nearest existing folder. `path` is absolute.
fn real_path(path: &Path) -> Result<String, Error> {
    resolve(path, 0).map(|p| plain(&p).to_string_lossy().into_owned())
}

fn resolve(path: &Path, links: u32) -> Result<PathBuf, Error> {
    if links > MAX_LINKS {
        return Err(Error::Other(format!("{}: too many levels of symbolic links", path.display())));
    }
    match fs::canonicalize(path) {
        Ok(p) => return Ok(p),
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(map_err(path, e)),
        Err(_) => {}
    }
    let parent = path.parent().ok_or_else(|| Error::NotFound(path.display().to_string()))?;
    if let Ok(target) = fs::read_link(path) {
        return resolve(&parent.join(target), links + 1);
    }
    let name = path.file_name().ok_or_else(|| Error::NotFound(path.display().to_string()))?;
    Ok(resolve(parent, links)?.join(name))
}

/// `C:\x` for Windows' `\\?\C:\x`, as paths are written everywhere else; elsewhere the path itself.
fn plain(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.starts_with("UNC\\") => PathBuf::from(format!(r"\\{}", &rest[4..])),
        Some(rest) => PathBuf::from(rest),
        None => path.to_path_buf(),
    }
}

fn stat(path: &Path) -> Result<Meta, Error> {
    let m = fs::metadata(path).map_err(|e| map_err(path, e))?;
    Ok(file_meta(m))
}

fn file_meta(m: fs::Metadata) -> Meta {
    let mtime_ms = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64);
    let created_ms = m.created().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64);
    Meta { kind: kind_of(m.file_type()), size: m.len(), mtime_ms, readonly: m.permissions().readonly(), file_id: file_id(&m), created_ms }
}

/// Device and inode in one value (a path deleted and created again gets a new one, writes keep it).
#[cfg(unix)]
fn file_id(m: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    m.ino() ^ m.dev().rotate_left(32)
}

/// Windows has no stable file index in the standard library; the creation time stands in for it.
#[cfg(not(unix))]
fn file_id(_: &fs::Metadata) -> u64 {
    0
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

/// The final mkdir must be exclusive even when concurrent requests create its parents.
fn create_dir(path: &Path) -> Result<(), Error> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|error| map_err(parent, error))?;
    }
    fs::create_dir(path).map_err(|error| map_err(path, error))
}

/// New file; for paths like "a/b/c.txt" missing directories are created too (like VS Code).
fn create_file(path: &Path) -> Result<(), Error> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| map_err(parent, e))?;
    }
    fs::OpenOptions::new().write(true).create_new(true).open(path).map(|_| ()).map_err(|e| map_err(path, e))
}

fn rename(from: &Path, to: &Path) -> Result<(), Error> {
    fileops::rename_exclusive(from, to).map_err(|e| map_err(to, e))
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
    let meta = fs::symlink_metadata(path).map_err(|e| map_err(path, e))?;
    if meta.file_type().is_symlink() {
        // Resolve only the parent: a dangling link has no canonical target, and
        // deleting a link to a protected directory does not delete that directory.
        let entry = |p: &Path| {
            let parent = p.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
            Some(fs::canonicalize(parent).ok()?.join(p.file_name()?))
        };
        let is_home_entry = entry(path).zip(entry(home)).is_some_and(|(path, home)| path == home);
        let home_uses_link = std::path::absolute(home).ok().zip(std::path::absolute(path).ok()).is_some_and(|(home, path)| home.starts_with(path));
        if is_home_entry || home_uses_link {
            return Err(Error::PermissionDenied(format!("{} (the home directory and its parents are never deleted)", path.display())));
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::FileTypeExt;
            if meta.file_type().is_symlink_dir() {
                return fs::remove_dir(path).map_err(|e| map_err(path, e));
            }
        }
        return fs::remove_file(path).map_err(|e| map_err(path, e));
    }
    let canon = fs::canonicalize(path).map_err(|e| map_err(path, e))?;
    // The root, the home directory and its parents (/home, /Users) are never deleted.
    let home_dir = fs::canonicalize(home).unwrap_or_default();
    if canon.parent().is_none() || home_dir.starts_with(&canon) {
        return Err(Error::PermissionDenied(format!("{} (the root, the home directory and its parents are never deleted)", path.display())));
    }
    let result = if meta.is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) };
    result.map_err(|e| map_err(path, e))
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Atomic save: temp file + fsync + rename. If ownership or a hard link
/// would be broken by rename, the file is written in place.
fn write(path: &Path, data: &[u8], expect_hash: Option<u64>) -> Result<u64, Error> {
    // Update the file the symlink points to, not the symlink itself.
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let target_lock = write_locks::for_path(&target).map_err(|error| map_err(path, error))?;
    let _write_guard = target_lock.locked();
    let existing = fs::metadata(&target).ok();

    // A write against a known version must not bring back a file that was deleted since that version was read.
    if existing.is_none() && expect_hash.is_some() {
        return Err(Error::Conflict { path: path.display().to_string(), current_hash: 0 });
    }
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

    if existing.as_ref().map(|metadata| must_write_in_place(&target, metadata)).transpose()?.unwrap_or(false) {
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
fn must_write_in_place(_: &Path, m: &fs::Metadata) -> Result<bool, Error> {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: geteuid takes no arguments, touches no memory and cannot fail; it is `unsafe` only as a foreign function.
    let euid = unsafe { libc::geteuid() };
    Ok(m.nlink() > 1 || m.uid() != euid)
}

#[cfg(windows)]
fn must_write_in_place(path: &Path, _: &fs::Metadata) -> Result<bool, Error> {
    file_identity::hard_link(path).map(|identity| identity.is_some()).map_err(|error| map_err(path, error))
}

#[cfg(not(any(unix, windows)))]
fn must_write_in_place(_: &Path, _: &fs::Metadata) -> Result<bool, Error> {
    Ok(false)
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
