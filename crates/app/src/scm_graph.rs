//! Source Control's GRAPH section: view modes (graph, compact, by day), filters ("mine", "hide
//! merges", search), rows and the card that opens when hovering a commit. Submodule of `scm.rs`: reads the panel's private fields.

use gpui::{AnyElement, Context, Focusable as _, Render, SharedString, Window, div, prelude::*, px, uniform_list};

use super::{GraphData, ScmEvent, ScmPanel, avatar, commit_matches};
use crate::graph::{self, GraphCommit, GraphItem, GraphView, RefKind, RefLabel};
use crate::sections;
use crate::ui::{self, Palette, c, icon, palette, text};

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
        .text_size(px(10.))
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

/// On commit hover: author and time, subject, all branches/tags, hash and hint; a tidy card instead of a wall of plain text.
pub struct CommitCard {
    commit: GraphCommit,
}

impl Render for CommitCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        let k = &self.commit;
        let refs = ordered_refs(k);
        let muted = |t: String| div().text_size(px(text::XXS.0)).text_color(c(p.text_muted)).child(t);
        div()
            .w(px(340.))
            .flex()
            .flex_col()
            .gap(px(theme::SPACE[4]))
            .p(px(theme::SPACE[5]))
            .bg(c(p.surface_raised))
            .border_1()
            .border_color(c(p.border))
            .rounded(px(theme::radius::MD))
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
            .child(div().font_weight(gpui::FontWeight::MEDIUM).child(k.subject.clone()))
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
                                .text_size(px(10.))
                                .child(crate::i18n::t("Merge")),
                        )
                    })
                    .child(div().flex_1())
                    .child(muted(k.email.clone())),
            )
            .child(
                div()
                    .pt(px(theme::SPACE[2]))
                    .border_t_1()
                    .border_color(c(p.border))
                    .child(muted(crate::i18n::t("Click: details · Double-click: open · Right-click: actions").into())),
            )
    }
}

/// Pill button in the view / filter bar.
fn pill(id: &'static str, label: &'static str, active: bool, tip: &'static str, p: &Palette) -> gpui::Stateful<gpui::Div> {
    let bg = if active { ui::mix(p.surface, p.accent, 0.22) } else { p.surface };
    let fg = if active { ui::ensure_contrast(p.accent, p.text, &[bg], 4.6) } else { p.text_muted };
    div()
        .id(id)
        .h(px(20.))
        .px(px(theme::SPACE[3]))
        .flex()
        .items_center()
        .rounded(px(10.))
        .bg(c(bg))
        .text_size(px(10.))
        .text_color(c(fg))
        .cursor_pointer()
        .when(!active, |d| d.hover(|s| s.bg(c(p.surface_hover)).text_color(c(p.text))))
        .tooltip(move |_, cx| cx.new(|_| crate::workbench::Tooltip(tip.into())).into())
        .child(label)
}

impl ScmPanel {
    pub(super) fn set_graph_view(&mut self, view: GraphView, cx: &mut Context<Self>) {
        self.graph_view = Some(view);
        cx.notify();
    }

