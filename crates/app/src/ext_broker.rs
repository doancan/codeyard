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
    /// What the user turned off on the extension page, so a denial says `revoked-by-user` and names the entry.
    pub revoked: Permissions,
    /// Native programs inside the extension's package (`run = ["./bin/server"]`), for the workspace host's platform.
    pub programs: Option<crate::ext_programs::Programs>,
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

    /// The denial for `kind` (`read`, `net` …) of `target`: the user's switch when one they turned off matches it, else
    /// the line to add to the manifest.
    fn refuse(&self, kind: &str, target: &str, revoked: &[String], matches: impl Fn(&str) -> bool, why: v::Why) -> v::Error {
        match revoked.iter().find(|r| matches(r)) {
            Some(r) => denied(format!("{kind} = [{r:?}]"), v::Why::RevokedByUser),
            None => denied(format!("{kind} = [{target:?}]"), why),
        }
    }

    pub fn may_read(&self, path: &str) -> Result<String, v::Error> {
        let rel = self.relative(path).ok_or_else(|| outside("read", path))?;
        if Self::allowed(&self.permissions.read, &rel) {
            Ok(normalize(path).unwrap_or_default())
        } else {
            Err(self.refuse("read", &rel, &self.revoked.read, |g| Self::allowed(&[g.to_string()], &rel), v::Why::NotDeclared))
        }
    }

    pub fn may_write(&self, path: &str) -> Result<String, v::Error> {
        let rel = self.relative(path).ok_or_else(|| outside("write", path))?;
        if Self::allowed(&self.permissions.write, &rel) {
            Ok(normalize(path).unwrap_or_default())
        } else {
            Err(self.refuse("write", &rel, &self.revoked.write, |g| Self::allowed(&[g.to_string()], &rel), self.why(true)))
        }
    }

    /// An edit a code action the user chose makes: the document it was asked for is the user's to change (as with a
    /// command they started), any other file needs `write`.
    pub fn may_edit(&self, document: &str, path: &str) -> Result<String, v::Error> {
        match normalize(path) {
            Some(p) if normalize(document).as_deref() == Some(p.as_str()) => Ok(p),
            _ => self.may_write(path),
        }
    }

    pub fn may_run(&self, program: &str) -> Result<(), v::Error> {
        let same = |p: &str| {
            p == program || (ext_manifest::is_package_program(p) && ext_manifest::package_program_path(p) == ext_manifest::package_program_path(program))
        };
        if self.permissions.run.iter().any(|p| same(p)) { Ok(()) } else { Err(self.refuse("run", program, &self.revoked.run, same, self.why(true))) }
    }

    /// A command the extension may run: its own, a dependency's, or a core command extensions may run
    /// (`ext_manifest::CORE_COMMANDS`) that its manifest lists. What a core command implies is checked when it runs.
    pub fn may_execute(&self, command: &str) -> Result<(), v::Error> {
        if let Some(core) = command.strip_prefix("core:") {
            if ext_manifest::core_command(core).is_none() {
                return Err(v::Error::Invalid(format!(
                    "{command} is not a core command extensions may run; pick one from docs/extensions/reference/core-commands.md"
                )));
            }
            let listed = self.permissions.commands.iter().any(|c| c == command || c == core);
            return if listed { Ok(()) } else { Err(self.refuse("commands", command, &self.revoked.commands, |c| c == command || c == core, self.why(true))) };
        }
        let owner_ok = |owner: &str| command.strip_prefix(owner).is_some_and(|r| r.starts_with('.'));
        if owner_ok(&self.extension) || self.dependencies.iter().any(|d| owner_ok(d)) {
            Ok(())
        } else {
            Err(v::Error::Invalid(format!("{command} is not this extension's command or a dependency's; add the extension to [dependencies]")))
        }
    }

    /// The permission a core command implies, on each of its targets: the files it reads or writes, or the program of
    /// the task it runs (`ext_manifest::core_commands::Implies`).
    pub fn may_imply(&self, implies: ext_manifest::core_commands::Implies, targets: &[String]) -> Result<(), v::Error> {
        use ext_manifest::core_commands::Implies;
        for t in targets {
            match implies {
                Implies::Nothing => {}
                Implies::ReadArgument => drop(self.may_read(t)?),
                Implies::WriteActiveFile | Implies::WriteUnsaved => drop(self.may_write(t)?),
                Implies::RunTask => self.may_run(t)?,
            }
        }
        Ok(())
    }
}

