//! Schema store: fetches a schema from a URL or local file, caches it on disk (7 days; a stale copy is used when
//! offline) and holds the SchemaStore catalog. Downloads use `curl` (ships with macOS, Linux, Windows 10+), so no
//! TLS stack or HTTP client is added. Downloads can be turned off; then only cached and local schemas are used.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde_json::Value;
use sha2::{Digest, Sha256};

pub const CATALOG_URL: &str = "https://www.schemastore.org/api/json/catalog.json";
const TTL: Duration = Duration::from_secs(7 * 24 * 3600);
/// Upper bound for a single schema file (SchemaStore's largest are a few hundred KB).
const MAX_BYTES: u64 = 8 * 1024 * 1024;

/// A local schema is a regular file of a sane size: `"$schema": "/dev/zero"` in any file must not fill the memory,
/// and a FIFO must not park the thread.
fn read_local(path: &str) -> Result<String, String> {
    use std::io::Read;
    let meta = std::fs::metadata(path).map_err(|e| format!("{path}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{path}: not a regular file"));
    }
    let mut text = String::new();
    std::fs::File::open(path).and_then(|f| f.take(MAX_BYTES + 1).read_to_string(&mut text)).map_err(|e| format!("{path}: {e}"))?;
    if text.len() as u64 > MAX_BYTES {
        return Err(format!("{path}: larger than {} MB", MAX_BYTES / 1024 / 1024));
    }
    Ok(text)
}

