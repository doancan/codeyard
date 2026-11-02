//! The `rust.*` settings: how Run and Debug build with Cargo (read here so `settings.rs` stays within its size budget).

use super::{Reader, Settings};

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
}
