//! The glob patterns of settings (`files.exclude`, `search.exclude`, file associations): `*`, `?` and `**`. The matcher
//! is the extension platform's (`ext_manifest::glob`, linear in time since a pattern can come from a project file), so
//! the wildcards mean the same here as in an extension's `read`.

pub use ext_manifest::glob::{name_matches, path_matches};

/// Is the path excluded: a pattern without "/" matches the name at every level (".git", "*.log"), one with it
/// ("**/node_modules", "build/out") matches the root-relative path; everything
/// under a matching folder is excluded too.
pub fn is_excluded(excludes: &[String], rel_path: &str) -> bool {
    let parts: Vec<&str> = rel_path.trim_matches('/').split('/').collect();
    excludes.iter().any(|e| {
        if e.trim_end_matches('/').contains('/') {
            (1..=parts.len()).any(|k| path_matches(e, &parts[..k].join("/")))
        } else {
            parts.iter().any(|part| name_matches(e, part))
        }
    })
}
