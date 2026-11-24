//! Source Control's GRAPH section: view modes (graph, compact, by day), filters ("mine", "hide
//! merges", search), rows and the card that opens when hovering a commit. Submodule of `scm.rs`: reads the panel's private fields.

use gpui::{AnyElement, Context, Focusable as _, Render, SharedString, Window, div, prelude::*, px, uniform_list};

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use host::Host;

use super::{AccessFrame, GraphData, HistoryFrame, ScmEvent, ScmPanel, avatar, commit_matches};
use crate::graph::{self, CommitDetail, GraphCommit, GraphItem, GraphView, RefKind, RefLabel};
use crate::i18n::{t, tf, tn};
use crate::sections;
use crate::ui::{self, Palette, c, icon, palette, text};
use crate::{git, github};

/// Branch / tag chip: colors from `graph::ref_chip_colors` (AA in every theme).
fn ref_chip(r: &RefLabel, p: &Palette) -> gpui::Div {
    let (fg, bg) = graph::ref_chip_colors(r.kind, p);
    let glyph = if r.kind == RefKind::Tag { "bookmark" } else { "git-branch" };
    div()
        .flex_none()
        .max_w(px(110.))
        .flex()
        .items_center()
        .gap(px(3.))
        .px(px(theme::SPACE[2]))
        .h(px(14.))
        .rounded(px(7.))
        .text_size(px(text::BADGE.0))
        .text_color(c(fg))
        .bg(c(bg))
        .child(icon(glyph, 9., fg))
        .child(div().overflow_hidden().whitespace_nowrap().child(r.name.clone()))
}

/// Labels in order of importance (HEAD, local, tag, remote); a remote branch with the same name as a local one is not repeated next to it.
fn ordered_refs(commit: &GraphCommit) -> Vec<&RefLabel> {
    let mut refs: Vec<&RefLabel> = commit.refs.iter().collect();
    refs.sort_by_key(|r| match r.kind {
        RefKind::Head => 0,
        RefKind::Local => 1,
        RefKind::Tag => 2,
        RefKind::Remote => 3,
    });
    let local: Vec<&str> = refs.iter().filter(|r| matches!(r.kind, RefKind::Head | RefKind::Local)).map(|r| r.name.as_str()).collect();
    refs.retain(|r| r.kind != RefKind::Remote || !local.contains(&r.name.split_once('/').map_or(r.name.as_str(), |(_, n)| n)));
    refs
}

/// Most lines of the message body a hover card shows, and the longest of those lines in characters.
const CARD_MESSAGE_LINES: usize = 3;
const CARD_LINE_CHARS: usize = 160;
/// Details kept for hovered commits; the oldest are dropped when it is full (a commit's changes never change, so nothing else
/// invalidates them).
const CARD_CACHE: usize = 64;

/// What the card needs to read a commit's changes: where the repository is and what was already read. Hovering one commit again
/// does not run git again, and nothing is read for a row the pointer only passes over: the card is built when the tooltip shows.
#[derive(Clone)]
pub(super) struct CardSource {
    host: Arc<dyn Host>,
    root: String,
    cache: CardCache,
}

/// What the cards have read so far: each commit's changes, and per repository the GitHub repo its remote points at (`None`:
/// it has no GitHub remote), so the links are worked out once and not on every hover.
#[derive(Clone, Default)]
pub(super) struct CardCache {
    details: Rc<RefCell<HashMap<String, Result<Arc<CommitDetail>, ()>>>>,
    repos: Rc<RefCell<HashMap<String, Option<github::Repo>>>>,
}

impl CardSource {
    pub(super) fn new(host: Arc<dyn Host>, root: String, cache: CardCache) -> Self {
        Self { host, root, cache }
    }

    fn key(&self, hash: &str) -> String {
        format!("{}\n{hash}", self.root)
    }
}

/// What the card knows of the commit's changes.
#[derive(Clone)]
pub(super) enum Changes {
    Loading,
    Ready(Arc<CommitDetail>),
    Failed,
}

/// On commit hover: author and time, subject, the start of the message, all branches/tags, hash, parents of a merge, how much the
/// commit changed (files, added and removed lines, never the file list) and links to the commit and its pull request on
/// GitHub; a short card instead of a wall of text. The card stays open while the pointer is on it, so the links can be clicked.
pub struct CommitCard {
    commit: GraphCommit,
    pub(super) changes: Changes,
    /// The GitHub repository of the remote, once it is known.
    repo: Option<github::Repo>,
    _reads: Vec<gpui::Task<()>>,
}

