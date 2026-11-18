//! Local history (like IntelliJ's "Local History"): a copy of the file is kept on every save,
//! so changes that never reached git can be restored too. Copies live in the data folder as
//! `history/<file digest>/<time ms>.txt`; at most `KEEP` per file, at most 30 days.

use std::path::{Path, PathBuf};

/// Number of copies kept per file.
pub const KEEP: usize = 40;
/// Files larger than this aren't copied.
pub const MAX_BYTES: usize = 2 * 1024 * 1024;
/// Days a copy is kept (the question about a server's files and the user guide quote this number).
pub const KEEP_DAYS: u64 = 30;
const MAX_AGE_MS: u64 = KEEP_DAYS * 24 * 3600 * 1000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Save time (Unix ms).
    pub at_ms: u64,
    pub path: PathBuf,
}

/// FNV-1a: a digest of the path that is stable and safe as a file name.
fn digest(key: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in key.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// A file's copy folder; `host`: SSH alias (empty for local).
pub fn dir_for(data: &Path, host: &str, path: &str) -> PathBuf {
    data.join("history").join(digest(&format!("{host}\u{1f}{path}")))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// The copies, newest first.
pub fn list(dir: &Path) -> Vec<Entry> {
    let mut v: Vec<Entry> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let at_ms = name.strip_suffix(".txt")?.parse().ok()?;
            Some(Entry { at_ms, path: e.path() })
        })
        .collect();
    v.sort_by_key(|e| std::cmp::Reverse(e.at_ms));
    v
}

/// Write a new copy. `false` if it equals the newest or is too large. Prunes old ones.
pub fn record(dir: &Path, text: &str) -> std::io::Result<bool> {
    if text.len() > MAX_BYTES {
        return Ok(false);
    }
    let existing = list(dir);
    if existing.first().is_some_and(|last| std::fs::read_to_string(&last.path).is_ok_and(|t| t == text)) {
        return Ok(false);
    }
    std::fs::create_dir_all(dir)?;
    let mut at = now_ms();
    // Second save in the same millisecond: keep the order intact.
    if let Some(last) = existing.first() {
        at = at.max(last.at_ms + 1);
    }
    std::fs::write(dir.join(format!("{at}.txt")), text)?;
    let now = now_ms();
    for (i, e) in existing.iter().enumerate() {
        // The new copy + (KEEP-1) old ones remain.
        if i + 1 >= KEEP || now.saturating_sub(e.at_ms) > MAX_AGE_MS {
            let _ = std::fs::remove_file(&e.path);
        }
    }
    Ok(true)
}

/// `localHistory.remote`: whether the copies of a remote server's files are kept on this computer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Remote {
    /// Ask once per host; nothing is written before the answer.
    #[default]
    Ask,
    Keep,
    Off,
}

impl Remote {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "ask" => Some(Remote::Ask),
            "keep" => Some(Remote::Keep),
            "off" => Some(Remote::Off),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Remote::Ask => "ask",
            Remote::Keep => "keep",
            Remote::Off => "off",
        }
    }
}

/// Files whose copies would leave secrets outside the project, on a disk the project's `.gitignore` and cleaning do not
/// reach. Matched against the file name, case-insensitively.
const SECRET_NAMES: &[&str] = &[
    ".env",
    ".env.*",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "id_rsa*",
    "id_ed25519*",
    "id_ecdsa*",
    "*.kdbx",
    ".netrc",
    ".npmrc",
    ".pypirc",
    "credentials*.json",
    "*.tfstate",
];
/// Names that follow the `.env` pattern but are examples by convention (not content-based secret detection).
const EXAMPLE_NAMES: &[&str] = &[".env.example", ".env.sample"];

/// Whether a save of `path` (a local or remote path, `/` or `\\` separated) is kept: not a secret file of the built-in list
/// and not matched by the user's `localHistory.exclude`. A pattern with a `/` matches a run of folders and the file at the
/// end of it, wherever it starts (the path is absolute, the pattern is written relative to the project).
pub fn keeps(path: &str, user_exclude: &[String]) -> bool {
    use crate::globs::{name_matches, path_matches};
    let path = path.replace('\\', "/").to_lowercase();
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let Some(name) = parts.last() else { return true };
    if !EXAMPLE_NAMES.contains(name) && SECRET_NAMES.iter().any(|p| name_matches(p, name)) {
        return false;
    }
    !user_exclude.iter().map(|p| p.to_lowercase()).any(|pattern| {
        if pattern.trim_end_matches('/').contains('/') {
            (0..parts.len()).any(|from| (from + 1..=parts.len()).any(|to| path_matches(&pattern, &parts[from..to].join("/"))))
        } else {
            parts.iter().any(|part| name_matches(&pattern, part))
        }
    })
}

