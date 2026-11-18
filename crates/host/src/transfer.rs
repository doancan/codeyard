//! File transfer: upload/download of files and folders between the computer and a server (or container/WSL).
//!
//! Streams (nothing held in memory, no 50 MB cap): a file goes through `cat`, a folder through `tar`,
//! piped into the target's `sh -c`; ssh reuses ControlMaster where supported. Publication uses a separate command.
//! Progress is in bytes; cancellation removes owned staging data. Already published destination changes may remain.
//!
//! Security: server `tar` output is extracted with local `tar -x`; GNU tar and bsdtar both reject/strip
//! leading `/` and `..` paths by default, and `--no-same-owner` skips applying ownership.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::ssh::{script_command, sh_quote};
use crate::{HostError, Result};

const CHUNK: usize = 256 * 1024;

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

/// Only an exclusively created directory is ours to clean up. A basename from
/// the source must never identify an existing user's file as temporary data.
struct Scratch(PathBuf);

impl Scratch {
    fn new(parent: &Path) -> Result<Self> {
        for _ in 0..100 {
            let path = parent.join(format!(".codeyard-transfer-{}-{}", std::process::id(), NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed)));
            let builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            let builder = {
                use std::os::unix::fs::DirBuilderExt;
                let mut builder = builder;
                builder.mode(0o700);
                builder
            };
            match builder.create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(fail(format!("{}: {error}", path.display()))),
            }
        }
        Err(fail("could not reserve a transfer staging directory"))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fail(msg: impl Into<String>) -> HostError {
    HostError::Disconnected(msg.into())
}

fn check_cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) { Err(HostError::Op(proto::Error::Cancelled)) } else { Ok(()) }
}

/// Builds the command that runs `sh -c <script>` on the target (ssh/docker/wsl in production, local `sh` in tests).
pub type Shell<'a> = &'a dyn Fn(&str) -> Command;

pub fn shell_for(alias: &str) -> impl Fn(&str) -> Command + use<> {
    let alias = alias.to_string();
    move |script| {
        // Invalid targets must fail before any remote command is executed.
        script_command(&alias, script).unwrap_or_else(|_| Command::new("invalid-codeyard-target"))
    }
}

/// Local `sh -c` (for tests).
pub fn local_shell(script: &str) -> Command {
    let mut c = Command::new("sh");
    c.arg("-c").arg(script);
    c
}

/// Total size in bytes of a file or folder (for progress percentage).
pub fn local_size(path: &Path) -> u64 {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => std::fs::read_dir(path).map(|d| d.flatten().map(|e| local_size(&e.path())).sum()).unwrap_or(0),
        Ok(m) => m.len(),
        Err(_) => 0,
    }
}

fn spawn(mut cmd: Command, stdin: Stdio, stdout: Stdio) -> Result<Child> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    cmd.stdin(stdin).stdout(stdout).stderr(Stdio::piped()).spawn().map_err(|e| fail(format!("could not start {program}: {e}")))
}

/// A stdio handle requested with `Stdio::piped()`; absence is an error rather than a panic.
fn piped<T>(handle: Option<T>) -> Result<T> {
    handle.ok_or_else(|| fail("child stdio was not piped"))
}

/// Turn stderr into an error when the process fails.
fn finish(mut child: Child, what: &str) -> Result<()> {
    let mut err = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut err);
    }
    let status = child.wait().map_err(|e| fail(e.to_string()))?;
    if status.success() {
        Ok(())
    } else {
        let err = err.trim();
        Err(fail(if err.is_empty() { format!("{what} failed ({status})") } else { err.to_string() }))
    }
}

