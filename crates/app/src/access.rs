//! macOS file access (TCC): whether we have Full Disk Access, and if not, where to send the user.
//!
//! The app can't request this permission itself (the system offers no dialog for it); what we can do
//! is detect its absence and open the "Full Disk Access" settings directly.

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

/// Whether to ask at startup: only when access is definitely missing.
pub fn should_ask(home: &Path) -> bool {
    full_disk_access(home) == Some(false)
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
