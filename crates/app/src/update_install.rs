//! Installing a downloaded, hash-verified update: replace the macOS app bundle in place, or hand the setup program to
//! Windows. Entry point: `stage_and_install`; what the UI does afterwards is `relaunch_command`.
//!
//! macOS in short: mount the disk image read-only -> copy `Codeyard.app` next to the installed app (same volume, so the
//! last step is a rename) -> unmount -> check the copy -> exchange the two atomically (`renamex_np(RENAME_SWAP)`; where the
//! file system cannot, `rename(installed, old)` and `rename(copy, installed)`, rolled back on failure) -> delete the old copy.
//! The installed app is untouched until every check has passed.
//!
//! Gatekeeper: the files are fetched by `curl`, which does not set the quarantine flag, so macOS does not run its first-launch
//! check on an app installed this way (release builds are notarized, but nothing asks Gatekeeper here). What protects the user is the signed
//! checksum list, the bundle identifier and version check and `codesign --verify`.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use crate::update::{Downloaded, RemoveOnDrop, UpdateError, io_err, private_dir, system_tool, verify_file};

const APP_NAME: &str = "Codeyard.app";

/// How this copy of Codeyard can be updated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallKind {
    /// Running from a `.app` bundle whose folder we may write to: replaced in place.
    MacBundle { app: PathBuf },
    /// Installed by the Inno Setup installer: the new setup program upgrades it.
    WindowsInstalled,
    /// Anything else: the UI opens the release page and says why this copy cannot update itself.
    Manual(ManualReason),
}

/// Why a copy of Codeyard is left to the user to update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManualReason {
    /// A debug build must never replace itself or be replaced by a release.
    DevelopmentBuild,
    /// Not run from an app bundle (a bare binary, `cargo run`), or the executable's own path is unknown.
    NotInstalled,
    /// An app bundle in a folder this account cannot write to (an administrator's `/Applications`, a read-only volume).
    ReadOnlyFolder(PathBuf),
    /// Windows, but not installed by the setup program: the portable ZIP.
    Portable,
    /// No installer exists for this system (Linux, Intel Macs).
    OtherSystem,
}

/// What `stage_and_install` did; pass it to `relaunch_command`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallPlan {
    /// The bundle at `app` now holds the new version; the running process still has the old code loaded.
    MacApp { app: PathBuf },
    /// The setup program is running (it closes this app and starts the new one itself).
    WindowsInstaller,
}

pub fn install_kind() -> InstallKind {
    if cfg!(debug_assertions) {
        return InstallKind::Manual(ManualReason::DevelopmentBuild);
    }
    let Ok(exe) = std::env::current_exe().and_then(std::fs::canonicalize) else { return InstallKind::Manual(ManualReason::NotInstalled) };
    kind_for(&exe, cfg!(target_os = "macos"), cfg!(windows), &dir_writable)
}

/// `install_kind` for an executable path; the system and the writability test are parameters so that every outcome can be tested.
fn kind_for(exe: &Path, macos: bool, windows: bool, writable: &dyn Fn(&Path) -> bool) -> InstallKind {
    if macos {
        return match mac_bundle(exe) {
            None => InstallKind::Manual(ManualReason::NotInstalled),
            Some(app) => match app.parent() {
                Some(dir) if writable(dir) => InstallKind::MacBundle { app },
                Some(dir) => InstallKind::Manual(ManualReason::ReadOnlyFolder(dir.to_path_buf())),
                None => InstallKind::Manual(ManualReason::NotInstalled),
            },
        };
    }
    if windows {
        return if exe.with_file_name("unins000.exe").is_file() { InstallKind::WindowsInstalled } else { InstallKind::Manual(ManualReason::Portable) };
    }
    InstallKind::Manual(ManualReason::OtherSystem)
}

/// `<name>.app` for an executable at `<name>.app/Contents/MacOS/<exe>`.
fn mac_bundle(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    (macos.file_name()? == "MacOS" && contents.file_name()? == "Contents" && app.extension()? == "app").then(|| app.to_path_buf())
}

