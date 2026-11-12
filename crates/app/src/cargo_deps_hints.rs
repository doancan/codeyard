//! What the editor shows for dependencies: the hint at the end of a dependency's line, the Problems entry for a yanked version,
//! and the hover text. Pure functions of the parsed manifest and the lookup results; all words go through `crate::i18n`.

use std::collections::HashMap;
use std::ops::Range;

use ext_api::value::Tone;
use semver::Version;

use super::index::CrateInfo;
use super::lock::Lock;
use super::manifest::{Dep, Manifest, Source};
use super::registry::{CrateMeta, Failure, Lookup};
use super::version::{Status, classify, parse_req};
use crate::i18n::{t, tf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    pub line: usize,
    /// The short text at the end of the line.
    pub text: String,
    pub tone: Tone,
    /// The sentence for the tooltip and for a screen reader.
    pub detail: String,
    /// The dependency's key.
    pub dep: String,
}

/// A warning for the Problems panel: a range of the text and what is wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub span: Range<usize>,
    pub message: String,
}

/// The lookups by lower-case crate name.
pub type Lookups = HashMap<String, Lookup>;

fn locked_of<'a>(dep: &Dep, lock: Option<&'a Lock>) -> Option<&'a Version> {
    let req = dep.req.as_ref().and_then(|r| parse_req(&r.text));
    lock?.resolved(&dep.package, req.as_ref())
}

fn status_of(dep: &Dep, info: &CrateInfo, lock: Option<&Lock>) -> Option<Status> {
    classify(&dep.req.as_ref()?.text, info, locked_of(dep, lock))
}

/// The dependencies that get a lookup: crates.io entries with a requirement written next to them.
pub fn wanted(manifest: &Manifest) -> Vec<&Dep> {
    manifest.deps.iter().filter(|d| d.req.is_some() && d.is_registry_lookup()).collect()
}

pub fn hint_for(dep: &Dep, lookup: &Lookup, lock: Option<&Lock>) -> Option<Hint> {
    let req = dep.req.as_ref()?;
    let mk = |text: String, tone: Tone, detail: String| Hint { line: dep.line, text, tone, detail, dep: dep.key.clone() };
    Some(match lookup {
        Lookup::Found { info, stale } => {
            let status = status_of(dep, info, lock)?;
            let from_cache = if *stale { format!(" {}", t("(from the cache, offline)")) } else { String::new() };
            let (text, tone, detail) = match status {
                Status::UpToDate { latest } => (
                    tf("up to date (latest {})", &[&latest]),
                    Tone::Success,
                    tf("{0} is up to date: “{1}” allows the latest release, {2}", &[&dep.key, &req.text, &latest]),
                ),
                Status::Outdated { latest } => (
                    tf("update available: {}", &[&latest]),
                    Tone::Warning,
                    tf("{0} can be updated: “{1}” does not allow the latest release, {2}", &[&dep.key, &req.text, &latest]),
                ),
                Status::Yanked { version } => {
                    (tf("yanked: {}", &[&version]), Tone::Warning, tf("{0}: version {1} was yanked and should not be used", &[&dep.key, &version]))
                }
                Status::NoMatch { latest } => (
                    t("no version matches").to_string(),
                    Tone::Danger,
                    match latest {
                        Some(l) => tf("{0}: no published version matches “{1}” (latest is {2})", &[&dep.key, &req.text, &l]),
                        None => tf("{0}: no published version matches “{1}”", &[&dep.key, &req.text]),
                    },
                ),
            };
            mk(text, tone, format!("{detail}{from_cache}"))
        }
        Lookup::NotFound => mk(t("not found on crates.io").into(), Tone::Danger, tf("{}: no crate with this name on crates.io", &[&dep.package])),
        Lookup::Offline => mk(t("offline").into(), Tone::Muted, tf("{}: crates.io could not be reached", &[&dep.key])),
        Lookup::Failed(why) => mk(t("lookup failed").into(), Tone::Muted, tf("{0}: lookup failed: {1}", &[&dep.key, why])),
    })
}

pub fn hints(manifest: &Manifest, lookups: &Lookups, lock: Option<&Lock>) -> Vec<Hint> {
    wanted(manifest).into_iter().filter_map(|d| hint_for(d, lookups.get(&d.package.to_ascii_lowercase())?, lock)).collect()
}