/// No entry reaches outside the workspace, so the denial says that instead of a line to add.
fn outside(kind: &str, path: &str) -> v::Error {
    let reason = "the file is outside the workspace, and extensions reach only files inside its folders";
    v::Error::Denied(v::Denied { permission: ext_manifest::ungrantable_line(&format!("{kind} {path}"), reason), why: v::Why::NotDeclared })
}

/// A path inside the workspace whose symbolic links lead out of it.
fn leads_outside(kind: &str, path: &str, real: &str) -> v::Error {
    let reason = format!("it leads through a symbolic link to {real}, outside the workspace, and extensions reach only files inside its folders");
    v::Error::Denied(v::Denied { permission: ext_manifest::ungrantable_line(&format!("{kind} {path}"), &reason), why: v::Why::NotDeclared })
}

/// The workspace as it is on the host's disk. A path is allowed as written (`Scope::may_read` …) and then again where it
/// really leads, every symbolic link followed on the workspace's host: a link inside the workspace must not reach out of
/// it (`src/link-out -> /etc`), or a file the rules do not allow (`src/settings.txt -> ../.env`, which must be named).
/// The call then reaches the resolved path, so what was checked is what is read or written.
struct OnDisk {
    /// The scope with its roots where they really are.
    scope: Scope,
}

impl OnDisk {
    /// Resolve the roots and `paths` in one round trip to the host.
    fn resolve(host: &dyn Host, scope: &Scope, paths: &[String]) -> (OnDisk, Vec<Result<String, v::Error>>) {
        let mut ask = scope.roots.clone();
        ask.extend(paths.iter().cloned());
        let mut answers: Vec<Result<String, v::Error>> = match host.real_paths(ask) {
            Ok(list) => list.into_iter().map(|r| r.map_err(|e| host_error(host::HostError::Op(e)))).collect(),
            Err(e) => {
                let e = host_error(e);
                std::iter::repeat_with(|| Err(e.clone())).take(scope.roots.len() + paths.len()).collect()
            }
        };
        let rest = answers.split_off(scope.roots.len());
        // A root that cannot be resolved is left out: nothing is allowed through it.
        let roots = answers.into_iter().filter_map(|r| r.ok().and_then(|r| normalize(&r))).collect();
        let real = rest.into_iter().map(|r| r.and_then(|p| normalize(&p).ok_or(v::Error::NotFound(p)))).collect();
        (OnDisk { scope: Scope { roots, ..scope.clone() } }, real)
    }

    /// `path` (allowed as written) where it really is, allowed there too: by `read` or by `write`.
    fn check(&self, write: bool, path: &str, real: Result<String, v::Error>) -> Result<String, v::Error> {
        let real = real?;
        let kind = if write { "write" } else { "read" };
        if self.scope.relative(&real).is_none() {
            return Err(leads_outside(kind, path, &real));
        }
        if write { self.scope.may_write(&real) } else { self.scope.may_read(&real) }
    }

    /// `path` checked as [`check`](Self::check), with one round trip of its own.
    fn one(host: &dyn Host, scope: &Scope, write: bool, path: &str) -> Result<String, v::Error> {
        let (disk, mut real) = Self::resolve(host, scope, &[path.to_string()]);
        disk.check(write, path, real.remove(0))
    }

    /// The entry `path` names (allowed as written), not what it points to, for a delete or rename: its folder where it
    /// really is, then its name, so deleting a link removes the link and renaming one moves it.
    fn entry(host: &dyn Host, scope: &Scope, path: &str) -> Result<String, v::Error> {
        let (folder, name) = path.rsplit_once('/').ok_or_else(|| outside("write", path))?;
        // `/` and `C:/` keep their slash.
        let folder = if folder.is_empty() || folder.ends_with(':') { format!("{folder}/") } else { folder.to_string() };
        let (disk, mut real) = Self::resolve(host, scope, &[folder]);
        let real = real.remove(0).map(|f| format!("{}/{name}", f.trim_end_matches('/')));
        disk.check(true, path, real)
    }
}

