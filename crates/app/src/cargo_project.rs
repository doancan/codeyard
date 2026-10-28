//! Cargo is authoritative for workspace members, target names, required features and executable paths.
//! Discovery runs only after workspace trust; commands use argv through Host, never a shell.

use host::Host;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrateContext {
    pub manifest: String,
    pub relative: String,
}

/// Resolve the nearest manifest from an already listed workspace without an extra host call per source file.
pub fn context_from_files(rel: &str, manifests: &std::collections::HashSet<String>) -> Option<CrateContext> {
    let mut dir = rel.rsplit_once('/').map_or("", |(d, _)| d);
    loop {
        let manifest = if dir.is_empty() { "Cargo.toml".to_string() } else { format!("{dir}/Cargo.toml") };
        if manifests.contains(&manifest) {
            let relative = if dir.is_empty() { rel.to_string() } else { rel.strip_prefix(&format!("{dir}/"))?.to_string() };
            return Some(CrateContext { manifest, relative });
        }
        if dir.is_empty() {
            return None;
        }
        dir = dir.rsplit_once('/').map_or("", |(parent, _)| parent);
    }
}

pub fn context_on_host(host: &dyn Host, root: &str, rel: &str) -> Option<CrateContext> {
    let manifest = nearest_manifest(host, &crate::ui::join(root, rel), Some(root))?;
    let manifest = manifest.strip_prefix(&format!("{}/", root.trim_end_matches('/')))?.to_string();
    context_from_files(rel, &[manifest].into())
}

