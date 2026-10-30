//! `cyx publish`, `cyx yank`, `cyx restore`, `cyx deprecate` (docs/extensions/developer-guide.md, "Publish").
//!
//! Publishing checks and packages the folder, signs a statement of its id, version and content digest keyless
//! (`sigstore.rs`), verifies that signature against the registry's own Sigstore roots before anything is sent, and
//! uploads both. The registry runs the same checks again. `--dry-run` stops before signing and prints what users will
//! see on the install sheet, in the same words as the app.

use std::path::Path;

use ext_core::yard::{self, Root, StatusChange};
use ext_manifest::{ExtId, Manifest, Version};

use crate::net;
use crate::sigstore::{self, Endpoints};

pub const DEFAULT_REGISTRY: &str = "https://yard.codeyard.dev";

/// `--registry`, else `CODEYARD_REGISTRY`, else the public registry.
pub fn registry_url(flag: Option<&str>) -> String {
    flag.map(str::to_string)
        .or_else(|| std::env::var("CODEYARD_REGISTRY").ok().filter(|v| !v.is_empty()))
        .unwrap_or_else(|| DEFAULT_REGISTRY.to_string())
        .trim_end_matches('/')
        .to_string()
}

pub struct Publish<'a> {
    pub dir: &'a Path,
    pub registry: &'a str,
    pub dry_run: bool,
    pub pre_release: bool,
}

/// What the install sheet will show for this manifest: permissions in plain words, the red line, the author's reason.
pub fn sheet(m: &Manifest) -> String {
    let mut out = format!("{} {} ({})\n", m.name, m.version, m.id);
    let lines = m.permissions.describe();
    if lines.is_empty() {
        out.push_str("  No permissions: it only adds data to Codeyard.\n");
    }
    for l in lines {
        out.push_str(&format!("  • {l}\n"));
    }
    if let Some(h) = m.risk().headline() {
        out.push_str(&format!("  {h}\n"));
    }
    if let Some(why) = &m.permissions.why {
        out.push_str(&format!("  “{why}” (the author)\n"));
    }
    out
}

/// Publish the folder; the report to print.
pub fn publish(p: &Publish, ep: &Endpoints, token: impl FnOnce() -> Result<String, String>, now: u64) -> Result<String, String> {
    let report = crate::check(p.dir)?;
    let files = ext_pkg::collect_dir(p.dir).map_err(|e| format!("error: {e}"))?;
    let (bytes, digest) = ext_pkg::write(files).map_err(|e| format!("error: {e}"))?;
    let m = ext_pkg::read(&bytes).map_err(|e| format!("error: {e}"))?.manifest;
    let mut notes = report.warnings;
    match (p.pre_release, m.version.pre.is_some()) {
        (true, false) => {
            return Err(format!(
                "error: --pre-release needs a version with a suffix, like {}.{}.{}-beta.1\n   = fix: change `version` in {}",
                m.version.major,
                m.version.minor,
                m.version.patch,
                ext_manifest::FILE_NAME
            ));
        }
        (false, true) => notes.push(format!("note: {} is a pre-release (it has a suffix): users get it only when they opt in to pre-releases", m.version)),
        _ => {}
    }
    // The registry reads the `code` flag of its index from the package (a code extension waits 48 h); nothing to send.
    let code = !m.is_declarative();
    if code {
        notes.push(format!("note: {} runs code: the index marks it as code, and users get each version 48 hours after it is published", m.id));
    }
    let mut out = notes.join("\n");
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    if p.dry_run {
        out.push_str(&format!(
            "Would publish {} {} to {} (content digest {}).\n\nThe install sheet will show:\n{}\nNothing was signed or sent.",
            m.id,
            m.version,
            p.registry,
            digest.hex(),
            sheet(&m)
        ));
        return Ok(out);
    }
    let pins = registry_pins(p.registry)?;
    let statement = ext_core::publisher::statement(&m.id, &m.version, &digest);
    let bundle = sigstore::sign(&statement, &token()?, ep)?;
    let signed = ext_core::publisher::verify_package(&bundle, &m.id, &m.version, &digest, &pins, now)
        .map_err(|e| format!("{e}\n   = note: the signature does not verify against the registry's Sigstore roots; nothing was uploaded"))?;
    let res = net::request("POST", &format!("{}/v1/publish", p.registry), &[("content-type", "application/octet-stream")], yard::frame(&bundle, &bytes))?;
    if !res.ok() {
        let why = res.text();
        // Phase 1 registries take themes, snippets, keymaps and languages only (docs/registry.md).
        if code && why.contains("accepts data-only extensions") {
            return Err(format!(
                "error: {} accepts data-only extensions; {} {} runs code, so it was not published\n   = note: code extensions open on the registry in a later phase; until then share the .cyx from `cyx package`, which installs from a file",
                p.registry, m.id, m.version
            ));
        }
        return Err(format!("error: the registry refused {} {}: {why}", m.id, m.version));
    }
    let published: serde_json::Value = serde_json::from_slice(&res.body).map_err(|e| format!("error: the registry's answer: {e}"))?;
    let available = published["available"].as_u64().unwrap_or(now);
    // The registry's clock decides; a few minutes either way is still "now".
    let when = if available <= now + 600 { "now".to_string() } else { format!("in {} hours (code waits 48 h)", (available - now).div_ceil(3600)) };
    out.push_str(&format!("Published {} {} as {}.\nUsers get it {when}.", m.id, m.version, signed.publisher));
    Ok(out)
}

