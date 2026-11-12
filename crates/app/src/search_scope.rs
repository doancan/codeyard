//! Search scopes and the search history: what a search is limited to besides the file filter, and what a remembered
//! query brings back. Pure data and text; the panel (`search.rs`) draws and runs it, the workbench says which files
//! each scope means (`workbench_search_scope.rs`).

/// Number of remembered searches.
pub const HISTORY_MAX: usize = 30;

/// What the search looks in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    /// Every folder of the workspace (the file filter applies).
    #[default]
    Workspace,
    /// The files that are open in an editor.
    OpenEditors,
    /// The files git reports as changed (staged, unstaged or untracked).
    ChangedFiles,
    /// The folder of the file in front, and the folders below it.
    CurrentFolder,
}

impl Scope {
    pub const ALL: [Scope; 4] = [Scope::Workspace, Scope::OpenEditors, Scope::ChangedFiles, Scope::CurrentFolder];

    /// The files the workbench has to name for this scope (`Workspace` needs none).
    pub fn needs_paths(self) -> bool {
        self != Scope::Workspace
    }
}

/// The include globs a scope puts on one root folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Limit {
    /// No limit.
    Everything,
    /// Only these (relative, `/`-separated) globs; none at all means there is nothing to search in this folder.
    Only(Vec<String>),
}

/// A path with every character a glob reads as a pattern written literally. A comma cannot be written in a glob list
/// (the filter is comma-separated), so it becomes `?`: it matches the file itself and, at worst, a sibling that differs
/// only in that character.
pub fn literal_glob(path: &str) -> String {
    let mut out = String::with_capacity(path.len() + 2);
    for (i, ch) in path.chars().enumerate() {
        match ch {
            '\\' | '*' | '?' | '[' | ']' | '{' | '}' => {
                out.push('\\');
                out.push(ch);
            }
            '!' | '#' if i == 0 => {
                out.push('\\');
                out.push(ch);
            }
            ',' => out.push('?'),
            _ => out.push(ch),
        }
    }
    out
}

/// A path as the scope compares it: `/` separators, no `\\?\` prefix, the drive letter in lower case. Windows paths may
/// arrive in any of these forms (a backslash cannot be part of a Windows file name; on Unix it practically never is).
pub fn normalize(path: &str) -> String {
    let mut p = if cfg!(windows) { path.replace('\\', "/") } else { path.to_string() };
    if let Some(rest) = p.strip_prefix("//?/UNC/") {
        p = format!("//{rest}");
    } else if let Some(rest) = p.strip_prefix("//?/") {
        p = rest.to_string();
    }
    if p.as_bytes().get(1) == Some(&b':') && p.as_bytes()[0].is_ascii_alphabetic() {
        p[..1].make_ascii_lowercase();
    }
    p
}

fn same(a: &str, b: &str) -> bool {
    if cfg!(windows) { a.eq_ignore_ascii_case(b) } else { a == b }
}

/// `path` inside folder `root`, relative to it (both normalized; on Windows the comparison ignores case).
fn relative<'a>(path: &'a str, root: &str) -> Option<&'a str> {
    let root = root.trim_end_matches('/');
    let head = path.get(..root.len()).filter(|head| same(head, root))?;
    path[head.len()..].strip_prefix('/').filter(|rel| !rel.is_empty())
}

/// The files of the scope that lie in `root`, relative to it. `keep_commas` false drops the names with a comma.
fn files_in(root: &str, paths: &[String], keep_commas: bool) -> Vec<String> {
    let root = normalize(root);
    paths.iter().map(|p| normalize(p)).filter_map(|p| relative(&p, &root).map(str::to_string)).filter(|rel| keep_commas || !rel.contains(',')).collect()
}

/// What `scope` limits the search of the folder `root` to. `paths` are what the workbench named for the scope: absolute
/// files for the open editors and the changed files, the absolute folder for the current folder. File globs are anchored
/// at the folder (`/b.txt`), so a file of that name deeper down is not a match.
pub fn limit_for(scope: Scope, root: &str, paths: &[String]) -> Limit {
    limit_with(scope, root, paths, true)
}

/// As `limit_for`, but names that cannot be written in a glob list (a comma) are left out: a Search Editor or a
/// replacement running a pattern for them could reach other files, so they are not reviewed.
pub fn limit_exact(scope: Scope, root: &str, paths: &[String]) -> Limit {
    limit_with(scope, root, paths, false)
}