impl CommitCard {
    pub(super) fn new(commit: GraphCommit, source: Option<CardSource>, cx: &mut Context<Self>) -> Self {
        let Some(source) = source else { return Self { commit, changes: Changes::Failed, repo: None, _reads: Vec::new() } };
        let key = source.key(&commit.hash);
        let known_repo = source.cache.repos.borrow().get(&source.root).cloned();
        let mut card = Self { commit, changes: Changes::Loading, repo: known_repo.clone().flatten(), _reads: Vec::new() };
        if known_repo.is_none() {
            card.read_repo(&source, cx);
        }
        if let Some(known) = source.cache.details.borrow().get(&key).cloned() {
            card.changes = known.map_or(Changes::Failed, Changes::Ready);
            return card;
        }
        let hash = card.commit.hash.clone();
        let (host, root) = (source.host.clone(), source.root.clone());
        let task = cx.background_executor().spawn(async move { graph::commit_detail(host.as_ref(), &root, &hash) });
        let read = cx.spawn(async move |this, cx| {
            let result = task.await.map(Arc::new).map_err(|_| ());
            let mut cache = source.cache.details.borrow_mut();
            if cache.len() >= CARD_CACHE {
                cache.clear();
            }
            cache.insert(key, result.clone());
            drop(cache);
            this.update(cx, |this, cx| {
                this.changes = result.map_or(Changes::Failed, Changes::Ready);
                cx.notify();
            })
            .ok();
        });
        card._reads.push(read);
        card
    }

    /// Finds the GitHub repository behind the `origin` remote (with `insteadOf` applied) off the UI thread.
    fn read_repo(&mut self, source: &CardSource, cx: &mut Context<Self>) {
        let (host, root, cache) = (source.host.clone(), source.root.clone(), source.cache.repos.clone());
        let task = cx.background_executor().spawn({
            let root = root.clone();
            async move { git::resolved_remote_url(host.as_ref(), &root, "origin").ok().and_then(|url| github::parse_remote(&url)) }
        });
        let read = cx.spawn(async move |this, cx| {
            let repo = task.await;
            cache.borrow_mut().insert(root, repo.clone());
            this.update(cx, |this, cx| {
                this.repo = repo;
                cx.notify();
            })
            .ok();
        });
        self._reads.push(read);
    }
}

/// The number of the pull request a commit subject names: `Merge pull request #12 from …` or a squash merge ending in `(#12)`.
fn pull_number(subject: &str) -> Option<u64> {
    if let Some(rest) = subject.strip_prefix("Merge pull request #") {
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        return digits.parse().ok();
    }
    let rest = subject.trim_end().strip_suffix(')')?;
    let (_, number) = rest.rsplit_once("(#")?;
    number.chars().all(|c| c.is_ascii_digit()).then(|| number.parse().ok()).flatten()
}

/// The rest of the message under its subject line, cut to what a card can hold.
fn message_body(message: &str) -> Vec<String> {
    let mut lines: Vec<String> =
        message.lines().skip(1).map(str::trim_end).skip_while(|l| l.is_empty()).map(|l| l.chars().take(CARD_LINE_CHARS).collect()).collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    if lines.len() > CARD_MESSAGE_LINES {
        lines.truncate(CARD_MESSAGE_LINES);
        lines.push("…".into());
    }
    lines
}

impl CommitCard {
    /// How much the commit changed: the number of files and the lines added and removed. The files themselves are in the
    /// commit's details; a card that listed them grew as tall as the commit was big.
    fn changes_section(&self, p: &Palette) -> gpui::Div {
        let muted = |t: String| div().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(t);
        let section = div().flex().flex_col().gap(px(theme::SPACE[2]));
        let detail = match &self.changes {
            Changes::Loading => return section.child(muted(t("Reading the changes…").into())),
            Changes::Failed => return section.child(muted(t("The changes could not be read.").into())),
            Changes::Ready(detail) => detail,
        };
        if detail.files.is_empty() {
            return section.child(muted(t("No files changed").into()));
        }
        let (added, deleted) = detail.totals();
        let danger = ui::ensure_contrast(p.danger, p.text, &[p.surface_raised], 4.6);
        let success = ui::ensure_contrast(p.success, p.text, &[p.surface_raised], 4.6);
        section.child(
            div()
                .debug_selector(|| "commit-card-summary".to_string())
                .flex()
                .items_center()
                .gap(px(theme::SPACE[3]))
                .text_size(px(text::XS.0))
                .child(icon("files", 12., p.text_muted))
                .child(div().text_color(c(p.text_muted)).child(tn("{} file changed", "{} files changed", detail.files.len(), &[])))
                .child(div().text_color(c(success)).child(format!("+{added}")))
                .child(div().text_color(c(danger)).child(format!("−{deleted}"))),
        )
    }

    /// Links to the commit and, when its subject names one, the pull request, on GitHub.
    fn links(&self, repo: &github::Repo, p: &Palette) -> gpui::Div {
        let link = |id: &'static str, label: String, url: String| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(theme::SPACE[2]))
                .h(px(22.))
                .px(px(theme::SPACE[3]))
                .rounded(px(theme::radius::MD))
                .cursor_pointer()
                .text_size(px(text::XS.0))
                .text_color(c(ui::ensure_contrast(p.accent, p.text, &[p.surface_raised], 4.6)))
                .hover(|d| d.bg(c(p.surface_hover)))
                .child(icon("external-link", 12., p.accent))
                .child(label)
                .on_click(move |_, _, cx| cx.open_url(&url))
        };
        div()
            .flex()
            .flex_wrap()
            .gap(px(theme::SPACE[2]))
            .child(link("commit-card-commit-link", t("Commit on GitHub").to_string(), repo.commit_url(&self.commit.hash)))
            .children(pull_number(&self.commit.subject).map(|n| link("commit-card-pull-link", tf("Pull request #{}", &[&n]), repo.pull_url(n))))
    }
}

