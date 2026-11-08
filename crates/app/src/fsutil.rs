//! File-writing helpers. Files the user edits by hand (settings, shortcuts, projects, themes) get truncated first when written with `std::fs::write`:
//! a crash, power loss or full disk leaves a partial or empty file.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Atomic write: a temp file in the same folder (owner-only, `O_EXCL`), `fsync`, then `rename`. If the target is a symlink
/// (dotfile managers link `settings.jsonc`) the link is kept and the file it points to is updated; an existing file's permissions
/// are kept. On error the original file stays as it was.
pub fn write_atomic(path: &Path, data: impl AsRef<[u8]>) -> std::io::Result<()> {
    write_atomic_with(path, data.as_ref(), 0o666)
}

/// `write_atomic` for files other users have no business reading (crash reports, hand-over files): a new file is owner-only
/// instead of following the umask (on Windows a protected ACL: this user, SYSTEM and Administrators, as in the profile folder).
pub fn write_private(path: &Path, data: impl AsRef<[u8]>) -> std::io::Result<()> {
    write_atomic_with(path, data.as_ref(), 0o600)
}

/// New file for writing (fails if it exists), owner-only (on Windows: the protected ACL of `write_private`).
pub fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let file = opts.open(path)?;
    #[cfg(windows)]
    if let Err(e) = acl::protect(path, false) {
        let _ = std::fs::remove_file(path);
        return Err(e);
    }
    Ok(file)
}

/// `create_dir_all` for a folder of private files: what is created is owner-only on unix, and a folder that already existed
/// with looser permissions (an earlier version used the umask) is tightened. On Windows the folder gets a protected ACL
/// that its files inherit (this user, SYSTEM and Administrators), so a folder outside the profile (`CODEYARD_CONFIG_DIR`
/// on a shared disk) is as private as `%APPDATA%`.
pub fn create_private_dir(path: &Path) -> std::io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        b.mode(0o700);
        b.create(path)?;
        let mode = std::fs::metadata(path)?.permissions().mode();
        if mode & 0o077 != 0 {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        b.create(path)?;
        acl::protect(path, true)
    }
    #[cfg(not(any(unix, windows)))]
    b.create(path)
}

