//! Folder comparison: walks a tree and reports size, mtime, symlink target and, for the files asked for, the content hash.
//! Two passes so the slow one stays small: the listing never reads file contents, and the caller then asks for hashes of
//! only the files that exist on both sides with the same size.

use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use proto::{Error, HashRecord, Kind};

/// Records go out in batches, but a slow walk must not hold the first ones back.
const BATCH: usize = 256;
const FLUSH_EVERY: Duration = Duration::from_millis(50);
/// Hashing is bound by the disk or the network share, not the CPU: a few readers overlap the waits.
const HASH_WORKERS: usize = 4;

pub struct Summary {
    pub entries: u32,
    pub truncated: bool,
    pub cancelled: bool,
}

pub struct Query<'a> {
    pub root: &'a Path,
    pub ignore: &'a [String],
    pub gitignore: bool,
    pub limit: usize,
    pub only: Option<&'a [String]>,
}

pub fn run(q: &Query, cancel: &AtomicBool, emit: impl FnMut(Vec<HashRecord>)) -> Result<Summary, Error> {
    if !q.root.is_dir() {
        return Err(crate::map_err(q.root, io::Error::from(io::ErrorKind::NotFound)));
    }
    match q.only {
        None => walk(q, cancel, emit),
        Some(paths) => hash_files(q.root, paths, cancel, emit),
    }
}

fn mtime_ms(m: &fs::Metadata) -> i64 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64)
}

fn why(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::PermissionDenied => "permission denied".into(),
        io::ErrorKind::NotFound => "disappeared during the scan".into(),
        _ => e.to_string(),
    }
}

/// The path an `ignore` error is about, if it names one.
fn error_path(e: &ignore::Error) -> Option<&Path> {
    match e {
        ignore::Error::WithPath { path, .. } => Some(path),
        ignore::Error::WithLineNumber { err, .. } | ignore::Error::WithDepth { err, .. } => error_path(err),
        ignore::Error::Partial(list) => list.iter().find_map(error_path),
        _ => None,
    }
}

fn error_text(e: &ignore::Error) -> String {
    match e.io_error() {
        Some(io) => why(io),
        None => e.to_string(),
    }
}

fn walk(q: &Query, cancel: &AtomicBool, mut emit: impl FnMut(Vec<HashRecord>)) -> Result<Summary, Error> {
    let mut overrides = ignore::overrides::OverrideBuilder::new(q.root);
    for g in q.ignore.iter().map(|g| g.trim()).filter(|g| !g.is_empty()) {
        overrides.add(&format!("!{g}")).map_err(|e| Error::InvalidPattern(e.to_string()))?;
    }
    let overrides = overrides.build().map_err(|e| Error::InvalidPattern(e.to_string()))?;
    let walker = ignore::WalkBuilder::new(q.root)
        .standard_filters(q.gitignore)
        .hidden(false)
        .require_git(false)
        .follow_links(false)
        .overrides(overrides)
        .sort_by_file_name(|a, b| a.cmp(b))
        .build();

    let mut batch = Vec::with_capacity(BATCH);
    let mut last_flush = Instant::now();
    let (mut entries, mut truncated) = (0u32, false);
    // The walker yields an unreadable folder first as an ordinary entry and then as an error: the record is held back by one
    // step so the error lands on that row instead of making a second one.
    let mut held: Option<HashRecord> = None;
    let mut push = |record: HashRecord, entries: &mut u32, batch: &mut Vec<HashRecord>, last_flush: &mut Instant| {
        *entries += 1;
        batch.push(record);
        if batch.len() >= BATCH || last_flush.elapsed() >= FLUSH_EVERY {
            emit(std::mem::take(batch));
            *last_flush = Instant::now();
        }
    };
    for item in walker {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let record = match item {
            Ok(entry) => {
                if entry.depth() == 0 {
                    continue;
                }
                let Ok(rel) = entry.path().strip_prefix(q.root) else { continue };
                let path = crate::relative_path(rel);
                let kind = entry.file_type().map_or(Kind::Other, crate::kind_of);
                match entry.metadata() {
                    Ok(m) => {
                        let link = (kind == Kind::Symlink).then(|| fs::read_link(entry.path()).map(|t| t.to_string_lossy().into_owned()).unwrap_or_default());
                        let size = if kind == Kind::File { m.len() } else { 0 };
                        HashRecord { path, kind, size, mtime_ms: mtime_ms(&m), hash: None, link, error: None }
                    }
                    Err(e) => HashRecord { path, kind, size: 0, mtime_ms: 0, hash: None, link: None, error: Some(error_text(&e)) },
                }
            }
            Err(e) => {
                let Some(rel) = error_path(&e).and_then(|p| p.strip_prefix(q.root).ok()).filter(|r| !r.as_os_str().is_empty()) else { continue };
                let (path, text) = (crate::relative_path(rel), error_text(&e));
                match held.as_mut() {
                    Some(h) if h.path == path => h.error = Some(text),
                    _ => {
                        if let Some(h) = held.take() {
                            push(h, &mut entries, &mut batch, &mut last_flush);
                        }
                        held = Some(HashRecord { path, kind: Kind::Dir, size: 0, mtime_ms: 0, hash: None, link: None, error: Some(text) });
                    }
                }
                continue;
            }
        };
        if let Some(h) = held.replace(record) {
            push(h, &mut entries, &mut batch, &mut last_flush);
        }
        if entries as usize >= q.limit {
            // One more entry exists beyond the limit (the one just held back).
            truncated = true;
            held = None;
            break;
        }
    }
    if let Some(h) = held.take() {
        push(h, &mut entries, &mut batch, &mut last_flush);
    }
    if !batch.is_empty() {
        emit(batch);
    }
    Ok(Summary { entries, truncated, cancelled: cancel.load(Ordering::Relaxed) })
}