impl Render for CommitCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        let k = &self.commit;
        let refs = ordered_refs(k);
        let body = match &self.changes {
            Changes::Ready(detail) => message_body(&detail.message),
            _ => Vec::new(),
        };
        let muted = |t: String| div().text_size(px(text::XXS.0)).text_color(c(p.text_muted)).child(t);
        div()
            .w(px(320.))
            .flex()
            .flex_col()
            .gap(px(theme::SPACE[4]))
            .p(px(theme::SPACE[5]))
            .bg(c(p.surface_raised))
            .border_1()
            .border_color(c(p.border))
            .rounded(px(theme::radius::LG))
            .shadow_md()
            .text_size(px(text::SM.0))
            .text_color(c(p.text))
            .child(
                div().flex().items_center().gap(px(theme::SPACE[4])).child(avatar(&k.author, 22., &p)).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().font_weight(gpui::FontWeight::SEMIBOLD).overflow_hidden().whitespace_nowrap().child(k.author.clone()))
                        .child(muted(format!("{} · {}", k.date, crate::git::short_ago(&k.when)))),
                ),
            )
            .when(k.incoming, |d| {
                let warning = ui::ensure_contrast(p.warning, p.text, &[p.surface_raised], 4.6);
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(theme::SPACE[3]))
                        .text_size(px(text::XS.0))
                        .text_color(c(warning))
                        .child(icon("arrow-down", 12., warning))
                        .child(crate::i18n::t("Not pulled yet. Sync brings it in.")),
                )
            })
            .child(div().font_weight(gpui::FontWeight::MEDIUM).child(k.subject.clone()))
            .children((!body.is_empty()).then(|| {
                div()
                    .flex()
                    .flex_col()
                    .text_size(px(text::XS.0))
                    .text_color(c(p.text_muted))
                    .children(body.iter().map(|l| div().whitespace_normal().child(if l.is_empty() { " ".to_string() } else { l.clone() })))
            }))
            .when(!refs.is_empty(), |d| d.child(div().flex().flex_wrap().gap(px(theme::SPACE[2])).children(refs.iter().map(|r| ref_chip(r, &p)))))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[4]))
                    .child(icon("git-commit", 12., p.text_muted))
                    .child(
                        div()
                            .font_family(ui::MONO_FONT)
                            .text_size(px(text::XS.0))
                            .text_color(c(ui::ensure_contrast(ui::mix(p.text_muted, p.accent, 0.6), p.text, &[p.surface_raised], 4.6)))
                            .child(k.short.clone()),
                    )
                    .when(k.parents.len() > 1, |d| {
                        d.child(
                            div()
                                .px(px(theme::SPACE[3]))
                                .h(px(14.))
                                .flex()
                                .items_center()
                                .rounded(px(7.))
                                .bg(c(p.surface_active))
                                .text_size(px(text::BADGE.0))
                                .child(crate::i18n::t("Merge")),
                        )
                    })
                    .child(div().flex_1())
                    .child(muted(k.email.clone())),
            )
            .when(k.parents.len() > 1, |d| {
                let parents = k.parents.iter().map(|h| h.chars().take(7).collect::<String>()).collect::<Vec<_>>().join(" · ");
                d.child(muted(tf("Parents: {}", &[&parents])))
            })
            .child(self.changes_section(&p))
            .children(self.repo.as_ref().map(|repo| self.links(repo, &p)))
            .child(
                div()
                    .pt(px(theme::SPACE[2]))
                    .border_t_1()
                    .border_color(c(p.border))
                    .child(muted(crate::i18n::t("Click: details · Double-click: open · Right-click: actions").into())),
            )
    }
}

/// Active filter shown under the GRAPH header; clicking it turns the filter off.
fn filter_chip(id: &'static str, label: &'static str, p: &Palette) -> gpui::Stateful<gpui::Div> {
    let bg = ui::mix(p.surface, p.accent, 0.18);
    let fg = ui::ensure_contrast(p.accent, p.text, &[bg], 4.6);
    div()
        .id(id)
        .h(px(18.))
        .pl(px(theme::SPACE[3]))
        .pr(px(theme::SPACE[2]))
        .flex()
        .items_center()
        .gap(px(theme::SPACE[1]))
        .rounded(px(9.))
        .bg(c(bg))
        .text_size(px(text::XXS.0))
        .text_color(c(fg))
        .cursor_pointer()
        .hover(|s| s.bg(c(ui::mix(p.surface, p.accent, 0.28))))
        .tooltip(|_, cx| cx.new(|_| crate::workbench::Tooltip(crate::i18n::t("Click to remove this filter").into())).into())
        .child(label)
        .child(icon("x", 10., fg))
}

