//! Git history graph: places `git log --all` output into lanes.
//! Only parsing and layout live here (drawing is in `scm.rs`); testable.

use host::Host;

/// Name label attached to a commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefLabel {
    pub name: String,
    pub kind: RefKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    /// The checked-out branch (HEAD).
    Head,
    Local,
    Remote,
    Tag,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GraphCommit {
    pub hash: String,
    pub short: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    /// "3 days ago"
    pub when: String,
    /// "2026-10-23 21:46"
    pub date: String,
    pub subject: String,
    pub refs: Vec<RefLabel>,
}

/// Whose history is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    /// All local and remote branches.
    #[default]
    All,
    /// Only the checked-out branch's (HEAD) history.
    Current,
}

/// Maximum lane count of the graph: more are squeezed into the last lane (so the left gutter doesn't keep growing).
pub const MAX_COLS: usize = 7;

/// Squeezes lanes into `max` columns: the overflow is drawn on top of each other in the last column.
pub fn cap_lanes(rows: &mut [GraphRow], max: usize) {
    let max = max.max(1);
    let fix = |c: &mut usize| *c = (*c).min(max - 1);
    for r in rows {
        fix(&mut r.col);
        for l in r.pass.iter_mut().chain(r.merge_in.iter_mut()).chain(r.fork_out.iter_mut()) {
            fix(&mut l.0);
        }
        r.width = r.width.min(max);
    }
}

/// A lane to draw: (column, color index).
pub type Lane = (usize, usize);

/// Drawing info for one row. Upper half: `above` and `merge_in`; lower half: `below` and `fork_out`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GraphRow {
    pub col: usize,
    pub color: usize,
    /// Lanes passing straight through the row.
    pub pass: Vec<Lane>,
    /// Line coming into the node from above (its color).
    pub above: Option<usize>,
    /// Other lanes waiting for the same commit: a curve from above into the node.
    pub merge_in: Vec<Lane>,
    /// Line continuing down from the node (first parent).
    pub below: Option<usize>,
    /// Curves from the node to the lanes of the extra parents (merge commit).
    pub fork_out: Vec<Lane>,
    /// Widest lane count in this row.
    pub width: usize,
}

const COLORS: usize = 8;

/// Drawing row for each commit. Commits must be ordered children-first (`git log --date-order`).
pub fn layout(commits: &[GraphCommit]) -> Vec<GraphRow> {
    // Lane: summary and color of the commit it's waiting for.
    let mut lanes: Vec<Option<(String, usize)>> = Vec::new();
    let mut next_color = 0usize;
    let take_color = |next: &mut usize| {
        let c = *next % COLORS;
        *next += 1;
        c
    };
    let mut rows = Vec::with_capacity(commits.len());
    for c in commits {
        let matching: Vec<usize> = lanes.iter().enumerate().filter(|(_, l)| l.as_ref().is_some_and(|(h, _)| *h == c.hash)).map(|(i, _)| i).collect();
        let width_before = lanes.len();
        let (col, color, above) = match matching.first() {
            Some(&i) => {
                let color = lanes[i].as_ref().map_or(0, |l| l.1);
                (i, color, Some(color))
            }
            None => {
                let i = lanes.iter().position(Option::is_none).unwrap_or(lanes.len());
                if i == lanes.len() {
                    lanes.push(None);
                }
                (i, take_color(&mut next_color), None)
            }
        };
        let merge_in: Vec<Lane> = matching.iter().skip(1).map(|&i| (i, lanes[i].as_ref().map_or(0, |l| l.1))).collect();
        let pass: Vec<Lane> = lanes
            .iter()
            .enumerate()
            .filter(|(i, l)| l.is_some() && *i != col && !matching.contains(i))
            .map(|(i, l)| (i, l.as_ref().map_or(0, |l| l.1)))
            .collect();
        for &i in matching.iter().skip(1) {
            lanes[i] = None;
        }
        let mut below = None;
        let mut fork_out = Vec::new();
        match c.parents.split_first() {
            None => lanes[col] = None,
            Some((first, rest)) => {
                lanes[col] = Some((first.clone(), color));
                below = Some(color);
                for p in rest {
                    if let Some(j) = lanes.iter().position(|l| l.as_ref().is_some_and(|(h, _)| h == p)) {
                        fork_out.push((j, lanes[j].as_ref().map_or(0, |l| l.1)));
                    } else {
                        let j = lanes.iter().position(Option::is_none).unwrap_or(lanes.len());
                        if j == lanes.len() {
                            lanes.push(None);
                        }
                        let k = take_color(&mut next_color);
                        lanes[j] = Some((p.clone(), k));
                        fork_out.push((j, k));
                    }
                }
            }
        }
        let width = width_before.max(lanes.len()).max(col + 1);
        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }
        rows.push(GraphRow { col, color, pass, above, merge_in, below, fork_out, width });
    }
    rows
}

