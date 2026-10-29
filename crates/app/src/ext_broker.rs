//! The broker (docs/plugin-architecture.md §6.2): every call a code extension makes that reaches the app is checked
//! here against what it may do in this window (its declared permissions, minus what the user turned off, masked by
//! Workspace Trust: `ext_core::trust::effective`), then against the path, program or command, and only then sent to
//! the window's `Host`, local or SSH alike. The exthost already refused what it could; nothing it sends is trusted.
//!
//! No UI here: calls that need the window (notices, picks, documents, commands) are answered by the workbench. This
//! file runs file, search and process calls on a background thread and returns their completion.

use std::sync::atomic::{AtomicBool, Ordering};

use ext_api::value::{self as v, Completion, Start, StreamEvent};
use ext_manifest::Permissions;
use host::Host;

/// What one extension may do in one window right now.
#[derive(Clone, Debug)]
pub struct Scope {
    pub extension: String,
    /// `ext_core::trust::effective(..).permissions`: declared, minus revoked, masked by trust.
    pub permissions: Permissions,
    /// The workspace folders, as paths on the window's host.
    pub roots: Vec<String>,
    pub trusted: bool,
    /// The extension's own dependencies, whose commands it may run.
    pub dependencies: Vec<String>,
}

fn denied(permission: String, why: v::Why) -> v::Error {
    v::Error::Denied(v::Denied { permission, why })
}

impl Scope {
    /// Why a missing permission is missing: the folder's trust, or the manifest.
    fn why(&self, sends: bool) -> v::Why {
        if sends && !self.trusted { v::Why::UntrustedWorkspace } else { v::Why::NotDeclared }
    }

    /// The workspace-relative path of `path` and its root, if it is inside one.
    pub fn relative(&self, path: &str) -> Option<String> {
        let path = normalize(path)?;
        self.roots.iter().find_map(|root| {
            let root = root.trim_end_matches('/');
            let rest = path.strip_prefix(root)?;
            if rest.is_empty() { Some(String::new()) } else { rest.strip_prefix('/').map(str::to_string) }
        })
    }

    fn allowed(globs: &[String], rel: &str) -> bool {
        // Credentials and repository internals must be named, never reached by a wide glob.
        let hidden = rel.split('/').any(|p| p.starts_with(".env") || p == ".git");
        globs.iter().any(|g| crate::globs::path_matches(g, rel) && (!hidden || g.split('/').any(|p| p.starts_with(".env") || p == ".git")))
    }

    pub fn may_read(&self, path: &str) -> Result<String, v::Error> {
        let rel = self.relative(path).ok_or_else(|| outside(path))?;
        if Self::allowed(&self.permissions.read, &rel) {
            Ok(normalize(path).unwrap_or_default())
        } else {
            Err(denied(format!("read = [\"{rel}\"]"), v::Why::NotDeclared))
        }
    }

    pub fn may_write(&self, path: &str) -> Result<String, v::Error> {
        let rel = self.relative(path).ok_or_else(|| outside(path))?;
        if Self::allowed(&self.permissions.write, &rel) {
            Ok(normalize(path).unwrap_or_default())
        } else {
            Err(denied(format!("write = [\"{rel}\"]"), self.why(true)))
        }
    }

    pub fn may_run(&self, program: &str) -> Result<(), v::Error> {
        if self.permissions.run.iter().any(|p| p == program) { Ok(()) } else { Err(denied(format!("run = [\"{program}\"]"), self.why(true))) }
    }

    /// A command the extension may run: its own, a dependency's, or a core command its manifest lists.
    pub fn may_execute(&self, command: &str) -> Result<(), v::Error> {
        if let Some(core) = command.strip_prefix("core:") {
            let listed = self.permissions.commands.iter().any(|c| c == command || c == core);
            return if listed { Ok(()) } else { Err(denied(format!("commands = [\"{command}\"]"), self.why(true))) };
        }
        let owner_ok = |owner: &str| command.strip_prefix(owner).is_some_and(|r| r.starts_with('.'));
        if owner_ok(&self.extension) || self.dependencies.iter().any(|d| owner_ok(d)) {
            Ok(())
        } else {
            Err(v::Error::Invalid(format!("{command} is not this extension's command or a dependency's; add the extension to [dependencies]")))
        }
    }
}

fn outside(path: &str) -> v::Error {
    v::Error::Denied(v::Denied { permission: format!("a path inside the workspace (not {path})"), why: v::Why::NotDeclared })
}

