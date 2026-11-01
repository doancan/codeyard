//! The client side of a registry (docs/registry.md): keep its signed index fresh without saying anything about the
//! user, and download a package with every check done before the consent sheet. No network of its own: the app hands
//! in a [`Fetch`] (curl), tests hand in a folder.
//!
//! ```text
//! <data>/extensions/registries/<name>/root.json, root.json.sig, index.json, index.json.sig   the last accepted copy
//! <data>/extensions/registries/<name>/state.json   versions seen (no rollback), when fetched, the index's ETag
//! <data>/extensions/registries/<name>/details/<sha256>.json   Market pages opened before, checked against the index
//! ```
//!
//! Every client fetches the same bytes (the whole index), at most once a day and only when it has something from the
//! registry or the user opens the Market, so the registry learns nothing about what anyone installed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ext_manifest::{ExtId, Manifest, Version};
use ext_pkg::ContentDigest;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::publisher::{self, Pins, Signed};
use crate::yard::{self, Publisher, Release, RootKeys, Seen, Status, Verified};

/// How long a fetched index is used before asking again (the index itself is good for 72 h).
pub const REFRESH_SECS: u64 = 24 * 3600;

/// A registry the user trusts: its address and the root keys that sign it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registry {
    /// Short and stable: it names the cache folder and is recorded in the lock ("codeyard").
    pub name: String,
    /// Base URL, without a trailing slash.
    pub url: String,
    pub keys: RootKeys,
}

