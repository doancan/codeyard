//! Plain words for the git failures that have a usual cause and a usual way out. Git's own text stays the fallback: this only
//! recognises what it is sure about, and the guesses stay out (a wrong hint is worse than none).

use crate::i18n::{t, tf};

/// The remote has commits this branch lacks, so git refuses to overwrite them (`fetch first`, `non-fast-forward`).
pub fn is_push_rejected(err: &str) -> bool {
    let e = err.to_lowercase();
    e.contains("[rejected]") && (e.contains("fetch first") || e.contains("non-fast-forward") || e.contains("tip of your current branch is behind"))
}

/// A switch that would overwrite local changes (tracked files, or untracked ones the other branch has).
pub fn is_checkout_blocked(err: &str) -> bool {
    let e = err.to_lowercase();
    e.contains("would be overwritten by checkout")
        || e.contains("would be overwritten by switch")
        || e.contains("please commit your changes or stash them before you switch")
}

/// The server wants a login this command could not give (git never asks on its own here).
pub fn needs_credentials(err: &str) -> bool {
    let e = err.to_lowercase();
    [
        "could not read username",
        "could not read password",
        "terminal prompts disabled",
        "authentication failed",
        "permission denied (publickey",
        "password authentication was removed",
        "returned error: 401",
        "returned error: 403",
    ]
    .iter()
    .any(|needle| e.contains(needle))
        || (e.contains("remote: permission to") && e.contains("denied to"))
}

/// The server was given a login and refused it (a wrong account, a token without the permission), as opposed to not being given one.
pub fn credentials_rejected(err: &str) -> bool {
    let e = err.to_lowercase();
    ["authentication failed", "returned error: 401", "returned error: 403", "invalid username or password"].iter().any(|needle| e.contains(needle))
        || (e.contains("remote: permission to") && e.contains("denied to"))
}

/// The text between the first `'` after `marker` and the next `'`: git quotes the path or name it complains about.
fn quoted_after<'a>(err: &'a str, marker: &str) -> Option<&'a str> {
    let rest = &err[err.find(marker)? + marker.len()..];
    let start = rest.find('\'')? + 1;
    let len = rest[start..].find('\'')?;
    Some(&rest[start..start + len])
}

