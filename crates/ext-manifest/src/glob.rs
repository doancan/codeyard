//! The one glob matcher of the extension platform: `read` / `write` permissions, `workspace.find-files`,
//! `activation.workspace-contains`, and the app's `files.exclude`-style settings. The app's broker, the mock hosts
//! and `cyx test` all match with it, so a glob means the same everywhere (docs/extensions/reference/globs.md, whose
//! table a test below checks).
//!
//! - A glob is matched against the whole workspace-relative path, `/`-separated: `main.rs` is the file at the top of
//!   the folder only, `**/main.rs` is one in any folder.
//! - `*` is any run of characters within one path part (empty too, a leading `.` too), `?` one character.
//! - `**` as a whole part is zero or more parts: `src/**` is `src` and everything under it.
//! - Everything else is literal, `[` and `]` included. `{a,b}` alternatives and a leading `!` are refused by
//!   [`check`] (`cyx check` reports them), so they never silently match nothing.
//!
//! Matching is linear in the length of the glob times the path, because a glob can come from a project file.

use crate::limits;
use crate::validate::is_deceptive;

/// Wildcard matching, non-recursive: `star` swallows anything (empty too), `one` matches a single item. Two pointers
/// backtracking to the last star: O(glob × input) worst case. A recursive matcher goes exponential on globs like
/// `*a*a*a*b`.
fn wildcard<P, N>(p: &[P], n: &[N], star: impl Fn(&P) -> bool, one: impl Fn(&P, &N) -> bool) -> bool {
    let (mut pi, mut ni) = (0, 0);
    let mut last_star: Option<(usize, usize)> = None;
    while ni < n.len() {
        if pi < p.len() && star(&p[pi]) {
            last_star = Some((pi, ni));
            pi += 1;
        } else if pi < p.len() && one(&p[pi], &n[ni]) {
            pi += 1;
            ni += 1;
        } else if let Some((sp, sn)) = last_star {
            pi = sp + 1;
            ni = sn + 1;
            last_star = Some((sp, sn + 1));
        } else {
            return false;
        }
    }
    while pi < p.len() && star(&p[pi]) {
        pi += 1;
    }
    pi == p.len()
}

/// One path part against one glob part: `*` and `?`. A trailing `/` on the glob is ignored.
pub fn name_matches(glob: &str, name: &str) -> bool {
    let p: Vec<char> = glob.trim_end_matches('/').chars().collect();
    let n: Vec<char> = name.chars().collect();
    wildcard(&p, &n, |c| *c == '*', |c, x| *c == '?' || c == x)
}

/// A workspace-relative path against a glob: `**` is zero or more parts, `*` and `?` stay within one part. A leading
/// `./` on the glob and `/` at either end of either are ignored.
pub fn path_matches(glob: &str, rel_path: &str) -> bool {
    let p: Vec<&str> = glob.trim_start_matches("./").trim_matches('/').split('/').collect();
    let n: Vec<&str> = rel_path.trim_matches('/').split('/').collect();
    wildcard(&p, &n, |s| *s == "**", |a, b| name_matches(a, b))
}

/// Whether `glob` is one the manifest may hold (`read`, `write`, `workspace-contains`, schema `file-match`): relative,
/// `/`-separated, and only syntax [`path_matches`] gives a meaning to.
pub fn check(glob: &str) -> Result<(), String> {
    if glob.is_empty() || glob.len() > limits::ITEM_LEN {
        return Err(format!("must be 1 to {} characters", limits::ITEM_LEN));
    }
    if glob.starts_with('/') || glob.starts_with('~') || glob.contains('\\') || glob.contains(':') {
        return Err("must be relative to the workspace and '/'-separated".into());
    }
    if glob.contains(['{', '}']) {
        return Err("must not use {a,b} alternatives; list each glob separately".into());
    }
    if glob.starts_with('!') {
        return Err("must not start with '!': globs only allow, so leave out what should not match".into());
    }
    if glob.chars().any(is_deceptive) {
        return Err("contains an invisible or control character".into());
    }
    let body = glob.strip_prefix("./").unwrap_or(glob);
    if body.split('/').any(|p| p.is_empty() || p == "." || p == "..") {
        return Err("must not contain '.', '..' or empty parts".into());
    }
    if body.split('/').any(|p| p.contains("**") && p != "**") {
        return Err("uses '**' inside a part; '**' must be a whole part, as in 'src/**/*.rs'".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_glob_is_matched_against_the_whole_path() {
        for (glob, path, want) in [
            ("main.rs", "main.rs", true),
            ("main.rs", "src/main.rs", false),
            ("**/main.rs", "src/main.rs", true),
            ("**/main.rs", "main.rs", true),
            ("src/*.rs", "src/lib.rs", true),
            ("src/*.rs", "src/a/lib.rs", false),
            ("src/**/*.rs", "src/a/b/lib.rs", true),
            ("src/**/*.rs", "src/lib.rs", true),
            ("src/**", "src/a/b", true),
            ("src/**", "src", true),
            ("**", "a/b", true),
            ("?.md", "a.md", true),
            ("?.md", "ab.md", false),
            ("*", ".env", true),
            ("pages/[id].tsx", "pages/[id].tsx", true),
            ("pages/[id].tsx", "pages/i.tsx", false),
            ("./build/out", "build/out", true),
            ("ü*.txt", "üç.txt", true),
        ] {
            assert_eq!(path_matches(glob, path), want, "{glob} vs {path}");
        }
    }

    /// The table in docs/extensions/reference/globs.md is what the matcher does.
    #[test]
    fn the_reference_table_is_right() {
        let doc = include_str!("../../../docs/extensions/reference/globs.md");
        let rows: Vec<Vec<&str>> =
            doc.lines().filter(|l| l.starts_with("| `")).map(|l| l.trim_matches('|').split('|').map(|c| c.trim().trim_matches('`')).collect()).collect();
        assert!(rows.len() >= 10);
        for r in rows {
            assert_eq!(path_matches(r[0], r[1]), r[2] == "yes", "{r:?}");
        }
    }

    #[test]
    fn hostile_globs_stay_linear() {
        let t = std::time::Instant::now();
        assert!(!name_matches("*a*a*a*a*a*a*a*a*a*a*a*a*a*a*a*b", &"a".repeat(60)));
        assert!(!path_matches("**/**/**/**/**/**/**/**/**/**/**/**/x", &vec!["d"; 60].join("/")));
        assert!(path_matches("**/**/**/**/**/**/**/d", &vec!["d"; 60].join("/")));
        assert!(!name_matches(&format!("{}b", "?*".repeat(200)), &"a".repeat(2000)));
        assert!(t.elapsed().as_secs() < 2, "took {:?}", t.elapsed());
    }

    #[test]
    fn syntax_without_a_meaning_is_refused() {
        assert!(check("src/**/*.rs").is_ok());
        assert!(check("pages/[id].tsx").is_ok());
        assert!(check("**/*.{rs,md}").unwrap_err().contains("{a,b}"));
        assert!(check("!secret.txt").unwrap_err().contains("'!'"));
        assert!(check("src/**.rs").unwrap_err().contains("whole part"));
        assert!(check("src/../etc").unwrap_err().contains(".."));
        assert!(check("/etc/passwd").is_err());
        assert!(check("C:/x").is_err());
    }
}
