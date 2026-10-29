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