/// `%D` output: "HEAD -> main, origin/main, tag: v1.0".
pub fn parse_refs(decor: &str) -> Vec<RefLabel> {
    let mut out = Vec::new();
    for part in decor.split(", ").map(str::trim).filter(|p| !p.is_empty()) {
        if let Some(rest) = part.strip_prefix("HEAD -> ") {
            out.push(RefLabel { name: rest.to_string(), kind: RefKind::Head });
        } else if part == "HEAD" {
            out.push(RefLabel { name: "HEAD".into(), kind: RefKind::Head });
        } else if let Some(t) = part.strip_prefix("tag: ") {
            out.push(RefLabel { name: t.to_string(), kind: RefKind::Tag });
        } else if part.contains('/') && !part.starts_with("refs/") {
            out.push(RefLabel { name: part.to_string(), kind: RefKind::Remote });
        } else {
            out.push(RefLabel { name: part.to_string(), kind: RefKind::Local });
        }
    }
    out
}

const FORMAT: &str = "--format=%x1e%H%x1f%h%x1f%P%x1f%an%x1f%ae%x1f%ar%x1f%ad%x1f%D%x1f%s";

pub fn parse_graph_log(out: &str) -> Vec<GraphCommit> {
    out.split('\x1e')
        .filter_map(|rec| {
            let f: Vec<&str> = rec.trim_end_matches('\n').split('\x1f').collect();
            let [hash, short, parents, author, email, when, date, decor, subject] = f[..] else { return None };
            if hash.is_empty() {
                return None;
            }
            Some(GraphCommit {
                hash: hash.into(),
                short: short.into(),
                parents: parents.split_whitespace().map(str::to_string).collect(),
                author: author.into(),
                email: email.into(),
                when: when.into(),
                date: date.into(),
                subject: subject.into(),
                refs: parse_refs(decor),
            })
        })
        .collect()
}

/// History of all branches (newest first, children first). Empty list for an empty repository.
pub fn log(host: &dyn Host, root: &str, limit: usize, scope: Scope) -> Result<Vec<GraphCommit>, String> {
    let n = format!("-n{limit}");
    let mut args = vec!["log", "--date-order", "--decorate=short", "--date=format:%Y-%m-%d %H:%M", &n, FORMAT];
    if scope == Scope::All {
        args.insert(1, "--all");
    }
    let out = host.exec("git", &args, Some(root), None).map_err(|e| e.to_string())?;
    if !out.ok() {
        let err = out.stderr_str();
        // No commits at all: not an error, an empty graph.
        if err.contains("does not have any commits") || err.contains("bad default revision") || out.code == Some(128) && err.contains("HEAD") {
            return Ok(Vec::new());
        }
        return Err(if err.is_empty() { "git log failed".into() } else { err });
    }
    Ok(parse_graph_log(&out.stdout_str()))
}

/// Files a commit changed: (status letter, path, old path).
pub fn changed_files(host: &dyn Host, root: &str, hash: &str) -> Result<Vec<(char, String, Option<String>)>, String> {
    let out = host.exec("git", &["show", "--name-status", "--format=", "-M", "--first-parent", hash], Some(root), None).map_err(|e| e.to_string())?;
    if !out.ok() {
        return Err(out.stderr_str());
    }
    Ok(out
        .stdout_str()
        .lines()
        .filter_map(|l| {
            let mut it = l.split('\t');
            let status = it.next()?.chars().next()?;
            let a = it.next()?.to_string();
            match it.next() {
                Some(b) => Some((status, b.to_string(), Some(a))),
                None => Some((status, a, None)),
            }
        })
        .collect())
}

/// A file change: status letter, path, old path (on rename), added/deleted lines.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileChange {
    pub status: char,
    pub path: String,
    pub old: Option<String>,
    /// `None` for a binary file.
    pub added: Option<usize>,
    pub deleted: Option<usize>,
}

/// Details of the selected commit (the section under the graph).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommitDetail {
    pub hash: String,
    pub author: String,
    pub email: String,
    pub date: String,
    pub parents: Vec<String>,
    /// Full message including the subject line.
    pub message: String,
    pub files: Vec<FileChange>,
}

impl CommitDetail {
    pub fn totals(&self) -> (usize, usize) {
        self.files.iter().fold((0, 0), |(a, d), f| (a + f.added.unwrap_or(0), d + f.deleted.unwrap_or(0)))
    }
}

/// Output of `git show -s --format=%an␟%ae␟%ad␟%P␟%B`.
pub fn parse_detail_head(out: &str, hash: &str) -> CommitDetail {
    let mut f = out.splitn(5, '\x1f');
    let (author, email, date, parents) = (f.next().unwrap_or(""), f.next().unwrap_or(""), f.next().unwrap_or(""), f.next().unwrap_or(""));
    CommitDetail {
        hash: hash.into(),
        author: author.trim().into(),
        email: email.into(),
        date: date.into(),
        parents: parents.split_whitespace().map(str::to_string).collect(),
        message: f.next().unwrap_or("").trim().to_string(),
        files: Vec::new(),
    }
}

