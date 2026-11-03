//! Patches: the working tree's changes as one patch (to share or keep), and a patch applied to the working tree.

use super::*;

/// The most patch text the app carries around (the clipboard, one message).
const MAX_PATCH: usize = 32 << 20;

/// Every change to tracked files since HEAD, staged or not, as one patch that `apply_patch` (or `git apply`) takes back.
/// Untracked files are not part of it: Git has no patch for a file it does not know.
pub fn changes_patch(host: &dyn Host, root: &str) -> Result<String, String> {
    let out = git(host, root, &["diff", "HEAD", "--binary", "--no-color", "--no-ext-diff", "--no-textconv"])?;
    if !out.ok() {
        return Err(failure_text(&out));
    }
    if out.stdout.len() > MAX_PATCH {
        return Err(crate::i18n::t("The changes are too large for one patch.").to_string());
    }
    Ok(out.stdout_str().to_string())
}

/// Whether `text` is shaped like a patch Git can apply, so that pasted prose is refused before Git is asked.
pub fn looks_like_patch(text: &str) -> bool {
    let has = |prefix: &str| text.lines().any(|l| l.starts_with(prefix));
    has("diff --git ") || (has("--- ") && has("+++ ") && has("@@ "))
}

/// Apply `patch` to the working tree (not the index). The whole patch is checked first: it applies completely or not at all.
pub fn apply_patch(host: &dyn Host, root: &str, patch: &str) -> Result<(), String> {
    if patch.len() > MAX_PATCH {
        return Err(crate::i18n::t("The patch is too large.").to_string());
    }
    if !looks_like_patch(patch) {
        return Err(crate::i18n::t("The text is not a patch.").to_string());
    }
    for args in [&["apply", "--check", "--whitespace=nowarn", "-"][..], &["apply", "--whitespace=nowarn", "-"][..]] {
        let out = git_stdin(host, root, args, Some(patch.as_bytes().to_vec()))?;
        if !out.ok() {
            return Err(failure_text(&out));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patches_are_recognised_by_their_shape() {
        assert!(looks_like_patch("diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n"));
        assert!(looks_like_patch("--- a.txt\n+++ b.txt\n@@ -1 +1 @@\n-a\n+b\n"));
        assert!(!looks_like_patch("just some words\nand --- more\n"));
        assert!(!looks_like_patch(""));
    }

    #[test]
    fn the_changes_of_one_checkout_apply_to_another() {
        let Some((dir, run)) = repo("patch-from") else { return };
        let Some((other, run_other)) = repo("patch-to") else { return };
        let host = host::local();
        let (root, other_root) = (dir.to_string_lossy().into_owned(), other.to_string_lossy().into_owned());
        // Both start from the same file; the first one then changes it (staged and not) and adds an untracked one.
        for (d, run) in [(&dir, &run as &dyn Fn(&[&str]) -> Option<String>), (&other, &run_other)] {
            std::fs::write(d.join("a.txt"), "one\ntwo\nthree\n").unwrap();
            run(&["add", "-A"]).unwrap();
            run(&["commit", "-q", "-m", "a"]).unwrap();
        }
        std::fs::write(dir.join("a.txt"), "one\nTWO\nthree\nfour\n").unwrap();
        run(&["add", "a.txt"]).unwrap();
        std::fs::write(dir.join("a.txt"), "ONE\nTWO\nthree\nfour\n").unwrap();
        std::fs::write(dir.join("untracked.txt"), "x\n").unwrap();

        let patch = changes_patch(&host, &root).unwrap();
        assert!(patch.contains("+ONE") && !patch.contains("untracked.txt"), "{patch}");
        assert_eq!(apply_patch(&host, &other_root, &patch), Ok(()));
        assert_eq!(std::fs::read_to_string(other.join("a.txt")).unwrap(), "ONE\nTWO\nthree\nfour\n");
        assert_eq!(run_other(&["diff", "--cached", "--name-only"]).unwrap().trim(), "", "the index is untouched");

        // Applying it again fails as a whole and changes nothing; prose is refused without asking Git.
        let before = std::fs::read_to_string(other.join("a.txt")).unwrap();
        assert!(apply_patch(&host, &other_root, &patch).is_err());
        assert_eq!(std::fs::read_to_string(other.join("a.txt")).unwrap(), before);
        assert_eq!(apply_patch(&host, &other_root, "hello"), Err("The text is not a patch.".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&other);
    }
}
