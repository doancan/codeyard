//! Yard, the extension registry's publish service (docs/registry.md, docs/plugin-architecture.md §8).
//!
//! Everything users download is static: `index.json` and its signature, `root.json` and its signatures, and
//! `packages/<digest>.cyx`, written into the public folder a CDN serves. This service exists for publishers: it takes a
//! package with its Sigstore bundle, runs the same checks as `cyx check` and the client, records it, and writes and
//! signs a new index. A publisher proves who they are with the bundle itself (the identity in its Fulcio certificate),
//! so there are no accounts, passwords or tokens. Yank, restore and deprecate are signed statements the same way.
//! The security role (revoke, clear an advisory fix for immediate update) acts from the server's command line.
//!
//! State is one SQLite file (`db.rs`). Every change runs under one lock, so version numbers and the index counter
//! cannot race.

mod db;
pub mod http;
pub mod names;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ext_core::publisher::{self, Pins};
use ext_core::yard::{self, Index, Publisher, Root, SigningKey, Status, StatusChange};
use ext_manifest::{ExtId, Version};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// The public-good Sigstore roots `yard init` pins: Fulcio's CA chain and Rekor v1's key.
pub const SIGSTORE_FULCIO: &str = include_str!("../sigstore/fulcio.pem");
pub const SIGSTORE_REKOR: &str = include_str!("../sigstore/rekor.pem");

/// How old a status statement may be: a captured one cannot be replayed later.
pub const STATEMENT_MAX_AGE_SECS: u64 = 15 * 60;
/// Code waits this long before clients update to it.
pub const CODE_WAIT_SECS: u64 = 48 * 3600;
pub const MAX_UPLOAD_BYTES: usize = ext_manifest::limits::PACKAGE_BYTES as usize + publisher::MAX_BUNDLE_BYTES + 4;

/// `yard.json`. Relative paths are relative to the file.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub database: PathBuf,
    /// The folder the CDN serves.
    pub public: PathBuf,
    /// `root.json`; its signatures are in `root.json.sig` beside it.
    pub root: PathBuf,
    pub online_key: PathBuf,
    #[serde(default = "default_listen")]
    pub listen: String,
}

fn default_listen() -> String {
    "127.0.0.1:8080".into()
}

impl Config {
    pub fn load(path: &Path) -> Result<Config, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let mut cfg: Config = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let base = path.parent().unwrap_or(Path::new("."));
        for p in [&mut cfg.database, &mut cfg.public, &mut cfg.root, &mut cfg.online_key] {
            if p.is_relative() {
                *p = base.join(&*p);
            }
        }
        Ok(cfg)
    }
}

/// Why a request was refused. The text is what `cyx` prints.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// Not a valid upload, package or statement (400).
    Invalid(String),
    /// Signed by someone who may not do this (403).
    Forbidden(String),
    /// Clashes with what is already there: a reused version, a confusable name (409).
    Conflict(String),
    NotFound(String),
    TooLarge(String),
    /// The server's own files or database (500).
    Internal(String),
}