/// `may_read`, then the same check where `uri` really leads on the host (symbolic links followed): what a core command
/// that opens a file needs. Blocking: run it on a background thread.
pub fn may_read_on_disk(host: &dyn Host, scope: &Scope, uri: &str) -> Result<String, v::Error> {
    let path = scope.may_read(uri)?;
    OnDisk::one(host, scope, false, &path).map(|_| path)
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
    let read_one = |uri: &str| scope.may_read(uri).and_then(|p| OnDisk::one(host, scope, false, &p));
    let write_one = |uri: &str| scope.may_write(uri).and_then(|p| OnDisk::one(host, scope, true, &p));
    let entry = |uri: &str| scope.may_write(uri).and_then(|p| OnDisk::entry(host, scope, &p));
    match call {
        Start::ReadFile { uri } => done(read_one(uri).and_then(|p| read(host, &p).map(Completion::Bytes))),
        Start::ReadMany { uris } => {
            let uris: Vec<&String> = uris.iter().take(ext_manifest::limits::PERMISSION_ENTRIES * 16).collect();
            let written: Vec<Result<String, v::Error>> = uris.iter().map(|u| scope.may_read(u)).collect();
            let allowed: Vec<String> = written.iter().filter_map(|w| w.as_ref().ok().cloned()).collect();
            let (disk, real) = OnDisk::resolve(host, scope, &allowed);
            let mut real = real.into_iter();
            let files = uris
                .into_iter()
                .zip(written)
                .map(|(u, w)| {
                    let contents = w.and_then(|p| disk.check(false, &p, real.next().unwrap_or_else(|| Err(v::Error::NotFound(p.clone())))));
                    v::FileResult { uri: u.clone(), contents: contents.and_then(|p| read(host, &p)) }
                })
                .collect();
            Some(Completion::Files(files))
        }
        Start::Stat { uri } => done(read_one(uri).and_then(|p| {
            let m = host.stat(&p).map_err(host_error)?;
            Ok(Completion::Stat(v::FileStat { is_dir: m.kind == proto::Kind::Dir, size: m.size, modified: m.mtime_ms.max(0) as u64 }))
        })),
        Start::FindFiles { glob, max } => {
            let mut found = Vec::new();
            for root in &scope.roots {
                let Ok(list) = host.list_files(root, 50_000) else { continue };
                for rel in list.paths {
                    if found.len() >= *max as usize {
                        break;
                    }
                    if crate::globs::path_matches(glob, &rel) && Scope::allowed(&scope.permissions.read, &rel) {
                        found.push(format!("{}/{rel}", root.trim_end_matches('/')));
                    }
                }
            }
            // A link among them is listed only when where it leads may be read too.
            let (disk, real) = OnDisk::resolve(host, scope, &found);
            let out = found.into_iter().zip(real).filter(|(p, r)| disk.check(false, p, r.clone()).is_ok()).map(|(p, _)| uri(&p)).collect();
            Some(Completion::Uris(out))
        }
        Start::Search { query } => done(search(host, scope, query, sink, cancel)),
        Start::WriteFile { uri, contents } => {
            done(write_one(uri).and_then(|p| host.write(&p, contents.clone(), None).map(|_| Completion::Done).map_err(host_error)))
        }
        Start::Delete { uri } => done(entry(uri).and_then(|p| host.delete(&p).map(|()| Completion::Done).map_err(host_error))),
        Start::Rename { from, to } => done(entry(from).and_then(|f| {
            let t = entry(to)?;
            host.rename(&f, &t).map(|()| Completion::Done).map_err(host_error)
        })),
        Start::CreateDirectory { uri } => done(write_one(uri).and_then(|p| host.create_dir(&p).map(|()| Completion::Done).map_err(host_error))),
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
            // Only what the extension may read: a search is a read of every file it reports, where it really is.
            let files: Vec<(String, proto::FileMatches)> = files
                .into_iter()
                .map(|f| (format!("{}/{}", root.trim_end_matches('/'), f.path.trim_start_matches('/')), f))
                .filter(|(_, f)| Scope::allowed(&scope.permissions.read, f.path.trim_start_matches('/')))
                .collect();
            if files.is_empty() {
                continue;
            }
            let paths: Vec<String> = files.iter().map(|(p, _)| p.clone()).collect();
            let (disk, real) = OnDisk::resolve(host, scope, &paths);
            for ((path, f), real) in files.into_iter().zip(real) {
                if disk.check(false, &path, real).is_err() {
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
                    let uri = uri(&path);
                    sink.event(StreamEvent::SearchMatch(v::SearchMatch { uri, range, line: m.preview }));
                    sent += 1;
                }
            }
        }
    }
    Ok(Completion::Done)
}