/// A network share (`\\host\share`, `//host/share`, `\\?\UNC\host\share`). Windows authenticates to the named host the moment such a path is
/// opened, handing over the user's NTLM hash; `file://host/share/x` becomes one of these in `file_uri::to_path`.
fn is_network_path(path: &str) -> bool {
    if let Some(rest) = path.strip_prefix(r"\\?\") {
        return rest.get(..4).is_some_and(|p| p.eq_ignore_ascii_case(r"UNC\"));
    }
    path.starts_with(r"\\") || path.starts_with("//")
}

/// Local file reader: in an SSH workspace schema files live on the file's host, not on this machine.
pub trait FileSource: Send + Sync {
    fn read(&self, path: &str) -> Result<String, String>;
    /// Modification stamp (for the cache key); `None` if unknown.
    fn mtime(&self, path: &str) -> Option<i64>;
}

pub trait Fetch: Send + Sync {
    fn get(&self, url: &str) -> Result<String, String>;
}

/// Download with `curl`.
pub struct Curl;

impl Fetch for Curl {
    fn get(&self, url: &str) -> Result<String, String> {
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(format!("unsupported schema address: {url}"));
        }
        let out = Command::new("curl")
            .args(["-fsSL", "--max-time", "20", "--max-filesize"])
            .arg(MAX_BYTES.to_string())
            .args(["-A", "codeyard", "--"])
            .arg(url)
            .output()
            .map_err(|e| format!("curl: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(if err.is_empty() { format!("could not download {url}") } else { err });
        }
        String::from_utf8(out.stdout).map_err(|_| format!("{url} is not text"))
    }
}

pub struct Store {
    fetch: Box<dyn Fetch>,
    cache: Option<PathBuf>,
    download: AtomicBool,
    /// Untrusted folder: downloads only from SchemaStore addresses (see `allowed_when_restricted`).
    restricted: AtomicBool,
    /// Hosts other than SchemaStore must be approved before a schema is downloaded from them (`http.confirmRequests`).
    gate: Arc<crate::gate::HostGate>,
    mem: Mutex<HashMap<String, Arc<Value>>>,
    catalog: Mutex<Option<Arc<Catalog>>>,
}

impl Store {
    pub fn new(cache: Option<PathBuf>, download: bool) -> Arc<Self> {
        Self::with_fetch(Box::new(Curl), cache, download)
    }

    pub fn with_fetch(fetch: Box<dyn Fetch>, cache: Option<PathBuf>, download: bool) -> Arc<Self> {
        // The library asks nobody by default; the application sets the policy from `http.confirmRequests`.
        let gate = crate::gate::HostGate::default();
        gate.set_mode(crate::gate::Confirm::Never);
        Arc::new(Store {
            fetch,
            cache,
            download: AtomicBool::new(download),
            restricted: AtomicBool::new(false),
            gate: Arc::new(gate),
            mem: Mutex::new(HashMap::new()),
            catalog: Mutex::new(None),
        })
    }

    pub fn set_download(&self, on: bool) {
        self.download.store(on, Ordering::Relaxed);
    }

    /// Resolution for a file from an untrusted folder: the file's `$schema` URL could make us hit any server (leaking
    /// the IP and file name); when restricted, only SchemaStore is downloaded from, cached copies are still used.
    pub fn set_restricted(&self, on: bool) {
        self.restricted.store(on, Ordering::Relaxed);
    }

    /// A schema in an untrusted folder must not point the editor at a network share: opening it, even to see whether it exists,
    /// would make Windows log in to that host. Called before the first filesystem access for a path.
    pub fn check_local(&self, path: &str) -> Result<(), String> {
        if self.restricted.load(Ordering::Relaxed) && is_network_path(path) {
            return Err(format!("{path} is not read: it is a network share and this folder is not trusted (Workspace: Trust This Folder)"));
        }
        Ok(())
    }

    /// The policy and the session's answers for downloads from hosts other than SchemaStore.
    pub fn gate(&self) -> &Arc<crate::gate::HostGate> {
        &self.gate
    }

    pub fn download_enabled(&self) -> bool {
        self.download.load(Ordering::Relaxed)
    }

    fn cache_file(&self, url: &str) -> Option<PathBuf> {
        let hash: String = Sha256::digest(url.as_bytes()).iter().take(12).map(|b| format!("{b:02x}")).collect();
        self.cache.as_ref().map(|d| d.join(format!("{hash}.json")))
    }

    /// Text: `file://` / local paths are read directly; http(s) tries cache, then network, then stale cache.
    pub fn text(&self, url: &str) -> Result<String, String> {
        if url.starts_with("file://") {
            let path = crate::file_uri::to_path(url).ok_or_else(|| format!("Invalid file URI: {url}"))?;
            self.check_local(&path)?;
            return read_local(&path);
        }
        if !url.contains("://") {
            self.check_local(url)?;
            return read_local(url);
        }
        let file = self.cache_file(url);
        let cached = file.as_deref().and_then(|f| std::fs::read_to_string(f).ok().map(|t| (t, fresh(f))));
        match cached {
            Some((text, true)) => return Ok(text),
            Some((text, false)) if !self.download_enabled() => return Ok(text),
            _ => {}
        }
        if !self.download_enabled() {
            return Err(format!("schema downloads are off, and {url} is not cached"));
        }
        if self.restricted.load(Ordering::Relaxed) && !allowed_when_restricted(url) {
            return Err(format!("{url} is not downloaded: this folder is not trusted (Workspace: Trust This Folder)"));
        }
        if !allowed_when_restricted(url) {
            let Some(host) = crate::gate::host_of(url) else {
                return Err(format!("{url} is not downloaded: its host cannot be read unambiguously"));
            };
            if self.gate.is_refused(&host) {
                return Err(format!("{url} is not downloaded: you declined {host} in this session"));
            }
            if self.gate.needs_approval(&host) {
                self.gate.request(&host);
                return Err(format!("{url} is not downloaded yet: {host} has to be approved first (http.confirmRequests)"));
            }
        }
        match self.fetch.get(url) {
            Ok(text) => {
                if let Some(f) = &file {
                    write_cache(f, &text);
                }
                Ok(text)
            }
            // No network: a stale copy beats nothing.
            Err(e) => cached.map(|(t, _)| t).ok_or(e),
        }
    }

    /// Schema document (tolerates comments and trailing commas); kept in memory.
    pub fn json(&self, url: &str) -> Result<Arc<Value>, String> {
        self.json_with(url, None)
    }

    /// With `files`, local paths are read through it (remote workspace); network ones always come from the store.
    pub fn json_with(&self, url: &str, files: Option<&dyn FileSource>) -> Result<Arc<Value>, String> {
        let url = if url.contains("://") { url.split('#').next().unwrap_or(url) } else { url };
        // Local files are re-read every time (the user may edit the schema); network ones stay in memory.
        let local = url.starts_with("file://") || !url.contains("://");
        if !local && let Some(v) = self.mem.lock().unwrap_or_else(|e| e.into_inner()).get(url) {
            return Ok(v.clone());
        }
        let text = match (local, files) {
            (true, Some(f)) => {
                let path =
                    if url.starts_with("file://") { crate::file_uri::to_path(url).ok_or_else(|| format!("Invalid file URI: {url}"))? } else { url.into() };
                self.check_local(&path)?;
                f.read(&path)?
            }
            _ => self.text(url)?,
        };
        let value = match crate::doc::parse_json(&text) {
            Ok(p) => p.value,
            Err(e) => {
                // A bad download (error page, partial file) must not sit in the cache for 7 days and break the schema.
                if !local {
                    self.evict(url);
                }
                return Err(format!("{url}: {} (at byte {})", e.message, e.offset));
            }
        };
        let value = Arc::new(value);
        if !local {
            self.mem.lock().unwrap_or_else(|e| e.into_inner()).insert(url.to_string(), value.clone());
        }
        Ok(value)
    }

    pub fn catalog(&self) -> Result<Arc<Catalog>, String> {
        if let Some(c) = self.catalog.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return Ok(c);
        }
        let text = self.text(CATALOG_URL)?;
        let catalog = match Catalog::parse(&text) {
            Ok(c) => Arc::new(c),
            Err(e) => {
                self.evict(CATALOG_URL);
                return Err(e);
            }
        };
        *self.catalog.lock().unwrap_or_else(|e| e.into_inner()) = Some(catalog.clone());
        Ok(catalog)
    }

    /// Delete this URL's cache file (when it turns out corrupt).
    fn evict(&self, url: &str) {
        if let Some(f) = self.cache_file(url) {
            let _ = std::fs::remove_file(f);
        }
    }

    /// Clears the cache and memory ("Clear schema cache").
    pub fn clear(&self) {
        self.mem.lock().unwrap_or_else(|e| e.into_inner()).clear();
        *self.catalog.lock().unwrap_or_else(|e| e.into_inner()) = None;
        if let Some(d) = &self.cache
            && let Ok(rd) = std::fs::read_dir(d)
        {
            for e in rd.flatten() {
                if e.path().extension().is_some_and(|x| x == "json") {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }
}

/// Addresses downloadable in an untrusted folder: SchemaStore itself (catalog and schemas).
fn allowed_when_restricted(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    matches!(host, "json.schemastore.org" | "schemastore.org" | "www.schemastore.org")
        || (host == "raw.githubusercontent.com" && path.starts_with("SchemaStore/"))
}

fn fresh(path: &Path) -> bool {
    std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok()).is_some_and(|age| age < TTL)
}

fn write_cache(file: &Path, text: &str) {
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Two threads downloading the same URL must not write one temp file and move a half-written file into place.
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = file.with_extension(format!("tmp{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    if std::fs::write(&tmp, text).is_ok() && std::fs::rename(&tmp, file).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

// --- Catalog ---

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub name: String,
    pub url: String,
    pub patterns: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Catalog {
    pub entries: Vec<CatalogEntry>,
}

impl Catalog {
    pub fn parse(text: &str) -> Result<Catalog, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("catalog: {e}"))?;
        let entries = v["schemas"]
            .as_array()
            .ok_or("catalog: no schemas")?
            .iter()
            .filter_map(|s| {
                let url = s["url"].as_str()?.to_string();
                let patterns: Vec<String> = s["fileMatch"].as_array()?.iter().filter_map(|p| p.as_str().map(str::to_string)).collect();
                (!patterns.is_empty()).then(|| CatalogEntry { name: s["name"].as_str().unwrap_or_default().into(), url, patterns })
            })
            .collect();
        Ok(Catalog { entries })
    }

    /// Schema matching a file path; if several match, the most specific (longest) pattern wins.
    pub fn find(&self, path: &str) -> Option<&CatalogEntry> {
        self.entries
            .iter()
            .filter_map(|e| e.patterns.iter().filter(|p| glob_match(p, path)).map(|p| p.len()).max().map(|n| (n, e)))
            .max_by_key(|(n, _)| *n)
            .map(|(_, e)| e)
    }
}

/// `fileMatch` pattern: one without a slash matches the file name, one with a slash matches the end of the path (like VS Code).
/// `*` is one directory level, `**` anything, `?` a single character.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let path = path.replace('\\', "/");
    let pattern = pattern.trim_start_matches("./");
    if !pattern.contains('/') {
        return matches(pattern.as_bytes(), path.rsplit('/').next().unwrap_or(&path).as_bytes());
    }
    let anchored = pattern.starts_with('/');
    let pattern = pattern.trim_start_matches('/');
    let p = if pattern.starts_with("**/") || anchored { pattern.to_string() } else { format!("**/{pattern}") };
    matches(p.as_bytes(), path.trim_start_matches('/').as_bytes())
}

/// `fileMatch` matching. Patterns come from the SchemaStore catalog (remote) and settings: recursive backtracking
/// would go exponential on patterns like `*a*a*a*b`. Failed (pattern, input) suffixes are memoized: at most |pattern| x |input| states.
fn matches(p: &[u8], s: &[u8]) -> bool {
    matches_memo(p, s, &mut std::collections::HashSet::new())
}

fn matches_memo(p: &[u8], s: &[u8], dead: &mut std::collections::HashSet<(usize, usize)>) -> bool {
    // Both are suffixes of the original strings, so their lengths determine the position.
    let key = (p.len(), s.len());
    if dead.contains(&key) {
        return false;
    }
    let ok = match (p.first(), s.first()) {
        (None, None) => true,
        (Some(b'*'), _) if p.starts_with(b"**/") => {
            matches_memo(&p[3..], s, dead) || (!s.is_empty() && matches_memo(p, &s[s.iter().position(|&c| c == b'/').map_or(s.len(), |i| i + 1)..], dead))
        }
        (Some(b'*'), _) if p.starts_with(b"**") => (0..=s.len()).any(|i| matches_memo(&p[2..], &s[i..], dead)),
        (Some(b'*'), _) => (0..=s.len()).take_while(|&i| i == 0 || s[i - 1] != b'/').any(|i| matches_memo(&p[1..], &s[i..], dead)),
        (Some(b'?'), Some(c)) if *c != b'/' => matches_memo(&p[1..], &s[1..], dead),
        (Some(a), Some(b)) if a == b => matches_memo(&p[1..], &s[1..], dead),
        _ => false,
    };
    if !ok {
        dead.insert(key);
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[cfg(unix)]
    #[test]
    fn local_schemas_must_be_regular_files_of_sane_size() {
        let store = Store::new(None, false);
        for url in ["/dev/zero", "file:///dev/zero", "/dev/null/x"] {
            assert!(store.text(url).is_err(), "{url}");
        }
        let dir = std::env::temp_dir().join(format!("codeyard-schema-big-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let big = dir.join("big.json");
        std::fs::write(&big, vec![b' '; MAX_BYTES as usize + 1]).unwrap();
        assert!(store.text(big.to_str().unwrap()).unwrap_err().contains("larger than"));
        let ok = dir.join("ok.json");
        std::fs::write(&ok, "{}").unwrap();
        assert_eq!(store.text(ok.to_str().unwrap()).unwrap(), "{}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Records every call; an untrusted folder's network-share schema must not reach it.
    struct Spy(AtomicUsize);
    impl FileSource for Spy {
        fn read(&self, _: &str) -> Result<String, String> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok("{}".into())
        }
        fn mtime(&self, _: &str) -> Option<i64> {
            self.0.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    #[test]
    fn untrusted_folders_do_not_open_network_shares() {
        let store = Store::new(None, false);
        store.set_restricted(true);
        let spy = Spy(AtomicUsize::new(0));
        for url in ["//host/share/x.json", r"\\host\share\x.json", r"\\?\UNC\host\share\x.json", "file://host/share/x.json"] {
            let e = store.json_with(url, Some(&spy)).unwrap_err();
            assert!(e.contains("network share"), "{url}: {e}");
            assert!(store.text(url).unwrap_err().contains("network share"), "{url}");
        }
        assert_eq!(spy.0.load(Ordering::Relaxed), 0, "the file source was never asked");
        // Ordinary local paths still work in an untrusted folder, and a trusted one may use a share.
        assert!(store.json_with("/srv/app/x.json", Some(&spy)).is_ok());
        assert!(store.json_with(r"\\?\C:\work\x.json", Some(&spy)).is_ok(), "a verbatim drive path is not a share");
        store.set_restricted(false);
        assert!(store.json_with("//host/share/x.json", Some(&spy)).is_ok());
    }

    #[test]
    fn untrusted_folders_may_only_download_from_schemastore() {
        let calls = Arc::new(AtomicUsize::new(0));
        let store = Store::with_fetch(Box::new(Fake(calls.clone(), Ok("{}".into()))), None, true);
        store.set_restricted(true);
        let e = store.json("https://attacker.example/x.json").unwrap_err();
        assert!(e.contains("not trusted"), "{e}");
        assert_eq!(calls.load(Ordering::Relaxed), 0, "no request was made");
        assert!(store.json("http://json.schemastore.org/x.json").is_err(), "plain http is not SchemaStore's address");
        assert!(store.json("https://json.schemastore.org.evil.example/x.json").is_err(), "look-alike hosts");
        assert!(store.json("https://json.schemastore.org/package.json").is_ok());
        assert!(store.json("https://raw.githubusercontent.com/SchemaStore/schemastore/master/x.json").is_ok());
        assert!(store.json("https://raw.githubusercontent.com/other/repo/x.json").is_err());
        store.set_restricted(false);
        assert!(store.json("https://attacker.example/x.json").is_ok(), "a trusted folder downloads as before");
    }

    /// Remote schema hosts other than SchemaStore wait for an approval; the request is recorded once for the UI to ask;
    /// local hosts, `never`, and cached copies do not ask; a refusal sticks for the session and makes no request.
    #[test]
    fn schema_downloads_from_new_hosts_wait_for_approval() {
        use crate::gate::Confirm;
        let calls = Arc::new(AtomicUsize::new(0));
        let store = Store::with_fetch(Box::new(Fake(calls.clone(), Ok("{}".into()))), None, true);
        store.gate().set_mode(Confirm::Remote);
        let url = "https://schemas.example.com/a.json";
        let e = store.json(url).unwrap_err();
        assert!(e.contains("approved first") && e.contains("schemas.example.com"), "{e}");
        assert!(store.json("https://schemas.example.com/b.json").is_err());
        assert_eq!(calls.load(Ordering::Relaxed), 0, "nothing was requested");
        assert_eq!(store.gate().take_pending(), ["schemas.example.com"], "asked once per host, not once per file");
        // SchemaStore is the built-in source and is never asked about.
        assert!(store.json("https://json.schemastore.org/package.json").is_ok());
        assert!(store.gate().take_pending().is_empty());
        // Local hosts do not ask in `remote` mode.
        assert!(store.json("http://127.0.0.1:9/s.json").is_ok());
        // Approval lets the download through.
        store.gate().approve("schemas.example.com");
        assert!(store.json(url).is_ok());
        // A refusal sticks and costs no request.
        let before = calls.load(Ordering::Relaxed);
        store.gate().refuse("other.example.com");
        let e = store.json("https://other.example.com/x.json").unwrap_err();
        assert!(e.contains("declined"), "{e}");
        assert_eq!(calls.load(Ordering::Relaxed), before);
        assert!(store.gate().take_pending().is_empty());
        // `never` restores the old behaviour; `always` asks for local hosts too; unreadable hosts are refused outright.
        store.gate().set_mode(Confirm::Never);
        assert!(store.json("https://brand-new.example.com/x.json").is_ok());
        store.gate().set_mode(Confirm::Always);
        assert!(store.json("http://localhost:9/other.json").is_err());
        assert!(store.json("https://a.example\\@b.example/x.json").unwrap_err().contains("unambiguously"));
    }

    #[test]
    fn hostile_file_match_patterns_finish_quickly() {
        let t = std::time::Instant::now();
        assert!(!glob_match("*a*a*a*a*a*a*a*a*a*a*a*a*a*a*b", &"a".repeat(80)));
        assert!(!glob_match("**/a*a*a*a*a*a*a*a*a*a*a*a*b.json", &format!("{}/{}", "d".repeat(40), "a".repeat(80))));
        assert!(!glob_match("**/**/**/**/**/**/**/**/x.json", &vec!["d"; 80].join("/")));
        assert!(glob_match("**/**/**/**/d", &vec!["d"; 80].join("/")));
        assert!(t.elapsed().as_millis() < 1500, "took {:?}", t.elapsed());
    }

    #[test]
    fn a_corrupt_download_is_evicted_so_the_next_try_refetches() {
        let dir = tmp("evict");
        let calls = Arc::new(AtomicUsize::new(0));
        let bad = Store::with_fetch(Box::new(Fake(calls.clone(), Ok("<html>502 Bad Gateway".into()))), Some(dir.clone()), true);
        assert!(bad.json_with("https://example.com/s.json", None).is_err());
        let cached = std::fs::read_dir(&dir).map(|d| d.flatten().count()).unwrap_or(0);
        assert_eq!(cached, 0, "the broken text is not kept for a week");
        let good = Store::with_fetch(Box::new(Fake(calls.clone(), Ok("{\"type\":\"object\"}".into()))), Some(dir.clone()), true);
        assert!(good.json_with("https://example.com/s.json", None).is_ok());
        assert_eq!(calls.load(Ordering::Relaxed), 2, "the second store fetched again instead of trusting the bad cache");
    }

    #[test]
    fn concurrent_downloads_of_one_schema_leave_a_whole_file() {
        let dir = tmp("concurrent");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("x.json");
        let text = "{\"k\":\"".to_string() + &"v".repeat(200_000) + "\"}";
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let (f, t) = (file.clone(), text.clone());
                std::thread::spawn(move || write_cache(&f, &t))
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
        let leftovers = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.path() != file).count();
        assert_eq!(leftovers, 0, "no temp files are left behind");
    }

    struct Fake(Arc<AtomicUsize>, Result<String, String>);
    impl Fetch for Fake {
        fn get(&self, _: &str) -> Result<String, String> {
            self.0.fetch_add(1, Ordering::Relaxed);
            self.1.clone()
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("codeyard-schema-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn globs_follow_schemastore_conventions() {
        assert!(glob_match("package.json", "/home/me/app/package.json"));
        assert!(!glob_match("package.json", "/home/me/app/my-package.json"));
        assert!(glob_match("*.schema.json", "/x/y/user.schema.json"));
        assert!(glob_match(".github/workflows/*.yml", "/repo/.github/workflows/ci.yml"));
        assert!(!glob_match(".github/workflows/*.yml", "/repo/.github/workflows/sub/ci.yml"));
        assert!(glob_match("**/tsconfig.json", "tsconfig.json"));
        assert!(glob_match("**/tsconfig*.json", "/a/b/tsconfig.base.json"));
        assert!(glob_match("/k8s/*.yaml", "/k8s/dep.yaml"));
        assert!(!glob_match("/k8s/*.yaml", "/other/k8s/dep.yaml"));
        assert!(glob_match("docker-compose?.yml", "docker-composeX.yml"));
        assert!(glob_match("**/x/*.json", "C:\\a\\x\\a.json"), "backslash paths are normalised");
    }

    #[test]
    fn catalog_picks_the_most_specific_match() {
        let c = Catalog::parse(
            r#"{"schemas":[
                {"name":"any json","url":"https://x/any","fileMatch":["*.json"]},
                {"name":"npm","url":"https://x/npm","fileMatch":["package.json"]},
                {"name":"no match list","url":"https://x/none"}]}"#,
        )
        .unwrap();
        assert_eq!(c.entries.len(), 2);
        assert_eq!(c.find("/a/package.json").unwrap().name, "npm");
        assert_eq!(c.find("/a/other.json").unwrap().name, "any json");
        assert!(c.find("/a/readme.md").is_none());
    }

    #[test]
    fn downloads_are_cached_stale_cache_survives_offline_and_off_means_off() {
        let dir = tmp("cache");
        let calls = Arc::new(AtomicUsize::new(0));
        let store = Store::with_fetch(Box::new(Fake(calls.clone(), Ok("{\"type\":\"object\"}".into()))), Some(dir.clone()), true);
        assert_eq!(store.json("https://x/s.json").unwrap()["type"], "object");
        // Memory hit, then a fresh store reads the disk cache: one download in total.
        assert_eq!(store.json("https://x/s.json#/definitions/a").unwrap()["type"], "object");
        let again = Store::with_fetch(Box::new(Fake(calls.clone(), Err("offline".into()))), Some(dir.clone()), true);
        assert!(again.json("https://x/s.json").is_ok());
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        // Stale copy + network failure: the stale copy wins.
        let file = again.cache_file("https://x/s.json").unwrap();
        let old = SystemTime::now() - Duration::from_secs(30 * 24 * 3600);
        std::fs::File::options().write(true).open(&file).unwrap().set_modified(old).unwrap();
        let offline = Store::with_fetch(Box::new(Fake(calls.clone(), Err("offline".into()))), Some(dir.clone()), true);
        assert!(offline.text("https://x/s.json").is_ok());
        // Uncached + network failure: an error with the reason. Downloads off: no network attempt at all.
        assert_eq!(offline.text("https://x/other.json").unwrap_err(), "offline");
        let before = calls.load(Ordering::Relaxed);
        offline.set_download(false);
        assert!(offline.text("https://x/third.json").unwrap_err().contains("downloads are off"));
        assert_eq!(calls.load(Ordering::Relaxed), before);
        offline.clear();
        assert!(offline.cache_file("https://x/s.json").is_some_and(|f| !f.exists()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn local_schema_files_are_read_directly() {
        let dir = tmp("local");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("s.json");
        std::fs::write(&f, "{ // comment\n \"type\": \"string\", }").unwrap();
        let store = Store::new(None, false);
        assert_eq!(store.json(f.to_str().unwrap()).unwrap()["type"], "string");
        assert_eq!(store.json(&format!("file://{}", f.display())).unwrap()["type"], "string");
        assert!(store.json("/definitely/missing.json").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
