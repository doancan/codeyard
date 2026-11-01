//! What assistive technology reads of Source Control's history: the Sync / Publish button, the Worktrees section, the
//! Graph section (its buttons, filters, search field and a bounded range of rows around the selected commit or the rows
//! on screen) and the selected commit's details. Actions go through the panel's own operations, each checked against
//! what the tree showed (the same commit hash, worktree path, filter or button).

use super::*;
use crate::graph::GraphItem;

/// Graph rows exposed at most (day headings and commits); the count says how many there are.
pub(crate) const HISTORY_ROWS: usize = 500;

/// A part of the history sections with a painted frame.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum HistoryFrame {
    /// The Sync Changes / Publish Branch button.
    Sync,
    /// A section ("graph", "worktrees", "details"), its header and its list.
    Section(&'static str),
    Header(&'static str),
    List(&'static str),
    /// A header button: "search", "fetch", "pull", "push", "refresh", "more", "worktree-add", "worktree-refresh",
    /// "details-close".
    Button(&'static str),
    /// A filter chip: 0 current branch, 1 mine, 2 no merges.
    Filter(u8),
    Query,
    /// A graph row: a commit's hash, or "day:" and its heading.
    Row(String),
    /// A worktree row, by its path.
    Worktree(String),
    /// A file of the selected commit, by its place.
    DetailFile(usize),
}

/// A row of the graph: a day heading or a commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AccessGraphRow {
    Day(String, usize),
    Commit { hash: String, subject: String, detail: String, selected: bool },
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(crate) struct AccessGraph {
    pub open: bool,
    pub count: Option<usize>,
    pub has_remote: bool,
    pub ahead: u32,
    pub behind: u32,
    /// History loading or a fetch, pull or push running.
    pub busy: bool,
    pub search_open: bool,
    pub query: String,
    pub query_focused: bool,
    /// Active filters (0 current branch, 1 mine, 2 no merges) and their labels.
    pub filters: Vec<(u8, String)>,
    pub rows: Vec<AccessGraphRow>,
    /// The first exposed row and the number of rows.
    pub first: usize,
    pub total: usize,
    /// Why there are no rows, and whether that is an error.
    pub note: Option<(String, bool)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AccessWorktree {
    pub path: String,
    pub title: String,
    /// Path, current / unavailable / locked state.
    pub detail: String,
    pub current: bool,
    pub available: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(crate) struct AccessWorktrees {
    pub open: bool,
    pub rows: Vec<AccessWorktree>,
    /// Loading, none found, or the error (`true`).
    pub note: Option<(String, bool)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AccessDetail {
    pub hash: String,
    pub title: String,
    pub open: bool,
    /// The message, then author, date and hash.
    pub lines: Vec<String>,
    /// (path, change) of each file.
    pub files: Vec<(String, String)>,
    pub note: Option<(String, bool)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(crate) struct AccessHistory {
    /// The Sync Changes / Publish Branch button's label.
    pub sync: Option<String>,
    pub syncing: bool,
    pub worktrees: AccessWorktrees,
    pub graph: AccessGraph,
    pub detail: Option<AccessDetail>,
}

fn change_name(status: char) -> String {
    match status {
        'A' => crate::i18n::t("Added"),
        'D' => crate::i18n::t("Deleted"),
        'R' => crate::i18n::t("Renamed"),
        'C' => crate::i18n::t("Copied"),
        _ => crate::i18n::t("Modified"),
    }
    .to_string()
}

impl ScmPanel {
    pub(crate) fn a11y_history(&self, window: &Window, cx: &App) -> AccessHistory {
        let sync = self.status.as_ref().and_then(|st| self.sync_action(st)).map(|(label, ..)| label.to_string());
        let open = self.sections.is_open("graph", true);
        let (ahead, behind) = self.status.as_ref().map_or((0, 0), |s| (s.ahead, s.behind));
        let mut filters = Vec::new();
        if self.scope == graph::Scope::Current {
            filters.push((0, crate::i18n::t("Current branch").to_string()));
        }
        if self.only_mine {
            filters.push((1, crate::i18n::t("Mine").to_string()));
        }
        if self.hide_merges {
            filters.push((2, crate::i18n::t("No merges").to_string()));
        }
        let mut graph = AccessGraph {
            open,
            count: self.graph.as_ref().map(|g| g.commits.len()),
            has_remote: self.has_remote,
            ahead,
            behind,
            busy: self.graph_loading || self.syncing,
            search_open: self.search_open,
            query: self.query.read(cx).text().to_string(),
            query_focused: self.query.focus_handle(cx).is_focused(window),
            filters,
            ..Default::default()
        };
        if open {
            match self.graph_items(cx) {
                Err((why, error)) => graph.note = Some((why.to_string(), error)),
                Ok((g, items, _)) => {
                    let selected = self.selected.as_ref().map(|c| c.hash.as_str());
                    let at = items.iter().position(|i| matches!(i, GraphItem::Commit(ix) if Some(g.commits[*ix].hash.as_str()) == selected));
                    let first = crate::workbench::a11y::window_start(items.len(), HISTORY_ROWS, at, self.visible_graph_rows(items.len()));
                    graph.first = first;
                    graph.total = items.len();
                    graph.rows = items
                        .iter()
                        .skip(first)
                        .take(HISTORY_ROWS)
                        .map(|item| match item {
                            GraphItem::Day(label, n) => AccessGraphRow::Day(label.clone(), *n),
                            GraphItem::Commit(ix) => {
                                let k = &g.commits[*ix];
                                let mut parts = vec![k.author.clone(), k.when.clone(), k.short.clone()];
                                parts.extend(k.refs.iter().map(|r| r.name.clone()));
                                if k.parents.len() > 1 {
                                    parts.push(crate::i18n::t("merge").to_string());
                                }
                                AccessGraphRow::Commit {
                                    hash: k.hash.clone(),
                                    subject: k.subject.clone(),
                                    detail: parts.join(", "),
                                    selected: Some(k.hash.as_str()) == selected,
                                }
                            }
                        })
                        .collect();
                }
            }
        }
        AccessHistory { sync, syncing: self.syncing, worktrees: self.a11y_worktrees(), graph, detail: self.a11y_detail() }
    }

    /// The first and last graph rows on screen, from the list's scroll position.
    fn visible_graph_rows(&self, total: usize) -> Option<(usize, usize)> {
        let state = self.graph_scroll.0.borrow();
        let size = state.last_item_size?;
        let row = f32::from(size.item.height) / total.max(1) as f32;
        if row <= 0. {
            return None;
        }
        let first = (-f32::from(state.base_handle.offset().y) / row).max(0.) as usize;
        let last = (first + (f32::from(size.item.height).min(f32::from(size.contents.height)) / row).ceil() as usize).min(total.saturating_sub(1));
        Some((first.min(last), last))
    }

    fn a11y_detail(&self) -> Option<AccessDetail> {
        let commit = self.selected.as_ref()?;
        let message = self.detail.as_ref().map_or(commit.subject.clone(), |d| d.message.clone());
        let mut lines: Vec<String> = message.lines().map(str::to_string).filter(|l| !l.trim().is_empty()).collect();
        lines.push(format!("{} <{}>, {}", commit.author, commit.email, commit.date));
        lines.push(commit.hash.clone());
        let files = self.detail.as_ref().map_or_else(Vec::new, |d| d.files.iter().map(|f| (f.path.clone(), change_name(f.status))).collect());
        let note = match (&self.detail, &self.detail_error) {
            (_, Some(e)) => Some((crate::i18n::tf("Could not read the commit: {}", &[&e]), true)),
            (None, None) => Some((crate::i18n::t("Loading…").to_string(), false)),
            _ => None,
        };
        Some(AccessDetail {
            hash: commit.hash.clone(),
            title: crate::i18n::tf("Commit {}", &[&commit.short]),
            open: self.sections.is_open("details", true),
            lines,
            files,
            note,
        })
    }

    /// A section header's press: the Graph, Worktrees or details section opens or closes, as a click on it does.
    pub(crate) fn a11y_toggle_history_section(&mut self, id: &'static str, cx: &mut Context<Self>) {
        match id {
            "worktrees" => self.a11y_toggle_worktrees(cx),
            "graph" | "details" => self.toggle_section(id, cx),
            _ => {}
        }
    }

    /// A Graph header button pressed from the tree (fetch, pull and push are the workbench's actions).
    pub(crate) fn a11y_graph_button(&mut self, which: &str, window: &mut Window, cx: &mut Context<Self>) {
        match which {
            "search" => self.apply_graph_option(GraphOption::Search, window, cx),
            "refresh" if !self.graph_loading && !self.syncing => self.load_graph(cx),
            // The "…" menu opens under its button, as a click there opens it.
            "more" => {
                let at = self.a11y_frames.borrow().get(&AccessFrame::History(HistoryFrame::Button("more"))).map(|b| b.bottom_left());
                cx.emit(ScmEvent::GraphMenu(at.unwrap_or_default()));
            }
            "details-close" => {
                self.selected = None;
                self.detail = None;
                cx.notify();
            }
            _ => {}
        }
    }

    /// A filter chip's press: the filter goes, as a click on the chip does.
    pub(crate) fn a11y_remove_filter(&mut self, which: u8, cx: &mut Context<Self>) {
        match which {
            0 => self.set_scope(graph::Scope::All, cx),
            1 => self.only_mine = false,
            2 => self.hide_merges = false,
            _ => {}
        }
        cx.notify();
    }

    pub(crate) fn a11y_set_graph_query(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.search_open {
            self.query.update(cx, |q, cx| q.set_text(text, cx));
            cx.notify();
        }
    }

    pub(crate) fn focus_graph_query(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_open {
            window.focus(&self.query.focus_handle(cx));
        }
    }

    /// The commit `hash` of the graph on screen, if it is still listed.
    fn listed_commit(&self, hash: &str, cx: &App) -> Option<GraphCommit> {
        let (g, items, _) = self.graph_items(cx).ok()?;
        items.iter().find_map(|i| match i {
            GraphItem::Commit(ix) if g.commits[*ix].hash == hash => Some(g.commits[*ix].clone()),
            _ => None,
        })
    }

    /// A commit row from the tree: `select` (a click), `open` (a double click opens its changes) or `menu` (the menu a
    /// right click opens, under the row). Nothing happens unless the commit is still in the graph.
    pub(crate) fn a11y_commit(&mut self, hash: &str, what: &str, cx: &mut Context<Self>) {
        let Some(commit) = self.listed_commit(hash, cx) else { return };
        match what {
            "select" => self.select_commit(commit, cx),
            "open" => {
                self.select_commit(commit.clone(), cx);
                cx.emit(ScmEvent::OpenCommit(commit));
            }
            "menu" => {
                let at = self.a11y_frames.borrow().get(&AccessFrame::History(HistoryFrame::Row(hash.to_string()))).map(|b| b.bottom_left());
                self.select_commit(commit.clone(), cx);
                cx.emit(ScmEvent::CommitMenu(commit, at.unwrap_or_default()));
            }
            _ => {}
        }
    }

    /// A file of the selected commit opens its diff, while the same commit lists the same file there.
    pub(crate) fn a11y_open_detail_file(&mut self, hash: &str, index: usize, path: &str, cx: &mut Context<Self>) {
        let (Some(commit), Some(detail)) = (self.selected.clone(), self.detail.clone()) else { return };
        if commit.hash != hash || detail.hash != hash {
            return;
        }
        if let Some(file) = detail.files.get(index).filter(|f| f.path == path) {
            cx.emit(ScmEvent::OpenCommitFile(commit, file.clone()));
        }
    }

    /// The Sync Changes or Publish Branch button, while it still says `label`.
    pub(crate) fn a11y_sync(&mut self, label: &str, cx: &mut Context<Self>) {
        if self.syncing {
            return;
        }
        if let Some((now, _, event)) = self.status.as_ref().and_then(|st| self.sync_action(st))
            && now.as_ref() == label
        {
            cx.emit(event());
        }
    }

    /// The header's Refresh: what a click on it does.
    pub(super) fn a11y_refresh(&mut self, cx: &mut Context<Self>) {
        // A remote added since the graph was read shows up here (HEAD did not move, so nothing else reloads it).
        self.load_graph(cx);
        self.refresh_worktrees(cx);
        cx.emit(ScmEvent::Refresh);
        // Repositories created or removed since the last search (searched again only after a while).
        cx.emit(ScmEvent::RefreshInventory);
        cx.emit(ScmEvent::Ext(ExtEvent::Refresh));
    }
}