/// A program from `run`, resolved to an absolute path on the workspace's host, refused when it lives inside the
/// workspace (a repository could otherwise plant `vercel` next to its files). A program inside the package runs from
/// the copy pushed to the host, after its hash is checked there.
fn spawn(host: &dyn Host, scope: &Scope, c: &v::Command, sink: &dyn Sink, cancel: &AtomicBool) -> Result<Completion, v::Error> {
    scope.may_run(&c.program)?;
    let program = if ext_manifest::is_package_program(&c.program) { package_program(host, scope, &c.program)? } else { resolve(host, &c.program)? };
    if scope.relative(&program).is_some() {
        return Err(v::Error::Denied(v::Denied { permission: format!("a program outside the workspace ({program} is inside it)"), why: v::Why::NotDeclared }));
    }
    let cwd = match &c.cwd {
        Some(u) => Some(scope.may_read(u).or_else(|_| normalize(u).filter(|p| scope.relative(p).is_some()).ok_or_else(|| outside("run in", u)))?),
        None => scope.roots.first().cloned(),
    };
    // Only what the extension passes reaches the program, never the user's environment (tokens live there).
    let proc = spawn_clean(host, &program, &c.args, cwd, &c.env).map_err(host_error)?;
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

/// `./bin/server` → the copy of the package's program for the host's platform, pushed to the host and checked by hash
/// there (`ext_programs::push`).
fn package_program(host: &dyn Host, scope: &Scope, program: &str) -> Result<String, v::Error> {
    let programs = scope.programs.as_ref().ok_or_else(|| v::Error::NotFound(format!("{program} is not in the extension's package")))?;
    let target = crate::ext_programs::host_target(host)
        .ok_or_else(|| v::Error::Other(format!("{} is a platform extension programs are not built for", host.label())))?;
    let p = (programs.0)(target, program).map_err(v::Error::NotFound)?;
    crate::ext_programs::push(host, &scope.extension, &p).map_err(v::Error::Other)
}

/// `vercel` → `/usr/local/bin/vercel` on the workspace's host: an executable on the login shell's PATH, never a builtin
/// or a function.
fn resolve(host: &dyn Host, program: &str) -> Result<String, v::Error> {
    if program.contains('/') || program.contains('\\') {
        return normalize(program).ok_or_else(|| v::Error::NotFound(program.to_string()));
    }
    let out = if windows(host) {
        host.exec("where", &[program], None, None)
    } else {
        // The login shell's PATH, but only executables on it; the name is an argument, never part of the script.
        let script = r#"IFS=:; for d in $PATH; do [ -f "$d/$1" ] && [ -x "$d/$1" ] && { echo "$d/$1"; exit 0; }; done; exit 1"#;
        host.exec("sh", &["-lc", script, "sh", program], None, None)
    };
    let out = out.map_err(host_error)?;
    let path = out.stdout_str().lines().next().unwrap_or_default().trim().to_string();
    if out.ok() && !path.is_empty() {
        normalize(&path).ok_or_else(|| v::Error::NotFound(program.to_string()))
    } else {
        Err(v::Error::NotFound(format!("{program} was not found on {}", host.label())))
    }
}

fn windows(host: &dyn Host) -> bool {
    host.info().os == "windows"
}

/// Start `program` with only `env` on top of a minimal environment: the user's variables, where tokens live, never
/// reach it. On Unix hosts through `env -i` (the agent's own spawn is unchanged); Windows hosts run code extensions
/// only once AppContainer is verified, and pass the environment as it is until then.
fn spawn_clean(host: &dyn Host, program: &str, args: &[String], cwd: Option<String>, env: &[(String, String)]) -> host::Result<host::Proc> {
    if windows(host) {
        return host.proc_spawn(program, args.to_vec(), cwd);
    }
    let dir = program.rsplit_once('/').map_or("", |(d, _)| d);
    let home = host.info().home.clone();
    let mut argv = vec!["-i".to_string(), format!("PATH={dir}:/usr/local/bin:/usr/bin:/bin"), format!("HOME={home}"), "LANG=C.UTF-8".into()];
    // A name with `=` would split into another variable, one starting with `-` would be an option of `env`.
    for (k, v) in env.iter().filter(|(k, _)| !k.is_empty() && !k.contains('=') && !k.starts_with('-')) {
        argv.push(format!("{k}={v}"));
    }
    // No `--`: BSD `env` would run it. The program is an absolute path, so it is never taken for an option, and one
    // with `=` in it, which `env` would take for a variable, is refused.
    if program.contains('=') || !program.starts_with('/') {
        return Err(host::HostError::Op(proto::Error::NotFound(format!("{program} cannot be run as an extension program"))));
    }
    argv.push(program.to_string());
    argv.extend(args.iter().cloned());
    host.proc_spawn("/usr/bin/env", argv, cwd)
}

#[cfg(test)]
#[path = "ext_broker_tests.rs"]
mod tests;