/// Keep stdin open while the remote merge runs. EOF asks its watcher to stop
/// the extractor; a grace period handles a peer which no longer responds.
fn finish_folder_commit(mut child: Child, cancel: &AtomicBool) -> Result<()> {
    let (send, receive) = std::sync::mpsc::channel();
    if let Some(mut stderr) = child.stderr.take() {
        std::thread::spawn(move || {
            let mut captured = Vec::new();
            let mut chunk = [0u8; 1024];
            while let Ok(count) = stderr.read(&mut chunk) {
                if count == 0 {
                    break;
                }
                let keep = count.min((64 * 1024usize).saturating_sub(captured.len()));
                captured.extend_from_slice(&chunk[..keep]);
            }
            let _ = send.send(captured);
        });
    }
    let mut interrupted = None;
    let status = loop {
        if cancel.load(Ordering::Relaxed) && interrupted.is_none() {
            drop(child.stdin.take());
            interrupted = Some(std::time::Instant::now());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                kill(&mut child);
                return Err(fail(error.to_string()));
            }
        }
        if interrupted.is_some_and(|start| start.elapsed() >= std::time::Duration::from_secs(2)) {
            kill(&mut child);
            break child.wait().map_err(|e| fail(e.to_string()))?;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let stderr = receive.recv_timeout(std::time::Duration::from_secs(1)).unwrap_or_default();
    if interrupted.is_some() {
        // What the remote side said before it stopped is the best clue to why, so it travels with the message.
        let said = String::from_utf8_lossy(&stderr);
        let said = said.trim();
        let message = "folder finalization was interrupted; completed destination files are kept";
        return Err(fail(if said.is_empty() { message.to_string() } else { format!("{message}\n{said}") }));
    }
    if status.success() {
        Ok(())
    } else {
        let message = String::from_utf8_lossy(&stderr);
        Err(fail(if message.trim().is_empty() { format!("folder upload failed ({status})") } else { message.trim().to_string() }))
    }
}

/// After cancel/error: delete only this upload's reported staging file.
fn cleanup_part(shell: Shell, part: &str) {
    let script = format!("rm -f -- {}", sh_quote(part));
    if let Ok(mut c) = spawn(shell(&script), Stdio::null(), Stdio::null()) {
        let _ = c.wait();
    }
}

fn upload_stage(child: &mut Child, directory: &str) -> Result<String> {
    let result = read_upload_stage(child, directory);
    if let Err(error) = result {
        kill(child);
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            let _ = pipe.read_to_string(&mut stderr);
        }
        Err(if stderr.trim().is_empty() { error } else { fail(stderr.trim()) })
    } else {
        result
    }
}

fn read_upload_stage(child: &mut Child, directory: &str) -> Result<String> {
    let mut bytes = Vec::new();
    BufReader::new(piped(child.stdout.take())?).take(64 * 1024).read_until(0, &mut bytes).map_err(|e| fail(e.to_string()))?;
    if bytes.pop() != Some(0) {
        return Err(fail("the server did not report an upload staging path"));
    }
    let path = String::from_utf8(bytes).map_err(|_| fail("invalid upload staging path"))?;
    let prefix = format!("{}/.codeyard-upload-", directory.trim_end_matches('/'));
    if !path.strip_prefix(&prefix).is_some_and(|suffix| suffix.len() == 8 && suffix.bytes().all(|b| b.is_ascii_alphanumeric())) {
        return Err(fail("invalid upload staging path"));
    }
    Ok(path)
}

/// User-facing reason for an I/O error; a dropped connection is named explicitly.
fn io_reason(e: &std::io::Error) -> String {
    use std::io::ErrorKind::*;
    match e.kind() {
        BrokenPipe | ConnectionReset | ConnectionAborted | UnexpectedEof | NotConnected => "the connection was lost during the transfer".to_string(),
        StorageFull => "the disk is full".to_string(),
        _ => e.to_string(),
    }
}

fn cleanup_upload_tree(shell: Shell, stage: &str) {
    if let Ok(mut child) = spawn(shell(&format!("rm -rf -- {}", sh_quote(stage))), Stdio::null(), Stdio::null()) {
        let _ = child.wait();
    }
}

