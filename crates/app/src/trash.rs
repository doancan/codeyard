//! Move local files to the trash (VS Code `files.enableTrash`).
//!
//! macOS: `~/.Trash`. Linux and other Unixes: the XDG trash (`files/` + `info/*.trashinfo`),
//! so that the file manager's "restore" command works. If a file on a different disk
//! cannot be moved, an error is returned; the UI asks separately about permanent deletion.
//! Windows uses the Shell's Recycle Bin, including Explorer restore/undo information.

use std::io;
use std::path::Path;
#[cfg(not(windows))]
use std::path::PathBuf;

#[cfg(windows)]
mod windows;

/// Move to the platform's recoverable trash. Errors leave permanent deletion to the UI's separate confirmation.
pub fn move_to_trash(path: &Path) -> io::Result<()> {
    path.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, crate::i18n::t("no file name")))?;
    #[cfg(windows)]
    return windows::recycle(path);
    #[cfg(not(windows))]
    move_to_directory_trash(path).map(|_| ())
}

#[cfg(not(windows))]
fn move_to_directory_trash(path: &Path) -> io::Result<PathBuf> {
    let dir = trash_dir()?;
    let files = if cfg!(target_os = "macos") { dir.clone() } else { dir.join("files") };
    std::fs::create_dir_all(&files)?;
    let name = path.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, crate::i18n::t("no file name")))?;
    let dest = unique(&files, &name.to_string_lossy());
    // XDG: write the info file first (it reserves the name); undone if the move fails, so no ghost stays in the trash.
    #[cfg(not(target_os = "macos"))]
    let info_file = {
        let info = dir.join("info");
        std::fs::create_dir_all(&info)?;
        let stem = dest.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let body = format!("[Trash Info]\nPath={}\nDeletionDate={}\n", encode_path(path), deletion_date());
        let f = info.join(format!("{stem}.trashinfo"));
        std::fs::write(&f, body)?;
        f
    };
    match std::fs::rename(path, &dest) {
        Ok(()) => Ok(dest),
        Err(e) => {
            #[cfg(not(target_os = "macos"))]
            let _ = std::fs::remove_file(&info_file);
            Err(e)
        }
    }
}

#[cfg(all(test, not(windows)))]
fn trash_dir() -> io::Result<PathBuf> {
    // Tests do not touch the user's real trash.
    Ok(std::env::temp_dir().join(format!("codeyard-test-trash-{}", std::process::id())))
}

#[cfg(all(not(test), not(windows)))]
fn trash_dir() -> io::Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, crate::i18n::t("HOME is not set")))?;
    if cfg!(target_os = "macos") {
        return Ok(home.join(".Trash"));
    }
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"));
    Ok(data.join("Trash"))
}

/// If something with the same name is already in the trash, "name 2.ext", "name 3.ext"…
#[cfg(not(windows))]
fn unique(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if std::fs::symlink_metadata(&first).is_err() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    (2..).map(|n| dir.join(format!("{stem} {n}{ext}"))).find(|p| std::fs::symlink_metadata(p).is_err()).unwrap_or(first)
}

/// `.trashinfo` path: percent-encoded as in RFC 3986 (slashes remain).
#[cfg(all(not(windows), any(not(target_os = "macos"), test)))]
fn encode_path(path: &Path) -> String {
    let mut out = String::new();
    for b in path.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// UTC "YYYY-MM-DDThh:mm:ss" without local-time information (the spec asks for local time; file
/// managers only use it for sorting).
#[cfg(all(not(windows), not(target_os = "macos")))]
fn deletion_date() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;

    #[test]
    fn moves_into_trash_with_unique_names() {
        let src = std::env::temp_dir().join(format!("codeyard-trash-src-{}", std::process::id()));
        std::fs::create_dir_all(&src).unwrap();
        let a = src.join("not.txt");
        std::fs::write(&a, "1").unwrap();
        let first = move_to_directory_trash(&a).unwrap();
        assert!(!a.exists());
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "1");
        std::fs::write(&a, "2").unwrap();
        let second = move_to_directory_trash(&a).unwrap();
        assert_eq!(second.file_name().unwrap(), "not 2.txt");
        // A folder is moved too.
        let d = src.join("klasör");
        std::fs::create_dir_all(d.join("iç")).unwrap();
        assert!(move_to_directory_trash(&d).unwrap().join("iç").is_dir());
        assert_eq!(encode_path(Path::new("/a b/ç.txt")), "/a%20b/%C3%A7.txt");
        // A file that cannot be moved (does not exist): an error, no trace is left in the trash.
        let listing = || std::fs::read_dir(trash_dir().unwrap().join(if cfg!(target_os = "macos") { "" } else { "info" })).map(|d| d.count()).unwrap_or(0);
        let before = listing();
        assert!(move_to_trash(&src.join("yok.txt")).is_err());
        assert_eq!(listing(), before);
    }
}
