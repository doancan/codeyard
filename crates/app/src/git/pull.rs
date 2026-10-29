//! Pull request checkout uses a private ref, independent of concurrent fetches' shared `FETCH_HEAD`.

use host::Host;
use ring::rand::{SecureRandom, SystemRandom};

use super::{git_ok, plain_name};

struct PullRef<'a> {
    host: &'a dyn Host,
    root: &'a str,
    name: String,
    oid: String,
}

impl Drop for PullRef<'_> {
    fn drop(&mut self) {
        // Do not remove a ref that another process has changed since we last read it.
        let _ = git_ok(self.host, self.root, &["update-ref", "--no-deref", "-d", &self.name, &self.oid]);
    }
}

/// Fetch and check out `pull/N/head` without reading or writing `FETCH_HEAD`.
pub fn checkout_pull(host: &dyn Host, root: &str, remote: &str, number: u64) -> Result<(), String> {
    plain_name(remote)?;
    let mut nonce = [0u8; 16];
    SystemRandom::new().fill(&mut nonce).map_err(|_| crate::i18n::t("Could not reserve a temporary Git reference.").to_string())?;
    let name = format!("refs/codeyard/pull/{}", nonce.iter().map(|byte| format!("{byte:02x}")).collect::<String>());
    checkout_pull_with_ref(host, root, remote, number, name)
}

fn checkout_pull_with_ref(host: &dyn Host, root: &str, remote: &str, number: u64, name: String) -> Result<(), String> {
    // An empty blob lets us reserve the ref atomically even in an unborn repository.
    let marker = git_ok(host, root, &["hash-object", "-w", "--stdin"])?;
    let marker = marker.trim().to_string();
    git_ok(host, root, &["update-ref", "--no-deref", &name, &marker, ""])?;
    let mut reference = PullRef { host, root, name, oid: marker };
    let fetched =
        git_ok(host, root, &["fetch", "--quiet", "--no-write-fetch-head", "--no-tags", "--", remote, &format!("+refs/pull/{number}/head:{}", reference.name)]);
    // Read the exact object for both checkout and conditional cleanup, including a non-commit head.
    // If a reply was lost, cleanup only removes the reservation we can still prove we own.
    let object = git_ok(host, root, &["rev-parse", "--verify", &reference.name]);
    if let Ok(oid) = &object {
        reference.oid = oid.trim().to_string();
    }
    fetched?;
    object?;
    git_ok(host, root, &["checkout", "-q", "-B", &format!("pr-{number}"), &reference.oid])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::qa_integration_tests::{git_in, repo};

    #[test]
    fn a_private_pull_ref_collision_preserves_the_existing_ref_and_checkout() {
        let root = repo("pull-ref-collision");
        let path = crate::ui::local_path(&root);
        let original = git_in(&root, &["rev-parse", "HEAD"]);
        let reference = "refs/codeyard/pull/existing";
        git_in(&root, &["update-ref", reference, "HEAD"]);
        let host = host::local();
        assert!(checkout_pull_with_ref(&host, &path, "missing-remote", 42, reference.into()).is_err());
        assert_eq!(git_in(&root, &["rev-parse", reference]), original);
        assert_eq!(git_in(&root, &["rev-parse", "HEAD"]), original);
        assert!(!root.join(".git/FETCH_HEAD").exists(), "the collision stops before fetch");
    }

    #[test]
    fn a_failed_pull_checkout_keeps_dirty_files_and_removes_its_private_ref() {
        let root = repo("pull-dirty-checkout");
        let origin = repo("pull-dirty-origin");
        std::fs::write(origin.join("README.md"), "PR version\n").unwrap();
        git_in(&origin, &["commit", "-qam", "PR"]);
        git_in(&origin, &["update-ref", "refs/pull/42/head", "HEAD"]);
        std::fs::write(root.join("README.md"), "unsaved local edits\n").unwrap();
        let original = git_in(&root, &["rev-parse", "HEAD"]);
        let host = host::local();
        assert!(checkout_pull(&host, &crate::ui::local_path(&root), &crate::ui::local_path(&origin), 42).is_err());
        assert_eq!(git_in(&root, &["rev-parse", "HEAD"]), original);
        assert_eq!(std::fs::read_to_string(root.join("README.md")).unwrap(), "unsaved local edits\n");
        assert!(git_in(&root, &["branch", "--list", "pr-42"]).is_empty());
        assert!(git_in(&root, &["for-each-ref", "refs/codeyard/pull"]).is_empty());
        assert!(!root.join(".git/FETCH_HEAD").exists());
    }

    #[test]
    fn a_pull_checkout_works_before_the_local_repository_has_any_commits() {
        let origin = repo("pull-unborn-origin");
        git_in(&origin, &["update-ref", "refs/pull/42/head", "HEAD"]);
        let root = origin.join("empty-checkout");
        git_in(&origin, &["init", "-q", &crate::ui::local_path(&root)]);
        let expected = git_in(&origin, &["rev-parse", "HEAD"]);
        let host = host::local();
        checkout_pull(&host, &crate::ui::local_path(&root), &crate::ui::local_path(&origin), 42).unwrap();
        assert_eq!(git_in(&root, &["rev-parse", "HEAD"]), expected);
        assert_eq!(git_in(&root, &["branch", "--show-current"]).trim(), "pr-42");
        assert!(git_in(&root, &["for-each-ref", "refs/codeyard/pull"]).is_empty());
    }
}
