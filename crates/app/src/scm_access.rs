//! The accessible Source Control model uses the same state and operations as the painted panel.

use super::changes_ui::{ChangeKey, ChangeLine, discardable};
use super::repos_ui::Group;
use super::*;

/// Lines of the change list (group headers and files) in the accessibility tree: a range around the cursor while it
/// is on screen, otherwise around the rows on screen, so a huge status stays a small tree.
pub(crate) const ACCESS_LINES: usize = 500;

/// A file row of the change list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AccessFile {
    pub change: Change,
    pub staged: bool,
    pub group: Group,
    /// Its line in the whole list and its place (from 1) in its group.
    pub line: usize,
    pub position: usize,
}

impl AccessFile {
    pub fn mark(&self) -> Mark {
        if self.staged { self.change.staged } else { self.change.unstaged }.unwrap_or(Mark::Modified)
    }

    pub fn key(&self) -> ChangeKey {
        ChangeKey::File(self.group, self.change.path.clone())
    }

    /// Discard applies to unstaged, non-conflicted rows only (as on the painted row).
    pub fn discardable(&self) -> bool {
        discardable(self.group, &self.change)
    }

    pub fn conflict(&self) -> bool {
        self.group == Group::Merge
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AccessGroup {
    pub group: Group,
    pub id: &'static str,
    pub label: String,
    pub expanded: bool,
    pub total: usize,
    /// Its header's line, and whether that line is in the exposed range.
    pub line: usize,
    pub header_shown: bool,
    /// Its files in the exposed range.
    pub files: Vec<AccessFile>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AccessView {
    pub revision: Option<Arc<Status>>,
    pub root: Option<String>,
    pub branch: String,
    pub message: String,
    pub message_focused: bool,
    pub repos_focused: bool,
    pub busy: bool,
    pub commit_label: String,
    pub can_commit: bool,
    pub result: Option<(String, bool)>,
    /// Groups with a line in the exposed range of the change list.
    pub groups: Vec<AccessGroup>,
    /// The exposed range of the change list (its first line, how many) and the number of lines of the whole list.
    pub lines_first: usize,
    pub lines_shown: usize,
    pub lines_total: usize,
    /// The cursor's line, and whether the change list has the keyboard focus.
    pub cursor: Option<usize>,
    pub list_focused: bool,
    /// The repositories of the workspace (empty with a single repository, as the panel shows none then).
    pub repos: Vec<AccessRepo>,
    /// Folders extensions' source control providers claim (#144).
    pub ext: Vec<AccessExt>,
}

/// A folder a provider claims, as its section shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AccessExt {
    pub root: String,
    pub title: String,
    pub state: Option<Arc<ext_api::value::ScmState>>,
    pub message: String,
    pub validation: Option<String>,
    pub error: Option<String>,
    pub busy: bool,
    /// Ids of the groups that are open.
    pub open: Vec<String>,
}

/// A repository of the inventory: what it is, where it stands, whether it is the one Source Control acts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AccessRepo {
    pub key: RepoKey,
    pub label: String,
    pub path: String,
    pub summary: String,
    pub selected: bool,
    pub busy: bool,
    pub result: Option<(String, bool)>,
}

impl ScmPanel {
    pub(super) fn commit_state(&self, status: &Status, cx: &App) -> (String, bool) {
        let staged = status.staged().count();
        let changed = status.unstaged().filter(|c| c.unstaged != Some(Mark::Untracked)).count();
        let smart = cx.global::<crate::settings::Settings>().git_smart_commit;
        if smart || staged > 0 { commit_button(staged, changed) } else { commit_button(0, 0) }
    }

    pub(crate) fn a11y_view(&self, window: &Window, cx: &App) -> AccessView {
        let git = self.status.as_ref().filter(|_| !self.git_hidden());
        let (groups, range, total, cursor) = git.map_or((Vec::new(), 0..0, 0, None), |st| self.access_groups(st));
        let (commit_label, can_commit) = self.status.as_ref().map_or_else(|| commit_button(0, 0), |s| self.commit_state(s, cx));
        let selected = self.selected_repo_key();
        let repos = if self.multi() {
            self.repos
                .iter()
                .map(|r| AccessRepo {
                    key: r.id.key,
                    label: r.label.clone(),
                    path: r.id.worktree.clone(),
                    summary: repos_ui::summary(r),
                    selected: selected == Some(r.id.key),
                    busy: self.busy.contains(&r.id.worktree),
                    result: self.repo_results.get(&r.id.key).map(|(s, e)| (s.to_string(), *e)),
                })
                .collect()
        } else {
            Vec::new()
        };
        let mut branch = self.status.as_ref().and_then(|s| s.branch.clone()).unwrap_or_else(|| crate::i18n::t("detached HEAD").into());
        if let Some(repo) = repos.iter().find(|r| r.selected) {
            branch = format!("{} · {branch}", repo.label);
        }
        let ext = self
            .ext_sources()
            .iter()
            .map(|s| AccessExt {
                root: s.root.clone(),
                title: s.title.clone(),
                state: s.state.clone(),
                message: self.ext_message(&s.root, cx),
                validation: s.validation.clone(),
                error: s.error.clone(),
                busy: s.busy,
                open: s.state.iter().flat_map(|st| &st.groups).filter(|g| self.ext_group_open(&s.root, &g.id)).map(|g| g.id.clone()).collect(),
            })
            .collect();
        AccessView {
            ext,
            revision: self.status.clone(),
            root: git.map(|s| s.root.clone()),
            branch,
            repos,

            message: self.message.read(cx).text().to_string(),
            message_focused: self.message.focus_handle(cx).is_focused(window),
            repos_focused: self.multi() && self.repo_focus.is_focused(window),
            busy: self.busy(),
            commit_label,
            can_commit: can_commit && !self.busy(),
            result: self.result.as_ref().map(|(s, error)| (s.to_string(), *error)),
            groups,
            lines_first: range.start,
            lines_shown: range.len(),
            lines_total: total,
            cursor,
            list_focused: self.change_focus().is_focused(window),
        }
    }