/// Output of `git show --raw --numstat --format=`: first the status lines starting with ":", then
/// the number lines in the same order.
pub fn parse_changes(out: &str) -> Vec<FileChange> {
    let mut files: Vec<FileChange> = Vec::new();
    let mut counts: Vec<(Option<usize>, Option<usize>)> = Vec::new();
    for l in out.lines() {
        if let Some(rest) = l.strip_prefix(':') {
            let mut t = rest.split('\t');
            let Some(status) = t.next().and_then(|m| m.split_whitespace().last()).and_then(|s| s.chars().next()) else { continue };
            let Some(a) = t.next() else { continue };
            files.push(match t.next() {
                Some(b) => FileChange { status, path: b.into(), old: Some(a.into()), ..Default::default() },
                None => FileChange { status, path: a.into(), ..Default::default() },
            });
        } else {
            let mut t = l.splitn(3, '\t');
            if let (Some(a), Some(d), Some(_)) = (t.next(), t.next(), t.next()) {
                counts.push((a.parse().ok(), d.parse().ok()));
            }
        }
    }
    for (f, (a, d)) in files.iter_mut().zip(counts) {
        f.added = a;
        f.deleted = d;
    }
    files
}

pub fn commit_detail(host: &dyn Host, root: &str, hash: &str) -> Result<CommitDetail, String> {
    let head = crate::git::git_ok(host, root, &["show", "-s", "--date=format:%Y-%m-%d %H:%M", "--format=%an%x1f%ae%x1f%ad%x1f%P%x1f%B", hash])?;
    let mut d = parse_detail_head(&head, hash);
    d.files = parse_changes(&crate::git::git_ok(host, root, &["show", "--raw", "--numstat", "--format=", "-M", "--first-parent", hash])?);
    Ok(d)
}

/// Commit operations run from the graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommitOp {
    /// Check out the commit (detached HEAD).
    Checkout,
    /// Create a new branch and switch to it.
    Branch(String),
    Tag(String),
    CherryPick,
    Revert,
    Reset(ResetMode),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
}

impl ResetMode {
    pub fn flag(self) -> &'static str {
        match self {
            ResetMode::Soft => "--soft",
            ResetMode::Mixed => "--mixed",
            ResetMode::Hard => "--hard",
        }
    }
}

/// Runs the commit operation; a conflicting cherry-pick/revert is rolled back (no half-done state is left behind).
pub fn run_op(host: &dyn Host, root: &str, hash: &str, op: &CommitOp) -> Result<String, String> {
    let git = |args: &[&str]| crate::git::git_ok(host, root, args);
    let short = &hash[..hash.len().min(7)];
    match op {
        CommitOp::Checkout => git(&["checkout", "--detach", hash]).map(|_| crate::i18n::tf("Checked out {} (detached HEAD)", &[&short])),
        CommitOp::Branch(name) => crate::git::plain_name(name)
            .and_then(|_| git(&["checkout", "-b", name, hash]).map(|_| crate::i18n::tf("Created branch {0} at {1}", &[&name, &short]))),
        CommitOp::Tag(name) => {
            crate::git::plain_name(name).and_then(|_| git(&["tag", name, hash]).map(|_| crate::i18n::tf("Tagged {0} as {1}", &[&short, &name])))
        }
        CommitOp::CherryPick => {
            let mut args = vec!["cherry-pick"];
            args.extend(mainline_args(host, root, hash));
            args.push(hash);
            git(&args).map(|_| crate::i18n::tf("Cherry-picked {}", &[&short])).map_err(|e| {
                let _ = git(&["cherry-pick", "--abort"]);
                crate::i18n::tf("Cherry-pick stopped and was undone: {}", &[&e.lines().next().unwrap_or("conflict")])
            })
        }
        CommitOp::Revert => {
            let mut args = vec!["revert", "--no-edit"];
            args.extend(mainline_args(host, root, hash));
            args.push(hash);
            git(&args).map(|_| crate::i18n::tf("Reverted {}", &[&short])).map_err(|e| {
                let _ = git(&["revert", "--abort"]);
                crate::i18n::tf("Revert stopped and was undone: {}", &[&e.lines().next().unwrap_or("conflict")])
            })
        }
        CommitOp::Reset(mode) => git(&["reset", mode.flag(), hash]).map(|_| crate::i18n::tf("Reset to {0} ({1})", &[&short, &&mode.flag()[2..]])),
    }
}

