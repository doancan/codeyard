//! Hosts named in the ssh config, including files pulled in with `Include`.
//!
//! `Include` follows ssh's rules: several patterns per line, `~` for the home folder, relative paths under `~/.ssh`,
//! wildcards (`*`, `?`, `[...]`) expanded in sorted order. Included files are untrusted input like the config itself,
//! so the walk is bounded: nesting depth, number of files, matches per pattern and file size are capped, loops
//! (including through symlinks) are cut, and only regular files are read. A file that cannot be read is skipped.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshHost {
    pub alias: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    /// File that defines the host (`None` when parsed from text).
    pub source: Option<PathBuf>,
}

/// Deepest `Include` nesting followed (ssh itself stops at 16).
const MAX_DEPTH: usize = 8;
/// Most files read for one listing.
const MAX_FILES: usize = 64;
/// Most files one pattern may match.
const MAX_MATCHES: usize = 256;
/// Largest config file read.
const MAX_BYTES: u64 = 1024 * 1024;

/// Concrete Hosts in the ssh config (no wildcard patterns), following `Include`.
pub fn list_hosts() -> Vec<SshHost> {
    // Same config as used for connecting (`CODEYARD_SSH_CONFIG` if set, else ~/.ssh/config).
    let Some(path) = crate::ssh_config::config_path() else { return Vec::new() };
    let ssh_dir = dirs::home_dir().map(|h| h.join(".ssh")).unwrap_or_else(|| path.parent().map(Path::to_path_buf).unwrap_or_default());
    list_hosts_from(&path, &ssh_dir)
}

/// Hosts of the config at `root`; relative `Include` paths resolve under `ssh_dir`.
pub fn list_hosts_from(root: &Path, ssh_dir: &Path) -> Vec<SshHost> {
    let mut p = Parser { ssh_dir: Some(ssh_dir.to_path_buf()), ..Parser::default() };
    p.read_file(root, 0);
    p.hosts
}

/// Hosts in one config text; `Include` lines are not followed.
pub fn parse_config(text: &str) -> Vec<SshHost> {
    let mut p = Parser::default();
    p.feed(text, None, 0);
    p.hosts
}

#[derive(Default)]
struct Parser {
    hosts: Vec<SshHost>,
    /// Hosts the current `Host` block applies to.
    current: Vec<usize>,
    /// `None`: do not follow `Include`.
    ssh_dir: Option<PathBuf>,
    /// Files being read (canonical), to cut loops.
    stack: Vec<PathBuf>,
    files: usize,
}

impl Parser {
    fn read_file(&mut self, path: &Path, depth: usize) {
        if depth > MAX_DEPTH || self.files >= MAX_FILES {
            return;
        }
        let Ok(canon) = std::fs::canonicalize(path) else { return };
        if self.stack.contains(&canon) {
            return;
        }
        let Ok(meta) = std::fs::metadata(&canon) else { return };
        if !meta.is_file() || meta.len() > MAX_BYTES {
            return;
        }
        let Ok(text) = std::fs::read_to_string(&canon) else { return };
        self.files += 1;
        self.stack.push(canon.clone());
        self.feed(&text, Some(&canon), depth);
        self.stack.pop();
    }

    fn feed(&mut self, text: &str, source: Option<&Path>, depth: usize) {
        let mut in_match = false;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = match line.split_once(|c: char| c.is_whitespace() || c == '=') {
                Some((k, v)) => (k.to_ascii_lowercase(), v.trim_start_matches(|c: char| c.is_whitespace() || c == '=').trim()),
                None => continue,
            };
            match key.as_str() {
                "host" => {
                    in_match = false;
                    self.current.clear();
                    for alias in value.split_whitespace() {
                        if alias.contains(['*', '?', '!']) || self.hosts.iter().any(|h| h.alias == alias) {
                            continue;
                        }
                        self.current.push(self.hosts.len());
                        self.hosts.push(SshHost { alias: alias.into(), hostname: None, user: None, source: source.map(Path::to_path_buf) });
                    }
                }
                "match" => {
                    in_match = true;
                    self.current.clear();
                }
                // Inside `Match` it depends on the connection; outside, the included file's Host blocks are read
                // and the current block continues after it.
                "include" if !in_match => {
                    let Some(ssh_dir) = self.ssh_dir.clone() else { continue };
                    let saved = std::mem::take(&mut self.current);
                    for pattern in words(value) {
                        for path in expand(&pattern, &ssh_dir) {
                            self.read_file(&path, depth + 1);
                        }
                    }
                    self.current = saved;
                }
                "hostname" => self.current.iter().for_each(|&i| self.hosts[i].hostname = Some(value.into())),
                "user" => self.current.iter().for_each(|&i| self.hosts[i].user = Some(value.into())),
                _ => {}
            }
        }
    }
}

