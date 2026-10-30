//! Protocol between the UI and the agent. The same messages are used in-process locally and
//! remotely over `ssh` stdio.
//!
//! Frame: 4-byte little-endian length + `Frame` encoded with postcard.
//!
//! Multiplexing: each request has an `id`; replies carry the same `id` (repeatedly for
//! streams like search). `id == 0` is special: agent → client it's an unsolicited
//! event (terminal output), client → agent it means "no reply wanted"
//! (terminal input, so a keystroke doesn't wait for a round trip).
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

/// Prefix of a daemon session id that only one live connection may hold (a window's terminals on the local terminal
/// daemon): a second `Hello` for it is refused while the first connection lives. Other sessions are taken over by the
/// newest connection (an SSH reconnect while the old connection is still half-open).
pub const EXCLUSIVE_SESSION: &str = "x-";

/// Compared in the handshake; on mismatch the agent is reinstalled.
pub const VERSION: u32 = 17;

/// Max size of one frame. Larger files are read in chunks.
pub const MAX_FRAME: usize = 64 * 1024 * 1024;

/// Largest file the agent returns in one go.
pub const MAX_FILE: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frame<T> {
    pub id: u32,
    pub body: T,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Request {
    /// `session`: ID of the previous session when reconnecting (in daemon mode
    /// terminals and watches resume where they were). A session id that starts with [`EXCLUSIVE_SESSION`] is refused
    /// with `PermissionDenied` while another connection is attached to it, instead of being taken over.
    Hello {
        client_version: u32,
        session: Option<String>,
    },
    /// Client is leaving on purpose (window closed): the daemon ends the session without waiting `GRACE`; shells and processes stop at once.
    Bye,
    /// Liveness probe: the dispatch thread answers `Done` at once (independent of the pool and processes).
    Ping,
    ReadDir {
        path: String,
    },
    Stat {
        path: String,
    },
    Read {
        path: String,
    },
    /// `expect_hash`: hash of the version the user started editing.
    /// If the file on disk differs nothing is written and `Conflict` comes back. `None`: new file
    /// or deliberate overwrite.
    Write {
        path: String,
        data: Vec<u8>,
        expect_hash: Option<u64>,
    },
    /// Files under the root (relative paths), honouring .gitignore. For quick open.
    ListFiles {
        root: String,
        limit: u32,
    },
    /// New empty file; `AlreadyExists` if present (never overwrites).
    CreateFile {
        path: String,
    },
    CreateDir {
        path: String,
    },
    /// `AlreadyExists` if the target exists.
    Rename {
        from: String,
        to: String,
    },
    /// File or folder (with its contents). The root and the home directory are refused.
    Delete {
        path: String,
    },
    /// Search in files: `SearchChunk` parts, then `SearchDone`.
    Search(SearchQuery),
    /// Stop a running stream (search).
    Cancel {
        id: u32,
    },
    /// Open a shell. `shell`: the user's shell (`$SHELL`) if unset.
    PtySpawn {
        cols: u16,
        rows: u16,
        cwd: Option<String>,
        shell: Option<String>,
    },
    PtyInput {
        pty: u32,
        data: Vec<u8>,
    },
    PtyResize {
        pty: u32,
        cols: u16,
        rows: u16,
    },
    PtyKill {
        pty: u32,
    },
    /// Change the watched path set (open files, folders expanded in the tree).
    /// Changes arrive as `FsChanged` events.
    Watch {
        paths: Vec<String>,
    },
    /// Run the command, `Exited` when done (git). A bare program name is looked up with the login shell.
    Exec {
        program: String,
        args: Vec<String>,
        cwd: Option<String>,
        stdin: Option<Vec<u8>>,
    },
    /// Long-running process over pipes (language server). Output as `ProcOutput` events.
    /// Processes are terminated if the connection drops; the client restarts them.
    ProcSpawn {
        program: String,
        args: Vec<String>,
        cwd: Option<String>,
    },
    ProcInput {
        proc: u32,
        data: Vec<u8>,
    },
    ProcKill {
        proc: u32,
    },
    /// Large file: line count and size (`LineIndex`); the index is cached in the agent.
    LineIndex {
        path: String,
    },
    /// Range of lines from a large file (`Lines`).
    ReadLines {
        path: String,
        start: u64,
        count: u32,
    },
    /// Folder comparison. `only: None` walks the root and streams a record per entry without hashing (`HashChunk`, then
    /// `HashDone`); `only: Some(paths)` hashes exactly those files (relative to the root, never `..` or absolute) and
    /// streams the same records with `hash` set. Symlinks are never followed. `ignore`: globs (gitignore syntax) to skip;
    /// `gitignore`: also honour `.gitignore` files. `limit` caps the number of entries walked.
    HashTree {
        root: String,
        ignore: Vec<String>,
        gitignore: bool,
        limit: u32,
        only: Option<Vec<String>>,
    },
    /// Open a terminal with a chosen program: `args` replace the default login flag, `env` is added to the environment;
    /// `integration` sets up shell integration (prompt and folder marks) for zsh, bash and fish started as login shells.
    /// Answered with `PtySpawned` like `PtySpawn`.
    PtySpawnEx {
        cols: u16,
        rows: u16,
        cwd: Option<String>,
        shell: Option<String>,
        args: Option<Vec<String>>,
        env: Vec<(String, String)>,
        integration: bool,
    },
    /// Is a program running in the foreground of this terminal, and where is its shell? (`PtyStatus`)
    PtyStatus {
        pty: u32,
    },
    /// The shells this machine offers (`Shells`).
    ListShells,
    /// Read one regular file beneath `root` through directory handles, without following symlinks.
    /// `path` is a nonempty '/'-separated relative path; hidden and special components are refused.
    /// Answers `File`, or `Error` (including `IsDirectory` and `TooLarge`).
    ReadPreview {
        root: String,
        path: String,
    },
    /// Copy a file, directory or symlink without following links. Publishes atomically;
    /// `AlreadyExists` if the destination is occupied. Failed copies leave no destination.
    Copy {
        from: String,
        to: String,
    },
    /// Run a task: answered with `ProcSpawned`, then `ProcOutput` (stdout and stderr in one stream, as written) and
    /// `ProcExit`. It leads its own process group (a job object on Windows), so `ProcKill` stops what it started too;
    /// the `ProcExit` after a kill has `code: None`. Standard input is closed.
    TaskSpawn {
        command: TaskCommand,
        cwd: Option<String>,
        /// Set (`Some`) or remove (`None`) variables, in order, on top of the agent's environment.
        env: Vec<(String, Option<String>)>,
    },
    /// The token a client keeps to attach to this terminal again after it restarts (`PtyToken`): the agent process's
    /// random instance id and a random secret of this terminal. Never a process id.
    PtyToken {
        pty: u32,
    },
    /// Take this session's terminal `pty` back after a restart, proving it with its token (`PtyAttached`). Refused with
    /// `NotFound` when the terminal has ended, and with `PermissionDenied` when the instance or secret is not the one this
    /// agent gave (another agent process, a guessed or stale token).
    PtyAttach {
        pty: u32,
        instance: String,
        secret: String,
    },
    /// The session's open terminals (`PtyList`), so a client can close the ones it did not take back.
    PtyList,
    /// Daemon mode: keep this session (its terminals) this many seconds after its connection ends, instead of the
    /// daemon's default; 0 ends it as soon as the connection ends. No reply.
    KeepFor {
        secs: u64,
    },
}

/// What `TaskSpawn` runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TaskCommand {
    /// `program` with exactly these arguments; a bare name is looked up on the user's PATH (login shell on Unix).
    Process { program: String, args: Vec<String> },
    /// `line` passed to a shell as one argument: `shell` (default: the user's login shell, `cmd.exe` on Windows) with
    /// `args` before it (default: `-l -c`, `/d /s /c` for cmd.exe).
    Shell { shell: Option<String>, args: Option<Vec<String>>, line: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ChangeKind {
    Created,
    Modified,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchQuery {
    pub root: String,
    pub pattern: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    /// Globs (gitignore syntax): everything if empty.
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub max_results: u32,
}

/// One entry of a `HashTree` stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HashRecord {
    /// Relative to the root, '/' separated.
    pub path: String,
    pub kind: Kind,
    pub size: u64,
    /// Unix epoch milliseconds.
    pub mtime_ms: i64,
    /// `content_hash` of the file; `None` when it was not asked for or could not be read (see `error`).
    pub hash: Option<u64>,
    /// Target of a symlink, as stored in the link.
    pub link: Option<String>,
    /// Why this entry could not be read (permission denied...); the walk went on.
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileMatches {
    /// Relative to the root.
    pub path: String,
    pub matches: Vec<LineMatch>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineMatch {
    /// 1-based.
    pub line: u32,
    /// Preview of the line (long lines are truncated).
    pub preview: String,
    /// Matches within the preview (bytes).
    pub ranges: Vec<(u32, u32)>,
    /// Column of the first match in the line (characters, 0-based), to scroll to it.
    pub column: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Response {
    Hello(HostInfo),
    Entries(Vec<DirEntry>),
    Stat(Meta),
    File {
        data: Vec<u8>,
        hash: u64,
    },
    Written {
        hash: u64,
    },
    Files {
        paths: Vec<String>,
        truncated: bool,
    },
    Done,
    SearchChunk(Vec<FileMatches>),
    SearchDone {
        files: u32,
        matches: u32,
        truncated: bool,
        cancelled: bool,
    },
    HashChunk(Vec<HashRecord>),
    HashDone {
        entries: u32,
        truncated: bool,
        cancelled: bool,
    },
    PtySpawned {
        pty: u32,
    },
    /// Event (id 0).
    PtyOutput {
        pty: u32,
        data: Vec<u8>,
    },
    /// Event (id 0).
    PtyExit {
        pty: u32,
        code: Option<i32>,
    },
    /// Event (id 0): change in the watched paths.
    FsChanged(Vec<(String, ChangeKind)>),
    Error(Error),
    Exited {
        code: Option<i32>,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    ProcSpawned {
        proc: u32,
    },
    /// Event (id 0).
    ProcOutput {
        proc: u32,
        data: Vec<u8>,
    },
    /// Event (id 0).
    ProcExit {
        proc: u32,
        code: Option<i32>,
    },
    LineIndex {
        lines: u64,
        size: u64,
    },
    Lines(Vec<String>),
    PtyStatus {
        /// A program other than the shell holds the terminal (`None`: cannot tell here).
        busy: Option<bool>,
        /// Its name.
        name: Option<String>,
        /// The shell's working folder.
        cwd: Option<String>,
    },
    Shells(Vec<String>),
    PtyToken {
        instance: String,
        secret: String,
    },
    PtyAttached {
        pty: u32,
        /// Bytes the terminal has written since it started (its output sequence number).
        sequence: u64,
        /// Bytes of its output lost while no client was attached (the backlog keeps the last 256 KB).
        dropped: u64,
    },
    PtyList(Vec<u32>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostInfo {
    pub version: u32,
    pub os: String,
    pub arch: String,
    pub home: String,
    pub hostname: String,
    /// Reconnected to the previous session (terminals are alive).
    pub resumed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    File,
    Dir,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub kind: Kind,
    /// If a symlink, the type of the target (broken link: `None`).
    pub target_kind: Option<Kind>,
}

impl DirEntry {
    pub fn is_dir(&self) -> bool {
        self.kind == Kind::Dir || self.target_kind == Some(Kind::Dir)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub kind: Kind,
    pub size: u64,
    /// Unix epoch milliseconds.
    pub mtime_ms: i64,
    pub readonly: bool,
    /// The file's identity on its filesystem (Unix: device and inode mixed into one value); 0 where unknown.
    /// Writing to a file or into a folder keeps it; deleting and creating the path again changes it.
    pub file_id: u64,
    /// Creation time in Unix epoch milliseconds; 0 where the filesystem does not keep it.
    pub created_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Error {
    NotFound(String),
    PermissionDenied(String),
    IsDirectory(String),
    AlreadyExists(String),
    TooLarge {
        path: String,
        size: u64,
        limit: u64,
    },
    /// The disk differs from the version the edit started from.
    Conflict {
        path: String,
        current_hash: u64,
    },
    VersionMismatch {
        agent: u32,
        client: u32,
    },
    InvalidPattern(String),
    Unsupported(String),
    Other(String),
    Cancelled,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Cancelled => write!(f, "operation cancelled"),
            Error::NotFound(p) => write!(f, "not found: {p}"),
            Error::PermissionDenied(p) => write!(f, "permission denied: {p}"),
            Error::IsDirectory(p) => write!(f, "is a folder: {p}"),
            Error::AlreadyExists(p) => write!(f, "already exists: {p}"),
            Error::TooLarge { path, size, limit } => {
                write!(f, "file too large ({} MB, limit {} MB): {path}", size >> 20, limit >> 20)
            }
            Error::Conflict { path, .. } => write!(f, "changed on disk: {path}"),
            Error::VersionMismatch { agent, client } => {
                write!(f, "protocol version mismatch (agent {agent}, client {client})")
            }
            Error::InvalidPattern(m) => write!(f, "invalid search pattern: {m}"),
            Error::Unsupported(m) => write!(f, "not supported here: {m}"),
            Error::Other(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

/// Hash of the file contents: for conflict detection (not cryptographic).
pub fn content_hash(data: &[u8]) -> u64 {
    xxhash_rust::xxh3::xxh3_64(data)
}

/// `content_hash` of a stream, for files too big to hold in memory (same value as hashing the bytes at once).
pub fn content_hash_reader(r: &mut impl Read) -> io::Result<u64> {
    let mut hasher = xxhash_rust::xxh3::Xxh3::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        match r.read(&mut buf) {
            Ok(0) => return Ok(hasher.digest()),
            Ok(n) => hasher.update(&buf[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
}

pub fn write_frame<T: Serialize>(w: &mut impl Write, frame: &Frame<T>) -> io::Result<()> {
    // `InvalidData`: rejected before any byte was written, so the connection is intact (oversized reply or unserializable
    // body). Real write errors use other kinds.
    let bytes = postcard::to_stdvec(frame).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if bytes.len() > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("frame too large: {} bytes", bytes.len())));
    }
    w.write_all(&(bytes.len() as u32).to_le_bytes())?;
    w.write_all(&bytes)?;
    w.flush()
}

/// `Ok(None)` if the stream closed cleanly.
pub fn read_frame<T: for<'de> Deserialize<'de>>(r: &mut impl Read) -> io::Result<Option<Frame<T>>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len[..1]) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    r.read_exact(&mut len[1..])?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::other(format!("frame too large: {len} bytes")));
    }
    // Grow with what actually arrives: a peer that announces a huge frame and stalls must not pin `len` bytes.
    let mut buf = Vec::new();
    r.take(len as u64).read_to_end(&mut buf)?;
    if buf.len() < len {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    postcard::from_bytes(&buf).map(Some).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_roundtrip() {
        let req = Frame { id: 7, body: Request::Write { path: "a/ğüş.rs".into(), data: vec![1, 2, 3], expect_hash: Some(9) } };
        let mut buf = Vec::new();
        write_frame(&mut buf, &req).unwrap();
        write_frame(&mut buf, &Frame { id: 8, body: Request::Hello { client_version: VERSION, session: None } }).unwrap();
        let mut r = buf.as_slice();
        let a: Frame<Request> = read_frame(&mut r).unwrap().unwrap();
        let b: Frame<Request> = read_frame(&mut r).unwrap().unwrap();
        assert_eq!((a.id, a.body), (7, req.body));
        assert_eq!(b.id, 8);
        assert!(read_frame::<Request>(&mut r).unwrap().is_none());
    }

    #[test]
    fn hash_tree_messages_roundtrip() {
        let req = Request::HashTree {
            root: "~/src".into(),
            ignore: vec![".git".into(), "target".into()],
            gitignore: true,
            limit: 50_000,
            only: Some(vec!["a/b.rs".into()]),
        };
        let rec =
            HashRecord { path: "ğ/x".into(), kind: Kind::Symlink, size: 3, mtime_ms: -5, hash: None, link: Some("../y".into()), error: Some("denied".into()) };
        let mut buf = Vec::new();
        write_frame(&mut buf, &Frame { id: 4, body: req.clone() }).unwrap();
        write_frame(&mut buf, &Frame { id: 4, body: Response::HashChunk(vec![rec.clone()]) }).unwrap();
        write_frame(&mut buf, &Frame { id: 4, body: Response::HashDone { entries: 1, truncated: true, cancelled: false } }).unwrap();
        let mut r = buf.as_slice();
        assert_eq!(read_frame::<Request>(&mut r).unwrap().unwrap().body, req);
        assert_eq!(read_frame::<Response>(&mut r).unwrap().unwrap().body, Response::HashChunk(vec![rec]));
        assert_eq!(read_frame::<Response>(&mut r).unwrap().unwrap().body, Response::HashDone { entries: 1, truncated: true, cancelled: false });
    }

    #[test]
    fn streamed_hash_equals_one_shot_hash() {
        let data: Vec<u8> = (0..700_000u32).map(|i| (i % 251) as u8).collect();
        assert_eq!(content_hash_reader(&mut data.as_slice()).unwrap(), content_hash(&data));
        assert_eq!(content_hash_reader(&mut [].as_slice()).unwrap(), content_hash(&[]));
    }

    #[test]
    fn truncated_header_is_an_error_not_a_clean_close() {
        assert!(read_frame::<Request>(&mut [].as_slice()).unwrap().is_none());
        for count in 1..4 {
            let bytes = [1u8, 0, 0];
            let err = read_frame::<Request>(&mut &bytes[..count]).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof, "header length {count}");
        }
    }

    #[test]
    fn truncated_body_is_an_error_not_a_clean_close() {
        let mut bytes = (MAX_FRAME as u32).to_le_bytes().to_vec();
        bytes.extend_from_slice(&[1, 2, 3]);
        let err = read_frame::<Request>(&mut bytes.as_slice()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    /// Records the largest single allocation made by the current thread, so a test can see a buffer sized from an untrusted length prefix.
    struct Biggest;
    thread_local! { static BIGGEST: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
    // SAFETY: forwards every call unchanged to the system allocator; the bookkeeping touches only a const-initialised thread-local.
    unsafe impl std::alloc::GlobalAlloc for Biggest {
        unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
            let _ = BIGGEST.try_with(|b| b.set(b.get().max(layout.size())));
            unsafe { std::alloc::System.alloc(layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
            unsafe { std::alloc::System.dealloc(ptr, layout) }
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, new_size: usize) -> *mut u8 {
            let _ = BIGGEST.try_with(|b| b.set(b.get().max(new_size)));
            unsafe { std::alloc::System.realloc(ptr, layout, new_size) }
        }
    }
    #[global_allocator]
    static ALLOC: Biggest = Biggest;

    /// Prefix says `MAX_FRAME`, then 10 body bytes arrive and the peer stalls. The call must fail on the stalled read without ever having
    /// allocated a buffer the size of the prefix (a `vec![0; len]` or `with_capacity(len)` would pin 64 MiB per connection).
    #[test]
    fn announced_length_is_not_preallocated() {
        struct Stalled {
            data: Vec<u8>,
            at: usize,
        }
        impl Read for Stalled {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                if self.at < self.data.len() {
                    buf[0] = self.data[self.at];
                    self.at += 1;
                    return Ok(1);
                }
                Err(io::ErrorKind::TimedOut.into())
            }
        }
        let mut data = (MAX_FRAME as u32).to_le_bytes().to_vec();
        data.extend_from_slice(&[7; 10]);
        BIGGEST.with(|b| b.set(0));
        let err = read_frame::<Request>(&mut Stalled { data, at: 0 }).unwrap_err();
        let biggest = BIGGEST.with(|b| b.get());
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(biggest < 1024 * 1024, "read_frame allocated {biggest} bytes for a frame that never arrived (MAX_FRAME is {MAX_FRAME})");
    }

    /// Valid frames of both directions with bytes flipped, cut short or stretched: decoding returns an error or a value,
    /// never panics or allocates without bound (the agent decodes what the app sends, the app what a server's agent sends).
    #[test]
    fn mutated_frames_never_panic() {
        let mut seed = 0x2545F4914F6CDD1Du64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut samples: Vec<Vec<u8>> = Vec::new();
        let requests = [
            Request::Write { path: "a/ğüş.rs".into(), data: vec![1, 2, 3], expect_hash: Some(9) },
            Request::ListFiles { root: "/".into(), limit: 10 },
            Request::Hello { client_version: VERSION, session: None },
            Request::ReadPreview { root: "/preview".into(), path: "assets/style.css".into() },
            Request::Copy { from: "/source".into(), to: "/copy".into() },
        ];
        for body in requests {
            let mut b = Vec::new();
            write_frame(&mut b, &Frame { id: 1, body }).unwrap();
            samples.push(b);
        }
        let responses =
            [Response::Files { paths: vec!["a".into(), "b/c".into()], truncated: true }, Response::Error(Error::NotFound("x".into())), Response::Done];
        for body in responses {
            let mut b = Vec::new();
            write_frame(&mut b, &Frame { id: 2, body }).unwrap();
            samples.push(b);
        }
        for _ in 0..20_000 {
            let mut b = samples[(next() % samples.len() as u64) as usize].clone();
            for _ in 0..(1 + next() % 4) {
                if b.len() <= 4 {
                    break;
                }
                let at = 4 + (next() as usize) % (b.len() - 4);
                match next() % 4 {
                    0 => b[at] = next() as u8,
                    1 => b.truncate(at),
                    2 => b.splice(at..at, (0..(next() % 9)).map(|_| 0xff)).for_each(drop),
                    _ => b[at] ^= 0x80,
                }
            }
            // Keep the length header honest half of the time, so the body itself is decoded.
            if next() % 2 == 0 && b.len() >= 4 {
                let n = (b.len() - 4) as u32;
                b[..4].copy_from_slice(&n.to_le_bytes());
            }
            BIGGEST.with(|c| c.set(0));
            let _ = read_frame::<Request>(&mut b.as_slice());
            let _ = read_frame::<Response>(&mut b.as_slice());
            assert!(BIGGEST.with(|c| c.get()) < 16 * 1024 * 1024, "a mutated frame made the decoder allocate {} bytes", BIGGEST.with(|c| c.get()));
        }
    }

    #[test]
    fn garbage_body_is_an_error() {
        let mut bytes = 5u32.to_le_bytes().to_vec();
        bytes.extend_from_slice(&[0xff; 5]);
        assert!(read_frame::<Request>(&mut bytes.as_slice()).is_err());
    }

    #[test]
    fn rejects_oversized_frame_header() {
        let mut r: &[u8] = &(u32::MAX).to_le_bytes();
        // Without the cap this also fails (the body is missing), but as `UnexpectedEof` after trying to read 4 GiB: check it is the cap.
        BIGGEST.with(|b| b.set(0));
        let err = read_frame::<Request>(&mut r).unwrap_err();
        assert!(err.to_string().contains("too large"), "{err}");
        assert_ne!(err.kind(), io::ErrorKind::UnexpectedEof);
        assert!(BIGGEST.with(|b| b.get()) < 1024 * 1024, "the header alone must not cause a big allocation");
    }
    #[test]
    fn copy_appends_after_the_existing_preview_request() {
        for (request, tag) in
            [(Request::ReadPreview { root: "/root".into(), path: "file.txt".into() }, 29), (Request::Copy { from: "/from".into(), to: "/to".into() }, 30)]
        {
            let bytes = postcard::to_stdvec(&request).unwrap();
            assert_eq!(bytes[0], tag);
            assert_eq!(postcard::from_bytes::<Request>(&bytes).unwrap(), request);
        }
    }
}
