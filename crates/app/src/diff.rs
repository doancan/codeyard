//! Side-by-side diff (S07): old version on the left (HEAD or index), new on the right.
//! Read-only; lines aligned, changed blocks colored, virtualized list.

use std::collections::HashMap;
use std::ops::Range;

use gpui::{
    App, Context, FocusHandle, Focusable, HighlightStyle, ScrollStrategy, SharedString, StyledText, UniformListScrollHandle, Window, actions, div, prelude::*,
    px, uniform_list,
};

use crate::git::{self, DiffRow};
use crate::settings::Settings;
use crate::ui::{self, MONO_FONT, c, ca, icon, palette, text};

actions!(diff, [NextChange, PrevChange, ToggleInline]);

pub struct DiffView {
    /// Path relative to the repo root (tab name and title).
    pub path: String,
    pub name: SharedString,
    /// The comparison in the title ("HEAD ↔ Working Tree").
    pub label: SharedString,
    left: Vec<String>,
    right: Vec<String>,
    rows: Vec<DiffRow>,
    /// First lines of the changed blocks (for navigation).
    hunks: Vec<usize>,
    /// The block navigated with F7 / ⇧F7.
    current: usize,
    stats: (usize, usize),
    /// Single-column (inline) view.
    inline: bool,
    inline_rows: Vec<git::InlineRow>,
    /// Inline (character-level) diffs: line number → byte ranges in the tab-expanded text.
    left_marks: HashMap<usize, Vec<Range<usize>>>,
    right_marks: HashMap<usize, Vec<Range<usize>>>,
    scroll: UniformListScrollHandle,
    focus_handle: FocusHandle,
}

/// A diff line for the tree: whether it was added or removed, its number and its text.
fn line_heard(sign: &str, line: Option<usize>, text: Option<&String>) -> String {
    let what = match sign {
        "+" => crate::i18n::t("Added"),
        "-" => crate::i18n::t("Removed"),
        _ => crate::i18n::t("Unchanged"),
    };
    let line = line.map(|n| crate::i18n::tf("line {}", &[&(n + 1)])).unwrap_or_default();
    format!("{what}, {line}: {}", text.map(String::as_str).unwrap_or_default())
}

/// The rendered line text: tabs expand to four spaces (mark ranges are computed against this text).
fn expand_tabs(line: &str) -> String {
    line.replace('\t', "    ")
}

/// Code cell: changed characters are highlighted with a darker shade of `tint`.
fn code_cell(text: Option<&String>, marks: Option<&Vec<Range<usize>>>, tint: u32) -> Option<StyledText> {
    let text = expand_tabs(text?);
    let style = HighlightStyle { background_color: Some(ca(tint, 0.4).into()), ..Default::default() };
    let marks = marks.into_iter().flatten().map(|r| (r.clone(), style));
    Some(StyledText::new(text).with_highlights(marks))
}

impl Focusable for DiffView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl DiffView {
    pub fn new(path: String, staged: bool, old: &str, new: &str, cx: &mut Context<Self>) -> Self {
        let label = if staged { crate::i18n::t("HEAD ↔ Staged") } else { crate::i18n::t("Index ↔ Working Tree") };
        let mut view = DiffView {
            name: ui::file_name(&path).to_string().into(),
            path,
            label: label.into(),
            left: Vec::new(),
            right: Vec::new(),
            rows: Vec::new(),
            hunks: Vec::new(),
            current: 0,
            stats: (0, 0),
            inline: cx.global::<Settings>().diff_inline,
            inline_rows: Vec::new(),
            left_marks: HashMap::new(),
            right_marks: HashMap::new(),
            scroll: UniformListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
        };
        view.set_texts(old, new);
        // Scroll to the first change (with a few lines of context).
        if let Some(&first) = view.hunks.first() {
            view.scroll.scroll_to_item(first.saturating_sub(3), ScrollStrategy::Top);
        }
        view
    }