/// Header button with a counter next to the icon (commits to pull / to push).
fn count_button(id: &'static str, glyph: &'static str, n: u32, tip: &'static str, p: &Palette) -> gpui::Stateful<gpui::Div> {
    ui::icon_button(id, glyph, tip, p).when(n > 0, |d| {
        d.w_auto().px(px(theme::SPACE[1])).gap(px(1.)).child(div().text_size(px(text::BADGE.0)).font_weight(gpui::FontWeight::NORMAL).child(n.to_string()))
    })
}

/// Toggles of the GRAPH section's "…" menu (the workbench builds the menu, the panel applies the choice).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphOption {
    View(GraphView),
    AllBranches,
    OnlyMine,
    HideMerges,
    Search,
    Reload,
}

/// What the "…" menu needs to draw its checks.
pub struct GraphMenuState {
    pub view: GraphView,
    pub all_branches: bool,
    pub only_mine: bool,
    pub hide_merges: bool,
    pub search_open: bool,
    /// "Only my commits" needs a git identity to compare against.
    pub has_identity: bool,
    pub has_remote: bool,
}

impl ScmPanel {
    pub(super) fn set_graph_view(&mut self, view: GraphView, cx: &mut Context<Self>) {
        self.graph_view = Some(view);
        cx.notify();
    }

    fn current_graph_view(&self, cx: &gpui::App) -> GraphView {
        self.graph_view.unwrap_or_else(|| GraphView::from_id(&cx.global::<crate::settings::Settings>().graph_view).unwrap_or_default())
    }

    pub fn graph_menu_state(&self, cx: &gpui::App) -> GraphMenuState {
        GraphMenuState {
            view: self.current_graph_view(cx),
            all_branches: self.scope == graph::Scope::All,
            only_mine: self.only_mine,
            hide_merges: self.hide_merges,
            search_open: self.search_open,
            has_identity: self.identity.as_ref().is_some_and(|(n, e)| n.is_some() || e.is_some()),
            has_remote: self.has_remote,
        }
    }

    pub fn apply_graph_option(&mut self, option: GraphOption, window: &mut Window, cx: &mut Context<Self>) {
        self.sections.set_open("graph", true);
        match option {
            GraphOption::View(v) => self.set_graph_view(v, cx),
            GraphOption::AllBranches => {
                let scope = if self.scope == graph::Scope::All { graph::Scope::Current } else { graph::Scope::All };
                self.set_scope(scope, cx);
            }
            GraphOption::OnlyMine => self.only_mine = !self.only_mine,
            GraphOption::HideMerges => self.hide_merges = !self.hide_merges,
            GraphOption::Search => self.toggle_graph_search(window, cx),
            GraphOption::Reload => self.load_graph(cx),
        }
        cx.notify();
    }

