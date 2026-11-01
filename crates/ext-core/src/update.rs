//! Updates and version status (#69, docs/plugin-architecture.md §8): what a registry's signed index says about one
//! installed extension, and what the client does next. No network and no UI: the app downloads, installs and asks.
//!
//! - A newer version installs by itself when `extensions.autoUpdate` is `install`: data-only versions at once, versions
//!   that run code 48 h after they were published (unless the registry cleared an advisory fix, or it is a
//!   pre-release the user opted into). **Update now** skips the wait.
//! - A yanked installed version goes back to the newest earlier one at once; that is not an update, so `off` does not
//!   stop it.
//! - A revoked installed version stops (with its dependents, see [`crate::stopped`]) until another version replaces it:
//!   a newer one that may install now, else the newest earlier one that is not withdrawn, as for a yanked version.

use ext_manifest::ExtId;
use serde::{Deserialize, Serialize};

use crate::Locked;
use crate::yard::{Index, Release, Status};

/// How long a version that runs code waits before it installs by itself.
pub const CODE_WAIT_SECS: u64 = 48 * 3600;

/// `extensions.autoUpdate`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoUpdate {
    /// Nothing is downloaded until the user asks.
    Off,
    /// Newer versions are shown; the user installs them.
    Notify,
    #[default]
    Install,
}

impl AutoUpdate {
    pub fn parse(s: &str) -> Option<AutoUpdate> {
        match s {
            "off" => Some(AutoUpdate::Off),
            "notify" => Some(AutoUpdate::Notify),
            "install" => Some(AutoUpdate::Install),
            _ => None,
        }
    }
}

/// What the index says about the installed version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Standing {
    Ok,
    /// Still works; the publisher names a replacement, if any.
    Deprecated {
        replacement: Option<ExtId>,
    },
    /// Withdrawn by its publisher.
    Yanked,
    /// Stopped by the registry, with its reason.
    Revoked {
        reason: String,
    },
    /// The index does not list this version (or the extension): nothing is known about it.
    Unlisted,
}

/// What the client does next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Next {
    Nothing,
    /// The installed version was yanked or revoked: go back to this one now.
    RollBack(Release),
    /// A newer version, which may install by itself from `ready_at` (Unix seconds) on.
    Update {
        release: Release,
        ready_at: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub standing: Standing,
    pub next: Next,
}

/// When `r` may install by itself.
pub fn ready_at(r: &Release) -> u64 {
    if r.code && !r.immediate && !r.is_pre_release() { r.published.saturating_add(CODE_WAIT_SECS) } else { r.published }
}

/// Check one installed extension (`id`, as the lock has it) against its registry's index at `now`.
pub fn check(index: &Index, id: &ExtId, locked: &Locked, now: u64) -> Check {
    let installed = index.release(id, &locked.version);
    let standing = match installed.map(|r| &r.status) {
        None => Standing::Unlisted,
        Some(Status::Ok) => Standing::Ok,
        Some(Status::Deprecated { replacement }) => Standing::Deprecated { replacement: replacement.clone() },
        Some(Status::Yanked) => Standing::Yanked,
        Some(Status::Revoked { reason }) => Standing::Revoked { reason: reason.clone() },
    };
    // Someone on a pre-release they picked by hand stays on that line until a release passes it.
    let pre = locked.pre_release || locked.version.pre.is_some();
    let newer = index.latest(id, locked.pre_release).filter(|r| r.version > locked.version);
    let update = newer.map(|r| Next::Update { release: r.clone(), ready_at: ready_at(r) });
    let next = match (&standing, update) {
        // A yanked or revoked version is left at once: forward when the newer one may install now, else back.
        (Standing::Yanked | Standing::Revoked { .. }, update)
            if !update.as_ref().is_some_and(|u| matches!(u, Next::Update { ready_at, .. } if *ready_at <= now)) =>
        {
            let versions = index.extensions.get(id).map(|e| e.versions.as_slice()).unwrap_or_default();
            let earlier = versions
                .iter()
                .rev()
                .find(|r| r.version < locked.version && matches!(r.status, Status::Ok | Status::Deprecated { .. }) && (pre || !r.is_pre_release()));
            earlier.map(|r| Next::RollBack(r.clone())).or(update).unwrap_or(Next::Nothing)
        }
        (_, update) => update.unwrap_or(Next::Nothing),
    };
    Check { standing, next }
}

#[cfg(test)]
#[path = "update_tests.rs"]
mod tests;