    /// The description in the title (instead of "HEAD ↔ Staged", e.g. a commit or comparison).
    pub fn with_label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = label.into();
        self
    }

    pub fn set_texts(&mut self, old: &str, new: &str) {
        let split = |s: &str| {
            let mut v: Vec<String> = s.split('\n').map(str::to_string).collect();
            if v.last().is_some_and(|l| l.is_empty()) {
                v.pop(); // the final newline shouldn't count as an empty line
            }
            v
        };
        self.left = split(old);
        self.right = split(new);
        self.rows = git::side_by_side(old, new);
        self.inline_rows = git::inline_rows(&self.rows);
        self.hunks.clear();
        self.left_marks.clear();
        self.right_marks.clear();
        let (mut added, mut removed) = (0, 0);
        for (i, row) in self.rows.iter().enumerate() {
            if let DiffRow::Changed(a, b) = row {
                if let (Some(a), Some(b)) = (*a, *b) {
                    let (old_marks, new_marks) = git::intraline(&expand_tabs(&self.left[a]), &expand_tabs(&self.right[b]));
                    if !old_marks.is_empty() {
                        self.left_marks.insert(a, old_marks);
                    }
                    if !new_marks.is_empty() {
                        self.right_marks.insert(b, new_marks);
                    }
                }
                if i == 0 || !matches!(self.rows[i - 1], DiffRow::Changed(..)) {
                    self.hunks.push(i);
                }
                removed += a.is_some() as usize;
                added += b.is_some() as usize;
            }
        }
        self.stats = (added, removed);
    }

    #[cfg(test)]
    pub fn summary(&self) -> (usize, usize, usize) {
        (self.stats.0, self.stats.1, self.hunks.len())
    }

    fn jump(&mut self, forward: bool, cx: &mut Context<Self>) {
        let n = self.hunks.len();
        if n == 0 {
            return;
        }
        self.current = if forward { (self.current + 1) % n } else { (self.current + n - 1) % n };
        self.scroll.scroll_to_item(self.hunks[self.current].saturating_sub(3), ScrollStrategy::Top);
        cx.notify();
    }

    fn toggle_inline(&mut self, _: &ToggleInline, _: &mut Window, cx: &mut Context<Self>) {
        self.inline = !self.inline;
        cx.notify();
    }

    #[cfg(test)]
    pub fn is_inline(&self) -> bool {
        self.inline
    }

    #[cfg(test)]
    pub fn set_inline_for_test(&mut self, v: bool) {
        self.inline = v;
    }

    fn next_change(&mut self, _: &NextChange, _: &mut Window, cx: &mut Context<Self>) {
        self.jump(true, cx);
    }
    fn prev_change(&mut self, _: &PrevChange, _: &mut Window, cx: &mut Context<Self>) {
        self.jump(false, cx);
    }
}

