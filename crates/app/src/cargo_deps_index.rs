//! The crates.io sparse index: where a crate's file is, how its lines (one JSON object per published version) become a
//! `CrateInfo`, and the compact form the on-disk cache keeps. Parsing is tolerant: a line that is not understood is skipped.

use std::collections::BTreeMap;

use semver::{Version, VersionReq};
use serde_json::{Value, json};

/// Feature lists are kept for this many newest versions only: older ones are never offered, and a crate like `tokio`
/// would otherwise cost about a megabyte per cache file.
const FEATURE_VERSIONS_KEPT: usize = 30;

pub type Features = BTreeMap<String, Vec<String>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionInfo {
    pub version: Version,
    pub yanked: bool,
    pub rust_version: Option<String>,
    /// `None` for versions older than the newest `FEATURE_VERSIONS_KEPT`.
    pub features: Option<Features>,
}

/// What the index says about one crate: every published version, oldest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrateInfo {
    pub name: String,
    pub versions: Vec<VersionInfo>,
}

impl CrateInfo {
    /// The newest version that is not yanked and not a pre-release.
    pub fn latest_stable(&self) -> Option<&VersionInfo> {
        self.versions.iter().rev().find(|v| !v.yanked && v.version.pre.is_empty())
    }

    /// The newest version that is not yanked, a pre-release if nothing else exists (what `cargo add` falls back to).
    pub fn latest(&self) -> Option<&VersionInfo> {
        self.latest_stable().or_else(|| self.versions.iter().rev().find(|v| !v.yanked))
    }

    pub fn find(&self, version: &Version) -> Option<&VersionInfo> {
        self.versions.iter().find(|v| &v.version == version)
    }

    /// Newest first, stable releases before pre-releases of the same or a newer series, yanked ones last.
    pub fn offered(&self) -> Vec<&VersionInfo> {
        let mut list: Vec<&VersionInfo> = self.versions.iter().collect();
        list.sort_by(|a, b| {
            let key = |v: &VersionInfo| (!v.yanked, v.version.pre.is_empty());
            key(b).cmp(&key(a)).then_with(|| b.version.cmp(&a.version))
        });
        list
    }

    /// The feature list of the newest non-yanked version `req` allows (any when there is none), else of the latest.
    pub fn features_for(&self, req: Option<&VersionReq>) -> Option<&Features> {
        let with_features = |v: &&VersionInfo| !v.yanked && v.features.is_some();
        let matching = req.and_then(|r| self.versions.iter().rev().filter(with_features).find(|v| r.matches(&v.version)));
        matching.or_else(|| self.versions.iter().rev().find(with_features)).and_then(|v| v.features.as_ref())
    }
}

/// Where the index keeps `name` (lower-cased): `1/a`, `2/ab`, `3/a/abc`, `ab/cd/abcd…`.
pub fn index_path(name: &str) -> String {
    let n = name.to_ascii_lowercase();
    match n.len() {
        0 => n,
        1 | 2 => format!("{}/{n}", n.len()),
        3 => format!("3/{}/{n}", &n[..1]),
        _ => format!("{}/{}/{n}", &n[..2], &n[2..4]),
    }
}

/// The features of one index line, with the implicit feature Cargo gives every optional dependency that no feature
/// names as `dep:…`.
fn feature_table(line: &Value) -> Features {
    let mut table = Features::new();
    for key in ["features", "features2"] {
        if let Some(map) = line[key].as_object() {
            for (name, enables) in map {
                let list = enables.as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
                table.insert(name.clone(), list);
            }
        }
    }
    let named: Vec<String> = table.values().flatten().filter_map(|e| e.strip_prefix("dep:").map(str::to_string)).collect();
    for dep in line["deps"].as_array().into_iter().flatten() {
        let (Some(name), true) = (dep["name"].as_str(), dep["optional"].as_bool() == Some(true)) else { continue };
        if !named.iter().any(|n| n == name) && !table.contains_key(name) {
            table.insert(name.to_string(), vec![format!("dep:{name}")]);
        }
    }
    table
}

/// Parse the body of a crate's index file.
pub fn parse_index(name: &str, body: &str) -> CrateInfo {
    let mut versions: Vec<VersionInfo> = body
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|line| {
            let version = Version::parse(line["vers"].as_str()?).ok()?;
            Some(VersionInfo {
                version,
                yanked: line["yanked"].as_bool().unwrap_or(false),
                rust_version: line["rust_version"].as_str().map(str::to_string),
                features: Some(feature_table(&line)),
            })
        })
        .collect();
    versions.sort_by(|a, b| a.version.cmp(&b.version));
    versions.dedup_by(|a, b| a.version == b.version);
    let drop_before = versions.len().saturating_sub(FEATURE_VERSIONS_KEPT);
    for v in &mut versions[..drop_before] {
        v.features = None;
    }
    CrateInfo { name: name.to_string(), versions }
}

/// The cache's form: `[[version, yanked, rust_version], …]` and the feature lists by version.
pub fn to_json(info: &CrateInfo) -> Value {
    let versions: Vec<Value> = info.versions.iter().map(|v| json!([v.version.to_string(), v.yanked, v.rust_version])).collect();
    let features: serde_json::Map<String, Value> = info.versions.iter().filter_map(|v| Some((v.version.to_string(), json!(v.features.as_ref()?)))).collect();
    json!({ "name": info.name, "versions": versions, "features": features })
}

pub fn from_json(value: &Value) -> Option<CrateInfo> {
    let name = value["name"].as_str()?.to_string();
    let mut versions = Vec::new();
    for row in value["versions"].as_array()? {
        let version = Version::parse(row[0].as_str()?).ok()?;
        let features = value["features"][version.to_string()].as_object().map(|map| {
            map.iter().map(|(k, v)| (k.clone(), v.as_array().into_iter().flatten().filter_map(|e| e.as_str().map(str::to_string)).collect())).collect()
        });
        versions.push(VersionInfo { version, yanked: row[1].as_bool()?, rust_version: row[2].as_str().map(str::to_string), features });
    }
    Some(CrateInfo { name, versions })
}