/// Really tries to create a file: read-only volumes and ACLs do not show in the permission bits.
pub(crate) fn dir_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".codeyard-write-test-{}", std::process::id()));
    let created = std::fs::OpenOptions::new().write(true).create_new(true).open(&probe).is_ok();
    if created && let Err(e) = std::fs::remove_file(&probe) {
        eprintln!("could not remove {}: {e}", probe.display());
    }
    created
}

/// Installs the downloaded update. The hash is checked again here, so a file replaced after `download` is never opened.
/// macOS: replaces the bundle (see the module comment); on error the installed app is exactly as before, except for
/// `RollbackFailed`, whose message says where the previous version is. Windows: starts the setup program detached
/// (`installer_args`) and returns at once; the UI must quit now. Blocking (a few seconds on macOS).
pub fn stage_and_install(dl: &Downloaded, kind: &InstallKind) -> Result<InstallPlan, UpdateError> {
    let asset = dl.release.asset.as_ref().ok_or(UpdateError::NoInstaller)?;
    verify_file(&dl.path, &asset.sha256, &asset.name)?;
    match kind {
        InstallKind::MacBundle { app } => install_mac(&dl.path, &dl.release.version, app),
        InstallKind::WindowsInstalled => {
            installer_command(&dl.path).spawn().map_err(|e| UpdateError::Tool { tool: "installer", detail: e.to_string() })?;
            Ok(InstallPlan::WindowsInstaller)
        }
        InstallKind::Manual(_) => Err(UpdateError::ManualOnly),
    }
}

// --- Windows ---

/// Silent, no dialogs, no reboot; `CloseApplications` lets Setup close this app. Setup's own restart of closed
/// applications is off and `/relaunch=1` makes `installer/windows/codeyard.iss` start the new version exactly once.
/// (Written from the Inno Setup documentation; not run on Windows yet.)
fn installer_args() -> [&'static str; 6] {
    ["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CLOSEAPPLICATIONS", "/NORESTARTAPPLICATIONS", "/relaunch=1"]
}

fn installer_command(setup: &Path) -> Command {
    let mut c = Command::new(setup);
    c.args(installer_args()).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0000_0008 | 0x0000_0200); // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    }
    c
}

// --- Relaunch ---

/// Waits (at most a minute) for `$1` to exit, then opens the app at `$2` as a new instance with `$3`, handing the arguments after
/// the third on to the app (`open --args`). Paths, the pid and the app's arguments are positional arguments, never part of the
/// script text.
const RELAUNCH_SCRIPT: &str = r#"i=0; while kill -0 "$1" 2>/dev/null; do i=$((i+1)); [ "$i" -le 300 ] || exit 1; sleep 0.2; done; app=$2; opener=$3; shift 3; exec "$opener" -n "$app" --args "$@""#;

/// The command that starts the new version after this process has exited. Spawn it BEFORE quitting; it is detached
/// (no stdio, own process group) and survives us. The new version gets `--after-update`, which makes it reopen the windows the
/// restart left for it (`update_restart.rs`). `None` on Windows, where Setup starts the app (`codeyard.iss` adds the same flag).
pub fn relaunch_command(plan: &InstallPlan) -> Option<Command> {
    match plan {
        InstallPlan::MacApp { app } => Some(relaunch_with(std::process::id(), app, Path::new("/usr/bin/open"), &[crate::update_restart::FLAG])),
        InstallPlan::WindowsInstaller => None,
    }
}

fn relaunch_with(pid: u32, app: &Path, opener: &Path, app_args: &[&str]) -> Command {
    let mut c = Command::new("/bin/sh");
    c.args(["-c", RELAUNCH_SCRIPT, "codeyard-relaunch"]).arg(pid.to_string()).arg(app).arg(opener).args(app_args);
    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    c
}

// --- macOS ---

/// Runs a helper and returns its stdout; a failure carries its stderr.
fn run(tool: &'static str, cmd: &mut Command) -> Result<String, UpdateError> {
    let out = cmd.stdin(Stdio::null()).output().map_err(|e| UpdateError::Tool { tool, detail: e.to_string() })?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(UpdateError::Tool { tool, detail: String::from_utf8_lossy(&out.stderr).trim().to_string() })
    }
}