/// A merge commit can only be cherry-picked or reverted relative to one parent; the first parent (the branch that was merged
/// into) is the usual mainline. Ordinary commits need no flag.
fn mainline_args(host: &dyn Host, root: &str, hash: &str) -> Vec<&'static str> {
    let parents = crate::git::git_ok(host, root, &["rev-list", "--parents", "-n", "1", hash]).map(|o| o.split_whitespace().count().saturating_sub(1));
    if parents.is_ok_and(|n| n > 1) { vec!["-m", "1"] } else { Vec::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(hash: &str, parents: &[&str]) -> GraphCommit {
        GraphCommit {
            hash: hash.into(),
            short: hash.into(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            subject: hash.into(),
            ..Default::default()
        }
    }

    #[test]
    fn linear_history_stays_in_one_lane() {
        let rows = layout(&[c("c", &["b"]), c("b", &["a"]), c("a", &[])]);
        assert!(rows.iter().all(|r| r.col == 0 && r.width == 1));
        assert_eq!((rows[0].above, rows[0].below), (None, Some(0)));
        assert_eq!((rows[1].above, rows[1].below), (Some(0), Some(0)));
        assert_eq!((rows[2].above, rows[2].below), (Some(0), None));
    }

    #[test]
    fn merge_opens_a_lane_and_the_branch_point_closes_it() {
        // m merges b (first parent a-side) and f (feature); both descend from a.
        //   m
        //   |\
        //   b f
        //   |/
        //   a
        let rows = layout(&[c("m", &["b", "f"]), c("b", &["a"]), c("f", &["a"]), c("a", &[])]);
        assert_eq!(rows[0].fork_out.len(), 1, "merge forks to a second lane");
        assert_eq!(rows[0].fork_out[0].0, 1);
        assert_eq!(rows[1].col, 0);
        assert_eq!(rows[1].pass.iter().map(|l| l.0).collect::<Vec<_>>(), [1], "feature lane passes behind b");
        assert_eq!(rows[2].col, 1);
        assert_eq!(rows[2].pass.iter().map(|l| l.0).collect::<Vec<_>>(), [0]);
        // a is awaited by both lanes: it sits in lane 0 and lane 1 curves into it.
        assert_eq!((rows[3].col, rows[3].above.is_some()), (0, true));
        assert_eq!(rows[3].merge_in.iter().map(|l| l.0).collect::<Vec<_>>(), [1]);
        assert_eq!(rows[3].below, None);
    }

    #[test]
    fn independent_tips_get_their_own_lanes_and_colors() {
        let rows = layout(&[c("x", &["root"]), c("y", &["root"]), c("root", &[])]);
        assert_eq!((rows[0].col, rows[1].col), (0, 1));
        assert_ne!(rows[0].color, rows[1].color);
        assert_eq!(rows[2].merge_in.len(), 1);
    }

    #[test]
    fn lanes_are_reused_after_they_close() {
        let rows = layout(&[c("x", &["root"]), c("y", &["root"]), c("root", &["older"]), c("older", &[]), c("lone", &[])]);
        assert_eq!(rows[4].col, 0, "a closed lane is reused");
        assert!(rows.iter().all(|r| r.width <= 2));
    }

    #[test]
    fn many_lanes_are_squeezed_into_the_cap() {
        // History with nine independent tips: lanes reach 9; with a limit of 4 none is drawn past 3.
        let mut commits: Vec<GraphCommit> = (0..9).map(|i| c(&format!("t{i}"), &["root"])).collect();
        commits.push(c("root", &[]));
        let mut rows = layout(&commits);
        assert_eq!(rows.iter().map(|r| r.width).max(), Some(9));
        cap_lanes(&mut rows, 4);
        assert!(rows.iter().all(|r| r.col < 4 && r.width <= 4));
        assert!(rows.iter().flat_map(|r| r.pass.iter().chain(&r.merge_in).chain(&r.fork_out)).all(|l| l.0 < 4));
    }

    #[test]
    fn commit_detail_is_parsed() {
        let head = parse_detail_head("Ada\x1fada@x.io\x1f2026-10-21 10:00\x1fp1 p2\x1fSubject line\n\nBody text\n", "abc");
        assert_eq!((head.author.as_str(), head.parents.len()), ("Ada", 2));
        assert_eq!(head.message, "Subject line\n\nBody text");
        let raw = ":100644 100644 a1 b1 M\tsrc/a.rs\n:000000 100644 0000 c1 A\tnew.txt\n:100644 100644 d1 e1 R087\told.rs\tnew.rs\n\n3\t1\tsrc/a.rs\n5\t0\tnew.txt\n-\t-\told.rs => new.rs\n";
        let files = parse_changes(raw);
        assert_eq!(files.len(), 3);
        assert_eq!((files[0].status, files[0].added, files[0].deleted), ('M', Some(3), Some(1)));
        assert_eq!((files[2].status, files[2].old.as_deref(), files[2].path.as_str(), files[2].added), ('R', Some("old.rs"), "new.rs", None));
        let d = CommitDetail { files, ..Default::default() };
        assert_eq!(d.totals(), (8, 1));
    }

    #[test]
    fn refs_are_classified() {
        let r = parse_refs("HEAD -> main, origin/main, tag: v0.1.0, feature");
        let kinds: Vec<RefKind> = r.iter().map(|l| l.kind).collect();
        assert_eq!(kinds, [RefKind::Head, RefKind::Remote, RefKind::Tag, RefKind::Local]);
        assert_eq!(r[0].name, "main");
        assert_eq!(r[2].name, "v0.1.0");
        assert!(parse_refs("").is_empty());
    }

    #[test]
    fn log_output_parses() {
        let out = "\x1eaaa\x1fa\x1fbbb ccc\x1fAda\x1fada@x.io\x1f2 days ago\x1f2026-10-21 10:00\x1fHEAD -> main\x1fMerge: x\n\x1ebbb\x1fb\x1f\x1fBob\x1fbob@x.io\x1f3 days ago\x1f2026-10-20 09:30\x1f\x1fRoot commit\n";
        let log = parse_graph_log(out);
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].parents, ["bbb", "ccc"]);
        assert_eq!(log[0].refs[0].kind, RefKind::Head);
        assert_eq!(log[1].parents.len(), 0);
        assert_eq!(log[1].subject, "Root commit");
        assert_eq!((log[0].email.as_str(), log[0].date.as_str()), ("ada@x.io", "2026-10-21 10:00"));
    }
}