    pub(super) fn graph_section(&self, query_focused: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let p = *palette(cx);
        let open = self.sections.is_open("graph", true);
        let query = self.query.read(cx).text().to_string();
        let searching = !query.trim().is_empty();
        let count = self.graph.as_ref().map(|g| g.commits.len());
        let scope = self.scope;
        let mode = self.graph_view.unwrap_or_else(|| GraphView::from_id(&cx.global::<crate::settings::Settings>().graph_view).unwrap_or_default());
        let (only_mine, hide_merges) = (self.only_mine, self.hide_merges);
        let has_identity = self.identity.as_ref().is_some_and(|(n, e)| n.is_some() || e.is_some());
        let header = sections::header("scm-graph-header", crate::i18n::t("Graph"), count, open, &p)
            .when(self.graph_loading, |d| d.child(ui::spinner("scm-graph-busy", 12., p.text_muted)))
            .child(
                ui::icon_button(
                    "scm-graph-scope",
                    "git-branch",
                    if scope == graph::Scope::All {
                        crate::i18n::t("Showing all branches (click: current branch only)")
                    } else {
                        crate::i18n::t("Showing the current branch (click: all branches)")
                    },
                    &p,
                )
                .when(scope == graph::Scope::Current, |d| d.bg(c(p.surface_active)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.set_scope(if scope == graph::Scope::All { graph::Scope::Current } else { graph::Scope::All }, cx);
                })),
            )
            .child(
                ui::icon_button("scm-graph-search", "search", crate::i18n::t("Search commits"), &p)
                    .when(self.search_open, |d| d.bg(c(p.surface_active)))
                    .on_click(cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.search_open = !this.search_open;
                        this.sections.set_open("graph", true);
                        if this.search_open {
                            window.focus(&this.query.focus_handle(cx));
                        } else {
                            this.query.update(cx, |q, cx| q.set_text("", cx));
                        }
                        cx.notify();
                    })),
            )
            .child(ui::icon_button("scm-graph-refresh", "refresh-cw", crate::i18n::t("Reload history"), &p).on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.load_graph(cx);
            })))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_section("graph", cx)));

        // View and filter bar: graph / compact / by day, then "mine" and "hide merges".
        let toolbar = div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap(px(theme::SPACE[1]))
            .px(px(theme::SPACE[4]))
            .py(px(theme::SPACE[2]))
            .children(GraphView::ALL.map(|v| {
                let tip = match v {
                    GraphView::Graph => crate::i18n::t("Branch lanes, two lines per commit"),
                    GraphView::Compact => crate::i18n::t("One line per commit: the most history in a narrow sidebar"),
                    GraphView::Dates => crate::i18n::t("Commits grouped by day"),
                };
                pill(v.id(), v.label(), mode == v, tip, &p).on_click(cx.listener(move |this, _, _, cx| this.set_graph_view(v, cx)))
            }))
            .child(div().w(px(1.)).h(px(12.)).mx(px(theme::SPACE[2])).bg(c(p.border)))
            .when(has_identity, |d| {
                d.child(
                    pill("scm-graph-mine", crate::i18n::t("Mine"), only_mine, crate::i18n::t("Only your own commits (by your git name or e-mail)"), &p)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.only_mine = !this.only_mine;
                            cx.notify();
                        })),
                )
            })
            .child(pill("scm-graph-nomerges", crate::i18n::t("No merges"), hide_merges, crate::i18n::t("Hide merge commits"), &p).on_click(cx.listener(
                |this, _, _, cx| {
                    this.hide_merges = !this.hide_merges;
                    cx.notify();
                },
            )));

        let height = self.section_height("graph", 260.);
        let selected = self.selected.as_ref().map(|c| c.hash.clone());
        let mine_identity = self.identity.clone();
        let body = open.then(|| {
            let Some(g) = self.graph.clone() else {
                let msg: SharedString = match &self.graph_error {
                    Some(e) => crate::i18n::tf("Could not read the history: {}", &[&e]).into(),
                    None => crate::i18n::t("Loading…").into(),
                };
                return div().px(px(theme::SPACE[6])).py(px(theme::SPACE[2])).text_color(c(p.text_muted)).child(msg).into_any_element();
            };
            if g.commits.is_empty() {
                return div()
                    .px(px(theme::SPACE[6]))
                    .py(px(theme::SPACE[2]))
                    .text_color(c(p.text_muted))
                    .child(crate::i18n::t("No commits yet."))
                    .into_any_element();
            }
            let mine = |k: &GraphCommit| {
                let Some((name, email)) = &mine_identity else { return false };
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
                return div().px(px(theme::SPACE[6])).py(px(theme::SPACE[2])).text_color(c(p.text_muted)).child(why).into_any_element();
            }
            let n = items.len();
            uniform_list(
                "scm-graph-rows",
                n,
                cx.processor(move |_this, range: std::ops::Range<usize>, _window, cx| {
                    let (show_author, show_hash) = {
                        let st = cx.global::<crate::settings::Settings>();
                        (st.graph_show_author, st.graph_show_hash)
                    };
                    range
                        .map(|i| match &items[i] {
                            GraphItem::Day(label, count) => day_header(label, *count, mode.row_height(), &p),
                            GraphItem::Commit(ix) => {
                                let is_sel = selected.as_deref() == Some(g.commits[*ix].hash.as_str());
                                if mode == GraphView::Graph {
                                    graph_row(&g, *ix, flat, is_sel, show_author, show_hash, &p, cx)
                                } else {
                                    compact_row(&g, *ix, is_sel, &p, cx)
                                }
                            }
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
            .min_h(px(super::layout::ROW_H + 4.))
            .child(self.top_sash("graph", &p, cx))
            .child(header)
            .when(open, |d| d.child(toolbar))
            .when(open && self.search_open, |d| {
                d.child(
                    div().px(px(theme::SPACE[4])).py(px(theme::SPACE[2])).child(
                        ui::input_box("scm-query-box", &p, p.bg, query_focused)
                            .child(ui::search_icon(&p))
                            .child(div().flex_1().min_w_0().child(self.query.clone())),
                    ),
                )
            })
            .when_some(body, |d, body| d.child(div().h(px(height)).flex_shrink().min_h(px(60.)).child(body)))
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
fn row_shell(ix: usize, k: &GraphCommit, h: f32, is_sel: bool, p: &Palette, cx: &mut Context<ScmPanel>) -> gpui::Stateful<gpui::Div> {
    let (entry, menu_entry, card) = (k.clone(), k.clone(), k.clone());
    div()
        .id(("scm-graph", ix))
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
        .tooltip(move |_, cx| cx.new(|_| CommitCard { commit: card.clone() }).into())
        .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, _, cx| {
            this.select_commit(entry.clone(), cx);
            if ev.click_count() >= 2 {
                cx.emit(ScmEvent::OpenCommit(entry.clone()));
            }
        }))
        .on_mouse_down(
            gpui::MouseButton::Right,
            cx.listener(move |_, ev: &gpui::MouseDownEvent, _, cx| cx.emit(ScmEvent::CommitMenu(menu_entry.clone(), ev.position))),
        )
}

/// Graph view: lane (or avatar if filtered) + two-line text.
fn graph_row(g: &GraphData, ix: usize, flat: bool, is_sel: bool, show_author: bool, show_hash: bool, p: &Palette, cx: &mut Context<ScmPanel>) -> AnyElement {
    let (commit, row) = (&g.commits[ix], &g.rows[ix]);
    let head = commit.refs.iter().any(|r| r.kind == RefKind::Head);
    let refs = ordered_refs(commit);
    let more = refs.len().saturating_sub(1);
    row_shell(ix, commit, graph::ROW_H, is_sel, p, cx)
        .child(if flat {
            div().w(px(graph::width_px(1))).flex_none().flex().justify_center().child(avatar(&commit.author, 14., p)).into_any_element()
        } else {
            graph::lane_element(row.clone(), g.cols, head, commit.parents.len() > 1, *p).into_any_element()
        })
        .child(ui::commit_lines(
            commit.subject.clone().into(),
            crate::git::short_ago(&commit.when).into(),
            div()
                .flex()
                .items_center()
                .gap(px(theme::SPACE[2]))
                .children(refs.first().map(|r| ref_chip(r, p)))
                .when(more > 0, |d| d.child(div().flex_none().child(format!("+{more}"))))
                .when(show_author, |d| d.child(avatar(&commit.author, 14., p))),
            if show_hash { commit.short.clone().into() } else { SharedString::default() },
            head,
            p,
        ))
        .into_any_element()
}

/// Compact and by-day views: single line, dot in the lane's color, subject, one label, time. The author is in the hover card
/// (an avatar would take space and truncate the subject).
fn compact_row(g: &GraphData, ix: usize, is_sel: bool, p: &Palette, cx: &mut Context<ScmPanel>) -> AnyElement {
    let (commit, row) = (&g.commits[ix], &g.rows[ix]);
    let head = commit.refs.iter().any(|r| r.kind == RefKind::Head);
    let merge = commit.parents.len() > 1;
    let refs = ordered_refs(commit);
    let color = graph::lane_color(p, row.color);
    row_shell(ix, commit, GraphView::Compact.row_height(), is_sel, p, cx)
        .gap(px(theme::SPACE[3]))
        .pl(px(theme::SPACE[4]))
        .pr(px(theme::SPACE[4]))
        // Dot: the lane's color; HEAD ringed, merge hollow (same language as the graph).
        .child(
            div()
                .flex_none()
                .size(px(8.))
                .rounded_full()
                .when(merge, |d| d.border_2().border_color(c(color)))
                .when(!merge, |d| d.bg(c(color)))
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