/// How long a child whose input was closed gets to finish and clean up before it is killed.
const STOP_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// Wait for `child` to exit on its own (its stdin is closed), killing it only when it does not within [`STOP_GRACE`].
fn stop_gracefully(child: &mut Child) {
    let start = std::time::Instant::now();
    while start.elapsed() < STOP_GRACE {
        if !matches!(child.try_wait(), Ok(None)) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    kill(child);
}

fn kill(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Copy `from` to `to`; calls `step(bytes)` per chunk; `Ok(false)` if `cancel` is set.
fn pump(from: &mut dyn Read, to: &mut dyn Write, cancel: &AtomicBool, step: &mut dyn FnMut(u64)) -> std::io::Result<bool> {
    let mut buf = vec![0u8; CHUNK];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let n = from.read(&mut buf)?;
        if n == 0 {
            return Ok(true);
        }
        to.write_all(&buf[..n])?;
        step(n as u64);
    }
}

fn file_name(path: &Path) -> Result<String> {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).filter(|n| !n.is_empty()).ok_or_else(|| fail(format!("{} has no file name", path.display())))
}

/// Upload a local file/folder into `remote_dir` on the server (same name), overwriting.
/// Returns the new path on the server.
pub fn upload(shell: Shell, local: &Path, remote_dir: &str, cancel: &AtomicBool, mut step: impl FnMut(u64)) -> Result<String> {
    check_cancelled(cancel)?;
    let name = file_name(local)?;
    let dir = remote_dir.trim_end_matches('/');
    let dest = format!("{}/{name}", if dir.is_empty() { "" } else { dir });
    let is_dir = std::fs::metadata(local).map_err(|e| fail(format!("{}: {e}", local.display())))?.is_dir();
    if is_dir {
        // Extract privately first. Never use a preflight existence check as
        // authority to recursively delete the requested destination on failure.
        let directory = if dir.is_empty() { "/" } else { dir };
        let template = format!("{}/.codeyard-upload-XXXXXXXX", directory.trim_end_matches('/'));
        // tar | (remote) tar -x keeps the tree, permissions and empty folders. Only flags common to bsdtar, GNU tar and
        // busybox are used: `-H` means "follow top-level symlinks" on bsdtar but takes a format argument on GNU tar,
        // so the top-level link is resolved here instead and the folder's contents (`.`) are archived. The remote
        // `-o` (extract) means "don't apply ownership" everywhere (a root server shouldn't get the uploader's uid);
        // it is only ever used with `-x` because with `-c` GNU tar reads it as `--old-archive`.
        let tree = std::fs::canonicalize(local).map_err(|e| fail(format!("{}: {e}", local.display())))?;
        let script = format!(
            r#"set -e; if [ -e {dest} ] && [ ! -d {dest} ]; then echo {msg} >&2; exit 1; fi
mkdir -p {directory}; t=$(mktemp -d {template}) || exit 1
keep=; trap '[ "$keep" = yes ] || rm -rf -- "$t"' EXIT
trap 'exit 1' HUP INT TERM
mkdir "$t/tree"; printf '%s\0' "$t"; tar -C "$t/tree" -xof -; keep=yes"#,
            directory = sh_quote(directory),
            template = sh_quote(&template),
            dest = sh_quote(&dest),
            msg = sh_quote(&format!("a file named “{name}” is in the way on the server")),
        );
        let mut tar = spawn(
            {
                let mut c = Command::new("tar");
                c.arg("-C").arg(&tree).arg("-cf").arg("-").arg(".");
                c
            },
            Stdio::null(),
            Stdio::piped(),
        )
        .map_err(|_| fail("tar is not available on this computer (needed to upload folders)"))?;
        let mut remote = spawn(shell(&script), Stdio::piped(), Stdio::piped()).inspect_err(|_| {
            kill(&mut tar);
        })?;
        let stage = upload_stage(&mut remote, directory).inspect_err(|_| {
            kill(&mut tar);
        })?;
        let (mut from, mut to) = (piped(tar.stdout.take())?, piped(remote.stdin.take())?);
        let done = pump(&mut from, &mut to, cancel, &mut step);
        drop(to);
        let undo = |remote_started: bool| {
            if remote_started {
                cleanup_upload_tree(shell, &stage);
            }
        };
        match done {
            Ok(true) => {
                // Wait for both: an unreadable source file fails `tar`, but the remote side has already written the tree so far;
                // returning early with `?` would leave that partial folder on the server.
                let (read, wrote) = (finish(tar, "tar"), finish(remote, "upload"));
                if let Err(e) = read.and(wrote) {
                    undo(true);
                    return Err(e);
                }
                if cancel.load(Ordering::Relaxed) {
                    undo(true);
                    return Err(HostError::Op(proto::Error::Cancelled));
                }
                let commit = format!(
                    r#"set -e
if [ -e {dest} ] && [ ! -d {dest} ]; then echo 'upload destination is not a folder' >&2; exit 1; fi
mkdir -p {dest}
receiver=; watcher=
exec 3<&0
cleanup() {{ [ -z "$receiver" ] || kill "$receiver" 2>/dev/null || :; [ -z "$watcher" ] || kill "$watcher" 2>/dev/null || :; }}
trap cleanup EXIT; trap 'exit 1' HUP INT TERM
( if tar -C {tree} -cf - .; then printf 0 > {status}; else printf 1 > {status}; fi ) | tar -C {dest} -xof - &
receiver=$!
( IFS= read -r ignored <&3 || kill "$receiver" 2>/dev/null || : ) >/dev/null 2>&1 &
watcher=$!
if wait "$receiver"; then extracted=0; else extracted=1; fi
receiver=
kill "$watcher" 2>/dev/null || :; wait "$watcher" 2>/dev/null || :; watcher=
[ "$extracted" = 0 ] || exit 1
count=0; while [ ! -f {status} ] && [ "$count" -lt 10 ]; do sleep 1; count=$((count + 1)); done
[ "$(cat {status})" = 0 ]"#,
                    dest = sh_quote(&dest),
                    tree = sh_quote(&format!("{stage}/tree")),
                    status = sh_quote(&format!("{stage}/sender-status")),
                );
                let result = spawn(shell(&commit), Stdio::piped(), Stdio::null()).and_then(|child| finish_folder_commit(child, cancel));
                undo(true);
                result.map_err(|error| HostError::Uncertain(format!("folder finalization failed; the destination may be partially updated: {error}")))?;
                Ok(dest)
            }
            Ok(false) => {
                kill(&mut tar);
                // Its input is closed: the remote shell stops and removes its staging folder itself. Killing it at once would leave
                // the `tar -x` it started running, still writing into the folder that is deleted next.
                stop_gracefully(&mut remote);
                undo(true);
                Err(HostError::Op(proto::Error::Cancelled))
            }
            Err(e) => {
                kill(&mut tar);
                // The real cause is in the remote stderr (else the I/O error).
                let why = finish(remote, "upload").err().map(|e| e.to_string()).unwrap_or_else(|| io_reason(&e));
                undo(true);
                Err(fail(why))
            }
        }
    } else {
        // Closing stdin after cancellation must not publish a truncated file.
        let directory = if dir.is_empty() { "/" } else { dir };
        let template = format!("{}/.codeyard-upload-XXXXXXXX", directory.trim_end_matches('/'));
        let script = format!(
            r#"set -e; if [ -d {dest} ]; then echo {msg} >&2; exit 1; fi
mkdir -p {d}; t=$(mktemp {template}) || exit 1
keep=; trap '[ "$keep" = yes ] || rm -f -- "$t"' EXIT
trap 'exit 1' HUP INT TERM
printf '%s\0' "$t"; cat > "$t"; keep=yes"#,
            d = sh_quote(directory),
            template = sh_quote(&template),
            dest = sh_quote(&dest),
            msg = sh_quote(&format!("a folder named “{name}” is in the way on the server")),
        );
        let mut file = std::fs::File::open(local).map_err(|e| fail(format!("{}: {e}", local.display())))?;
        let mut remote = spawn(shell(&script), Stdio::piped(), Stdio::piped())?;
        let part = upload_stage(&mut remote, directory)?;
        let mut to = piped(remote.stdin.take())?;
        let done = pump(&mut file, &mut to, cancel, &mut step);
        drop(to);
        match done {
            Ok(true) => {
                if let Err(error) = finish(remote, "upload") {
                    cleanup_part(shell, &part);
                    return Err(error);
                }
                if cancel.load(Ordering::Relaxed) {
                    cleanup_part(shell, &part);
                    return Err(HostError::Op(proto::Error::Cancelled));
                }
                let commit = format!(
                    "set -e; if [ -d {dest} ]; then echo 'upload destination is a folder' >&2; exit 1; fi; mv -f -- {part} {dest}",
                    dest = sh_quote(&dest),
                    part = sh_quote(&part)
                );
                let result = spawn(shell(&commit), Stdio::null(), Stdio::null()).and_then(|child| finish(child, "upload"));
                if result.is_err() {
                    cleanup_part(shell, &part);
                }
                result.map(|()| dest)
            }
            Ok(false) => {
                kill(&mut remote);
                cleanup_part(shell, &part);
                Err(HostError::Op(proto::Error::Cancelled))
            }
            Err(e) => {
                let why = finish(remote, "upload").err().map(|e| e.to_string()).unwrap_or_else(|| io_reason(&e));
                cleanup_part(shell, &part);
                Err(fail(why))
            }
        }
    }
}

