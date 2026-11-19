//! `storage`: a key-value store in the extension's data folder, answered by the exthost itself. One file per key
//! (named by the key's hex), written atomically; the whole store stays under `limits::DATA_DIR_BYTES`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use ext_manifest::limits;

pub struct Storage {
    dir: PathBuf,
    sizes: BTreeMap<String, u64>,
}

/// File names are limited to 255 bytes and a key's hex doubles its length.
const KEY_BYTES: usize = 120;

impl Storage {
    pub fn open(data_dir: &str) -> Storage {
        let dir = PathBuf::from(data_dir).join("storage");
        let mut sizes = BTreeMap::new();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if let (Some(key), Ok(meta)) = (unhex(&name), e.metadata()) {
                    sizes.insert(key, meta.len());
                }
            }
        }
        Storage { dir, sizes }
    }

    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        self.sizes.contains_key(key).then(|| std::fs::read(self.dir.join(hex(key))).ok()).flatten()
    }

    pub fn set(&mut self, key: &str, value: &[u8]) -> Result<(), String> {
        if key.is_empty() || key.len() > KEY_BYTES {
            return Err(format!("a storage key is 1 to {KEY_BYTES} bytes"));
        }
        let total: u64 = self.sizes.iter().filter(|(k, _)| k.as_str() != key).map(|(_, s)| s).sum();
        if total + value.len() as u64 > limits::DATA_DIR_BYTES {
            return Err(format!("storage is limited to {} MiB", limits::DATA_DIR_BYTES >> 20));
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let path = self.dir.join(hex(key));
        let tmp = self.dir.join(format!(".{}.tmp", hex(key)));
        std::fs::write(&tmp, value).and_then(|()| std::fs::rename(&tmp, &path)).map_err(|e| e.to_string())?;
        self.sizes.insert(key.to_string(), value.len() as u64);
        Ok(())
    }

    pub fn delete(&mut self, key: &str) {
        if self.sizes.remove(key).is_some() {
            let _ = std::fs::remove_file(self.dir.join(hex(key)));
        }
    }

    pub fn keys(&self) -> Vec<String> {
        self.sizes.keys().cloned().collect()
    }
}

fn hex(key: &str) -> String {
    key.bytes().map(|b| format!("{b:02x}")).collect()
}

fn unhex(name: &str) -> Option<String> {
    if !name.len().is_multiple_of(2) {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..name.len()).step_by(2).map(|i| u8::from_str_radix(name.get(i..i + 2)?, 16).ok()).collect();
    String::from_utf8(bytes?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data_dir(name: &str) -> String {
        let d = std::env::temp_dir().join(format!("exthost-storage-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.to_string_lossy().into_owned()
    }

    #[test]
    fn a_value_is_kept_across_a_reopen_and_deleted_for_good() {
        let d = data_dir("persist");
        let mut s = Storage::open(&d);
        s.set("ünï/key", "değer ✓".as_bytes()).unwrap();
        s.set("other", b"2").unwrap();
        let s = Storage::open(&d);
        assert_eq!(s.get("ünï/key").as_deref(), Some("değer ✓".as_bytes()));
        assert_eq!(s.keys(), vec!["other".to_string(), "ünï/key".to_string()]);
        let mut s = s;
        s.delete("other");
        assert_eq!(Storage::open(&d).keys(), vec!["ünï/key".to_string()]);
        assert_eq!(Storage::open(&d).get("other"), None);
    }

    /// The limit is on the sum of the values: exactly 50 MiB fits, one byte more does not, and replacing a key counts only its new
    /// value (a store full of one key can still be rewritten).
    #[test]
    fn the_store_stays_under_its_limit() {
        let d = data_dir("cap");
        let mut s = Storage::open(&d);
        let limit = limits::DATA_DIR_BYTES as usize;
        s.set("big", &vec![1; limit - 10]).unwrap();
        s.set("small", &[2; 10]).unwrap();
        let refused = s.set("one-more", b"x").unwrap_err();
        assert_eq!(refused, "storage is limited to 50 MiB");
        assert_eq!(s.get("one-more"), None, "a refused value is not stored");
        // Rewriting a key is measured without its old value.
        s.set("big", &vec![3; limit - 10]).unwrap();
        assert!(s.set("big", &vec![3; limit - 9]).is_err(), "one byte over, with the other key's 10 bytes");
        // A refused rewrite leaves the old value.
        assert_eq!(s.get("big").map(|v| (v.len(), v[0])), Some((limit - 10, 3)));
        // The sum is recomputed from the disk when the store is opened again, so the limit holds across restarts.
        let mut s = Storage::open(&d);
        assert!(s.set("one-more", b"x").is_err());
        s.delete("small");
        s.set("one-more", b"x").unwrap();
    }

    #[test]
    fn keys_are_bounded_and_stray_files_are_not_keys() {
        let d = data_dir("keys");
        let mut s = Storage::open(&d);
        assert!(s.set("", b"x").is_err(), "an empty key");
        assert!(s.set(&"k".repeat(KEY_BYTES + 1), b"x").is_err(), "a key too long for a file name");
        s.set(&"k".repeat(KEY_BYTES), b"x").unwrap();
        // A temporary file left by a crash, and a file that is not hex, are not keys.
        let dir = std::path::Path::new(&d).join("storage");
        std::fs::write(dir.join(".6b.tmp"), b"half").unwrap();
        std::fs::write(dir.join("not hex"), b"x").unwrap();
        assert_eq!(Storage::open(&d).keys(), vec!["k".repeat(KEY_BYTES)]);
    }
}