impl Render for DiffView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        let settings = cx.global::<Settings>();
        let (font_size, line_h) = (settings.editor_font_size, settings.editor_line_height);
        let font: SharedString = settings.editor_font_family.clone().map_or(MONO_FONT.into(), Into::into);
        let digits = self.left.len().max(self.right.len()).to_string().len().max(3) as f32;
        let num_w = digits * font_size * 0.62 + theme::SPACE[5];
        let (added, removed) = self.stats;
        let this_scroll = self.scroll.clone();
        div()
            .key_context("DiffView")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::next_change))
            .on_action(cx.listener(Self::prev_change))
            .on_action(cx.listener(Self::toggle_inline))
            .size_full()
            .flex()
            .flex_col()
            .bg(c(p.bg))
            .child(
                div()
                    .h(px(30.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[4]))
                    .px(px(theme::SPACE[6]))
                    .border_b_1()
                    .border_color(c(p.border))
                    .text_size(px(text::XS.0))
                    .text_color(c(p.text_muted))
                    .child(icon("git-compare", 14., p.text_muted))
                    .child(div().text_color(c(p.text)).child(self.path.clone()))
                    .child(self.label.clone())
                    .relative()
                    .child(crate::a11y_controls::said(
                        self.path.clone(),
                        "summary",
                        accesskit::Role::Heading,
                        format!(
                            "{} · {} · {}",
                            self.label,
                            crate::i18n::tf("{} added", &[&added]),
                            if self.hunks.is_empty() { crate::i18n::t("No changes").to_string() } else { crate::i18n::tf("{} removed", &[&removed]) }
                        ),
                    ))
                    .child(div().flex_1())
                    .child(div().text_color(c(p.success)).child(format!("+{added}")))
                    .child(div().text_color(c(p.danger)).child(format!("−{removed}")))
                    .child(
                        div()
                            .id("diff-inline-toggle")
                            .relative()
                            .child(crate::a11y_controls::control(
                                crate::a11y_controls::Control::button(self.path.clone(), "inline", crate::i18n::t("Toggle inline view"), |_, _| {})
                                    .description(if self.inline { crate::i18n::t("Inline") } else { crate::i18n::t("Side by side") })
                                    .on_entity(cx.entity().downgrade(), |this: &mut Self, w, cx| this.toggle_inline(&ToggleInline, w, cx)),
                            ))
                            .cursor_pointer()
                            .px(px(theme::SPACE[3]))
                            .rounded(px(theme::radius::LG))
                            .hover(|s| s.bg(c(p.surface_hover)))
                            .text_color(c(p.text))
                            .child(if self.inline { crate::i18n::t("Inline") } else { crate::i18n::t("Side by side") })
                            .tooltip(|_, cx| cx.new(|_| crate::workbench::Tooltip(crate::i18n::t("Toggle inline view").into())).into())
                            .on_click(cx.listener(|this, _, w, cx| this.toggle_inline(&ToggleInline, w, cx))),
                    )
                    .when(self.hunks.is_empty(), |d| d.child(crate::i18n::t("No changes"))),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .font_family(font.clone())
                    .text_size(px(font_size))
                    .line_height(px(line_h))
                    .when(self.inline, |d| {
                        d.child(
                            uniform_list(
                                "diff-inline-rows",
                                self.inline_rows.len(),
                                cx.processor(move |this, range: Range<usize>, _, _| {
                                    range
                                        .map(|ix| {
                                            let (old_n, new_n, text, sign, tint) = match this.inline_rows[ix] {
                                                git::InlineRow::Same(a, b) => (Some(a), Some(b), this.right.get(b), " ", None),
                                                git::InlineRow::Removed(a) => (Some(a), None, this.left.get(a), "-", Some(p.danger)),
                                                git::InlineRow::Added(b) => (None, Some(b), this.right.get(b), "+", Some(p.success)),
                                            };
                                            let heard = crate::a11y_controls::said(
                                                this.path.clone(),
                                                format!("i{ix}"),
                                                accesskit::Role::ListItem,
                                                line_heard(sign, old_n.or(new_n), text),
                                            );
                                            let num = |n: Option<usize>| {
                                                div()
                                                    .w(px(num_w))
                                                    .flex_none()
                                                    .flex()
                                                    .justify_end()
                                                    .pr(px(theme::SPACE[4]))
                                                    .text_color(c(p.line_number))
                                                    .children(n.map(|n| (n + 1).to_string()))
                                            };
                                            div()
                                                .w_full()
                                                .h(px(line_h))
                                                .flex()
                                                .overflow_hidden()
                                                .relative()
                                                .child(heard)
                                                .when_some(tint, |d, t| d.bg(ca(t, 0.16)))
                                                .child(num(old_n))
                                                .child(num(new_n))
                                                .child(div().w(px(font_size)).flex_none().text_color(c(tint.unwrap_or(p.text_muted))).child(sign))
                                                .child(div().flex_1().min_w_0().whitespace_nowrap().overflow_hidden().text_color(c(p.text)).children(
                                                    match this.inline_rows[ix] {
                                                        git::InlineRow::Removed(a) => code_cell(text, this.left_marks.get(&a), p.danger),
                                                        git::InlineRow::Added(b) => code_cell(text, this.right_marks.get(&b), p.success),
                                                        git::InlineRow::Same(..) => code_cell(text, None, p.text),
                                                    },
                                                ))
                                        })
                                        .collect::<Vec<_>>()
                                }),
                            )
                            .track_scroll(this_scroll.clone())
                            .size_full(),
                        )
                    })
                    .when(!self.inline, |d| {
                        d.child(
                            uniform_list(
                                "diff-rows",
                                self.rows.len(),
                                cx.processor(move |this, range: Range<usize>, _, _| {
                                    range
                                        .map(|ix| {
                                            let (l, r, changed) = match this.rows[ix] {
                                                DiffRow::Same(a, b) => (Some(a), Some(b), false),
                                                DiffRow::Changed(a, b) => (a, b, true),
                                            };
                                            let path = this.path.clone();
                                            let side = |n: Option<usize>, lines: &Vec<String>, marks: &HashMap<usize, Vec<Range<usize>>>, tint: u32| {
                                                let sign = match (changed, tint == p.danger) {
                                                    (false, _) => " ",
                                                    (true, true) => "-",
                                                    (true, false) => "+",
                                                };
                                                let key = format!("{}{ix}", if tint == p.danger { "l" } else { "r" });
                                                let heard = n.map(|n| {
                                                    crate::a11y_controls::said(
                                                        path.clone(),
                                                        key,
                                                        accesskit::Role::ListItem,
                                                        line_heard(sign, Some(n), lines.get(n)),
                                                    )
                                                });
                                                let bg = match (changed, n) {
                                                    (true, Some(_)) => Some(ca(tint, 0.16)),
                                                    // Line with no counterpart: empty, muted like strikethrough.
                                                    (true, None) => Some(ca(p.text_muted, 0.06)),
                                                    _ => None,
                                                };
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .h(px(line_h))
                                                    .flex()
                                                    .overflow_hidden()
                                                    .relative()
                                                    .children(heard)
                                                    .when_some(bg, |d, bg| d.bg(bg))
                                                    .child(
                                                        div()
                                                            .w(px(num_w))
                                                            .flex_none()
                                                            .flex()
                                                            .justify_end()
                                                            .pr(px(theme::SPACE[4]))
                                                            .text_color(c(p.line_number))
                                                            .children(n.map(|n| (n + 1).to_string())),
                                                    )
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .min_w_0()
                                                            .whitespace_nowrap()
                                                            .overflow_hidden()
                                                            .text_color(c(p.text))
                                                            .children(code_cell(n.and_then(|n| lines.get(n)), n.and_then(|n| marks.get(&n)), tint)),
                                                    )
                                            };
                                            div()
                                                .w_full()
                                                .flex()
                                                .child(side(l, &this.left, &this.left_marks, p.danger))
                                                .child(div().w(px(1.)).h(px(line_h)).flex_none().bg(c(p.border)))
                                                .child(side(r, &this.right, &this.right_marks, p.success))
                                        })
                                        .collect::<Vec<_>>()
                                }),
                            )
                            .track_scroll(self.scroll.clone())
                            .size_full(),
                        )
                    }),
            )
    }
}
