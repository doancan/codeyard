//! `Cargo.toml` dependency intelligence: a hint at the end of each dependency line (latest version, up to date, outdated,
//! yanked, not found, offline), completion of crate names, versions and features, hover with the crate's description and
//! the version `Cargo.lock` resolved, and updates, adds and removals that keep the file's comments and layout.
//!
//! This module is Codeyard-independent: parsing, version logic, the registry client behind the `Fetch` boundary and the
//! text edits. The editor's and the workbench's side are `editor_cargo.rs` and `workbench_cargo.rs`. What is sent over the
//! network is documented in `docs/user-guide.md` (“Cargo dependencies”).

#[path = "cargo_deps_cache.rs"]
pub mod cache;
#[path = "cargo_deps_config.rs"]
pub mod config;
#[path = "cargo_deps_context.rs"]
pub mod context;
#[path = "cargo_deps_edit.rs"]
pub mod edit;
#[path = "cargo_deps_fetch.rs"]
pub mod fetch;
#[path = "cargo_deps_hints.rs"]
pub mod hints;
#[path = "cargo_deps_index.rs"]
pub mod index;
#[path = "cargo_deps_items.rs"]
pub mod items;
#[path = "cargo_deps_lock.rs"]
pub mod lock;
#[path = "cargo_deps_manifest.rs"]
pub mod manifest;
#[path = "cargo_deps_registry.rs"]
pub mod registry;
#[cfg(test)]
#[path = "cargo_deps_tests.rs"]
pub(crate) mod tests;
#[path = "cargo_deps_version.rs"]
pub mod version;

pub use registry::valid_crate_name;

/// `Cargo.toml` of a package or workspace (the lookups and hints apply to nothing else).
pub fn is_manifest(path: &str) -> bool {
    path.rsplit(['/', '\\']).next() == Some("Cargo.toml")
}
