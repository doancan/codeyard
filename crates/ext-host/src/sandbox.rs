//! The OS sandbox (docs/plugin-architecture.md §3): no network, no new processes, read-only access to the extension's
//! package and compiled code, read/write access to its data folder, nothing else. `enter` runs once, after the
//! handshake and before the component is loaded; it cannot be undone.
//!
//! - macOS: Seatbelt, a deny-default profile (sandbox.sb) through `sandbox_init`.
//! - Linux: an unshared network namespace (when user namespaces allow it), landlock (required: without it no code
//!   runs) and a seccomp filter that refuses sockets, exec, new processes and debugging other processes.
//! - Windows: the parent starts the exthost in an AppContainer with a Job Object (`ext_proto::appcontainer`); here
//!   the exthost only checks that it is in one. Off until it passes on physical hardware.
//!
//! The memory cap is not set here: `RLIMIT_AS` would count wasmtime's guard reservations (hundreds of GB of address
//! space), so the app caps the process instead (a Job Object commit limit on Windows; the resident size elsewhere,
//! against `limits::EXTHOST_MEMORY_BYTES`).

use std::path::PathBuf;

/// What the sandboxed process may touch. (On Windows the parent applies it: `ext_proto::appcontainer::Policy`.)
#[derive(Debug, Default)]
#[cfg_attr(windows, allow(dead_code))]
pub struct Policy {
    /// Files and directories (with everything below them) it may read.
    pub read: Vec<PathBuf>,
    /// Directories it may read and write.
    pub write: Vec<PathBuf>,
}

impl Policy {
    /// The policy for one extension. Creates its data folder: rules name existing paths.
    pub fn for_hello(hello: &ext_proto::Hello) -> Result<Policy, String> {
        std::fs::create_dir_all(&hello.data_dir).map_err(|e| format!("cannot create {}: {e}", hello.data_dir))?;
        Ok(Policy { read: vec![hello.package_dir.clone().into(), hello.compiled.clone().into()], write: vec![hello.data_dir.clone().into()] })
    }

    /// Canonical paths (rules match resolved paths: on macOS `/var` is `/private/var`), each with whether it is a
    /// directory.
    #[cfg_attr(windows, allow(dead_code))]
    fn resolved(paths: &[PathBuf]) -> Result<Vec<(PathBuf, bool)>, String> {
        paths
            .iter()
            .map(|p| {
                let c = std::fs::canonicalize(p).map_err(|e| format!("sandbox: cannot resolve {}: {e}", p.display()))?;
                let dir = c.is_dir();
                Ok((c, dir))
            })
            .collect()
    }
}

/// Enter the sandbox. Returns what is enforced, for `--check-sandbox`; an `Err` means no extension code may run.
pub fn enter(policy: &Policy) -> Result<String, String> {
    #[cfg(windows)]
    if !ext_proto::appcontainer::VERIFIED {
        return Err("AppContainer has not passed the hardware check yet (scripts/check-exthost-sandbox-windows.ps1)".into());
    }
    imp::enter(policy)
}

/// `enter` for `--sandbox-selftest`, which is how a sandbox gets verified: on Windows it runs before `VERIFIED`.
pub fn enter_unverified(policy: &Policy) -> Result<String, String> {
    imp::enter(policy)
}