/// Moves `stage/name` to `dest` after checking that the extracted stream held that one folder and nothing else.
/// An existing folder is merged into (files of the same name are replaced, other files stay).
fn commit_staged(stage: &Path, name: &str, dest: &Path) -> Result<()> {
    let mut entries = std::fs::read_dir(stage).map_err(|e| fail(format!("{}: {e}", stage.display())))?;
    let only = entries.next().and_then(|e| e.ok()).filter(|e| e.file_name() == name);
    if only.is_none() || entries.next().is_some() {
        return Err(fail("the server sent files outside the folder you asked for; nothing was written"));
    }
    let staged = stage.join(name);
    if !std::fs::symlink_metadata(&staged).is_ok_and(|m| m.is_dir()) {
        return Err(fail("the server sent something other than the folder you asked for; nothing was written"));
    }
    merge_dir(&staged, dest).map_err(|e| fail(format!("{}: {e}", dest.display())))
}

fn merge_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    if !std::fs::symlink_metadata(to).is_ok_and(|m| m.is_dir()) {
        return std::fs::rename(from, to);
    }
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        let src_is_dir = std::fs::symlink_metadata(&src)?.is_dir();
        match std::fs::symlink_metadata(&dst) {
            Ok(m) if m.is_dir() && src_is_dir => merge_dir(&src, &dst)?,
            Ok(m) => {
                if m.is_dir() {
                    std::fs::remove_dir_all(&dst)?;
                } else {
                    std::fs::remove_file(&dst)?;
                }
                std::fs::rename(&src, &dst)?;
            }
            Err(_) => std::fs::rename(&src, &dst)?,
        }
    }
    Ok(())
}

