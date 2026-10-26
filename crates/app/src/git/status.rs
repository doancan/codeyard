//! Working tree state: `git status` parsing, staging, committing, identity.

use std::collections::HashMap;

use host::Host;

use super::{git, git_ok};
use crate::ui::Palette;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Mark {
    // Order matters: a folder shows the most important status among its contents.
    Untracked,
    Renamed,
    Added,
    Modified,
    Deleted,
    Conflict,
}

impl Mark {
    fn from_code(c: u8) -> Option<Mark> {
        match c {
            b'M' | b'T' => Some(Mark::Modified),
            b'A' | b'C' => Some(Mark::Added),
            b'D' => Some(Mark::Deleted),
            b'R' => Some(Mark::Renamed),
            b'U' => Some(Mark::Conflict),
            b'?' => Some(Mark::Untracked),
            _ => None,
        }
    }

    pub fn letter(self) -> &'static str {
        match self {
            Mark::Untracked => "N",
            Mark::Renamed => "R",
            Mark::Added => "A",
            Mark::Modified => "M",
            Mark::Deleted => "D",
            Mark::Conflict => "!",
        }
    }

    pub fn color(self, p: &Palette) -> u32 {
        let base = match self {
            Mark::Untracked | Mark::Added => p.success,
            Mark::Renamed => p.info,
            // Orange: between yellow `warning` and red `danger`.
            Mark::Modified => crate::ui::mix(p.warning, p.danger, 0.4),
            Mark::Deleted | Mark::Conflict => p.danger,
        };
        // Stay readable on every file-list background (normal, hover, selected): AA text ratio, with a little to spare.
        crate::ui::ensure_contrast(base, p.text, &[p.bg, p.surface, p.surface_hover, p.surface_active, crate::ui::focused_selection(p)], 4.6)
    }

    pub fn describe(self) -> &'static str {
        match self {
            Mark::Untracked => crate::i18n::t("New (untracked)"),
            Mark::Renamed => crate::i18n::t("Renamed"),
            Mark::Added => crate::i18n::t("Added"),
            Mark::Modified => crate::i18n::t("Modified"),
            Mark::Deleted => crate::i18n::t("Deleted"),
            Mark::Conflict => crate::i18n::t("Conflict"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// Relative to the repository root.
    pub path: String,
    /// Old path on a rename.
    pub orig: Option<String>,
    /// Change in the index (staged).
    pub staged: Option<Mark>,
    /// Change in the working tree (unstaged).
    pub unstaged: Option<Mark>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// Absolute root of the repository (`git rev-parse --show-toplevel`).
    pub root: String,
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub changes: Vec<Change>,
    /// HEAD commit (`None` before the first commit): when it changes, the gutter baseline is refreshed.
    pub head: Option<String>,
    /// The real git directory: in worktrees and submodules `.git` is a file and the directory lives elsewhere.
    pub git_dir: Option<String>,
    /// Fingerprint of every ref (branches, remote branches, tags, stash): changes when one is created, moved or deleted,
    /// even if HEAD stays where it is (the graph's labels depend on it).
    pub refs_stamp: u64,
}

impl Status {
    pub fn abs(&self, rel: &str) -> String {
        format!("{}/{rel}", self.root.trim_end_matches('/'))
    }

    /// Absolute path → most important mark; folders take the most important mark among their contents.
    pub fn marks(&self) -> HashMap<String, Mark> {
        let mut out: HashMap<String, Mark> = HashMap::new();
        let root = self.root.trim_end_matches('/');
        for c in &self.changes {
            let Some(mark) = c.staged.max(c.unstaged) else { continue };
            let path = self.abs(&c.path);
            let mut at = path.as_str();
            loop {
                let e = out.entry(at.to_string()).or_insert(mark);
                *e = (*e).max(mark);
                match at.rfind('/') {
                    Some(i) if i > root.len() => at = &at[..i],
                    _ => break,
                }
            }
        }
        out
    }

    pub fn staged(&self) -> impl Iterator<Item = &Change> {
        self.changes.iter().filter(|c| c.staged.is_some())
    }

    pub fn unstaged(&self) -> impl Iterator<Item = &Change> {
        self.changes.iter().filter(|c| c.unstaged.is_some())
    }
}

/// Output of `git status --porcelain=v1 -z --branch`.
pub fn parse_status(root: &str, out: &[u8]) -> Status {
    let mut st = Status { root: root.to_string(), ..Default::default() };
    let mut parts = out.split(|&b| b == 0).filter(|p| !p.is_empty());
    while let Some(entry) = parts.next() {
        let text = String::from_utf8_lossy(entry);
        if let Some(head) = text.strip_prefix("## ") {
            parse_branch(head, &mut st);
            continue;
        }
        if entry.len() < 4 {
            continue;
        }
        let (x, y) = (entry[0], entry[1]);
        let path = String::from_utf8_lossy(&entry[3..]).into_owned();
        // Rename/copy: the old path is a separate entry.
        let orig = (x == b'R' || x == b'C' || y == b'R' || y == b'C').then(|| parts.next().map(|o| String::from_utf8_lossy(o).into_owned())).flatten();
        let conflict = matches!((x, y), (b'D', b'D') | (b'A', b'A') | (b'U', _) | (_, b'U'));
        let (staged, unstaged) = if conflict {
            (None, Some(Mark::Conflict))
        } else if x == b'?' {
            (None, Some(Mark::Untracked))
        } else {
            (Mark::from_code(x), Mark::from_code(y))
        };
        st.changes.push(Change { path, orig, staged, unstaged });
    }
    st.changes.sort_by(|a, b| a.path.cmp(&b.path));
    st
}

fn parse_branch(head: &str, st: &mut Status) {
    // "main...origin/main [ahead 1, behind 2]", "No commits yet on main", "HEAD (no branch)"
    let (names, counts) = match head.split_once(" [") {
        Some((n, c)) => (n, Some(c.trim_end_matches(']'))),
        None => (head, None),
    };
    if let Some(b) = names.strip_prefix("No commits yet on ").or_else(|| names.strip_prefix("Initial commit on ")) {
        st.branch = Some(b.to_string());
    } else if names.starts_with("HEAD (no branch)") {
        st.branch = None;
    } else {
        let (b, up) = names.split_once("...").map_or((names, None), |(b, u)| (b, Some(u)));
        st.branch = Some(b.to_string());
        st.upstream = up.map(str::to_string);
    }
    for c in counts.into_iter().flat_map(|c| c.split(", ")) {
        if let Some(n) = c.strip_prefix("ahead ") {
            st.ahead = n.parse().unwrap_or(0);
        } else if let Some(n) = c.strip_prefix("behind ") {
            st.behind = n.parse().unwrap_or(0);
        }
    }
}

/// Status if the folder is inside a git repository; `None` otherwise.
/// `trusted == false`: the folder's author is not trusted. A repository's `core.fsmonitor` can run a command every time `git status`
/// runs (on open and every 4 seconds), so it is disabled in untrusted folders (the built-in watcher is unaffected).
pub fn status_for(host: &dyn Host, dir: &str, trusted: bool) -> Result<Option<Status>, String> {
    let top = git(host, dir, &["rev-parse", "--show-toplevel", "--show-prefix", "--absolute-git-dir"])?;
    if !top.ok() {
        return Ok(None); // not a repository
    }
    // The root is derived from the opened path: --show-toplevel resolves symlinks
    // (/tmp → /private/tmp), which would not match the paths in the tree.
    let text = top.stdout_str();
    let mut lines = text.lines();
    let toplevel = lines.next().unwrap_or_default().trim().to_string();
    let prefix = lines.next().unwrap_or_default().trim().trim_end_matches('/');
    let git_dir = lines.next().map(|l| l.trim().to_string()).filter(|l| !l.is_empty());
    let dir = dir.trim_end_matches('/');
    let root = if prefix.is_empty() { dir.to_string() } else { dir.strip_suffix(&format!("/{prefix}")).map(str::to_string).unwrap_or(toplevel) };
    // --no-optional-locks: status must not rewrite the index; otherwise the watched
    // .git/index changes and triggers a new refresh (loop).
    let mut args = vec!["--no-optional-locks"];
    if !trusted {
        args.extend(["-c", "core.fsmonitor=false"]);
    }
    args.extend(["status", "--porcelain=v1", "-z", "--branch", "--untracked-files=all"]);
    let out = git_ok(host, &root, &args)?;
    let mut st = parse_status(&root, out.as_bytes());
    st.git_dir = git_dir;
    // One process for HEAD and all refs. Exit code 1 only means "no refs yet" (a repository without commits).
    let refs = git(host, &root, &["show-ref", "--head"]).ok().filter(|o| o.ok() || o.code == Some(1)).map(|o| o.stdout_str()).unwrap_or_default();
    (st.head, st.refs_stamp) = parse_show_ref(&refs);
    Ok(Some(st))
}

/// `git show-ref --head` output: HEAD's commit (the line ending in " HEAD") and a fingerprint of all lines.
pub fn parse_show_ref(out: &str) -> (Option<String>, u64) {
    use std::hash::{Hash as _, Hasher as _};
    let head = out.lines().find_map(|l| l.strip_suffix(" HEAD")).map(|h| h.trim().to_string()).filter(|h| !h.is_empty());
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    out.hash(&mut hasher);
    (head, hasher.finish())
}

/// Content at a revision: `rev` is "HEAD" or "" (index). `None` if the file is not there.
pub fn show(host: &dyn Host, root: &str, rev: &str, rel: &str) -> Option<String> {
    let spec = format!("{rev}:{rel}");
    // `--no-textconv`: `.gitattributes` + `diff.<driver>.textconv` can run a command and yield converted text instead of the raw content.
    let out = git(host, root, &["show", "--no-textconv", &spec]).ok()?;
    // The editor does not put the BOM into the text: line 0 must not always look different in the comparison.
    out.ok().then(|| String::from_utf8_lossy(&out.stdout).trim_start_matches('\u{feff}').replace("\r\n", "\n"))
}

pub fn stage(host: &dyn Host, root: &str, paths: &[String]) -> Result<(), String> {
    let mut args = vec!["add", "-A", "--"];
    args.extend(paths.iter().map(String::as_str));
    git_ok(host, root, &args).map(|_| ())
}

pub fn unstage(host: &dyn Host, root: &str, paths: &[String]) -> Result<(), String> {
    // Before the first commit there is no HEAD: `rm --cached` is needed.
    let has_head = git(host, root, &["rev-parse", "--verify", "-q", "HEAD"]).is_ok_and(|o| o.ok());
    let mut args = if has_head { vec!["reset", "-q", "HEAD", "--"] } else { vec!["rm", "-q", "--cached", "-r", "--"] };
    args.extend(paths.iter().map(String::as_str));
    git_ok(host, root, &args).map(|_| ())
}

/// Discard unstaged changes. Untracked files are deleted (the UI asks first).
pub fn discard(host: &dyn Host, root: &str, changes: &[Change]) -> Result<(), String> {
    let (untracked, tracked): (Vec<&Change>, Vec<&Change>) = changes.iter().partition(|c| c.unstaged == Some(Mark::Untracked));
    if !tracked.is_empty() {
        let mut args = vec!["checkout", "-q", "--"];
        args.extend(tracked.iter().map(|c| c.path.as_str()));
        git_ok(host, root, &args)?;
    }
    if !untracked.is_empty() {
        let mut args = vec!["clean", "-f", "-q", "--"];
        args.extend(untracked.iter().map(|c| c.path.as_str()));
        git_ok(host, root, &args)?;
    }
    Ok(())
}

/// Identity commits in this repository are made with: (name, email). `None` if not set.
pub fn identity(host: &dyn Host, root: &str) -> (Option<String>, Option<String>) {
    let get =
        |key: &str| git(host, root, &["config", "--get", key]).ok().filter(|o| o.ok()).map(|o| o.stdout_str().trim().to_string()).filter(|v| !v.is_empty());
    (get("user.name"), get("user.email"))
}

/// Turn git's error message into guidance for the user (what to do if the identity is missing).
pub fn explain_commit_error(err: &str) -> String {
    let e = err.to_lowercase();
    if e.contains("author identity unknown") || e.contains("unable to auto-detect email") || e.contains("please tell me who you are") {
        return crate::i18n::tf(
            "Git doesn't know who you are on this host. Run these in the terminal: {0} and {1}",
            &[&"git config --global user.name \"Your Name\"", &"git config --global user.email you@example.com"],
        );
    }
    err.to_string()
}

/// How a commit is made. `all`: also stage tracked changes (for "Commit All" when nothing is staged). `amend`: change the
/// last commit; if the message is empty the old message is kept. `signoff`: add a `Signed-off-by` trailer.
/// `no_verify`: skip the pre-commit and commit-msg hooks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CommitOptions {
    pub all: bool,
    pub amend: bool,
    pub signoff: bool,
    pub no_verify: bool,
}

