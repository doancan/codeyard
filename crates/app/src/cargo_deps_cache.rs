//! The on-disk cache of dependency lookups: one small JSON file per crate and kind, written atomically, with the time it was
//! fetched and the `ETag` the server gave (so an old entry is revalidated with a conditional request, not downloaded again).

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

/// Entries older than this are deleted when the cache opens, so it cannot grow without bound.
pub const MAX_AGE: Duration = Duration::from_secs(60 * 24 * 3600);

#[derive(Clone, Debug, PartialEq)]
pub struct Stored {
    pub etag: Option<String>,
    /// Seconds since the Unix epoch.
    pub fetched: u64,
    pub payload: Value,
}

#[derive(Clone, Debug)]
pub struct DiskCache {
    dir: Option<PathBuf>,
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl DiskCache {
    /// `None`: nothing is read or written (tests, or no cache folder on this machine).
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self { dir }
    }

    fn file(&self, kind: &str, name: &str) -> Option<PathBuf> {
        // Names were checked by `valid_crate_name`; this is the last line of defence against a path escape.
        let safe = !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        safe.then(|| self.dir.as_ref().map(|d| d.join(kind).join(format!("{}.json", name.to_ascii_lowercase())))).flatten()
    }

    pub fn read(&self, kind: &str, name: &str) -> Option<Stored> {
        let text = std::fs::read_to_string(self.file(kind, name)?).ok()?;
        let v: Value = serde_json::from_str(&text).ok()?;
        Some(Stored { etag: v["etag"].as_str().map(str::to_string), fetched: v["fetched"].as_u64()?, payload: v["payload"].clone() })
    }

    /// Best effort: a cache that cannot be written only costs a request next time.
    pub fn write(&self, kind: &str, name: &str, stored: &Stored) {
        let Some(path) = self.file(kind, name) else { return };
        let Some(parent) = path.parent() else { return };
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
        let body = json!({ "etag": stored.etag, "fetched": stored.fetched, "payload": stored.payload }).to_string();
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        if std::fs::write(&tmp, body).and_then(|()| std::fs::rename(&tmp, &path)).is_err() {
            std::fs::remove_file(&tmp).ok();
        }
    }

    /// Delete the entries not fetched for `MAX_AGE`.
    pub fn prune(&self, now: u64) {
        let Some(dir) = &self.dir else { return };
        for kind in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            for file in std::fs::read_dir(kind.path()).into_iter().flatten().flatten() {
                let old = std::fs::read_to_string(file.path())
                    .ok()
                    .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                    .and_then(|v| v["fetched"].as_u64())
                    .is_none_or(|f| now.saturating_sub(f) > MAX_AGE.as_secs());
                if old {
                    std::fs::remove_file(file.path()).ok();
                }
            }
        }
    }
}
