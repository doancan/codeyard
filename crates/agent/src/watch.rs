//! File watching: polls a small set of paths (open files, open folders) once a
//! second. No dependencies, behaves the same on every file system (NFS included).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use proto::ChangeKind;

use crate::Locked;

/// At most this many paths are watched (the UI doesn't show more than that).
pub const MAX_PATHS: usize = 2048;
pub const INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Stamp {
    mtime: Option<SystemTime>,
    len: u64,
    dir: bool,
    entries: Option<u64>,
}

/// Hash of a directory's entry names. Windows only needs it (NTFS may defer a directory's last-write time); it is a plain
/// function so its cost can be measured on every platform (see the tests).
fn entries_digest(path: &PathBuf) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    std::fs::read_dir(path).ok().map(|entries| {
        let mut names = entries.filter_map(|e| e.ok().map(|e| e.file_name())).collect::<Vec<_>>();
        names.sort();
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        names.hash(&mut hash);
        hash.finish()
    })
}

fn stamp(path: &PathBuf) -> Option<Stamp> {
    let m = std::fs::metadata(path).ok()?;
    // NTFS may defer a directory's last-write timestamp. Observe its entries too,
    // otherwise creating/renaming a child can leave the Explorer stale indefinitely.
    let entries = if cfg!(windows) && m.is_dir() { entries_digest(path) } else { None };
    Some(Stamp { mtime: m.modified().ok(), len: m.len(), dir: m.is_dir(), entries })
}

#[derive(Default)]
pub struct Watcher {
    /// path (as given by the client) -> (opened path, last known state)
    paths: Mutex<HashMap<String, (PathBuf, Option<Stamp>)>>,
    running: AtomicBool,
}

impl Watcher {
    /// Replace the set; newly added paths use their current state as the baseline (no event).
    pub fn set(&self, paths: Vec<(String, PathBuf)>) {
        let mut map = self.paths.locked();
        let mut next = HashMap::new();
        for (key, full) in paths.into_iter().take(MAX_PATHS) {
            let old = map.remove(&key);
            let st = old.map(|(_, s)| s).unwrap_or_else(|| stamp(&full));
            next.insert(key, (full, st));
        }
        *map = next;
    }

    /// One round: return what changed.
    pub fn poll(&self) -> Vec<(String, ChangeKind)> {
        self.poll_with(stamp)
    }

    /// The lock is held only to copy the paths and to store the results, never while `stat` runs: a slow share must not block `set`.
    fn poll_with(&self, stat: impl Fn(&PathBuf) -> Option<Stamp>) -> Vec<(String, ChangeKind)> {
        let watched: Vec<(String, PathBuf, Option<Stamp>)> = self.paths.locked().iter().map(|(key, (full, last))| (key.clone(), full.clone(), *last)).collect();
        let seen: Vec<_> = watched.into_iter().map(|(key, full, last)| (stat(&full), key, full, last)).collect();
        let mut map = self.paths.locked();
        let mut out = Vec::new();
        for (now, key, full, last) in seen {
            // `set` may have removed or replaced the path meanwhile; its own baseline then stands.
            let Some((current, stored)) = map.get_mut(&key) else { continue };
            if *current != full {
                continue;
            }
            let kind = match (last, now) {
                (None, Some(_)) => Some(ChangeKind::Created),
                (Some(_), None) => Some(ChangeKind::Removed),
                (Some(a), Some(b)) if a != b => Some(ChangeKind::Modified),
                _ => None,
            };
            if let Some(k) = kind {
                out.push((key, k));
            }
            *stored = now;
        }
        out.sort();
        out
    }

