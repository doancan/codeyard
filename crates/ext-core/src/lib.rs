//! Installed extensions: the content-addressed store, the lock file, install / uninstall / rollback / disable as
//! transactions, and the contribution index the app reads at startup. No UI and no network.
//!
//! Layout (docs/plugin-architecture.md §11):
//!
//! ```text
//! <config>/extensions.lock.jsonc      the installed set (lock.rs): the only record of what is installed
//! <data>/extensions/store/<digest>/   unpacked packages, read-only, named by content digest
//! <data>/extensions/data/<id>/        each extension's own storage
//! <data>/extensions/index.json        derived from the lock and the store; rebuilt when the lock changes
//! <data>/extensions/.lock             held while a transaction runs, so two windows cannot interleave
//! ```
//!
//! Every change follows one order: verify the package, unpack it into a temporary folder, rename that into the store,
//! write the lock file atomically, then the index. A crash before the lock is written leaves the old set and at most a
//! temporary folder (removed on the next transaction); after it, the new set and an index that the next `load`
//! rebuilds. The lock is never ahead of the store.

mod der;
pub mod lock;
pub mod market;
pub mod publisher;
pub mod registry;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod trust;
pub mod yard;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use ext_manifest::{ExtId, Manifest};
use ext_pkg::ContentDigest;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

pub use lock::{Lock, LockError, Locked, Source};
pub use trust::{Effective, Trust, effective};

/// The index format; a different one is rebuilt, never read.
const INDEX_FORMAT: u32 = 1;

#[derive(Debug)]
pub enum Error {
    Package(ext_pkg::Error),
    Lock(LockError),
    NotInstalled(ExtId),
    /// Rollback asked for, but there is no previous version in the store.
    NoPrevious(ExtId),
    Io(String, std::io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Package(e) => write!(f, "{e}"),
            Error::Lock(e) => write!(f, "{e}"),
            Error::NotInstalled(id) => write!(f, "{id} is not installed"),
            Error::NoPrevious(id) => write!(f, "{id} has no previous version to go back to"),
            Error::Io(what, e) => write!(f, "{what}: {e}"),
        }
    }
}

impl std::error::Error for Error {}

fn io<T>(what: impl FnOnce() -> String, r: std::io::Result<T>) -> Result<T, Error> {
    r.map_err(|e| Error::Io(what(), e))
}

/// One installed extension as the app sees it at startup.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Installed {
    pub manifest: Manifest,
    #[serde(with = "hex_digest")]
    pub digest: ContentDigest,
    pub disabled: bool,
    pub source: Source,
}

/// What `load` returns: the installed extensions in id order, and anything that could not be loaded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Loaded {
    pub extensions: Vec<Installed>,
    /// One line per extension the lock names but the store cannot serve (deleted or damaged); it is skipped.
    pub problems: Vec<String>,
    /// The index was stale and rebuilt from the store (each manifest parsed); false on a normal start.
    pub rebuilt: bool,
}

#[derive(Serialize, Deserialize)]
struct IndexFile {
    format: u32,
    /// SHA-256 of the lock file's bytes the index was built from.
    lock_sha256: String,
    extensions: Vec<Installed>,
    problems: Vec<String>,
}

/// Steps a test can make fail, to check that a crash at any point leaves a consistent state.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Unpack,
    Rename,
    Lock,
    Index,
}

pub struct Store {
    /// `<data>/extensions`
    root: PathBuf,
    lock_path: PathBuf,
    #[cfg(test)]
    pub(crate) fail_at: std::cell::Cell<Option<Step>>,
}