/// A relative path from the client: only plain names, so it cannot leave the root by `..` or an absolute prefix.
pub fn is_plain_relative(rel: &str) -> bool {
    !rel.is_empty() && !rel.contains('\0') && Path::new(rel).components().all(|c| matches!(c, Component::Normal(_)))
}

/// Read that fails as soon as the cancel flag is set, so a huge file does not delay a cancel until it is read to the end.
struct Guarded<'a, R>(R, &'a AtomicBool);

impl<R: Read> Read for Guarded<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.1.load(Ordering::Relaxed) {
            return Err(io::Error::other("cancelled"));
        }
        self.0.read(buf)
    }
}

fn hash_one(root: &Path, canon_root: &Path, rel: &str, cancel: &AtomicBool) -> HashRecord {
    let failed = |kind, error: String| HashRecord { path: rel.to_string(), kind, size: 0, mtime_ms: 0, hash: None, link: None, error: Some(error) };
    if !is_plain_relative(rel) {
        return failed(Kind::Other, format!("not a path inside the folder: {rel}"));
    }
    let full = root.join(rel);
    let meta = match fs::symlink_metadata(&full) {
        Ok(m) => m,
        Err(e) => return failed(Kind::Other, why(&e)),
    };
    if meta.file_type().is_symlink() {
        let link = fs::read_link(&full).map(|t| t.to_string_lossy().into_owned()).ok();
        return HashRecord { path: rel.to_string(), kind: Kind::Symlink, size: 0, mtime_ms: mtime_ms(&meta), hash: None, link, error: None };
    }
    if !meta.is_file() {
        return failed(crate::kind_of(meta.file_type()), "not a regular file".into());
    }
    // A parent that is a link would lead out of the root even though the last part is a plain file.
    if !fs::canonicalize(&full).is_ok_and(|c| c.starts_with(canon_root)) {
        return failed(Kind::File, "outside the folder".into());
    }
    let result = fs::File::open(&full).and_then(|f| {
        let m = f.metadata()?;
        let hash = proto::content_hash_reader(&mut Guarded(f, cancel))?;
        Ok((m, hash))
    });
    match result {
        Ok((m, hash)) => {
            HashRecord { path: rel.to_string(), kind: Kind::File, size: m.len(), mtime_ms: mtime_ms(&m), hash: Some(hash), link: None, error: None }
        }
        Err(e) => failed(Kind::File, why(&e)),
    }
}