// --- View modes ---

/// View of the GRAPH section.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GraphView {
    /// Two-line graph with lanes (default).
    #[default]
    Graph,
    /// Single-line dense list without lanes: the most commits in a narrow sidebar.
    Compact,
    /// List grouped by day ("Saturday, 24 Oct 2026").
    Dates,
}

impl GraphView {
    pub const ALL: [GraphView; 3] = [GraphView::Graph, GraphView::Compact, GraphView::Dates];

    pub fn id(self) -> &'static str {
        match self {
            GraphView::Graph => "graph",
            GraphView::Compact => "compact",
            GraphView::Dates => "dates",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            GraphView::Graph => crate::i18n::t("Graph"),
            GraphView::Compact => crate::i18n::t("List"),
            GraphView::Dates => crate::i18n::t("Days"),
        }
    }

    pub fn from_id(id: &str) -> Option<GraphView> {
        Self::ALL.into_iter().find(|v| v.id() == id)
    }

    /// Row height (px): the graph uses two lines, the others one.
    pub fn row_height(self) -> f32 {
        if self == GraphView::Graph { ROW_H } else { 30. }
    }
}

/// A row in the list: a commit (its index) or a day heading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphItem {
    Commit(usize),
    /// (heading, number of commits that day).
    Day(String, usize),
}

/// Month abbreviation, in the UI language (full name in Turkish).
fn month_name(i: usize) -> &'static str {
    use crate::i18n::t;
    match i {
        0 => t("Jan"),
        1 => t("Feb"),
        2 => t("Mar"),
        3 => t("Apr"),
        4 => t("May"),
        5 => t("Jun"),
        6 => t("Jul"),
        7 => t("Aug"),
        8 => t("Sep"),
        9 => t("Oct"),
        10 => t("Nov"),
        _ => t("Dec"),
    }
}

/// Day of the week (Monday=0), in the UI language.
fn weekday_name(i: usize) -> &'static str {
    use crate::i18n::t;
    match i {
        0 => t("Monday"),
        1 => t("Tuesday"),
        2 => t("Wednesday"),
        3 => t("Thursday"),
        4 => t("Friday"),
        5 => t("Saturday"),
        _ => t("Sunday"),
    }
}