/// Held for the length of a transaction; dropping it releases the file lock.
struct Guard(#[allow(dead_code)] File);

impl Store {
    /// `root` is `<data>/extensions`; `lock_path` is `<config>/extensions.lock.jsonc`.
    pub fn new(root: impl Into<PathBuf>, lock_path: impl Into<PathBuf>) -> Store {
        Store {
            root: root.into(),
            lock_path: lock_path.into(),
            #[cfg(test)]
            fail_at: std::cell::Cell::new(None),
        }
    }

    pub fn store_dir(&self, digest: &ContentDigest) -> PathBuf {
        self.root.join("store").join(digest.hex())
    }

    /// The extension's own storage folder (`storage` in the API).
    pub fn data_dir(&self, id: &ExtId) -> PathBuf {
        self.root.join("data").join(id.as_str())
    }

    fn index_path(&self) -> PathBuf {
        self.root.join("index.json")
    }

    #[cfg(test)]
    fn fail(&self, step: Step) -> Result<(), Error> {
        if self.fail_at.get() == Some(step) {
            return Err(Error::Io(format!("simulated crash at {step:?}"), std::io::Error::other("injected")));
        }
        Ok(())
    }

    fn guard(&self) -> Result<Guard, Error> {
        io(|| format!("cannot create {}", self.root.display()), std::fs::create_dir_all(&self.root))?;
        let path = self.root.join(".lock");
        let f = io(|| format!("cannot open {}", path.display()), File::options().create(true).truncate(false).write(true).open(&path))?;
        io(|| "cannot lock the extension store".into(), f.lock())?;
        Ok(Guard(f))
    }

    /// The lock file; a missing file is an empty set.
    pub fn read_lock(&self) -> Result<(Lock, Vec<u8>), Error> {
        match std::fs::read(&self.lock_path) {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                Ok((Lock::parse(&text).map_err(Error::Lock)?, bytes))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((Lock { format: lock::FORMAT, ..Default::default() }, Vec::new())),
            Err(e) => Err(Error::Io(format!("cannot read {}", self.lock_path.display()), e)),
        }
    }

    /// The installed extensions, for startup. Reads one small file when the index is current; parses the manifests only
    /// when the lock changed since the index was written (another window installed, or the user synced the lock).
    pub fn load(&self) -> Result<Loaded, Error> {
        let (lock, bytes) = self.read_lock()?;
        let lock_sha = hex(&Sha256::digest(&bytes));
        if let Ok(text) = std::fs::read(self.index_path())
            && let Ok(index) = serde_json::from_slice::<IndexFile>(&text)
            && index.format == INDEX_FORMAT
            && index.lock_sha256 == lock_sha
        {
            return Ok(Loaded { extensions: index.extensions, problems: index.problems, rebuilt: false });
        }
        let loaded = self.build_index(&lock);
        // Best effort: if it cannot be written, the next start rebuilds it again.
        let _ = self.write_index(&lock_sha, &loaded);
        Ok(Loaded { rebuilt: true, ..loaded })
    }

    fn build_index(&self, lock: &Lock) -> Loaded {
        let mut out = Loaded::default();
        for (id, l) in &lock.extensions {
            let path = self.store_dir(&l.digest).join(ext_manifest::FILE_NAME);
            let manifest = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|t| ext_manifest::parse(&t).map_err(|d| d.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ")));
            match manifest {
                Ok(m) if &m.id == id && m.version == l.version => {
                    out.extensions.push(Installed { manifest: m, digest: l.digest, disabled: l.disabled, source: l.source.clone() })
                }
                Ok(m) => out.problems.push(format!("{id}: the store holds {} {} instead; reinstall it", m.id, m.version)),
                Err(e) => out.problems.push(format!("{id}: the installed files are missing or damaged ({e}); reinstall it")),
            }
        }
        out
    }

    fn write_index(&self, lock_sha: &str, loaded: &Loaded) -> Result<(), Error> {
        let file =
            IndexFile { format: INDEX_FORMAT, lock_sha256: lock_sha.to_string(), extensions: loaded.extensions.clone(), problems: loaded.problems.clone() };
        let data = serde_json::to_vec(&file).expect("index serializes");
        write_atomic(&self.index_path(), &data)
    }

    /// Install a package file, replacing any installed version of the same extension (kept as `previous`).
    pub fn install(&self, bytes: &[u8], source: Source) -> Result<Installed, Error> {
        let pkg = ext_pkg::read(bytes).map_err(Error::Package)?;
        self.commit_package(pkg, source)
    }

    /// Package a folder and install it (`codeyard ext install ./folder`).
    pub fn install_dir(&self, dir: &Path) -> Result<Installed, Error> {
        let files = ext_pkg::collect_dir(dir).map_err(Error::Package)?;
        let (bytes, _) = ext_pkg::write(files).map_err(Error::Package)?;
        self.install(&bytes, Source::Folder { path: dir.display().to_string() })
    }

    fn commit_package(&self, pkg: ext_pkg::Package, source: Source) -> Result<Installed, Error> {
        let _guard = self.guard()?;
        self.sweep_temporaries();
        let (mut lock, _) = self.read_lock()?;
        let dir = self.store_dir(&pkg.digest);
        if !dir.join(ext_manifest::FILE_NAME).is_file() {
            self.unpack(&pkg, &dir)?;
        }
        let id = pkg.manifest.id.clone();
        let old = lock.extensions.get(&id);
        let previous = match old {
            Some(o) if o.digest != pkg.digest => Some(o.digest),
            Some(o) => o.previous,
            None => None,
        };
        let entry =
            Locked { version: pkg.manifest.version.clone(), digest: pkg.digest, previous, source: source.clone(), disabled: old.is_some_and(|o| o.disabled) };
        lock.extensions.insert(id.clone(), entry.clone());
        self.commit_lock(&lock)?;
        let _ = std::fs::create_dir_all(self.data_dir(&id));
        Ok(Installed { manifest: pkg.manifest, digest: pkg.digest, disabled: entry.disabled, source })
    }

    /// Write the files into `store/.tmp-<digest>-<pid>`, make them read-only and rename the folder into place.
    fn unpack(&self, pkg: &ext_pkg::Package, dir: &Path) -> Result<(), Error> {
        let store = self.root.join("store");
        io(|| format!("cannot create {}", store.display()), std::fs::create_dir_all(&store))?;
        let tmp = store.join(format!(".tmp-{}-{}", pkg.digest.hex(), std::process::id()));
        let _ = remove_tree(&tmp);
        let result = (|| {
            for e in pkg.files.values() {
                let path = tmp.join(&e.path);
                if let Some(parent) = path.parent() {
                    io(|| format!("cannot create {}", parent.display()), std::fs::create_dir_all(parent))?;
                }
                let mut f = io(|| format!("cannot write {}", path.display()), File::options().write(true).create_new(true).open(&path))?;
                io(|| format!("cannot write {}", path.display()), f.write_all(&e.data))?;
                io(|| format!("cannot write {}", path.display()), f.sync_all())?;
                drop(f);
                set_read_only(&path, e.executable);
            }
            #[cfg(test)]
            self.fail(Step::Unpack)?;
            #[cfg(test)]
            self.fail(Step::Rename)?;
            if dir.exists() {
                // A damaged copy without its manifest: replace it.
                remove_tree(dir).map_err(|e| Error::Io(format!("cannot replace {}", dir.display()), e))?;
            }
            io(|| format!("cannot move the package into {}", dir.display()), std::fs::rename(&tmp, dir))
        })();
        if result.is_err() {
            let _ = remove_tree(&tmp);
        }
        result
    }

    /// Write the lock, then the index, then drop store folders nothing refers to.
    fn commit_lock(&self, lock: &Lock) -> Result<(), Error> {
        #[cfg(test)]
        self.fail(Step::Lock)?;
        let text = lock.render();
        if let Some(parent) = self.lock_path.parent() {
            io(|| format!("cannot create {}", parent.display()), std::fs::create_dir_all(parent))?;
        }
        write_atomic(&self.lock_path, text.as_bytes())?;
        #[cfg(test)]
        if self.fail_at.get() == Some(Step::Index) {
            return Ok(());
        }
        let loaded = self.build_index(lock);
        let _ = self.write_index(&hex(&Sha256::digest(text.as_bytes())), &loaded);
        self.collect_garbage(lock);
        Ok(())
    }

    /// Remove an extension, its store folders and its data.
    pub fn uninstall(&self, id: &ExtId) -> Result<(), Error> {
        let _guard = self.guard()?;
        let (mut lock, _) = self.read_lock()?;
        lock.extensions.remove(id).ok_or_else(|| Error::NotInstalled(id.clone()))?;
        self.commit_lock(&lock)?;
        let _ = remove_tree(&self.data_dir(id));
        Ok(())
    }

    /// Go back to the version this one replaced. No network: the previous package is still in the store.
    pub fn rollback(&self, id: &ExtId) -> Result<Installed, Error> {
        let _guard = self.guard()?;
        let (mut lock, _) = self.read_lock()?;
        let cur = lock.extensions.get(id).cloned().ok_or_else(|| Error::NotInstalled(id.clone()))?;
        let prev = cur.previous.ok_or_else(|| Error::NoPrevious(id.clone()))?;
        let text = std::fs::read_to_string(self.store_dir(&prev).join(ext_manifest::FILE_NAME)).map_err(|_| Error::NoPrevious(id.clone()))?;
        let m = ext_manifest::parse(&text).map_err(|_| Error::NoPrevious(id.clone()))?;
        let entry = Locked { version: m.version.clone(), digest: prev, previous: Some(cur.digest), source: cur.source.clone(), disabled: cur.disabled };
        lock.extensions.insert(id.clone(), entry);
        self.commit_lock(&lock)?;
        Ok(Installed { manifest: m, digest: prev, disabled: cur.disabled, source: cur.source })
    }

    /// Turn an extension off or on without removing it.
    pub fn set_disabled(&self, id: &ExtId, disabled: bool) -> Result<(), Error> {
        let _guard = self.guard()?;
        let (mut lock, _) = self.read_lock()?;
        let entry = lock.extensions.get_mut(id).ok_or_else(|| Error::NotInstalled(id.clone()))?;
        if entry.disabled == disabled {
            return Ok(());
        }
        entry.disabled = disabled;
        self.commit_lock(&lock)
    }

    /// Check an installed extension's files against its `CONTENTS.sha256` (the Extensions view's **Verify**, and before a
    /// repair). Returns the paths that are missing or changed.
    pub fn verify(&self, digest: &ContentDigest) -> Result<Vec<String>, Error> {
        let dir = self.store_dir(digest);
        let list = io(|| format!("cannot read {}", dir.display()), std::fs::read_to_string(dir.join(ext_pkg::CONTENTS)))?;
        let mut bad = Vec::new();
        for line in list.lines() {
            let Some((hash, path)) = line.split_once("  ") else {
                bad.push(line.to_string());
                continue;
            };
            match std::fs::read(dir.join(path)) {
                Ok(data) if hex(&Sha256::digest(&data)) == hash => {}
                _ => bad.push(path.to_string()),
            }
        }
        Ok(bad)
    }

    /// Store folders that neither a current nor a previous version refers to, and data folders of extensions that are
    /// no longer installed, are removed. Errors are ignored: the next transaction tries again.
    fn collect_garbage(&self, lock: &Lock) {
        let keep: BTreeSet<String> = lock.digests().map(|d| d.hex()).collect();
        if let Ok(entries) = std::fs::read_dir(self.root.join("store")) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if !name.starts_with('.') && !keep.contains(&name) {
                    let _ = remove_tree(&e.path());
                }
            }
        }
        let ids: BTreeSet<&str> = lock.extensions.keys().map(ExtId::as_str).collect();
        if let Ok(entries) = std::fs::read_dir(self.root.join("data")) {
            for e in entries.flatten() {
                if !ids.contains(e.file_name().to_string_lossy().as_ref()) {
                    let _ = remove_tree(&e.path());
                }
            }
        }
    }

    /// Temporary folders left by a transaction that died (only ever written under the store lock, so any found while
    /// holding it are dead).
    fn sweep_temporaries(&self) {
        if let Ok(entries) = std::fs::read_dir(self.root.join("store")) {
            for e in entries.flatten() {
                if e.file_name().to_string_lossy().starts_with(".tmp-") {
                    let _ = remove_tree(&e.path());
                }
            }
        }
    }

    /// Every installed extension's lock entry, by id.
    pub fn locked(&self) -> Result<BTreeMap<ExtId, Locked>, Error> {
        Ok(self.read_lock()?.0.extensions)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Read-only files (and executable where the package says so): an extension's own code cannot change what was verified.
fn set_read_only(path: &Path, executable: bool) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(if executable { 0o555 } else { 0o444 }));
    }
    #[cfg(not(unix))]
    {
        let _ = executable;
        if let Ok(m) = std::fs::metadata(path) {
            let mut p = m.permissions();
            p.set_readonly(true);
            let _ = std::fs::set_permissions(path, p);
        }
    }
}

