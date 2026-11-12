//! Cargo is authoritative for workspace members, target names, required features and executable paths.
//! Discovery runs only after workspace trust; commands use argv through Host, never a shell.

use host::Host;
use serde_json::Value;

/// Dependency intelligence for `Cargo.toml` (hints, completion, hover, updates).
#[path = "cargo_deps.rs"]
pub mod deps;

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
    /// The target's `required-features`.
    pub features: Vec<String>,
    /// The user's features, profile and output folder, set when the build starts.
    pub build: BuildOptions,
}

/// The `rust.*` settings: how Cargo builds what is run or debugged (`rust.features`, `rust.noDefaultFeatures`,
/// `rust.profile`, `rust.targetDir`, `rust.target`), what runs tests, and how rust-analyzer's hints look. Cargo resolves a relative folder against the workspace root, on the machine that builds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildOptions {
    pub features: Vec<String>,
    pub no_default_features: bool,
    pub profile: Option<String>,
    pub target_dir: Option<String>,
    /// `rust.target`: the target triple to build for (cross-compilation).
    pub target: Option<String>,
    /// `rust.testRunner`: `cargo test` or cargo-nextest for tests.
    pub test_runner: crate::rust_toolchain::nextest::TestRunner,
    /// `rust.inlayHints.*`: which kinds of hints rust-analyzer sends.
    pub hints: crate::rust_toolchain::hints::HintKinds,
    /// `rust.semanticModifiers`: which token modifiers are drawn.
    pub modifiers: crate::rust_toolchain::hints::Modifiers,
}

impl BuildOptions {
    /// Short description for the configuration picker, e.g. "release · features: cli, tls"; `None` for Cargo's defaults.
    pub fn summary(&self) -> Option<String> {
        let mut parts: Vec<String> = self.profile.iter().cloned().collect();
        if !self.features.is_empty() {
            parts.push(crate::i18n::tf("features: {}", &[&self.features.join(", ")]));
        }
        if self.no_default_features {
            parts.push(crate::i18n::t("no default features").into());
        }
        if let Some(dir) = &self.target_dir {
            parts.push(crate::i18n::tf("output: {}", &[dir]));
        }
        if let Some(target) = &self.target {
            parts.push(crate::i18n::tf("target: {}", &[target]));
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }
}

impl CargoTarget {
    pub fn binary(name: String) -> Self {
        Self { package: name.clone(), name, manifest: None, kind: TargetKind::Binary, features: Vec::new(), build: BuildOptions::default() }
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
        let mut features = self.features.clone();
        features.extend(self.build.features.iter().filter(|f| !self.features.contains(f)).cloned());
        if !features.is_empty() {
            args.extend(["--features".into(), features.join(",")]);
        }
        if self.build.no_default_features {
            args.push("--no-default-features".into());
        }
        if let Some(profile) = &self.build.profile {
            args.extend(["--profile".into(), profile.clone()]);
        }
        if let Some(dir) = &self.build.target_dir {
            args.extend(["--target-dir".into(), dir.clone()]);
        }
        if let Some(target) = &self.build.target {
            args.extend(["--target".into(), target.clone()]);
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

/// What to do when `cargo` could not be started or rustup could not pick a toolchain; `None` when the failure is
/// something else and the tool's own text is the best explanation.
pub fn toolchain_hint(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let missing = lower.contains("no such file or directory")
        || lower.contains("program not found")
        || lower.contains("cannot find the file")
        || lower.contains("cannot find the path");
    if missing && lower.contains("cargo") {
        return Some(
            crate::i18n::t("Cargo was not found. Install Rust from https://rustup.rs and make sure `cargo` is on the PATH of this machine.").to_string(),
        );
    }
    if lower.contains("rustup could not choose a version") || lower.contains("no default toolchain") || lower.contains("override and no default") {
        return Some(crate::i18n::t("No Rust toolchain is selected. Run `rustup default stable` in a terminal.").to_string());
    }
    if lower.contains("toolchain") && lower.contains("is not installed") {
        return Some(crate::i18n::t("The project's Rust toolchain is not installed. Run `rustup toolchain install` in the project folder.").to_string());
    }
    None
}

/// A failed `cargo` run as one message: the hint first when there is one, then the text itself.
pub fn explain_failure(text: &str) -> String {
    match toolchain_hint(text) {
        Some(hint) => format!("{hint}\n{}", text.trim()),
        None => text.trim().to_string(),
    }
}

pub fn discover(host: &dyn Host, root: &str) -> Result<Vec<CargoTarget>, String> {
    let output =
        host.exec("cargo", &["metadata", "--no-deps", "--format-version=1", "--offline"], Some(root), None).map_err(|e| explain_failure(&e.to_string()))?;
    if !output.ok() {
        return Err(explain_failure(&format!("{}{}", output.stdout_str(), output.stderr_str())));
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
            let target =
                CargoTarget { package: name.into(), name: target_name.into(), manifest: Some(manifest.into()), kind, features, build: BuildOptions::default() };
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
                pre_launch_task: None,
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
