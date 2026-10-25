//! Folder comparison without any UI: matching two scans, deciding what differs, and planning copies and deletes.
//! The view (`compare_folders`) shows these rows; the workbench (`workbench_compare`) carries out the plans.

use std::collections::{BTreeMap, HashMap, HashSet};

use proto::{HashRecord, Kind};

/// Files above this are compared by size and hash only: no text diff is offered for them.
pub const DIFF_LIMIT: u64 = 2 * 1024 * 1024;

/// Entries of one side, by path relative to its root.
pub type Tree = BTreeMap<String, HashRecord>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    Size,
    Content,
    /// A file on one side and a folder or link on the other.
    Type,
    LinkTarget,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    OnlyLeft,
    OnlyRight,
    Changed(Why),
    Identical,
    /// Differs byte for byte, but not once whitespace or line endings are ignored (see the toggles).
    Equivalent,
    /// Same size on both sides, the hashes are still being read.
    Pending,
    /// A folder that exists on both sides and has differences inside.
    Nested,
    Error,
}

impl Status {
    /// Rows the "show identical" toggle hides.
    pub fn is_same(self) -> bool {
        matches!(self, Status::Identical | Status::Equivalent)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SideInfo {
    pub size: u64,
    pub mtime_ms: i64,
    pub link: Option<String>,
    pub hash: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub path: String,
    pub depth: usize,
    pub kind: Kind,
    pub status: Status,
    pub left: Option<SideInfo>,
    pub right: Option<SideInfo>,
    /// Why the row could not be compared (permission denied...), with the side it happened on.
    pub error: Option<String>,
    /// Another entry differs from this one only in letter case: copying could overwrite it on a case-insensitive disk.
    pub case_clash: bool,
}

impl Row {
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    pub fn is_dir(&self) -> bool {
        self.kind == Kind::Dir
    }

    /// A text diff can be opened: a regular file on both sides that differs, and small enough to show.
    pub fn diffable(&self) -> bool {
        let small = |s: &Option<SideInfo>| s.as_ref().is_some_and(|s| s.size <= DIFF_LIMIT);
        self.kind == Kind::File && matches!(self.status, Status::Changed(Why::Size | Why::Content)) && small(&self.left) && small(&self.right)
    }
}

/// A path from the other side of a connection: plain names only, so it cannot lead out of the chosen root.
pub fn safe_rel(rel: &str) -> bool {
    !rel.is_empty()
        && !rel.contains('\0')
        && !(cfg!(windows) && rel.contains(['\\', ':']))
        && rel.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
}

/// A record as it will be used, or `None` if its path is not acceptable.
pub fn sanitize(rec: HashRecord) -> Option<HashRecord> {
    safe_rel(&rec.path).then_some(rec)
}

/// Both sides name the same folder on the same machine.
pub fn same_place(machine_a: &str, path_a: &str, machine_b: &str, path_b: &str) -> bool {
    let trim = |p: &str| p.trim_end_matches('/').to_string();
    machine_a == machine_b && trim(path_a) == trim(path_b)
}

fn info(r: &HashRecord) -> SideInfo {
    SideInfo { size: r.size, mtime_ms: r.mtime_ms, link: r.link.clone(), hash: r.hash }
}

fn pair_status(l: &HashRecord, r: &HashRecord, refine: &HashMap<String, bool>) -> (Status, Option<String>) {
    let error = match (&l.error, &r.error) {
        (Some(e), _) => Some(format!("{}: {e}", crate::i18n::t("left"))),
        (_, Some(e)) => Some(format!("{}: {e}", crate::i18n::t("right"))),
        _ => None,
    };
    if error.is_some() {
        return (Status::Error, error);
    }
    let status = if l.kind != r.kind {
        Status::Changed(Why::Type)
    } else {
        match l.kind {
            Kind::Symlink if l.link == r.link => Status::Identical,
            Kind::Symlink => Status::Changed(Why::LinkTarget),
            Kind::File if l.size != r.size => Status::Changed(Why::Size),
            Kind::File if l.size == 0 => Status::Identical,
            Kind::File => match (l.hash, r.hash) {
                (Some(a), Some(b)) if a == b => Status::Identical,
                (Some(_), Some(_)) => Status::Changed(Why::Content),
                _ => Status::Pending,
            },
            _ => Status::Identical,
        }
    };
    let status = match status {
        Status::Changed(Why::Size | Why::Content) if refine.get(&l.path) == Some(&true) => Status::Equivalent,
        s => s,
    };
    (status, None)
}

fn is_difference(s: Status) -> bool {
    !s.is_same()
}

/// Match the two scans into rows in tree order. A side that is not `done` may still deliver entries, so what only the
/// other side has is left out until then: rows shown early never turn into something else.
pub fn classify(left: &Tree, right: &Tree, done: [bool; 2], refine: &HashMap<String, bool>) -> Vec<Row> {
    let mut paths: Vec<&String> = left.keys().chain(right.keys().filter(|k| !left.contains_key(*k))).collect();
    paths.sort_by(|a, b| a.split('/').cmp(b.split('/')));
    // A folder that could not be read on one side says nothing about what is inside it.
    fn unreadable(t: &Tree) -> HashSet<&str> {
        t.values().filter(|r| r.kind == Kind::Dir && r.error.is_some()).map(|r| r.path.as_str()).collect()
    }
    let (blind_left, blind_right) = (unreadable(left), unreadable(right));
    let under = |set: &HashSet<&str>, path: &str| path.match_indices('/').any(|(i, _)| set.contains(&path[..i]));

    let mut rows = Vec::with_capacity(paths.len());
    for path in paths {
        let (l, r) = (left.get(path), right.get(path));
        let (kind, status, error) = match (l, r) {
            (Some(l), Some(r)) => {
                let (status, error) = pair_status(l, r, refine);
                (l.kind, status, error)
            }
            (Some(_), None) if !done[1] => continue,
            (Some(l), None) if under(&blind_right, path) => (l.kind, Status::Error, Some(crate::i18n::t("the folder cannot be read on the right").to_string())),
            (Some(l), None) => (l.kind, Status::OnlyLeft, l.error.as_ref().map(|e| format!("{}: {e}", crate::i18n::t("left")))),
            (None, Some(_)) if !done[0] => continue,
            (None, Some(r)) if under(&blind_left, path) => (r.kind, Status::Error, Some(crate::i18n::t("the folder cannot be read on the left").to_string())),
            (None, Some(r)) => (r.kind, Status::OnlyRight, r.error.as_ref().map(|e| format!("{}: {e}", crate::i18n::t("right")))),
            (None, None) => continue,
        };
        rows.push(Row {
            depth: path.matches('/').count(),
            path: path.clone(),
            kind,
            status,
            left: l.map(info),
            right: r.map(info),
            error,
            case_clash: false,
        });
    }

    // Folders present on both sides are "nested" when anything below them differs.
    let mut dirty: HashSet<&str> = HashSet::new();
    for row in rows.iter().filter(|r| is_difference(r.status)) {
        for (i, _) in row.path.match_indices('/') {
            dirty.insert(&row.path[..i]);
        }
    }
    let nested: Vec<usize> = (0..rows.len()).filter(|&i| rows[i].is_dir() && rows[i].status == Status::Identical && dirty.contains(rows[i].path.as_str())).collect();
    for i in nested {
        rows[i].status = Status::Nested;
    }

    let mut by_lower: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, row) in rows.iter().enumerate() {
        by_lower.entry(row.path.to_lowercase()).or_default().push(i);
    }
    for i in by_lower.into_values().filter(|v| v.len() > 1).flatten() {
        rows[i].case_clash = true;
    }
    rows
}

/// Files that exist on both sides with the same size and still need their hashes read.
pub fn hash_candidates(left: &Tree, right: &Tree) -> Vec<String> {
    left.values()
        .filter(|l| l.kind == Kind::File && l.size > 0 && l.error.is_none())
        .filter_map(|l| right.get(&l.path).filter(|r| r.kind == Kind::File && r.size == l.size && r.error.is_none()).map(|_| l))
        .filter(|l| l.hash.is_none() || right.get(&l.path).is_some_and(|r| r.hash.is_none()))
        .map(|l| l.path.clone())
        .collect()
}

/// Add a hashed record to the scan it came from. The size is taken from the newer record: a file that changed since the
/// listing then shows up as a size difference instead of a wrong hash match.
pub fn apply_hash(tree: &mut Tree, rec: HashRecord) {
    if let Some(entry) = tree.get_mut(&rec.path) {
        entry.hash = rec.hash;
        entry.error = rec.error;
        if rec.hash.is_some() {
            entry.size = rec.size;
            entry.mtime_ms = rec.mtime_ms;
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub only_left: usize,
    pub only_right: usize,
    pub changed: usize,
    pub identical: usize,
    pub errors: usize,
    pub pending: usize,
}

/// Files and links only: a folder is not a thing that differs by itself.
pub fn counts(rows: &[Row]) -> Counts {
    let mut c = Counts::default();
    for row in rows.iter().filter(|r| !r.is_dir()) {
        match row.status {
            Status::OnlyLeft => c.only_left += 1,
            Status::OnlyRight => c.only_right += 1,
            Status::Changed(_) => c.changed += 1,
            Status::Identical | Status::Equivalent => c.identical += 1,
            Status::Error => c.errors += 1,
            Status::Pending => c.pending += 1,
            Status::Nested => {}
        }
    }
    c
}

/// Rows to draw: tree order, without identical rows unless asked and without what is inside a folded folder. Folders that
/// exist on one side only start folded (they can hold thousands of files); `toggled` holds the folders the user flipped.
pub fn visible(rows: &[Row], show_identical: bool, toggled: &HashSet<String>) -> Vec<usize> {
    let mut out = Vec::new();
    let mut folded: Option<String> = None;
    for (i, row) in rows.iter().enumerate() {
        if let Some(prefix) = &folded {
            if row.path.starts_with(prefix.as_str()) {
                continue;
            }
            folded = None;
        }
        if !show_identical && row.status.is_same() {
            continue;
        }
        out.push(i);
        if row.is_dir() && is_folded(row, toggled) {
            folded = Some(format!("{}/", row.path));
        }
    }
    out
}

pub fn is_folded(row: &Row, toggled: &HashSet<String>) -> bool {
    let default = matches!(row.status, Status::OnlyLeft | Status::OnlyRight);
    default != toggled.contains(&row.path)
}

/// Text of a file with the differences the user chose to ignore removed. Binary data is returned as is.
pub fn normalize(data: &[u8], ignore_ws: bool, ignore_eol: bool) -> Vec<u8> {
    if data[..data.len().min(8192)].contains(&0) {
        return data.to_vec();
    }
    let mut out = Vec::with_capacity(data.len());
    for (i, &b) in data.iter().enumerate() {
        match b {
            b'\r' if ignore_ws || (ignore_eol && data.get(i + 1) == Some(&b'\n')) => {}
            b' ' | b'\t' if ignore_ws => {}
            _ => out.push(b),
        }
    }
    out
}

pub fn equal_ignoring(a: &[u8], b: &[u8], ignore_ws: bool, ignore_eol: bool) -> bool {
    normalize(a, ignore_ws, ignore_eol) == normalize(b, ignore_ws, ignore_eol)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    LeftToRight,
    RightToLeft,
}

/// What the destination must look like when a copy writes to it: anything else means it changed after the scan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expect {
    Absent,
    Hash(u64),
    /// The size was different, so the file was never hashed: size and time stand in.
    Stat { size: u64, mtime_ms: i64 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct CopyJob {
    pub path: String,
    pub is_dir: bool,
    /// What the source looked like in the scan.
    pub source: Expect,
    pub dest: Expect,
}

#[derive(Debug, Default, PartialEq)]
pub struct CopyPlan {
    pub jobs: Vec<CopyJob>,
    /// Entries left out, each with the reason.
    pub skipped: Vec<(String, String)>,
}

fn expect_of(s: &SideInfo) -> Expect {
    s.hash.map_or(Expect::Stat { size: s.size, mtime_ms: s.mtime_ms }, Expect::Hash)
}

/// Everything under `path` (the entry itself included) that has to be copied for the other side to match. Entries the other
/// side has extra are not touched; links, kind mismatches and case clashes are named and left out.
pub fn plan_copy(rows: &[Row], path: &str, dir: Direction) -> CopyPlan {
    let prefix = format!("{path}/");
    let mut plan = CopyPlan::default();
    for row in rows.iter().filter(|r| r.path == path || r.path.starts_with(&prefix)) {
        let (src, dst, only_src) = match dir {
            Direction::LeftToRight => (&row.left, &row.right, Status::OnlyLeft),
            Direction::RightToLeft => (&row.right, &row.left, Status::OnlyRight),
        };
        let Some(src) = src else { continue };
        let wanted = row.status == only_src || matches!(row.status, Status::Changed(_));
        if !wanted {
            if matches!(row.status, Status::Error | Status::Pending) {
                plan.skipped.push((row.path.clone(), crate::i18n::t("it could not be compared").to_string()));
            }
            continue;
        }
        let reason = if row.case_clash {
            Some(crate::i18n::t("its name differs from another entry only in letter case"))
        } else if row.kind == Kind::Symlink {
            Some(crate::i18n::t("symbolic links are not copied"))
        } else if row.kind == Kind::Other {
            Some(crate::i18n::t("it is not a regular file or folder"))
        } else if row.status == Status::Changed(Why::Type) {
            Some(crate::i18n::t("it is a file on one side and a folder or link on the other"))
        } else {
            None
        };
        if let Some(why) = reason {
            plan.skipped.push((row.path.clone(), why.to_string()));
        } else if row.is_dir() {
            plan.jobs.push(CopyJob { path: row.path.clone(), is_dir: true, source: Expect::Absent, dest: Expect::Absent });
        } else {
            plan.jobs.push(CopyJob { path: row.path.clone(), is_dir: false, source: expect_of(src), dest: dst.as_ref().map_or(Expect::Absent, expect_of) });
        }
    }
    plan
}

/// What a delete has to find still in place: the files and folders of the subtree as the scan saw them.
#[derive(Debug, PartialEq)]
pub struct DeletePlan {
    pub path: String,
    pub is_dir: bool,
    pub snapshot: Vec<(String, Kind, u64, i64)>,
}

/// Delete `path` on one side. Offered for entries the side has (whatever the other side has).
pub fn plan_delete(rows: &[Row], path: &str, side: Side) -> Option<DeletePlan> {
    let prefix = format!("{path}/");
    let mut plan = DeletePlan { path: path.to_string(), is_dir: false, snapshot: Vec::new() };
    let mut found = false;
    for row in rows.iter().filter(|r| r.path == path || r.path.starts_with(&prefix)) {
        let on = match side {
            Side::Left => &row.left,
            Side::Right => &row.right,
        };
        let Some(on) = on else { continue };
        if row.path == path {
            found = true;
            plan.is_dir = row.is_dir();
        }
        plan.snapshot.push((row.path.clone(), row.kind, if row.is_dir() { 0 } else { on.size }, on.mtime_ms));
    }
    found.then_some(plan)
}

/// "1.5 MB": binary units, one decimal from the kilobyte up.
pub fn human_size(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let (mut v, mut i) = (n as f64, 0);
    while v >= 1024. && i < UNITS.len() - 1 {
        v /= 1024.;
        i += 1;
    }
    if i == 0 { format!("{n} B") } else { format!("{v:.1} {}", UNITS[i]) }
}

/// "2026-10-24 21:46" in UTC (the time zone of a remote machine is not known here).
pub fn utc_text(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", rem / 3600, rem % 3600 / 60)
}

/// The line under the toolbar and at the top of the report.
pub fn summary(c: &Counts) -> String {
    crate::i18n::tf("{0} only on the left, {1} only on the right, {2} changed, {3} identical, {4} errors", &[&c.only_left, &c.only_right, &c.changed, &c.identical, &c.errors])
}

/// Plain text for "Copy Report" and "Export as text": counts, then every difference grouped by kind.
pub fn report(rows: &[Row], left: &str, right: &str, show_identical: bool) -> String {
    use std::fmt::Write;
    let t = crate::i18n::t;
    let mut out = format!("{} ↔ {}\n{}\n", left, right, summary(&counts(rows)));
    let groups: [(&str, &dyn Fn(&Row) -> bool); 4] = [
        (crate::i18n::tr("Only on the left"), &|r| r.status == Status::OnlyLeft),
        (crate::i18n::tr("Only on the right"), &|r| r.status == Status::OnlyRight),
        (crate::i18n::tr("Changed"), &|r| matches!(r.status, Status::Changed(_))),
        (crate::i18n::tr("Could not be compared"), &|r| r.status == Status::Error),
    ];
    for (title, pick) in groups {
        let list: Vec<&Row> = rows.iter().filter(|r| !r.is_dir() && pick(r)).collect();
        if list.is_empty() {
            continue;
        }
        let _ = writeln!(out, "\n{} ({})", t(title), list.len());
        for r in list {
            let note = match (&r.status, &r.error) {
                (Status::Changed(why), _) => format!("  [{}]", why_text(*why)),
                (_, Some(e)) => format!("  [{e}]"),
                _ => String::new(),
            };
            let _ = writeln!(out, "  {}{note}", r.path);
        }
    }
    if show_identical {
        let list: Vec<&Row> = rows.iter().filter(|r| !r.is_dir() && r.status.is_same()).collect();
        if !list.is_empty() {
            let _ = writeln!(out, "\n{} ({})", t("Identical"), list.len());
            for r in list {
                let _ = writeln!(out, "  {}", r.path);
            }
        }
    }
    out
}

pub fn why_text(why: Why) -> &'static str {
    match why {
        Why::Size => crate::i18n::t("size differs"),
        Why::Content => crate::i18n::t("content differs"),
        Why::Type => crate::i18n::t("different kinds"),
        Why::LinkTarget => crate::i18n::t("link target differs"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(path: &str, kind: Kind, size: u64, hash: Option<u64>) -> HashRecord {
        HashRecord { path: path.into(), kind, size, mtime_ms: 1000, hash, link: None, error: None }
    }

    fn file(path: &str, size: u64, hash: u64) -> HashRecord {
        rec(path, Kind::File, size, Some(hash))
    }

    fn tree(list: Vec<HashRecord>) -> Tree {
        list.into_iter().map(|r| (r.path.clone(), r)).collect()
    }

    fn status_of(rows: &[Row], path: &str) -> Status {
        rows.iter().find(|r| r.path == path).unwrap_or_else(|| panic!("no row {path}")).status
    }

    fn rows(l: Vec<HashRecord>, r: Vec<HashRecord>) -> Vec<Row> {
        classify(&tree(l), &tree(r), [true, true], &HashMap::new())
    }

    #[test]
    fn classifies_by_presence_size_and_hash() {
        let rows = rows(
            vec![file("only-l", 1, 1), file("same", 3, 7), file("size", 3, 7), file("body", 3, 7)],
            vec![file("only-r", 1, 1), file("same", 3, 7), file("size", 4, 7), file("body", 3, 8)],
        );
        assert_eq!(status_of(&rows, "only-l"), Status::OnlyLeft);
        assert_eq!(status_of(&rows, "only-r"), Status::OnlyRight);
        assert_eq!(status_of(&rows, "same"), Status::Identical);
        assert_eq!(status_of(&rows, "size"), Status::Changed(Why::Size));
        assert_eq!(status_of(&rows, "body"), Status::Changed(Why::Content));
    }

    #[test]
    fn equal_size_and_time_without_a_hash_is_pending_never_identical() {
        let l = rec("a", Kind::File, 5, None);
        let r = rec("a", Kind::File, 5, None);
        assert_eq!(l.mtime_ms, r.mtime_ms);
        let rows = rows(vec![l.clone()], vec![r.clone()]);
        assert_eq!(rows[0].status, Status::Pending);
        let (lt, rt) = (tree(vec![l]), tree(vec![r]));
        assert_eq!(hash_candidates(&lt, &rt), ["a"]);
    }

    #[test]
    fn only_equal_sized_unhashed_files_are_hash_candidates() {
        let l = tree(vec![rec("a", Kind::File, 5, None), rec("b", Kind::File, 6, None), rec("c", Kind::File, 0, None), rec("d", Kind::File, 1, None)]);
        let r = tree(vec![rec("a", Kind::File, 5, None), rec("b", Kind::File, 7, None), rec("c", Kind::File, 0, None), rec("e", Kind::File, 1, None)]);
        assert_eq!(hash_candidates(&l, &r), ["a"]);
        let rows = classify(&l, &r, [true, true], &HashMap::new());
        assert_eq!(status_of(&rows, "c"), Status::Identical, "two empty files need no hash");
    }

    #[test]
    fn a_file_that_changed_after_the_listing_shows_as_a_size_difference() {
        let mut l = tree(vec![rec("a", Kind::File, 5, None)]);
        let r = tree(vec![rec("a", Kind::File, 5, Some(1))]);
        apply_hash(&mut l, HashRecord { size: 9, hash: Some(1), ..rec("a", Kind::File, 9, None) });
        assert_eq!(classify(&l, &r, [true, true], &HashMap::new())[0].status, Status::Changed(Why::Size));
    }

    #[test]
    fn rows_come_in_tree_order_and_folders_with_differences_are_nested() {
        let dir = |p: &str| rec(p, Kind::Dir, 0, None);
        let rows = rows(
            vec![dir("a"), file("a/x", 1, 1), dir("a-b"), dir("same"), file("same/f", 1, 1)],
            vec![dir("a"), file("a/x", 1, 2), dir("a-b"), dir("same"), file("same/f", 1, 1)],
        );
        let order: Vec<&str> = rows.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(order, ["a", "a/x", "a-b", "same", "same/f"], "children follow their folder even though '-' sorts before '/'");
        assert_eq!(status_of(&rows, "a"), Status::Nested);
        assert_eq!(status_of(&rows, "same"), Status::Identical);
        assert_eq!(rows[1].depth, 1);
    }

    #[test]
    fn counts_files_not_folders() {
        let rows = rows(
            vec![rec("d", Kind::Dir, 0, None), file("d/a", 1, 1), file("l", 1, 1)],
            vec![file("r", 1, 1), file("l", 1, 1)],
        );
        assert_eq!(counts(&rows), Counts { only_left: 1, only_right: 1, changed: 0, identical: 1, errors: 0, pending: 0 });
    }

    #[test]
    fn symlinks_compare_by_target() {
        let link = |p: &str, t: &str| HashRecord { link: Some(t.into()), ..rec(p, Kind::Symlink, 0, None) };
        let rows = rows(vec![link("same", "x"), link("moved", "x"), link("vs-file", "x")], vec![link("same", "x"), link("moved", "y"), file("vs-file", 1, 1)]);
        assert_eq!(status_of(&rows, "same"), Status::Identical);
        assert_eq!(status_of(&rows, "moved"), Status::Changed(Why::LinkTarget));
        assert_eq!(status_of(&rows, "vs-file"), Status::Changed(Why::Type));
    }

    #[test]
    fn an_unreadable_folder_is_an_error_and_what_it_hides_is_not_reported_as_missing() {
        let bad = HashRecord { error: Some("permission denied".into()), ..rec("locked", Kind::Dir, 0, None) };
        let rows = rows(vec![bad, rec("ok", Kind::Dir, 0, None)], vec![rec("locked", Kind::Dir, 0, None), file("locked/inner", 1, 1), rec("ok", Kind::Dir, 0, None)]);
        assert_eq!(status_of(&rows, "locked"), Status::Error);
        assert_eq!(rows.iter().find(|r| r.path == "locked").unwrap().error.as_deref(), Some("left: permission denied"));
        assert_eq!(status_of(&rows, "locked/inner"), Status::Error, "not 'only on the right'");
        assert_eq!(status_of(&rows, "ok"), Status::Identical);
    }

    #[test]
    fn names_that_differ_only_in_case_are_flagged_and_not_copied() {
        let rows = rows(vec![file("Readme", 1, 1), file("fine", 1, 1)], vec![file("readme", 1, 1), file("fine", 1, 1)]);
        assert!(rows.iter().filter(|r| r.path.to_lowercase() == "readme").all(|r| r.case_clash));
        assert!(!rows.iter().find(|r| r.path == "fine").unwrap().case_clash);
        let plan = plan_copy(&rows, "Readme", Direction::LeftToRight);
        assert!(plan.jobs.is_empty());
        assert_eq!(plan.skipped.len(), 1);
    }

    #[test]
    fn entries_still_arriving_do_not_show_up_as_only_one_side() {
        let (l, r) = (tree(vec![file("a", 1, 1), file("b", 1, 1)]), tree(vec![file("a", 1, 1)]));
        let early = classify(&l, &r, [true, false], &HashMap::new());
        assert_eq!(early.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(), ["a"]);
        assert_eq!(classify(&l, &r, [true, true], &HashMap::new()).len(), 2);
    }

    #[test]
    fn whitespace_and_eol_refinement_turns_changed_into_equivalent() {
        assert!(equal_ignoring(b"a b\r\nc\r\n", b"a b\nc\n", false, true));
        assert!(!equal_ignoring(b"a b\r\nc\r\n", b"a b\nc\n", false, false));
        assert!(!equal_ignoring(b"a  b\n", b"a b\n", false, true), "spaces still count with only the EOL toggle");
        assert!(equal_ignoring(b"a  b\t\n", b"a b\n", true, false));
        assert!(!equal_ignoring(b"ab\n", b"a\nb\n", true, true), "line breaks are not whitespace here");
        assert!(!equal_ignoring(&[0, b' ', 1], &[0, 1], true, true), "binary data is never normalised");
        let mut refine = HashMap::new();
        refine.insert("a".to_string(), true);
        let rows = classify(&tree(vec![file("a", 3, 1)]), &tree(vec![file("a", 4, 2)]), [true, true], &refine);
        assert_eq!(rows[0].status, Status::Equivalent);
        assert_eq!(counts(&rows).identical, 1);
    }

    #[test]
    fn visibility_hides_identical_and_folded_rows() {
        let dir = |p: &str| rec(p, Kind::Dir, 0, None);
        let rows = rows(
            vec![dir("both"), file("both/a", 1, 1), file("both/same", 1, 1), dir("gone"), file("gone/x", 1, 1), file("same", 1, 1)],
            vec![dir("both"), file("both/a", 1, 2), file("both/same", 1, 1), file("same", 1, 1)],
        );
        let toggled = HashSet::new();
        let shown = |v: Vec<usize>| v.into_iter().map(|i| rows[i].path.clone()).collect::<Vec<_>>();
        assert_eq!(shown(visible(&rows, false, &toggled)), ["both", "both/a", "gone"], "only-left folder starts folded, identical hidden");
        let unfolded: HashSet<String> = ["gone".to_string()].into();
        assert_eq!(shown(visible(&rows, false, &unfolded)), ["both", "both/a", "gone", "gone/x"]);
        assert_eq!(visible(&rows, true, &toggled).len(), 5, "everything but the folded child");
        let fold_both: HashSet<String> = ["both".to_string()].into();
        assert_eq!(shown(visible(&rows, true, &fold_both)), ["both", "gone", "same"]);
    }

    #[test]
    fn rejects_paths_that_could_leave_the_root() {
        for bad in ["", "/etc/passwd", "../x", "a/../b", "a//b", "./a", "a/.", "a\0b", "a/"] {
            assert!(!safe_rel(bad), "{bad:?}");
        }
        for good in ["a", "a/b.rs", "ğüş/ı", ".hidden/x", "a b/c"] {
            assert!(safe_rel(good), "{good:?}");
        }
        assert!(sanitize(file("../evil", 1, 1)).is_none());
        assert!(sanitize(file("fine/x", 1, 1)).is_some());
    }

    #[test]
    fn same_folder_means_same_machine_and_path() {
        assert!(same_place("local", "/a/b/", "local", "/a/b"));
        assert!(!same_place("local", "/a/b", "ssh://web", "/a/b"));
        assert!(!same_place("local", "/a/b", "local", "/a/c"));
    }

    #[test]
    fn copy_plan_covers_a_subtree_and_names_what_it_leaves_out() {
        let dir = |p: &str| rec(p, Kind::Dir, 0, None);
        let link = HashRecord { link: Some("t".into()), ..rec("d/link", Kind::Symlink, 0, None) };
        let rows = rows(
            vec![dir("d"), file("d/new", 2, 5), file("d/changed", 2, 6), file("d/same", 1, 1), link, dir("d/sub")],
            vec![dir("d"), file("d/changed", 2, 7), file("d/same", 1, 1), file("d/extra", 1, 1)],
        );
        let plan = plan_copy(&rows, "d", Direction::LeftToRight);
        let jobs: Vec<(&str, bool)> = plan.jobs.iter().map(|j| (j.path.as_str(), j.is_dir)).collect();
        assert_eq!(jobs, [("d/changed", false), ("d/new", false), ("d/sub", true)]);
        assert_eq!(plan.jobs[0].dest, Expect::Hash(7));
        assert_eq!(plan.jobs[0].source, Expect::Hash(6));
        assert_eq!(plan.jobs[1].dest, Expect::Absent);
        assert_eq!(plan.skipped.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(), ["d/link"]);
        let back = plan_copy(&rows, "d/extra", Direction::RightToLeft);
        assert_eq!(back.jobs.len(), 1);
        assert!(plan_copy(&rows, "d/extra", Direction::LeftToRight).jobs.is_empty(), "nothing on the left to copy");
    }

    #[test]
    fn a_size_difference_is_guarded_by_size_and_time() {
        let rows = rows(vec![file("a", 2, 1)], vec![rec("a", Kind::File, 3, None)]);
        let plan = plan_copy(&rows, "a", Direction::LeftToRight);
        assert_eq!(plan.jobs[0].dest, Expect::Stat { size: 3, mtime_ms: 1000 });
    }

    #[test]
    fn delete_plan_snapshots_the_subtree_of_one_side() {
        let dir = |p: &str| rec(p, Kind::Dir, 0, None);
        let rows = rows(vec![dir("d"), file("d/a", 4, 1)], vec![dir("d")]);
        let plan = plan_delete(&rows, "d", Side::Left).unwrap();
        assert!(plan.is_dir);
        assert_eq!(plan.snapshot, [("d".to_string(), Kind::Dir, 0, 1000), ("d/a".to_string(), Kind::File, 4, 1000)]);
        assert_eq!(plan_delete(&rows, "d", Side::Right).unwrap().snapshot.len(), 1);
        assert!(plan_delete(&rows, "d/a", Side::Right).is_none(), "the right side has no such file");
    }

    #[test]
    fn sizes_and_times_read_like_a_file_manager() {
        assert_eq!((human_size(0).as_str(), human_size(1023).as_str(), human_size(1536).as_str()), ("0 B", "1023 B", "1.5 KB"));
        assert_eq!(human_size(5 * 1024 * 1024 * 1024), "5.0 GB");
        assert_eq!(utc_text(0), "1970-01-01 00:00");
        assert_eq!(utc_text(1_793_000_060_000), "2026-10-26 07:34");
        assert_eq!(utc_text(-1000), "1969-12-31 23:59");
    }

    #[test]
    fn the_report_lists_differences_by_group() {
        let rows = rows(vec![file("l", 1, 1), file("c", 1, 1), file("s", 1, 1)], vec![file("r", 1, 1), file("c", 1, 2), file("s", 1, 1)]);
        let text = report(&rows, "A", "B", false);
        assert!(text.starts_with("A ↔ B\n1 only on the left, 1 only on the right, 1 changed, 1 identical, 0 errors"), "{text}");
        assert!(text.contains("Only on the left (1)\n  l\n"));
        assert!(text.contains("Changed (1)\n  c  [content differs]\n"));
        assert!(!text.contains("Identical"));
        assert!(report(&rows, "A", "B", true).contains("Identical (1)\n  s\n"));
    }
}
