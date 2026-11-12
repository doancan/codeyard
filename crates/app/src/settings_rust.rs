//! The `rust.*` settings: how Run and Debug build with Cargo (read here so `settings.rs` stays within its size budget).

use super::{Reader, Settings};

/// The settings of this file besides the ones in `settings.rs`'s own list: keys that are not these are reported as unknown.
pub(super) const KEYS: [&str; 6] =
    ["rust.target", "rust.testRunner", "rust.inlayHints.types", "rust.inlayHints.parameters", "rust.inlayHints.chaining", "rust.semanticModifiers"];

pub(super) fn parse(s: &mut Settings, r: &mut Reader<'_>) {
    let b = &mut s.rust_build;
    if let Some(v) = r.strings("rust.features") {
        b.features = v.into_iter().map(|f| f.trim().to_string()).filter(|f| !f.is_empty()).collect();
    }
    if let Some(v) = r.bool("rust.noDefaultFeatures") {
        b.no_default_features = v;
    }
    // Cargo's names: `dev` (default), `release` or a custom `[profile.*]`; empty means Cargo's default.
    b.profile = r.string("rust.profile").map(|p| p.trim().to_string());
    b.target_dir = r.string("rust.targetDir").map(|d| d.trim().to_string());
    if let Some(v) = r.choice("rust.testRunner", &crate::rust_toolchain::nextest::TestRunner::CHOICES) {
        b.test_runner = crate::rust_toolchain::nextest::TestRunner::parse(&v).unwrap_or_default();
    }
    for (key, kind) in [
        ("rust.inlayHints.types", &mut b.hints.types),
        ("rust.inlayHints.parameters", &mut b.hints.parameters),
        ("rust.inlayHints.chaining", &mut b.hints.chaining),
    ] {
        if let Some(v) = r.bool(key) {
            *kind = v;
        }
    }
    if let Some(names) = r.strings("rust.semanticModifiers") {
        let (modifiers, unknown) = crate::rust_toolchain::hints::Modifiers::parse(&names);
        for name in unknown {
            r.warn(crate::i18n::tf(
                "rust.semanticModifiers: unknown modifier {} ({})",
                &[&format!("\"{name}\""), &crate::rust_toolchain::hints::Modifiers::NAMES.join(", ")],
            ));
        }
        b.modifiers = modifiers;
    }
    // A target triple; anything that is not one is reported and ignored, so it never reaches a command line.
    b.target = r.string("rust.target").map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).filter(|t| {
        let ok = crate::rust_toolchain::target::is_valid_triple(t);
        if !ok {
            r.warn(crate::i18n::tf("rust.target: {} is not a target triple (letters, digits, - _ .)", &[&t]));
        }
        ok
    });
    if let Some(v) = r.bool("rust.cargo.dependencyLookups") {
        s.rust_dependency_lookups = v;
    }
}

/// Hands the inlay hint kinds that are switched off to rust-analyzer as its own settings, unless the user's `lsp.servers.rust`
/// says otherwise. Runs after `lsp.servers` is read. (An absent Rust entry and an empty one are the same server; see
/// `servers_reload`, which is why switching a kind on or off reaches the running server without restarting any.)
pub(super) fn apply_hint_settings(s: &mut Settings) {
    let kinds = s.rust_build.hints;
    if kinds == crate::rust_toolchain::hints::HintKinds::default() {
        return;
    }
    match s.lsp_servers.iter_mut().rev().find(|(language, _)| language == "rust") {
        // The Rust server is turned off: there is nobody to tell.
        Some((_, None)) => {}
        Some((_, Some(spec))) => spec.settings = std::sync::Arc::new(kinds.applied_to(&spec.settings)),
        None => s.lsp_servers.push((
            "rust".into(),
            Some(crate::lsp::ServerSpec { settings: std::sync::Arc::new(kinds.applied_to(&serde_json::Value::Null)), ..Default::default() }),
        )),
    }
}
