//! Git: the system `git` runs through the agent, with the same code path
//! locally and over SSH. This file holds only the commands and output parsing (independent of the UI).

use std::collections::HashMap;
use std::ops::Range;

use host::Host;

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

fn git(host: &dyn Host, cwd: &str, args: &[&str]) -> Result<host::ExecOutput, String> {
    let out = host.exec("git", args, Some(cwd), None).map_err(|e| e.to_string())?;
    if out.code == Some(127) {
        return Err(crate::i18n::t("git is not installed on this host").into());
    }
    Ok(out)
}

pub(crate) fn git_ok(host: &dyn Host, cwd: &str, args: &[&str]) -> Result<String, String> {
    let out = git(host, cwd, args)?;
    if out.ok() {
        Ok(out.stdout_str())
    } else {
        let err = out.stderr_str();
        Err(if err.is_empty() { crate::i18n::tf("git {} failed", &[args.first().unwrap_or(&"")]) } else { err })
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
    st.head = git(host, &root, &["rev-parse", "-q", "--verify", "HEAD"]).ok().filter(|o| o.ok()).map(|o| o.stdout_str().trim().to_string());
    Ok(Some(st))
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

/// Turns English `relative_time` text such as "5 minutes ago" into the full phrase in the current language ("5 dakika önce");
/// unrecognized text is left as is. (`relative_time` itself is an English intermediate form: `short_ago` parses it.)
pub fn ago_phrase(text: &str) -> String {
    use crate::i18n::{t, tn};
    let text = text.trim();
    if text == "just now" {
        return t("just now").into();
    }
    let mut parts = text.split_whitespace();
    let (Some(n), Some(unit)) = (parts.next(), parts.next()) else { return text.to_string() };
    let Ok(n) = n.parse::<usize>() else { return text.to_string() };
    match unit.trim_end_matches(',').trim_end_matches('s') {
        "minute" => tn("{} minute ago", "{} minutes ago", n, &[]),
        "hour" => tn("{} hour ago", "{} hours ago", n, &[]),
        "day" => tn("{} day ago", "{} days ago", n, &[]),
        "week" => tn("{} week ago", "{} weeks ago", n, &[]),
        "month" => tn("{} month ago", "{} months ago", n, &[]),
        "year" => tn("{} year ago", "{} years ago", n, &[]),
        _ => text.to_string(),
    }
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

/// The message goes through stdin (`-F -`): no quoting or line-ending trouble. `all`: also stage
/// tracked changes (for "Commit All" when nothing is staged).
/// `amend`: change the last commit; if the message is empty the old message is kept.
pub fn commit(host: &dyn Host, root: &str, message: &str, all: bool, amend: bool) -> Result<String, String> {
    let mut args = vec!["commit", "-q"];
    let keep_message = amend && message.trim().is_empty();
    args.extend(if keep_message { ["--no-edit"].as_slice() } else { ["-F", "-"].as_slice() });
    if all {
        args.push("-a");
    }
    if amend {
        args.push("--amend");
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

/// `git stash push`; `untracked`: untracked files too.
pub fn stash(host: &dyn Host, root: &str, untracked: bool) -> Result<(), String> {
    let mut args = vec!["stash", "push", "-q"];
    if untracked {
        args.push("-u");
    }
    let before = git_ok(host, root, &["stash", "list", "--format=%H"]).unwrap_or_default();
    git_ok(host, root, &args)?;
    // With no changes git returns success but no stash is created.
    if git_ok(host, root, &["stash", "list", "--format=%H"]).unwrap_or_default() == before {
        return Err(crate::i18n::t("There are no local changes to stash").into());
    }
    Ok(())
}

/// (ref "stash@{0}", description), newest first.
pub fn stash_list(host: &dyn Host, root: &str) -> Result<Vec<(String, String)>, String> {
    let out = git_ok(host, root, &["stash", "list", "--format=%gd%x09%s"])?;
    Ok(out.lines().filter_map(|l| l.split_once('\t')).map(|(r, m)| (r.to_string(), m.to_string())).collect())
}

/// `pop`: apply and drop from the list; otherwise just apply.
pub fn stash_apply(host: &dyn Host, root: &str, stash: &str, pop: bool) -> Result<(), String> {
    git_ok(host, root, &["stash", if pop { "pop" } else { "apply" }, "-q", stash]).map(|_| ())
}

pub fn stash_drop(host: &dyn Host, root: &str, stash: &str) -> Result<(), String> {
    git_ok(host, root, &["stash", "drop", "-q", stash]).map(|_| ())
}

/// A commit in a file's history.
#[derive(Clone, Debug, PartialEq)]
pub struct LogEntry {
    pub hash: String,
    pub short: String,
    pub author: String,
    /// "3 days ago"
    pub when: String,
    pub subject: String,
    /// The path at that commit (may be the old name after a rename).
    pub path: String,
}

const LOG_FORMAT: &str = "--format=%x1e%H%x1f%h%x1f%an%x1f%ar%x1f%s";

/// Output of `git log --follow --name-only` (records separated by \x1e, fields by \x1f).
pub fn parse_log(out: &str) -> Vec<LogEntry> {
    out.split('\x1e')
        .filter_map(|rec| {
            let (head, names) = rec.split_once('\n').unwrap_or((rec, ""));
            let f: Vec<&str> = head.split('\x1f').collect();
            let [hash, short, author, when, subject] = f[..] else { return None };
            let path = names.lines().map(str::trim).find(|l| !l.is_empty())?.to_string();
            Some(LogEntry { hash: hash.into(), short: short.into(), author: author.into(), when: when.into(), subject: subject.into(), path })
        })
        .collect()
}

/// A file's history (follows renames), newest first.
pub fn file_log(host: &dyn Host, root: &str, rel: &str, limit: usize) -> Result<Vec<LogEntry>, String> {
    let n = format!("-n{limit}");
    git_ok(host, root, &["log", "--follow", &n, LOG_FORMAT, "--name-only", "--", rel]).map(|o| parse_log(&o))
}

/// The commit that last changed the line. `contents`: unsaved text (so lines do not shift).
/// For a line not yet committed, `hash` is all zeros.
pub fn blame_line(host: &dyn Host, root: &str, rel: &str, line: usize, contents: Option<String>) -> Result<LogEntry, String> {
    let range = format!("-L{},{}", line + 1, line + 1);
    let mut args = vec!["blame", "--no-textconv", "--porcelain", range.as_str()];
    if contents.is_some() {
        args.extend(["--contents", "-"]);
    }
    args.extend(["--", rel]);
    let out = host.exec("git", &args, Some(root), contents.map(String::into_bytes)).map_err(|e| e.to_string())?;
    if !out.ok() {
        return Err(out.stderr_str());
    }
    parse_blame(&out.stdout_str(), rel).ok_or_else(|| crate::i18n::t("git blame gave no answer").into())
}

pub fn parse_blame(out: &str, rel: &str) -> Option<LogEntry> {
    let mut lines = out.lines();
    let hash = lines.next()?.split_whitespace().next()?.to_string();
    let (mut author, mut time, mut subject) = (String::new(), 0i64, String::new());
    for l in lines {
        if let Some(v) = l.strip_prefix("author ") {
            author = v.to_string();
        } else if let Some(v) = l.strip_prefix("author-time ") {
            time = v.parse().unwrap_or(0);
        } else if let Some(v) = l.strip_prefix("summary ") {
            subject = v.to_string();
        }
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    Some(LogEntry { short: hash.chars().take(7).collect(), hash, author, when: relative_time(now - time), subject, path: rel.to_string() })
}

/// "4 days ago" → "4d", "25 minutes ago" → "25m", "just now" → "now"; unrecognized text is left as is.
/// Units: s, m, h, d, w, mo, y ("1 year, 2 months ago" takes only the first part: "1y").
pub fn short_ago(text: &str) -> String {
    use crate::i18n::tf;
    let t = text.trim();
    if t == "just now" || t == "now" {
        return crate::i18n::t("now").into();
    }
    let mut parts = t.split_whitespace();
    let (Some(n), Some(unit)) = (parts.next(), parts.next()) else { return t.to_string() };
    let Ok(n) = n.parse::<u64>() else { return t.to_string() };
    match unit.trim_end_matches(',').trim_end_matches('s') {
        "second" => tf("{}s", &[&n]),
        "minute" => tf("{}m", &[&n]),
        "hour" => tf("{}h", &[&n]),
        "day" => tf("{}d", &[&n]),
        "week" => tf("{}w", &[&n]),
        "month" => tf("{}mo", &[&n]),
        "year" => tf("{}y", &[&n]),
        _ => t.to_string(),
    }
}

/// "just now", "5 minutes ago", "3 days ago", "2 years ago".
pub fn relative_time(secs: i64) -> String {
    let units = [(31_536_000, "year"), (2_592_000, "month"), (604_800, "week"), (86_400, "day"), (3_600, "hour"), (60, "minute")];
    match units.iter().find(|(u, _)| secs >= *u) {
        Some((u, name)) => {
            let n = secs / u;
            format!("{n} {name}{} ago", if n == 1 { "" } else { "s" })
        }
        None => "just now".into(),
    }
}

/// (local branches, remote branches "origin/x"); `HEAD` aliases excluded.
pub fn branches(host: &dyn Host, root: &str) -> Result<(Vec<String>, Vec<String>), String> {
    let out = git_ok(host, root, &["for-each-ref", "--format=%(refname)", "refs/heads", "refs/remotes"])?;
    Ok(parse_branches(&out))
}

/// A branch, tag or remote name that git would not take for an option. Git lets a repository hold a ref called
/// `--exec=cmd` (a hostile server can name its default branch so), and `git rebase --exec` runs it: names that start
/// with `-` never reach a command line.
pub(crate) fn plain_name(name: &str) -> Result<&str, String> {
    if name.is_empty() || name.starts_with('-') || name.contains(['\0', '\n', '\r']) {
        return Err(crate::i18n::tf("“{}” is not a usable branch or remote name", &[&name.escape_debug()]));
    }
    Ok(name)
}

pub fn parse_branches(out: &str) -> (Vec<String>, Vec<String>) {
    let (mut local, mut remote) = (Vec::new(), Vec::new());
    for line in out.lines() {
        if let Some(b) = line.strip_prefix("refs/heads/") {
            if plain_name(b).is_ok() {
                local.push(b.to_string());
            }
        } else if let Some(b) = line.strip_prefix("refs/remotes/").filter(|b| !b.ends_with("/HEAD")) {
            remote.push(b.to_string());
        }
    }
    (local, remote)
}

/// Switch branch. For a remote branch ("origin/x") a tracking local branch is created; `create`: new branch.
pub fn checkout(host: &dyn Host, root: &str, branch: &str, remote: bool, create: bool) -> Result<(), String> {
    plain_name(branch)?;
    let args: Vec<&str> = match (remote, create) {
        (_, true) => vec!["checkout", "-q", "-b", branch],
        (true, false) => vec!["checkout", "-q", "--track", branch],
        (false, false) => vec!["checkout", "-q", branch],
    };
    git_ok(host, root, &args).map(|_| ())
}

/// Rebase the current branch onto `onto` (non-interactive). `Ok(true)` on conflict: rebase in progress
/// (resolve the conflicts, then `rebase_continue` or `rebase_abort`).
pub fn rebase(host: &dyn Host, root: &str, onto: &str) -> Result<bool, String> {
    plain_name(onto)?;
    if rebase_in_progress(host, root) {
        return Err(crate::i18n::t("A rebase is already in progress: use Git: Continue Rebase or Git: Abort Rebase").into());
    }
    let out = git(host, root, &["-c", "core.editor=true", "rebase", onto])?;
    if out.ok() {
        return Ok(false);
    }
    // Conflict: a half-done rebase + an unresolved file (independent of the language of git's output).
    if rebase_in_progress(host, root) && has_unmerged(host, root) {
        return Ok(true);
    }
    Err(failure_text(&out))
}

pub fn rebase_continue(host: &dyn Host, root: &str) -> Result<bool, String> {
    let out = git(host, root, &["-c", "core.editor=true", "rebase", "--continue"])?;
    if out.ok() {
        return Ok(false);
    }
    if rebase_in_progress(host, root) && has_unmerged(host, root) {
        return Ok(true);
    }
    Err(failure_text(&out))
}

/// Whether there is an unresolved (conflicted) file.
pub fn has_unmerged(host: &dyn Host, root: &str) -> bool {
    git_ok(host, root, &["diff", "--name-only", "--diff-filter=U"]).is_ok_and(|o| !o.trim().is_empty())
}

/// Error message: stderr first (the real cause is there), otherwise stdout.
fn failure_text(out: &host::ExecOutput) -> String {
    let err = out.stderr_str();
    let text = if err.trim().is_empty() { out.stdout_str() } else { err };
    text.trim().to_string()
}

pub fn rebase_abort(host: &dyn Host, root: &str) -> Result<(), String> {
    git_ok(host, root, &["rebase", "--abort"]).map(|_| ())
}

pub fn rebase_in_progress(host: &dyn Host, root: &str) -> bool {
    ["rebase-merge", "rebase-apply"].iter().any(|d| {
        git_ok(host, root, &["rev-parse", "--git-path", d])
            .ok()
            .map(|p| p.trim().to_string())
            .is_some_and(|p| host.stat(&if p.starts_with('/') { p } else { format!("{}/{p}", root.trim_end_matches('/')) }).is_ok())
    })
}

/// `git worktree list --porcelain` → (path, branch or None).
pub fn worktrees(host: &dyn Host, root: &str) -> Result<Vec<(String, Option<String>)>, String> {
    Ok(parse_worktrees(&git_ok(host, root, &["worktree", "list", "--porcelain"])?))
}

pub fn parse_worktrees(out: &str) -> Vec<(String, Option<String>)> {
    let mut v: Vec<(String, Option<String>)> = Vec::new();
    for line in out.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            v.push((p.to_string(), None));
        } else if let (Some(b), Some(last)) = (line.strip_prefix("branch "), v.last_mut()) {
            last.1 = Some(b.trim_start_matches("refs/heads/").to_string());
        }
    }
    v
}

/// New worktree: checks out `branch` if given; otherwise creates it from the current branch with `-b`.
pub fn add_worktree(host: &dyn Host, root: &str, path: &str, branch: &str) -> Result<(), String> {
    let exists = branches(host, root)?.0.iter().any(|b| b == branch);
    if exists {
        git_ok(host, root, &["worktree", "add", "--", path, branch]).map(|_| ())
    } else {
        git_ok(host, root, &["worktree", "add", "-b", branch, "--", path]).map(|_| ())
    }
}

/// Merge the branch into the current branch (`--no-edit`). On conflict `Ok(true)`: merge in progress,
/// conflicts are resolved in Source Control. Any other failure is `Err`.
pub fn merge(host: &dyn Host, root: &str, branch: &str) -> Result<bool, String> {
    plain_name(branch)?;
    let out = git(host, root, &["merge", "--no-edit", branch])?;
    if out.ok() {
        return Ok(false);
    }
    if merge_in_progress(host, root) && has_unmerged(host, root) {
        return Ok(true);
    }
    Err(failure_text(&out))
}

pub fn merge_abort(host: &dyn Host, root: &str) -> Result<(), String> {
    git_ok(host, root, &["merge", "--abort"]).map(|_| ())
}

/// Whether `.git/MERGE_HEAD` exists (merge half-done).
pub fn merge_in_progress(host: &dyn Host, root: &str) -> bool {
    git(host, root, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_ok_and(|o| o.ok())
}

/// Delete a local branch. Without `force` an unmerged branch is not deleted (git `-d`).
pub fn delete_branch(host: &dyn Host, root: &str, branch: &str, force: bool) -> Result<(), String> {
    git_ok(host, root, &["branch", if force { "-D" } else { "-d" }, "--", branch]).map(|_| ())
}

/// Number of commits on `branch` that are not on the current branch (for the delete confirmation).
pub fn unmerged_commits(host: &dyn Host, root: &str, branch: &str) -> Result<usize, String> {
    let range = format!("HEAD..{branch}");
    let out = git_ok(host, root, &["rev-list", "--count", &range])?;
    out.trim().parse().map_err(|_| crate::i18n::tf("unexpected output: {}", &[&out.trim()]))
}

pub fn rename_branch(host: &dyn Host, root: &str, old: &str, new: &str) -> Result<(), String> {
    git_ok(host, root, &["branch", "-m", "--", old, new]).map(|_| ())
}

/// Remotes: (name, URL).
pub fn remotes(host: &dyn Host, root: &str) -> Result<Vec<(String, String)>, String> {
    let out = git_ok(host, root, &["remote", "-v"])?;
    Ok(parse_remotes(&out))
}

pub fn parse_remotes(out: &str) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = Vec::new();
    for line in out.lines() {
        let mut p = line.split_whitespace();
        if let (Some(n), Some(u)) = (p.next(), p.next())
            && !v.iter().any(|(x, _)| x == n)
        {
            v.push((n.to_string(), u.to_string()));
        }
    }
    v
}

pub fn add_remote(host: &dyn Host, root: &str, name: &str, url: &str) -> Result<(), String> {
    git_ok(host, root, &["remote", "add", "--", name, url]).map(|_| ())
}

pub fn remove_remote(host: &dyn Host, root: &str, name: &str) -> Result<(), String> {
    git_ok(host, root, &["remote", "remove", "--", name]).map(|_| ())
}

/// Publish the current branch to the first remote (error if none) and set up tracking.
pub fn publish_branch(host: &dyn Host, root: &str) -> Result<String, String> {
    let remotes = remotes(host, root)?;
    let remote = remotes
        .iter()
        .find(|(n, _)| n == "origin")
        .or(remotes.first())
        .map(|(n, _)| n.clone())
        .ok_or(crate::i18n::t("This repository has no remote. Add one with Git: Add Remote…"))?;
    plain_name(&remote)?;
    git_ok(host, root, &["push", "-u", &remote, "HEAD"])?;
    Ok(remote)
}

/// Network operations. If the server asks for a password no prompt is shown (GIT_TERMINAL_PROMPT=0); an error is returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncOp {
    Pull,
    Push,
    Fetch,
}

pub fn sync(host: &dyn Host, root: &str, op: SyncOp) -> Result<(), String> {
    match op {
        SyncOp::Pull => git_ok(host, root, &["pull", "--no-rebase", "--no-edit"]).map(|_| ()),
        SyncOp::Fetch => git_ok(host, root, &["fetch", "--prune"]).map(|_| ()),
        SyncOp::Push => match git_ok(host, root, &["push"]) {
            Ok(_) => Ok(()),
            // Branch not yet published: push to origin, like "Publish Branch" in VS Code.
            Err(e) if e.contains("has no upstream branch") => git_ok(host, root, &["push", "-u", "origin", "HEAD"]).map(|_| ()),
            Err(e) => Err(e),
        },
    }
}

// --- Line diff (gutter and diff view) ---

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineMark {
    Added,
    Modified,
    /// There are deleted line(s) right above this line.
    DeletedAbove,
}

/// Mark for each line of the new text (mostly `None`).
pub fn line_marks(base: &str, current: &str) -> Vec<Option<LineMark>> {
    // Unchanged file (the most common case): skip the diff computation, no allocation.
    if base == current {
        return Vec::new();
    }
    let lines = current.split('\n').count();
    let mut marks = vec![None; lines];
    for (tag, _old, new) in diff_ops(base, current) {
        match tag {
            similar::DiffTag::Insert => new.clone().for_each(|i| set(&mut marks, i, LineMark::Added)),
            similar::DiffTag::Replace => new.clone().for_each(|i| set(&mut marks, i, LineMark::Modified)),
            similar::DiffTag::Delete => {
                let at = new.start.min(lines.saturating_sub(1));
                if marks.get(at).is_some_and(|m| m.is_none()) {
                    marks[at] = Some(LineMark::DeletedAbove);
                }
            }
            similar::DiffTag::Equal => {}
        }
    }
    marks
}

fn set(marks: &mut [Option<LineMark>], i: usize, m: LineMark) {
    if let Some(slot) = marks.get_mut(i) {
        *slot = Some(m);
    }
}

/// (kind, old lines, new lines). Lines are split on `\n`.
pub fn diff_ops(base: &str, current: &str) -> Vec<(similar::DiffTag, Range<usize>, Range<usize>)> {
    let diff = similar::TextDiff::configure().algorithm(similar::Algorithm::Myers).timeout(std::time::Duration::from_millis(200)).diff_lines(base, current);
    diff.ops().iter().map(|op| op.as_tag_tuple()).collect()
}

/// Inline (character-level) diffs within a changed line pair: byte ranges in the old and new line.
/// Returns empty if the two lines are only slightly similar or very long (piecemeal highlighting would be noise; the whole line
/// is already painted).
pub fn intraline(old: &str, new: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    const MAX_LEN: usize = 2000;
    const MIN_RATIO: f32 = 0.4;
    if old.len() > MAX_LEN || new.len() > MAX_LEN {
        return Default::default();
    }
    let diff = similar::TextDiff::configure().algorithm(similar::Algorithm::Myers).timeout(std::time::Duration::from_millis(20)).diff_chars(old, new);
    if diff.ratio() < MIN_RATIO {
        return Default::default();
    }
    fn push(v: &mut Vec<Range<usize>>, r: Range<usize>) {
        match v.last_mut() {
            Some(last) if last.end == r.start => last.end = r.end,
            _ => v.push(r),
        }
    }
    let (mut o, mut n) = (0, 0);
    let (mut old_marks, mut new_marks) = (Vec::new(), Vec::new());
    for change in diff.iter_all_changes() {
        let len = change.value().len();
        match change.tag() {
            similar::ChangeTag::Equal => {
                o += len;
                n += len;
            }
            similar::ChangeTag::Delete => {
                push(&mut old_marks, o..o + len);
                o += len;
            }
            similar::ChangeTag::Insert => {
                push(&mut new_marks, n..n + len);
                n += len;
            }
        }
    }
    (old_marks, new_marks)
}

/// A change block: line ranges in the old text and the new text (0-based).
/// For a pure deletion `new` is empty, for a pure addition `old` is empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub old: Range<usize>,
    pub new: Range<usize>,
}

pub fn hunks(base: &str, current: &str) -> Vec<Hunk> {
    diff_ops(base, current).into_iter().filter(|(tag, _, _)| *tag != similar::DiffTag::Equal).map(|(_, old, new)| Hunk { old, new }).collect()
}

/// Blocks intersecting `lo..=hi` (lines of the new text). A deletion block (empty new range)
/// maps to the line at the deletion point; a cursor on the next line selects it too.
pub fn hunks_touching(all: &[Hunk], lo: usize, hi: usize) -> Vec<Hunk> {
    all.iter().filter(|h| if h.new.is_empty() { (lo..=hi).contains(&h.new.start) } else { h.new.start <= hi && lo < h.new.end }).cloned().collect()
}

/// Apply the selected blocks. `to_current`: start from `base`, put `current`'s lines into the blocks
/// (stage: index + blocks). Otherwise start from `current`, put `base`'s lines into the blocks
/// (revert a block). Lines are kept together with their terminators.
#[cfg(test)]
pub fn apply_hunks(base: &str, current: &str, chosen: &[Hunk], to_current: bool) -> String {
    let (a, b): (Vec<&str>, Vec<&str>) = if to_current {
        (base.split_inclusive('\n').collect(), current.split_inclusive('\n').collect())
    } else {
        (current.split_inclusive('\n').collect(), base.split_inclusive('\n').collect())
    };
    // The range in `a` and its counterpart in `b`.
    let mut spans: Vec<(Range<usize>, Range<usize>)> =
        chosen.iter().map(|h| if to_current { (h.old.clone(), h.new.clone()) } else { (h.new.clone(), h.old.clone()) }).collect();
    spans.sort_by_key(|(r, _)| r.start);
    let mut out = String::new();
    let mut at = 0;
    for (r, with) in spans {
        out.extend(a[at.min(a.len())..r.start.min(a.len())].iter().copied());
        out.extend(b[with.start.min(b.len())..with.end.min(b.len())].iter().copied());
        at = r.end;
    }
    out.extend(a[at.min(a.len())..].iter().copied());
    out
}

/// Make the file's index content `content` (empty if absent); file mode is preserved. Does not touch
/// the working tree: only for "stage these blocks".
pub fn write_index(host: &dyn Host, root: &str, rel: &str, content: &str) -> Result<(), String> {
    let mode = git_ok(host, root, &["ls-files", "-s", "--", rel])
        .ok()
        .and_then(|o| o.split_whitespace().next().map(str::to_string))
        .unwrap_or_else(|| "100644".into());
    let out = host.exec("git", &["hash-object", "-w", "--stdin"], Some(root), Some(content.as_bytes().to_vec())).map_err(|e| e.to_string())?;
    if !out.ok() {
        return Err(out.stderr_str());
    }
    let sha = out.stdout_str().trim().to_string();
    let info = format!("{mode},{sha},{rel}");
    git_ok(host, root, &["update-index", "--add", "--cacheinfo", &info]).map(|_| ())
}

/// Stage the blocks touching lines `lo..=hi` (write to the index); returns how many blocks there were.
/// `current`: the text in the editor (may be unsaved). If the file is not in the index it counts as empty.
pub fn stage_hunks(host: &dyn Host, root: &str, rel: &str, current: &str, lo: usize, hi: usize) -> Result<usize, String> {
    // Index entry: if conflicted (stage != 0), a symlink or a submodule, leave it alone.
    let entries = git_ok(host, root, &["ls-files", "-s", "--", rel])?;
    let mut exists = false;
    for line in entries.lines() {
        let mut f = line.split_whitespace();
        let (mode, _sha, stage) = (f.next().unwrap_or(""), f.next(), f.next().unwrap_or("0"));
        if stage != "0" {
            return Err(crate::i18n::t("This file has merge conflicts: resolve them first (Source Control marks it resolved when staged)").into());
        }
        if mode == "120000" || mode == "160000" {
            return Err(crate::i18n::t("Staging changes inside a symlink or submodule is not supported").into());
        }
        exists = true;
    }
    let raw_bytes = if exists {
        let spec = format!(":{rel}");
        let out = git(host, root, &["show", "--no-textconv", &spec])?;
        if !out.ok() {
            return Err(failure_text(&out));
        }
        out.stdout
    } else {
        Vec::new()
    };
    let raw =
        String::from_utf8(raw_bytes).map_err(|_| crate::i18n::t("Staging changes needs a UTF-8 file (this one is stored in another encoding)").to_string())?;
    if has_lone_cr(&raw) || has_lone_cr(current) {
        return Err(crate::i18n::t("Staging changes is not supported in files with old-Mac (CR only) line endings").into());
    }
    let (bom, raw) = match raw.strip_prefix('\u{feff}') {
        Some(r) => (true, r.to_string()),
        None => (false, raw),
    };
    let index = raw.replace("\r\n", "\n");
    let chosen = hunks_touching(&hunks(&index, current), lo, hi);
    if chosen.is_empty() {
        return Ok(0);
    }
    // Line endings: unchanged lines stay as they are, new lines use the file's dominant style.
    let crlf = raw.matches("\r\n").count();
    let lf = raw.matches('\n').count() - crlf;
    let eol = if crlf > lf { "\r\n" } else { "\n" };
    let mut text = apply_hunks_raw(&raw, current, &chosen, eol);
    if bom {
        text.insert(0, '\u{feff}');
    }
    write_index(host, root, rel, &text)?;
    Ok(chosen.len())
}

/// Whether any line ends in a bare `\r` (old Mac); diff and rope line counting would disagree.
pub fn has_lone_cr(s: &str) -> bool {
    let b = s.as_bytes();
    b.iter().enumerate().any(|(i, &c)| c == b'\r' && b.get(i + 1) != Some(&b'\n'))
}

/// `apply_hunks(.., to_current = true)`: the raw index lines (including CRLF) are kept; the selected
/// blocks get `current`'s lines written with `eol`. Block ranges are relative to the LF-converted text.
pub fn apply_hunks_raw(raw: &str, current: &str, chosen: &[Hunk], eol: &str) -> String {
    let a: Vec<&str> = raw.split_inclusive('\n').collect();
    let b: Vec<&str> = current.split_inclusive('\n').collect();
    let mut spans: Vec<(Range<usize>, Range<usize>)> = chosen.iter().map(|h| (h.old.clone(), h.new.clone())).collect();
    spans.sort_by_key(|(r, _)| r.start);
    let mut out = String::new();
    let mut at = 0;
    for (r, with) in spans {
        out.extend(a[at.min(a.len())..r.start.min(a.len())].iter().copied());
        for line in &b[with.start.min(b.len())..with.end.min(b.len())] {
            match line.strip_suffix('\n') {
                Some(body) => {
                    out.push_str(body);
                    out.push_str(eol);
                }
                None => out.push_str(line),
            }
        }
        at = r.end;
    }
    out.extend(a[at.min(a.len())..].iter().copied());
    out
}

/// Side-by-side diff row: left (old) and right (new) line numbers, 0-based.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffRow {
    Same(usize, usize),
    /// Side by side in a changed block; one side may be missing.
    Changed(Option<usize>, Option<usize>),
}

/// Inline (single column) diff row: in a changed block, deletions first, then additions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InlineRow {
    Same(usize, usize),
    Removed(usize),
    Added(usize),
}