fn limit_with(scope: Scope, root: &str, paths: &[String], keep_commas: bool) -> Limit {
    match scope {
        Scope::Workspace => Limit::Everything,
        Scope::OpenEditors | Scope::ChangedFiles => {
            Limit::Only(files_in(root, paths, keep_commas).iter().map(|rel| format!("/{}", literal_glob(rel))).collect())
        }
        Scope::CurrentFolder => match paths.first().map(|p| normalize(p)) {
            None => Limit::Everything,
            Some(dir) if same(dir.trim_end_matches('/'), normalize(root).trim_end_matches('/')) => Limit::Everything,
            Some(dir) => {
                Limit::Only(relative(&dir, &normalize(root)).map(|rel| format!("/{}/**", literal_glob(rel.trim_end_matches('/')))).into_iter().collect())
            }
        },
    }
}

/// Whether the file `abs` is one the scope names: exactly these files for the open editors and the changed files,
/// anything below the folder for the current folder. This decides what is listed, whatever globs the agent matched.
pub fn admits(scope: Scope, paths: &[String], abs: &str) -> bool {
    let abs = normalize(abs);
    match scope {
        Scope::Workspace => true,
        Scope::OpenEditors | Scope::ChangedFiles => paths.iter().any(|p| same(&normalize(p), &abs)),
        Scope::CurrentFolder => paths.first().is_none_or(|dir| relative(&abs, &normalize(dir)).is_some()),
    }
}

/// A search as it was run: the text, the options and the file filters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    pub query: String,
    pub include: String,
    pub exclude: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    pub preserve_case: bool,
    pub scope: Scope,
}

/// The remembered searches (newest last) and the position being browsed with Up and Down.
#[derive(Default)]
pub struct History {
    entries: Vec<HistoryEntry>,
    at: Option<usize>,
}

/// Where a step through the history went.
#[derive(Debug, PartialEq, Eq)]
pub enum Step<'a> {
    Entry(&'a HistoryEntry),
    /// Past the newest entry: an empty search.
    Cleared,
    /// Nothing to step through.
    Stay,
}

impl History {
    /// Remembers a search; the same search again moves to the end. A blank query is not remembered.
    pub fn push(&mut self, entry: HistoryEntry) {
        self.at = None;
        if entry.query.trim().is_empty() {
            return;
        }
        self.entries.retain(|e| *e != entry);
        self.entries.push(entry);
        let extra = self.entries.len().saturating_sub(HISTORY_MAX);
        self.entries.drain(..extra);
    }