/// `remove_dir_all` that first makes read-only files writable again (Windows refuses to delete them).
fn remove_tree(path: &Path) -> std::io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    #[cfg(not(unix))]
    for e in walk(path) {
        if let Ok(m) = std::fs::metadata(&e) {
            let mut p = m.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            p.set_readonly(false);
            let _ = std::fs::set_permissions(&e, p);
        }
    }
    std::fs::remove_dir_all(path)
}

#[cfg(not(unix))]
fn walk(path: &Path) -> Vec<PathBuf> {
    let mut out = vec![path.to_path_buf()];
    if let Ok(entries) = std::fs::read_dir(path) {
        for e in entries.flatten() {
            out.extend(walk(&e.path()));
        }
    }
    out
}

/// A temp file in the same folder, `fsync`, then `rename`: a crash leaves the old file or the new one, never half. A
/// symlinked target (a lock file kept in a dotfiles repository) stays a link; the file it points to is replaced.
fn write_atomic(path: &Path, data: &[u8]) -> Result<(), Error> {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let path = target.as_path();
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut f = File::options().write(true).create(true).truncate(true).open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    io(|| format!("cannot write {}", path.display()), result)
}

mod hex_digest {
    use ext_pkg::ContentDigest;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(d: &ContentDigest, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&d.hex())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<ContentDigest, D::Error> {
        let s = String::deserialize(d)?;
        ContentDigest::parse(&s).ok_or_else(|| serde::de::Error::custom("not a digest"))
    }
}