/// The sentence to show for a failure with a known cause; `None` for anything else (the caller shows git's own line).
pub fn explain(err: &str) -> Option<String> {
    let e = err.to_lowercase();
    if is_push_rejected(err) {
        return Some(t("The remote has commits you do not have: pull first (Git: Pull), then push. To overwrite them instead, use Git: Push (Force).").into());
    }
    if e.contains("protected branch") {
        return Some(t("The server protects this branch and refuses direct pushes: push to another branch and open a pull request.").into());
    }
    if e.contains("index.lock") && e.contains("file exists") {
        let lock = quoted_after(err, "Unable to create").unwrap_or(".git/index.lock");
        return Some(tf("Another git process is using this repository. If none is running, a crash left {} behind: delete it and try again.", &[&lock]));
    }
    if e.contains("detected dubious ownership") {
        let path = quoted_after(err, "in repository at").unwrap_or("this folder");
        return Some(tf(
            "Git does not trust this folder because another user owns it. If you trust it, run: git config --global --add safe.directory {}",
            &[&path],
        ));
    }
    if is_checkout_blocked(err) || e.contains("would be overwritten by merge") {
        return Some(t("Local changes would be overwritten: commit or stash them first.").into());
    }
    if e.contains("nothing to commit") || e.contains("no changes added to commit") {
        return Some(t("There is nothing to commit: no change is staged.").into());
    }
    if e.contains("no configured push destination") {
        return Some(t("This repository has no remote. Add one with Git: Add Remote…").into());
    }
    if e.contains("password authentication was removed") {
        return Some(
            t("The server no longer accepts an account password: use a personal access token as the password (or sign in with the GitHub CLI: gh auth login).")
                .into(),
        );
    }
    if e.contains("refusing to merge unrelated histories") {
        return Some(
            t("These branches share no history, so git will not merge them. In a terminal, `git merge --allow-unrelated-histories` merges them anyway.").into(),
        );
    }
    if e.contains("gpg failed to sign the data") {
        return Some(t("Signing the commit failed: gpg could not sign it. Check that the key is available and that gpg-agent can ask for its passphrase, or turn commit.gpgsign off.").into());
    }
    if e.contains("git-lfs") && (e.contains("not found") || e.contains("command not found")) {
        return Some(t("This repository uses Git LFS, but git-lfs is not installed on this host.").into());
    }
    if ["could not resolve host", "failed to connect to", "connection timed out", "network is unreachable", "operation timed out"].iter().any(|n| e.contains(n))
    {
        return Some(t("Cannot reach the server: check the network connection (and a proxy or VPN, if you use one).").into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const REJECTED: &str = "To github.com:x/y.git\n ! [rejected]        main -> main (fetch first)\nerror: failed to push some refs to 'github.com:x/y.git'\nhint: Updates were rejected";

    #[test]
    fn a_rejected_push_says_to_pull_first() {
        assert!(is_push_rejected(REJECTED));
        assert!(is_push_rejected(" ! [rejected] main -> main (non-fast-forward)"));
        assert!(!is_push_rejected(" ! [remote rejected] main -> main (protected branch hook declined)"), "a protected branch is another problem");
        assert!(explain(REJECTED).unwrap().contains("pull first"));
        assert!(explain("remote: Support for password authentication was removed on August 13, 2021.").unwrap().contains("personal access token"));
    }

    #[test]
    fn a_lock_file_names_the_file_to_delete() {
        let err = "fatal: Unable to create '/work/repo/.git/index.lock': File exists.\n\nAnother git process seems to be running";
        let text = explain(err).unwrap();
        assert!(text.contains("/work/repo/.git/index.lock") && text.contains("delete it"), "{text}");
    }

    #[test]
    fn an_untrusted_folder_gets_the_command_that_trusts_it() {
        let err = "fatal: detected dubious ownership in repository at '/srv/code/app'\nTo add an exception for this directory, call:";
        assert!(explain(err).unwrap().ends_with("safe.directory /srv/code/app"));
    }

    #[test]
    fn local_changes_blocking_a_switch_are_recognised_in_both_spellings() {
        assert!(is_checkout_blocked("error: Your local changes to the following files would be overwritten by checkout:\n\tf.txt"));
        assert!(is_checkout_blocked("error: The following untracked working tree files would be overwritten by checkout:\n\ta.txt"));
        assert!(is_checkout_blocked("error: Your local changes would be overwritten by switch"));
        assert!(!is_checkout_blocked("error: pathspec 'x' did not match any file(s) known to git"));
    }

    #[test]
    fn credential_and_network_failures_are_told_apart() {
        for e in [
            "fatal: could not read Username for 'https://github.com': terminal prompts disabled",
            "remote: Invalid username or password.\nfatal: Authentication failed for 'https://github.com/x/y.git/'",
            "git@github.com: Permission denied (publickey).",
            "remote: Support for password authentication was removed on August 13, 2021.",
        ] {
            assert!(needs_credentials(e), "{e}");
        }
        assert!(!needs_credentials("fatal: unable to access 'https://x/': Could not resolve host: x"));
        assert!(explain("fatal: unable to access 'https://x/': Could not resolve host: x").unwrap().contains("network"));
    }

    #[test]
    fn a_refused_login_is_not_a_missing_one() {
        assert!(credentials_rejected("remote: Permission to x/y.git denied to bob.\nfatal: unable to access: The requested URL returned error: 403"));
        assert!(credentials_rejected("fatal: Authentication failed for 'https://github.com/x/y.git/'"));
        assert!(!credentials_rejected("fatal: could not read Username for 'https://github.com': terminal prompts disabled"));
        assert!(needs_credentials("fatal: could not read Username for 'https://github.com': terminal prompts disabled"));
    }

    #[test]
    fn an_empty_commit_and_a_missing_remote_are_said_in_one_sentence() {
        assert!(explain("On branch main\n\nnothing to commit, working tree clean").unwrap().starts_with("There is nothing to commit"));
        assert!(explain("no changes added to commit (use \"git add\" and/or \"git commit -a\")").is_some());
        assert!(explain("fatal: No configured push destination.\nEither specify the URL").unwrap().contains("Git: Add Remote"));
    }

    #[test]
    fn a_protected_branch_is_not_mistaken_for_a_stale_one() {
        let github = "remote: error: GH006: Protected branch update failed for refs/heads/main.\nremote: error: Changes must be made through a pull request.\nTo github.com:x/y.git\n ! [remote rejected] main -> main (protected branch hook declined)";
        let gitlab = "remote: GitLab: You are not allowed to push code to protected branches on this project.\n ! [remote rejected] main -> main (pre-receive hook declined)";
        for e in [github, gitlab] {
            let text = explain(e).unwrap();
            assert!(text.contains("protects") && text.contains("pull request"), "{text}");
            assert!(!is_push_rejected(e));
        }
        assert_eq!(explain(" ! [remote rejected] main -> main (pre-receive hook declined)"), None, "an unknown hook keeps its own words");
    }

    #[test]
    fn unknown_failures_are_left_to_git() {
        assert_eq!(explain("fatal: pathspec 'zzz' did not match any files"), None);
        assert_eq!(explain(""), None);
    }
}