    /// Start the polling thread (once); it ends when `stop` becomes true.
    pub fn start(self: &Arc<Self>, stop: Arc<AtomicBool>, mut emit: impl FnMut(Vec<(String, ChangeKind)>) -> bool + Send + 'static) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = self.clone();
        std::thread::Builder::new()
            .name("watch".into())
            .spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(INTERVAL);
                    let changes = me.poll();
                    if !changes.is_empty() && !emit(changes) {
                        break;
                    }
                }
            })
            .ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_create_modify_remove_and_dir_changes() {
        let d = std::env::temp_dir().join(format!("codeyard-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("a.txt");
        let w = Watcher::default();
        let key = |p: &PathBuf| p.to_string_lossy().into_owned();
        w.set(vec![(key(&f), f.clone()), (key(&d), d.clone())]);
        assert!(w.poll().is_empty());

        std::fs::write(&f, "1").unwrap(); // file created, directory contents changed
        let c = w.poll();
        assert!(c.contains(&(key(&f), ChangeKind::Created)), "{c:?}");
        assert!(c.iter().any(|(p, k)| *p == key(&d) && *k == ChangeKind::Modified), "{c:?}");

        std::fs::write(&f, "uzunluk değişti").unwrap();
        assert_eq!(w.poll(), [(key(&f), ChangeKind::Modified)]);

        std::fs::remove_file(&f).unwrap();
        assert!(w.poll().contains(&(key(&f), ChangeKind::Removed)));
    }

    #[test]
    fn a_slow_stat_does_not_block_changing_the_watched_set() {
        let d = big_tree("slow", 1, 1);
        let f = d.join("d000").join("f00000.txt");
        let w = Arc::new(Watcher::default());
        w.set(vec![(f.to_string_lossy().into_owned(), f.clone())]);
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let other = d.clone();
        let w2 = w.clone();
        w.poll_with(|p| {
            let (w3, tx, other) = (w2.clone(), done_tx.clone(), other.clone());
            std::thread::spawn(move || {
                w3.set(vec![("other".into(), other)]);
                let _ = tx.send(());
            });
            let finished = done_rx.recv_timeout(Duration::from_secs(3)).is_ok();
            assert!(finished, "set() waited for the poll's stat of {p:?}");
            stamp(p)
        });
        let _ = std::fs::remove_dir_all(&d);
    }

    fn big_tree(name: &str, dirs: usize, files_per_dir: usize) -> PathBuf {
        let d = std::env::temp_dir().join(format!("codeyard-watch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for i in 0..dirs {
            let sub = d.join(format!("d{i:03}"));
            std::fs::create_dir_all(&sub).unwrap();
            for j in 0..files_per_dir {
                std::fs::write(sub.join(format!("f{j:05}.txt")), b"x").unwrap();
            }
        }
        d
    }

    /// The claim "polls a small path set once a second": a poll stats exactly the paths it was given, once each, so its
    /// cost follows the watched set and not the size of the tree around it. The number of stats is counted, not timed.
    #[test]
    fn poll_cost_follows_the_watched_set_not_the_tree() {
        let d = big_tree("cost", 50, 20); // 1 000 files around the 200 watched paths
        let mut watched: Vec<(String, PathBuf)> = Vec::new();
        for i in 0..50 {
            let dir = d.join(format!("d{i:03}"));
            watched.push((dir.to_string_lossy().into_owned(), dir.clone()));
            for j in 0..3 {
                let f = dir.join(format!("f{j:05}.txt"));
                watched.push((f.to_string_lossy().into_owned(), f));
            }
        }
        let w = Watcher::default();
        w.set(watched.clone());
        assert!(w.poll().is_empty());
        let stats = std::sync::atomic::AtomicUsize::new(0);
        for _ in 0..20 {
            let changes = w.poll_with(|p| {
                stats.fetch_add(1, Ordering::Relaxed);
                assert!(watched.iter().any(|(_, w)| w == p), "{p:?} is not a watched path");
                stamp(p)
            });
            assert!(changes.is_empty());
        }
        assert_eq!(stats.load(Ordering::Relaxed), 20 * watched.len(), "one stat per watched path and poll");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The agent never watches more than `MAX_PATHS`; the rest of a longer request is ignored (documented, not silent in the
    /// UI sense: the explorer cannot show more expanded folders than that). Pins the cap and which paths survive.
    #[test]
    fn a_longer_request_is_cut_at_the_cap_in_the_order_given() {
        let d = big_tree("cap", 1, 1);
        let many: Vec<(String, PathBuf)> = (0..MAX_PATHS + 500).map(|i| (format!("/p/{i:05}"), d.clone())).collect();
        let w = Watcher::default();
        w.set(many);
        let kept = w.paths.lock().unwrap();
        assert_eq!(kept.len(), MAX_PATHS);
        assert!(kept.contains_key("/p/00000") && kept.contains_key(&format!("/p/{:05}", MAX_PATHS - 1)));
        assert!(!kept.contains_key(&format!("/p/{:05}", MAX_PATHS)), "paths past the cap are not watched");
        drop(kept);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// What the Windows branch does on every poll for every watched directory: read, sort and hash all of its names. The
    /// digest has to stay put while nothing changes and change when an entry is renamed, in a directory of a few thousand.
    #[test]
    fn entry_digest_of_a_big_directory_follows_its_names() {
        let d = big_tree("digest", 1, 2_000);
        let dir = d.join("d000");
        let a = entries_digest(&dir).unwrap();
        for _ in 0..5 {
            assert_eq!(entries_digest(&dir), Some(a), "stable while nothing changes");
        }
        std::fs::rename(dir.join("f00000.txt"), dir.join("renamed.txt")).unwrap();
        assert_ne!(entries_digest(&dir), Some(a), "a rename changes the digest");
        let _ = std::fs::remove_dir_all(&d);
    }
}
