//! Source Control with several repositories: one list that shows every repository of the workspace (branch, change,
//! sync and operation state) with its changes grouped under it. Rows and group buttons act on their own repository;
//! the selected repository is the one the message box, Commit, Sync and the graph use. The list is virtualized.

use std::collections::HashSet;
use std::sync::Arc;

use gpui::{Context, ScrollStrategy, SharedString, div, prelude::*, px, uniform_list};

use super::{ScmEvent, ScmPanel};
use crate::git::{self, Change, Mark};
use crate::i18n::t;
use crate::repos::{Repo, RepoKey};
use crate::ui::{self, c, icon, layout, palette, text};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Group {
    Merge,
    Staged,
    Changes,
}

impl Group {
    pub(crate) const ALL: [Group; 3] = [Group::Merge, Group::Staged, Group::Changes];

    pub(crate) fn id(self) -> &'static str {
        match self {
            Group::Merge => "scm-merge",
            Group::Staged => "scm-staged",
            Group::Changes => "scm-changes",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Group::Merge => t("Merge Changes"),
            Group::Staged => t("Staged Changes"),
            Group::Changes => t("Changes"),
        }
    }

    pub(crate) fn staged(self) -> bool {
        self == Group::Staged
    }

    pub(crate) fn holds(self, c: &Change) -> bool {
        match self {
            Group::Merge => c.unstaged == Some(Mark::Conflict),
            Group::Staged => c.staged.is_some(),
            Group::Changes => c.unstaged.is_some() && c.unstaged != Some(Mark::Conflict),
        }
    }
}

/// One row of the aggregated list. Indexes point into the panel's repositories and their status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Line {
    Repo(usize),
    Group(usize, Group),
    File(usize, Group, usize),
    /// "No changes.", "Loading…" or the repository's error.
    Note(usize),
}

/// Short state of a repository for its row and its accessible description: branch (or detached), ahead/behind,
/// upstream, operation in progress, change count.
pub(crate) fn summary(repo: &Repo) -> String {
    let Some(st) = &repo.status else {
        return repo.error.clone().unwrap_or_else(|| t("Loading…").to_string());
    };
    let mut parts = vec![st.branch.clone().unwrap_or_else(|| t("detached HEAD").to_string())];
    if st.ahead > 0 || st.behind > 0 {
        parts.push(git::counts_label(st.ahead, st.behind));
    }
    if st.branch.is_some() && st.upstream.is_none() {
        parts.push(t("no upstream").to_string());
    }
    if let Some(op) = repo.op.label() {
        parts.push(op.to_string());
    }
    let conflicts = st.changes.iter().filter(|c| c.unstaged == Some(Mark::Conflict)).count();
    if conflicts > 0 {
        parts.push(crate::i18n::tn("{} conflict", "{} conflicts", conflicts, &[]));
    }
    parts.push(crate::i18n::tn("{} change", "{} changes", st.changes.len(), &[]));
    parts.join(" · ")
}

impl ScmPanel {
    /// More than one repository: the panel shows the aggregated list.
    pub(crate) fn multi(&self) -> bool {
        self.repos.len() > 1
    }

    /// The workbench's inventory changed. State kept for repositories that are gone is dropped.
    pub fn set_repos(&mut self, repos: Arc<Vec<Repo>>, cx: &mut Context<Self>) {
        let keys: HashSet<RepoKey> = repos.iter().map(|r| r.id.key).collect();
        let roots: HashSet<&str> = repos.iter().map(|r| r.id.worktree.as_str()).collect();
        self.repo_results.retain(|k, _| keys.contains(k));
        self.collapsed_repos.retain(|k| keys.contains(k));
        self.drafts.retain(|r, _| roots.contains(r.as_str()));
        self.repos = repos;
        cx.notify();
    }

    #[cfg(test)]
    pub fn repos_for_test(&self) -> Arc<Vec<Repo>> {
        self.repos.clone()
    }

    #[cfg(test)]
    pub fn repo_result_for_test(&self, key: RepoKey) -> Option<String> {
        self.repo_results.get(&key).map(|(s, _)| s.to_string())
    }

    #[cfg(test)]
    pub(crate) fn lines_for_test(&self) -> Vec<Line> {
        self.lines()
    }

    #[cfg(test)]
    pub fn busy_in(&self, root: &str) -> bool {
        self.busy.contains(root)
    }

