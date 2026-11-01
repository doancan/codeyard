//! Whether one call fits the declared permissions: a host for `net`, a program for `run`, a workspace path for `read`
//! and `write`. The app's broker, the mock host (`ext-test`) and `cyx test` all answer calls with these, so a test fails
//! where the app would refuse. A path is first made workspace-relative with [`relative`] (`..` resolved, `%2e`
//! decoded, nothing outside a workspace folder); `.env*` and `.git/` match only globs that name them
//! (docs/plugin-architecture.md §5.2).

use crate::Permissions;
use crate::glob::path_matches;

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

    /// A workspace-relative path (from [`relative`]) against `read`.
    pub fn allows_read(&self, rel: &str) -> bool {
        globs_allow(&self.read, rel)
    }

    /// A workspace-relative path (from [`relative`]) against `write`.
    pub fn allows_write(&self, rel: &str) -> bool {
        globs_allow(&self.write, rel)
    }
}

/// Whether one of `globs` allows the workspace-relative path `rel`. Credentials and repository internals (`.env*`,
/// `.git/`) must be named by a glob part, never reached by a wide one such as `**`.
pub fn globs_allow(globs: &[String], rel: &str) -> bool {
    let names_hidden = |s: &str| s.split('/').any(|p| p.starts_with(".env") || p == ".git");
    let hidden = names_hidden(rel);
    globs.iter().any(|g| path_matches(g, rel) && (!hidden || names_hidden(g)))
}

/// A `file://` URI or a plain path → an absolute path with `/` separators and no `.`/`..` parts, or none (relative,
/// `..` above the root, a bad `%` escape, or a decoded control character).
pub fn normalize(uri_or_path: &str) -> Option<String> {
    let raw = uri_or_path.strip_prefix("file://").unwrap_or(uri_or_path);
    let decoded = percent_decode(raw)?;
    let s = decoded.replace('\\', "/");
    // `/C:/x` from a Windows file URI is `C:/x`.
    let s = match s.as_bytes() {
        [b'/', d, b':', ..] if d.is_ascii_alphabetic() => s[1..].to_string(),
        _ => s,
    };
    let (prefix, rest) = match s.strip_prefix('/') {
        Some(rest) => ("/", rest),
        // `C:/x`; a drive-relative `C:x` names no fixed file.
        None => match s.as_bytes() {
            [d, b':'] if d.is_ascii_alphabetic() => (&s[..2], ""),
            [d, b':', b'/', ..] if d.is_ascii_alphabetic() => (&s[..2], &s[3..]),
            _ => return None,
        },
    };
    let prefix = if prefix == "/" { "/".to_string() } else { format!("{prefix}/") };
    let mut parts: Vec<&str> = Vec::new();
    for p in rest.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            p => parts.push(p),
        }
    }
    Some(format!("{prefix}{}", parts.join("/")))
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(h, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    let out = String::from_utf8(out).ok()?;
    // A decoded NUL or control character in a path is never legitimate.
    (!out.chars().any(char::is_control)).then_some(out)
}

/// The path of `uri` relative to the workspace folder (a `file://` URI or a path) that holds it, after
/// [`normalize`]; `""` for a folder itself. None when it is in no folder: such a call is refused with
/// [`outside_line`].
pub fn relative(folders: &[String], uri: &str) -> Option<String> {
    let path = normalize(uri)?;
    folders.iter().filter_map(|f| normalize(f)).find_map(|root| {
        let rest = path.strip_prefix(root.trim_end_matches('/'))?;
        if rest.is_empty() { Some(String::new()) } else { rest.strip_prefix('/').map(str::to_string) }
    })
}

/// The `permission` of the denial for a path outside every workspace folder.
pub fn outside_line(path: &str) -> String {
    format!("a path inside the workspace (not {path})")
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
    fn a_file_name_glob_is_the_file_at_the_top_only() {
        assert!(perms(&["main.rs"], &[]).allows_read("main.rs"));
        assert!(!perms(&["main.rs"], &[]).allows_read("src/main.rs"));
        assert!(perms(&["**/main.rs"], &[]).allows_read("src/main.rs"));
    }

    #[test]
    fn secrets_need_naming() {
        assert!(!perms(&["**"], &[]).allows_read(".env"));
        assert!(!perms(&["**"], &[]).allows_read("app/.env.local"));
        assert!(!perms(&["**"], &[]).allows_read(".git/config"));
        assert!(perms(&[".env"], &[]).allows_read(".env"));
        assert!(perms(&[".git/config"], &[]).allows_read(".git/config"));
        assert!(perms(&["**/.env*"], &[]).allows_read("app/.env.local"));
        assert!(perms(&["**"], &[]).allows_read("src/main.rs"));
        assert!(perms(&["**"], &[]).allows_read(".gitignore"));
        // A glob that only mentions `.git` in a file name does not name the folder.
        assert!(!perms(&["**/*.git*"], &[]).allows_read(".git/config"));
    }

    #[test]
    fn paths_are_resolved_before_they_are_matched() {
        let folders = ["file:///work".to_string()];
        assert_eq!(relative(&folders, "file:///work/src/main.rs").as_deref(), Some("src/main.rs"));
        assert_eq!(relative(&folders, "file:///work").as_deref(), Some(""));
        assert_eq!(relative(&folders, "file:///work/src/../../etc/passwd"), None);
        assert_eq!(relative(&folders, "file:///work/src/%2e%2e/%2e%2e/etc/passwd"), None);
        assert_eq!(relative(&folders, "file:///work/src/./a/../b.rs").as_deref(), Some("src/b.rs"));
        assert_eq!(relative(&folders, "file:///workshop/a"), None, "a sibling with the same prefix is outside");
        assert_eq!(relative(&folders, "file:///etc/passwd"), None);
        assert_eq!(relative(&folders, "src/main.rs"), None, "a relative path names no file");
        assert_eq!(relative(&folders, "file:///work/a%00b"), None);
        assert_eq!(relative(&["/home/me/p".to_string()], "/home/me/p/İstanbul/ğ.txt").as_deref(), Some("İstanbul/ğ.txt"));
        assert_eq!(relative(&["C:/p".to_string()], "file:///C:/p/a.txt").as_deref(), Some("a.txt"));
    }
}