/// A `file://` URI or a plain path → an absolute path with `/` separators and no `.`/`..` parts, or none.
pub fn normalize(uri_or_path: &str) -> Option<String> {
    let raw = uri_or_path.strip_prefix("file://").unwrap_or(uri_or_path);
    let decoded = percent_decode(raw)?;
    let s = decoded.replace('\\', "/");
    // `/C:/x` from a Windows file URI is `C:/x`.
    let s = match s.as_bytes() {
        [b'/', d, b':', ..] if d.is_ascii_alphabetic() => s[1..].to_string(),
        _ => s,
    };
    let absolute = s.starts_with('/') || s.as_bytes().get(1) == Some(&b':');
    if !absolute {
        return None;
    }
    let (prefix, rest) = match s.strip_prefix('/') {
        Some(rest) => ("/", rest),
        None => s.split_at(3),
    };
    let mut parts: Vec<&str> = Vec::new();
    for p in rest.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            p => parts.push(p),
        }
    }
    Some(format!("{prefix}{}", parts.join("/")))
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(h, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    let out = String::from_utf8(out).ok()?;
    // A decoded NUL or control character in a path is never legitimate.
    (!out.chars().any(char::is_control)).then_some(out)
}

pub fn uri(path: &str) -> String {
    if path.starts_with('/') { format!("file://{path}") } else { format!("file:///{path}") }
}

fn host_error(e: host::HostError) -> v::Error {
    let s = e.to_string();
    if s.contains("not found") || s.contains("No such file") { v::Error::NotFound(s) } else { v::Error::Other(s) }
}

/// Where streamed data for a ticket goes.
pub trait Sink: Send + Sync {
    fn event(&self, event: StreamEvent);
}

/// Runs one call that needs no window, blocking: run it on a background thread. `None` when the call needs the
/// workbench (notices, documents, commands, clipboard, terminal) or another component (`net`, `auth`).
pub fn run(host: &dyn Host, scope: &Scope, call: &Start, sink: &dyn Sink, cancel: &AtomicBool) -> Option<Completion> {
    let done = |r: Result<Completion, v::Error>| Some(r.unwrap_or_else(Completion::Failed));
    match call {
        Start::ReadFile { uri } => done(scope.may_read(uri).and_then(|p| read(host, &p).map(Completion::Bytes))),
        Start::ReadMany { uris } => {
            let files = uris
                .iter()
                .take(ext_manifest::limits::PERMISSION_ENTRIES * 16)
                .map(|u| v::FileResult { uri: u.clone(), contents: scope.may_read(u).and_then(|p| read(host, &p)) })
                .collect();
            Some(Completion::Files(files))
        }
        Start::Stat { uri } => done(scope.may_read(uri).and_then(|p| {
            let m = host.stat(&p).map_err(host_error)?;
            Ok(Completion::Stat(v::FileStat { is_dir: m.kind == proto::Kind::Dir, size: m.size, modified: m.mtime_ms.max(0) as u64 }))
        })),
        Start::FindFiles { glob, max } => {
            let mut out = Vec::new();
            for root in &scope.roots {
                let Ok(list) = host.list_files(root, 50_000) else { continue };
                for rel in list.paths {
                    if out.len() >= *max as usize {
                        break;
                    }
                    if crate::globs::path_matches(glob, &rel) && Scope::allowed(&scope.permissions.read, &rel) {
                        out.push(uri(&format!("{}/{rel}", root.trim_end_matches('/'))));
                    }
                }
            }
            Some(Completion::Uris(out))
        }
        Start::Search { query } => done(search(host, scope, query, sink, cancel)),
        Start::WriteFile { uri, contents } => {
            done(scope.may_write(uri).and_then(|p| host.write(&p, contents.clone(), None).map(|_| Completion::Done).map_err(host_error)))
        }
        Start::Delete { uri } => done(scope.may_write(uri).and_then(|p| host.delete(&p).map(|()| Completion::Done).map_err(host_error))),
        Start::Rename { from, to } => done(scope.may_write(from).and_then(|f| {
            let t = scope.may_write(to)?;
            host.rename(&f, &t).map(|()| Completion::Done).map_err(host_error)
        })),
        Start::CreateDirectory { uri } => done(scope.may_write(uri).and_then(|p| host.create_dir(&p).map(|()| Completion::Done).map_err(host_error))),
        Start::Spawn { command } => done(spawn(host, scope, command, sink, cancel)),
        _ => None,
    }
}

