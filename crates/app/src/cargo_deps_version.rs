//! What a version requirement says about the versions a crate has: up to date, outdated, yanked, matching nothing; and how
//! the requirement text is rewritten for an update (the written precision and operator stay).

use semver::{Version, VersionReq};

use super::index::CrateInfo;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// The requirement allows the latest stable release.
    UpToDate { latest: Version },
    /// The latest stable release is outside the requirement.
    Outdated { latest: Version },
    /// The version Cargo.lock holds, or every version the requirement allows, was yanked.
    Yanked { version: Version },
    /// No published version satisfies the requirement.
    NoMatch { latest: Option<Version> },
}

/// Cargo reads `*`, `1`, `1.2`, `^1`, `~1.2`, `=1.2.3`, `>=1, <2`, `1.*`; so does the `semver` crate.
pub fn parse_req(text: &str) -> Option<VersionReq> {
    VersionReq::parse(text.trim()).ok()
}

pub fn classify(req_text: &str, info: &CrateInfo, locked: Option<&Version>) -> Option<Status> {
    let req = parse_req(req_text)?;
    if let Some(v) = locked.and_then(|l| info.find(l)).filter(|v| v.yanked) {
        return Some(Status::Yanked { version: v.version.clone() });
    }
    let latest = info.latest().map(|v| v.version.clone());
    let allowed = |yanked: bool| info.versions.iter().rev().find(|v| v.yanked == yanked && req.matches(&v.version));
    if allowed(false).is_none() {
        return Some(match allowed(true) {
            Some(v) => Status::Yanked { version: v.version.clone() },
            None => Status::NoMatch { latest },
        });
    }
    let latest = latest?;
    Some(if req.matches(&latest) { Status::UpToDate { latest } } else { Status::Outdated { latest } })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Update {
    /// The newest release, which may break the build (a new major version).
    Latest,
    /// The newest release the requirement already allows: only the written minimum moves up.
    Compatible,
}

/// The new text for a requirement, or `None` when the update would not change it.
pub fn updated_requirement(req_text: &str, info: &CrateInfo, how: Update) -> Option<String> {
    let req = parse_req(req_text)?;
    let target = match how {
        Update::Latest => info.latest()?,
        Update::Compatible => info.versions.iter().rev().find(|v| !v.yanked && v.version.pre.is_empty() && req.matches(&v.version))?,
    };
    let new = rewrite(req_text, &target.version)?;
    (new != req_text && parse_req(&new).is_some_and(|r| r.matches(&target.version))).then_some(new)
}

/// `req_text` raised to `to`, keeping its operator, its spacing and as many components as it had.
pub fn rewrite(req_text: &str, to: &Version) -> Option<String> {
    let text = req_text.trim();
    if text.is_empty() || text == "*" {
        return None;
    }
    let full = to.to_string();
    if text.contains([',', '|']) {
        return Some(full);
    }
    let op_len = text.find(|c: char| !matches!(c, '^' | '~' | '=' | '>' | '<')).unwrap_or(text.len());
    let (op, rest) = text.split_at(op_len);
    if matches!(op, ">" | "<" | "<=") {
        return Some(full);
    }
    let body = rest.trim_start();
    let gap = &rest[..rest.len() - body.len()];
    if body.contains(['-', '+']) {
        return Some(format!("{op}{gap}{full}"));
    }
    let parts: Vec<&str> = body.split('.').collect();
    let numbers = parts.iter().take_while(|p| p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty()).count();
    let wildcard = parts.len() > numbers;
    let shown = if !to.pre.is_empty() {
        full
    } else {
        match numbers {
            0 => return None,
            1 => to.major.to_string(),
            2 => format!("{}.{}", to.major, to.minor),
            _ => format!("{}.{}.{}", to.major, to.minor, to.patch),
        }
    };
    let tail = if wildcard && to.pre.is_empty() { ".*" } else { "" };
    Some(format!("{op}{gap}{shown}{tail}"))
}