/// "2026-10-24 21:46" → "Saturday, 24 Oct 2026" (no time zone needed: the local date in the text is used).
pub fn day_label(date: &str) -> Option<String> {
    let mut it = date.get(..10)?.split('-');
    let (y, m, d): (i64, i64, i64) = (it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // Day count (since 1970-01-01): the inverse of Howard Hinnant's civil-from-days.
    let (yy, mm) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * mm + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    // 1970-01-01 is a Thursday (Monday=0 → Thursday=3).
    let weekday = (days + 3).rem_euclid(7) as usize;
    Some(crate::i18n::tf("{0}, {1} {2} {3}", &[&weekday_name(weekday), &d, &month_name((m - 1) as usize), &y]))
}

/// Rows to list given the view and filter. `keep`: whether the commit is listed (search, "mine",
/// "hide merges"). In the `Dates` view a heading row is added when the day changes.
pub fn build_items(commits: &[GraphCommit], keep: impl Fn(&GraphCommit) -> bool, view: GraphView) -> Vec<GraphItem> {
    let mut out: Vec<GraphItem> = Vec::new();
    let mut day: Option<(String, usize)> = None; // (day key, index of the heading)
    for (ix, c) in commits.iter().enumerate() {
        if !keep(c) {
            continue;
        }
        if view == GraphView::Dates {
            let key = c.date.get(..10).unwrap_or("").to_string();
            if day.as_ref().is_none_or(|(k, _)| *k != key) {
                out.push(GraphItem::Day(day_label(&c.date).unwrap_or_else(|| key.clone()), 0));
                day = Some((key, out.len() - 1));
            }
            if let Some((_, h)) = &day
                && let GraphItem::Day(_, n) = &mut out[*h]
            {
                *n += 1;
            }
        }
        out.push(GraphItem::Commit(ix));
    }
    out
}

// --- Drawing ---

use gpui::{Bounds, IntoElement, PathBuilder, Pixels, Styled as _, canvas, fill, point, px, size};

use crate::ui::{self, Palette};

pub const LANE_W: f32 = 11.;
const PAD: f32 = 7.;
const LINE_W: f32 = 1.8;
/// Height of a graph row: two lines (subject, author · time).
pub const ROW_H: f32 = 46.;

/// Lane colors: from the theme colors, in a shuffled order so neighbors are distinguishable.
pub fn lane_color(p: &Palette, n: usize) -> u32 {
    [p.info, p.success, p.warning, p.syntax_keyword, p.danger, p.syntax_function, p.accent, p.syntax_type][n % COLORS]
}

/// Width of the graph (px): `cols` lanes.
pub fn width_px(cols: usize) -> f32 {
    PAD * 2. + cols.saturating_sub(1) as f32 * LANE_W
}

fn x_of(left: Pixels, col: usize) -> Pixels {
    left + px(PAD + col as f32 * LANE_W)
}

/// Lane drawing for a row; `head`: the checked-out commit (ringed node), `merge`: merge.
pub fn lane_element(row: GraphRow, cols: usize, head: bool, merge: bool, p: Palette) -> impl IntoElement {
    lane_element_h(row, cols, head, merge, p, ROW_H)
}

pub fn lane_element_h(row: GraphRow, cols: usize, head: bool, merge: bool, p: Palette, height: f32) -> impl IntoElement {
    let w = width_px(cols);
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _| {
            let (top, bottom) = (bounds.top(), bounds.bottom());
            let mid = top + (bottom - top) / 2.;
            let left = bounds.left();
            let stroke = |window: &mut gpui::Window, color: u32, build: &dyn Fn(&mut PathBuilder)| {
                let mut b = PathBuilder::stroke(px(LINE_W));
                build(&mut b);
                if let Ok(path) = b.build() {
                    window.paint_path(path, ui::c(color));
                }
            };
            for &(col, color) in &row.pass {
                let x = x_of(left, col);
                stroke(window, lane_color(&p, color), &|b| {
                    b.move_to(point(x, top));
                    b.line_to(point(x, bottom));
                });
            }
            let nx = x_of(left, row.col);
            if let Some(color) = row.above {
                stroke(window, lane_color(&p, color), &|b| {
                    b.move_to(point(nx, top));
                    b.line_to(point(nx, mid));
                });
            }
            if let Some(color) = row.below {
                stroke(window, lane_color(&p, color), &|b| {
                    b.move_to(point(nx, mid));
                    b.line_to(point(nx, bottom));
                });
            }
            for &(col, color) in &row.merge_in {
                let x = x_of(left, col);
                stroke(window, lane_color(&p, color), &|b| {
                    b.move_to(point(x, top));
                    b.curve_to(point(nx, mid), point(x, mid));
                });
            }
            for &(col, color) in &row.fork_out {
                let x = x_of(left, col);
                stroke(window, lane_color(&p, color), &|b| {
                    b.move_to(point(nx, mid));
                    b.curve_to(point(x, bottom), point(x, mid));
                });
            }
            let color = lane_color(&p, row.color);
            let dot = |window: &mut gpui::Window, r: f32, color: u32| {
                let b = Bounds::new(point(nx - px(r), mid - px(r)), size(px(r * 2.), px(r * 2.)));
                window.paint_quad(fill(b, ui::c(color)).corner_radii(px(r)));
            };
            if head {
                dot(window, 6., p.text);
                dot(window, 4.4, color);
            } else if merge {
                dot(window, 5., color);
                dot(window, 2.4, p.surface);
            } else {
                dot(window, 4.4, color);
            }
        },
    )
    .w(px(w))
    .h(px(height))
    .flex_none()
}

/// (text, background) colors of a branch / tag chip: HEAD is filled with the accent color, the others get colored text and a background
/// lightly mixed from the color. To hit AA (4.5) for small (10 px) text in every theme, the text darkens/lightens
/// toward the text color when needed.
pub fn ref_chip_colors(kind: RefKind, p: &Palette) -> (u32, u32) {
    let fg = match kind {
        RefKind::Head => return (p.on_accent, p.accent),
        RefKind::Local => p.info,
        RefKind::Remote => p.success,
        RefKind::Tag => p.warning,
    };
    // Background is 20% of the color; if unreadable (low-contrast themes like Solarized) the mix is reduced and the text is also
    // shifted toward the text color.
    let mut best = (fg, tint(fg, p.bg));
    for share in [0.20, 0.14, 0.08, 0.04, 0.0] {
        let bg = ui::mix(p.bg, fg, share);
        let text = ui::ensure_contrast(fg, p.text, &[bg], 4.6);
        best = (text, bg);
        if theme::themes::contrast(text, bg) >= 4.5 {
            break;
        }
    }
    best
}