    /// The groups and files of the change list's exposed range, that range, the number of lines and the cursor line.
    fn access_groups(&self, st: &Status) -> (Vec<AccessGroup>, std::ops::Range<usize>, usize, Option<usize>) {
        let lines = self.current_lines(st);
        let total = lines.len();
        // With several repositories the aggregated list is drawn instead: no cursor, no scroll position of this one.
        let (cursor, visible) = if self.multi() { (None, None) } else { (self.cursor_in(&lines, st), self.visible_change_lines(total)) };
        let first = crate::workbench::a11y::window_start(total, ACCESS_LINES, cursor, visible);
        let range = first..(first + ACCESS_LINES).min(total);
        let mut groups: Vec<AccessGroup> = Vec::new();
        let mut position = 0;
        for (ix, line) in lines.iter().enumerate().take(range.end) {
            match *line {
                ChangeLine::Group(g, n) => {
                    position = 0;
                    let expanded = self.sections.is_open(g.id(), true);
                    let header_shown = range.contains(&ix);
                    groups.push(AccessGroup {
                        group: g,
                        id: g.id(),
                        label: g.label().to_string(),
                        expanded,
                        total: n,
                        line: ix,
                        header_shown,
                        files: Vec::new(),
                    });
                }
                ChangeLine::File(g, i) => {
                    position += 1;
                    if let (true, Some(group), Some(change)) = (range.contains(&ix), groups.last_mut(), st.changes.get(i)) {
                        group.files.push(AccessFile { change: change.clone(), staged: g.staged(), group: g, line: ix, position });
                    }
                }
                ChangeLine::Empty => {}
            }
        }
        groups.retain(|g| g.header_shown || !g.files.is_empty());
        (groups, range, total, cursor)
    }

    pub(crate) fn a11y_set_message(&mut self, text: &str, cx: &mut Context<Self>) {
        if !self.busy() {
            self.message.update(cx, |m, cx| m.set_text(text, cx));
        }
    }

    pub(crate) fn a11y_toggle_group(&mut self, id: &str, cx: &mut Context<Self>) {
        self.sections.toggle(id, true);
        cx.emit(ScmEvent::SectionsChanged(self.sections.encode()));
        cx.notify();
    }

    /// Reject a stale action: the file must still be in the same group, in the same index and worktree state.
    pub(crate) fn a11y_has_file(&self, file: &AccessFile) -> bool {
        self.status.as_ref().filter(|_| !self.git_hidden()).is_some_and(|s| s.changes.contains(&file.change)) && file.group.holds(&file.change)
    }

    /// Is the group listed now (Merge and Staged Changes only while they hold something)?
    pub(crate) fn a11y_has_group(&self, group: Group) -> bool {
        self.status
            .as_ref()
            .filter(|_| !self.git_hidden())
            .is_some_and(|st| self.current_lines(st).iter().any(|l| matches!(l, ChangeLine::Group(g, _) if *g == group)))
    }

    pub(crate) fn a11y_group_open(&self, group: Group) -> bool {
        self.sections.is_open(group.id(), true)
    }

    /// The row's main action, as its painted button: stage it (a conflict: mark it resolved), or unstage it.
    pub(crate) fn a11y_stage_toggle(&mut self, file: &AccessFile, cx: &mut Context<Self>) {
        let paths = vec![file.change.path.clone()];
        if file.staged { self.unstage(paths, cx) } else { self.stage(paths, cx) }
    }

    /// Discard the row's changes after the question the painted button asks.
    pub(crate) fn a11y_discard(&mut self, file: &AccessFile, window: &mut Window, cx: &mut Context<Self>) {
        if file.discardable() {
            self.confirm_discard_in(None, vec![file.change.clone()], window, cx);
        }
    }
}
