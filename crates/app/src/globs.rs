//! The glob patterns of settings (`files.exclude`, `search.exclude`, file associations): `*`, `?` and `**`, matched in
//! linear time, since a pattern can come from a project file.

/// Wildcard matching, non-recursive: `star` swallows anything (empty too), `one` matches a single item. Two pointers
/// backtracking to the last star: O(pattern × input) worst case. The recursive version went exponential on patterns like `*a*a*a*b`, and the pattern
/// can come from a project file (`.vscode/settings.json`).
fn glob_match<P, N>(p: &[P], n: &[N], star: impl Fn(&P) -> bool, one: impl Fn(&P, &N) -> bool) -> bool {
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

/// Simple glob: `*` and `?`; a pattern without `/` applies to the file/folder name.
pub fn name_matches(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.trim_end_matches('/').chars().collect();
    let n: Vec<char> = name.chars().collect();
    glob_match(&p, &n, |c| *c == '*', |c, x| *c == '?' || c == x)
}

/// Does the root-relative path match the pattern: `**` is zero or more folders, `*`/`?` within one segment.
pub fn path_matches(pattern: &str, rel_path: &str) -> bool {
    let p: Vec<&str> = pattern.trim_start_matches("./").trim_matches('/').split('/').collect();
    let n: Vec<&str> = rel_path.trim_matches('/').split('/').collect();
    glob_match(&p, &n, |s| *s == "**", |a, b| name_matches(a, b))
}

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
