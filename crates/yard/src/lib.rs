//! Yard, the extension registry's publish service (docs/registry.md, docs/plugin-architecture.md §8).
//!
//! Everything users download is static: `index.json` and its signature, `root.json` and its signatures, and
//! `packages/<digest>.cyx` with `<digest>.details.json` (its manifest and README, for the Market), written into the public folder a CDN serves. This service exists for publishers: it takes a
//! package with its Sigstore bundle, runs the same checks as `cyx check` and the client, records it, and writes and
//! signs a new index. A publisher proves who they are with the bundle itself (the identity in its Fulcio certificate),
//! so there are no accounts, passwords or tokens. Yank, restore and deprecate are signed statements the same way.
//! The security role (revoke, clear an advisory fix for immediate update) acts from the server's command line.
//!
//! State is one SQLite file (`db.rs`). Every change runs under one lock, so version numbers and the index counter
//! cannot race.
//!
//! The online key that signs the index lives on the server and is good for 90 days. [`rotate`] prepares the next one
//! and a `root.next.json` naming it; the root key holders sign that file offline (`cyx yard sign root`), and
//! [`promote`] checks their signatures against the root keys in `yard.json` before it goes live.

mod db;
pub mod http;
pub mod names;
mod review;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ext_core::publisher::{self, Pins};
use ext_core::yard::{self, Index, PublicKey, Publisher, Root, RootKeys, SigningKey, Status, StatusChange};
use ext_manifest::{ExtId, Version};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// The public-good Sigstore roots `yard init` pins: Fulcio's CA chain and Rekor v1's key.
pub const SIGSTORE_FULCIO: &str = include_str!("../sigstore/fulcio.pem");
pub const SIGSTORE_REKOR: &str = include_str!("../sigstore/rekor.pem");

/// How long an online key is good for: the root key holders sign the next one before then.
pub const ONLINE_KEY_LIFETIME_SECS: u64 = 90 * 86400;
/// `yard serve` warns this long before the online key expires.
pub const ROTATE_WARNING_SECS: u64 = 14 * 86400;

/// How old a status statement may be: a captured one cannot be replayed later.
pub const STATEMENT_MAX_AGE_SECS: u64 = 15 * 60;
/// Code waits this long before clients update to it.
pub const CODE_WAIT_SECS: u64 = 48 * 3600;
/// The largest upload: a per-platform package (the larger limit; `ext_pkg::read` holds others to theirs) and its bundle.
pub const MAX_UPLOAD_BYTES: usize = ext_manifest::limits::TARGET_PACKAGE_BYTES as usize + publisher::MAX_BUNDLE_BYTES + 4;

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
    /// The root public keys clients trust for this registry (what the app ships), and how many must sign a root.
    /// `promote` refuses a new root they did not sign; a development registry has none.
    #[serde(default, rename = "root-keys", skip_serializing_if = "Vec::is_empty")]
    pub root_keys: Vec<PublicKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<usize>,
    /// Publisher names (the part of an id before the dot) invited to publish code: Wasm and native programs. Everyone
    /// else may publish data-only extensions. Empty, the default, keeps the registry data-only (Phase 1). A
    /// publisher's first native program waits for a reviewer (`yard review`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invited: Vec<String>,
}

fn default_listen() -> String {
    "127.0.0.1:8080".into()
}

impl Config {
    /// The root keys and threshold, when the registry has them.
    pub fn trust(&self) -> Option<RootKeys> {
        (!self.root_keys.is_empty()).then(|| RootKeys { keys: self.root_keys.clone(), threshold: self.threshold.unwrap_or(self.root_keys.len()) })
    }

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
    /// The publisher's first native program: it stays unlisted until a reviewer approves it (`yard review`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub review: bool,
}

/// What [`Yard::health`] reports; `yard status` and `GET /metrics` print it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Health {
    pub index_version: u64,
    /// Unix seconds; clients wait for the user before installing after it.
    pub index_expires: u64,
    pub online_expires: u64,
    pub extensions: usize,
    pub versions: usize,
    pub revoked: usize,
    pub waiting_for_review: usize,
}

