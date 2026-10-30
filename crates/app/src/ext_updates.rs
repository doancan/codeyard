//! Updates and version status of extensions installed from a registry (#69, docs/registry.md). The rules are in
//! `ext_core::update`; this file runs them against each registry's daily index, downloads and installs what may
//! install by itself, and reports the rest to the Extensions view.
//!
//! - `extensions.autoUpdate: install` installs a newer version when it is ready (data-only at once, code after 48 h)
//!   and asks the same or fewer permissions from the same publisher. More permissions keep the installed version and
//!   wait for the user's OK on a card that lists only what is new; another publisher, or a list older than 3 days,
//!   needs the full consent sheet. `notify` only shows the version; `off` downloads nothing.
//! - A yanked installed version goes back to the newest earlier one at once (from the store when it is still there).
//! - A revoked version is recorded in `revoked.json` and stops, with its dependents, in every window (`ext::reload`).
//!
//! Privacy is as in `ext_registry`: only registries an installed extension came from are asked, at most once a day.

use std::collections::BTreeMap;
use std::path::Path;

use ext_core::registry::{self, Registry};
use ext_core::update::{self, AutoUpdate, Next, Standing};
use ext_core::yard::{Publisher, Release, Verified};
use ext_core::{Locked, Revoked, Source};
use ext_manifest::{ExtId, Manifest, Permissions, Version};

use crate::ext_registry::{cache_root, with_fetch};

/// What the user may do about a newer version that did not install by itself.
#[derive(Clone, Debug, PartialEq)]
pub enum Offer {
    /// Runs code: installs by itself from `ready_at` (Unix seconds) on. **Update now** skips the wait.
    Waiting { version: Version, ready_at: u64 },
    /// `autoUpdate` is `notify` or `off`: the user installs it.
    Available { version: Version },
    /// Asks for more than the installed version: `added` is only what is new.
    MorePermissions { version: Version, added: Permissions },
    /// Signed for another publisher than the one this extension came from: the full sheet.
    NewPublisher { version: Version, publisher: Publisher, was: Publisher },
    /// The registry's list is more than 3 days old: the user confirms on the full sheet.
    Stale { version: Version },
    /// Could not be installed now (network, checks); tried again on the next check.
    Failed { version: Version, error: String },
}

impl Offer {
    pub fn version(&self) -> &Version {
        match self {
            Offer::Waiting { version, .. }
            | Offer::Available { version }
            | Offer::MorePermissions { version, .. }
            | Offer::NewPublisher { version, .. }
            | Offer::Stale { version }
            | Offer::Failed { version, .. } => version,
        }
    }

    /// The full consent sheet answers it, not the card.
    pub fn needs_sheet(&self) -> bool {
        matches!(self, Offer::NewPublisher { .. } | Offer::Stale { .. })
    }
}

/// One installed registry extension after a check.
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub registry: String,
    pub standing: Standing,
    pub offer: Option<Offer>,
}

/// What a check did and found.
#[derive(Debug, Default)]
pub struct Report {
    /// The store or the list of revoked versions changed: reload.
    pub changed: bool,
    /// What happened, one sentence each (updated, rolled back).
    pub done: Vec<String>,
    /// Versions stopped by this check (newly revoked).
    pub stopped: Vec<String>,
    pub status: BTreeMap<ExtId, Status>,
    /// Registries that could not be read, and why.
    pub errors: Vec<String>,
}