impl Registry {
    /// One entry of `extensions.registries`: `{ "name", "url", "root-keys": [hex…], "threshold" }`.
    pub fn from_setting(v: &serde_json::Value) -> Result<Registry, String> {
        let name =
            v["name"].as_str().filter(|n| !n.is_empty() && n.len() <= 40 && n.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'));
        let name = name.ok_or("“name” is lowercase a-z, 0-9 and -, at most 40 characters")?;
        let url = v["url"]
            .as_str()
            .map(|u| u.trim_end_matches('/'))
            .filter(|u| u.starts_with("https://") || u.starts_with("http://localhost") || u.starts_with("http://127.0.0.1"));
        let url = url.ok_or("“url” is an https:// address")?;
        let keys = v["root-keys"].as_array().ok_or("“root-keys” lists the registry's root public keys (hex)")?;
        let keys: Vec<_> = keys.iter().map(|k| k.as_str().and_then(yard::PublicKey::parse)).collect::<Option<_>>().ok_or("a root key is 64 hex digits")?;
        let threshold = v["threshold"].as_u64().unwrap_or(keys.len() as u64) as usize;
        if keys.is_empty() || threshold == 0 || threshold > keys.len() {
            return Err("give at least one root key and a threshold between 1 and their number".into());
        }
        Ok(Registry { name: name.into(), url: url.into(), keys: RootKeys { keys, threshold } })
    }

    fn file_url(&self, name: &str) -> String {
        format!("{}/{name}", self.url)
    }
}

/// What a [`Fetch`] got.
#[derive(Debug)]
pub enum Got {
    /// The ETag matched (304): nothing to read.
    NotModified,
    Body {
        bytes: Vec<u8>,
        etag: Option<String>,
    },
}

/// One HTTP GET, at most `limit` bytes, with `If-None-Match` when `etag` is given. A 404 is an `Err`.
pub trait Fetch {
    fn get(&self, url: &str, etag: Option<&str>, limit: usize) -> Result<Got, String>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// The network or the server.
    Network(String),
    /// The registry's files did not verify.
    Index(yard::Error),
    /// No such extension or version, or it may not be installed (yanked, revoked).
    NotFound(String),
    /// The package or its signature is not what the index promised.
    Package(String),
    Signature(publisher::Error),
    /// Our own cache folder.
    Cache(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Network(m) | Error::NotFound(m) | Error::Package(m) | Error::Cache(m) => f.write_str(m),
            Error::Index(e) => write!(f, "{e}"),
            Error::Signature(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Default, Serialize, Deserialize)]
struct State {
    seen: Seen,
    fetched: u64,
    etag: Option<String>,
}

const FILES: [&str; 4] = ["root.json", "root.json.sig", "index.json", "index.json.sig"];
const LIMITS: [usize; 4] = [yard::MAX_ROOT_BYTES, yard::MAX_SIG_BYTES, yard::MAX_INDEX_BYTES, yard::MAX_SIG_BYTES];

/// The cache folder of one registry under `<data>/extensions/registries`.
pub fn cache_dir(registries: &Path, reg: &Registry) -> PathBuf {
    registries.join(&reg.name)
}

fn read_state(dir: &Path) -> State {
    std::fs::read(dir.join("state.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn read_cached(dir: &Path) -> Option<[Vec<u8>; 4]> {
    let read = |n: &str| std::fs::read(dir.join(n)).ok();
    Some([read(FILES[0])?, read(FILES[1])?, read(FILES[2])?, read(FILES[3])?])
}

fn verify_files(reg: &Registry, f: &[Vec<u8>; 4], now: u64, seen: Seen) -> Result<Verified, Error> {
    yard::verify(&reg.keys, [&f[0], &f[1], &f[2], &f[3]], now, seen).map_err(Error::Index)
}

/// The last accepted index, from the cache only (no network); `None` when there is none or it no longer verifies.
pub fn cached(reg: &Registry, dir: &Path, now: u64) -> Option<Verified> {
    verify_files(reg, &read_cached(dir)?, now, Seen::default()).ok()
}

/// When the cached index was fetched (Unix seconds), or `None` if never.
pub fn fetched_at(dir: &Path) -> Option<u64> {
    let s = read_state(dir);
    (s.fetched > 0).then_some(s.fetched)
}

/// The registry's index, fetched when the cached one is older than a day (or `force`), else the cached one. A stale
/// index (past its 72 h) comes back marked; the caller asks the user before installing from it.
pub fn index(reg: &Registry, dir: &Path, fetch: &dyn Fetch, now: u64, force: bool) -> Result<Verified, Error> {
    let state = read_state(dir);
    let cached = read_cached(dir);
    if !force
        && now < state.fetched + REFRESH_SECS
        && let Some(files) = &cached
        && let Ok(v) = verify_files(reg, files, now, Seen::default())
    {
        return Ok(v);
    }
    // A registry writes the signature, then the index: reading between the two gives a mismatch, so read once more.
    match refresh(reg, dir, fetch, now, &state, cached.as_ref()) {
        Err(Error::Index(_)) => refresh(reg, dir, fetch, now, &state, cached.as_ref()),
        r => r,
    }
}

fn refresh(reg: &Registry, dir: &Path, fetch: &dyn Fetch, now: u64, state: &State, cached: Option<&[Vec<u8>; 4]>) -> Result<Verified, Error> {
    let etag = cached.and(state.etag.as_deref());
    let index = match fetch.get(&reg.file_url(FILES[2]), etag, LIMITS[2]).map_err(Error::Network)? {
        Got::NotModified => None,
        Got::Body { bytes, etag } => Some((bytes, etag)),
    };
    let (files, etag) = match (index, cached) {
        (None, Some(c)) => (c.clone(), state.etag.clone()),
        (None, None) => return Err(Error::Network("the registry answered “not modified” to a first request".into())),
        (Some((bytes, etag)), _) => {
            let get = |i: usize| match fetch.get(&reg.file_url(FILES[i]), None, LIMITS[i]).map_err(Error::Network)? {
                Got::Body { bytes, .. } => Ok(bytes),
                Got::NotModified => Err(Error::Network(format!("{}: unexpected “not modified”", FILES[i]))),
            };
            ([get(0)?, get(1)?, bytes, get(3)?], etag)
        }
    };
    let v = verify_files(reg, &files, now, state.seen)?;
    std::fs::create_dir_all(dir).map_err(|e| Error::Cache(format!("cannot create {}: {e}", dir.display())))?;
    for (name, bytes) in FILES.iter().zip(&files) {
        write_atomic(&dir.join(name), bytes)?;
    }
    let state = State { seen: v.seen(), fetched: now, etag };
    write_atomic(&dir.join("state.json"), &serde_json::to_vec(&state).map_err(|e| Error::Cache(e.to_string()))?)?;
    Ok(v)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, path)).map_err(|e| Error::Cache(format!("cannot write {}: {e}", path.display())))
}

/// `publisher.name` or `publisher.name@1.2.0`.
pub fn parse_spec(spec: &str) -> Result<(ExtId, Option<Version>), String> {
    let (id, version) = match spec.split_once('@') {
        Some((id, v)) => (id, Some(Version::parse(v).map_err(|e| format!("version {e}"))?)),
        None => (spec, None),
    };
    Ok((ExtId::parse(id).map_err(|e| format!("id {e}"))?, version))
}

/// A package downloaded and checked: ready for the consent sheet.
#[derive(Debug)]
pub struct Download {
    pub bytes: Vec<u8>,
    pub manifest: Manifest,
    pub release: Release,
    pub signed: Signed,
    /// For a release with native programs: every platform's content digest, as the index lists them (for the lock).
    pub targets: BTreeMap<String, ContentDigest>,
}

/// Choose the version (`version`, else the newest installable one, pre-releases only if asked), download the package
/// and its publisher signature, and check everything the index promises about them. A release with native programs is
/// downloaded for this machine's platform; [`download_for`] names another (the workspace's host).
pub fn download(
    reg: &Registry,
    v: &Verified,
    id: &ExtId,
    version: Option<&Version>,
    pre_release: bool,
    fetch: &dyn Fetch,
    now: u64,
) -> Result<Download, Error> {
    download_for(reg, v, id, version, pre_release, ext_manifest::this_target().unwrap_or("unknown"), fetch, now)
}

/// [`download`] the package for platform `target` (a target triple); the same package for a release without native
/// programs.
#[allow(clippy::too_many_arguments)]
pub fn download_for(
    reg: &Registry,
    v: &Verified,
    id: &ExtId,
    version: Option<&Version>,
    pre_release: bool,
    target: &str,
    fetch: &dyn Fetch,
    now: u64,
) -> Result<Download, Error> {
    let entry = v.index.extensions.get(id).ok_or_else(|| Error::NotFound(format!("{id} is not in the {} registry", reg.name)))?;
    let release = match version {
        Some(ver) => v.index.release(id, ver).ok_or_else(|| Error::NotFound(format!("{id} has no version {ver}")))?,
        None => v.index.latest(id, pre_release).ok_or_else(|| {
            // Only pre-releases are left: say so, and how to take one.
            match v.index.latest(id, true) {
                Some(pre) => Error::NotFound(format!(
                    "{id} has only pre-releases so far (the newest is {0}); install it with {id}@{0}, or opt the extension in to pre-releases on its page",
                    pre.version
                )),
                None => Error::NotFound(format!("{id} has no version that can be installed")),
            }
        })?,
    };
    match &release.status {
        Status::Revoked { reason } => return Err(Error::NotFound(format!("{id} {} was revoked by the registry: {reason}", release.version))),
        Status::Yanked => return Err(Error::NotFound(format!("{id} {} was withdrawn by its publisher; install another version", release.version))),
        Status::Ok | Status::Deprecated { .. } => {}
    }
    let body = |name: String, limit: usize| match fetch.get(&reg.file_url(&name), None, limit).map_err(Error::Network)? {
        Got::Body { bytes, .. } => Ok(bytes),
        Got::NotModified => Err(Error::Network(format!("{name}: unexpected “not modified”"))),
    };
    let wanted = release.package_for(target).ok_or_else(|| {
        let built: Vec<&str> = release.targets.keys().map(String::as_str).collect();
        Error::NotFound(format!("{id} {} has native programs and is not built for {target} (only for {})", release.version, built.join(", ")))
    })?;
    let limit = if release.targets.is_empty() { ext_manifest::limits::PACKAGE_BYTES } else { ext_manifest::limits::TARGET_PACKAGE_BYTES };
    let bytes = body(format!("packages/{}.cyx", wanted.digest), limit as usize)?;
    let bad = |what: &str| {
        Error::Package(format!("{id} {}: the downloaded package {what} does not match the registry's index; nothing was installed", release.version))
    };
    if bytes.len() as u64 != wanted.size || hex(&Sha256::digest(&bytes)) != wanted.sha256 {
        return Err(bad("file"));
    }
    let pkg = ext_pkg::read(&bytes).map_err(|e| Error::Package(e.to_string()))?;
    if pkg.digest.hex() != wanted.digest || &pkg.manifest.id != id || pkg.manifest.version != release.version {
        return Err(bad("content"));
    }
    // A per-platform package says which platform it is for, and it must be the one the index lists it under.
    if !release.targets.is_empty() && pkg.target.as_deref() != Some(target) {
        return Err(bad("platform"));
    }
    let mut targets = BTreeMap::new();
    for (t, p) in release.targets.iter().filter(|(t, _)| ext_manifest::is_target(t)) {
        targets.insert(t.clone(), ContentDigest::parse(&p.digest).ok_or_else(|| bad("platform list"))?);
    }
    if yard::permission_digest(&pkg.manifest) != release.permissions {
        return Err(bad("permission list"));
    }
    let bundle = body(format!("packages/{}.sigstore.json", wanted.digest), publisher::MAX_BUNDLE_BYTES)?;
    let pins = Pins::from_pem(&v.root.fulcio, &v.root.rekor).map_err(Error::Signature)?;
    let signed = publisher::verify_package(&bundle, id, &release.version, &pkg.digest, &pins, now).map_err(Error::Signature)?;
    if signed.publisher != entry.publisher {
        return Err(Error::Package(format!(
            "{id} {}: signed by {}, but the registry lists {} as its publisher; nothing was installed",
            release.version, signed.publisher, entry.publisher
        )));
    }
    Ok(Download { bytes, manifest: pkg.manifest, release: release.clone(), signed, targets })
}

/// What the Market's page shows of one version, checked against the index.
#[derive(Clone, Debug)]
pub struct Page {
    pub manifest: Manifest,
    pub readme: Option<String>,
    /// The README was cut by the registry; the full one is in the package.
    pub readme_cut: bool,
}

/// The page of one version: from the cache folder `dir` when it was opened before, else downloaded (one request, not
/// counted as a download). Its sha256 is in the signed index, and its manifest must be the version's.
pub fn page(reg: &Registry, dir: &Path, id: &ExtId, release: &Release, fetch: &dyn Fetch) -> Result<Page, Error> {
    let sha = release.details.as_deref().ok_or_else(|| Error::NotFound(format!("the {} registry has no page for {id} {}", reg.name, release.version)))?;
    let cache = dir.join("details").join(format!("{sha}.json"));
    let bytes = match std::fs::read(&cache) {
        Ok(b) if hex(&Sha256::digest(&b)) == sha => b,
        _ => {
            let bytes =
                match fetch.get(&reg.file_url(&format!("packages/{}.details.json", release.digest)), None, yard::MAX_DETAILS_BYTES).map_err(Error::Network)? {
                    Got::Body { bytes, .. } => bytes,
                    Got::NotModified => return Err(Error::Network("details: unexpected “not modified”".into())),
                };
            if hex(&Sha256::digest(&bytes)) != sha {
                return Err(Error::Package(format!("{id} {}: the page does not match the registry's index", release.version)));
            }
            std::fs::create_dir_all(cache.parent().expect("has a parent")).map_err(|e| Error::Cache(format!("cannot create {}: {e}", dir.display())))?;
            write_atomic(&cache, &bytes)?;
            bytes
        }
    };
    let bad = |what: String| Error::Package(format!("{id} {}: the registry's page {what}", release.version));
    let d: yard::Details = serde_json::from_slice(&bytes).map_err(|e| bad(format!("is not valid: {e}")))?;
    let manifest = ext_manifest::parse(&d.manifest).map_err(|e| bad(format!("has a manifest that does not parse ({} errors)", e.len())))?;
    if &manifest.id != id || manifest.version != release.version || yard::permission_digest(&manifest) != release.permissions {
        return Err(bad("is for another package".into()));
    }
    Ok(Page { manifest, readme: d.readme, readme_cut: d.readme_cut })
}

/// The publisher an install from this registry pins, and what that means against the lock's pin.
pub fn pin(locked: Option<&crate::Locked>, registry: &str, signed: &Publisher) -> publisher::Pin {
    let pinned = locked.and_then(|l| l.source.registry()).filter(|(r, _)| *r == registry).map(|(_, p)| p);
    publisher::check_pin(pinned, signed)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