/// A key of `<app>/Contents/Info.plist`; `None` when the file or the key is missing.
fn plist_string(app: &Path, key: &str) -> Result<Option<String>, UpdateError> {
    let out = Command::new(system_tool("plutil"))
        .args(["-extract", key, "raw", "-o", "-"])
        .arg(app.join("Contents/Info.plist"))
        .stdin(Stdio::null())
        .output()
        .map_err(|e| UpdateError::Tool { tool: "plutil", detail: e.to_string() })?;
    Ok(out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string()))
}

/// A mounted disk image; unmounted when dropped, on every path.
struct Mount {
    point: PathBuf,
}

impl Mount {
    fn attach(dmg: &Path) -> Result<Self, UpdateError> {
        static N: AtomicU32 = AtomicU32::new(0);
        // A private folder (the user's temp folder is owner-only on macOS) instead of /Volumes, where other programs look.
        let point = std::env::temp_dir().join(format!("codeyard-update-mnt-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        private_dir(&point)?;
        let args = ["attach", "-nobrowse", "-readonly", "-noverify", "-noautoopen", "-mountpoint"];
        if let Err(e) = run("hdiutil", Command::new(system_tool("hdiutil")).args(args).arg(&point).arg(dmg)) {
            if let Err(e) = std::fs::remove_dir(&point) {
                eprintln!("could not remove {}: {e}", point.display());
            }
            return Err(match e {
                UpdateError::Tool { detail, .. } => UpdateError::MountFailed(detail),
                other => other,
            });
        }
        Ok(Mount { point })
    }
}

impl Drop for Mount {
    fn drop(&mut self) {
        let detach = |force: bool| {
            let mut c = Command::new(system_tool("hdiutil"));
            c.arg("detach").arg(&self.point);
            if force {
                c.arg("-force");
            }
            run("hdiutil", &mut c)
        };
        // A process (Spotlight, the copy we just made) can hold the volume for a moment: retry by force.
        if let Err(first) = detach(false)
            && let Err(second) = detach(true)
        {
            eprintln!("could not unmount {}: {first}; {second}", self.point.display());
            return;
        }
        if let Err(e) = std::fs::remove_dir(&self.point) {
            eprintln!("could not remove {}: {e}", self.point.display());
        }
    }
}

fn remove_leftover(path: &Path) -> Result<(), UpdateError> {
    match std::fs::remove_dir_all(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(io_err("remove a leftover of an earlier update")(e)),
        _ => Ok(()),
    }
}

/// The staged copy must be our app, at the version the signed list promised, with an intact code signature.
fn verify_staged(staged: &Path, bundle_id: &str, version: &str) -> Result<(), UpdateError> {
    let mismatch = |what: &str, want: &str, got: Option<String>| {
        UpdateError::BundleMismatch(format!("{what} is {}, expected {want}", got.as_deref().unwrap_or("missing")))
    };
    let id = plist_string(staged, "CFBundleIdentifier")?;
    if id.as_deref() != Some(bundle_id) {
        return Err(mismatch("the app identifier", bundle_id, id));
    }
    let v = plist_string(staged, "CFBundleShortVersionString")?;
    if v.as_deref() != Some(version) {
        return Err(mismatch("the version", version, v));
    }
    run("codesign", Command::new(system_tool("codesign")).args(["--verify", "--deep", "--strict"]).arg(staged)).map(drop).map_err(|e| match e {
        UpdateError::Tool { detail, .. } => UpdateError::CodesignFailed(detail),
        other => other,
    })
}

/// Atomically exchanges two directories (`renamex_np` with `RENAME_SWAP`): at no moment is there no app at the installed path.
/// Fails with `Unsupported` where the system or the file system cannot.
#[cfg(target_os = "macos")]
fn exchange(a: &Path, b: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = |p: &Path| CString::new(p.as_os_str().as_bytes()).map_err(io::Error::other);
    let (a, b) = (c(a)?, c(b)?);
    // SAFETY: both pointers are valid NUL-terminated strings that outlive the call.
    match unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), libc::RENAME_SWAP) } {
        0 => Ok(()),
        _ => Err(io::Error::last_os_error()),
    }
}