/// Read-only lookup bounded by the file's workspace. A standalone file checks only its immediate folder.
pub fn nearest_manifest(host: &dyn Host, path: &str, root: Option<&str>) -> Option<String> {
    let (parent, _) = path.rsplit_once('/')?;
    let stop = root.unwrap_or(parent).trim_end_matches('/');
    if parent != stop && !parent.starts_with(&format!("{stop}/")) {
        return None;
    }
    let mut dir = parent;
    loop {
        let candidate = crate::ui::join(dir, "Cargo.toml");
        if host.stat(&candidate).is_ok_and(|m| matches!(m.kind, proto::Kind::File | proto::Kind::Symlink)) {
            return Some(candidate);
        }
        if dir == stop {
            return None;
        }
        dir = dir.rsplit_once('/')?.0;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TargetKind {
    Binary,
    Example,
    LibraryTest,
    IntegrationTest,
}

impl TargetKind {
    fn cargo_kind(self) -> &'static str {
        match self {
            Self::Binary => "bin",
            Self::Example => "example",
            Self::LibraryTest => "lib",
            Self::IntegrationTest => "test",
        }
    }

    pub fn is_test(self) -> bool {
        matches!(self, Self::LibraryTest | Self::IntegrationTest)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CargoTarget {
    pub package: String,
    pub name: String,
    pub manifest: Option<String>,
    pub kind: TargetKind,
    pub features: Vec<String>,
}

impl CargoTarget {
    pub fn binary(name: String) -> Self {
        Self { package: name.clone(), name, manifest: None, kind: TargetKind::Binary, features: Vec::new() }
    }

    pub fn build_args(&self) -> Vec<String> {
        let mut args = vec![if self.kind.is_test() { "test" } else { "build" }.into(), "--message-format=json".into()];
        if let Some(manifest) = &self.manifest {
            args.extend(["--manifest-path".into(), manifest.clone()]);
        }
        match self.kind {
            TargetKind::LibraryTest => args.extend(["--lib".into(), "--no-run".into()]),
            kind => {
                args.extend([format!("--{}", kind.cargo_kind()), self.name.clone()]);
                if kind.is_test() {
                    args.push("--no-run".into());
                }
            }
        }
        if !self.features.is_empty() {
            args.extend(["--features".into(), self.features.join(",")]);
        }
        args
    }

    pub fn accepts_artifact(&self, message: &Value) -> bool {
        message["reason"] == "compiler-artifact"
            && message["target"]["name"] == self.name
            && message["target"]["kind"].as_array().is_some_and(|kinds| {
                kinds.iter().any(|kind| kind == self.kind.cargo_kind() || self.kind == TargetKind::LibraryTest && kind.as_str().is_some_and(is_library_kind))
            })
            && (message["profile"]["test"] == true) == self.kind.is_test()
    }
}

pub fn discover(host: &dyn Host, root: &str) -> Result<Vec<CargoTarget>, String> {
    let output = host.exec("cargo", &["metadata", "--no-deps", "--format-version=1", "--offline"], Some(root), None).map_err(|e| e.to_string())?;
    if !output.ok() {
        return Err(format!("{}{}", output.stdout_str(), output.stderr_str()).trim().to_string());
    }
    parse_metadata(&output.stdout_str())
}

pub fn parse_metadata(text: &str) -> Result<Vec<CargoTarget>, String> {
    let metadata: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let members = metadata["workspace_members"].as_array().ok_or_else(|| crate::i18n::t("Cargo metadata has no workspace members").to_string())?;
    let packages = metadata["packages"].as_array().ok_or_else(|| crate::i18n::t("Cargo metadata has no packages").to_string())?;
    let mut targets = Vec::new();
    let mut tests = Vec::new();
    for package in packages.iter().filter(|package| members.contains(&package["id"])) {
        let name = package["name"].as_str().filter(|s| !s.is_empty()).ok_or_else(|| crate::i18n::t("Cargo package has no name").to_string())?;
        let manifest =
            package["manifest_path"].as_str().filter(|s| !s.is_empty()).ok_or_else(|| crate::i18n::t("Cargo package has no manifest path").to_string())?;
        let declarations = package["targets"].as_array().ok_or_else(|| crate::i18n::t("Cargo package has no targets").to_string())?;
        for target in declarations {
            let Some(kind) = target["kind"].as_array().and_then(|kinds| {
                if kinds.iter().any(|k| k == "bin") {
                    Some(TargetKind::Binary)
                } else if kinds.iter().any(|k| k == "example") {
                    Some(TargetKind::Example)
                } else if target["test"] != false && kinds.iter().any(|k| k.as_str().is_some_and(is_library_kind)) {
                    Some(TargetKind::LibraryTest)
                } else if target["test"] != false && kinds.iter().any(|k| k == "test") {
                    Some(TargetKind::IntegrationTest)
                } else {
                    None
                }
            }) else {
                continue;
            };
            let target_name = target["name"].as_str().filter(|s| !s.is_empty()).ok_or_else(|| crate::i18n::t("Cargo target has no name").to_string())?;
            let features =
                target["required-features"].as_array().map(|a| a.iter().filter_map(|f| f.as_str().map(str::to_string)).collect()).unwrap_or_default();
            let target = CargoTarget { package: name.into(), name: target_name.into(), manifest: Some(manifest.into()), kind, features };
            if kind.is_test() { tests.push(target) } else { targets.push(target) }
        }
    }
    // F5 prefers runnable programs. A library-only workspace offers its test harnesses instead of inventing a binary.
    if targets.is_empty() {
        targets = tests;
    }
    targets.sort_by(|a, b| (&a.package, a.kind, &a.name).cmp(&(&b.package, b.kind, &b.name)));
    targets.dedup();
    Ok(targets)
}

pub fn launch_configs(targets: Vec<CargoTarget>, root: &str) -> Vec<crate::debug_config::LaunchConfig> {
    use crate::debug_config::{LaunchConfig, adapter_for};
    let many = targets.len() > 1;
    targets
        .into_iter()
        .map(|target| {
            let name = match target.kind {
                TargetKind::Binary if !many => crate::i18n::tf("Debug Rust: {}", &[&target.name]),
                TargetKind::Binary => crate::i18n::tf("Debug Rust: {0} / {1}", &[&target.package, &target.name]),
                TargetKind::Example => crate::i18n::tf("Debug Rust example: {0} / {1}", &[&target.package, &target.name]),
                TargetKind::LibraryTest => crate::i18n::tf("Debug Rust library tests: {0} / {1}", &[&target.package, &target.name]),
                TargetKind::IntegrationTest => crate::i18n::tf("Debug Rust integration test: {0} / {1}", &[&target.package, &target.name]),
            };
            let args = if target.kind.is_test() { vec!["--nocapture", "--test-threads=1"] } else { Vec::new() };
            LaunchConfig {
                name,
                adapter: adapter_for("lldb"),
                request: "launch".into(),
                args: serde_json::json!({"args": args, "cwd": root}),
                pre_launch: None,
                cargo_target: Some(target),
                post_debug: None,
                no_debug: false,
                inputs: Vec::new(),
            }
        })
        .collect()
}

/// Human compiler messages, with Cargo's JSON artifact/progress records removed.
pub fn diagnostics(output: &str) -> String {
    let mut text = String::new();
    for line in output.lines() {
        match serde_json::from_str::<Value>(line) {
            Ok(message) if message["reason"] == "compiler-message" => {
                if let Some(rendered) = message["message"]["rendered"].as_str() {
                    text.push_str(rendered);
                } else if let Some(message) = message["message"]["message"].as_str() {
                    text.push_str(message);
                    text.push('\n');
                }
            }
            Ok(_) => {}
            Err(_) => {
                text.push_str(line);
                text.push('\n');
            }
        }
    }
    text
}

pub fn build_finished(output: &str) -> bool {
    !output.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).any(|m| m["reason"] == "build-finished" && m["success"] == false)
}

fn is_library_kind(kind: &str) -> bool {
    matches!(kind, "lib" | "rlib" | "dylib" | "cdylib" | "staticlib" | "proc-macro")
}

#[cfg(test)]
#[path = "cargo_project_tests.rs"]
mod tests;