/// What a deletion did: copies removed, and copies that could not be (the caller must not call that a success).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Deleted {
    pub copies: usize,
    pub failed: usize,
}

/// Delete one file's copies (its folder `dir`, see [`dir_for`]).
pub fn delete_file(dir: &Path) -> Deleted {
    let mut outcome = Deleted::default();
    for entry in list(dir) {
        match std::fs::remove_file(&entry.path) {
            Ok(()) => outcome.copies += 1,
            Err(_) => outcome.failed += 1,
        }
    }
    // Only an empty folder goes; a stray file someone else put there stays.
    let _ = std::fs::remove_dir(dir);
    outcome
}

/// Delete every file's copies under the data folder `data`.
pub fn delete_all(data: &Path) -> Deleted {
    let root = data.join("history");
    let mut outcome = Deleted::default();
    for dir in std::fs::read_dir(&root).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        let one = delete_file(&dir);
        outcome.copies += one.copies;
        outcome.failed += one.failed;
    }
    let _ = std::fs::remove_dir(&root);
    outcome
}

pub fn read(entry: &Entry) -> std::io::Result<String> {
    std::fs::read_to_string(&entry.path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("codeyard-history-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn same_path_same_dir_and_hosts_are_separate() {
        let d = Path::new("/data");
        assert_eq!(dir_for(d, "", "/a/b.rs"), dir_for(d, "", "/a/b.rs"));
        assert_ne!(dir_for(d, "", "/a/b.rs"), dir_for(d, "srv", "/a/b.rs"));
        assert_ne!(dir_for(d, "", "/a/b.rs"), dir_for(d, "", "/a/c.rs"));
    }

    #[test]
    fn copies_are_recorded_newest_first_and_identical_text_is_skipped() {
        let dir = tmp("record");
        assert!(record(&dir, "bir").unwrap());
        assert!(!record(&dir, "bir").unwrap(), "identical content isn't written again");
        assert!(record(&dir, "iki").unwrap());
        assert!(record(&dir, "üç").unwrap());
        let l = list(&dir);
        assert_eq!(l.len(), 3);
        assert_eq!(read(&l[0]).unwrap(), "üç");
        assert_eq!(read(&l[2]).unwrap(), "bir");
        assert!(l[0].at_ms > l[1].at_ms && l[1].at_ms > l[2].at_ms);
    }

    #[test]
    fn old_copies_are_pruned_and_big_files_skipped() {
        let dir = tmp("prune");
        for i in 0..(KEEP + 5) {
            record(&dir, &format!("sürüm {i}")).unwrap();
        }
        let l = list(&dir);
        assert_eq!(l.len(), KEEP);
        assert_eq!(read(&l[0]).unwrap(), format!("sürüm {}", KEEP + 4), "the newest stays");
        assert!(!record(&dir, &"x".repeat(MAX_BYTES + 1)).unwrap());
        assert!(list(&tmp("yok")).is_empty());
    }

    #[test]
    fn secret_files_are_never_kept() {
        for path in [
            "/p/.env",
            "/p/.env.local",
            "/p/.env.production",
            "/p/server.pem",
            "/p/tls.key",
            "/p/cert.p12",
            "/p/cert.pfx",
            "/home/u/.ssh/id_rsa",
            "/home/u/.ssh/id_rsa.pub",
            "/h/id_ed25519",
            "/h/id_ecdsa",
            "/p/vault.kdbx",
            "/home/u/.netrc",
            "/p/.npmrc",
            "/home/u/.pypirc",
            "/p/credentials.json",
            "/p/credentials-prod.json",
            "/p/infra/terraform.tfstate",
        ] {
            assert!(!keeps(path, &[]), "{path} must not be kept");
        }
    }

    #[test]
    fn ordinary_files_and_example_env_files_are_kept() {
        for path in ["/p/src/main.rs", "/p/.env.example", "/p/.env.sample", "/p/environment.md", "/p/keys.md", "/p/monkey.rs", "/p/credentials.md"] {
            assert!(keeps(path, &[]), "{path} must be kept");
        }
    }

    #[test]
    fn matching_ignores_case_and_reads_windows_separators() {
        assert!(!keeps(r"C:\Users\me\proj\.ENV", &[]));
        assert!(!keeps(r"C:\Users\me\.ssh\ID_RSA", &[]));
        assert!(keeps(r"C:\Users\me\proj\.Env.Example", &[]));
        assert!(!keeps(r"C:\proj\config\Db.Secret.yaml", &["config/*.secret.yaml".into()]));
    }

    #[test]
    fn user_patterns_add_to_the_built_in_list() {
        let user = vec!["*.secret.yaml".to_string(), "**/private/**".to_string()];
        assert!(!keeps("/p/db.secret.yaml", &user), "AC2: a name pattern");
        assert!(!keeps("/p/app/private/notes.md", &user), "a path pattern");
        assert!(!keeps("/p/.env", &user), "the built-in list still applies");
        assert!(keeps("/p/db.yaml", &user));
    }

    #[test]
    fn a_pattern_that_matches_everything_keeps_nothing() {
        assert!(!keeps("/p/src/main.rs", &["*".into()]));
    }

    #[test]
    fn deleting_history_removes_one_file_or_everything_and_counts_what_it_did() {
        let data = tmp("delete");
        let (a, b) = (dir_for(&data, "", "/p/a.rs"), dir_for(&data, "srv", "/p/b.rs"));
        record(&a, "one").unwrap();
        record(&a, "two").unwrap();
        record(&b, "other").unwrap();
        assert_eq!(delete_file(&a), Deleted { copies: 2, failed: 0 });
        assert!(list(&a).is_empty() && !a.exists());
        assert_eq!(list(&b).len(), 1, "another file's copies stay");
        assert_eq!(delete_all(&data), Deleted { copies: 1, failed: 0 });
        assert!(!data.join("history").exists(), "AC4: the history folder is gone");
        assert_eq!(delete_all(&data), Deleted::default(), "nothing to delete is not an error");
    }

    #[cfg(unix)]
    #[test]
    fn a_copy_that_cannot_be_deleted_is_counted_as_failed() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return; // root ignores directory permissions
        }
        let dir = tmp("locked");
        record(&dir, "one").unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let outcome = delete_file(&dir);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(outcome, Deleted { copies: 0, failed: 1 });
        assert_eq!(list(&dir).len(), 1);
    }
}