/// Yank, restore or deprecate a version of the folder's extension.
pub fn change_status(
    dir: &Path,
    registry: &str,
    version: &Version,
    change: StatusChange,
    ep: &Endpoints,
    token: impl FnOnce() -> Result<String, String>,
    now: u64,
) -> Result<String, String> {
    let text = std::fs::read_to_string(dir.join(ext_manifest::FILE_NAME)).map_err(|e| format!("error: cannot read {}: {e}", ext_manifest::FILE_NAME))?;
    let id: ExtId = ext_manifest::parse_with(&text, ext_manifest::Options { require_why: false })
        .map_err(|_| "error: run `cyx check` first; the manifest is not valid")?
        .id;
    let pins = registry_pins(registry)?;
    let statement = change.statement(&id, version);
    let bundle = sigstore::sign(&statement, &token()?, ep)?;
    ext_core::publisher::verify(&bundle, &statement, &pins, now)
        .map_err(|e| format!("{e}\n   = note: the signature does not verify against the registry's Sigstore roots; nothing was sent"))?;
    let res = net::request("POST", &format!("{registry}/v1/status"), &[("content-type", "application/octet-stream")], yard::frame(&bundle, &statement))?;
    if !res.ok() {
        return Err(format!("error: the registry refused: {}", res.text()));
    }
    Ok(match change {
        StatusChange::Yank => format!("Yanked {id} {version}: hidden from installs; users on it go back to the version before."),
        StatusChange::Restore => format!("Restored {id} {version}."),
        StatusChange::Deprecate { replacement: Some(r) } => format!("Deprecated {id} {version}; users are pointed to {r}."),
        StatusChange::Deprecate { replacement: None } => format!("Deprecated {id} {version}."),
    })
}

/// The Sigstore roots the registry pins, so a signature that would be refused is caught before upload.
fn registry_pins(registry: &str) -> Result<ext_core::publisher::Pins, String> {
    let res = net::request("GET", &format!("{registry}/{}", yard::ROOT_FILE), &[], Vec::new())?;
    if !res.ok() {
        return Err(format!("error: cannot read the registry at {registry} ({})", res.status));
    }
    let root: Root = serde_json::from_slice(&res.body).map_err(|e| format!("error: {registry}/{}: {e}", yard::ROOT_FILE))?;
    ext_core::publisher::Pins::from_pem(&root.fulcio, &root.rekor).map_err(|e| e.to_string())
}

#[cfg(test)]
#[path = "publish_tests.rs"]
mod tests;