#[cfg(not(target_os = "macos"))]
fn exchange(_: &Path, _: &Path) -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}

/// The ordinary ways of "this file system cannot exchange": nothing to report, the two-step replace follows.
fn exchange_unsupported(e: &io::Error) -> bool {
    #[cfg(unix)]
    if matches!(e.raw_os_error(), Some(libc::EINVAL | libc::ENOTSUP)) {
        return true;
    }
    e.kind() == io::ErrorKind::Unsupported
}

/// Puts `staged` in place of `app` and returns where the previous version is now (to be deleted). First choice is one atomic
/// exchange: the old app ends up at `staged`. If that fails (an older file system, or anything else: it changed nothing),
/// `app -> old`, `staged -> app`, and if the second step fails the first is undone.
/// `rename` and `exchange` are parameters so that tests can fail either way.
fn swap(
    app: &Path,
    staged: &Path,
    old: &Path,
    rename: &dyn Fn(&Path, &Path) -> io::Result<()>,
    exchange: &dyn Fn(&Path, &Path) -> io::Result<()>,
) -> Result<PathBuf, UpdateError> {
    match exchange(app, staged) {
        Ok(()) => return Ok(staged.to_path_buf()),
        Err(e) if exchange_unsupported(&e) => {}
        Err(e) => eprintln!("could not exchange the app atomically, replacing it in two steps: {e}"),
    }
    rename(app, old).map_err(io_err("move the installed app aside"))?;
    if let Err(e) = rename(staged, app) {
        return Err(match rename(old, app) {
            Ok(()) => UpdateError::Io { what: "put the new app in place", source: e },
            Err(cause) => UpdateError::RollbackFailed { app: app.to_path_buf(), backup: old.to_path_buf(), cause },
        });
    }
    Ok(old.to_path_buf())
}

pub(crate) fn install_mac(dmg: &Path, version: &str, app: &Path) -> Result<InstallPlan, UpdateError> {
    let parent = app.parent().ok_or_else(|| UpdateError::BundleMismatch("the installed app has no folder".into()))?;
    if !dir_writable(parent) {
        return Err(UpdateError::NoWritePermission(parent.to_path_buf()));
    }
    let bundle_id = plist_string(app, "CFBundleIdentifier")?.ok_or_else(|| UpdateError::BundleMismatch("the installed app has no identifier".into()))?;
    sweep_stale_copies(app);
    let pid = std::process::id();
    let (staged, old) = (parent.join(format!("{STAGED_PREFIX}{pid}.app")), parent.join(format!("{OLD_PREFIX}{pid}.app")));
    remove_leftover(&staged)?;
    remove_leftover(&old)?;
    let staged_guard = RemoveOnDrop::new(staged.clone());
    {
        let mount = Mount::attach(dmg)?;
        let source = mount.point.join(APP_NAME);
        // A symlink here could point anywhere on the disk image's host.
        if !std::fs::symlink_metadata(&source).is_ok_and(|m| m.is_dir()) {
            return Err(UpdateError::BundleMismatch(format!("the disk image has no {APP_NAME}")));
        }
        run("ditto", Command::new(system_tool("ditto")).arg(&source).arg(&staged))?;
    } // unmounted here, before the checks
    verify_staged(&staged, &bundle_id, version)?;
    let previous = swap(app, &staged, &old, &|a, b| std::fs::rename(a, b), &exchange)?;
    staged_guard.keep();
    // The new app is in place; a failure from here on must not turn the update into an error.
    if let Err(e) = std::fs::remove_dir_all(&previous) {
        eprintln!("could not remove the previous version at {}: {e}", previous.display());
    }
    Ok(InstallPlan::MacApp { app: app.to_path_buf() })
}

// --- Leftovers of earlier runs ---