    pub(crate) fn lines(&self) -> Vec<Line> {
        let mut out = Vec::new();
        for (i, repo) in self.repos.iter().enumerate() {
            out.push(Line::Repo(i));
            if self.collapsed_repos.contains(&repo.id.key) {
                continue;
            }
            let Some(st) = &repo.status else {
                out.push(Line::Note(i));
                continue;
            };
            if st.changes.is_empty() {
                out.push(Line::Note(i));
                continue;
            }
            for g in Group::ALL {
                let files: Vec<usize> = st.changes.iter().enumerate().filter(|(_, c)| g.holds(c)).map(|(ci, _)| ci).collect();
                if files.is_empty() {
                    continue;
                }
                out.push(Line::Group(i, g));
                if self.sections.is_open(g.id(), true) {
                    out.extend(files.into_iter().map(|ci| Line::File(i, g, ci)));
                }
            }
        }
        out
    }

    /// Keyboard focus on the repository list (where the arrow keys pick a repository).
    pub fn focus_repositories(&self, window: &mut gpui::Window, _cx: &mut Context<Self>) {
        window.focus(&self.repo_focus);
    }

    fn toggle_repo(&mut self, key: RepoKey, cx: &mut Context<Self>) {
        if !self.collapsed_repos.remove(&key) {
            self.collapsed_repos.insert(key);
        }
        cx.notify();
    }

    /// Select the repository `delta` rows away from the selected one (keyboard), keeping focus on the list.
    fn step_repo(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.repos.is_empty() {
            return;
        }
        let current = self.selected_repo_key().and_then(|k| self.repos.iter().position(|r| r.id.key == k));
        let last = self.repos.len() as isize - 1;
        let next = match (current, delta) {
            (_, isize::MIN) => 0,
            (_, isize::MAX) => last,
            (Some(i), d) => (i as isize + d).clamp(0, last),
            (None, _) => 0,
        } as usize;
        let key = self.repos[next].id.key;
        if let Some(ix) = self.lines().iter().position(|l| *l == Line::Repo(next)) {
            self.repo_scroll.scroll_to_item(ix, ScrollStrategy::Center);
        }
        if Some(key) != self.selected_repo_key() {
            cx.emit(ScmEvent::SelectRepository(key));
        }
        cx.notify();
    }