#[cfg(test)]
mod hardening_tests {
    use super::*;

    #[test]
    fn hostile_patterns_and_paths_neither_panic_nor_take_long() {
        let started = std::time::Instant::now();
        let long = format!("/{}/.env", "a/".repeat(5000));
        for pattern in ["", "[", "[]", "\\", "**/**/**/**/**", "*".repeat(500).as_str(), "?*?*?*?*?*?*?*?*x", "[a-"] {
            let _ = keeps(&long, &[pattern.to_string()]);
            let _ = keeps("/p/\u{0}\n\t/x.txt", &[pattern.to_string()]);
            let _ = keeps("", &[pattern.to_string()]);
        }
        assert!(!keeps(&long, &[]), "a secret file deep in a long path is still found");
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "matching is linear in the path, whatever the pattern: {:?}", started.elapsed());
    }

    #[test]
    fn a_path_that_only_looks_like_a_secret_is_kept_and_one_that_is_one_is_not() {
        for path in ["/p/.env.example/readme", "/p/dir.env/a.txt", "/p/key.md", "/p/.envrc", "/p/pem", "/p/my.keyboard"] {
            assert!(keeps(path, &[]), "{path}");
        }
        // The name is what counts, wherever the folder is, even with a trailing slash from a caller or odd separators.
        for path in ["/p/.env/", "//p//.env", "/p/sub/./.env", "C:/p/.ENV.local"] {
            assert!(!keeps(path, &[]), "{path}");
        }
    }
}
