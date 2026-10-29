//! Managed language service catalogue. Installation is explicit and independent of workspace files.

use std::collections::HashMap;

use serde_json::Value;
use sha2::{Digest, Sha256};

pub const PACKAGE_JSON: &str = include_str!("../resources/language-services/package.json");
pub const PACKAGE_LOCK: &str = include_str!("../resources/language-services/package-lock.json");
const MAX_LOCK: usize = 256 * 1024;
const MAX_PACKAGES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    TypeScript,
    Html,
    Css,
}

impl Service {
    pub fn initialization_options(self, options: Option<&Value>) -> Option<Value> {
        if self == Self::TypeScript {
            return options.cloned();
        }
        let mut options = options.and_then(Value::as_object).cloned().unwrap_or_default();
        options.entry("provideFormatter").or_insert(Value::Bool(true));
        Some(Value::Object(options))
    }

    /// The HTML/CSS servers read requested configuration sections as objects. Missing/null
    /// sections must use their empty defaults; passing null can disable diagnostics upstream.
    pub fn configuration(self, settings: &Value) -> Value {
        let sections: &[&str] = match self {
            Self::Html => &["css", "html", "javascript", "js/ts"],
            Self::Css => &["css", "scss", "less"],
            Self::TypeScript => return settings.clone(),
        };
        let mut settings = settings.as_object().cloned().unwrap_or_default();
        for section in sections {
            let entry = settings.entry((*section).to_string()).or_insert_with(|| serde_json::json!({}));
            if entry.is_null() {
                *entry = serde_json::json!({});
            }
        }
        Value::Object(settings)
    }

    pub fn for_language(language: &str) -> Option<Self> {
        match language {
            "typescript" => Some(Self::TypeScript),
            "html" => Some(Self::Html),
            "css" => Some(Self::Css),
            _ => None,
        }
    }
    pub fn entry(self) -> &'static str {
        match self {
            Self::TypeScript => "node_modules/typescript-language-server/lib/cli.mjs",
            Self::Html => "node_modules/vscode-langservers-extracted/bin/vscode-html-language-server",
            Self::Css => "node_modules/vscode-langservers-extracted/bin/vscode-css-language-server",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    pub fingerprint: String,
    pub packages: usize,
    pub versions: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogError {
    InvalidManifest,
    InvalidLock,
    UnsafePackage,
    IntegrityMissing,
    DependencyMismatch,
}

fn exact_version(value: &str) -> bool {
    if value.len() > 128 {
        return false;
    }
    let identifiers = |text: &str, prerelease: bool| {
        text.split('.').all(|part| {
            !part.is_empty()
                && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && (!prerelease || !part.bytes().all(|b| b.is_ascii_digit()) || part.len() == 1 || !part.starts_with('0'))
        })
    };
    let main = match value.split_once('+') {
        Some((main, build)) if identifiers(build, false) => main,
        Some(_) => return false,
        None => value,
    };
    let main = match main.split_once('-') {
        Some((main, pre)) if identifiers(pre, true) => main,
        Some(_) => return false,
        None => main,
    };
    let parts: Vec<_> = main.split('.').collect();
    parts.len() == 3 && parts.iter().all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()) && (part.len() == 1 || !part.starts_with('0')))
}

fn package_path(value: &str) -> bool {
    value.starts_with("node_modules/") && !value.contains(['\\', '\0']) && value.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
}

fn integrity(value: &str) -> bool {
    let Some(value) = value.strip_prefix("sha512-") else { return false };
    value.len() == 88 && value.ends_with("==") && value.as_bytes()[..86].iter().copied().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
}

impl Catalog {
    pub fn bundled() -> Result<Self, CatalogError> {
        Self::parse(PACKAGE_JSON, PACKAGE_LOCK)
    }

    pub fn parse(manifest: &str, lock: &str) -> Result<Self, CatalogError> {
        if manifest.len() > 16 * 1024 || lock.len() > MAX_LOCK {
            return Err(CatalogError::InvalidLock);
        }
        let manifest: Value = serde_json::from_str(manifest).map_err(|_| CatalogError::InvalidManifest)?;
        if manifest.get("private") != Some(&Value::Bool(true)) || manifest.get("scripts").is_some() || manifest.get("devDependencies").is_some() {
            return Err(CatalogError::InvalidManifest);
        }
        let dependencies = manifest.get("dependencies").and_then(Value::as_object).ok_or(CatalogError::InvalidManifest)?;
        let required = ["typescript", "typescript-language-server", "vscode-langservers-extracted"];
        if dependencies.len() != required.len() || required.iter().any(|key| !dependencies.get(*key).and_then(Value::as_str).is_some_and(exact_version)) {
            return Err(CatalogError::InvalidManifest);
        }
        let parsed: Value = serde_json::from_str(lock).map_err(|_| CatalogError::InvalidLock)?;
        if parsed.get("lockfileVersion").and_then(Value::as_u64) != Some(3) {
            return Err(CatalogError::InvalidLock);
        }
        let packages = parsed.get("packages").and_then(Value::as_object).ok_or(CatalogError::InvalidLock)?;
        if packages.len() <= required.len() || packages.len() > MAX_PACKAGES {
            return Err(CatalogError::InvalidLock);
        }
        if packages.get("").and_then(|v| v.get("dependencies")) != Some(&Value::Object(dependencies.clone())) {
            return Err(CatalogError::DependencyMismatch);
        }
        for (path, package) in packages.iter().filter(|(path, _)| !path.is_empty()) {
            let resolved = package.get("resolved").and_then(Value::as_str).ok_or(CatalogError::UnsafePackage)?;
            let suffix = resolved.strip_prefix("https://registry.npmjs.org/").ok_or(CatalogError::UnsafePackage)?;
            if !package_path(path)
                || package.get("link").is_some()
                || suffix.is_empty()
                || !suffix.ends_with(".tgz")
                || !suffix.bytes().all(|b| b.is_ascii_alphanumeric() || b"/@._-".contains(&b))
                || suffix.contains(['\\', '\0', '?', '#', '%'])
                || suffix.split('/').any(|part| part.is_empty() || part == ".." || part == ".")
                || !package.get("version").and_then(Value::as_str).is_some_and(exact_version)
            {
                return Err(CatalogError::UnsafePackage);
            }
            if !package.get("integrity").and_then(Value::as_str).is_some_and(integrity) {
                return Err(CatalogError::IntegrityMissing);
            }
        }
        let mut versions = HashMap::new();
        for name in required {
            let expected = dependencies[name].as_str().ok_or(CatalogError::InvalidManifest)?;
            if packages.get(&format!("node_modules/{name}")).and_then(|p| p.get("version")).and_then(Value::as_str) != Some(expected) {
                return Err(CatalogError::DependencyMismatch);
            }
            versions.insert(name.into(), expected.into());
        }
        let fingerprint = Sha256::digest(lock.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect();
        Ok(Self { fingerprint, packages: packages.len() - 1, versions })
    }
}

/// The pinned TypeScript service requires Node >=22.22.2. Reject malformed and prerelease values.
pub fn supported_node(version: &str) -> bool {
    let version = version.trim().strip_prefix('v').unwrap_or(version.trim());
    if !exact_version(version) || version.contains(['-', '+']) {
        return false;
    }
    let parts: Vec<_> = version.split('.').filter_map(|part| part.parse::<u32>().ok()).collect();
    parts.len() == 3 && (parts[0], parts[1], parts[2]) >= (22, 22, 2)
}

#[cfg(test)]
#[path = "language_services_tests.rs"]
mod tests;