impl Health {
    /// The problem to alert on at `now`: the index expires within `index_hours`, or the online key within `key_days`.
    pub fn problem(&self, now: u64, index_hours: u64, key_days: u64) -> Option<String> {
        if self.index_expires <= now {
            return Some("the index has expired: clients wait for the user before installing. Is `yard serve` running? Run `yard resign`".into());
        }
        if self.index_expires < now + index_hours * 3600 {
            return Some(format!(
                "the index expires in {} h (it is signed daily). Is `yard serve` running? Run `yard resign`",
                (self.index_expires - now) / 3600
            ));
        }
        if self.online_expires < now + key_days * 86400 {
            return Some(format!(
                "the online key expires in {} days: rotate it (docs/registry.md, Rotate the online key)",
                self.online_expires.saturating_sub(now) / 86400
            ));
        }
        None
    }

    /// The Prometheus text format, for `GET /metrics`.
    pub fn metrics(&self) -> String {
        let rows: [(&str, &str, u64); 7] = [
            ("yard_index_version", "The version of the signed index being served.", self.index_version),
            ("yard_index_expires_timestamp_seconds", "When the served index expires (Unix seconds).", self.index_expires),
            ("yard_online_key_expires_timestamp_seconds", "When the online key expires (Unix seconds).", self.online_expires),
            ("yard_extensions", "Extensions in the index.", self.extensions as u64),
            ("yard_versions", "Versions in the index.", self.versions as u64),
            ("yard_versions_revoked", "Revoked versions in the index.", self.revoked as u64),
            ("yard_reviews_waiting", "First native programs waiting for a reviewer.", self.waiting_for_review as u64),
        ];
        rows.iter().map(|(name, help, v)| format!("# HELP {name} {help}\n# TYPE {name} gauge\n{name} {v}\n")).collect()
    }
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
        let yard = Yard { cfg, root, pins, online, db: Mutex::new(db) };
        yard.add_missing_details()?;
        Ok(yard)
    }

    fn packages(&self) -> PathBuf {
        self.cfg.public.join("packages")
    }

    /// Packages waiting for a reviewer: beside the database, never in the folder the CDN serves.
    fn review_dir(&self) -> PathBuf {
        self.cfg.database.with_file_name("review")
    }

    /// Whether the publisher name `prefix` may publish code and native programs.
    pub fn invited(&self, prefix: &str) -> bool {
        self.cfg.invited.iter().any(|p| p == prefix)
    }

    /// Versions published before details files existed get theirs from the stored package; the next signing lists them.
    fn add_missing_details(&self) -> Result<(), String> {
        let mut conn = self.lock();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        for (id, mut release) in db::releases_without_details(&tx).map_err(|e| e.to_string())? {
            let path = self.packages().join(format!("{}.cyx", release.digest));
            let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            let pkg = ext_pkg::read(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
            let details = yard::Details::of(&pkg).to_bytes();
            write_atomic(&self.packages().join(format!("{}.details.json", release.digest)), &details).map_err(|e| e.to_string())?;
            release.details = Some(yard::Details::digest(&details));
            db::put_release(&tx, &id, &release).map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
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
        // Code and native programs by invitation only; without one the registry stays data-only.
        let invited = self.invited(m.id.publisher());
        if !m.is_declarative() && !invited {
            return Err(Error::Invalid(format!("{} {} runs code; the registry accepts data-only extensions until code extensions ship", m.id, m.version)));
        }
        // A component built against a 0.x API still runs during the move to 1.0, but new uploads must be 1.x builds.
        if let Some(c) = pkg.component.as_ref().filter(|c| c.in_transition()) {
            return Err(Error::Invalid(format!(
                "{} {} was built against API {}; the registry takes API {} builds (docs/extensions/migrating-to-1.0.md)",
                m.id,
                m.version,
                c.built_for,
                ext_api::VERSION
            )));
        }
        if m.has_native_programs() && !invited {
            return Err(Error::Forbidden(format!(
                "{} {} ships native programs; this registry takes them from invited publishers only (docs/registry.md)",
                m.id, m.version
            )));
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
        let digest = pkg.digest.hex();
        let details = yard::Details::of(&pkg).to_bytes();
        if let Some(t) = &pkg.target
            && let Some(release) = db::release_of(&tx, &m.id, &m.version).map_err(internal("database"))?
        {
            // Another platform of a version with native programs: the same manifest and README, a platform not there yet.
            let p = self.add_platform(&tx, &pkg, (bytes, bundle), release, t, &details, &who, now)?;
            tx.commit().map_err(internal("database"))?;
            return Ok(p);
        }
        if db::version_exists(&tx, &m.id, &m.version).map_err(internal("database"))? {
            return Err(Error::Conflict(format!("{} {} was published before; a version number is never reused, so raise it", m.id, m.version)));
        }
        let release = yard::Release {
            version: m.version.clone(),
            digest: digest.clone(),
            sha256: hex(&Sha256::digest(bytes)),
            size: bytes.len() as u64,
            permissions: yard::permission_digest(m),
            // Native programs are code too: their updates wait like a Wasm extension's.
            code: !m.is_declarative() || m.has_native_programs(),
            published: now,
            status: Status::Ok,
            downloads: 0,
            immediate: false,
            details: Some(yard::Details::digest(&details)),
            targets: pkg
                .target
                .iter()
                .map(|t| (t.clone(), yard::TargetPackage { digest: digest.clone(), sha256: hex(&Sha256::digest(bytes)), size: bytes.len() as u64 }))
                .collect(),
        };
        db::upsert_extension(&tx, m, &who).map_err(internal("database"))?;
        db::add_version(&tx, &m.id, &release).map_err(internal("database"))?;
        if let Some(t) = &pkg.target {
            db::add_target_package(&tx, &m.id, &m.version, t, &digest).map_err(internal("database"))?;
        }
        // A publisher's first native program waits, unlisted, for a reviewer; its files stay out of the public folder.
        let review = m.has_native_programs() && !db::owner_reviewed(&tx, &owner).map_err(internal("database"))?;
        if review {
            db::queue_review(&tx, &m.id, &m.version, &owner, now).map_err(internal("database"))?;
        }
        // The package lands before the index that lists it; a failure after this leaves an unlisted file, never a
        // listed one that is missing.
        self.write_package(review, &digest, bytes, bundle, &details)?;
        self.sign_index(&tx, now)?;
        tx.commit().map_err(internal("database"))?;
        let available = if release.code { now + CODE_WAIT_SECS } else { now };
        Ok(Published { id: m.id.clone(), version: m.version.clone(), digest, available, publisher: who, review })
    }

    /// A package, its signature and its details file, in the public folder or (waiting for review) the review folder.
    fn write_package(&self, review: bool, digest: &str, bytes: &[u8], bundle: &[u8], details: &[u8]) -> Result<(), Error> {
        let dir = if review { self.review_dir() } else { self.packages() };
        std::fs::create_dir_all(&dir).map_err(|e| Error::Internal(format!("cannot create {}: {e}", dir.display())))?;
        write_atomic(&dir.join(format!("{digest}.cyx")), bytes)?;
        // Clients check the publisher's signature themselves; the registry only passes it on.
        write_atomic(&dir.join(format!("{digest}.sigstore.json")), bundle)?;
        write_atomic(&dir.join(format!("{digest}.details.json")), details)
    }

    /// Add platform `target`'s package to a version published for other platforms.
    #[allow(clippy::too_many_arguments)]
    fn add_platform(
        &self,
        tx: &rusqlite::Transaction,
        pkg: &ext_pkg::Package,
        (bytes, bundle): (&[u8], &[u8]),
        mut release: yard::Release,
        target: &str,
        details: &[u8],
        who: &Publisher,
        now: u64,
    ) -> Result<Published, Error> {
        let m = &pkg.manifest;
        if release.targets.is_empty() {
            return Err(Error::Conflict(format!("{} {} was published without native programs; raise the version", m.id, m.version)));
        }
        if release.targets.contains_key(target) {
            return Err(Error::Conflict(format!("{} {} for {target} was published before; a version number is never reused, so raise it", m.id, m.version)));
        }
        if release.details.as_deref() != Some(yard::Details::digest(details).as_str()) || release.permissions != yard::permission_digest(m) {
            return Err(Error::Conflict(format!(
                "{} {} for {target} has another manifest or README than its other platforms; every platform's package of a version has the same ones",
                m.id, m.version
            )));
        }
        let review = match db::review(tx, &m.id, &m.version).map_err(internal("database"))? {
            Some(db::Review::Rejected) => return Err(Error::Forbidden(format!("{} {} was rejected by a reviewer; raise the version", m.id, m.version))),
            Some(db::Review::Pending) => true,
            Some(db::Review::Approved) | None => false,
        };
        let digest = pkg.digest.hex();
        release.targets.insert(target.into(), yard::TargetPackage { digest: digest.clone(), sha256: hex(&Sha256::digest(bytes)), size: bytes.len() as u64 });
        db::put_release(tx, &m.id, &release).map_err(internal("database"))?;
        db::add_target_package(tx, &m.id, &m.version, target, &digest).map_err(internal("database"))?;
        self.write_package(review, &digest, bytes, bundle, details)?;
        self.sign_index(tx, now)?;
        let available = if release.code { release.published + CODE_WAIT_SECS } else { now };
        Ok(Published { id: m.id.clone(), version: m.version.clone(), digest, available, publisher: who.clone(), review })
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

    /// When the online key expires (Unix seconds); the index cannot be signed past it.
    pub fn online_expires(&self) -> u64 {
        self.root.online.expires
    }

    /// The registry's state as monitoring sees it: the signed index on disk, the online key, the review queue.
    pub fn health(&self) -> Result<Health, Error> {
        let path = self.cfg.public.join(yard::INDEX_FILE);
        let bytes = std::fs::read(&path).map_err(|e| Error::Internal(format!("cannot read {}: {e}", path.display())))?;
        let index: Index = serde_json::from_slice(&bytes).map_err(|e| Error::Internal(format!("{}: {e}", path.display())))?;
        let releases = index.extensions.values().flat_map(|e| &e.versions);
        Ok(Health {
            index_version: index.version,
            index_expires: index.expires,
            online_expires: self.root.online.expires,
            extensions: index.extensions.len(),
            versions: releases.clone().count(),
            revoked: releases.filter(|r| matches!(r.status, Status::Revoked { .. })).count(),
            waiting_for_review: self.review_queue()?.len(),
        })
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

    /// The Market's page data for a package (not counted as a download).
    pub fn details(&self, digest: &str) -> Result<PathBuf, Error> {
        if ext_pkg::ContentDigest::parse(digest).is_none() {
            return Err(Error::NotFound("no such package".into()));
        }
        let path = self.packages().join(format!("{digest}.details.json"));
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

/// A new registry in `dir`: an online key, `root.json` pinning the given Sigstore roots, and `yard.json`. Without
/// `trust` it is a development registry whose root is signed by one key made here (`keys/root.key`). With the
/// production root keys the root is left unsigned for their holders (`cyx yard sign root`), and no root key is ever
/// on the server. Refuses a folder that already has a config.
pub fn init(dir: &Path, registry: &str, fulcio: &str, rekor: &str, trust: Option<RootKeys>, now: u64) -> Result<PathBuf, String> {
    let cfg_path = dir.join("yard.json");
    if cfg_path.exists() {
        return Err(format!("{} exists; this folder already has a registry", cfg_path.display()));
    }
    Pins::from_pem(fulcio, rekor).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir.join("keys")).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    if let Some(t) = &trust
        && (t.keys.is_empty() || t.threshold == 0 || t.threshold > t.keys.len())
    {
        return Err("give the root public keys and a threshold between 1 and their number".into());
    }
    let (online_key, online_text) = SigningKey::generate()?;
    write_private(&dir.join("keys/online.key"), &online_text)?;
    let root = Root {
        format: yard::FORMAT,
        version: 1,
        registry: registry.into(),
        online: yard::OnlineKey { key: online_key.public(), expires: now + ONLINE_KEY_LIFETIME_SECS },
        fulcio: fulcio.into(),
        rekor: rekor.into(),
    };
    let root_bytes = serde_json::to_vec_pretty(&root).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("root.json"), &root_bytes).map_err(|e| e.to_string())?;
    let sigs = match &trust {
        Some(_) => serde_json::to_string_pretty(&yard::Signatures::default()).map_err(|e| e.to_string())? + "\n",
        None => {
            let (root_key, root_text) = SigningKey::generate()?;
            write_private(&dir.join("keys/root.key"), &root_text)?;
            std::fs::write(dir.join("root-key.txt"), format!("{}\n", root_key.public().hex())).map_err(|e| e.to_string())?;
            yard::add_signature("", &root_key, yard::Role::Root, &root_bytes)?
        }
    };
    std::fs::write(dir.join("root.json.sig"), sigs).map_err(|e| e.to_string())?;
    let cfg = Config {
        database: "yard.sqlite".into(),
        public: "public".into(),
        root: "root.json".into(),
        online_key: "keys/online.key".into(),
        listen: default_listen(),
        root_keys: trust.as_ref().map(|t| t.keys.clone()).unwrap_or_default(),
        threshold: trust.map(|t| t.threshold),
        invited: Vec::new(),
    };
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())? + "\n").map_err(|e| e.to_string())?;
    Ok(cfg_path)
}

/// Where the next root and online key wait for the root key holders.
fn next_paths(cfg: &Config) -> (PathBuf, PathBuf) {
    (cfg.root.with_file_name("root.next.json"), cfg.online_key.with_file_name("online.next.key"))
}

/// Start an online key rotation: a new online key (`online.next.key`) and `root.next.json`, the current root with a
/// higher version naming that key for 90 days from `now`, with no signatures yet. Returns the file to sign.
pub fn rotate(cfg_path: &Path, now: u64) -> Result<PathBuf, String> {
    let cfg = Config::load(cfg_path)?;
    let (next_root, next_key) = next_paths(&cfg);
    if next_key.exists() {
        return Err(format!("{} exists: a rotation is under way; finish it with `yard promote` or delete both next files", next_key.display()));
    }
    let root: Root = serde_json::from_slice(&std::fs::read(&cfg.root).map_err(|e| format!("cannot read {}: {e}", cfg.root.display()))?)
        .map_err(|e| format!("{}: {e}", cfg.root.display()))?;
    let (key, text) = SigningKey::generate()?;
    // After a restore from backup there is no keys folder: the online key is never backed up.
    if let Some(dir) = next_key.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    write_private(&next_key, &text)?;
    let next = Root { version: root.version + 1, online: yard::OnlineKey { key: key.public(), expires: now + ONLINE_KEY_LIFETIME_SECS }, ..root };
    std::fs::write(&next_root, serde_json::to_vec_pretty(&next).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::write(sig_path(&next_root), serde_json::to_string_pretty(&yard::Signatures::default()).map_err(|e| e.to_string())? + "\n")
        .map_err(|e| e.to_string())?;
    Ok(next_root)
}

/// Finish a rotation once the root key holders signed `root.next.json`: check their signatures against `yard.json`'s
/// root keys, put the new root and online key in place, and sign a fresh index with the new key. A running `yard
/// serve` keeps the old key in memory: restart it. Returns the new root version.
pub fn promote(cfg_path: &Path, now: u64) -> Result<u64, String> {
    let cfg = Config::load(cfg_path)?;
    let trust = cfg.trust().ok_or_else(|| format!("{} has no root-keys: add the root public keys clients trust and the threshold", cfg_path.display()))?;
    let (next_root, next_key) = next_paths(&cfg);
    let read = |p: &Path| std::fs::read(p).map_err(|e| format!("cannot read {}: {e}", p.display()));
    let (bytes, sigs) = (read(&next_root)?, read(&sig_path(&next_root))?);
    let root = yard::verify_root(&trust, &bytes, &sigs).map_err(|e| match e {
        yard::Error::Signature { valid, needed, .. } => format!(
            "{} has {valid} valid signature(s) from the root keys in yard.json; {needed} needed: another holder signs it with `cyx yard sign root`",
            sig_path(&next_root).display()
        ),
        e => e.to_string(),
    })?;
    let key = SigningKey::from_file(&String::from_utf8_lossy(&read(&next_key)?)).map_err(|e| format!("{}: {e}", next_key.display()))?;
    if key.public() != root.online.key {
        return Err(format!("{} is not the online key {} names", next_key.display(), next_root.display()));
    }
    if root.online.expires <= now {
        return Err(format!("{} names an online key that has expired; start again with `yard rotate`", next_root.display()));
    }
    // The signatures first, then the root: a crash between them leaves a root that does not verify, which `promote`
    // repairs when run again; the old key stays until the new root is in place.
    write_atomic(&sig_path(&cfg.root), &sigs).map_err(|e| e.to_string())?;
    write_atomic(&cfg.root, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&next_key, &cfg.online_key).map_err(|e| format!("cannot move {} into place: {e}", next_key.display()))?;
    let _ = std::fs::remove_file(sig_path(&next_root));
    let _ = std::fs::remove_file(&next_root);
    Yard::open(cfg)?.resign(now).map_err(|e| e.to_string())?;
    Ok(root.version)
}

fn write_private(path: &Path, text: &str) -> Result<(), String> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut f = opts.open(path).map_err(|e| format!("cannot create {}: {e}", path.display()))?;
    std::io::Write::write_all(&mut f, text.as_bytes()).map_err(|e| format!("cannot write {}: {e}", path.display()))
}
