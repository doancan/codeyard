//! Workbench: the Problems panel's filter bar, rows, folding and copying. The filter is kept per workspace in the state
//! database (`problems.filter:<workspace>`), so a folder opens with the panel as it was left.

use gpui::div;

use super::filter::copy_line;
use super::*;
use crate::a11y_controls::{Control, press_on};
use crate::input::{FieldEvent, TextField};

gpui::actions!(problems, [CopyAllProblems, ToggleProblemsActiveFile]);

/// The panel's filter and its text field (created when a folder opens or the panel is shown).
#[derive(Default)]
pub struct ProblemsView {
    filter: ProblemFilter,
    field: Option<Entity<TextField>>,
}

/// The text of the rows for the clipboard: one line per problem, in the order shown (folded files included).
fn rows_text(rows: &[ProblemRow], root: Option<&str>) -> (String, usize) {
    let lines: Vec<String> = rows
        .iter()
        .filter_map(|r| match r {
            ProblemRow::Problem { path, severity, source, code, message, line, col, .. } => {
                Some(copy_line(crate::lsp::related_path(path, root), *severity, source, code, message, *line, *col))
            }
            _ => None,
        })
        .collect();
    (lines.join("\n"), lines.len())
}

impl Workbench {
    fn problems_state_key(&self) -> Option<String> {
        self.persist.workspace_id.map(|ws| format!("problems.filter:{ws}"))
    }

    /// A folder was opened: its saved filter comes back, and the text field shows it.
    pub(in crate::workbench) fn load_problems_filter(&mut self, cx: &mut Context<Self>) {
        let saved = self.problems_state_key().and_then(|key| self.persist.state.as_ref()?.get(&key));
        self.chrome.problems.filter = saved.map(|s| ProblemFilter::decode(&s)).unwrap_or_default();
        self.ensure_problems_field(cx);
        let text = self.chrome.problems.filter.text.clone();
        if let Some(field) = &self.chrome.problems.field {
            field.update(cx, |f, cx| f.set_text(&text, cx));
        }
        cx.notify();
    }

    pub(in crate::workbench) fn ensure_problems_field(&mut self, cx: &mut Context<Self>) {
        if self.chrome.problems.field.is_some() {
            return;
        }
        let field = cx.new(|cx| TextField::new(crate::i18n::tr("Filter problems"), cx));
        self._subs.push(cx.subscribe(&field, |this, field, ev: &FieldEvent, cx| {
            if matches!(ev, FieldEvent::Changed) {
                let text = field.read(cx).text().to_string();
                this.change_problems_filter(|f| f.text = text, cx);
            }
        }));
        self.chrome.problems.field = Some(field);
    }

    /// Changes the filter, keeps it for the workspace and draws the panel again.
    pub(in crate::workbench) fn change_problems_filter(&mut self, change: impl FnOnce(&mut ProblemFilter), cx: &mut Context<Self>) {
        let before = self.chrome.problems.filter.clone();
        change(&mut self.chrome.problems.filter);
        if self.chrome.problems.filter == before {
            return;
        }
        if let (Some(key), Some(state)) = (self.problems_state_key(), &self.persist.state) {
            let _ = state.set(&key, &self.chrome.problems.filter.encode());
        }
        cx.notify();
    }

    fn open_file_path(&self, cx: &App) -> Option<String> {
        self.tab_editor().map(|e| e.read(cx).path.clone())
    }

    /// The rows the panel shows now, under its filter.
    pub(in crate::workbench) fn problem_rows_now(&self, cx: &App) -> Vec<ProblemRow> {
        problem_rows(&self.problems_shown(), self.root(cx).as_deref(), &self.chrome.problems.filter, self.open_file_path(cx).as_deref())
    }

    pub(in crate::workbench) fn problem_filter(&self) -> &ProblemFilter {
        &self.chrome.problems.filter
    }