/// Download a server file/folder into local `local_dir` (same name); returns the local path.
/// Written as `.part` first and renamed when done, so a partial file never has the real name.
pub fn download(shell: Shell, remote: &str, is_dir: bool, local_dir: &Path, cancel: &AtomicBool, mut step: impl FnMut(u64)) -> Result<PathBuf> {
    check_cancelled(cancel)?;
    let remote = remote.trim_end_matches('/');
    let (parent, name) = remote.rsplit_once('/').ok_or_else(|| fail(format!("{remote} is not an absolute path")))?;
    if name.is_empty() {
        return Err(fail("cannot download the root folder"));
    }
    std::fs::create_dir_all(local_dir).map_err(|e| fail(format!("{}: {e}", local_dir.display())))?;
    let dest = local_dir.join(name);
    // Name the clash explicitly when kinds differ (a file can't replace a folder or the reverse).
    match (is_dir, std::fs::symlink_metadata(&dest)) {
        (true, Ok(m)) if !m.is_dir() => return Err(fail(format!("a file named “{name}” is in the way in {}", local_dir.display()))),
        (false, Ok(m)) if m.is_dir() => return Err(fail(format!("a folder named “{name}” is in the way in {}", local_dir.display()))),
        _ => {}
    }
    if is_dir {
        let script = format!("tar -C {} -cf - {}", sh_quote(if parent.is_empty() { "/" } else { parent }), sh_quote(&format!("./{name}")));
        // The server decides what is in the stream: extract into a scratch folder first and only move `name` out of it,
        // so an entry like `./.zshrc` next to it can never land in the folder the user picked.
        let scratch = Scratch::new(local_dir)?;
        let stage = &scratch.0;
        let mut remote_p = spawn(shell(&script), Stdio::null(), Stdio::piped())?;
        let mut tar = spawn(
            {
                let mut c = Command::new("tar");
                c.arg("-C").arg(stage).arg("-xf").arg("-").arg("--no-same-owner");
                c
            },
            Stdio::piped(),
            Stdio::null(),
        )
        .map_err(|_| {
            kill(&mut remote_p);
            fail("tar is not available on this computer (needed to download folders)")
        })?;
        let (mut from, mut to) = (piped(remote_p.stdout.take())?, piped(tar.stdin.take())?);
        let done = pump(&mut from, &mut to, cancel, &mut step);
        drop(to);
        match done {
            Ok(true) => {
                let received = finish(remote_p, "download");
                let extracted = finish(tar, "tar");
                let result = received.and(extracted).and_then(|()| commit_staged(stage, name, &dest));
                result.map(|()| dest)
            }
            Ok(false) => {
                kill(&mut remote_p);
                kill(&mut tar);
                Err(HostError::Op(proto::Error::Cancelled))
            }
            Err(e) => {
                kill(&mut tar);
                let why = finish(remote_p, "download").err().map(|e| e.to_string()).unwrap_or_else(|| io_reason(&e));
                Err(fail(why))
            }
        }
    } else {
        let scratch = Scratch::new(local_dir)?;
        let part = scratch.0.join("partial");
        let mut out = std::fs::OpenOptions::new().write(true).create_new(true).open(&part).map_err(|e| fail(format!("{}: {e}", part.display())))?;
        let mut child = spawn(shell(&format!("cat -- {}", sh_quote(remote))), Stdio::null(), Stdio::piped())?;
        let mut from = piped(child.stdout.take())?;
        let done = pump(&mut from, &mut out, cancel, &mut step);
        drop(out);
        match done {
            Ok(true) => match finish(child, "download") {
                Ok(()) => {
                    std::fs::rename(&part, &dest).map_err(|e| fail(format!("{}: {e}", dest.display())))?;
                    Ok(dest)
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&part);
                    Err(e)
                }
            },
            Ok(false) => {
                kill(&mut child);
                let _ = std::fs::remove_file(&part);
                Err(HostError::Op(proto::Error::Cancelled))
            }
            Err(e) => {
                kill(&mut child);
                let _ = std::fs::remove_file(&part);
                Err(fail(io_reason(&e)))
            }
        }
    }
}

