//! Links to a file on the hosting service of a remote: GitHub, GitLab, Bitbucket and Codeberg (Gitea) addresses built
//! from the remote's URL. Nothing is fetched: the link is opened in the browser or copied.

fn encode_path(path: &str) -> String {
    path.split('/').map(crate::texttools::url_encode).collect::<Vec<_>>().join("/")
}

/// The web address of a remote repository (`https://host/group/repo`) and its host. Accepts `https://`, `ssh://`, `git@host:path`
/// and `git://` URLs, drops credentials and ports (an ssh port says nothing about the web port) and `.git`.
fn web_address(remote: &str) -> Option<(String, String)> {
    let remote = remote.trim();
    let scheme = ["https://", "http://", "ssh://", "git+ssh://", "git://"].iter().find_map(|p| remote.strip_prefix(p));
    let rest = scheme.unwrap_or(remote);
    let rest = rest.rsplit_once('@').filter(|(user, _)| !user.contains('/')).map_or(rest, |(_, r)| r);
    // The scp form (`host:group/repo`) has a colon where the URL forms have a slash.
    let (host, path) = match (scheme, rest.split_once([':', '/'])) {
        (_, Some((host, path))) if rest[host.len()..].starts_with('/') => (host, path),
        (None, Some((host, path))) => (host, path),
        (Some(_), Some((host, path))) => {
            (host, path.split_once('/').map_or(path, |(port, tail)| if port.chars().all(|c| c.is_ascii_digit()) { tail } else { path }))
        }
        _ => return None,
    };
    // `github.com-work` is an ssh alias for one of several accounts; the site is still github.com.
    let host = host.to_ascii_lowercase();
    let host = if host.starts_with("github.com-") { "github.com".to_string() } else { host };
    if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-')) || !host.contains('.') {
        return None;
    }
    let path = path.trim_matches('/').trim_end_matches(".git").trim_end_matches('/');
    let valid = |s: &str| !s.is_empty() && s != "." && s != ".." && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~'));
    let segments: Vec<&str> = path.split('/').collect();
    (segments.len() >= 2 && segments.iter().all(|s| valid(s))).then(|| (host.clone(), format!("https://{host}/{}", segments.join("/"))))
}

/// The link to `path` (relative to the repository root, `/` separated) at `rev` (a commit or a branch name), with lines
/// `lines.0..=lines.1` (1-based) marked. `None` for a host that is not known or a remote that is not a web address.
pub fn file_url(remote: &str, rev: &str, path: &str, lines: (usize, usize)) -> Option<String> {
    let (host, base) = web_address(remote)?;
    let (a, b) = lines;
    let (rev, path) = (encode_path(rev), encode_path(path));
    let range = |single: String, span: String| if a == b { single } else { span };
    Some(if host == "github.com" {
        format!("{base}/blob/{rev}/{path}#{}", range(format!("L{a}"), format!("L{a}-L{b}")))
    } else if host == "gitlab.com" || host.starts_with("gitlab.") {
        format!("{base}/-/blob/{rev}/{path}#{}", range(format!("L{a}"), format!("L{a}-{b}")))
    } else if host == "bitbucket.org" {
        format!("{base}/src/{rev}/{path}#{}", range(format!("lines-{a}"), format!("lines-{a}:{b}")))
    } else if host == "codeberg.org" {
        format!("{base}/src/commit/{rev}/{path}#{}", range(format!("L{a}"), format!("L{a}-L{b}")))
    } else {
        return None;
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_form_of_a_remote_gives_the_same_web_address() {
        for remote in [
            "https://github.com/doancan/codeyard.git",
            "https://user:token@github.com/doancan/codeyard",
            "git@github.com:doancan/codeyard.git",
            "ssh://git@github.com:22/doancan/codeyard.git",
            "git://github.com/doancan/codeyard/",
        ] {
            assert_eq!(web_address(remote), Some(("github.com".into(), "https://github.com/doancan/codeyard".into())), "{remote}");
        }
        assert_eq!(web_address("git@gitlab.com:group/sub/repo.git").unwrap().1, "https://gitlab.com/group/sub/repo");
        assert_eq!(web_address("git@github.com-work:o/r.git").unwrap().1, "https://github.com/o/r", "an ssh alias is the site itself");
    }

    #[test]
    fn what_is_not_a_web_address_gives_none() {
        for remote in ["", "/srv/git/repo.git", "file:///srv/repo", "../repo", "https://github.com/onlyowner", "git@localhost:a/b", "https://github.com/a/../b"]
        {
            assert_eq!(web_address(remote), None, "{remote:?}");
        }
        assert_eq!(file_url("git@example.org:a/b.git", "main", "x.rs", (1, 1)), None, "an unknown host has an unknown layout");
        assert_eq!(file_url("https://notgitlab.evil.com/a/b", "main", "x.rs", (1, 1)), None, "only gitlab.com and gitlab.* hosts");
    }

    #[test]
    fn each_host_gets_its_own_layout_and_line_marks() {
        let (sha, path) = ("abc123", "src/my file#1.rs");
        let enc = "src/my%20file%231.rs";
        assert_eq!(file_url("https://github.com/o/r.git", sha, path, (5, 5)), Some(format!("https://github.com/o/r/blob/{sha}/{enc}#L5")));
        assert_eq!(file_url("https://github.com/o/r.git", sha, path, (5, 9)), Some(format!("https://github.com/o/r/blob/{sha}/{enc}#L5-L9")));
        assert_eq!(file_url("git@gitlab.com:g/r.git", sha, path, (5, 9)), Some(format!("https://gitlab.com/g/r/-/blob/{sha}/{enc}#L5-9")));
        assert_eq!(file_url("https://bitbucket.org/o/r", sha, path, (5, 9)), Some(format!("https://bitbucket.org/o/r/src/{sha}/{enc}#lines-5:9")));
        assert_eq!(file_url("https://codeberg.org/o/r", sha, path, (5, 5)), Some(format!("https://codeberg.org/o/r/src/commit/{sha}/{enc}#L5")));
        assert_eq!(
            file_url("https://github.com/o/r", "feature/ü x", "a.rs", (1, 1)),
            Some("https://github.com/o/r/blob/feature/%C3%BC%20x/a.rs#L1".into()),
            "a branch name keeps its slash and encodes the rest"
        );
    }
}
