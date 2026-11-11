//! Completion items for `Cargo.toml`: crate names, versions (newest first, releases before pre-releases) and feature names,
//! with what the index knows in the detail and the documentation.

use semver::VersionReq;

use super::index::CrateInfo;
use super::registry::SearchHit;
use crate::i18n::{t, tf};

/// LSP `CompletionItemKind` numbers: module, value, enum member.
pub const KIND_CRATE: u8 = 9;
pub const KIND_VERSION: u8 = 12;
pub const KIND_FEATURE: u8 = 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub label: String,
    /// The text that replaces what was typed.
    pub insert: String,
    pub detail: Option<String>,
    pub doc: Option<String>,
    pub sort: String,
    pub kind: u8,
}

fn closing(closed: bool) -> &'static str {
    if closed { "" } else { "\"" }
}

/// Search hits with what was typed first: the crate of exactly that name, then the names that start with it, then the rest in
/// the order crates.io ranked them (its relevance order alone puts `serde-saphyr` before `serde_json` for `serd`).
pub fn ranked<'a>(hits: &'a [SearchHit], typed: &str) -> Vec<&'a SearchHit> {
    let typed = typed.to_ascii_lowercase();
    let rank = |name: &str| match name.to_ascii_lowercase() {
        n if n == typed => 0,
        n if n.starts_with(&typed) => 1,
        _ => 2,
    };
    let mut ordered: Vec<&SearchHit> = hits.iter().collect();
    ordered.sort_by_key(|h| rank(&h.name));
    ordered
}

/// Crates from a search (`typed` first, see `ranked`). Choosing one writes the whole entry.
pub fn crate_items(hits: &[SearchHit], typed: &str) -> Vec<Item> {
    ranked(hits, typed)
        .into_iter()
        .enumerate()
        .map(|(i, h)| Item {
            label: h.name.clone(),
            insert: format!("{} = \"{}\"", h.name, h.version),
            detail: Some(h.version.clone()),
            doc: (!h.description.is_empty()).then(|| h.description.clone()),
            sort: format!("{i:04}"),
            kind: KIND_CRATE,
        })
        .collect()
}

pub fn version_items(info: &CrateInfo, closed: bool) -> Vec<Item> {
    let latest = info.latest().map(|v| &v.version);
    info.offered()
        .into_iter()
        .enumerate()
        .map(|(i, v)| {
            let detail = if v.yanked {
                t("yanked")
            } else if Some(&v.version) == latest {
                t("latest")
            } else if !v.version.pre.is_empty() {
                t("pre-release")
            } else {
                ""
            };
            Item {
                label: v.version.to_string(),
                insert: format!("{}{}", v.version, closing(closed)),
                detail: (!detail.is_empty()).then(|| detail.to_string()),
                doc: v.rust_version.as_ref().map(|r| tf("Needs Rust {} or newer", &[r])),
                sort: format!("{i:05}"),
                kind: KIND_VERSION,
            }
        })
        .collect()
}

/// The features of the version `req` allows, minus the ones already listed in the array.
pub fn feature_items(info: &CrateInfo, req: Option<&VersionReq>, listed: &[String], closed: bool) -> Vec<Item> {
    let Some(features) = info.features_for(req) else { return Vec::new() };
    let defaults: &[String] = features.get("default").map_or(&[], Vec::as_slice);
    let mut list: Vec<Item> = features
        .iter()
        .filter(|(name, _)| !listed.contains(name))
        .map(|(name, enables)| {
            let optional_dep = matches!(enables.as_slice(), [one] if one.starts_with("dep:"));
            let detail = if defaults.contains(name) {
                Some(t("enabled by default").to_string())
            } else if optional_dep {
                Some(t("optional dependency").to_string())
            } else {
                None
            };
            Item {
                label: name.clone(),
                insert: format!("{name}{}", closing(closed)),
                detail,
                doc: Some(if enables.is_empty() { t("Enables nothing else").to_string() } else { tf("Enables: {}", &[&enables.join(", ")]) }),
                // `default` first, the rest alphabetical.
                sort: format!("{}{name}", if name == "default" { 0 } else { 1 }),
                kind: KIND_FEATURE,
            }
        })
        .collect();
    list.sort_by(|a, b| a.sort.cmp(&b.sort));
    list
}
