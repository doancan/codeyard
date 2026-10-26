//! macOS file access (TCC): whether we have Full Disk Access, and if not, where to send the user.
//!
//! Nothing here runs by itself. Like any editor, Codeyard asks for nothing at start: the first read of Desktop, Documents,
//! Downloads, iCloud Drive or an external drive (opening a folder there) makes the system ask, in its own dialog, with the
//! usage descriptions from `Info.plist`. Full Disk Access has no dialog of its own: `full_disk_access` tells whether the switch
//! is on, and the settings page opens from `FULL_DISK_ACCESS_URL` (*Help: Grant Full Disk Access…*).

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// The "Privacy & Security > Full Disk Access" page in System Settings.
pub const FULL_DISK_ACCESS_URL: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles";

/// The `.app` folder the running binary lives in; `None` when run from a plain build directory.
pub fn app_bundle() -> Option<PathBuf> {
    std::env::current_exe().ok()?.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf)
}

/// Locations found on most Macs that can't be read without Full Disk Access.
fn probes(home: &Path) -> Vec<PathBuf> {
    vec![home.join("Library/Safari"), home.join("Library/Mail"), home.join("Library/Messages"), PathBuf::from("/Library/Application Support/com.apple.TCC")]
}

/// `Some(true)` access granted, `Some(false)` the system denied, `None` none of the locations exist (can't decide).
pub fn full_disk_access(home: &Path) -> Option<bool> {
    verdict(probes(home).iter().map(|p| std::fs::read_dir(p).map(|_| ())))
}

fn verdict(results: impl Iterator<Item = std::io::Result<()>>) -> Option<bool> {
    let mut seen = None;
    for r in results {
        match r {
            Ok(()) => return Some(true),
            Err(e) if e.kind() == ErrorKind::PermissionDenied => seen = Some(false),
            Err(_) => {}
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind::*};

    #[test]
    fn denied_means_no_access() {
        assert_eq!(verdict([Err(Error::from(NotFound)), Err(Error::from(PermissionDenied))].into_iter()), Some(false));
    }

    #[test]
    fn any_readable_probe_means_access() {
        assert_eq!(verdict([Err(Error::from(PermissionDenied)), Ok(())].into_iter()), Some(true));
    }

    #[test]
    fn nothing_to_probe_is_undecided() {
        assert_eq!(verdict([Err(Error::from(NotFound))].into_iter()), None);
        assert_eq!(verdict(std::iter::empty()), None);
    }

    #[test]
    fn url_targets_the_full_disk_pane() {
        assert!(FULL_DISK_ACCESS_URL.ends_with("Privacy_AllFiles"));
    }
}
