//! Filesystem-native, exclusive publication for Explorer moves and recursive copies.

use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use proto::Error;

use super::map_err;

static NEXT_COPY: AtomicU64 = AtomicU64::new(0);
const MAX_DEPTH: usize = 128;
const MAX_ENTRIES: usize = 100_000;

/// Unlike `std::fs::rename` on Unix, this operation cannot replace an entry that
/// appears after the caller checked the destination. Unsupported filesystems fail closed.
pub(super) fn rename_exclusive(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let from = CString::new(from.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        let to = CString::new(to.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        // SAFETY: both C strings are valid for the call. The exclusive flag is
        // enforced in the filesystem operation, rather than in a preceding stat.
        #[cfg(target_os = "macos")]
        let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
        #[cfg(target_os = "linux")]
        let result = unsafe { libc::renameat2(libc::AT_FDCWD, from.as_ptr(), libc::AT_FDCWD, to.as_ptr(), libc::RENAME_NOREPLACE) };
        if result == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
        let wide = |path: &Path| -> io::Result<Vec<u16>> {
            let absolute = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir()?.join(path) };
            let parent = absolute.parent().ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
            let name = absolute.file_name().ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
            // Canonicalize the parent for extended-length paths; canonicalizing
            // the entry itself would resolve a symlink we must move unchanged.
            Ok(fs::canonicalize(parent)?.join(name).as_os_str().encode_wide().chain(Some(0)).collect())
        };
        let from = wide(from)?;
        let to = wide(to)?;
        if from[..from.len() - 1].contains(&0) || to[..to.len() - 1].contains(&0) {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        // SAFETY: NUL-terminated UTF-16 strings live through the call. Zero flags
        // deliberately excludes MOVEFILE_REPLACE_EXISTING and cross-volume copies.
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } != 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        let _ = (from, to);
        Err(io::Error::new(io::ErrorKind::Unsupported, "exclusive rename is unavailable"))
    }
}

pub(super) fn copy(from: &Path, to: &Path) -> Result<(), Error> {
    copy_cancellable(from, to, &AtomicBool::new(false))
}

pub(super) fn copy_cancellable(from: &Path, to: &Path, cancel: &AtomicBool) -> Result<(), Error> {
    let source = fs::symlink_metadata(from).map_err(|e| map_err(from, e))?;
    match fs::symlink_metadata(to) {
        Ok(_) => return Err(Error::AlreadyExists(to.display().to_string())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(map_err(to, e)),
    }
    let parent = to.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let parent = fs::canonicalize(parent).map_err(|e| map_err(parent, e))?;
    if source.is_dir() {
        let source_path = fs::canonicalize(from).map_err(|e| map_err(from, e))?;
        if parent.starts_with(&source_path) {
            return Err(Error::PermissionDenied("a folder cannot be copied into itself".into()));
        }
    }
    // Reserve a sibling with an exclusive create, populate it privately, then publish.
    // The destination stays absent until the whole operation succeeds.
    for _ in 0..100 {
        let stage = parent.join(format!(".codeyard-copy-{}-{}", std::process::id(), NEXT_COPY.fetch_add(1, Ordering::Relaxed)));
        let file = match reserve(from, &stage, &source) {
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(map_err(&stage, e)),
            Ok(file) => file,
        };
        let mut remaining = MAX_ENTRIES;
        let result = populate(from, &stage, &source, file, 0, &mut remaining, cancel).and_then(|()| {
            check_cancel(cancel)?;
            rename_exclusive(&stage, to)
        });
        if result.is_err() {
            // Only our exclusively reserved staging entry is removed. In particular,
            // never remove the requested destination after a conflicting publication.
            if let Err(cleanup) = remove_stage(&stage) {
                return Err(Error::Other(format!(
                    "{}; could not remove {}: {cleanup}",
                    result.err().map_or(String::new(), |e| e.to_string()),
                    stage.display()
                )));
            }
        }
        return result.map_err(|e| if e.kind() == io::ErrorKind::Interrupted { Error::Cancelled } else { map_err(to, e) });
    }
    Err(Error::Other("could not reserve a temporary copy".into()))
}

fn reserve(from: &Path, to: &Path, meta: &fs::Metadata) -> io::Result<Option<fs::File>> {
    if meta.file_type().is_symlink() {
        let target = fs::read_link(from)?;
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, to).map(|()| None)
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::{FileTypeExt, symlink_dir, symlink_file};
            if meta.file_type().is_symlink_dir() { symlink_dir(target, to) } else { symlink_file(target, to) }.map(|()| None)
        }
    } else if meta.is_dir() {
        fs::create_dir(to).map(|()| None)
    } else if meta.is_file() {
        fs::OpenOptions::new().write(true).create_new(true).open(to).map(Some)
    } else {
        Err(io::Error::new(io::ErrorKind::Unsupported, "only regular files, folders and symlinks can be copied"))
    }
}

fn populate(from: &Path, to: &Path, meta: &fs::Metadata, file: Option<fs::File>, depth: usize, remaining: &mut usize, cancel: &AtomicBool) -> io::Result<()> {
    check_cancel(cancel)?;
    if depth > MAX_DEPTH || *remaining == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "copy exceeds the folder depth or entry limit"));
    }
    *remaining -= 1;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    if meta.is_dir() {
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            let source = entry.path();
            let dest = to.join(entry.file_name());
            let child_meta = fs::symlink_metadata(&source)?;
            let file = reserve(&source, &dest, &child_meta)?;
            populate(&source, &dest, &child_meta, file, depth + 1, remaining, cancel)?;
        }
    } else {
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let mut source = options.open(from)?;
        if !source.metadata()?.is_file() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "source is no longer a regular file"));
        }
        let mut dest = file.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing reserved copy handle"))?;
        let mut chunk = vec![0u8; 256 * 1024];
        loop {
            check_cancel(cancel)?;
            let read = source.read(&mut chunk)?;
            if read == 0 {
                break;
            }
            dest.write_all(&chunk[..read])?;
        }
        dest.flush()?;
        dest.set_permissions(meta.permissions())?;
        return dest.sync_all();
    }
    fs::set_permissions(to, meta.permissions())
}

fn check_cancel(cancel: &AtomicBool) -> io::Result<()> {
    if cancel.load(Ordering::Acquire) { Err(io::Error::new(io::ErrorKind::Interrupted, "copy cancelled")) } else { Ok(()) }
}

fn remove_stage(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() {
        let mut permissions = meta.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(permissions.mode() | 0o700);
        }
        #[cfg(windows)]
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions)?;
        for child in fs::read_dir(path)? {
            remove_stage(&child?.path())?;
        }
        fs::remove_dir(path)
    } else {
        #[cfg(windows)]
        if !meta.file_type().is_symlink() {
            let mut p = meta.permissions();
            p.set_readonly(false);
            fs::set_permissions(path, p)?;
        }
        fs::remove_file(path)
    }
}

#[cfg(test)]
#[path = "fileops_tests.rs"]
mod tests;
