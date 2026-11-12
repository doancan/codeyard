//! Which index the user's Cargo configuration points crates.io at. `[source.crates-io] replace-with = "mirror"` with
//! `[source.mirror] registry = "sparse+https://…"` moves the lookups to that mirror (and turns off crates.io's API: a mirror
//! has none); a mirror Codeyard cannot ask (a git index, a vendored directory) turns the lookups off for that project.

use std::path::{Path, PathBuf};

use super::registry::Endpoints;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Endpoints(Endpoints),
    /// crates.io is replaced by something this feature cannot ask.
    Unsupported,
}

/// The first `[source.crates-io] replace-with` found in `configs` (closest to the project first), resolved to an index.
pub fn resolve(configs: &[String]) -> Source {
    let docs: Vec<toml::Table> = configs.iter().filter_map(|c| toml::from_str(c).ok()).collect();
    let source = |doc: &toml::Table, name: &str| doc.get("source").and_then(|s| s.get(name)).cloned();
    let Some(replacement) = docs.iter().find_map(|d| source(d, "crates-io")?.get("replace-with")?.as_str().map(str::to_string)) else {
        return Source::Endpoints(Endpoints::default());
    };
    // The replacement's definition may sit in any file of the chain.
    let Some(def) = docs.iter().find_map(|d| source(d, &replacement)) else { return Source::Unsupported };
    match def.get("registry").and_then(|r| r.as_str()).and_then(|r| r.strip_prefix("sparse+")) {
        // Credentials in the address would end up in a folder name and on the command line of `curl`: not asked.
        Some(url) if url.starts_with("https://") && !url["https://".len()..].split('/').next().unwrap_or_default().contains('@') => {
            Source::Endpoints(Endpoints { index: url.trim_end_matches('/').to_string(), api: None })
        }
        _ => Source::Unsupported,
    }
}

/// `$CARGO_HOME`, else `~/.cargo`.
pub fn cargo_home(env: Option<std::ffi::OsString>, home: Option<PathBuf>) -> Option<PathBuf> {
    env.filter(|e| !e.is_empty()).map(PathBuf::from).or_else(|| home.map(|h| h.join(".cargo")))
}

/// The configuration files that apply to `dir` on this machine, closest first: `.cargo/config.toml` (or the old `config`) of
/// `dir` and its parents when `project` is true, then Cargo home's. Missing files are skipped.
pub fn read_chain(dir: &Path, cargo_home: Option<&Path>, project: bool) -> Vec<String> {
    let mut files = Vec::new();
    let read = |base: &Path| ["config.toml", "config"].iter().find_map(|n| std::fs::read_to_string(base.join(n)).ok());
    if project {
        for ancestor in dir.ancestors() {
            files.extend(read(&ancestor.join(".cargo")));
        }
    }
    if let Some(home) = cargo_home {
        files.extend(read(home));
    }
    files
}