    pub(super) fn repo_list(&self, focused: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = *palette(cx);
        let lines = Arc::new(self.lines());
        let n = lines.len();
        div()
            .id("scm-repos")
            .debug_selector(|| "scm-repos".into())
            .track_focus(&self.repo_focus)
            .tab_index(0)
            .key_context("ScmRepositories")
            .flex_1()
            .min_h(px(layout::ROW_H * 3.))
            .border_1()
            .border_color(if focused { c(p.focus_ring) } else { ui::ca(p.focus_ring, 0.) })
            .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, _, cx| {
                let m = ev.keystroke.modifiers;
                if m.control || m.alt || m.platform || m.shift || m.function {
                    return;
                }
                match ev.keystroke.key.as_str() {
                    "up" => this.step_repo(-1, cx),
                    "down" => this.step_repo(1, cx),
                    "home" => this.step_repo(isize::MIN, cx),
                    "end" => this.step_repo(isize::MAX, cx),
                    "left" | "right" | "enter" | "space" => {
                        let Some(key) = this.selected_repo_key() else { return };
                        let collapsed = this.collapsed_repos.contains(&key);
                        let want = match ev.keystroke.key.as_str() {
                            "left" => true,
                            "right" => false,
                            _ => !collapsed,
                        };
                        if want != collapsed {
                            this.toggle_repo(key, cx);
                        }
                    }
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(
                uniform_list(
                    "scm-repo-rows",
                    n,
                    cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                        range
                            .filter_map(|ix| {
                                let line = *lines.get(ix)?;
                                Some(this.line(ix, line, &p, cx))
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(self.repo_scroll.clone())
                .size_full(),
            )
    }

    fn line(&self, ix: usize, line: Line, p: &ui::Palette, cx: &mut Context<Self>) -> gpui::AnyElement {
        let (Line::Repo(i) | Line::Group(i, _) | Line::File(i, _, _) | Line::Note(i)) = line;
        // Rows are built from the list computed in the same frame; a missing entry renders as an empty row.
        let Some(repo) = self.repos.get(i).cloned() else { return div().h(px(layout::ROW_H)).into_any_element() };
        let repo = &repo;
        match line {
            Line::Repo(i) => self.repo_row(i, repo, p, cx).into_any_element(),
            Line::Group(_, g) => {
                let n = repo.status.as_ref().map_or(0, |st| st.changes.iter().filter(|c| g.holds(c)).count());
                div().pl(px(theme::SPACE[4])).child(self.group_header(g.id(), g.label(), n, g.staged(), Some(repo.id.key), cx)).into_any_element()
            }
            Line::File(_, g, ci) => match repo.status.as_ref().and_then(|st| st.changes.get(ci)) {
                Some(change) => div().pl(px(theme::SPACE[4])).child(self.row(ix, change, g.staged(), Some(repo.id.key), cx)).into_any_element(),
                None => div().h(px(layout::ROW_H)).into_any_element(),
            },
            Line::Note(_) => {
                let (text, color) = match (&repo.status, &repo.error) {
                    (None, Some(e)) => (e.lines().next().unwrap_or(e).to_string(), p.danger),
                    (None, None) => (t("Loading…").to_string(), p.text_muted),
                    (Some(_), _) => (t("No changes.").to_string(), p.text_muted),
                };
                div()
                    .id(("scm-repo-note", ix))
                    .h(px(layout::ROW_H))
                    .flex()
                    .items_center()
                    .pl(px(theme::SPACE[8]))
                    .pr(px(theme::SPACE[4]))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_size(px(text::XS.0))
                    .text_color(c(color))
                    .child(text)
                    .into_any_element()
            }
        }
    }

    fn repo_row(&self, i: usize, repo: &Repo, p: &ui::Palette, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = *p;
        let key = repo.id.key;
        let selected = self.selected_repo_key() == Some(key);
        let collapsed = self.collapsed_repos.contains(&key);
        let busy = self.busy.contains(&repo.id.worktree);
        let result = self.repo_results.get(&key).cloned();
        let branch = match &repo.status {
            Some(st) => {
                let name = st.branch.clone().unwrap_or_else(|| t("detached HEAD").to_string());
                if st.ahead > 0 || st.behind > 0 { format!("{name}  {}", git::counts_label(st.ahead, st.behind)) } else { name }
            }
            None => String::new(),
        };
        let conflicts = repo.status.as_ref().map_or(0, |st| st.changes.iter().filter(|c| c.unstaged == Some(Mark::Conflict)).count());
        let tip: SharedString = [repo.id.worktree.clone(), summary(repo), result.as_ref().map(|(r, _)| r.to_string()).unwrap_or_default()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
            .into();
        div()
            .id(("scm-repo", key as usize))
            .debug_selector(move || format!("scm-repo-row-{i}"))
            .w_full()
            .h(px(layout::ROW_H))
            .flex()
            .items_center()
            .gap(px(theme::SPACE[2]))
            .px(px(theme::SPACE[2]))
            .whitespace_nowrap()
            .overflow_hidden()
            .cursor_pointer()
            .text_size(px(text::XS.0))
            .when(selected, |d| d.bg(c(p.surface_active)))
            .when(!selected, |d| d.hover(|d| d.bg(c(p.surface_hover))))
            .child(
                div()
                    .id(("scm-repo-fold", key as usize))
                    .flex_none()
                    .child(icon(if collapsed { "chevron-right" } else { "chevron-down" }, 14., p.text_muted))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_repo(key, cx);
                    })),
            )
            .child(div().flex_none().max_w(px(160.)).overflow_hidden().font_weight(gpui::FontWeight::SEMIBOLD).text_color(c(p.text)).child(repo.label.clone()))
            .child(icon("git-branch", 12., p.text_muted))
            .child(div().flex_1().min_w_0().overflow_hidden().text_color(c(p.text_muted)).child(branch))
            .children(repo.op.label().map(|op| div().flex_none().text_color(c(p.warning)).child(op.to_string())))
            .when(conflicts > 0, |d| d.child(div().flex_none().text_color(c(p.danger)).child(format!("! {conflicts}"))))
            .when(repo.error.is_some() || result.as_ref().is_some_and(|(_, e)| *e), |d| d.child(icon("alert", 12., p.danger)))
            .when(busy, |d| d.child(ui::spinner(("scm-repo-busy", key as usize), 12., p.text_muted)))
            .when(repo.changes() > 0, |d| {
                d.child(
                    div()
                        .flex_none()
                        .px(px(theme::SPACE[2]))
                        .rounded(px(theme::radius::LG))
                        .bg(c(p.surface_active))
                        .text_color(c(p.text))
                        .child(repo.changes().to_string()),
                )
            })
            .when(!self.menu_open, |d| d.tooltip(move |_, cx| cx.new(|_| crate::workbench::Tooltip(tip.clone())).into()))
            .on_click(cx.listener(move |this, _, window, cx| {
                window.focus(&this.repo_focus);
                if this.selected_repo_key() != Some(key) {
                    cx.emit(ScmEvent::SelectRepository(key));
                }
            }))
    }
}

#[cfg(test)]
#[path = "scm_repo_tests.rs"]
mod tests;
