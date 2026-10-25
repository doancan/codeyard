//! "Add to .gitignore": append a path to the repository's own `.gitignore`.

use host::Host;

/// The `.gitignore` line for `rel` (relative to the repository root). It is anchored with a leading `/`, so
/// `build` in the root does not also hide `src/build`; a folder ends with `/`, so a file of that name is left alone.
pub fn ignore_pattern(rel: &str, is_dir: bool) -> String {
    let rel = rel.trim_matches('/');
    let mut line = String::from("/");
    for (i, c) in rel.chars().enumerate() {
        // `#` and `!` only mean something at the start of a pattern; the rest are wildcards anywhere.
        if matches!(c, '*' | '?' | '[' | '\\') || (i == 0 && matches!(c, '#' | '!')) {
            line.push('\\');
        }
        line.push(c);
    }
    // A trailing space is dropped by git unless it is escaped.
    if line.ends_with(' ') {
        line.pop();
        line.push_str("\\ ");
    }
    if is_dir {
        line.push('/');
    }
    line
}

/// Whether `existing` already has a line that ignores exactly this path.
fn already_listed(existing: &str, pattern: &str) -> bool {
    let bare = pattern.trim_start_matches('/').trim_end_matches('/');
    existing.lines().map(|l| l.trim_end_matches('\r')).any(|l| {
        let l = l.trim_start_matches('/');
        l == bare || (pattern.ends_with('/') && l.trim_end_matches('/') == bare)
    })
}

/// Append the path to `<root>/.gitignore`, creating the file if needed. `Ok(false)`: it was listed already.
pub fn add_to_gitignore(host: &dyn Host, root: &str, rel: &str, is_dir: bool) -> Result<bool, String> {
    if rel.trim_matches('/').is_empty() || rel.contains(['\n', '\r']) {
        return Err(crate::i18n::t("There is nothing to ignore here").into());
    }
    let path = format!("{}/.gitignore", root.trim_end_matches('/'));
    let (text, hash) = match host.read(&path) {
        Ok((data, hash)) => (String::from_utf8_lossy(&data).into_owned(), Some(hash)),
        Err(_) if host.stat(&path).is_err() => (String::new(), None),
        Err(e) => return Err(e.to_string()),
    };
    let pattern = ignore_pattern(rel, is_dir);
    if already_listed(&text, &pattern) {
        return Ok(false);
    }
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out = text;
    // A file that ends without a newline would otherwise get the new entry glued to its last line.
    if !out.is_empty() && !out.ends_with('\n') {
        out.push_str(eol);
    }
    out.push_str(&pattern);
    out.push_str(eol);
    host.write(&path, out.into_bytes(), hash).map_err(|e| e.to_string())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::repo;

    #[test]
    fn patterns_are_anchored_and_escaped() {
        assert_eq!(ignore_pattern("target", true), "/target/");
        assert_eq!(ignore_pattern("src/a.log", false), "/src/a.log");
        assert_eq!(ignore_pattern("/src/gen/", true), "/src/gen/");
        assert_eq!(ignore_pattern("#notes.txt", false), "/\\#notes.txt");
        assert_eq!(ignore_pattern("a[1]*.txt", false), "/a\\[1]\\*.txt");
        assert_eq!(ignore_pattern("trail ", false), "/trail\\ ");
    }

    #[test]
    fn a_listed_path_is_recognised_in_its_other_spellings() {
        assert!(already_listed("a.log\n", "/a.log"));
        assert!(already_listed("/a.log\r\n", "/a.log"));
        assert!(already_listed("build/\n", "/build/"));
        assert!(already_listed("/build\n", "/build/"));
        assert!(!already_listed("build/\n", "/build"), "a folder entry does not cover a file of that name");
        assert!(!already_listed("# a.log\n", "/a.log"));
        assert!(!already_listed("sub/a.log\n", "/a.log"));
    }

    #[test]
    fn gitignore_is_created_extended_and_never_duplicated() {
        let Some((dir, run)) = repo("gitignore") else { return };
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        let file = dir.join(".gitignore");
        // Missing file: created with a single line and a trailing newline.
        assert_eq!(add_to_gitignore(&host, &root, "secret.env", false), Ok(true));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "/secret.env\n");
        // Again: nothing changes.
        assert_eq!(add_to_gitignore(&host, &root, "secret.env", false), Ok(false));
        // A folder gets a trailing slash; a last line without a newline is not glued to the new one.
        std::fs::write(&file, "*.tmp").unwrap();
        assert_eq!(add_to_gitignore(&host, &root, "out/cache", true), Ok(true));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "*.tmp\n/out/cache/\n");
        assert_eq!(add_to_gitignore(&host, &root, "out/cache", true), Ok(false));
        // CRLF files stay CRLF.
        std::fs::write(&file, "a\r\nb").unwrap();
        assert_eq!(add_to_gitignore(&host, &root, "c.txt", false), Ok(true));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "a\r\nb\r\n/c.txt\r\n");
        // An entry written by hand in another spelling counts.
        std::fs::write(&file, "dist/\n").unwrap();
        assert_eq!(add_to_gitignore(&host, &root, "dist", true), Ok(false));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "dist/\n");
        // Git itself agrees that the entries do what they say.
        std::fs::write(&file, "").unwrap();
        std::fs::create_dir_all(dir.join("d/sub")).unwrap();
        std::fs::write(dir.join("d/sub/x.txt"), "x\n").unwrap();
        std::fs::write(dir.join("#odd.txt"), "x\n").unwrap();
        assert_eq!(add_to_gitignore(&host, &root, "d", true), Ok(true));
        assert_eq!(add_to_gitignore(&host, &root, "#odd.txt", false), Ok(true));
        assert!(run(&["check-ignore", "-q", "d/sub/x.txt"]).is_some());
        assert!(run(&["check-ignore", "-q", "#odd.txt"]).is_some());
        assert!(add_to_gitignore(&host, &root, "", false).is_err());
        assert!(add_to_gitignore(&host, &root, "/", true).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