    fn toggle_graph_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_open = !self.search_open;
        self.sections.set_open("graph", true);
        if self.search_open {
            window.focus(&self.query.focus_handle(cx));
        } else {
            self.query.update(cx, |q, cx| q.set_text("", cx));
        }
        cx.notify();
    }

    /// The graph's rows as the section draws them (filters, search and view mode applied), the history they index and
    /// whether lanes are left out; or why there are none (and whether that is an error).
    pub(super) fn graph_items(&self, cx: &gpui::App) -> Result<(Arc<GraphData>, Vec<GraphItem>, bool), (SharedString, bool)> {
        let Some(g) = self.graph.clone() else {
            return Err(match &self.graph_error {
                Some(e) => (crate::i18n::tf("Could not read the history: {}", &[&e]).into(), true),
                None => (crate::i18n::t("Loading…").into(), false),
            });
        };
        if g.commits.is_empty() {
            return Err((crate::i18n::t("No commits yet.").into(), false));
        }
        let query = self.query.read(cx).text().to_string();
        let searching = !query.trim().is_empty();
        let (only_mine, hide_merges, mode) = (self.only_mine, self.hide_merges, self.current_graph_view(cx));
        let mine = |k: &GraphCommit| {
            let Some((name, email)) = &self.identity else { return false };
            email.as_deref().is_some_and(|e| !e.is_empty() && e.eq_ignore_ascii_case(&k.email))
                || name.as_deref().is_some_and(|n| !n.is_empty() && n == k.author)
        };
        let keep = |k: &GraphCommit| (!searching || commit_matches(k, &query)) && (!only_mine || mine(k)) && (!hide_merges || k.parents.len() < 2);
        // Lanes only make sense in the unfiltered graph view (lines break when rows are skipped).
        let flat = mode != GraphView::Graph || searching || only_mine || hide_merges;
        let items = graph::build_items(&g.commits, keep, mode);
        if items.is_empty() {
            let why = if searching {
                crate::i18n::t("No commit matches.")
            } else if only_mine {
                crate::i18n::t("None of these commits is yours.")
            } else {
                crate::i18n::t("No commits to show.")
            };
            return Err((why.into(), false));
        }
        Ok((g, items, flat))
    }

    /// `fill`: nothing else needs the room, so the graph grows to the bottom of the panel instead of its fixed height.
    pub(super) fn graph_section(&self, query_focused: bool, fill: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = *palette(cx);
        let open = self.sections.is_open("graph", true);
        let count = self.graph.as_ref().map(|g| g.commits.len());
        let scope = self.scope;
        let mode = self.current_graph_view(cx);
        let (only_mine, hide_merges) = (self.only_mine, self.hide_merges);
        let (ahead, behind) = self.status.as_ref().map_or((0, 0), |s| (s.ahead, s.behind));
        let has_remote = self.has_remote;
        let syncing = self.syncing;
        let dispatch = |action: Box<dyn gpui::Action>| {
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut gpui::App| {
                cx.stop_propagation();
                window.dispatch_action(action.boxed_clone(), cx);
            }
        };
        // The remote round-trip (fetch, pull, push) first, then refresh and the "…" menu.
        let probe = |part: HistoryFrame| crate::painted::probe(&self.a11y_frames, AccessFrame::History(part));
        let header = sections::header("scm-graph-header", crate::i18n::t("Graph"), count, open, &p)
            .relative()
            .child(probe(HistoryFrame::Header("graph")))
            .child(
                ui::icon_button("scm-graph-search", "search", crate::i18n::t("Search commits"), &p)
                    .relative()
                    .child(probe(HistoryFrame::Button("search")))
                    .when(self.search_open, |d| d.bg(c(p.surface_active)))
                    .on_click(cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.toggle_graph_search(window, cx);
                    })),
            )
            .when(has_remote, |d| {
                d.child(
                    ui::icon_button("scm-graph-fetch", "cloud-download", crate::i18n::t("Fetch from all remotes"), &p)
                        .relative()
                        .child(probe(HistoryFrame::Button("fetch")))
                        .on_click(dispatch(Box::new(crate::workbench::GitFetchAll))),
                )
                .child(
                    count_button("scm-graph-pull", "arrow-down", behind, "Pull", &p)
                        .relative()
                        .child(probe(HistoryFrame::Button("pull")))
                        .on_click(dispatch(Box::new(crate::workbench::GitPull))),
                )
                .child(
                    count_button("scm-graph-push", "arrow-up", ahead, "Push", &p)
                        .relative()
                        .child(probe(HistoryFrame::Button("push")))
                        .on_click(dispatch(Box::new(crate::workbench::GitPush))),
                )
            })
            // While history loads or a sync runs, the refresh button turns into the spinner (no extra icon squeezing the title).
            .child(if self.graph_loading || syncing {
                div()
                    .id("scm-graph-busy")
                    .relative()
                    .child(probe(HistoryFrame::Button("refresh")))
                    .size(px(22.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(ui::spinner("scm-graph-spin", 14., p.text_muted))
            } else {
                ui::icon_button("scm-graph-refresh", "refresh-cw", crate::i18n::t("Reload history"), &p)
                    .relative()
                    .child(probe(HistoryFrame::Button("refresh")))
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.load_graph(cx);
                    }))
            })
            .child(
                ui::icon_button("scm-graph-more", "ellipsis", crate::i18n::t("More Actions…"), &p)
                    .relative()
                    .child(probe(HistoryFrame::Button("more")))
                    .on_click(cx.listener(|_, ev: &gpui::ClickEvent, _, cx| {
                        cx.stop_propagation();
                        cx.emit(ScmEvent::GraphMenu(ev.position()));
                    })),
            )
            .on_click(cx.listener(|this, _, _, cx| this.toggle_section("graph", cx)));

        // Filters that hide commits stay visible as chips, so an emptier graph never looks like lost history.
        let filtered = scope == graph::Scope::Current || only_mine || hide_merges;
        let filters = div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap(px(theme::SPACE[2]))
            .px(px(theme::SPACE[4]))
            .py(px(theme::SPACE[2]))
            .when(scope == graph::Scope::Current, |d| {
                d.child(filter_chip("scm-graph-f-current", crate::i18n::t("Current branch"), &p).relative().child(probe(HistoryFrame::Filter(0))).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.set_scope(graph::Scope::All, cx);
                    }),
                ))
            })
            .when(only_mine, |d| {
                d.child(filter_chip("scm-graph-f-mine", crate::i18n::t("Mine"), &p).relative().child(probe(HistoryFrame::Filter(1))).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.only_mine = false;
                        cx.notify();
                    },
                )))
            })
            .when(hide_merges, |d| {
                d.child(filter_chip("scm-graph-f-merges", crate::i18n::t("No merges"), &p).relative().child(probe(HistoryFrame::Filter(2))).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.hide_merges = false;
                        cx.notify();
                    }),
                ))
            });

        let height = self.section_height("graph", 260.);
        let selected = self.selected.as_ref().map(|c| c.hash.clone());
        let body = open.then(|| {
            let (g, items, flat) = match self.graph_items(cx) {
                Ok(shown) => shown,
                Err((why, _)) => {
                    return div().px(px(theme::SPACE[6])).py(px(theme::SPACE[2])).text_color(c(p.text_muted)).child(why).into_any_element();
                }
            };
            let frames = self.a11y_frames.clone();
            let n = items.len();
            uniform_list(
                "scm-graph-rows",
                n,
                cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                    let (show_author, show_hash) = {
                        let st = cx.global::<crate::settings::Settings>();
                        (st.graph_show_author, st.graph_show_hash)
                    };
                    let picked: std::collections::HashSet<String> = this.range_run().into_iter().collect();
                    let tooltips = this.status.as_ref().map(|s| CardSource::new(this.host.clone(), s.root.clone(), this.card_cache.clone()));
                    range
                        .map(|i| {
                            let (row, key) = match &items[i] {
                                GraphItem::Day(label, count) => (day_header(label, *count, mode.row_height(), &p), format!("day:{label}")),
                                GraphItem::Commit(ix) => {
                                    let is_sel = selected.as_deref() == Some(g.commits[*ix].hash.as_str()) || picked.contains(&g.commits[*ix].hash);
                                    let row = if mode == GraphView::Graph {
                                        graph_row(&g, *ix, flat, is_sel, show_author, show_hash, tooltips.clone(), !this.menu_open, &p, cx)
                                    } else {
                                        compact_row(&g, *ix, is_sel, tooltips.clone(), !this.menu_open, &p, cx)
                                    };
                                    (row, g.commits[*ix].hash.clone())
                                }
                            };
                            div()
                                .w_full()
                                .relative()
                                .child(row)
                                .child(crate::painted::probe(&frames, AccessFrame::History(HistoryFrame::Row(key))))
                                .into_any_element()
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(self.graph_scroll.clone())
            .size_full()
            .into_any_element()
        });
        div()
            .flex()
            .flex_col()
            .flex_shrink()
            .when(fill, |d| d.flex_1())
            .min_h(px(super::layout::ROW_H + 4.))
            .relative()
            .child(probe(HistoryFrame::Section("graph")))
            .child(self.top_sash("graph", &p, cx))
            .child(header)
            .when(open && filtered, |d| d.child(filters))
            .when(open && self.search_open, |d| {
                d.child(
                    div().px(px(theme::SPACE[4])).py(px(theme::SPACE[2])).child(
                        ui::input_box("scm-query-box", &p, p.bg, query_focused)
                            .relative()
                            .child(probe(HistoryFrame::Query))
                            .child(ui::search_icon(&p))
                            .child(div().flex_1().min_w_0().child(self.query.clone())),
                    ),
                )
            })
            .when_some(body, |d, body| {
                d.child(
                    div()
                        .when(fill, |d| d.flex_1())
                        .when(!fill, |d| d.h(px(height)))
                        .flex_shrink()
                        .min_h(px(60.))
                        .relative()
                        .child(probe(HistoryFrame::List("graph")))
                        .child(body),
                )
            })
    }
}

/// Day header: "Saturday, 24 Oct 2026" and that day's count.
fn day_header(label: &str, count: usize, h: f32, p: &Palette) -> AnyElement {
    div()
        .w_full()
        .h(px(h))
        .flex()
        .items_center()
        .gap(px(theme::SPACE[3]))
        .px(px(theme::SPACE[5]))
        .bg(c(p.surface))
        .text_size(px(text::XXS.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(c(p.text_muted))
        .child(div().flex_1().overflow_hidden().whitespace_nowrap().child(label.to_string()))
        .child(div().flex_none().font_weight(gpui::FontWeight::NORMAL).child(count.to_string()))
        .into_any_element()
}

/// Shared row shell: selected highlight, hover, click, right click and hover card.
fn row_shell(
    ix: usize,
    k: &GraphCommit,
    h: f32,
    is_sel: bool,
    tooltips: Option<CardSource>,
    show_tooltips: bool,
    p: &Palette,
    cx: &mut Context<ScmPanel>,
) -> gpui::Stateful<gpui::Div> {
    let (entry, menu_entry, card) = (k.clone(), k.clone(), k.clone());
    let here = k.refs.iter().any(|r| r.kind == RefKind::Head);
    div()
        .id(("scm-graph", ix))
        // Not pulled yet: the whole row recedes; the row of the checked-out commit is the one that is lit.
        .when(k.incoming, |d| d.opacity(0.6))
        .when(here && !is_sel, |d| d.bg(ui::ca(p.accent, 0.1)))
        .w_full()
        .h(px(h))
        .flex()
        .items_center()
        .whitespace_nowrap()
        .overflow_hidden()
        .cursor_pointer()
        // Thin highlight line on the left of the selected row; no divider line.
        .border_l_2()
        .border_color(c(if is_sel { p.accent } else { p.surface }))
        .when(is_sel, |d| d.bg(c(p.surface_active)))
        .hover(|d| d.bg(c(p.surface_hover)))
        .when(show_tooltips, |d| d.hoverable_tooltip(move |_, cx| cx.new(|cx| CommitCard::new(card.clone(), tooltips.clone(), cx)).into()))
        .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, _, cx| {
            this.pick_commit(entry.clone(), ev.modifiers().shift, cx);
            if ev.click_count() >= 2 {
                cx.emit(ScmEvent::OpenCommit(entry.clone()));
            }
        }))
        .on_mouse_down(
            gpui::MouseButton::Right,
            cx.listener(move |_, ev: &gpui::MouseDownEvent, _, cx| cx.emit(ScmEvent::CommitMenu(menu_entry.clone(), ev.position))),
        )
}

/// Graph view: one line per commit. The lanes take only the room this row's branches need, then the
/// subject, the author (dimmed), the most important branch / tag and, if enabled, the short hash.
fn graph_row(
    g: &GraphData,
    ix: usize,
    flat: bool,
    is_sel: bool,
    show_author: bool,
    show_hash: bool,
    tooltips: Option<CardSource>,
    show_tooltips: bool,
    p: &Palette,
    cx: &mut Context<ScmPanel>,
) -> AnyElement {
    let (commit, row) = (&g.commits[ix], &g.rows[ix]);
    let head = commit.refs.iter().any(|r| r.kind == RefKind::Head);
    let merge = commit.parents.len() > 1;
    let refs = ordered_refs(commit);
    let more = refs.len().saturating_sub(1);
    row_shell(ix, commit, graph::ROW_H, is_sel, tooltips, show_tooltips, p, cx)
        .pr(px(theme::SPACE[3]))
        .child(if flat {
            // Rows are skipped, so lines would break: a dot in the commit's lane color stands in for the graph.
            let color = graph::lane_color(p, row.color);
            div()
                .w(px(graph::width_px(1)))
                .flex_none()
                .flex()
                .justify_center()
                .child(
                    div()
                        .size(px(8.))
                        .rounded_full()
                        .when(merge || commit.incoming, |d| d.border_2().border_color(c(color)))
                        .when(!merge && !commit.incoming, |d| d.bg(c(color))),
                )
                .into_any_element()
        } else {
            graph::lane_element(row.clone(), head, merge, commit.incoming, *p).into_any_element()
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(theme::SPACE[3]))
                .overflow_hidden()
                .pl(px(theme::SPACE[1]))
                .child(
                    div()
                        .flex_shrink()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_size(px(text::SM.0))
                        .text_color(c(if merge { p.text_muted } else { p.text }))
                        .when(head, |d| d.font_weight(gpui::FontWeight::SEMIBOLD))
                        .child(commit.subject.clone()),
                )
                .when(show_author, |d| {
                    d.child(
                        div()
                            .min_w_0()
                            // The author gives way long before the subject does.
                            .map(|mut d| {
                                d.style().flex_shrink = Some(12.);
                                d
                            })
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_size(px(text::XXS.0))
                            .text_color(c(p.text_muted))
                            .child(commit.author.clone()),
                    )
                }),
        )
        .children(refs.first().map(|r| div().flex_none().ml(px(theme::SPACE[2])).child(ref_chip(r, p))))
        .when(more > 0, |d| {
            d.child(div().flex_none().ml(px(theme::SPACE[1])).text_size(px(text::BADGE.0)).text_color(c(p.text_muted)).child(format!("+{more}")))
        })
        // Not pulled yet: an arrow says where it is going, whatever the row's other columns show.
        .when(commit.incoming, |d| d.child(div().flex_none().ml(px(theme::SPACE[2])).child(icon("arrow-down", 12., p.warning))))
        .when(show_hash, |d| {
            d.child(
                div()
                    .flex_none()
                    .ml(px(theme::SPACE[3]))
                    .font_family(ui::MONO_FONT)
                    .text_size(px(text::XXS.0))
                    .text_color(c(p.text_muted))
                    .child(commit.short.clone()),
            )
        })
        .into_any_element()
}

/// Compact and by-day views: single line, dot in the lane's color, subject, one label, time. The author is in the hover card
/// (an avatar would take space and truncate the subject).
fn compact_row(
    g: &GraphData,
    ix: usize,
    is_sel: bool,
    tooltips: Option<CardSource>,
    show_tooltips: bool,
    p: &Palette,
    cx: &mut Context<ScmPanel>,
) -> AnyElement {
    let (commit, row) = (&g.commits[ix], &g.rows[ix]);
    let head = commit.refs.iter().any(|r| r.kind == RefKind::Head);
    let merge = commit.parents.len() > 1;
    let refs = ordered_refs(commit);
    let color = graph::lane_color(p, row.color);
    row_shell(ix, commit, GraphView::Compact.row_height(), is_sel, tooltips, show_tooltips, p, cx)
        .gap(px(theme::SPACE[3]))
        .pl(px(theme::SPACE[4]))
        .pr(px(theme::SPACE[4]))
        // Dot: the lane's color; HEAD ringed, merge hollow (same language as the graph).
        .child(
            div()
                .flex_none()
                .size(px(8.))
                .rounded_full()
                .when(merge || commit.incoming, |d| d.border_2().border_color(c(color)))
                .when(!merge && !commit.incoming, |d| d.bg(c(color)))
                .when(head, |d| d.border_2().border_color(c(p.text))),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_size(px(text::SM.0))
                .text_color(c(if merge { p.text_muted } else { p.text }))
                .when(head, |d| d.font_weight(gpui::FontWeight::SEMIBOLD))
                .child(commit.subject.clone()),
        )
        .children(refs.first().map(|r| ref_chip(r, p)))
        .child(
            div().flex_none().w(px(26.)).flex().justify_end().text_size(px(text::XXS.0)).text_color(c(p.text_muted)).child(crate::git::short_ago(&commit.when)),
        )
        .into_any_element()
}

#[cfg(test)]
#[path = "scm_graph_card_tests.rs"]
mod card_tests;

/// What tests ask of the graph's selection (kept here so `scm.rs` stays within its line limit).
#[cfg(test)]
impl ScmPanel {
    pub fn selected_hash(&self) -> Option<String> {
        self.selected.as_ref().map(|c| c.hash.clone())
    }

    pub fn detail_for_test(&self) -> Option<Arc<CommitDetail>> {
        self.detail.clone()
    }
}

impl ScmPanel {
    /// The commits of the range picked with Shift-click, oldest first: the run of first parents between the two rows. Empty when
    /// there is no range, or the rows do not lie on one such run (a squash needs one).
    pub(crate) fn range_run(&self) -> Vec<String> {
        let (Some((from, to)), Some(g)) = (&self.range, &self.graph) else { return Vec::new() };
        let at = |hash: &str| g.commits.iter().position(|c| c.hash == hash);
        let (Some(a), Some(b)) = (at(from), at(to)) else { return Vec::new() };
        // Commits are listed newest first: walk from the newer row down the first parents to the older one.
        let (older, mut current) = (a.max(b), a.min(b));
        // Only the branch's own line (the checked-out commit and its first parents) can be rewritten.
        let mut line = std::collections::HashSet::new();
        let mut walk = self.head_hash().and_then(|head| at(&head));
        while let Some(i) = walk.filter(|i| line.insert(*i)) {
            walk = g.commits[i].parents.first().and_then(|p| at(p));
        }
        if !line.contains(&a) || !line.contains(&b) {
            return Vec::new();
        }
        let mut run = Vec::new();
        loop {
            run.push(g.commits[current].hash.clone());
            if current == older {
                run.reverse();
                return run;
            }
            match g.commits[current].parents.first().and_then(|p| at(p)) {
                Some(next) if next > current && next <= older => current = next,
                _ => return Vec::new(),
            }
        }
    }

    /// A click on a graph row: Shift extends the range, a plain click selects the commit alone.
    pub(crate) fn pick_commit(&mut self, commit: GraphCommit, shift: bool, cx: &mut Context<Self>) {
        if shift {
            return self.extend_range(commit, cx);
        }
        self.range = None;
        self.select_commit(commit, cx);
    }

    /// Shift-click: the range runs from the selected commit (or the range's start) to this one.
    fn extend_range(&mut self, commit: GraphCommit, cx: &mut Context<Self>) {
        let anchor = self.range.as_ref().map(|(from, _)| from.clone()).or_else(|| self.selected.as_ref().map(|c| c.hash.clone()));
        let Some(anchor) = anchor else { return self.select_commit(commit, cx) };
        self.range = Some((anchor, commit.hash.clone()));
        self.select_commit(commit, cx);
    }

    /// The commit the checked-out branch was at when the graph was read: what a rewrite is pinned to.
    pub(crate) fn head_hash(&self) -> Option<String> {
        self.graph.as_ref()?.commits.iter().find(|c| c.refs.iter().any(|r| r.kind == RefKind::Head)).map(|c| c.hash.clone())
    }

    /// Rewrite the branch's unpushed commits in `repo`: what happens, and what is refused, is in `git::rewrite`. The graph
    /// reads the new history afterwards and the old selection (whose hashes are gone) is dropped.
    pub fn rewrite_in(&mut self, repo: Option<super::Target>, tip: String, what: crate::git::Rewrite, cx: &mut Context<Self>) {
        let Some(target) = repo.or_else(|| self.history.clone()).or_else(|| self.target(None).ok()) else { return };
        self.run_in(
            target,
            move |host, root| crate::git::rewrite(host, root, &tip, &what),
            |this, _, cx| {
                this.range = None;
                this.selected = None;
                this.detail = None;
                cx.notify();
            },
            cx,
        );
    }
}

#[cfg(test)]
#[path = "scm_graph_rewrite_tests.rs"]
mod rewrite_tests;
