//! Whether one call fits the declared permissions: a host for `net`, a program for `run`, a workspace path for `read`
//! and `write`. The mock host (`ext-test`) and `cyx test` answer calls with these, so a test fails where the app's
//! broker would refuse. Paths are workspace-relative with `/` separators; `.env*` and `.git/` match only globs that name
//! them (docs/plugin-architecture.md §5.2).

use crate::Permissions;

impl Permissions {
    /// `api.example.com` against `net` entries: the same host, or `*.example.com` for its subdomains.
    pub fn allows_net(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        self.net.iter().any(|h| match h.strip_prefix("*.") {
            Some(base) => host.ends_with(&format!(".{base}")),
            None => h.eq_ignore_ascii_case(&host),
        })
    }

    pub fn allows_run(&self, program: &str) -> bool {
        self.run.iter().any(|p| p == program)
    }

    pub fn allows_read(&self, path: &str) -> bool {
        allowed(&self.read, path)
    }

    pub fn allows_write(&self, path: &str) -> bool {
        allowed(&self.write, path)
    }
}

fn allowed(globs: &[String], path: &str) -> bool {
    let path = path.trim_start_matches("./").trim_start_matches('/');
    let guarded = path.split('/').any(|p| p.starts_with(".env") || p == ".git");
    globs.iter().any(|g| glob_match(g, path) && (!guarded || g.contains(".env") || g.contains(".git")))
}

/// `*` and `?` within one path part, `**` across parts. A glob without `/` matches the file name in any folder, as in
/// `.gitignore`.
pub fn glob_match(glob: &str, path: &str) -> bool {
    let glob = glob.trim_start_matches("./");
    let g: Vec<&str> = glob.split('/').collect();
    let p: Vec<&str> = path.split('/').collect();
    if g.len() == 1 && g[0] != "**" {
        return p.last().is_some_and(|name| part_match(g[0].as_bytes(), name.as_bytes()));
    }
    parts_match(&g, &p)
}

fn parts_match(g: &[&str], p: &[&str]) -> bool {
    match g.split_first() {
        None => p.is_empty(),
        Some((&"**", rest)) => (0..=p.len()).any(|i| parts_match(rest, &p[i..])),
        Some((first, rest)) => p.split_first().is_some_and(|(name, more)| part_match(first.as_bytes(), name.as_bytes()) && parts_match(rest, more)),
    }
}

fn part_match(g: &[u8], s: &[u8]) -> bool {
    match g.split_first() {
        None => s.is_empty(),
        Some((b'*', rest)) => (0..=s.len()).any(|i| part_match(rest, &s[i..])),
        Some((b'?', rest)) => !s.is_empty() && part_match(rest, &s[1..]),
        Some((c, rest)) => s.first() == Some(c) && part_match(rest, &s[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn perms(read: &[&str], net: &[&str]) -> Permissions {
        Permissions { read: read.iter().map(|s| s.to_string()).collect(), net: net.iter().map(|s| s.to_string()).collect(), ..Default::default() }
    }

    #[test]
    fn hosts() {
        let p = perms(&[], &["api.example.com", "*.vercel.app"]);
        assert!(p.allows_net("api.example.com"));
        assert!(p.allows_net("API.example.com"));
        assert!(!p.allows_net("example.com"));
        assert!(p.allows_net("x.vercel.app"));
        assert!(!p.allows_net("vercel.app"));
    }

    #[test]
    fn globs() {
        assert!(glob_match("vercel.json", "vercel.json"));
        assert!(glob_match("vercel.json", "apps/web/vercel.json"));
        assert!(glob_match("src/*.rs", "src/lib.rs"));
        assert!(!glob_match("src/*.rs", "src/a/lib.rs"));
        assert!(glob_match("src/**/*.rs", "src/a/b/lib.rs"));
        assert!(glob_match("src/**", "src/a/b"));
        assert!(glob_match("**", "a/b"));
        assert!(glob_match("?.md", "a.md"));
        assert!(!glob_match("?.md", "ab.md"));
    }

    #[test]
    fn secrets_need_naming() {
        assert!(!perms(&["**"], &[]).allows_read(".env"));
        assert!(!perms(&["**"], &[]).allows_read(".git/config"));
        assert!(perms(&[".env"], &[]).allows_read(".env"));
        assert!(perms(&["**"], &[]).allows_read("src/main.rs"));
    }
}