    /// Up (`older`) or Down through the entries; Down from the newest gives `Cleared`.
    pub fn step(&mut self, older: bool) -> Step<'_> {
        if self.entries.is_empty() {
            return Step::Stay;
        }
        self.at = match (self.at, older) {
            (None, true) => Some(self.entries.len() - 1),
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) if i + 1 < self.entries.len() => Some(i + 1),
            (Some(_), false) | (None, false) => None,
        };
        self.at.map_or(Step::Cleared, |i| Step::Entry(&self.entries[i]))
    }

    #[cfg(test)]
    pub fn queries(&self) -> Vec<String> {
        self.entries.iter().map(|e| e.query.clone()).collect()
    }

    #[cfg(test)]
    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(query: &str) -> HistoryEntry {
        HistoryEntry {
            query: query.into(),
            include: String::new(),
            exclude: String::new(),
            case_sensitive: false,
            whole_word: false,
            regex: false,
            preserve_case: false,
            scope: Scope::Workspace,
        }
    }

    #[test]
    fn globs_write_special_characters_literally() {
        assert_eq!(literal_glob("src/a.rs"), "src/a.rs");
        assert_eq!(literal_glob("src/[id]/page{1}.rs"), r"src/\[id\]/page\{1\}.rs");
        assert_eq!(literal_glob("a*b?.rs"), r"a\*b\?.rs");
        assert_eq!(literal_glob("!bang.rs"), r"\!bang.rs");
        assert_eq!(literal_glob("a!b.rs"), "a!b.rs");
        assert_eq!(literal_glob("#top.md"), r"\#top.md");
        assert_eq!(literal_glob("a,b.rs"), "a?b.rs", "a comma would split the list");
        assert_eq!(literal_glob("çay/şeker.rs"), "çay/şeker.rs");
    }

    #[test]
    fn scopes_become_globs_relative_to_each_root() {
        let files = ["/w/src/a.rs".to_string(), "/w/b[1].rs".to_string(), "/other/c.rs".to_string()];
        assert_eq!(limit_for(Scope::Workspace, "/w", &files), Limit::Everything);
        assert_eq!(limit_for(Scope::OpenEditors, "/w", &files), Limit::Only(vec!["/src/a.rs".into(), r"/b\[1\].rs".into()]), "anchored at the folder");
        assert_eq!(limit_for(Scope::ChangedFiles, "/other/", &files), Limit::Only(vec!["/c.rs".into()]));
        assert_eq!(limit_for(Scope::OpenEditors, "/w", &[]), Limit::Only(vec![]), "no files means nothing to search");
        assert_eq!(limit_for(Scope::OpenEditors, "/w", &["/w".to_string(), "/wx/y.rs".to_string()]), Limit::Only(vec![]), "a sibling folder is not inside");
    }

    #[test]
    fn the_current_folder_is_a_glob_below_it_and_the_root_means_everything() {
        let folder = |p: &str| vec![p.to_string()];
        assert_eq!(limit_for(Scope::CurrentFolder, "/w", &folder("/w/src/ui")), Limit::Only(vec!["/src/ui/**".into()]));
        assert_eq!(limit_for(Scope::CurrentFolder, "/w", &folder("/w/")), Limit::Everything);
        assert_eq!(limit_for(Scope::CurrentFolder, "/w", &folder("/w")), Limit::Everything);
        assert_eq!(limit_for(Scope::CurrentFolder, "/w", &folder("/elsewhere/x")), Limit::Only(vec![]));
        assert_eq!(limit_for(Scope::CurrentFolder, "/w", &[]), Limit::Everything, "no file in front: the folder is the workspace");
    }

    #[test]
    fn names_with_a_comma_are_left_out_of_the_exact_limit() {
        let files = ["/w/co,mma.txt".to_string(), "/w/plain.txt".to_string()];
        assert_eq!(limit_for(Scope::OpenEditors, "/w", &files), Limit::Only(vec!["/co?mma.txt".into(), "/plain.txt".into()]));
        assert_eq!(limit_exact(Scope::OpenEditors, "/w", &files), Limit::Only(vec!["/plain.txt".into()]));
    }

    #[test]
    fn admits_names_exactly_the_scope_files_whatever_a_glob_matched() {
        let files = ["/w/README.md".to_string(), "/w/co,mma.txt".to_string()];
        assert!(admits(Scope::OpenEditors, &files, "/w/README.md"));
        assert!(!admits(Scope::OpenEditors, &files, "/w/docs/README.md"), "the same name deeper down");
        assert!(!admits(Scope::ChangedFiles, &files, "/w/node_modules/x/README.md"));
        assert!(admits(Scope::ChangedFiles, &files, "/w/co,mma.txt"));
        assert!(!admits(Scope::ChangedFiles, &files, "/w/co-mma.txt"), "a sibling that `?` would match");
        assert!(admits(Scope::CurrentFolder, &["/w/src".to_string()], "/w/src/deep/a.rs"));
        assert!(!admits(Scope::CurrentFolder, &["/w/src".to_string()], "/w/srcx/a.rs"));
        assert!(admits(Scope::Workspace, &[], "/anything"));
    }

    #[test]
    fn windows_forms_of_a_path_compare_equal() {
        assert_eq!(normalize("//?/C:/w/a.rs"), "c:/w/a.rs");
        assert_eq!(normalize("//?/UNC/srv/share/a.rs"), "//srv/share/a.rs");
        assert_eq!(normalize("/w/a.rs"), "/w/a.rs");
        assert_eq!(limit_for(Scope::OpenEditors, "C:/w", &["c:/w/a.rs".to_string()]), Limit::Only(vec!["/a.rs".into()]), "drive letter case");
    }

    #[test]
    fn history_remembers_options_moves_repeats_to_the_end_and_stays_bounded() {
        let mut h = History::default();
        let mut with_options = entry("needle");
        with_options.include = "*.rs".into();
        with_options.regex = true;
        with_options.scope = Scope::OpenEditors;
        h.push(entry("needle"));
        h.push(with_options.clone());
        h.push(entry("   "));
        assert_eq!(h.entries().len(), 2, "the same text with other options is another search; blank is not kept");
        h.push(entry("needle"));
        assert_eq!(h.queries(), ["needle", "needle"]);
        assert_eq!(h.entries()[0], with_options, "the repeated search moved to the end");
        for i in 0..100 {
            h.push(entry(&format!("q{i}")));
        }
        assert_eq!(h.entries().len(), HISTORY_MAX);
        assert_eq!(h.entries().last().unwrap().query, "q99");
    }

    #[test]
    fn stepping_goes_older_then_newer_and_ends_with_an_empty_search() {
        let mut h = History::default();
        assert_eq!(h.step(true), Step::Stay);
        h.push(entry("a"));
        h.push(entry("b"));
        let q = |s: Step<'_>| match s {
            Step::Entry(e) => e.query.clone(),
            Step::Cleared => "<clear>".into(),
            Step::Stay => "<stay>".into(),
        };
        assert_eq!(
            [q(h.step(true)), q(h.step(true)), q(h.step(true)), q(h.step(false)), q(h.step(false)), q(h.step(false))],
            ["b", "a", "a", "b", "<clear>", "<clear>"]
        );
        assert_eq!(q(h.step(true)), "b", "browsing starts again from the newest");
    }
}