fn hash_files(root: &Path, paths: &[String], cancel: &AtomicBool, mut emit: impl FnMut(Vec<HashRecord>)) -> Result<Summary, Error> {
    let canon_root = fs::canonicalize(root).map_err(|e| crate::map_err(root, e))?;
    let next = AtomicUsize::new(0);
    let (tx, rx) = mpsc::channel::<HashRecord>();
    let mut entries = 0u32;
    std::thread::scope(|scope| {
        for _ in 0..HASH_WORKERS.min(paths.len()) {
            let (tx, next, canon_root) = (tx.clone(), &next, &canon_root);
            scope.spawn(move || {
                while !cancel.load(Ordering::Relaxed) {
                    let Some(rel) = paths.get(next.fetch_add(1, Ordering::Relaxed)) else { break };
                    if tx.send(hash_one(root, canon_root, rel, cancel)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        let mut batch = Vec::new();
        let mut last_flush = Instant::now();
        for record in rx {
            if cancel.load(Ordering::Relaxed) {
                continue; // a cancelled read would show up as an error row
            }
            entries += 1;
            batch.push(record);
            if batch.len() >= BATCH || last_flush.elapsed() >= FLUSH_EVERY {
                emit(std::mem::take(&mut batch));
                last_flush = Instant::now();
            }
        }
        if !batch.is_empty() {
            emit(batch);
        }
    });
    Ok(Summary { entries, truncated: false, cancelled: cancel.load(Ordering::Relaxed) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("codeyard-hashtree-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn list(root: &Path, ignore: &[&str], gitignore: bool, limit: usize) -> (Vec<HashRecord>, Summary) {
        let ignore: Vec<String> = ignore.iter().map(|s| s.to_string()).collect();
        let q = Query { root, ignore: &ignore, gitignore, limit, only: None };
        let mut out = Vec::new();
        let s = run(&q, &AtomicBool::new(false), |b| out.extend(b)).unwrap();
        (out, s)
    }

    fn names(records: &[HashRecord]) -> Vec<&str> {
        records.iter().map(|r| r.path.as_str()).collect()
    }

    #[test]
    fn lists_files_and_folders_without_hashing() {
        let dir = tmp("list");
        fs::create_dir_all(dir.join("src/deep")).unwrap();
        fs::write(dir.join("src/a.rs"), "abc").unwrap();
        fs::write(dir.join("src/deep/b.rs"), "").unwrap();
        fs::write(dir.join("top.txt"), "xy").unwrap();
        let (records, s) = list(&dir, &[], true, 100);
        assert_eq!(names(&records), ["src", "src/a.rs", "src/deep", "src/deep/b.rs", "top.txt"]);
        assert_eq!(records[1].size, 3);
        assert!(records.iter().all(|r| r.hash.is_none() && r.error.is_none()));
        assert_eq!((s.entries, s.truncated, s.cancelled), (5, false, false));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ignore_globs_and_gitignore_are_honoured_separately() {
        let dir = tmp("ignore");
        fs::create_dir_all(dir.join("target/debug")).unwrap();
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::create_dir_all(dir.join("sub/target")).unwrap();
        fs::write(dir.join("target/debug/x"), "1").unwrap();
        fs::write(dir.join("sub/target/y"), "1").unwrap();
        fs::write(dir.join(".git/HEAD"), "ref").unwrap();
        fs::write(dir.join(".gitignore"), "*.log\n").unwrap();
        fs::write(dir.join("keep.rs"), "1").unwrap();
        fs::write(dir.join("noise.log"), "1").unwrap();
        let (records, _) = list(&dir, &[".git", "target"], true, 100);
        assert_eq!(names(&records), [".gitignore", "keep.rs", "sub"], "target at any depth, .git and *.log are gone");
        let (records, _) = list(&dir, &[".git", "target"], false, 100);
        assert_eq!(names(&records), [".gitignore", "keep.rs", "noise.log", "sub"], "without gitignore the log file is listed");
        let (records, _) = list(&dir, &[], false, 100);
        assert!(names(&records).contains(&".git/HEAD") && names(&records).contains(&"target/debug/x"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_reported_by_target_and_never_followed() {
        let dir = tmp("links");
        let outside = tmp("links-outside");
        fs::write(outside.join("secret.txt"), "s").unwrap();
        fs::create_dir_all(dir.join("real")).unwrap();
        fs::write(dir.join("real/f"), "1").unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("escape")).unwrap();
        std::os::unix::fs::symlink("real/f", dir.join("alias")).unwrap();
        std::os::unix::fs::symlink("nowhere", dir.join("dangling")).unwrap();
        let (records, _) = list(&dir, &[], true, 100);
        assert_eq!(names(&records), ["alias", "dangling", "escape", "real", "real/f"], "nothing from inside the link to a folder");
        let get = |n: &str| records.iter().find(|r| r.path == n).unwrap();
        assert_eq!((get("alias").kind, get("alias").link.as_deref()), (Kind::Symlink, Some("real/f")));
        assert_eq!(get("dangling").link.as_deref(), Some("nowhere"));
        assert_eq!(get("escape").link.as_deref(), Some(outside.to_str().unwrap()));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn the_limit_truncates_and_says_so() {
        let dir = tmp("limit");
        for i in 0..10 {
            fs::write(dir.join(format!("f{i}")), "x").unwrap();
        }
        let (records, s) = list(&dir, &[], true, 4);
        assert_eq!(records.len(), 4);
        assert!(s.truncated);
        let (records, s) = list(&dir, &[], true, 10);
        assert_eq!((records.len(), s.truncated), (10, false), "exactly the limit is not truncated");
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_folder_and_file_are_error_rows_and_the_scan_continues() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp("denied");
        fs::create_dir_all(dir.join("locked")).unwrap();
        fs::write(dir.join("locked/inner"), "1").unwrap();
        fs::write(dir.join("secret"), "1").unwrap();
        fs::write(dir.join("ok"), "1").unwrap();
        fs::set_permissions(dir.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
        fs::set_permissions(dir.join("secret"), fs::Permissions::from_mode(0o000)).unwrap();
        // Running as root reads everything: the test would prove nothing.
        if fs::read_dir(dir.join("locked")).is_ok() {
            fs::set_permissions(dir.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
            let _ = fs::remove_dir_all(&dir);
            return;
        }
        let (records, _) = list(&dir, &[], true, 100);
        let locked = records.iter().find(|r| r.path == "locked").unwrap();
        assert_eq!(locked.error.as_deref(), Some("permission denied"));
        assert!(records.iter().any(|r| r.path == "ok" && r.error.is_none()), "the rest was still scanned");
        let only = vec!["secret".to_string(), "ok".to_string()];
        let mut out = Vec::new();
        run(&Query { root: &dir, ignore: &[], gitignore: true, limit: 10, only: Some(&only) }, &AtomicBool::new(false), |b| out.extend(b)).unwrap();
        let secret = out.iter().find(|r| r.path == "secret").unwrap();
        assert_eq!((secret.hash, secret.error.as_deref()), (None, Some("permission denied")));
        assert!(out.iter().any(|r| r.path == "ok" && r.hash.is_some()));
        fs::set_permissions(dir.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hashes_only_the_files_asked_for_and_matches_the_protocol_hash() {
        let dir = tmp("hash");
        fs::write(dir.join("a.txt"), "merhaba").unwrap();
        fs::write(dir.join("b.txt"), "merhaba").unwrap();
        fs::write(dir.join("c.txt"), "dünya").unwrap();
        let only = vec!["a.txt".to_string(), "c.txt".to_string()];
        let mut out = Vec::new();
        let s = run(&Query { root: &dir, ignore: &[], gitignore: true, limit: 1, only: Some(&only) }, &AtomicBool::new(false), |b| out.extend(b)).unwrap();
        out.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(s.entries, 2);
        assert_eq!(names(&out), ["a.txt", "c.txt"]);
        assert_eq!(out[0].hash, Some(proto::content_hash(b"merhaba")));
        assert_eq!(out[1].hash, Some(proto::content_hash("dünya".as_bytes())));
        assert_eq!(out[0].size, 7);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hash_requests_cannot_leave_the_root() {
        let dir = tmp("escape");
        let root = dir.join("root");
        fs::create_dir_all(&root).unwrap();
        fs::write(dir.join("outside.txt"), "private").unwrap();
        let abs = dir.join("outside.txt").to_string_lossy().into_owned();
        let only = vec!["../outside.txt".to_string(), abs, "a/../../outside.txt".to_string(), String::new()];
        let mut out = Vec::new();
        run(&Query { root: &root, ignore: &[], gitignore: true, limit: 10, only: Some(&only) }, &AtomicBool::new(false), |b| out.extend(b)).unwrap();
        assert_eq!(out.len(), 4);
        assert!(out.iter().all(|r| r.hash.is_none() && r.error.is_some()), "{out:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_reached_through_a_linked_folder_is_refused() {
        let dir = tmp("viadir");
        let outside = tmp("viadir-outside");
        fs::write(outside.join("x.txt"), "private").unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("link")).unwrap();
        let only = vec!["link/x.txt".to_string()];
        let mut out = Vec::new();
        run(&Query { root: &dir, ignore: &[], gitignore: true, limit: 10, only: Some(&only) }, &AtomicBool::new(false), |b| out.extend(b)).unwrap();
        assert_eq!(out[0].hash, None);
        assert_eq!(out[0].error.as_deref(), Some("outside the folder"));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn cancel_stops_the_walk() {
        let dir = tmp("cancel");
        for i in 0..20 {
            fs::write(dir.join(format!("f{i}")), "x").unwrap();
        }
        let cancel = AtomicBool::new(true);
        let mut out = Vec::new();
        let s = run(&Query { root: &dir, ignore: &[], gitignore: true, limit: 100, only: None }, &cancel, |b| out.extend(b)).unwrap();
        assert!(out.is_empty() && s.cancelled);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_root_is_an_error() {
        let q = Query { root: Path::new("/definitely/not/here"), ignore: &[], gitignore: true, limit: 1, only: None };
        assert!(matches!(run(&q, &AtomicBool::new(false), |_| {}), Err(Error::NotFound(_))));
    }
}