/// Label background: 20% of the color, the rest is the background; readable with the text on light and dark themes.
pub fn tint(color: u32, bg: u32) -> u32 {
    let mix = |shift: u32| {
        let (c, b) = ((color >> shift) & 0xff, (bg >> shift) & 0xff);
        (c * 20 + b * 80) / 100
    };
    (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

#[cfg(test)]
mod tint_tests {
    use super::tint;

    fn channels(c: u32) -> [u32; 3] {
        [(c >> 16) & 0xff, (c >> 8) & 0xff, c & 0xff]
    }

    #[test]
    fn a_tag_tint_is_a_fifth_of_the_color_over_the_background() {
        // Light on a light background, dark on a dark one: the label reads against the text color in every theme.
        let color = 0xb45309;
        let (light, dark) = (tint(color, 0xffffff), tint(color, 0x1b1b1d));
        let lum = |c: u32| channels(c).iter().sum::<u32>();
        assert!(lum(light) > 3 * 0x80, "the tint over white is light: {light:06x}");
        assert!(lum(dark) < 3 * 0x80, "the tint over a dark background is dark: {dark:06x}");
        assert!(lum(light) > lum(dark), "light {light:06x} dark {dark:06x}");
        assert_eq!(tint(0x123456, 0x123456), 0x123456, "the same color over itself is unchanged");
        assert_eq!(tint(0xffffff, 0x000000), 0x333333, "20% of white over black");
    }

    #[test]
    fn tint_never_overflows_a_channel() {
        for (c, b) in [(0xffffff, 0xffffff), (0x000000, 0xffffff), (0xff00ff, 0x00ff00)] {
            assert!(channels(tint(c, b)).iter().all(|&x| x <= 255));
        }
    }
}

#[cfg(test)]
mod preview {
    use super::*;

    /// `GRAPH_SVG=/path.svg cargo test dump_graph_svg -- --ignored`: dumps the layout to SVG (for eyeballing).
    #[test]
    #[ignore = "manual: writes an SVG for eyeballing the layout, asserts nothing; needs GRAPH_SVG=<path>"]
    fn dump_graph_svg() {
        let Ok(out) = std::env::var("GRAPH_SVG") else {
            eprintln!("dump_graph_svg: GRAPH_SVG is not set, nothing was written");
            return;
        };
        let c = |h: &str, p: &[&str]| GraphCommit {
            hash: h.into(),
            short: h.into(),
            parents: p.iter().map(|x| x.to_string()).collect(),
            subject: format!("commit {h}"),
            ..Default::default()
        };
        // main plus two feature branches: one merged into main, the other open; there is also an old branch.
        let commits = vec![
            c("m3", &["m2", "g2"]),
            c("g2", &["g1"]),
            c("m2", &["m1", "f3"]),
            c("g1", &["m1"]),
            c("f3", &["f2"]),
            c("m1", &["m0"]),
            c("f2", &["f1"]),
            c("f1", &["m0"]),
            c("old2", &["old1"]),
            c("m0", &["root"]),
            c("old1", &["root"]),
            c("root", &[]),
        ];
        let rows = layout(&commits);
        let cols = rows.iter().map(|r| r.width).max().unwrap_or(1);
        let p = theme::DARK;
        let (lane_w, row_h, pad) = (LANE_W, 22.0, 8.0);
        let x = |col: usize| pad + col as f32 * lane_w;
        let mut svg = format!("<svg xmlns='http://www.w3.org/2000/svg' width='520' height='{}' style='background:#1b1f23'>", rows.len() as f32 * row_h);
        let color = |n: usize| format!("#{:06x}", lane_color(&p, n));
        for (i, r) in rows.iter().enumerate() {
            let (top, mid, bot) = (i as f32 * row_h, i as f32 * row_h + row_h / 2., (i + 1) as f32 * row_h);
            let line = |a: (f32, f32), b: (f32, f32), ctrl: Option<(f32, f32)>, col: &str| match ctrl {
                Some(k) => format!("<path d='M{} {} Q{} {} {} {}' stroke='{col}' stroke-width='1.6' fill='none'/>", a.0, a.1, k.0, k.1, b.0, b.1),
                None => format!("<line x1='{}' y1='{}' x2='{}' y2='{}' stroke='{col}' stroke-width='1.6'/>", a.0, a.1, b.0, b.1),
            };
            for &(c, k) in &r.pass {
                svg += &line((x(c), top), (x(c), bot), None, &color(k));
            }
            if let Some(k) = r.above {
                svg += &line((x(r.col), top), (x(r.col), mid), None, &color(k));
            }
            if let Some(k) = r.below {
                svg += &line((x(r.col), mid), (x(r.col), bot), None, &color(k));
            }
            for &(c, k) in &r.merge_in {
                svg += &line((x(c), top), (x(r.col), mid), Some((x(c), mid)), &color(k));
            }
            for &(c, k) in &r.fork_out {
                svg += &line((x(r.col), mid), (x(c), bot), Some((x(c), mid)), &color(k));
            }
            svg += &format!("<circle cx='{}' cy='{}' r='4' fill='{}'/>", x(r.col), mid, color(r.color));
            svg += &format!(
                "<text x='{}' y='{}' fill='#ddd' font-size='12' font-family='Helvetica'>{} ({} cols)</text>",
                pad * 2. + cols as f32 * lane_w,
                mid + 4.,
                commits[i].subject,
                r.width
            );
        }
        svg += "</svg>";
        std::fs::write(out, svg).unwrap();
    }
}

#[cfg(test)]
mod hardening {
    use super::*;

    /// Layout must not break on random (but valid: a parent is always older) history.
    #[test]
    fn random_histories_lay_out_consistently() {
        let mut x = 424242u64;
        let mut next = |m: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % m
        };
        for round in 0..300 {
            let n = 1 + next(60) as usize;
            // Parents of commit i come only from i+1.. (children-first order).
            let commits: Vec<GraphCommit> = (0..n)
                .map(|i| {
                    let mut parents = Vec::new();
                    if i + 1 < n {
                        parents.push(format!("c{}", i + 1 + next((n - i - 1).min(6) as u64) as usize));
                        if next(4) == 0 {
                            let p = format!("c{}", i + 1 + next((n - i - 1).min(6) as u64) as usize);
                            if !parents.contains(&p) {
                                parents.push(p);
                            }
                        }
                    }
                    GraphCommit { hash: format!("c{i}"), short: format!("c{i}"), parents, subject: format!("s{i}"), ..Default::default() }
                })
                .collect();
            let mut rows = layout(&commits);
            assert_eq!(rows.len(), n, "round {round}");
            cap_lanes(&mut rows, MAX_COLS);
            for (i, r) in rows.iter().enumerate() {
                assert!(r.col < MAX_COLS && r.width <= MAX_COLS && r.width > r.col.min(r.width.saturating_sub(1)), "round {round} row {i}: {r:?}");
                assert!(r.pass.iter().chain(&r.merge_in).chain(&r.fork_out).all(|l| l.0 < MAX_COLS), "round {round} row {i}");
                assert_eq!(r.below.is_some(), !commits[i].parents.is_empty(), "round {round} row {i}");
            }
        }
    }

    #[test]
    fn garbage_log_output_is_tolerated() {
        assert!(parse_graph_log("").is_empty());
        assert!(parse_graph_log("\x1e\x1e\x1e").is_empty());
        assert!(parse_graph_log("\x1ehash\x1fonly").is_empty(), "record with missing fields is skipped");
        let _ = parse_changes(":\n:::\t\n\t\t\n-\t-\n99999999999999999999\t1\tx\n");
        let _ = parse_detail_head("\x1f\x1f", "h");
        assert_eq!(parse_refs(", , HEAD -> , tag: ").len(), 2);
    }
}

#[cfg(test)]
mod view_tests {
    use super::*;

    fn commit(i: usize, date: &str, parents: usize) -> GraphCommit {
        GraphCommit {
            hash: format!("h{i}"),
            short: format!("h{i}"),
            parents: (0..parents).map(|p| format!("p{p}")).collect(),
            date: date.into(),
            ..Default::default()
        }
    }

    #[test]
    fn day_labels_have_the_right_weekday_and_month() {
        assert_eq!(day_label("2026-10-24 21:46").as_deref(), Some("Saturday, 24 Oct 2026"));
        assert_eq!(day_label("1970-01-01 00:00").as_deref(), Some("Thursday, 1 Jan 1970"));
        assert_eq!(day_label("2000-02-29 12:00").as_deref(), Some("Tuesday, 29 Feb 2000"), "leap day");
        assert_eq!(day_label("2024-12-31 23:59").as_deref(), Some("Tuesday, 31 Dec 2024"));
        assert_eq!(day_label("2026-13-01 00:00"), None);
        assert_eq!(day_label("garbage"), None);
        assert_eq!(day_label(""), None);
    }

    #[test]
    fn the_dates_view_groups_by_day_and_counts() {
        let commits =
            vec![commit(0, "2026-10-24 21:46", 1), commit(1, "2026-10-24 09:00", 1), commit(2, "2026-10-23 18:00", 2), commit(3, "2026-10-21 08:00", 1)];
        let items = build_items(&commits, |_| true, GraphView::Dates);
        assert_eq!(
            items,
            vec![
                GraphItem::Day("Saturday, 24 Oct 2026".into(), 2),
                GraphItem::Commit(0),
                GraphItem::Commit(1),
                GraphItem::Day("Friday, 23 Oct 2026".into(), 1),
                GraphItem::Commit(2),
                GraphItem::Day("Wednesday, 21 Oct 2026".into(), 1),
                GraphItem::Commit(3),
            ]
        );
        // Filter also fixes headings and counts: with the merges (2 parents) gone, 23 Oct doesn't appear at all.
        let no_merges = build_items(&commits, |c| c.parents.len() < 2, GraphView::Dates);
        assert_eq!(no_merges.iter().filter(|i| matches!(i, GraphItem::Day(..))).count(), 2);
        assert!(!no_merges.contains(&GraphItem::Commit(2)));
    }

    #[test]
    fn graph_and_compact_views_have_no_headers_and_keep_order() {
        let commits: Vec<_> = (0..5).map(|i| commit(i, "2026-10-24 10:00", 1)).collect();
        for view in [GraphView::Graph, GraphView::Compact] {
            let items = build_items(&commits, |c| c.hash != "h2", view);
            assert_eq!(items, vec![GraphItem::Commit(0), GraphItem::Commit(1), GraphItem::Commit(3), GraphItem::Commit(4)], "{view:?}");
        }
        assert!(build_items(&[], |_| true, GraphView::Dates).is_empty());
    }

    #[test]
    fn view_ids_round_trip_and_row_heights_fit_their_content() {
        for v in GraphView::ALL {
            assert_eq!(GraphView::from_id(v.id()), Some(v));
            assert!(!v.label().is_empty());
        }
        assert_eq!(GraphView::from_id("nope"), None);
        assert_eq!(GraphView::default(), GraphView::Graph);
        assert!(GraphView::Compact.row_height() < GraphView::Graph.row_height());
    }
}