/// `codeyard-exthost --check-sandbox`: enters a sandbox with an empty policy (the process exits right after) and
/// prints one line, `ok: <what is enforced>` with exit code 0, or `unavailable: <why>` with exit code 1. The app runs
/// it once per session; where it fails, code extensions show "Needs a sandbox this system does not provide".
pub fn check() -> std::process::ExitCode {
    let null = Policy { read: Vec::new(), write: Vec::new() };
    match enter(&null) {
        Ok(what) => {
            println!("ok: {what}");
            std::process::ExitCode::SUCCESS
        }
        Err(why) => {
            println!("unavailable: {why}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::path::Path;

    use super::*;

    const PROFILE: &str = include_str!("sandbox.sb");

    unsafe extern "C" {
        fn sandbox_init(profile: *const libc::c_char, flags: u64, errorbuf: *mut *mut libc::c_char) -> libc::c_int;
        fn sandbox_free_error(errorbuf: *mut libc::c_char);
    }

    pub fn enter(policy: &Policy) -> Result<String, String> {
        let profile = profile(policy)?;
        let c = std::ffi::CString::new(profile).map_err(|_| "sandbox: a path contains a NUL byte".to_string())?;
        let mut err: *mut libc::c_char = std::ptr::null_mut();
        // Safety: a NUL-terminated profile; on failure `err` is a C string owned by libsandbox, freed below.
        if unsafe { sandbox_init(c.as_ptr(), 0, &mut err) } != 0 {
            let why = if err.is_null() {
                "unknown error".to_string()
            } else {
                // Safety: as above.
                let s = unsafe { std::ffi::CStr::from_ptr(err) }.to_string_lossy().into_owned();
                unsafe { sandbox_free_error(err) };
                s
            };
            return Err(format!("sandbox_init: {why}"));
        }
        Ok("seatbelt".into())
    }

    /// sandbox.sb plus one rule per path.
    pub(super) fn profile(policy: &Policy) -> Result<String, String> {
        let mut p = PROFILE.to_string();
        p.push_str("\n;; The extension's own paths.\n");
        for (path, dir) in Policy::resolved(&policy.read)? {
            p.push_str(&format!("(allow file-read* {})\n", filter(&path, dir)?));
        }
        for (path, dir) in Policy::resolved(&policy.write)? {
            p.push_str(&format!("(allow file-read* file-write* {})\n", filter(&path, dir)?));
        }
        // Opening a path as given (`/var/folders/…`) stats each directory above it and follows symlinks such as
        // `/var`: their metadata only, nothing else of the disk.
        let mut above = std::collections::BTreeSet::new();
        for path in policy.read.iter().chain(&policy.write) {
            let canonical = std::fs::canonicalize(path).unwrap_or_default();
            above.extend(path.ancestors().skip(1).chain(canonical.ancestors().skip(1)).filter(|a| a.is_absolute()).map(Path::to_path_buf));
        }
        for a in above {
            p.push_str(&format!("(allow file-read-metadata {})\n", filter(&a, false)?));
        }
        Ok(p)
    }

    fn filter(path: &Path, dir: bool) -> Result<String, String> {
        let s = path.to_str().ok_or_else(|| format!("sandbox: {} is not UTF-8", path.display()))?;
        let quoted = s.replace('\\', "\\\\").replace('"', "\\\"");
        Ok(if dir { format!("(subpath \"{quoted}\")") } else { format!("(literal \"{quoted}\")") })
    }
}

#[cfg(target_os = "linux")]
#[path = "sandbox_linux.rs"]
mod imp;

#[cfg(windows)]
mod imp {
    use super::*;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TokenIsAppContainer};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// The parent did the work (`ext_proto::appcontainer::spawn`); refuse to run code outside an AppContainer.
    pub fn enter(_: &Policy) -> Result<String, String> {
        let mut token: HANDLE = std::ptr::null_mut();
        let mut is: u32 = 0;
        let mut len = 0u32;
        // Safety: queries this process's own token into a u32; the handle is closed below.
        let ok = unsafe {
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) != 0
                && GetTokenInformation(token, TokenIsAppContainer, (&raw mut is).cast(), 4, &mut len) != 0
        };
        if !token.is_null() {
            // Safety: the handle came from OpenProcessToken.
            unsafe { CloseHandle(token) };
        }
        if !ok {
            return Err(format!("cannot read the process token: {}", std::io::Error::last_os_error()));
        }
        if is == 0 {
            return Err("codeyard-exthost was not started in an AppContainer".into());
        }
        Ok("appcontainer".into())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod imp {
    use super::*;

    pub fn enter(_: &Policy) -> Result<String, String> {
        Err("no OS sandbox is implemented for this system".into())
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn the_profile_denies_by_default_and_quotes_paths() {
        let d = std::env::temp_dir().join(format!("sb \"q\\ {}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("code.cwasm");
        std::fs::write(&f, b"").unwrap();
        let p = imp::profile(&Policy { read: vec![f.clone()], write: vec![d.clone()] }).unwrap();
        assert!(p.contains("(deny default)"));
        assert!(!p.contains("(allow network"), "{p}");
        assert!(!p.contains("(allow process-exec") && !p.contains("(allow process-fork"), "{p}");
        let dir = std::fs::canonicalize(&d).unwrap().to_str().unwrap().replace('\\', "\\\\").replace('"', "\\\"");
        assert!(p.contains(&format!("(allow file-read* (literal \"{dir}/code.cwasm\"))")), "{p}");
        assert!(p.contains(&format!("(allow file-read* file-write* (subpath \"{dir}\"))")), "{p}");
        // Every allow of files names its paths: a bare `(allow file-read*)` would open the whole disk.
        assert!(!p.lines().any(|l| l.trim() == "(allow file-read*)" || l.trim() == "(allow file-write*)"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_missing_path_is_an_error() {
        assert!(imp::profile(&Policy { read: vec!["/nonexistent/codeyard".into()], write: Vec::new() }).is_err());
    }
}