/// The message goes through stdin (`-F -`): no quoting or line-ending trouble.
pub fn commit(host: &dyn Host, root: &str, message: &str, opts: CommitOptions) -> Result<String, String> {
    let mut args = vec!["commit", "-q"];
    let keep_message = opts.amend && message.trim().is_empty();
    args.extend(if keep_message { ["--no-edit"].as_slice() } else { ["-F", "-"].as_slice() });
    for (on, flag) in [(opts.all, "-a"), (opts.amend, "--amend"), (opts.signoff, "--signoff"), (opts.no_verify, "--no-verify")] {
        if on {
            args.push(flag);
        }
    }
    let stdin = (!keep_message).then(|| message.as_bytes().to_vec());
    let out = host.exec("git", &args, Some(root), stdin).map_err(|e| e.to_string())?;
    if out.ok() {
        Ok(git_ok(host, root, &["log", "-1", "--format=%h %s"]).unwrap_or_default().trim().to_string())
    } else {
        let msg = [out.stderr_str(), out.stdout_str().trim().to_string()].into_iter().find(|s| !s.is_empty());
        Err(explain_commit_error(&msg.unwrap_or_else(|| crate::i18n::t("git commit failed").into())))
    }
}

/// Undo the last commit: changes stay staged, and the message is returned (so it can be written back into the box).
/// On the first commit there is no `HEAD~1`: the branch ref is deleted (VS Code does the same).
pub fn undo_last_commit(host: &dyn Host, root: &str) -> Result<String, String> {
    let message = git_ok(host, root, &["log", "-1", "--format=%B"]).map_err(|_| crate::i18n::t("There is no commit to undo").to_string())?;
    let has_parent = git(host, root, &["rev-parse", "--verify", "-q", "HEAD~1"]).is_ok_and(|o| o.ok());
    if has_parent {
        git_ok(host, root, &["reset", "-q", "--soft", "HEAD~1"])?;
    } else {
        git_ok(host, root, &["update-ref", "-d", "HEAD"])?;
    }
    Ok(message.trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::repo;

    #[test]
    fn show_ref_gives_head_and_a_fingerprint_that_follows_every_ref() {
        let base = "1111 HEAD\n1111 refs/heads/main\n2222 refs/remotes/origin/main\n";
        let (head, stamp) = parse_show_ref(base);
        assert_eq!(head.as_deref(), Some("1111"));
        // A new branch on the same commit, a moved remote branch, a tag: HEAD is the same, the fingerprint is not.
        for changed in [format!("{base}1111 refs/heads/feature/x\n"), base.replace("2222", "3333"), format!("{base}1111 refs/tags/v1\n")] {
            let (h, s) = parse_show_ref(&changed);
            assert_eq!(h.as_deref(), Some("1111"));
            assert_ne!(s, stamp, "{changed}");
        }
        // No commits yet: no HEAD.
        assert_eq!(parse_show_ref("").0, None);
    }

    #[test]
    fn parses_porcelain_status() {
        let out = b"## main...origin/main [ahead 2, behind 1]\0 M src/a.rs\0M  b.txt\0MM c.rs\0A  yeni.rs\0R  yeni ad.rs\0eski.rs\0?? dir/x.md\0UU cakis.rs\0 D silindi.rs\0";
        let st = parse_status("/r", out);
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(st.upstream.as_deref(), Some("origin/main"));
        assert_eq!((st.ahead, st.behind), (2, 1));
        let get = |p: &str| st.changes.iter().find(|c| c.path == p).unwrap().clone();
        assert_eq!((get("src/a.rs").staged, get("src/a.rs").unstaged), (None, Some(Mark::Modified)));
        assert_eq!((get("b.txt").staged, get("b.txt").unstaged), (Some(Mark::Modified), None));
        assert_eq!((get("c.rs").staged, get("c.rs").unstaged), (Some(Mark::Modified), Some(Mark::Modified)));
        assert_eq!(get("yeni ad.rs").orig.as_deref(), Some("eski.rs"), "path with spaces and the old name");
        assert_eq!(get("dir/x.md").unstaged, Some(Mark::Untracked));
        assert_eq!(get("cakis.rs").unstaged, Some(Mark::Conflict));
        assert_eq!(st.staged().count(), 4);
        assert_eq!(st.unstaged().count(), 5);

        let marks = st.marks();
        assert_eq!(marks["/r/src/a.rs"], Mark::Modified);
        assert_eq!(marks["/r/src"], Mark::Modified, "folder carries its contents' mark");
        assert_eq!(marks["/r/dir"], Mark::Untracked);
        assert_eq!(marks["/r/silindi.rs"], Mark::Deleted);
        assert!(!marks.contains_key("/r"), "root is not marked");
    }

    #[test]
    fn branch_header_variants() {
        assert_eq!(parse_status("/r", b"## No commits yet on main\0").branch.as_deref(), Some("main"));
        assert_eq!(parse_status("/r", b"## HEAD (no branch)\0").branch, None);
        let st = parse_status("/r", b"## feature/x\0");
        assert_eq!((st.branch.as_deref(), st.upstream), (Some("feature/x"), None));
    }

    #[test]
    fn missing_identity_gets_actionable_message() {
        let m = explain_commit_error("Author identity unknown\n\n*** Please tell me who you are.\n\nRun\n\n  git config --global user.email");
        assert!(m.contains("git config --global user.name"), "{m}");
        assert_eq!(explain_commit_error("nothing to commit"), "nothing to commit");
    }

    #[test]
    fn identity_is_read_from_the_repo_config() {
        let dir = std::env::temp_dir().join(format!("codeyard-identity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| crate::test_git::git().args(args).current_dir(&dir).output().is_ok_and(|o| o.status.success());
        if !run(&["init", "-q"]) {
            return;
        }
        let host = host::local();
        let d = dir.to_string_lossy().into_owned();
        // Local setting: read independently of the global setting.
        assert!(run(&["config", "user.name", "Ada Lovelace"]) && run(&["config", "user.email", "ada@example.com"]));
        assert_eq!(identity(&host, &d), (Some("Ada Lovelace".into()), Some("ada@example.com".into())));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A repository can name a program in `core.fsmonitor` that `git status` runs; in an untrusted folder it must not run.
    #[cfg(unix)]
    #[test]
    fn status_in_an_untrusted_folder_does_not_run_the_repository_fsmonitor_command() {
        use std::os::unix::fs::PermissionsExt;
        let Some((dir, run)) = repo("fsmonitor-hook") else { return };
        let marker = dir.join("..").join(format!("codeyard-fsmon-marker-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let hook = dir.join("hook.sh");
        std::fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\nprintf '\\0'\n", marker.display())).unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        let hook_path = hook.to_string_lossy().into_owned();
        if run(&["config", "core.fsmonitor", &hook_path]).is_none() {
            return;
        }
        let host = host::local();
        let d = dir.to_string_lossy().into_owned();
        // Untrusted: the command is not run.
        assert!(status_for(&host, &d, false).unwrap().is_some());
        assert!(!marker.exists(), "git status executed the repository's fsmonitor command in an untrusted folder");
        // Trusted: git behaves as in a terminal (this proves the test would notice the difference).
        let _ = status_for(&host, &d, true);
        assert!(marker.exists(), "the fsmonitor hook is expected to run for a trusted folder");
        let _ = std::fs::remove_file(&marker);
    }

    #[test]
    fn commit_flags_add_a_signoff_trailer_and_skip_hooks() {
        let Some((dir, run)) = repo("commit-flags") else { return };
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        // Plain commit: no trailer.
        std::fs::write(dir.join("a.txt"), "a\n").unwrap();
        run(&["add", "-A"]).unwrap();
        commit(&host, &root, "plain", CommitOptions::default()).unwrap();
        assert!(!run(&["log", "-1", "--format=%B"]).unwrap().contains("Signed-off-by"));
        // Signed off.
        std::fs::write(dir.join("b.txt"), "b\n").unwrap();
        run(&["add", "-A"]).unwrap();
        commit(&host, &root, "signed", CommitOptions { signoff: true, ..Default::default() }).unwrap();
        assert!(run(&["log", "-1", "--format=%B"]).unwrap().contains("Signed-off-by: Test <t@example.com>"));
        // A pre-commit hook that refuses: the commit fails, and --no-verify gets past it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let hook = dir.join(".git/hooks/pre-commit");
            std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
            std::fs::write(&hook, "#!/bin/sh\necho refused by hook >&2\nexit 1\n").unwrap();
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::write(dir.join("c.txt"), "c\n").unwrap();
            run(&["add", "-A"]).unwrap();
            assert!(commit(&host, &root, "hooked", CommitOptions::default()).unwrap_err().contains("refused by hook"));
            commit(&host, &root, "skipped", CommitOptions { no_verify: true, ..Default::default() }).unwrap();
            assert_eq!(run(&["log", "-1", "--format=%s"]).unwrap().trim(), "skipped");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
