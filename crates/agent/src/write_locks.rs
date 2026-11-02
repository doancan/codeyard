//! Serialize a target's version check and publication across local sessions and agent workers.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use super::Locked;

#[derive(Hash, PartialEq, Eq)]
enum Key {
    Path(PathBuf),
    File(super::file_identity::Identity),
}

pub(super) fn for_path(path: &Path) -> io::Result<Arc<Mutex<()>>> {
    static LOCKS: OnceLock<Mutex<HashMap<Key, Weak<Mutex<()>>>>> = OnceLock::new();
    let identity = super::file_identity::hard_link(path)?.map(Key::File);
    let path = Key::Path(path.into());
    let mut locks = LOCKS.get_or_init(Mutex::default).locked();
    let existing = locks.get(&path).and_then(Weak::upgrade).or_else(|| identity.as_ref().and_then(|key| locks.get(key)).and_then(Weak::upgrade));
    // Waiting writers also hold an Arc. Idle paths need no permanent lock or registry entry.
    locks.retain(|_, lock| lock.strong_count() != 0);
    let lock = existing.unwrap_or_else(|| Arc::new(Mutex::new(())));
    locks.insert(path, Arc::downgrade(&lock));
    if let Some(identity) = identity {
        locks.insert(identity, Arc::downgrade(&lock));
    }
    Ok(lock)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiting_writers_share_their_target_lock_without_blocking_other_files() {
        let first = for_path(Path::new("first-file")).unwrap();
        let held = first.locked();
        let waiting = for_path(Path::new("first-file")).unwrap();
        assert!(Arc::ptr_eq(&first, &waiting));
        assert!(waiting.try_lock().is_err());
        let other = for_path(Path::new("second-file")).unwrap();
        assert!(other.try_lock().is_ok(), "unrelated files can still save concurrently");
        drop(held);
        drop(first);
        assert!(Arc::ptr_eq(&waiting, &for_path(Path::new("first-file")).unwrap()), "a waiting writer retains the same lock");
    }
}