    /// Folds every file shut, or opens them all when all are shut already.
    fn toggle_all_problem_files(&mut self, cx: &mut Context<Self>) {
        let files: Vec<String> =
            self.problem_rows_now(cx).into_iter().filter_map(|r| if let ProblemRow::File { path, .. } = r { Some(path) } else { None }).collect();
        let all_shut = files.iter().all(|p| self.chrome.problems.filter.collapsed.contains(p));
        self.change_problems_filter(
            |f| {
                if all_shut {
                    f.collapsed.clear();
                } else {
                    f.collapsed.extend(files);
                }
            },
            cx,
        );
    }

    pub(in crate::workbench) fn toggle_problem_file(&mut self, path: &str, cx: &mut Context<Self>) {
        self.change_problems_filter(
            |f| {
                if !f.collapsed.remove(path) {
                    f.collapsed.insert(path.to_string());
                }
            },
            cx,
        );
    }

    /// The problems of the rows for the clipboard: all shown, or those of row `ix` (a file's, or one problem).
    fn copy_problems(&mut self, only: Option<usize>, cx: &mut Context<Self>) {
        // Folded files are copied too: folding is about what is drawn, not what the user filtered out.
        let mut filter = self.chrome.problems.filter.clone();
        filter.collapsed.clear();
        let root = self.root(cx);
        let all = problem_rows(&self.problems_shown(), root.as_deref(), &filter, self.open_file_path(cx).as_deref());
        let shown = self.problem_rows_now(cx);
        let rows: Vec<ProblemRow> = match only.and_then(|ix| shown.get(ix)) {
            None => all,
            Some(ProblemRow::File { path, .. }) => all.into_iter().filter(|r| matches!(r, ProblemRow::Problem { path: p, .. } if p == path)).collect(),
            Some(row @ ProblemRow::Problem { .. }) => vec![row.clone()],
            Some(ProblemRow::Related { .. }) => Vec::new(),
        };
        let (text, n) = rows_text(&rows, root.as_deref());
        if n == 0 {
            return;
        }
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        self.notify(NoticeSource::App, Notice::Info(crate::i18n::tn("Copied {} problem", "Copied {} problems", n, &[]).into()));
        cx.notify();
    }

    pub(in crate::workbench) fn copy_all_problems(&mut self, _: &CopyAllProblems, _: &mut Window, cx: &mut Context<Self>) {
        self.copy_problems(None, cx);
    }

    /// *Problems: Toggle Active File Only*.
    pub(in crate::workbench) fn toggle_problems_active_file(&mut self, _: &ToggleProblemsActiveFile, _: &mut Window, cx: &mut Context<Self>) {
        self.change_problems_filter(|f| f.active_file_only = !f.active_file_only, cx);
    }

    /// Enter on, or a click on, row `i`: a file folds or opens, a problem or a related location opens at its position.
    pub(in crate::workbench) fn open_problem_row(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        match self.problem_rows_now(cx).into_iter().nth(i) {
            Some(ProblemRow::File { path, .. }) => self.toggle_problem_file(&path, cx),
            Some(ProblemRow::Problem { path, line, col, .. } | ProblemRow::Related { path, line, col, .. }) => self.open_location(path, line, col, window, cx),
            None => {}
        }
    }