fn resolve_copy_destination(path: &Path) -> std::io::Result<PathBuf> {
    let absolute = std::env::current_dir()?.join(path);
    let mut ancestor = absolute.as_path();
    let mut resolved = loop {
        match std::fs::canonicalize(ancestor) {
            Ok(path) => break path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ancestor = ancestor.parent().ok_or(error)?;
            }
            Err(error) => return Err(error),
        }
    };
    for component in absolute.strip_prefix(ancestor).map_err(std::io::Error::other)?.components() {
        if component == std::path::Component::ParentDir {
            resolved.pop();
        } else {
            resolved.push(component);
        }
        match std::fs::canonicalize(&resolved) {
            Ok(path) => resolved = path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(resolved)
}

/// Local copy (file/folder dragged from Finder into the workspace): streaming, with progress and cancel.
/// Symlinks are skipped so we don't leak into their targets. Returns the new path.
pub fn copy_local(src: &Path, dest_dir: &Path, cancel: &AtomicBool, mut step: impl FnMut(u64)) -> Result<PathBuf> {
    check_cancelled(cancel)?;
    let name = file_name(src)?;
    let dest = dest_dir.join(&name);
    let meta = std::fs::symlink_metadata(src).map_err(|e| fail(format!("{}: {e}", src.display())))?;
    if meta.is_dir() {
        let source = std::fs::canonicalize(src).map_err(|e| fail(format!("{}: {e}", src.display())))?;
        // Resolve aliases before creating anything, including when the chosen destination
        // has missing parents below a symlink or a Windows directory junction.
        let destination = resolve_copy_destination(dest_dir).map_err(|e| fail(format!("{}: {e}", dest_dir.display())))?;
        if destination.starts_with(source) {
            return Err(fail(format!("Cannot copy “{name}” into itself")));
        }
    }
    std::fs::create_dir_all(dest_dir).map_err(|e| fail(format!("{}: {e}", dest_dir.display())))?;
    match (meta.is_dir(), std::fs::symlink_metadata(&dest)) {
        (true, Ok(m)) if !m.is_dir() => return Err(fail(format!("a file named “{name}” is in the way in {}", dest_dir.display()))),
        (false, Ok(m)) if m.is_dir() => return Err(fail(format!("a folder named “{name}” is in the way in {}", dest_dir.display()))),
        _ => {}
    }
    let existed = dest.exists();
    if let Err(e) = copy_node(src, &dest, cancel, &mut step) {
        // No partial new tree left behind; merging into an existing one touches nothing.
        if !existed && meta.is_dir() {
            let _ = std::fs::remove_dir_all(&dest);
        }
        return Err(e);
    }
    Ok(dest)
}

fn copy_node(src: &Path, dest: &Path, cancel: &AtomicBool, step: &mut dyn FnMut(u64)) -> Result<()> {
    check_cancelled(cancel)?;
    let meta = std::fs::symlink_metadata(src).map_err(|e| fail(format!("{}: {e}", src.display())))?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    if meta.is_dir() {
        std::fs::create_dir_all(dest).map_err(|e| fail(format!("{}: {e}", dest.display())))?;
        for entry in std::fs::read_dir(src).map_err(|e| fail(format!("{}: {e}", src.display())))?.flatten() {
            copy_node(&entry.path(), &dest.join(entry.file_name()), cancel, step)?;
        }
        return Ok(());
    }
    let scratch = Scratch::new(dest.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new(".")))?;
    let part = scratch.0.join("partial");
    let result = (|| -> Result<()> {
        let mut from = std::fs::File::open(src).map_err(|e| fail(format!("{}: {e}", src.display())))?;
        let mut to = std::fs::OpenOptions::new().write(true).create_new(true).open(&part).map_err(|e| fail(format!("{}: {e}", part.display())))?;
        if !pump(&mut from, &mut to, cancel, step).map_err(|e| fail(e.to_string()))? {
            return Err(HostError::Op(proto::Error::Cancelled));
        }
        drop(to);
        let _ = std::fs::set_permissions(&part, meta.permissions());
        std::fs::rename(&part, dest).map_err(|e| fail(format!("{}: {e}", dest.display())))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

#[cfg(test)]
#[path = "transfer_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "transfer_cancel_tests.rs"]
mod cancel_tests;
