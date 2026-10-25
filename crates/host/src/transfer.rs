//! File transfer: upload/download of files and folders between the computer and a server (or container/WSL).
//!
//! Streams (nothing held in memory, no 50 MB cap): a file goes through `cat`, a folder through `tar`,
//! piped into the target's `sh -c`; ssh reuses the open ControlMaster connection (no new authentication).
//! Progress is in bytes; cancel kills the process and removes the partial file.
//!
//! Security: server `tar` output is extracted with local `tar -x`; GNU tar and bsdtar both reject/strip
//! leading `/` and `..` paths by default, and `--no-same-owner` skips applying ownership.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::ssh::{sh_quote, target_command};
use crate::{HostError, Result};

const CHUNK: usize = 256 * 1024;

fn fail(msg: impl Into<String>) -> HostError {
    HostError::Disconnected(msg.into())
}

/// Builds the command that runs `sh -c <script>` on the target (ssh/docker/wsl in production, local `sh` in tests).
pub type Shell<'a> = &'a dyn Fn(&str) -> Command;

pub fn shell_for(alias: &str) -> impl Fn(&str) -> Command + use<> {
    let alias = alias.to_string();
    move |script| {
        let mut c = target_command(&alias);
        c.arg(script);
        c
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

/// After cancel/error: delete the partial temp upload on the server (SIGKILL skips the `trap`; best effort).
fn cleanup_part(shell: Shell, dest: &str) {
    let script = format!("rm -f {}.codeyard-part.*", sh_quote(dest));
    if let Ok(mut c) = spawn(shell(&script), Stdio::null(), Stdio::null()) {
        let _ = c.wait();
    }
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

/// Whether a path exists on the server (`test -e`); errors or no connection count as "missing" (only used for cleanup decisions).
fn remote_exists(shell: Shell, path: &str) -> bool {
    spawn(shell(&format!("test -e {}", sh_quote(path))), Stdio::null(), Stdio::null())
        .and_then(|mut c| c.wait().map_err(|e| fail(e.to_string())))
        .is_ok_and(|s| s.success())
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
    let name = file_name(local)?;
    let dir = remote_dir.trim_end_matches('/');
    let dest = format!("{}/{name}", if dir.is_empty() { "" } else { dir });
    let is_dir = std::fs::metadata(local).map_err(|e| fail(format!("{}: {e}", local.display())))?.is_dir();
    if is_dir {
        // A folder we create is removed on cancel/error; one we merge into stays as is.
        let existed = remote_exists(shell, &dest);
        // tar | (remote) tar -x keeps the tree, permissions and empty folders. Only flags common to bsdtar, GNU tar and
        // busybox are used: `-H` means "follow top-level symlinks" on bsdtar but takes a format argument on GNU tar,
        // so the top-level link is resolved here instead and the folder's contents (`.`) are archived. The remote
        // `-o` (extract) means "don't apply ownership" everywhere (a root server shouldn't get the uploader's uid);
        // it is only ever used with `-x` because with `-c` GNU tar reads it as `--old-archive`.
        let tree = std::fs::canonicalize(local).map_err(|e| fail(format!("{}: {e}", local.display())))?;
        let script = format!(
            "set -e; if [ -e {dest} ] && [ ! -d {dest} ]; then echo {msg} >&2; exit 1; fi; mkdir -p {dest}; tar -C {dest} -xof -",
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
        let mut remote = spawn(shell(&script), Stdio::piped(), Stdio::null())?;
        let (mut from, mut to) = (piped(tar.stdout.take())?, piped(remote.stdin.take())?);
        let done = pump(&mut from, &mut to, cancel, &mut step);
        drop(to);
        let undo = |remote_started: bool| {
            if remote_started && !existed {
                let _ = spawn(shell(&format!("rm -rf -- {}", sh_quote(&dest))), Stdio::null(), Stdio::null())
                    .and_then(|mut c| c.wait().map_err(|e| fail(e.to_string())));
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
                Ok(dest)
            }
            Ok(false) => {
                kill(&mut tar);
                kill(&mut remote);
                undo(true);
                Err(fail("cancelled"))
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
        // Write to a temp file and move into place when done; an interrupted upload (cancel, drop) deletes it.
        let script = format!(
            "set -e; if [ -d {dest} ]; then echo {msg} >&2; exit 1; fi; mkdir -p {d}; t={dest}.codeyard-part.$$; trap 'rm -f \"$t\"' EXIT HUP INT TERM; cat > \"$t\"; mv -f \"$t\" {dest}; trap - EXIT",
            d = sh_quote(if dir.is_empty() { "/" } else { dir }),
            dest = sh_quote(&dest),
            msg = sh_quote(&format!("a folder named “{name}” is in the way on the server")),
        );
        let mut file = std::fs::File::open(local).map_err(|e| fail(format!("{}: {e}", local.display())))?;
        let mut remote = spawn(shell(&script), Stdio::piped(), Stdio::null())?;
        let mut to = piped(remote.stdin.take())?;
        let done = pump(&mut file, &mut to, cancel, &mut step);
        drop(to);
        match done {
            Ok(true) => finish(remote, "upload").map(|()| dest),
            Ok(false) => {
                kill(&mut remote);
                cleanup_part(shell, &dest);
                Err(fail("cancelled"))
            }
            Err(e) => {
                let why = finish(remote, "upload").err().map(|e| e.to_string()).unwrap_or_else(|| io_reason(&e));
                cleanup_part(shell, &dest);
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
        let stage = local_dir.join(format!(".{name}.codeyard-dl"));
        let _ = std::fs::remove_dir_all(&stage);
        std::fs::create_dir(&stage).map_err(|e| fail(format!("{}: {e}", stage.display())))?;
        let mut remote_p = spawn(shell(&script), Stdio::null(), Stdio::piped())?;
        let mut tar = spawn(
            {
                let mut c = Command::new("tar");
                c.arg("-C").arg(&stage).arg("-xf").arg("-").arg("--no-same-owner");
                c
            },
            Stdio::piped(),
            Stdio::null(),
        )
        .map_err(|_| {
            let _ = std::fs::remove_dir_all(&stage);
            fail("tar is not available on this computer (needed to download folders)")
        })?;
        let (mut from, mut to) = (piped(remote_p.stdout.take())?, piped(tar.stdin.take())?);
        let done = pump(&mut from, &mut to, cancel, &mut step);
        drop(to);
        match done {
            Ok(true) => {
                let result = finish(remote_p, "download").and_then(|()| finish(tar, "tar")).and_then(|()| commit_staged(&stage, name, &dest));
                let _ = std::fs::remove_dir_all(&stage);
                result.map(|()| dest)
            }
            Ok(false) => {
                kill(&mut remote_p);
                kill(&mut tar);
                let _ = std::fs::remove_dir_all(&stage);
                Err(fail("cancelled"))
            }
            Err(e) => {
                kill(&mut tar);
                let why = finish(remote_p, "download").err().map(|e| e.to_string()).unwrap_or_else(|| io_reason(&e));
                let _ = std::fs::remove_dir_all(&stage);
                Err(fail(why))
            }
        }
    } else {
        let part = local_dir.join(format!(".{name}.codeyard-part"));
        let mut out = std::fs::File::create(&part).map_err(|e| fail(format!("{}: {e}", part.display())))?;
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
                Err(fail("cancelled"))
            }
            Err(e) => {
                kill(&mut child);
                let _ = std::fs::remove_file(&part);
                Err(fail(io_reason(&e)))
            }
        }
    }
}

/// Local copy (file/folder dragged from Finder into the workspace): streaming, with progress and cancel.
/// Symlinks are skipped so we don't leak into their targets. Returns the new path.
pub fn copy_local(src: &Path, dest_dir: &Path, cancel: &AtomicBool, mut step: impl FnMut(u64)) -> Result<PathBuf> {
    let name = file_name(src)?;
    let dest = dest_dir.join(&name);
    let meta = std::fs::symlink_metadata(src).map_err(|e| fail(format!("{}: {e}", src.display())))?;
    if meta.is_dir() && dest_dir.starts_with(src) {
        return Err(fail(format!("Cannot copy “{name}” into itself")));
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
    let part = dest.with_file_name(format!(".{}.codeyard-part", dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
    let result = (|| -> Result<()> {
        let mut from = std::fs::File::open(src).map_err(|e| fail(format!("{}: {e}", src.display())))?;
        let mut to = std::fs::File::create(&part).map_err(|e| fail(format!("{}: {e}", part.display())))?;
        if !pump(&mut from, &mut to, cancel, step).map_err(|e| fail(e.to_string()))? {
            return Err(fail("cancelled"));
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
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("codeyard-transfer-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    #[cfg_attr(windows, ignore = "uses a local POSIX shell to emulate the remote server")]
    fn file_round_trip_with_spaces_and_progress() {
        let d = dir("file");
        let src = d.join("a b 'quoted'.bin");
        let data: Vec<u8> = (0..1_500_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&src, &data).unwrap();
        let remote_dir = d.join("srv/new dir");
        let cancel = AtomicBool::new(false);
        let mut seen = 0u64;
        let dest = upload(&local_shell, &src, remote_dir.to_str().unwrap(), &cancel, |n| seen += n).unwrap();
        assert_eq!(seen, data.len() as u64);
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        assert!(std::fs::read_dir(&remote_dir).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().contains("codeyard-part")), "no partial files left");

        let back = d.join("downloads");
        let mut got = 0u64;
        let local = download(&local_shell, &dest, false, &back, &cancel, |n| got += n).unwrap();
        assert_eq!(local, back.join("a b 'quoted'.bin"));
        assert_eq!(std::fs::read(&local).unwrap(), data);
        assert_eq!(got, data.len() as u64);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    #[cfg_attr(windows, ignore = "uses a local POSIX shell to emulate the remote server")]
    fn folder_round_trip_keeps_tree_and_empty_dirs() {
        let d = dir("folder");
        let src = d.join("project");
        std::fs::create_dir_all(src.join("src/deep")).unwrap();
        std::fs::create_dir_all(src.join("empty")).unwrap();
        std::fs::write(src.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(src.join("src/deep/ğ ü.txt"), "türkçe").unwrap();
        let remote_dir = d.join("srv");
        let cancel = AtomicBool::new(false);
        let dest = upload(&local_shell, &src, remote_dir.to_str().unwrap(), &cancel, |_| {}).unwrap();
        assert_eq!(std::fs::read_to_string(format!("{dest}/src/deep/ğ ü.txt")).unwrap(), "türkçe");
        assert!(std::path::Path::new(&format!("{dest}/empty")).is_dir());
        let back = d.join("back");
        let local = download(&local_shell, &dest, true, &back, &cancel, |_| {}).unwrap();
        assert_eq!(std::fs::read_to_string(local.join("src/main.rs")).unwrap(), "fn main() {}\n");
        assert!(local.join("empty").is_dir());
        assert!(local_size(&src) > 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    #[cfg_attr(windows, ignore = "uses a local POSIX shell to emulate the remote server")]
    fn cancel_leaves_nothing_behind() {
        let d = dir("cancel");
        let src = d.join("big.bin");
        std::fs::write(&src, vec![7u8; 5_000_000]).unwrap();
        let remote_dir = d.join("srv");
        let cancel = AtomicBool::new(false);
        let err = upload(&local_shell, &src, remote_dir.to_str().unwrap(), &cancel, |_| cancel.store(true, Ordering::Relaxed)).unwrap_err();
        assert!(err.to_string().contains("cancelled"), "{err}");
        let left: Vec<_> = std::fs::read_dir(&remote_dir).map(|r| r.flatten().map(|e| e.file_name()).collect()).unwrap_or_default();
        assert!(left.is_empty(), "no partial or final file after cancel: {left:?}");
        // Download cancel: no `.part` and no final file.
        let remote_file = d.join("r.bin");
        std::fs::write(&remote_file, vec![1u8; 5_000_000]).unwrap();
        let cancel = AtomicBool::new(false);
        let out = d.join("out");
        let err = download(&local_shell, remote_file.to_str().unwrap(), false, &out, &cancel, |_| cancel.store(true, Ordering::Relaxed)).unwrap_err();
        assert!(err.to_string().contains("cancelled"));
        assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn local_copy_keeps_tree_skips_symlinks_and_refuses_self_copy() {
        let d = dir("copy");
        let src = d.join("pack");
        std::fs::create_dir_all(src.join("a/b")).unwrap();
        std::fs::write(src.join("a/b/f.txt"), "içerik").unwrap();
        std::fs::write(src.join("top.txt"), "x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/hosts", src.join("link")).unwrap();
        let cancel = AtomicBool::new(false);
        let mut bytes = 0u64;
        let dest = copy_local(&src, &d.join("dest"), &cancel, |n| bytes += n).unwrap();
        assert_eq!(std::fs::read_to_string(dest.join("a/b/f.txt")).unwrap(), "içerik");
        assert!(!dest.join("link").exists(), "symlinks are not followed or copied");
        assert_eq!(bytes, local_size(&src) - if cfg!(unix) { std::fs::symlink_metadata(src.join("link")).unwrap().len() } else { 0 });
        let err = copy_local(&src, &src.join("a"), &cancel, |_| {}).unwrap_err();
        assert!(err.to_string().contains("into itself"), "{err}");
        // Cancel leaves no `.part` file.
        let big = d.join("big.bin");
        std::fs::write(&big, vec![3u8; 3_000_000]).unwrap();
        let err = copy_local(&big, &d.join("out"), &cancel, |_| cancel.store(true, Ordering::Relaxed)).unwrap_err();
        assert!(err.to_string().contains("cancelled"));
        assert_eq!(std::fs::read_dir(d.join("out")).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// `Shell` whose remote side is an arbitrary script, to script "server" behaviour (early exit…) in tests.
    fn script(body: &'static str) -> impl Fn(&str) -> Command {
        move |_: &str| {
            let mut c = Command::new("sh");
            c.arg("-c").arg(body);
            c
        }
    }

    /// A server may send any tar it likes: whatever sits next to the folder that was asked for must not reach the target folder.
    #[test]
    #[cfg_attr(windows, ignore = "uses a local POSIX shell to emulate the remote server")]
    fn a_hostile_tar_cannot_write_beside_the_downloaded_folder() {
        let d = dir("hostile");
        let evil = d.join("evil");
        std::fs::create_dir_all(evil.join("project")).unwrap();
        std::fs::write(evil.join("project/a.txt"), "ok").unwrap();
        std::fs::write(evil.join(".zshrc"), "touch pwned").unwrap();
        let tarball = d.join("stream.tar");
        let made = Command::new("tar").arg("-C").arg(&evil).arg("-cf").arg(&tarball).arg("./project").arg("./.zshrc").status().unwrap();
        assert!(made.success());
        let cat = tarball.clone();
        let hostile = move |_: &str| {
            let mut c = Command::new("cat");
            c.arg(&cat);
            c
        };
        let out = d.join("out");
        let cancel = AtomicBool::new(false);
        let err = download(&hostile, "/srv/project", true, &out, &cancel, |_| {}).unwrap_err().to_string();
        assert!(err.contains("outside the folder"), "{err}");
        assert!(!out.join(".zshrc").exists());
        assert!(!out.join("project").exists(), "nothing of a refused stream is kept");
        assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0, "the scratch folder is gone too");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// An existing folder is still merged into, as before the stream went through a scratch folder.
    #[test]
    #[cfg_attr(windows, ignore = "uses a local POSIX shell to emulate the remote server")]
    fn downloading_into_an_existing_folder_merges() {
        let d = dir("merge");
        let src = d.join("srv/project");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("new.txt"), "new").unwrap();
        std::fs::write(src.join("sub/both.txt"), "from server").unwrap();
        let out = d.join("out");
        std::fs::create_dir_all(out.join("project/sub")).unwrap();
        std::fs::write(out.join("project/mine.txt"), "mine").unwrap();
        std::fs::write(out.join("project/sub/both.txt"), "old").unwrap();
        let cancel = AtomicBool::new(false);
        let got = download(&local_shell, src.to_str().unwrap(), true, &out, &cancel, |_| {}).unwrap();
        assert_eq!(std::fs::read_to_string(got.join("mine.txt")).unwrap(), "mine");
        assert_eq!(std::fs::read_to_string(got.join("new.txt")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(got.join("sub/both.txt")).unwrap(), "from server");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    #[cfg_attr(windows, ignore = "uses a local POSIX shell to emulate the remote server")]
    fn a_file_and_a_folder_never_replace_each_other_and_nothing_is_damaged() {
        let d = dir("types");
        let cancel = AtomicBool::new(false);
        // File over an existing folder of the same name: refused with a reason, the folder is intact.
        let src = d.join("thing");
        std::fs::write(&src, "file").unwrap();
        let srv = d.join("srv");
        std::fs::create_dir_all(srv.join("thing/inside")).unwrap();
        std::fs::write(srv.join("thing/inside/keep.txt"), "keep").unwrap();
        let err = upload(&local_shell, &src, srv.to_str().unwrap(), &cancel, |_| {}).unwrap_err().to_string();
        assert!(err.contains("folder named") && err.contains("thing"), "{err}");
        assert_eq!(std::fs::read_to_string(srv.join("thing/inside/keep.txt")).unwrap(), "keep");
        assert!(!srv.join("thing/thing").exists(), "mv must not drop the file inside the folder");
        // Folder over an existing file.
        let srcdir = d.join("pack");
        std::fs::create_dir_all(&srcdir).unwrap();
        std::fs::write(srcdir.join("a.txt"), "a").unwrap();
        let srv2 = d.join("srv2");
        std::fs::create_dir_all(&srv2).unwrap();
        std::fs::write(srv2.join("pack"), "i am a file").unwrap();
        let err = upload(&local_shell, &srcdir, srv2.to_str().unwrap(), &cancel, |_| {}).unwrap_err().to_string();
        assert!(err.contains("file named") && err.contains("pack"), "{err}");
        assert_eq!(std::fs::read_to_string(srv2.join("pack")).unwrap(), "i am a file");
        // Downloads: same two cases on the local side.
        std::fs::create_dir_all(d.join("dl/thing")).unwrap();
        let err = download(&local_shell, src.to_str().unwrap(), false, &d.join("dl"), &cancel, |_| {}).unwrap_err().to_string();
        assert!(err.contains("folder named"), "{err}");
        std::fs::write(d.join("dl2-file"), "x").unwrap();
        std::fs::create_dir_all(d.join("dl2")).unwrap();
        std::fs::write(d.join("dl2/pack"), "local file").unwrap();
        let err = download(&local_shell, srcdir.to_str().unwrap(), true, &d.join("dl2"), &cancel, |_| {}).unwrap_err().to_string();
        assert!(err.contains("file named"), "{err}");
        assert_eq!(std::fs::read_to_string(d.join("dl2/pack")).unwrap(), "local file");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    #[cfg_attr(windows, ignore = "uses a local POSIX shell to emulate the remote server")]
    fn cancelling_a_folder_removes_a_new_folder_but_never_an_existing_one() {
        let d = dir("folder-cancel");
        let src = d.join("proj");
        std::fs::create_dir_all(&src).unwrap();
        for i in 0..40 {
            std::fs::write(src.join(format!("f{i}.bin")), vec![i as u8; 200_000]).unwrap();
        }
        // New destination: cancel leaves nothing on the "server" or locally.
        let cancel = AtomicBool::new(false);
        let srv = d.join("srv");
        let err = upload(&local_shell, &src, srv.to_str().unwrap(), &cancel, |_| cancel.store(true, Ordering::Relaxed)).unwrap_err().to_string();
        assert!(err.contains("cancelled"), "{err}");
        assert!(!srv.join("proj").exists(), "a cancelled new folder is removed again");
        let cancel = AtomicBool::new(false);
        let err =
            download(&local_shell, src.to_str().unwrap(), true, &d.join("dl"), &cancel, |_| cancel.store(true, Ordering::Relaxed)).unwrap_err().to_string();
        assert!(err.contains("cancelled"));
        assert!(!d.join("dl/proj").exists());
        // Existing destination (the user agreed to merge): cancelling must keep what was already there.
        std::fs::create_dir_all(srv.join("proj")).unwrap();
        std::fs::write(srv.join("proj/precious.txt"), "do not lose").unwrap();
        let cancel = AtomicBool::new(false);
        let _ = upload(&local_shell, &src, srv.to_str().unwrap(), &cancel, |_| cancel.store(true, Ordering::Relaxed));
        assert_eq!(std::fs::read_to_string(srv.join("proj/precious.txt")).unwrap(), "do not lose");
        std::fs::create_dir_all(d.join("dl2/proj")).unwrap();
        std::fs::write(d.join("dl2/proj/mine.txt"), "my local file").unwrap();
        let cancel = AtomicBool::new(false);
        let _ = download(&local_shell, src.to_str().unwrap(), true, &d.join("dl2"), &cancel, |_| cancel.store(true, Ordering::Relaxed));
        assert_eq!(std::fs::read_to_string(d.join("dl2/proj/mine.txt")).unwrap(), "my local file", "a cancelled merge never deletes the user's folder");
        // Same for the local copy.
        std::fs::create_dir_all(d.join("in/proj")).unwrap();
        std::fs::write(d.join("in/proj/mine.txt"), "mine").unwrap();
        let cancel = AtomicBool::new(false);
        let _ = copy_local(&src, &d.join("in"), &cancel, |_| cancel.store(true, Ordering::Relaxed));
        assert_eq!(std::fs::read_to_string(d.join("in/proj/mine.txt")).unwrap(), "mine");
        let cancel = AtomicBool::new(false);
        let _ = copy_local(&src, &d.join("fresh"), &cancel, |_| cancel.store(true, Ordering::Relaxed));
        assert!(!d.join("fresh/proj").exists(), "a cancelled new local copy is removed");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    #[cfg_attr(windows, ignore = "uses a local POSIX shell to emulate the remote server")]
    fn a_dropped_connection_is_reported_as_such() {
        let d = dir("lost");
        let src = d.join("big.bin");
        std::fs::write(&src, vec![1u8; 6_000_000]).unwrap();
        let cancel = AtomicBool::new(false);
        // The "server" reads a little and hangs up cleanly: the write side sees a broken pipe.
        let err = upload(&script("head -c 100 > /dev/null"), &src, "/srv", &cancel, |_| {}).unwrap_err().to_string();
        assert!(err.contains("connection was lost"), "{err}");
        // The server dies with an error message: that message wins over the pipe error.
        let err = upload(&script("head -c 100 > /dev/null; echo 'Disk quota exceeded' >&2; exit 1"), &src, "/srv", &cancel, |_| {}).unwrap_err().to_string();
        assert!(err.contains("Disk quota exceeded"), "{err}");
        // Download: the server sends less than nothing and fails.
        let err = download(&script("echo 'cat: /x: Permission denied' >&2; exit 1"), "/x", false, &d.join("o"), &cancel, |_| {}).unwrap_err().to_string();
        assert!(err.contains("Permission denied"), "{err}");
        assert_eq!(std::fs::read_dir(d.join("o")).unwrap().count(), 0, "no partial file after a failed download");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    #[cfg(unix)]
    fn a_folder_named_like_an_option_keeps_its_modes_both_ways() {
        use std::os::unix::fs::PermissionsExt;
        let d = dir("dash");
        let cancel = AtomicBool::new(false);
        let tree = d.join("-xvf");
        std::fs::create_dir_all(tree.join("sub")).unwrap();
        std::fs::write(tree.join("sub/run.sh"), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(tree.join("sub/run.sh"), std::fs::Permissions::from_mode(0o750)).unwrap();
        std::fs::set_permissions(tree.join("sub"), std::fs::Permissions::from_mode(0o700)).unwrap();
        let mode = |p: PathBuf| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let dest = upload(&local_shell, &tree, d.join("srv").to_str().unwrap(), &cancel, |_| {}).unwrap();
        assert_eq!((mode(Path::new(&dest).join("sub/run.sh")), mode(Path::new(&dest).join("sub"))), (0o750, 0o700));
        let back = download(&local_shell, &dest, true, &d.join("back"), &cancel, |_| {}).unwrap();
        assert_eq!(back.file_name().unwrap(), "-xvf");
        assert_eq!(mode(back.join("sub/run.sh")), 0o750);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    #[cfg_attr(windows, ignore = "uses a local POSIX shell to emulate the remote server")]
    fn unusual_names_empty_files_and_folders_and_symlinks_survive() {
        let d = dir("names");
        let cancel = AtomicBool::new(false);
        let odd = d.join("it's a \"test\" $HOME `x` ğüş\nline2.txt");
        std::fs::write(&odd, "").unwrap(); // empty file
        let srv = d.join("srv dir");
        let dest = upload(&local_shell, &odd, srv.to_str().unwrap(), &cancel, |_| {}).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"");
        assert_eq!(std::path::Path::new(&dest).file_name().unwrap(), odd.file_name().unwrap());
        // Folder: empty subfolder, a file, a symlink to a file, a broken symlink (kept as a link, not an error).
        let tree = d.join("tree");
        std::fs::create_dir_all(tree.join("empty")).unwrap();
        std::fs::write(tree.join("real.txt"), "data").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("real.txt", tree.join("link.txt")).unwrap();
            std::os::unix::fs::symlink("/definitely/missing", tree.join("broken")).unwrap();
        }
        let dest = upload(&local_shell, &tree, srv.to_str().unwrap(), &cancel, |_| {}).unwrap();
        assert!(Path::new(&dest).join("empty").is_dir());
        assert_eq!(std::fs::read_to_string(Path::new(&dest).join("real.txt")).unwrap(), "data");
        #[cfg(unix)]
        {
            assert_eq!(std::fs::read_link(Path::new(&dest).join("link.txt")).unwrap(), Path::new("real.txt"));
            assert!(std::fs::symlink_metadata(Path::new(&dest).join("broken")).is_ok(), "the dangling link is kept");
        }
        // A symlink given as the folder itself uploads what it points to.
        #[cfg(unix)]
        {
            let link = d.join("treelink");
            std::os::unix::fs::symlink(&tree, &link).unwrap();
            let dest = upload(&local_shell, &link, srv.to_str().unwrap(), &cancel, |_| {}).unwrap();
            assert!(Path::new(&dest).is_dir() && !std::fs::symlink_metadata(&dest).unwrap().file_type().is_symlink(), "the content, not the link");
            assert_eq!(std::fs::read_to_string(Path::new(&dest).join("real.txt")).unwrap(), "data");
        }
        // Download an empty remote folder and the odd file name back.
        let back = download(&local_shell, &srv.join(odd.file_name().unwrap()).to_string_lossy(), false, &d.join("back"), &cancel, |_| {}).unwrap();
        assert_eq!(back.file_name().unwrap(), odd.file_name().unwrap());
        std::fs::create_dir_all(srv.join("void")).unwrap();
        let void = download(&local_shell, &srv.join("void").to_string_lossy(), true, &d.join("back"), &cancel, |_| {}).unwrap();
        assert!(void.is_dir());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_source_fails_with_the_reason_and_leaves_no_new_folder() {
        use std::os::unix::fs::PermissionsExt;
        let d = dir("unreadable");
        let tree = d.join("proj");
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::write(tree.join("ok.txt"), "ok").unwrap();
        std::fs::write(tree.join("secret.txt"), "no").unwrap();
        std::fs::set_permissions(tree.join("secret.txt"), std::fs::Permissions::from_mode(0o000)).unwrap();
        let cancel = AtomicBool::new(false);
        let srv = d.join("srv");
        let res = upload(&local_shell, &tree, srv.to_str().unwrap(), &cancel, |_| {});
        std::fs::set_permissions(tree.join("secret.txt"), std::fs::Permissions::from_mode(0o644)).unwrap();
        // Running as root nothing is unreadable and the upload succeeds.
        if let Err(e) = res {
            assert!(e.to_string().to_lowercase().contains("permission denied") || e.to_string().contains("tar"), "{e}");
            assert!(!srv.join("proj").exists(), "the half-copied new folder is removed");
        }
        let file_err = upload(&local_shell, &tree.join("nonexistent.txt"), "/srv", &cancel, |_| {}).unwrap_err().to_string();
        assert!(file_err.contains("nonexistent.txt"), "{file_err}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    #[cfg_attr(windows, ignore = "uses a local POSIX shell to emulate the remote server")]
    fn errors_carry_the_remote_reason() {
        let d = dir("err");
        let src = d.join("x.txt");
        std::fs::write(&src, "x").unwrap();
        let cancel = AtomicBool::new(false);
        // Parent is a file: mkdir fails on the "server".
        let blocker = d.join("blocker");
        std::fs::write(&blocker, "f").unwrap();
        let err = upload(&local_shell, &src, blocker.join("sub").to_str().unwrap(), &cancel, |_| {}).unwrap_err();
        assert!(err.to_string().to_lowercase().contains("not a directory") || err.to_string().contains("mkdir"), "{err}");
        let err = download(&local_shell, d.join("missing.txt").to_str().unwrap(), false, &d.join("o"), &cancel, |_| {}).unwrap_err();
        assert!(err.to_string().to_lowercase().contains("no such file"), "{err}");
        assert!(download(&local_shell, "relative", false, &d, &cancel, |_| {}).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