    /// "Quick Fix…" on Problems row `i`: the problem opens at its place, then its fixes are asked for there, as Quick
    /// Fix at the caret does (a file that is still opening asks once it is in front).
    pub(in crate::workbench) fn quick_fix_problem_row(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ProblemRow::Problem { path, line, col, .. }) = self.problem_rows_now(cx).into_iter().nth(i) else { return };
        self.open_location(path.clone(), line, col, window, cx);
        match &mut self.lsp.pending_reveal {
            // Still opening: the fixes are asked for when it is in front.
            Some((p, _, _, fix)) if *p == path => *fix = true,
            _ => self.code_actions(&CodeActions, window, cx),
        }
    }

    /// "Copy Problem" on row `i`.
    pub(in crate::workbench) fn copy_problem_row(&mut self, i: usize, cx: &mut Context<Self>) {
        self.copy_problems(Some(i), cx);
    }

    /// How many rows the panel's list has now.
    pub(in crate::workbench) fn problem_row_count(&self, cx: &App) -> usize {
        self.problem_rows_now(cx).len()
    }

    pub(in crate::workbench) fn problems_view(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let all = self.problems_shown();
        let totals = problem_totals(&all);
        let p = *p;
        if totals.total() == 0 {
            let msg = if self.lsp.servers.is_empty() {
                crate::i18n::t("No problems. Language servers start when you open a file.")
            } else {
                crate::i18n::t("No problems.")
            };
            return div().px(px(theme::SPACE[6])).py(px(theme::SPACE[3])).text_color(c(p.text_muted)).child(msg).into_any_element();
        }
        let filter = self.chrome.problems.filter.clone();
        let rows = self.problem_rows_now(cx);
        let shown: usize = rows.iter().filter(|r| matches!(r, ProblemRow::Problem { .. })).count();
        let list = if rows.is_empty() {
            let msg = if filter.active_file_only && self.open_file_path(cx).is_none() {
                crate::i18n::t("No file is open.")
            } else {
                crate::i18n::t("No problems match the filter.")
            };
            div().px(px(theme::SPACE[6])).py(px(theme::SPACE[3])).text_color(c(p.text_muted)).child(msg).into_any_element()
        } else {
            let count = rows.len();
            self.row_list(
                a11y::RowFrame::Problem,
                gpui::uniform_list(
                    "problem-rows",
                    count,
                    cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                        range
                            .filter_map(|ix| {
                                let row = this.problem_row_el(ix, rows.get(ix)?, &p, cx);
                                // Each row records where it is painted, for the accessibility tree.
                                let framed = div().relative().child(row).child(a11y::part_frame(a11y::Part::Problem(ix), cx)).into_any_element();
                                Some(this.nav_row(a11y::RowFrame::Problem, ix, framed, window, cx))
                            })
                            .collect()
                    }),
                )
                .track_scroll(self.row_scroll(a11y::RowFrame::Problem))
                .size_full(),
                cx,
            )
        };
        div()
            .id("problems")
            .size_full()
            .flex()
            .flex_col()
            .text_size(px(text::SM.0))
            .child(self.problems_bar(&filter, totals, shown, &p, cx))
            .child(div().flex_1().min_h_0().child(list))
            .into_any_element()
    }

    /// The filter bar: severity switches with their counts, the text filter, and the buttons.
    fn problems_bar(&self, filter: &ProblemFilter, totals: filter::Counts, shown: usize, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let surface = crate::i18n::t("Problems");
        let chip = |key: &'static str, glyph: &'static str, color: u32, n: usize, on: bool, label: &'static str| {
            let label = crate::i18n::t(label).to_string();
            let el = div()
                .id(key)
                .h(px(22.))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(theme::SPACE[1]))
                .px(px(theme::SPACE[2]))
                .rounded(px(theme::radius::SM))
                .cursor_pointer()
                .text_size(px(text::XS.0))
                .when(on, |d| d.bg(c(p.surface_active)).text_color(c(p.text)))
                .when(!on, |d| d.text_color(c(p.text_muted)))
                .hover(|d| d.bg(c(p.surface_hover)))
                .child(icon(glyph, 14., if on { color } else { p.text_muted }))
                .child(n.to_string())
                .tooltip({
                    let label = label.clone();
                    move |_, cx| cx.new(|_| Tooltip(label.clone().into())).into()
                });
            let control = Control::new(surface, key, accesskit::Role::Button, label).description(crate::i18n::tf("{} problems", &[&n])).toggled(on);
            press_on(el, control, cx, move |this: &mut Self, _, cx| {
                this.change_problems_filter(
                    |f| match key {
                        "problems-errors" => f.errors = !f.errors,
                        "problems-warnings" => f.warnings = !f.warnings,
                        _ => f.info = !f.info,
                    },
                    cx,
                )
            })
        };
        let field = self.chrome.problems.field.clone().map(|field| {
            let (text, handle) = (field.read(cx).text().to_string(), field.read(cx).focus_handle(cx));
            let access = crate::a11y_controls::control(
                Control::new(surface, "problems-filter", accesskit::Role::TextInput, crate::i18n::t("Filter problems"))
                    .value(text)
                    .focus(handle.clone())
                    .on_entity(cx.entity().downgrade(), move |_: &mut Self, w, _| w.focus(&handle)),
            );
            div()
                .relative()
                .flex_1()
                .min_w(px(120.))
                .px(px(theme::SPACE[2]))
                .py(px(theme::SPACE[1]))
                .rounded(px(theme::radius::SM))
                .border_1()
                .border_color(c(p.border_input))
                .bg(c(p.bg))
                .child(access)
                .child(field)
        });
        let sort_label = match filter.sort {
            Sort::File => crate::i18n::t("Sort: File"),
            Sort::Severity => crate::i18n::t("Sort: Severity"),
            Sort::Line => crate::i18n::t("Sort: Line"),
        };
        let all_shut =
            !filter.collapsed.is_empty() && shown > 0 && self.problem_rows_now(cx).iter().all(|r| !matches!(r, ProblemRow::File { collapsed: false, .. }));
        let fold_label = if all_shut { crate::i18n::t("Expand All Files") } else { crate::i18n::t("Collapse All Files") };
        let active_label = crate::i18n::t("Active File Only");
        let copy_label = crate::i18n::t("Copy All Problems");
        let sort = press_on(
            div()
                .id("problems-sort")
                .h(px(22.))
                .flex_none()
                .flex()
                .items_center()
                .px(px(theme::SPACE[2]))
                .rounded(px(theme::radius::SM))
                .cursor_pointer()
                .text_size(px(text::XS.0))
                .text_color(c(p.text_muted))
                .hover(|d| d.bg(c(p.surface_hover)).text_color(c(p.text)))
                .child(sort_label),
            Control::new(surface, "problems-sort", accesskit::Role::Button, sort_label).description(crate::i18n::t("Changes the order of the problems")),
            cx,
            |this: &mut Self, _, cx| this.change_problems_filter(|f| f.sort = f.sort.next(), cx),
        );
        let active = press_on(
            ui::toggle_button("problems-active-file", "file", active_label, filter.active_file_only, p),
            Control::new(surface, "problems-active-file", accesskit::Role::Button, active_label).toggled(filter.active_file_only),
            cx,
            |this: &mut Self, _, cx| this.change_problems_filter(|f| f.active_file_only = !f.active_file_only, cx),
        );
        let fold = press_on(
            ui::icon_button("problems-fold", if all_shut { "chevrons-up-down" } else { "chevrons-down-up" }, fold_label, p),
            Control::new(surface, "problems-fold", accesskit::Role::Button, fold_label),
            cx,
            |this: &mut Self, _, cx| this.toggle_all_problem_files(cx),
        );
        let copy = press_on(
            ui::icon_button("problems-copy-all", "copy", copy_label, p),
            Control::new(surface, "problems-copy-all", accesskit::Role::Button, copy_label),
            cx,
            |this: &mut Self, _, cx| this.copy_problems(None, cx),
        );
        let summary = filter.narrows().then(|| {
            div()
                .px(px(theme::SPACE[4]))
                .pb(px(theme::SPACE[1]))
                .text_size(px(text::XS.0))
                .text_color(c(p.text_muted))
                .child(crate::i18n::tf("Showing {0} of {1} problems", &[&shown, &totals.total()]))
        });
        div()
            .flex_none()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(theme::SPACE[1]))
                    .px(px(theme::SPACE[3]))
                    .py(px(theme::SPACE[1]))
                    .child(chip("problems-errors", "circle-x", p.danger, totals.errors, filter.errors, crate::i18n::tr("Show Errors")))
                    .child(chip("problems-warnings", "alert", p.warning, totals.warnings, filter.warnings, crate::i18n::tr("Show Warnings")))
                    .child(chip("problems-info", "info", p.info, totals.info, filter.info, crate::i18n::tr("Show Information")))
                    .children(field)
                    .child(active)
                    .child(sort)
                    .child(fold)
                    .child(copy),
            )
            .children(summary)
            .into_any_element()
    }

    /// One row of the list.
    fn problem_row_el(&self, ix: usize, row: &ProblemRow, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let base = |indent: f32| {
            div()
                .id(("problem", ix))
                .h(px(layout::ROW_H))
                .w_full()
                .flex()
                .items_center()
                .gap(px(theme::SPACE[2]))
                .pl(px(indent))
                .pr(px(theme::SPACE[4]))
                .whitespace_nowrap()
                .overflow_hidden()
                .cursor_pointer()
                .hover(|s| s.bg(c(p.surface_hover)))
                .on_click(cx.listener(move |this, _, window, cx| this.open_problem_row(ix, window, cx)))
        };
        match row {
            ProblemRow::File { path, dir, count, collapsed } => base(theme::SPACE[3])
                .child(icon(if *collapsed { "chevron-right" } else { "chevron-down" }, 14., p.text_muted))
                .child(crate::icon_theme::file_icon_el(ui::file_name(path), layout::ICON, p))
                .child(div().flex_none().child(ui::file_name(path).to_string()))
                .child(div().flex_shrink().min_w_0().overflow_hidden().text_ellipsis().text_color(c(p.text_muted)).text_size(px(text::XS.0)).child(dir.clone()))
                .child(div().flex_1())
                .child(
                    div()
                        .flex_none()
                        .px(px(theme::SPACE[2]))
                        .rounded(px(theme::radius::LG))
                        .bg(c(p.surface_active))
                        .text_size(px(text::XS.0))
                        .child(count.to_string()),
                )
                .into_any_element(),
            ProblemRow::Problem { severity, message, source, code, line, col, explain, .. } => {
                let (glyph, color) = match severity {
                    1 => ("circle-x", p.danger),
                    2 => ("alert", p.warning),
                    _ => ("info", p.info),
                };
                let group = SharedString::from(format!("problem-row-{ix}"));
                let muted = |s: String, max: f32| {
                    div().flex_none().max_w(px(max)).overflow_hidden().text_ellipsis().text_color(c(p.text_muted)).text_size(px(text::XS.0)).child(s)
                };
                base(theme::SPACE[8])
                    .group(group.clone())
                    .relative()
                    .child(icon(glyph, 14., color))
                    .child(div().flex_shrink().min_w_0().overflow_hidden().text_ellipsis().child(message.clone()))
                    .child(div().flex_1())
                    .when(!code.is_empty(), |d| d.child(muted(code.clone(), 90.)))
                    .when(!source.is_empty(), |d| d.child(muted(source.clone(), 90.)))
                    .child(muted(format!("{}:{}", line + 1, col + 1), 70.))
                    .when_some(explain.clone(), |row, (code, href)| row.child(self.explain_button(ix, code, href, p, cx)))
                    // The copy button replaces the position while the row is hovered.
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .right_0()
                            .pl(px(theme::SPACE[2]))
                            .pr(px(theme::SPACE[2]))
                            .flex()
                            .items_center()
                            .bg(c(p.surface_hover))
                            .invisible()
                            .group_hover(group, |d| d.visible())
                            .child(ui::icon_button(("problem-copy", ix), "copy", crate::i18n::t("Copy Problem"), p).on_click(cx.listener(
                                move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.copy_problem_row(ix, cx);
                                },
                            ))),
                    )
                    .into_any_element()
            }
            ProblemRow::Related { text, .. } => base(theme::SPACE[8] + 22.)
                .text_color(c(p.text_muted))
                .child(div().min_w_0().overflow_hidden().text_ellipsis().child(text.clone()))
                .into_any_element(),
        }
    }
}

#[cfg(test)]
#[path = "workbench_problems_tests.rs"]
mod tests;