fn read(host: &dyn Host, path: &str) -> Result<Vec<u8>, v::Error> {
    let meta = host.stat(path).map_err(host_error)?;
    if meta.size as usize > ext_manifest::limits::CALL_BYTES {
        return Err(v::Error::Invalid(format!("{path} is larger than {} MiB", ext_manifest::limits::CALL_BYTES >> 20)));
    }
    host.read(path).map(|(data, _)| data).map_err(host_error)
}

fn search(host: &dyn Host, scope: &Scope, q: &v::SearchQuery, sink: &dyn Sink, cancel: &AtomicBool) -> Result<Completion, v::Error> {
    let mut sent = 0u32;
    for root in &scope.roots {
        let query = proto::SearchQuery {
            root: root.clone(),
            pattern: q.pattern.clone(),
            regex: q.regex,
            case_sensitive: q.case_sensitive,
            whole_word: q.whole_word,
            include: q.include.clone(),
            exclude: Vec::new(),
            max_results: q.max,
        };
        let stream = host.search(query).map_err(host_error)?;
        for msg in stream.rx.iter() {
            let files = match msg {
                proto::Response::SearchChunk(files) => files,
                _ => break,
            };
            for f in files {
                let rel = f.path.trim_start_matches('/').to_string();
                // Only what the extension may read: a search is a read of every file it reports.
                if !Scope::allowed(&scope.permissions.read, &rel) {
                    continue;
                }
                for m in f.matches {
                    if cancel.load(Ordering::Relaxed) || sent >= q.max {
                        host.cancel(stream.id);
                        return Ok(Completion::Done);
                    }
                    let width = m.ranges.first().map_or(0, |(a, b)| b.saturating_sub(*a));
                    let line = m.line.saturating_sub(1);
                    let range = v::Range { start: v::Position { line, character: m.column }, end: v::Position { line, character: m.column + width } };
                    let uri = uri(&format!("{}/{rel}", root.trim_end_matches('/')));
                    sink.event(StreamEvent::SearchMatch(v::SearchMatch { uri, range, line: m.preview }));
                    sent += 1;
                }
            }
        }
    }
    Ok(Completion::Done)
}

/// A program from `run`, resolved to an absolute path on the workspace's host, refused when it lives inside the
/// workspace (a repository could otherwise plant `vercel` next to its files).
fn spawn(host: &dyn Host, scope: &Scope, c: &v::Command, sink: &dyn Sink, cancel: &AtomicBool) -> Result<Completion, v::Error> {
    scope.may_run(&c.program)?;
    let program = resolve(host, &c.program)?;
    if scope.relative(&program).is_some() {
        return Err(v::Error::Denied(v::Denied { permission: format!("a program outside the workspace ({program} is inside it)"), why: v::Why::NotDeclared }));
    }
    let cwd = match &c.cwd {
        Some(u) => Some(scope.may_read(u).or_else(|_| normalize(u).filter(|p| scope.relative(p).is_some()).ok_or_else(|| outside(u)))?),
        None => scope.roots.first().cloned(),
    };
    // Only what the extension passes reaches the program, never the user's environment (tokens live there).
    let proc = host.proc_spawn_env(&program, c.args.clone(), cwd, c.env.clone()).map_err(host_error)?;
    if let Some(input) = &c.stdin {
        host.proc_input(proc.id, input.clone());
    }
    loop {
        if cancel.load(Ordering::Relaxed) {
            host.proc_kill(proc.id);
            return Ok(Completion::Exit(None));
        }
        match proc.events.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(host::PtyEvent::Output(bytes)) => sink.event(StreamEvent::Stdout(bytes)),
            Ok(host::PtyEvent::Exit(code)) => return Ok(Completion::Exit(code)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(Completion::Exit(None)),
        }
    }
}

/// `vercel` → `/usr/local/bin/vercel` on the workspace's host: an executable on the login shell's PATH (`Which`),
/// never a builtin or a function.
fn resolve(host: &dyn Host, program: &str) -> Result<String, v::Error> {
    if program.contains('/') || program.contains('\\') {
        return normalize(program).ok_or_else(|| v::Error::NotFound(program.to_string()));
    }
    match host.which(program).map_err(host_error)? {
        Some(path) => normalize(&path).ok_or_else(|| v::Error::NotFound(program.to_string())),
        None => Err(v::Error::NotFound(format!("{program} was not found on {}", host.label()))),
    }
}

#[cfg(test)]
#[path = "ext_broker_tests.rs"]
mod tests;