const STAGED_PREFIX: &str = ".Codeyard.update-";
const OLD_PREFIX: &str = ".Codeyard.old-";
const MOUNT_PREFIX: &str = "codeyard-update-mnt-";
/// A download folder this old belongs to no running update (the transfer itself stops after 30 minutes).
const STALE_DOWNLOADS: std::time::Duration = std::time::Duration::from_secs(2 * 60 * 60);

/// Is a process with this id running? Not being allowed to signal it (`EPERM`) still means it exists.
fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let Ok(pid) = libc::pid_t::try_from(pid) else { return true };
        // SAFETY: signal 0 only checks that the process exists.
        unsafe { libc::kill(pid, 0) == 0 || io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// The process id in `<prefix><pid>[-n][.app]`.
fn pid_in_name(name: &str, prefix: &str) -> Option<u32> {
    name.strip_prefix(prefix)?.split(['.', '-']).next()?.parse().ok()
}

/// Staging copies, backups and mount points that an update which crashed or was killed left behind. Whatever belongs to a live
/// process is left alone. The backup is the user's only copy when the installed app is missing (`RollbackFailed`), so it is only
/// deleted while the app is there.
pub(crate) fn sweep_stale_copies(app: &Path) {
    let Some(parent) = app.parent() else { return };
    sweep_dir(parent, app.exists(), &std::env::temp_dir());
}

fn sweep_dir(parent: &Path, app_present: bool, temp: &Path) {
    for (dir, prefix, wanted) in [(parent, STAGED_PREFIX, true), (parent, OLD_PREFIX, app_present), (temp, MOUNT_PREFIX, true)] {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let (name, path) = (entry.file_name(), entry.path());
            let Some(pid) = name.to_str().and_then(|n| pid_in_name(n, prefix)) else { continue };
            if !wanted || pid_alive(pid) {
                continue;
            }
            if prefix == MOUNT_PREFIX {
                remove_mount_point(&path);
            } else if let Err(e) = std::fs::remove_dir_all(&path) {
                eprintln!("could not remove {}: {e}", path.display());
            }
        }
    }
}

/// Detaches the image if it is still mounted there, then removes the (empty) folder. `remove_dir`, never `remove_dir_all`: if
/// the detach did not work, what is inside is somebody's disk image.
fn remove_mount_point(point: &Path) {
    if is_mount_point(point)
        && let Err(e) = run("hdiutil", Command::new(system_tool("hdiutil")).args(["detach", "-force"]).arg(point))
    {
        eprintln!("could not unmount {}: {e}", point.display());
        return;
    }
    if let Err(e) = std::fs::remove_dir(point) {
        eprintln!("could not remove {}: {e}", point.display());
    }
}

/// A mount point lives on another device than the folder that contains it.
fn is_mount_point(dir: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let dev = |p: &Path| std::fs::metadata(p).map(|m| m.dev());
        matches!((dev(dir), dir.parent().map(dev)), (Ok(a), Some(Ok(b))) if a != b)
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        false
    }
}

/// At startup, on a macOS app bundle: the sweep above, plus a download folder that no update is using any more. Cheap, so it
/// runs on a thread of its own and only logs.
pub fn sweep_at_start(kind: &InstallKind, downloads: Option<PathBuf>) {
    let InstallKind::MacBundle { app } = kind else { return };
    let app = app.clone();
    let spawned = std::thread::Builder::new().name("update-sweep".into()).spawn(move || {
        sweep_stale_copies(&app);
        if let Some(dir) = downloads {
            sweep_downloads(&dir, std::time::SystemTime::now());
        }
    });
    if let Err(e) = spawned {
        eprintln!("could not start the clean-up of old update files: {e}");
    }
}

fn sweep_downloads(dir: &Path, now: std::time::SystemTime) {
    let age = std::fs::metadata(dir).and_then(|m| m.modified()).map(|t| now.duration_since(t).unwrap_or_default());
    match age {
        Ok(age) if age >= STALE_DOWNLOADS => {
            if let Err(e) = std::fs::remove_dir_all(dir) {
                eprintln!("could not remove {}: {e}", dir.display());
            }
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => eprintln!("could not look at {}: {e}", dir.display()),
    }
}

#[cfg(test)]
#[path = "update_install_tests.rs"]
mod tests;