fn write_atomic_with(path: &Path, data: &[u8], new_mode: u32) -> std::io::Result<()> {
    let target: PathBuf = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = target.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = target.file_name().map(|n| n.to_string_lossy().chars().take(60).collect::<String>()).unwrap_or_default();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    let tmp = dir.join(format!(".{name}.codeyard-{}-{}-{nanos:x}.tmp", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed)));
    let existing = std::fs::metadata(&target).ok();
    let result = (|| {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // While writing, an existing file's contents (possibly private) must not be readable by others: owner-only at first, the original permissions only after
            // the data is written. A new file gets `new_mode` & ~umask (0666 like `std::fs::write`, or owner-only).
            opts.mode(if existing.is_some() { 0o600 } else { new_mode });
        }
        let mut f = opts.open(&tmp)?;
        // A new private file is protected before any data is in it.
        #[cfg(windows)]
        if existing.is_none() && new_mode & 0o077 == 0 {
            acl::protect(&tmp, false)?;
        }
        #[cfg(not(any(unix, windows)))]
        let _ = new_mode;
        f.write_all(data)?;
        if let Some(m) = &existing {
            f.set_permissions(m.permissions())?;
        }
        f.sync_all()?;
        std::fs::rename(&tmp, &target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Owner-only on Windows: a protected DACL (nothing inherited from the parent) granting full access to this user, SYSTEM and
/// Administrators, what a profile folder has. The same SID lookup as the agent's terminal pipe (`daemon_windows.rs`).
#[cfg(windows)]
pub(crate) mod acl {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows::Win32::Foundation::{CloseHandle, ERROR_INVALID_FUNCTION, ERROR_NOT_SUPPORTED, ERROR_SUCCESS, HANDLE, HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1, SE_FILE_OBJECT, SetNamedSecurityInfoW,
    };
    use windows::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, GetTokenInformation, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, TOKEN_QUERY,
        TOKEN_USER, TokenUser,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows::core::{BOOL, PCWSTR, PWSTR};

    /// The SID of the user this process runs as, in its `S-1-5-…` form.
    pub(crate) fn current_user() -> io::Result<String> {
        let mut token = HANDLE::default();
        // SAFETY: the pseudo-handle of this process needs no closing; `token` receives a new handle, closed below.
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }?;
        let result = (|| {
            let mut len = 0u32;
            // SAFETY: asking for the size only (fails with ERROR_INSUFFICIENT_BUFFER, `len` is set).
            let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut len) };
            // u64 words keep the buffer aligned for TOKEN_USER.
            let mut buffer = vec![0u64; (len as usize).div_ceil(8).max(1)];
            // SAFETY: the buffer is writable and at least `len` bytes long.
            unsafe { GetTokenInformation(token, TokenUser, Some(buffer.as_mut_ptr().cast()), len, &mut len) }?;
            // SAFETY: GetTokenInformation(TokenUser) filled the aligned buffer with a TOKEN_USER whose SID points into it.
            let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
            let mut text = PWSTR::null();
            // SAFETY: `sid` is valid while `buffer` lives; `text` receives a LocalAlloc'd string, freed below.
            unsafe { ConvertSidToStringSidW(sid, &mut text) }?;
            // SAFETY: a NUL-terminated wide string from ConvertSidToStringSidW.
            let out = unsafe { text.to_string() }.map_err(io::Error::other);
            // SAFETY: allocated by ConvertSidToStringSidW, freed once.
            unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
            out
        })();
        // SAFETY: opened above, closed once.
        let _ = unsafe { CloseHandle(token) };
        result
    }

    /// Replaces the ACL of `path` with the owner-only one; a folder's entries are inherited by what is created in it.
    /// A file system without ACLs (FAT, exFAT) is left as it is, as `chmod` does on Unix.
    pub(crate) fn protect(path: &Path, folder: bool) -> io::Result<()> {
        let inherit = if folder { "OICI" } else { "" };
        let sddl = format!("D:P(A;{inherit};FA;;;{})(A;{inherit};FA;;;SY)(A;{inherit};FA;;;BA)", current_user()?);
        let sddl: Vec<u16> = sddl.encode_utf16().chain([0]).collect();
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: a NUL-terminated SDDL string; `sd` receives a LocalAlloc'd descriptor, freed below.
        unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(sddl.as_ptr()), SDDL_REVISION_1, &mut sd, None) }?;
        let result = (|| {
            let (mut present, mut defaulted) = (BOOL(0), BOOL(0));
            let mut dacl: *mut ACL = std::ptr::null_mut();
            // SAFETY: `sd` is the live descriptor from above; `dacl` points into it.
            unsafe { GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted) }?;
            let name: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
            let info = DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION;
            // SAFETY: a NUL-terminated path and the DACL of `sd`, which lives for the call.
            let status = unsafe { SetNamedSecurityInfoW(PCWSTR(name.as_ptr()), SE_FILE_OBJECT, info, None, None, Some(dacl), None) };
            match status {
                ERROR_SUCCESS | ERROR_NOT_SUPPORTED | ERROR_INVALID_FUNCTION => Ok(()),
                e => Err(io::Error::from_raw_os_error(e.0 as i32)),
            }
        })();
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW, freed once.
        unsafe { LocalFree(Some(HLOCAL(sd.0))) };
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("codeyard-fsutil-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn writes_replace_whole_files_and_leave_no_temp_files() {
        let d = dir("basic");
        let f = d.join("settings.jsonc");
        write_atomic(&f, "one").unwrap();
        write_atomic(&f, b"two").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "two");
        let names: Vec<_> = std::fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["settings.jsonc"], "no temp file remains");
    }

    #[test]
    fn a_failed_write_keeps_the_old_content() {
        let d = dir("fail");
        let f = d.join("keep.json");
        std::fs::write(&f, "precious").unwrap();
        // The target is a directory: the rename fails and nothing else changes.
        let sub = d.join("sub");
        std::fs::create_dir(&sub).unwrap();
        assert!(write_atomic(&sub, "x").is_err());
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "precious");
        assert_eq!(std::fs::read_dir(&d).unwrap().flatten().count(), 2, "temp file cleaned up after the failure");
        // A missing folder is an error, not a panic.
        assert!(write_atomic(&d.join("no/such/dir/file"), "x").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn private_files_and_folders_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let d = dir("private");
        let sub = d.join("a/b");
        create_private_dir(&sub).unwrap();
        assert_eq!((mode(&d.join("a")), mode(&sub)), (0o700, 0o700));
        // A folder that exists with the umask's permissions is tightened.
        let old = d.join("old");
        std::fs::create_dir(&old).unwrap();
        std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o755)).unwrap();
        create_private_dir(&old).unwrap();
        assert_eq!(mode(&old), 0o700);
        let (a, b) = (d.join("a.txt"), d.join("b.txt"));
        write_private(&a, "x").unwrap();
        create_private(&b).unwrap().write_all(b"x").unwrap();
        assert_eq!((mode(&a), mode(&b)), (0o600, 0o600));
        assert!(create_private(&b).is_err(), "never replaces a file");
        // An existing file keeps the permissions it has, as with write_atomic.
        std::fs::set_permissions(&a, std::fs::Permissions::from_mode(0o640)).unwrap();
        write_private(&a, "y").unwrap();
        assert_eq!(mode(&a), 0o640);
    }

    /// The DACL of `path` as SDDL text (`D:P(A;OICI;FA;;;SY)…`).
    #[cfg(windows)]
    fn dacl(path: &Path) -> String {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Foundation::{ERROR_SUCCESS, HLOCAL, LocalFree};
        use windows::Win32::Security::Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT,
        };
        use windows::Win32::Security::{DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR};
        use windows::core::{PCWSTR, PWSTR};
        let name: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: a NUL-terminated path; `sd` receives a LocalAlloc'd descriptor, freed below.
        let status = unsafe { GetNamedSecurityInfoW(PCWSTR(name.as_ptr()), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, None, None, None, None, &mut sd) };
        assert_eq!(status, ERROR_SUCCESS, "{}", path.display());
        let mut text = PWSTR::null();
        // SAFETY: `sd` is live; `text` receives a LocalAlloc'd string, freed below.
        unsafe { ConvertSecurityDescriptorToStringSecurityDescriptorW(sd, SDDL_REVISION_1, DACL_SECURITY_INFORMATION, &mut text, None) }.unwrap();
        // SAFETY: a NUL-terminated string from the call above.
        let out = unsafe { text.to_string() }.unwrap();
        // SAFETY: both allocated by the calls above, freed once.
        unsafe {
            LocalFree(Some(HLOCAL(text.0.cast())));
            LocalFree(Some(HLOCAL(sd.0)));
        }
        out
    }

    /// Gives `path` the DACL `sddl` (`D:(A;OICI;FA;;;WD)`, everyone), as a folder made by hand or another tool could have.
    #[cfg(windows)]
    fn set_dacl(path: &Path, sddl: &str) {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Foundation::{ERROR_SUCCESS, HLOCAL, LocalFree};
        use windows::Win32::Security::Authorization::{
            ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1, SE_FILE_OBJECT, SetNamedSecurityInfoW,
        };
        use windows::Win32::Security::{ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, PSECURITY_DESCRIPTOR};
        use windows::core::{BOOL, PCWSTR};
        let text: Vec<u16> = sddl.encode_utf16().chain([0]).collect();
        let name: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let mut sd = PSECURITY_DESCRIPTOR::default();
        let (mut present, mut defaulted, mut acl) = (BOOL(0), BOOL(0), std::ptr::null_mut::<ACL>());
        // SAFETY: NUL-terminated strings; `sd` is LocalAlloc'd and freed below, `acl` points into it.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(text.as_ptr()), SDDL_REVISION_1, &mut sd, None).unwrap();
            GetSecurityDescriptorDacl(sd, &mut present, &mut acl, &mut defaulted).unwrap();
            assert_eq!(SetNamedSecurityInfoW(PCWSTR(name.as_ptr()), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, None, None, Some(acl), None), ERROR_SUCCESS);
            LocalFree(Some(HLOCAL(sd.0)));
        }
    }

    /// The access entries of an SDDL DACL, without the `D:` flags: `["A;OICI;FA;;;SY", …]`.
    #[cfg(windows)]
    fn aces(sddl: &str) -> Vec<String> {
        sddl.split('(').skip(1).map(|a| a.trim_end_matches(')').to_string()).collect()
    }

    /// `create_private_dir`, `create_private` and `write_private` leave a folder and files that only this user, SYSTEM and
    /// Administrators can open, whatever the parent folder allows: what 0700/0600 are on Unix.
    #[cfg(windows)]
    #[test]
    fn windows_private_files_and_folders_are_owner_only() {
        let user = acl::current_user().unwrap();
        let d = dir("private-acl");
        // The parent lets everyone in, as a folder on a shared disk (`CODEYARD_CONFIG_DIR`) might.
        set_dacl(&d, "D:(A;OICI;FA;;;WD)(A;OICI;FA;;;BU)");
        let sub = d.join("a/b");
        create_private_dir(&sub).unwrap();
        let folder = dacl(&sub);
        assert!(folder.starts_with("D:P"), "protected: nothing inherited from the parent: {folder}");
        let (a, b) = (sub.join("a.txt"), sub.join("b.txt"));
        write_private(&a, "x").unwrap();
        create_private(&b).unwrap().write_all(b"x").unwrap();
        assert!(create_private(&b).is_err(), "never replaces a file");
        for path in [&sub, &a, &b] {
            let sddl = dacl(path);
            let entries = aces(&sddl);
            assert_eq!(entries.len(), 3, "{}: {sddl}", path.display());
            for e in &entries {
                let trustee = e.rsplit(';').next().unwrap();
                // An access entry names this user by SID, or by an alias for a well-known account (`LA` on CI runners).
                assert!(["SY", "BA", "LA", user.as_str()].contains(&trustee), "{}: {e} in {sddl}", path.display());
                assert!(e.starts_with("A;"), "allow entries only: {e}");
            }
            assert!(!sddl.contains(";WD)") && !sddl.contains(";BU)") && !sddl.contains(";AU)"), "{}: {sddl}", path.display());
        }
        // A folder that exists with a loose ACL is tightened, and what is already in it follows.
        let old = d.join("old");
        std::fs::create_dir(&old).unwrap();
        std::fs::write(old.join("crash-1.txt"), "trace").unwrap();
        assert!(dacl(&old.join("crash-1.txt")).contains(";WD)"), "the setup gives everyone access");
        create_private_dir(&old).unwrap();
        for path in [old.clone(), old.join("crash-1.txt")] {
            let sddl = dacl(&path);
            assert!(!sddl.contains(";WD)") && !sddl.contains(";BU)"), "{}: {sddl}", path.display());
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_and_permissions_are_preserved() {
        use std::os::unix::fs::PermissionsExt;
        let d = dir("link");
        let real = d.join("dotfiles-settings.jsonc");
        std::fs::write(&real, "old").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = d.join("settings.jsonc");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_atomic(&link, "new").unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "the link is still a link");
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "new");
        assert_eq!(std::fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o600);
        // A new file gets the same permissions `std::fs::write` would give (umask applied), not the temp file's 0600.
        let (fresh, plain) = (d.join("fresh.json"), d.join("plain.json"));
        write_atomic(&fresh, "x").unwrap();
        std::fs::write(&plain, "x").unwrap();
        assert_eq!(std::fs::metadata(&fresh).unwrap().permissions().mode() & 0o777, std::fs::metadata(&plain).unwrap().permissions().mode() & 0o777);
    }
}