pub fn inline_rows(rows: &[DiffRow]) -> Vec<InlineRow> {
    let mut out = Vec::with_capacity(rows.len());
    let mut i = 0;
    while i < rows.len() {
        match rows[i] {
            DiffRow::Same(a, b) => {
                out.push(InlineRow::Same(a, b));
                i += 1;
            }
            DiffRow::Changed(..) => {
                let start = i;
                while i < rows.len() && matches!(rows[i], DiffRow::Changed(..)) {
                    i += 1;
                }
                out.extend(rows[start..i].iter().filter_map(|r| if let DiffRow::Changed(Some(a), _) = r { Some(InlineRow::Removed(*a)) } else { None }));
                out.extend(rows[start..i].iter().filter_map(|r| if let DiffRow::Changed(_, Some(b)) = r { Some(InlineRow::Added(*b)) } else { None }));
            }
        }
    }
    out
}

pub fn side_by_side(base: &str, current: &str) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    for (tag, old, new) in diff_ops(base, current) {
        match tag {
            similar::DiffTag::Equal => rows.extend(old.zip(new).map(|(a, b)| DiffRow::Same(a, b))),
            _ => {
                let n = old.len().max(new.len());
                for k in 0..n {
                    let a = (k < old.len()).then(|| old.start + k);
                    let b = (k < new.len()).then(|| new.start + k);
                    rows.push(DiffRow::Changed(a, b));
                }
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn parses_log_and_blame() {
        let out =
            "\x1eaaaa111\x1faaaa1\x1fAyşe\x1f2 days ago\x1fyeni ad\n\nsrc/yeni.rs\n\x1ebbbb222\x1fbbbb2\x1fAli\x1f3 weeks ago\x1filk | boru\n\nsrc/eski.rs\n";
        let log = parse_log(out);
        assert_eq!(log.len(), 2);
        assert_eq!((log[0].author.as_str(), log[0].path.as_str()), ("Ayşe", "src/yeni.rs"));
        assert_eq!(log[1].path, "src/eski.rs", "path before the rename");
        assert!(parse_log("").is_empty());
        let blame =
            "0123456789abcdef0123456789abcdef01234567 5 5 1\nauthor Ali Veli\nauthor-mail <a@b>\nauthor-time 0\nsummary düzeltme\nfilename x.rs\n\tsatır\n";
        let b = parse_blame(blame, "x.rs").unwrap();
        assert_eq!((b.short.as_str(), b.author.as_str(), b.subject.as_str()), ("0123456", "Ali Veli", "düzeltme"));
        assert_eq!(relative_time(30), "just now");
        assert_eq!(relative_time(3600), "1 hour ago");
        for (long, short) in [
            ("4 days ago", "4d"),
            ("25 minutes ago", "25m"),
            ("1 minute ago", "1m"),
            ("2 hours ago", "2h"),
            ("3 weeks ago", "3w"),
            ("5 months ago", "5mo"),
            ("2 years ago", "2y"),
            ("1 year, 2 months ago", "1y"),
            ("12 seconds ago", "12s"),
            ("just now", "now"),
            ("", ""),
            ("yesterday", "yesterday"),
            ("3 fortnights ago", "3 fortnights ago"),
        ] {
            assert_eq!(short_ago(long), short, "{long:?}");
        }
        assert_eq!(short_ago(&relative_time(86_400 * 4)), "4d");
        assert_eq!(relative_time(86_400 * 3), "3 days ago");
    }

    #[test]
    fn branch_header_variants() {
        assert_eq!(parse_status("/r", b"## No commits yet on main\0").branch.as_deref(), Some("main"));
        assert_eq!(parse_status("/r", b"## HEAD (no branch)\0").branch, None);
        let st = parse_status("/r", b"## feature/x\0");
        assert_eq!((st.branch.as_deref(), st.upstream), (Some("feature/x"), None));
    }

    #[test]
    fn line_marks_from_diff() {
        let base = "a\nb\nc\nd\n";
        assert!(line_marks(base, base).is_empty(), "same content: no marks, no diff");
        let m = line_marks(base, "a\nB\nc\nyeni\nd\n");
        assert_eq!(m[1], Some(LineMark::Modified));
        assert_eq!(m[3], Some(LineMark::Added));
        assert_eq!((m[0], m[2], m[4]), (None, None, None));
        let m = line_marks(base, "a\nd\n");
        assert_eq!(m[1], Some(LineMark::DeletedAbove), "b and c deleted, above d");
    }

    #[test]
    fn side_by_side_aligns_rows() {
        let rows = side_by_side("a\nb\nc\n", "a\nB\nx\nc\n");
        assert_eq!(rows[0], DiffRow::Same(0, 0));
        assert_eq!(rows[1], DiffRow::Changed(Some(1), Some(1)));
        assert_eq!(rows[2], DiffRow::Changed(None, Some(2)));
        assert_eq!(rows[3], DiffRow::Same(2, 3));
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;

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

    fn nums(n: usize, change: &[(usize, &str)]) -> String {
        (1..=n).map(|i| format!("{}\n", change.iter().find(|(k, _)| *k == i).map_or(format!("l{i}"), |(_, t)| t.to_string()))).collect()
    }

    #[test]
    fn hunks_pick_stage_and_revert_by_line() {
        let base = nums(12, &[]);
        // Line 3 changed, a line was added after line 8, line 11 was deleted.
        let mut cur: Vec<String> = base.split_inclusive('\n').map(str::to_string).collect();
        cur[2] = "L3\n".into();
        cur.insert(8, "yeni\n".into());
        cur.remove(11);
        let cur = cur.concat();
        let all = hunks(&base, &cur);
        assert_eq!(all.len(), 3);
        // Only the block at line 3 (0-based 2).
        let first = hunks_touching(&all, 2, 2);
        assert_eq!(first.len(), 1);
        let staged = apply_hunks(&base, &cur, &first, true);
        assert_eq!(staged, nums(12, &[(3, "L3")]));
        // All three blocks: index = working text.
        assert_eq!(apply_hunks(&base, &cur, &all, true), cur);
        // Revert: only the added line goes away.
        let added = hunks_touching(&all, 8, 8);
        assert_eq!(added.len(), 1);
        let reverted = apply_hunks(&base, &cur, &added, false);
        assert!(!reverted.contains("yeni") && reverted.contains("L3"));
        // Reverting everything returns to HEAD; the deleted line comes back.
        assert_eq!(apply_hunks(&base, &cur, &all, false), base);
        // Empty selection: leave alone.
        assert!(hunks_touching(&all, 5, 6).is_empty());
        assert_eq!(apply_hunks(&base, &cur, &[], true), base);
    }

    #[test]
    fn hunks_handle_missing_final_newline_and_empty_base() {
        assert_eq!(apply_hunks("", "a\nb", &hunks("", "a\nb"), true), "a\nb");
        assert_eq!(apply_hunks("a\nb", "a\nb\nc", &hunks("a\nb", "a\nb\nc"), true), "a\nb\nc");
        assert_eq!(apply_hunks("a\nb", "a\nb\nc", &hunks("a\nb", "a\nb\nc"), false), "a\nb");
    }

    #[test]
    fn stage_hunks_writes_only_the_chosen_block_to_the_index() {
        let dir = std::env::temp_dir().join(format!("codeyard-hunk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| crate::test_git::git().args(args).current_dir(&dir).output().ok().filter(|o| o.status.success());
        if run(&["init", "-q"]).is_none() {
            return;
        }
        let base = nums(10, &[]);
        std::fs::write(dir.join("f.txt"), &base).unwrap();
        run(&["add", "-A"]).unwrap();
        run(&["commit", "-q", "-m", "x"]).unwrap();
        let cur = nums(10, &[(2, "B"), (9, "I")]);
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        assert_eq!(stage_hunks(&host, &root, "f.txt", &cur, 1, 1), Ok(1));
        let staged = run(&["show", ":f.txt"]).unwrap().stdout;
        assert_eq!(String::from_utf8(staged).unwrap(), nums(10, &[(2, "B")]));
        // Second block afterwards: the index is now the full working text.
        assert_eq!(stage_hunks(&host, &root, "f.txt", &cur, 8, 8), Ok(1));
        assert_eq!(String::from_utf8(run(&["show", ":f.txt"]).unwrap().stdout).unwrap(), cur);
        // No changes left.
        assert_eq!(stage_hunks(&host, &root, "f.txt", &cur, 0, 9), Ok(0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn repo(name: &str) -> Option<(std::path::PathBuf, impl Fn(&[&str]) -> Option<String>)> {
        let dir = std::env::temp_dir().join(format!("codeyard-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.clone();
        let run = move |args: &[&str]| {
            crate::test_git::git()
                .args(args)
                .current_dir(&d)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        };
        run(&["init", "-q"])?;
        std::fs::write(dir.join("f.txt"), "base\n").unwrap();
        run(&["add", "-A"])?;
        run(&["commit", "-q", "-m", "base"])?;
        Some((dir, run))
    }

    /// A hostile server can name a branch `--exec=<cmd>`; `git rebase <name>` would run it. Such names are neither offered nor passed on.
    #[cfg(unix)]
    #[test]
    fn a_branch_named_like_an_option_never_reaches_git_as_one() {
        let Some((dir, run)) = repo("dashbranch") else { return };
        let marker = std::env::temp_dir().join(format!("codeyard-dashbranch-marker-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        // Ref names cannot hold a space, so the shell gets `${IFS}`.
        let evil = format!("--exec=touch${{IFS}}{}", marker.display());
        run(&["update-ref", &format!("refs/heads/{evil}"), "HEAD"]).expect("git accepts the ref name");
        run(&["checkout", "-q", "-b", "mine"]).unwrap();
        let host = host::local();
        let root = dir.to_str().unwrap();
        let (local, _) = branches(&host, root).unwrap();
        assert!(local.iter().any(|b| b == "mine"), "{local:?}");
        assert!(!local.iter().any(|b| b.starts_with('-')), "{local:?}");
        assert!(rebase(&host, root, &evil).is_err());
        assert!(merge(&host, root, &evil).is_err());
        assert!(checkout(&host, root, &evil, false, false).is_err());
        assert!(!marker.exists(), "the command in the branch name ran");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `diff.<driver>.textconv` from the repository's config runs a program and replaces file content in `show` and `blame`;
    /// the editor needs the raw bytes (hunk staging writes them back to the index), so it is never applied.
    #[cfg(unix)]
    #[test]
    fn show_and_blame_ignore_textconv_drivers() {
        use std::os::unix::fs::PermissionsExt;
        let Some((dir, run)) = repo("textconv") else { return };
        let marker = dir.join("..").join(format!("codeyard-textconv-marker-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let conv = dir.join("conv.sh");
        std::fs::write(&conv, format!("#!/bin/sh\ntouch '{}'\necho CONVERTED\n", marker.display())).unwrap();
        std::fs::set_permissions(&conv, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(dir.join(".gitattributes"), "*.txt diff=evil\n").unwrap();
        if run(&["config", "diff.evil.textconv", &conv.to_string_lossy()]).is_none() {
            return;
        }
        let host = host::local();
        let d = dir.to_string_lossy().into_owned();
        assert_eq!(show(&host, &d, "HEAD", "f.txt").as_deref(), Some("base\n"), "raw content, not the converter's output");
        let entry = blame_line(&host, &d, "f.txt", 0, None).unwrap();
        assert!(!entry.hash.is_empty());
        assert!(!marker.exists(), "a textconv program from the repository was executed");
        let _ = std::fs::remove_file(&marker);
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
    fn merge_delete_rename_and_abort() {
        let Some((dir, run)) = repo("branchops") else { return };
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        // feature: changes f.txt; main: a separate file (merges cleanly).
        run(&["checkout", "-q", "-b", "feature"]).unwrap();
        std::fs::write(dir.join("g.txt"), "g\n").unwrap();
        run(&["add", "-A"]).unwrap();
        run(&["commit", "-q", "-m", "g"]).unwrap();
        run(&["checkout", "-q", "main"]).unwrap();
        assert_eq!(merge(&host, &root, "feature"), Ok(false));
        assert!(dir.join("g.txt").exists());
        // A merged branch is deleted with -d; the current branch cannot be deleted.
        assert!(delete_branch(&host, &root, "main", false).is_err());
        assert_eq!(delete_branch(&host, &root, "feature", false), Ok(()));
        // Unmerged branch: -d refuses, -D deletes.
        run(&["checkout", "-q", "-b", "wip"]).unwrap();
        std::fs::write(dir.join("w.txt"), "w\n").unwrap();
        run(&["add", "-A"]).unwrap();
        run(&["commit", "-q", "-m", "w"]).unwrap();
        run(&["checkout", "-q", "main"]).unwrap();
        assert!(delete_branch(&host, &root, "wip", false).unwrap_err().contains("not fully merged"));
        assert_eq!(delete_branch(&host, &root, "wip", true), Ok(()));
        // Rename.
        run(&["branch", "old"]).unwrap();
        assert_eq!(rename_branch(&host, &root, "old", "new"), Ok(()));
        assert!(branches(&host, &root).unwrap().0.contains(&"new".to_string()));
        // Conflict: both change f.txt differently → Ok(true), abort cleans up.
        run(&["checkout", "-q", "-b", "c1"]).unwrap();
        std::fs::write(dir.join("f.txt"), "one\n").unwrap();
        run(&["commit", "-qam", "one"]).unwrap();
        run(&["checkout", "-q", "main"]).unwrap();
        std::fs::write(dir.join("f.txt"), "two\n").unwrap();
        run(&["commit", "-qam", "two"]).unwrap();
        assert_eq!(merge(&host, &root, "c1"), Ok(true));
        assert!(merge_in_progress(&host, &root));
        assert_eq!(merge_abort(&host, &root), Ok(()));
        assert!(!merge_in_progress(&host, &root));
        assert_eq!(std::fs::read_to_string(dir.join("f.txt")).unwrap(), "two\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remotes_add_remove_publish() {
        let Some((dir, run)) = repo("remotes") else { return };
        let bare = std::env::temp_dir().join(format!("codeyard-bare-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&bare);
        crate::test_git::git().args(["init", "-q", "--bare"]).arg(&bare).output().unwrap();
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        assert!(publish_branch(&host, &root).unwrap_err().contains("no remote"));
        assert_eq!(add_remote(&host, &root, "origin", &bare.to_string_lossy()), Ok(()));
        assert_eq!(remotes(&host, &root).unwrap().len(), 1);
        assert_eq!(publish_branch(&host, &root), Ok("origin".to_string()));
        assert!(run(&["rev-parse", "--abbrev-ref", "main@{upstream}"]).is_some(), "tracking was set up");
        assert_eq!(remove_remote(&host, &root, "origin"), Ok(()));
        assert!(remotes(&host, &root).unwrap().is_empty());
        assert_eq!(parse_remotes("o\tgit@x:a.git (fetch)\no\tgit@x:a.git (push)\nu\thttps://h/u (fetch)\n").len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&bare);
    }

    #[test]
    fn inline_rows_put_removed_before_added_per_block() {
        let rows = side_by_side("a\nb\nc\nd\n", "a\nB\nB2\nd\n");
        assert_eq!(
            inline_rows(&rows),
            vec![InlineRow::Same(0, 0), InlineRow::Removed(1), InlineRow::Removed(2), InlineRow::Added(1), InlineRow::Added(2), InlineRow::Same(3, 3)]
        );
        assert!(inline_rows(&side_by_side("x\n", "x\n")).iter().all(|r| matches!(r, InlineRow::Same(..))));
        assert_eq!(inline_rows(&side_by_side("", "n\n")), vec![InlineRow::Added(0)]);
    }

    #[test]
    fn intraline_marks_only_the_changed_characters() {
        let (o, n) = intraline("let total = a + b;", "let total = a - b;");
        assert_eq!((o, n), (vec![14..15], vec![14..15]));
        // Addition: mark only on the new line.
        let (o, n) = intraline("foo(a)", "foo(a, b)");
        assert!(o.is_empty(), "{o:?}");
        assert_eq!(n, vec![5..8]);
        // Very different lines: no mark (the whole line is already painted).
        assert_eq!(intraline("alpha beta gamma", "0123456789 xyz!!"), Default::default());
        // A very long line is skipped; with multibyte characters the range stays on a character boundary.
        assert_eq!(intraline(&"a".repeat(3000), &"b".repeat(3000)), Default::default());
        let (_, n) = intraline("ğüş x", "ğüş y");
        assert_eq!(n, vec![7..8]);
    }

    /// Cherry-picking or reverting a *merge* commit needs `-m 1` (mainline = first parent); without it git refuses.
    #[test]
    fn cherry_pick_and_revert_of_a_merge_commit_use_the_first_parent() {
        use crate::graph::{CommitOp, run_op};
        let Some((dir, run)) = repo("mergepick") else { return };
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        // main --- m (merge) ; side adds s.txt
        run(&["checkout", "-q", "-b", "side"]).unwrap();
        std::fs::write(dir.join("s.txt"), "s\n").unwrap();
        run(&["add", "-A"]).unwrap();
        run(&["commit", "-q", "-m", "s"]).unwrap();
        run(&["checkout", "-q", "main"]).unwrap();
        std::fs::write(dir.join("m.txt"), "m\n").unwrap();
        run(&["add", "-A"]).unwrap();
        run(&["commit", "-q", "-m", "m"]).unwrap();
        run(&["merge", "-q", "--no-ff", "-m", "merge side", "side"]).unwrap();
        let merge = run(&["rev-parse", "HEAD"]).unwrap().trim().to_string();
        // Revert of the merge removes s.txt (the side's changes), keeps m.txt.
        let msg = run_op(&host, &root, &merge, &CommitOp::Revert).expect("revert of a merge commit");
        assert!(msg.starts_with("Reverted"), "{msg}");
        assert!(!dir.join("s.txt").exists() && dir.join("m.txt").exists());
        // Cherry-pick of the merge onto a branch from the base brings in the side's changes.
        let base = run(&["rev-list", "--max-parents=0", "HEAD"]).unwrap().trim().to_string();
        run(&["checkout", "-q", "-b", "target", &base]).unwrap();
        let msg = run_op(&host, &root, &merge, &CommitOp::CherryPick).expect("cherry-pick of a merge commit");
        assert!(msg.starts_with("Cherry-picked"), "{msg}");
        assert!(dir.join("s.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rebase_and_worktrees() {
        let Some((dir, run)) = repo("rebasewt") else { return };
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        // Clean rebase: feature lands on top of main's advanced state.
        run(&["checkout", "-q", "-b", "feat"]).unwrap();
        std::fs::write(dir.join("a.txt"), "a\n").unwrap();
        run(&["add", "-A"]).unwrap();
        run(&["commit", "-q", "-m", "a"]).unwrap();
        run(&["checkout", "-q", "main"]).unwrap();
        std::fs::write(dir.join("b.txt"), "b\n").unwrap();
        run(&["add", "-A"]).unwrap();
        run(&["commit", "-q", "-m", "b"]).unwrap();
        run(&["checkout", "-q", "feat"]).unwrap();
        assert_eq!(rebase(&host, &root, "main"), Ok(false));
        assert!(dir.join("b.txt").exists() && dir.join("a.txt").exists());
        assert!(!rebase_in_progress(&host, &root));
        // Conflicting rebase: Ok(true) → abort puts everything back.
        run(&["checkout", "-q", "-b", "c1", "main"]).unwrap();
        std::fs::write(dir.join("f.txt"), "one\n").unwrap();
        run(&["commit", "-qam", "one"]).unwrap();
        run(&["checkout", "-q", "main"]).unwrap();
        std::fs::write(dir.join("f.txt"), "two\n").unwrap();
        run(&["commit", "-qam", "two"]).unwrap();
        run(&["checkout", "-q", "c1"]).unwrap();
        assert_eq!(rebase(&host, &root, "main"), Ok(true));
        assert!(rebase_in_progress(&host, &root));
        assert_eq!(rebase_abort(&host, &root), Ok(()));
        assert!(!rebase_in_progress(&host, &root));
        assert_eq!(std::fs::read_to_string(dir.join("f.txt")).unwrap(), "one\n");
        // Worktree: a new branch and an existing branch.
        let wt = std::env::temp_dir().join(format!("codeyard-wt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&wt);
        assert_eq!(add_worktree(&host, &root, &wt.to_string_lossy(), "wt-new"), Ok(()));
        let list = worktrees(&host, &root).unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.iter().any(|(_, b)| b.as_deref() == Some("wt-new")));
        assert!(wt.join("f.txt").exists() || wt.exists());
        let _ = crate::test_git::git().args(["worktree", "remove", "--force"]).arg(&wt).current_dir(&dir).output();
        assert_eq!(
            parse_worktrees("worktree /a\nHEAD x\nbranch refs/heads/main\n\nworktree /b\nHEAD y\ndetached\n"),
            vec![("/a".to_string(), Some("main".to_string())), ("/b".to_string(), None)]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stage_hunks_keeps_bytes_and_refuses_unsafe_files() {
        let Some((dir, run)) = repo("stagesafe") else { return };
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        let commit = |name: &str, bytes: &[u8]| {
            std::fs::write(dir.join(name), bytes).unwrap();
            run(&["add", name]).unwrap();
            run(&["commit", "-q", "-m", name]).unwrap();
        };
        let index = |name: &str| {
            let o = crate::test_git::git().args(["show", &format!(":{name}")]).current_dir(&dir).output().unwrap();
            o.stdout
        };
        // Mixed line endings: unchanged line stays as is, the new line uses the dominant (CRLF) style.
        commit("mixed.txt", b"l1\r\nl2\r\nl3\n");
        assert_eq!(stage_hunks(&host, &root, "mixed.txt", "l1\nL2\nl3\n", 1, 1), Ok(1));
        assert_eq!(index("mixed.txt"), b"l1\r\nL2\r\nl3\n");
        // The BOM is preserved; the working text has no BOM (the editor strips the BOM).
        commit("bom.txt", b"\xef\xbb\xbfa\nb\nc\n");
        assert_eq!(stage_hunks(&host, &root, "bom.txt", "a\nb\nC\n", 2, 2), Ok(1));
        assert_eq!(index("bom.txt"), b"\xef\xbb\xbfa\nb\nC\n");
        // Non-UTF-8 file: rejected, index unchanged.
        commit("latin.txt", b"caf\xe9\nx\n");
        let err = stage_hunks(&host, &root, "latin.txt", "café\ny\n", 1, 1).unwrap_err();
        assert!(err.contains("UTF-8"), "{err}");
        assert_eq!(index("latin.txt"), b"caf\xe9\nx\n");
        // CR-only line endings: rejected.
        commit("cr.txt", b"a\rb\nl2\nl3\n");
        assert!(stage_hunks(&host, &root, "cr.txt", "a\rb\nl2\nX\nl3\n", 2, 2).unwrap_err().contains("CR"));
        // Symlink: rejected.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("mixed.txt", dir.join("link")).unwrap();
            run(&["add", "link"]).unwrap();
            run(&["commit", "-q", "-m", "link"]).unwrap();
            assert!(stage_hunks(&host, &root, "link", "x\n", 0, 0).unwrap_err().contains("symlink"));
        }
        // Conflicted file: not written to stage 0 as "resolved".
        run(&["checkout", "-q", "-b", "c1"]).unwrap();
        std::fs::write(dir.join("f.txt"), "one\n").unwrap();
        run(&["commit", "-qam", "one"]).unwrap();
        run(&["checkout", "-q", "main"]).unwrap();
        std::fs::write(dir.join("f.txt"), "two\n").unwrap();
        run(&["commit", "-qam", "two"]).unwrap();
        assert_eq!(merge(&host, &root, "c1"), Ok(true));
        let err = stage_hunks(&host, &root, "f.txt", "<<<<<<< HEAD\ntwo\n=======\none\n>>>>>>> c1\n", 0, 5).unwrap_err();
        assert!(err.contains("conflict"), "{err}");
        assert!(has_unmerged(&host, &root), "conflict is still unresolved");
        merge_abort(&host, &root).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn error_texts_and_unmerged_count_report_the_real_cause() {
        let Some((dir, run)) = repo("errtext") else { return };
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        // Dirty tree + merge: the error line must be the real cause, not "Updating …".
        run(&["checkout", "-q", "-b", "other"]).unwrap();
        std::fs::write(dir.join("f.txt"), "other\n").unwrap();
        run(&["commit", "-qam", "other"]).unwrap();
        run(&["checkout", "-q", "main"]).unwrap();
        std::fs::write(dir.join("f.txt"), "dirty\n").unwrap();
        let err = merge(&host, &root, "other").unwrap_err();
        assert!(!err.starts_with("Updating"), "{err}");
        assert!(!merge_in_progress(&host, &root));
        std::fs::write(dir.join("f.txt"), "base\n").unwrap();
        // Count: a number on a normal branch, an error on an unborn HEAD.
        assert_eq!(unmerged_commits(&host, &root, "other"), Ok(1));
        run(&["checkout", "-q", "--orphan", "orph"]).unwrap();
        assert!(unmerged_commits(&host, &root, "other").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rebase_refuses_when_one_is_running_and_reports_non_conflict_failures() {
        let Some((dir, run)) = repo("rebaseerr") else { return };
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        run(&["checkout", "-q", "-b", "c1"]).unwrap();
        std::fs::write(dir.join("f.txt"), "one\n").unwrap();
        run(&["commit", "-qam", "one"]).unwrap();
        run(&["checkout", "-q", "main"]).unwrap();
        std::fs::write(dir.join("f.txt"), "two\n").unwrap();
        run(&["commit", "-qam", "two"]).unwrap();
        run(&["checkout", "-q", "c1"]).unwrap();
        assert_eq!(rebase(&host, &root, "main"), Ok(true));
        assert!(rebase(&host, &root, "main").unwrap_err().contains("already in progress"));
        // Without resolving, continue: the conflict persists (Ok(true)); resolving and adding finishes it.
        assert_eq!(rebase_continue(&host, &root), Ok(true));
        std::fs::write(dir.join("f.txt"), "resolved\n").unwrap();
        run(&["add", "f.txt"]).unwrap();
        assert_eq!(rebase_continue(&host, &root), Ok(false));
        assert!(!rebase_in_progress(&host, &root));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
