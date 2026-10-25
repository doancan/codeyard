//! Tags: create (lightweight or annotated), delete, list, and their remote side.

use host::Host;

use super::refs::default_remote;
use super::{git_ok, plain_name};

/// Local tags, newest first. A tag that starts with `-` is left out (see `plain_name`).
pub fn tags(host: &dyn Host, root: &str) -> Result<Vec<String>, String> {
    let out = git_ok(host, root, &["for-each-ref", "--sort=-creatordate", "--format=%(refname)", "refs/tags"])?;
    Ok(out.lines().filter_map(|l| l.strip_prefix("refs/tags/")).filter(|t| plain_name(t).is_ok()).map(str::to_string).collect())
}

/// A tag at `target`. With a message it is annotated (it records who tagged, when, and why); without one it is a plain
/// pointer, like `git tag name`.
pub fn create_tag(host: &dyn Host, root: &str, target: &str, name: &str, message: Option<&str>) -> Result<(), String> {
    plain_name(name)?;
    match message.map(str::trim).filter(|m| !m.is_empty()) {
        Some(m) => git_ok(host, root, &["tag", "-a", "-m", m, "--", name, target]),
        None => git_ok(host, root, &["tag", "--", name, target]),
    }
    .map(|_| ())
}

pub fn delete_tag(host: &dyn Host, root: &str, name: &str) -> Result<(), String> {
    plain_name(name)?;
    git_ok(host, root, &["tag", "-d", "--", name]).map(|_| ())
}

/// Tags that exist on `remote` (asks the server).
pub fn remote_tags(host: &dyn Host, root: &str, remote: &str) -> Result<Vec<String>, String> {
    plain_name(remote)?;
    let out = git_ok(host, root, &["ls-remote", "--tags", "--refs", remote])?;
    Ok(parse_remote_tags(&out))
}

pub fn parse_remote_tags(out: &str) -> Vec<String> {
    let mut tags: Vec<String> = out
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .filter_map(|(_, r)| r.strip_prefix("refs/tags/"))
        .filter(|t| plain_name(t).is_ok())
        .map(str::to_string)
        .collect();
    // ls-remote lists by name and the server does not say which tag is newest; names that sort last usually are.
    tags.reverse();
    tags
}

pub fn delete_remote_tag(host: &dyn Host, root: &str, remote: &str, name: &str) -> Result<(), String> {
    plain_name(remote)?;
    plain_name(name)?;
    git_ok(host, root, &["push", remote, &format!(":refs/tags/{name}")]).map(|_| ())
}

/// Push every local tag (`name`: only that one) to the default remote; returns the remote's name.
pub fn push_tags(host: &dyn Host, root: &str, name: Option<&str>) -> Result<String, String> {
    let remote = default_remote(host, root)?;
    match name {
        Some(n) => {
            plain_name(n)?;
            git_ok(host, root, &["push", &remote, &format!("refs/tags/{n}")])?;
        }
        None => {
            git_ok(host, root, &["push", &remote, "--tags"])?;
        }
    }
    Ok(remote)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::repo;

    fn bare(name: &str) -> std::path::PathBuf {
        let bare = std::env::temp_dir().join(format!("codeyard-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&bare);
        crate::test_git::git().args(["init", "-q", "--bare"]).arg(&bare).output().unwrap();
        bare
    }

    #[test]
    fn annotated_and_lightweight_tags() {
        let Some((dir, run)) = repo("tags") else { return };
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        assert_eq!(create_tag(&host, &root, "HEAD", "plain", None), Ok(()));
        assert_eq!(create_tag(&host, &root, "HEAD", "blank", Some("   ")), Ok(()));
        assert_eq!(create_tag(&host, &root, "HEAD", "v1", Some("First release\n\nwith a body")), Ok(()));
        // A lightweight tag points at the commit, an annotated one at a tag object that carries the message.
        assert_eq!(run(&["cat-file", "-t", "plain"]).unwrap().trim(), "commit");
        assert_eq!(run(&["cat-file", "-t", "blank"]).unwrap().trim(), "commit");
        assert_eq!(run(&["cat-file", "-t", "v1"]).unwrap().trim(), "tag");
        assert!(run(&["tag", "-n9", "-l", "v1"]).unwrap().contains("First release"));
        // Duplicates are refused by git, not overwritten.
        assert!(create_tag(&host, &root, "HEAD", "v1", None).is_err());
        let listed = tags(&host, &root).unwrap();
        assert_eq!(listed.len(), 3, "{listed:?}");
        assert_eq!(delete_tag(&host, &root, "plain"), Ok(()));
        assert!(!tags(&host, &root).unwrap().contains(&"plain".to_string()));
        assert!(delete_tag(&host, &root, "plain").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tag_named_like_an_option_is_never_passed_to_git() {
        let Some((dir, run)) = repo("dashtag") else { return };
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        assert!(create_tag(&host, &root, "HEAD", "--force", None).is_err());
        assert!(create_tag(&host, &root, "HEAD", "-d", Some("m")).is_err());
        assert!(delete_tag(&host, &root, "-a").is_err());
        assert!(delete_remote_tag(&host, &root, "origin", "--all").is_err());
        assert!(push_tags(&host, &root, Some("--mirror")).is_err());
        // A repository can still hold such a ref (a hostile remote can create one); it is not offered.
        run(&["update-ref", "refs/tags/--exec=x", "HEAD"]).expect("git accepts the ref name");
        run(&["tag", "ok"]).unwrap();
        assert_eq!(tags(&host, &root).unwrap(), ["ok"]);
        assert_eq!(parse_remote_tags("aa\trefs/tags/--exec=x\nbb\trefs/tags/v1\n"), ["v1"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn push_and_delete_tags_on_the_remote() {
        let Some((dir, run)) = repo("remotetags") else { return };
        let bare = bare("remotetags-bare");
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        assert!(push_tags(&host, &root, None).unwrap_err().contains("no remote"));
        run(&["remote", "add", "origin", &bare.to_string_lossy()]).unwrap();
        run(&["tag", "v1"]).unwrap();
        run(&["tag", "-a", "-m", "two", "v2"]).unwrap();
        // One tag first, then all of them.
        assert_eq!(push_tags(&host, &root, Some("v1")), Ok("origin".to_string()));
        assert_eq!(remote_tags(&host, &root, "origin").unwrap(), ["v1"]);
        assert_eq!(push_tags(&host, &root, None), Ok("origin".to_string()));
        let mut remote = remote_tags(&host, &root, "origin").unwrap();
        remote.sort();
        assert_eq!(remote, ["v1", "v2"]);
        // Deleting on the remote leaves the local tag alone.
        assert_eq!(delete_remote_tag(&host, &root, "origin", "v1"), Ok(()));
        assert_eq!(remote_tags(&host, &root, "origin").unwrap(), ["v2"]);
        assert!(tags(&host, &root).unwrap().contains(&"v1".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&bare);
    }
}
