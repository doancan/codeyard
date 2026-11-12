//! The crates.io versions a `Cargo.lock` resolved, for the hover and for flagging a locked version that was yanked.

use std::collections::HashMap;

use semver::{Version, VersionReq};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lock {
    versions: HashMap<String, Vec<Version>>,
}

/// A package of the default registry; path, git and other registries' packages never get a hint.
fn from_crates_io(source: &str) -> bool {
    source.ends_with("github.com/rust-lang/crates.io-index") || source == "sparse+https://index.crates.io/"
}

impl Lock {
    pub fn parse(text: &str) -> Option<Self> {
        let doc: toml::Table = toml::from_str(text).ok()?;
        let mut versions: HashMap<String, Vec<Version>> = HashMap::new();
        for package in doc.get("package")?.as_array()? {
            let (Some(name), Some(version), Some(source)) = (package.get("name"), package.get("version"), package.get("source")) else { continue };
            let (Some(name), Some(version), Some(source)) = (name.as_str(), version.as_str(), source.as_str()) else { continue };
            if let (true, Ok(v)) = (from_crates_io(source), Version::parse(version)) {
                versions.entry(name.to_string()).or_default().push(v);
            }
        }
        Some(Self { versions })
    }

    /// The locked version a requirement resolved to: the highest of the locked ones it allows (a workspace can lock two
    /// majors of a crate).
    pub fn resolved(&self, package: &str, req: Option<&VersionReq>) -> Option<&Version> {
        let all = self.versions.get(package)?;
        match req {
            Some(r) => all.iter().filter(|v| r.matches(v)).max(),
            None if all.len() == 1 => all.first(),
            None => None,
        }
    }
}