/// Yanked versions as warnings, never errors: the project still builds from its lock file.
pub fn problems(manifest: &Manifest, lookups: &Lookups, lock: Option<&Lock>) -> Vec<Problem> {
    wanted(manifest)
        .into_iter()
        .filter_map(|d| {
            let Lookup::Found { info, .. } = lookups.get(&d.package.to_ascii_lowercase())? else { return None };
            let Status::Yanked { version } = status_of(d, info, lock)? else { return None };
            Some(Problem {
                span: d.req.as_ref()?.span.clone(),
                message: tf("{0}: version {1} was yanked from crates.io. Update to a release that is not yanked.", &[&d.key, &version]),
            })
        })
        .collect()
}

/// The text of the hover over a dependency (or over one of its features).
pub fn hover_text(dep: &Dep, lookup: &Lookup, meta: Result<&CrateMeta, &Failure>, lock: Option<&Lock>, offset: usize) -> String {
    let mut out = Vec::new();
    if let Some(feature) = dep.features.iter().find(|f| f.span.contains(&offset) || f.span.end == offset)
        && let Lookup::Found { info, .. } = lookup
    {
        let req = dep.req.as_ref().and_then(|r| parse_req(&r.text));
        return match info.features_for(req.as_ref()).and_then(|f| f.get(&feature.name)) {
            Some(enables) if enables.is_empty() => tf("feature {0} of {1}: enables nothing else", &[&feature.name, &dep.package]),
            Some(enables) => tf("feature {0} of {1}: enables {2}", &[&feature.name, &dep.package, &enables.join(", ")]),
            None => tf("{1} has no feature named {0}", &[&feature.name, &dep.package]),
        };
    }
    let title = match (&dep.source, lookup) {
        (_, Lookup::Found { info, .. }) => match info.latest() {
            Some(latest) => format!("{} {}", dep.package, latest.version),
            None => dep.package.clone(),
        },
        _ => dep.package.clone(),
    };
    out.push(title);
    match &dep.source {
        Source::Workspace => out.push(t("The version comes from [workspace.dependencies].").into()),
        Source::Path => out.push(t("A path dependency: not looked up on crates.io.").into()),
        Source::Git => out.push(t("A git dependency: not looked up on crates.io.").into()),
        Source::Registry(name) => out.push(tf("From the registry “{}”: not looked up.", &[name])),
        Source::CratesIo => {}
    }
    if let Ok(m) = meta {
        out.extend(m.description.clone());
        out.extend(m.license.as_ref().map(|l| tf("License: {}", &[l])));
    }
    if let (Some(req), Lookup::Found { info, .. }) = (&dep.req, lookup) {
        let locked = locked_of(dep, lock);
        let line = match locked {
            Some(l) => tf("Requirement “{0}”, Cargo.lock has {1}", &[&req.text, l]),
            None => tf("Requirement “{}”", &[&req.text]),
        };
        out.push(line);
        if let Some(status) = classify(&req.text, info, locked) {
            out.push(match status {
                Status::UpToDate { latest } => tf("Up to date: the latest release is {}", &[&latest]),
                Status::Outdated { latest } => tf("Update available: {}", &[&latest]),
                Status::Yanked { version } => tf("Version {} was yanked", &[&version]),
                Status::NoMatch { .. } => t("No published version matches the requirement").into(),
            });
        }
    } else if matches!(lookup, Lookup::NotFound) {
        out.push(tf("{}: no crate with this name on crates.io", &[&dep.package]));
    } else if matches!(lookup, Lookup::Offline) {
        out.push(t("offline").into());
    }
    if let Ok(m) = meta {
        out.push(tf("Documentation: {}", &[&m.documentation]));
        out.extend(m.repository.as_ref().map(|r| tf("Repository: {}", &[r])));
        out.extend(m.homepage.as_ref().filter(|h| Some(*h) != m.repository.as_ref()).map(|h| tf("Homepage: {}", &[h])));
    } else if dep.is_registry_lookup() {
        out.push(tf("Documentation: {}", &[&format!("https://docs.rs/{}", dep.package)]));
    }
    out.join("\n")
}