/// One check at a time in this process: two windows checking together would download the same update twice.
static CHECKING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Check every extension installed from one of `registries`, act on what may happen by itself and report the rest.
pub fn check(cfg_dir: &Path, registries: &[Registry], auto: AutoUpdate, now: u64) -> Report {
    let _one = CHECKING.lock().unwrap_or_else(|e| e.into_inner());
    let mut report = Report::default();
    let store = crate::ext::store(cfg_dir);
    let (Ok(loaded), Ok(locked)) = (store.load(), store.locked()) else { return report };
    let revoked_before = store.revoked();
    let mut revocations = Vec::new();
    for reg in registries {
        let mine: Vec<(&ExtId, &Locked)> = locked.iter().filter(|(_, l)| l.source.registry().is_some_and(|(r, _)| r == reg.name)).collect();
        if mine.is_empty() {
            continue;
        }
        let dir = registry::cache_dir(&cache_root(cfg_dir), reg);
        let v = match with_fetch(|f| registry::index(reg, &dir, f, now, false)) {
            Ok(v) => v,
            Err(e) => {
                report.errors.push(format!("{}: {e}", reg.name));
                continue;
            }
        };
        for (id, l) in mine {
            let Some(entry) = v.index.extensions.get(id) else {
                report.status.insert(id.clone(), Status { registry: reg.name.clone(), standing: Standing::Unlisted, offer: None });
                continue;
            };
            // The installed version and the one a rollback would go to: stopped while revoked, wherever it is.
            for d in std::iter::once(l.digest).chain(l.previous) {
                if let Some(r) = entry.versions.iter().find(|r| r.digest == d.hex()) {
                    let mark = match &r.status {
                        ext_core::yard::Status::Revoked { reason } => Some(Revoked { id: id.clone(), version: r.version.clone(), reason: reason.clone() }),
                        _ => None,
                    };
                    revocations.push((d, mark));
                }
            }
            let Some(current) = loaded.extensions.iter().find(|e| &e.manifest.id == id).map(|e| &e.manifest) else { continue };
            let c = update::check(&v.index, id, l, now);
            let mut standing = c.standing;
            let offer = match c.next {
                Next::Nothing => None,
                Next::RollBack(r) if v.stale => Some(Offer::Stale { version: r.version }),
                Next::RollBack(r) => match roll_back(&From { cfg_dir, reg, v: &v, now }, id, l, current, &r) {
                    Ok(None) => {
                        report.changed = true;
                        report
                            .done
                            .push(crate::i18n::tf("{0} went back to version {1}: its publisher withdrew {2}.", &[&current.name, &r.version, &l.version]));
                        standing = standing_of(&r);
                        None
                    }
                    Ok(offer) => offer,
                    Err(error) => Some(Offer::Failed { version: r.version, error }),
                },
                Next::Update { release, .. } if auto != AutoUpdate::Install => Some(Offer::Available { version: release.version }),
                Next::Update { release, .. } if v.stale => Some(Offer::Stale { version: release.version }),
                Next::Update { release, ready_at } if ready_at > now => Some(Offer::Waiting { version: release.version, ready_at }),
                Next::Update { release, .. } => match install(&From { cfg_dir, reg, v: &v, now }, id, l, current, &release, None) {
                    Ok(None) => {
                        report.changed = true;
                        report.done.push(crate::i18n::tf("{0} was updated to version {1}.", &[&current.name, &release.version]));
                        standing = standing_of(&release);
                        None
                    }
                    Ok(offer) => offer,
                    Err(error) => Some(Offer::Failed { version: release.version, error }),
                },
            };
            report.status.insert(id.clone(), Status { registry: reg.name.clone(), standing, offer });
        }
    }
    match store.set_revoked(&revocations) {
        Ok(true) => {
            report.changed = true;
            for r in store.revoked().values().filter(|r| !revoked_before.values().any(|b| b.id == r.id && b.version == r.version)) {
                if locked.get(&r.id).is_some_and(|l| l.version == r.version) {
                    report.stopped.push(crate::ext::stopped_line(&r.id, &ext_core::Stopped::Revoked { reason: r.reason.clone() }));
                }
            }
        }
        Ok(false) => {}
        Err(e) => report.errors.push(e.to_string()),
    }
    report
}

fn standing_of(r: &Release) -> Standing {
    match &r.status {
        ext_core::yard::Status::Deprecated { replacement } => Standing::Deprecated { replacement: replacement.clone() },
        _ => Standing::Ok,
    }
}