/// Space-separated words; `"..."` keeps spaces.
fn words(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for ch in value.chars() {
        match ch {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Files an `Include` pattern names, sorted, capped.
fn expand(pattern: &str, ssh_dir: &Path) -> Vec<PathBuf> {
    let path = match pattern.strip_prefix("~/") {
        Some(rest) => match dirs::home_dir() {
            Some(home) => home.join(rest),
            None => return Vec::new(),
        },
        None if Path::new(pattern).is_absolute() => PathBuf::from(pattern),
        None => ssh_dir.join(pattern),
    };
    if !path.to_string_lossy().contains(['*', '?', '[']) {
        return vec![path];
    }
    let mut found = vec![PathBuf::new()];
    for comp in path.components() {
        let part = comp.as_os_str().to_string_lossy();
        if !part.contains(['*', '?', '[']) {
            found.iter_mut().for_each(|p| p.push(comp));
            continue;
        }
        let mut next = Vec::new();
        for dir in &found {
            let Ok(entries) = std::fs::read_dir(dir) else { continue };
            let mut names: Vec<String> = entries.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
            names.sort();
            // Like the shell, a wildcard does not match a leading dot.
            next.extend(names.into_iter().filter(|n| !n.starts_with('.') && wild_match(&part, n)).map(|n| dir.join(n)));
            if next.len() >= MAX_MATCHES {
                break;
            }
        }
        next.truncate(MAX_MATCHES);
        found = next;
    }
    found
}

/// `*`, `?` and `[...]` (with `!` or `^` to negate and `a-z` ranges) against one name.
fn wild_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    // Iterative matcher with backtracking to the last `*`: linear in practice, no recursion.
    let (mut pi, mut ni) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ni < n.len() {
        let step = match p.get(pi) {
            Some('*') => {
                star = Some((pi, ni));
                pi += 1;
                continue;
            }
            Some('?') => Some(pi + 1),
            Some('[') => class(&p, pi, n[ni]),
            Some(&c) if c == n[ni] => Some(pi + 1),
            _ => None,
        };
        match (step, star) {
            (Some(next), _) => {
                pi = next;
                ni += 1;
            }
            (None, Some((sp, sn))) => {
                pi = sp + 1;
                ni = sn + 1;
                star = Some((sp, sn + 1));
            }
            (None, None) => return false,
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// A `[...]` class at `p[at]` against `c`: the index after the class when it matches. An unclosed `[` is a literal.
fn class(p: &[char], at: usize, c: char) -> Option<usize> {
    let Some(close) = p[at + 1..].iter().skip(1).position(|&x| x == ']').map(|i| at + 2 + i) else {
        return (c == '[').then_some(at + 1);
    };
    let body = &p[at + 1..close];
    let (negate, body) = match body.first() {
        Some('!' | '^') => (true, &body[1..]),
        _ => (false, body),
    };
    let mut hit = false;
    let mut i = 0;
    while i < body.len() {
        if i + 2 < body.len() && body[i + 1] == '-' {
            hit |= (body[i]..=body[i + 2]).contains(&c);
            i += 3;
        } else {
            hit |= body[i] == c;
            i += 1;
        }
    }
    (hit != negate).then_some(close + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("codeyard-sshinc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn aliases(hosts: &[SshHost]) -> Vec<&str> {
        hosts.iter().map(|h| h.alias.as_str()).collect()
    }

    #[test]
    fn wildcards_match_like_the_shell() {
        assert!(wild_match("*", "work"));
        assert!(wild_match("*.conf", "a.conf") && !wild_match("*.conf", "a.conf.bak"));
        assert!(wild_match("h?st", "host") && !wild_match("h?st", "hst"));
        assert!(wild_match("[a-c]*", "box") && !wild_match("[!a-c]*", "box"));
        assert!(wild_match("a*b*c", "aXXbYYc") && !wild_match("a*b*c", "aXXbYY"));
        assert!(wild_match("[x", "[x"), "an unclosed class is literal");
        assert!(!wild_match(&"*a".repeat(40), &"b".repeat(200)), "no blow-up on hostile patterns");
    }

    #[test]
    fn included_files_add_hosts_in_order_and_relative_to_the_ssh_folder() {
        let dir = temp("order");
        std::fs::create_dir_all(dir.join("config.d")).unwrap();
        std::fs::write(dir.join("config"), "Host top\n  HostName t\nInclude config.d/*\nHost last\n").unwrap();
        std::fs::write(dir.join("config.d/20-b"), "Host b\n  User bee\n").unwrap();
        std::fs::write(dir.join("config.d/10-a"), "Host a top\n").unwrap();
        std::fs::write(dir.join("config.d/.hidden"), "Host hidden\n").unwrap();
        let hosts = list_hosts_from(&dir.join("config"), &dir);
        assert_eq!(aliases(&hosts), vec!["top", "a", "b", "last"], "sorted includes, first definition wins, dot files skipped");
        assert_eq!(hosts[2].user.as_deref(), Some("bee"));
        assert_eq!(hosts[1].source.as_deref(), Some(std::fs::canonicalize(dir.join("config.d/10-a")).unwrap().as_path()));
    }

    #[test]
    fn the_block_before_an_include_continues_after_it() {
        let dir = temp("cont");
        std::fs::write(dir.join("config"), "Host one\nInclude extra\n  User after\n").unwrap();
        std::fs::write(dir.join("extra"), "Host two\n  User inside\n").unwrap();
        let hosts = list_hosts_from(&dir.join("config"), &dir);
        assert_eq!(hosts.iter().map(|h| (h.alias.as_str(), h.user.as_deref())).collect::<Vec<_>>(), vec![("one", Some("after")), ("two", Some("inside"))]);
    }

    #[test]
    fn loops_depth_size_and_odd_files_are_bounded() {
        let dir = temp("bounds");
        std::fs::write(dir.join("config"), "Host root\nInclude a\n").unwrap();
        std::fs::write(dir.join("a"), "Host a\nInclude b\n").unwrap();
        std::fs::write(dir.join("b"), "Host b\nInclude a config\n").unwrap();
        assert_eq!(aliases(&list_hosts_from(&dir.join("config"), &dir)), vec!["root", "a", "b"], "a loop ends");

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.join("loop"), dir.join("loop")).ok();
            std::fs::write(dir.join("sym"), "Host s\nInclude loop\n").unwrap();
            assert_eq!(aliases(&list_hosts_from(&dir.join("sym"), &dir)), vec!["s"], "a symlink loop is skipped");
        }

        for i in 0..20 {
            std::fs::write(dir.join(format!("d{i}")), format!("Host d{i}\nInclude d{}\n", i + 1)).unwrap();
        }
        let deep = list_hosts_from(&dir.join("d0"), &dir);
        assert_eq!(deep.len(), MAX_DEPTH + 1, "nesting stops at the cap");

        std::fs::create_dir_all(dir.join("many")).unwrap();
        for i in 0..100 {
            std::fs::write(dir.join(format!("many/{i:03}")), format!("Host m{i}\n")).unwrap();
        }
        std::fs::write(dir.join("fan"), "Include many/*\n").unwrap();
        assert!(list_hosts_from(&dir.join("fan"), &dir).len() < MAX_FILES, "the number of files is capped");

        std::fs::write(dir.join("huge"), format!("Host big\n#{}\n", "x".repeat(MAX_BYTES as usize))).unwrap();
        std::fs::create_dir_all(dir.join("adir")).unwrap();
        std::fs::write(dir.join("odd"), "Host ok\nInclude huge adir missing \"with space\"\n").unwrap();
        assert_eq!(aliases(&list_hosts_from(&dir.join("odd"), &dir)), vec!["ok"], "oversized, folder and missing includes are skipped");
    }

    #[test]
    fn include_inside_match_is_ignored_and_plain_parsing_does_not_follow_includes() {
        let dir = temp("match");
        std::fs::write(dir.join("config"), "Match host x\n  Include extra\nHost a\n").unwrap();
        std::fs::write(dir.join("extra"), "Host never\n").unwrap();
        assert_eq!(aliases(&list_hosts_from(&dir.join("config"), &dir)), vec!["a"]);
        assert_eq!(aliases(&parse_config("Host a\nInclude extra\n")), vec!["a"]);
        assert_eq!(words(r#"a "b c"  d"#), vec!["a", "b c", "d"]);
    }
}
