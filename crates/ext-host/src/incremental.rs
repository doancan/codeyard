//! Cranelift's incremental cache in one file, for `--compile --cache FILE`: a rebuilt extension recompiles only the
//! functions that changed. Every TypeScript component embeds the same JavaScript engine, so a reload in `cyx dev` or a
//! second `cyx test` compiles in a fraction of the time (docs/extensions-spike.md, decision 7). Install-time compiles
//! do not use it. The file keeps only the functions of the last compile, so it does not grow.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use wasmtime::CacheStore;

/// Entries as `[u32 key length][key][u32 value length][value]`, little-endian.
#[derive(Debug, Default)]
pub struct FileCache {
    path: PathBuf,
    entries: Mutex<HashMap<Vec<u8>, Vec<u8>>>,
    used: Mutex<HashSet<Vec<u8>>>,
}

impl FileCache {
    /// The cache in `path`; empty when it does not exist or cannot be read.
    pub fn open(path: &Path) -> FileCache {
        let entries = std::fs::read(path).ok().and_then(|b| parse(&b)).unwrap_or_default();
        FileCache { path: path.to_path_buf(), entries: Mutex::new(entries), used: Mutex::default() }
    }

    /// Write the entries this compile used or added. Best effort: a cache that cannot be written only costs time.
    pub fn save(&self) {
        let (Ok(entries), Ok(used)) = (self.entries.lock(), self.used.lock()) else { return };
        let mut out = Vec::new();
        for (k, v) in entries.iter().filter(|(k, _)| used.contains(*k)) {
            for part in [k, v] {
                out.extend_from_slice(&(part.len() as u32).to_le_bytes());
                out.extend_from_slice(part);
            }
        }
        let tmp = self.path.with_extension(format!("tmp-{}", std::process::id()));
        if std::fs::write(&tmp, out).is_ok() && std::fs::rename(&tmp, &self.path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

fn parse(mut b: &[u8]) -> Option<HashMap<Vec<u8>, Vec<u8>>> {
    let mut map = HashMap::new();
    let take = |b: &mut &[u8]| -> Option<Vec<u8>> {
        let len = u32::from_le_bytes(b.get(..4)?.try_into().ok()?) as usize;
        let part = b.get(4..4 + len)?.to_vec();
        *b = &b[4 + len..];
        Some(part)
    };
    while !b.is_empty() {
        let k = take(&mut b)?;
        let v = take(&mut b)?;
        map.insert(k, v);
    }
    Some(map)
}

impl CacheStore for FileCache {
    fn get(&self, key: &[u8]) -> Option<std::borrow::Cow<'_, [u8]>> {
        let v = self.entries.lock().ok()?.get(key).cloned()?;
        self.used.lock().ok()?.insert(key.to_vec());
        Some(std::borrow::Cow::Owned(v))
    }

    fn insert(&self, key: &[u8], value: Vec<u8>) -> bool {
        let (Ok(mut entries), Ok(mut used)) = (self.entries.lock(), self.used.lock()) else { return false };
        entries.insert(key.to_vec(), value);
        used.insert(key.to_vec());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_what_the_last_compile_used() {
        let path = std::env::temp_dir().join(format!("exthost-incremental-{}", std::process::id()));
        let c = FileCache::open(&path);
        assert!(c.insert(b"a", b"1".to_vec()) && c.insert(b"b", b"2".to_vec()));
        c.save();
        let c = FileCache::open(&path);
        assert_eq!(c.get(b"a").as_deref(), Some(&b"1"[..]));
        c.save();
        // `b` was not used by the second compile.
        let c = FileCache::open(&path);
        assert!(c.get(b"b").is_none() && c.get(b"a").is_some());
        assert!(parse(&[1, 0, 0]).is_none());
        let _ = std::fs::remove_file(&path);
    }
}