impl Error {
    pub fn status(&self) -> u16 {
        match self {
            Error::Invalid(_) => 400,
            Error::Forbidden(_) => 403,
            Error::NotFound(_) => 404,
            Error::Conflict(_) => 409,
            Error::TooLarge(_) => 413,
            Error::Internal(_) => 500,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Error::Invalid(m) | Error::Forbidden(m) | Error::Conflict(m) | Error::NotFound(m) | Error::TooLarge(m) | Error::Internal(m) => m,
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

fn internal(what: &str) -> impl Fn(rusqlite::Error) -> Error + '_ {
    move |e| Error::Internal(format!("{what}: {e}"))
}

/// What a publish returns: what users will see and when.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Published {
    pub id: ExtId,
    pub version: Version,
    pub digest: String,
    /// Unix seconds from which clients update to it (now for data-only packages).
    pub available: u64,
    pub publisher: Publisher,
}

pub struct Yard {
    cfg: Config,
    root: Root,
    pins: Pins,
    online: SigningKey,
    db: Mutex<rusqlite::Connection>,
}

impl Yard {
    pub fn open(cfg: Config) -> Result<Yard, String> {
        let root_bytes = std::fs::read(&cfg.root).map_err(|e| format!("cannot read {}: {e}", cfg.root.display()))?;
        let root: Root = serde_json::from_slice(&root_bytes).map_err(|e| format!("{}: {e}", cfg.root.display()))?;
        let pins = Pins::from_pem(&root.fulcio, &root.rekor).map_err(|e| e.to_string())?;
        let key_text = std::fs::read_to_string(&cfg.online_key).map_err(|e| format!("cannot read {}: {e}", cfg.online_key.display()))?;
        let online = SigningKey::from_file(&key_text).map_err(|e| format!("{}: {e}", cfg.online_key.display()))?;
        if online.public() != root.online.key {
            return Err(format!("{} is not the online key {} names", cfg.online_key.display(), cfg.root.display()));
        }
        std::fs::create_dir_all(cfg.public.join("packages")).map_err(|e| format!("cannot create {}: {e}", cfg.public.display()))?;
        let db = db::open(&cfg.database).map_err(|e| format!("{}: {e}", cfg.database.display()))?;
        Ok(Yard { cfg, root, pins, online, db: Mutex::new(db) })
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, rusqlite::Connection> {
        self.db.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Take a framed upload (bundle, then package), check it, record it, and sign a new index.
    pub fn publish(&self, body: &[u8], now: u64) -> Result<Published, Error> {
        if body.len() > MAX_UPLOAD_BYTES {
            return Err(Error::TooLarge(format!("the upload is larger than {} MiB", MAX_UPLOAD_BYTES >> 20)));
        }
        let (bundle, bytes) = yard::unframe(body).ok_or_else(|| Error::Invalid("not an upload: a bundle and a package".into()))?;
        // The package rules first: the same code `cyx check` and the app run.
        let pkg = ext_pkg::read(bytes).map_err(|e| Error::Invalid(e.to_string()))?;
        let m = &pkg.manifest;
        if !m.is_declarative() {
            return Err(Error::Invalid(format!("{} {} runs code; the registry accepts data-only extensions until code extensions ship", m.id, m.version)));
        }
        let signed = publisher::verify_package(bundle, &m.id, &m.version, &pkg.digest, &self.pins, now).map_err(|e| Error::Forbidden(e.to_string()))?;
        let who = signed.publisher;

        let mut conn = self.lock();
        let tx = conn.transaction().map_err(internal("database"))?;
        let owner = names::owner_key(&who);
        let prefix = m.id.publisher();
        match db::prefix_owner(&tx, prefix).map_err(internal("database"))? {
            Some(o) if o != owner => return Err(Error::Forbidden(format!("the publisher name “{prefix}” belongs to another publisher; choose another one"))),
            Some(_) => {}
            None => {
                let others = db::prefixes(&tx).map_err(internal("database"))?;
                if let Some(clash) = names::clashing_prefix(prefix, others.iter().filter(|(_, o)| *o != owner).map(|(p, _)| p.as_str())) {
                    return Err(Error::Conflict(format!("the publisher name “{prefix}” is too close to “{clash}”; choose another one")));
                }
                db::add_prefix(&tx, prefix, &owner, now).map_err(internal("database"))?;
            }
        }
        match db::extension_publisher(&tx, &m.id).map_err(internal("database"))? {
            Some(p) if p != who => {
                return Err(Error::Forbidden(format!(
                    "{} is published from {p}; this was signed by {who}. Publish from the same repository and workflow",
                    m.id
                )));
            }
            Some(_) => {}
            None => {
                let others = db::extensions_not_owned_by(&tx, &owner).map_err(internal("database"))?;
                if let Some(clash) = names::clashing_extension(&m.id, &m.name, &others) {
                    return Err(Error::Conflict(format!("{} is too close to {clash}; choose another name", m.id)));
                }
            }
        }
        if db::version_exists(&tx, &m.id, &m.version).map_err(internal("database"))? {
            return Err(Error::Conflict(format!("{} {} was published before; a version number is never reused, so raise it", m.id, m.version)));
        }
        let digest = pkg.digest.hex();
        let release = yard::Release {
            version: m.version.clone(),
            digest: digest.clone(),
            sha256: hex(&Sha256::digest(bytes)),
            size: bytes.len() as u64,
            permissions: yard::permission_digest(m),
            code: !m.is_declarative(),
            published: now,
            status: Status::Ok,
            downloads: 0,
            immediate: false,
        };
        db::upsert_extension(&tx, m, &who).map_err(internal("database"))?;
        db::add_version(&tx, &m.id, &release).map_err(internal("database"))?;
        // The package lands before the index that lists it; a failure after this leaves an unlisted file, never a
        // listed one that is missing.
        write_atomic(&self.cfg.public.join("packages").join(format!("{digest}.cyx")), bytes)?;
        // Clients check the publisher's signature themselves; the registry only passes it on.
        write_atomic(&self.cfg.public.join("packages").join(format!("{digest}.sigstore.json")), bundle)?;
        self.sign_index(&tx, now)?;
        tx.commit().map_err(internal("database"))?;
        let available = if release.code { now + CODE_WAIT_SECS } else { now };
        Ok(Published { id: m.id.clone(), version: m.version.clone(), digest, available, publisher: who })
    }

    /// A publisher's yank, restore or deprecate: a framed bundle over a status statement, signed by the identity that
    /// publishes the extension, within the last 15 minutes.
    pub fn change_status(&self, body: &[u8], now: u64) -> Result<(ExtId, Version, Status), Error> {
        let (bundle, statement) = yard::unframe(body).ok_or_else(|| Error::Invalid("not an upload: a bundle and a statement".into()))?;
        let (id, version, change) = StatusChange::parse(statement).ok_or_else(|| Error::Invalid("not a status statement".into()))?;
        let signed = publisher::verify(bundle, statement, &self.pins, now).map_err(|e| Error::Forbidden(e.to_string()))?;
        if signed.logged + STATEMENT_MAX_AGE_SECS < now {
            return Err(Error::Forbidden("the statement was signed more than 15 minutes ago; sign it again".into()));
        }
        let mut conn = self.lock();
        let tx = conn.transaction().map_err(internal("database"))?;
        match db::extension_publisher(&tx, &id).map_err(internal("database"))? {
            None => return Err(Error::NotFound(format!("{id} is not in the registry"))),
            Some(p) if p != signed.publisher => return Err(Error::Forbidden(format!("{id} is published from {p}; this was signed by {}", signed.publisher))),
            Some(_) => {}
        }
        let current =
            db::status(&tx, &id, &version).map_err(internal("database"))?.ok_or_else(|| Error::NotFound(format!("{id} {version} was never published")))?;
        if matches!(current, Status::Revoked { .. }) {
            return Err(Error::Forbidden(format!("{id} {version} was revoked by the registry; only the registry can change that")));
        }
        let status = match change {
            StatusChange::Yank => Status::Yanked,
            StatusChange::Restore => Status::Ok,
            StatusChange::Deprecate { replacement } => {
                if let Some(r) = &replacement
                    && db::extension_publisher(&tx, r).map_err(internal("database"))?.is_none()
                {
                    return Err(Error::Invalid(format!("the replacement {r} is not in the registry")));
                }
                Status::Deprecated { replacement }
            }
        };
        db::set_status(&tx, &id, &version, &status).map_err(internal("database"))?;
        self.sign_index(&tx, now)?;
        tx.commit().map_err(internal("database"))?;
        Ok((id, version, status))
    }

    /// The security role: revoke a version everywhere (clients stop it and its dependents).
    pub fn revoke(&self, id: &ExtId, version: &Version, reason: &str, now: u64) -> Result<(), Error> {
        if reason.trim().is_empty() || reason.len() > 500 || reason.chars().any(char::is_control) {
            return Err(Error::Invalid("give a reason of at most 500 characters on one line; users see it".into()));
        }
        self.operator_change(id, version, now, |tx| db::set_status(tx, id, version, &Status::Revoked { reason: reason.trim().into() }))
    }

    /// The security role: let clients update to this version at once (an advisory fix), skipping the wait for code.
    pub fn clear_for_immediate_update(&self, id: &ExtId, version: &Version, now: u64) -> Result<(), Error> {
        self.operator_change(id, version, now, |tx| db::set_immediate(tx, id, version))
    }

    fn operator_change(
        &self,
        id: &ExtId,
        version: &Version,
        now: u64,
        change: impl FnOnce(&rusqlite::Transaction) -> rusqlite::Result<()>,
    ) -> Result<(), Error> {
        let mut conn = self.lock();
        let tx = conn.transaction().map_err(internal("database"))?;
        db::status(&tx, id, version).map_err(internal("database"))?.ok_or_else(|| Error::NotFound(format!("{id} {version} was never published")))?;
        change(&tx).map_err(internal("database"))?;
        self.sign_index(&tx, now)?;
        tx.commit().map_err(internal("database"))
    }

    /// Sign a new index with nothing changed: run daily so it never expires while the service is up.
    pub fn resign(&self, now: u64) -> Result<u64, Error> {
        let mut conn = self.lock();
        let tx = conn.transaction().map_err(internal("database"))?;
        let version = self.sign_index(&tx, now)?;
        tx.commit().map_err(internal("database"))?;
        Ok(version)
    }

    /// The publisher signature of a package, for clients to check themselves (not counted as a download).
    pub fn bundle(&self, digest: &str) -> Result<PathBuf, Error> {
        if ext_pkg::ContentDigest::parse(digest).is_none() {
            return Err(Error::NotFound("no such package".into()));
        }
        let path = self.cfg.public.join("packages").join(format!("{digest}.sigstore.json"));
        if path.is_file() { Ok(path) } else { Err(Error::NotFound("no such package".into())) }
    }

    /// Count one download of a package by its digest, for the day; the path to serve. Nothing about who asked is kept.
    pub fn download(&self, digest: &str, now: u64) -> Result<PathBuf, Error> {
        if ext_pkg::ContentDigest::parse(digest).is_none() {
            return Err(Error::NotFound("no such package".into()));
        }
        let conn = self.lock();
        if !db::count_download(&conn, digest, now / 86400).map_err(internal("database"))? {
            return Err(Error::NotFound("no such package".into()));
        }
        Ok(self.cfg.public.join("packages").join(format!("{digest}.cyx")))
    }

    /// Write and sign the next index (and copy the root beside it) from the database as `tx` sees it.
    fn sign_index(&self, tx: &rusqlite::Transaction, now: u64) -> Result<u64, Error> {
        let version = db::next_index_version(tx).map_err(internal("database"))?;
        let extensions = db::entries(tx).map_err(internal("database"))?;
        let index = Index {
            format: yard::FORMAT,
            registry: self.root.registry.clone(),
            version,
            expires: (now + yard::INDEX_LIFETIME_SECS).min(self.root.online.expires),
            extensions,
        };
        let bytes = serde_json::to_vec(&index).map_err(|e| Error::Internal(e.to_string()))?;
        let sig = yard::add_signature("", &self.online, yard::Role::Index, &bytes).map_err(Error::Internal)?;
        let public = &self.cfg.public;
        let copy = |from: &Path, name: &str| -> Result<(), Error> {
            let b = std::fs::read(from).map_err(|e| Error::Internal(format!("cannot read {}: {e}", from.display())))?;
            write_atomic(&public.join(name), &b)
        };
        copy(&self.cfg.root, yard::ROOT_FILE)?;
        copy(&sig_path(&self.cfg.root), &format!("{}{}", yard::ROOT_FILE, yard::SIG_SUFFIX))?;
        // A client that reads between these two writes sees a signature mismatch and fetches again.
        write_atomic(&public.join(format!("{}{}", yard::INDEX_FILE, yard::SIG_SUFFIX)), sig.as_bytes())?;
        write_atomic(&public.join(yard::INDEX_FILE), &bytes)?;
        Ok(version)
    }
}

pub fn sig_path(file: &Path) -> PathBuf {
    let mut s = file.as_os_str().to_owned();
    s.push(yard::SIG_SUFFIX);
    PathBuf::from(s)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, path)).map_err(|e| Error::Internal(format!("cannot write {}: {e}", path.display())))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A new registry in `dir` for development or a first deployment: one root key (threshold 1; a production root is
/// signed by its holders with `cyx yard sign`), an online key, `root.json` pinning the given Sigstore roots, and
/// `yard.json`. Refuses a folder that already has a config.
pub fn init(dir: &Path, registry: &str, fulcio: &str, rekor: &str, now: u64) -> Result<PathBuf, String> {
    let cfg_path = dir.join("yard.json");
    if cfg_path.exists() {
        return Err(format!("{} exists; this folder already has a registry", cfg_path.display()));
    }
    Pins::from_pem(fulcio, rekor).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir.join("keys")).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let (root_key, root_text) = SigningKey::generate()?;
    let (online_key, online_text) = SigningKey::generate()?;
    write_private(&dir.join("keys/root.key"), &root_text)?;
    write_private(&dir.join("keys/online.key"), &online_text)?;
    let root = Root {
        format: yard::FORMAT,
        version: 1,
        registry: registry.into(),
        online: yard::OnlineKey { key: online_key.public(), expires: now + 90 * 86400 },
        fulcio: fulcio.into(),
        rekor: rekor.into(),
    };
    let root_bytes = serde_json::to_vec_pretty(&root).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("root.json"), &root_bytes).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("root.json.sig"), yard::add_signature("", &root_key, yard::Role::Root, &root_bytes)?).map_err(|e| e.to_string())?;
    let cfg = Config {
        database: "yard.sqlite".into(),
        public: "public".into(),
        root: "root.json".into(),
        online_key: "keys/online.key".into(),
        listen: default_listen(),
    };
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())? + "\n").map_err(|e| e.to_string())?;
    std::fs::write(dir.join("root-key.txt"), format!("{}\n", root_key.public().hex())).map_err(|e| e.to_string())?;
    Ok(cfg_path)
}

fn write_private(path: &Path, text: &str) -> Result<(), String> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut f = opts.open(path).map_err(|e| format!("cannot create {}: {e}", path.display()))?;
    std::io::Write::write_all(&mut f, text.as_bytes()).map_err(|e| format!("cannot write {}: {e}", path.display()))
}