/// **Update now** on one extension (no wait), or the user's OK on its card: `accepted` is what the card showed, and
/// the version installs only if it asks for nothing beyond it. `Ok(None)` when installed; an offer when it needs more.
pub fn update_now(cfg_dir: &Path, registries: &[Registry], id: &ExtId, accepted: Option<&Permissions>, now: u64) -> Result<Option<Offer>, String> {
    let _one = CHECKING.lock().unwrap_or_else(|e| e.into_inner());
    let store = crate::ext::store(cfg_dir);
    let locked = store.locked().map_err(|e| e.to_string())?;
    let l = locked.get(id).ok_or_else(|| crate::i18n::tf("{} is not installed", &[id]))?;
    let name = l.source.registry().map(|(r, _)| r).ok_or_else(|| crate::i18n::tf("{} was not installed from a registry", &[id]))?;
    let reg = registries.iter().find(|r| r.name == name).ok_or_else(|| crate::i18n::tf("the {} registry is no longer in your settings", &[&name]))?;
    let loaded = store.load().map_err(|e| e.to_string())?;
    let current = loaded.extensions.iter().find(|e| &e.manifest.id == id).map(|e| &e.manifest).ok_or_else(|| crate::i18n::tf("{} is not installed", &[id]))?;
    let dir = registry::cache_dir(&cache_root(cfg_dir), reg);
    let v = with_fetch(|f| registry::index(reg, &dir, f, now, false)).map_err(|e| e.to_string())?;
    let release = v.index.latest(id, l.pre_release).filter(|r| r.version > l.version).ok_or_else(|| crate::i18n::tf("{} has no newer version", &[id]))?;
    if v.stale {
        return Ok(Some(Offer::Stale { version: release.version.clone() }));
    }
    install(&From { cfg_dir, reg, v: &v, now }, id, l, current, release, accepted)
}

/// Where an update comes from: a registry and its index as read now.
struct From<'a> {
    cfg_dir: &'a Path,
    reg: &'a Registry,
    v: &'a Verified,
    now: u64,
}

/// Install `release` in place of the current version when nothing about it needs the user: the same publisher and no
/// permission beyond the current version's (or `accepted`). The permission digest in the index says "the same" without
/// a download; otherwise the version's page (its manifest, a small file) says what is new before the package is
/// downloaded.
fn install(from: &From, id: &ExtId, l: &Locked, current: &Manifest, release: &Release, accepted: Option<&Permissions>) -> Result<Option<Offer>, String> {
    let From { cfg_dir, reg, v, now } = *from;
    let version = release.version.clone();
    let entry = v.index.extensions.get(id).ok_or_else(|| crate::i18n::tf("{} is not in the registry", &[id]))?;
    if let Some((_, was)) = l.source.registry().filter(|(_, p)| **p != entry.publisher) {
        return Ok(Some(Offer::NewPublisher { version, publisher: entry.publisher.clone(), was: was.clone() }));
    }
    let more = |p: &Permissions| {
        let added = p.added_since(&current.permissions);
        (!added.is_empty() && !accepted.is_some_and(|a| added.added_since(a).is_empty())).then_some(added)
    };
    if release.permissions != ext_core::yard::permission_digest(current) {
        let dir = registry::cache_dir(&cache_root(cfg_dir), reg);
        if let Ok(page) = with_fetch(|f| registry::page(reg, &dir, id, release, f))
            && let Some(added) = more(&page.manifest.permissions)
        {
            return Ok(Some(Offer::MorePermissions { version, added }));
        }
    }
    // An extension with native programs stays on the platform it was installed for.
    let platform = l.targets.iter().find(|(_, d)| **d == l.digest).map(|(t, _)| t.as_str()).or(ext_manifest::this_target()).unwrap_or("unknown");
    let d =
        with_fetch(|f| registry::download_for(reg, v, id, Some(&release.version), release.is_pre_release(), platform, f, now)).map_err(|e| e.to_string())?;
    // The package decides, whatever the page said.
    if let Some(added) = more(&d.manifest.permissions) {
        return Ok(Some(Offer::MorePermissions { version, added }));
    }
    if d.manifest.code.is_some() {
        crate::ext_runtime::code_runs_here(id)?;
    }
    if let ext_core::publisher::Pin::Changed { was } = registry::pin(Some(l), &reg.name, &d.signed.publisher) {
        return Ok(Some(Offer::NewPublisher { version, publisher: d.signed.publisher, was }));
    }
    let source = Source::Registry { registry: reg.name.clone(), publisher: d.signed.publisher };
    crate::ext::store(cfg_dir).install_with_targets(&d.bytes, source, d.targets).map_err(|e| e.to_string())?;
    Ok(None)
}

/// Leave a yanked version for `target`: from the store when it is the version this one replaced, else downloaded with
/// the same checks as an update.
fn roll_back(from: &From, id: &ExtId, l: &Locked, current: &Manifest, target: &Release) -> Result<Option<Offer>, String> {
    if l.previous.is_some_and(|p| p.hex() == target.digest || target.targets.values().any(|t| t.digest == p.hex())) {
        return crate::ext::store(from.cfg_dir).rollback(id).map(|_| None).map_err(|e| e.to_string());
    }
    install(from, id, l, current, target, None)
}
