//! Side-by-side diff (S07): old version on the left (HEAD or index), new on the right.
//! Lines aligned, changed blocks colored, virtualized list. Read-only, except that Source Control's diffs stage and
//! unstage picked lines (`diff_stage`).

use std::collections::{BTreeSet, HashMap};
use std::ops::Range;

use gpui::{
    App, Context, FocusHandle, Focusable, HighlightStyle, ScrollStrategy, SharedString, StyledText, UniformListScrollHandle, Window, actions, div, prelude::*,
    px, uniform_list,
};

use crate::git::{self, DiffRow};
use crate::settings::Settings;
use crate::ui::{self, MONO_FONT, c, ca, icon, palette, text};

actions!(diff, [NextChange, PrevChange, ToggleInline]);

/// The diff's keys: changes (F7), the inline view, and picking lines to stage (`diff_stage`). `cyx` lists them as
/// built-in keys too (its `BINDING_SOURCES`).
pub fn key_bindings() -> [gpui::KeyBinding; 7] {
    use crate::diff_stage::{ApplyPicked, ClearPicked, PickChange};
    use gpui::KeyBinding;
    let at = Some("DiffView");
    [
        KeyBinding::new("alt-f5", PrevChange, at),
        KeyBinding::new("f7", NextChange, at),
        KeyBinding::new("shift-f7", PrevChange, at),
        KeyBinding::new("secondary-k secondary-i", ToggleInline, at),
        KeyBinding::new("enter", ApplyPicked, at),
        KeyBinding::new("space", PickChange, at),
        KeyBinding::new("escape", ClearPicked, at),
    ]
}

pub struct DiffView {
    /// Path relative to the repo root (tab name and title).
    pub path: String,
    pub name: SharedString,
    /// The comparison in the title ("HEAD ↔ Working Tree").
    pub label: SharedString,
    left: Vec<String>,
    right: Vec<String>,
    pub(crate) rows: Vec<DiffRow>,
    /// The texts shown (`old` on the left), as given.
    pub(crate) old_text: String,
    pub(crate) new_text: String,
    /// First lines of the changed blocks (for navigation).
    hunks: Vec<usize>,
    /// Row ranges of the changed blocks.
    pub(crate) blocks: Vec<Range<usize>>,
    /// The block navigated with F7 / ⇧F7.
    pub(crate) current: usize,
    stats: (usize, usize),
    /// Single-column (inline) view.
    inline: bool,
    inline_rows: Vec<git::InlineRow>,
    /// The side-by-side row each inline row shows.
    inline_src: Vec<usize>,
    /// Inline (character-level) diffs: line number → byte ranges in the tab-expanded text.
    left_marks: HashMap<usize, Vec<Range<usize>>>,
    right_marks: HashMap<usize, Vec<Range<usize>>>,
    scroll: UniformListScrollHandle,
    focus_handle: FocusHandle,
    /// Staging from this diff (Source Control's diffs only).
    pub(crate) stage: Option<crate::diff_stage::Stager>,
    /// Changed rows picked for staging, the row a Shift-click extends from, and the row under the pointer.
    pub(crate) picked: BTreeSet<usize>,
    pub(crate) anchor: Option<usize>,
    pub(crate) hover_row: Option<usize>,
    pub(crate) busy: bool,
    /// The last operation's result: its message and whether it failed.
    pub(crate) outcome: Option<(SharedString, bool)>,
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

/// For each inline row, the side-by-side row it shows: a block's removed lines first, then its added ones.
fn inline_sources(rows: &[DiffRow], blocks: &[Range<usize>]) -> Vec<usize> {
    let mut out = Vec::with_capacity(rows.len());
    let mut i = 0;
    while i < rows.len() {
        match blocks.iter().find(|b| b.start == i) {
            Some(b) => {
                out.extend(b.clone().filter(|&r| matches!(rows[r], DiffRow::Changed(Some(_), _))));
                out.extend(b.clone().filter(|&r| matches!(rows[r], DiffRow::Changed(_, Some(_)))));
                i = b.end;
            }
            None => {
                out.push(i);
                i += 1;
            }
        }
    }
    out
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
            old_text: String::new(),
            new_text: String::new(),
            hunks: Vec::new(),
            blocks: Vec::new(),
            current: 0,
            stats: (0, 0),
            inline: cx.global::<Settings>().diff_inline,
            inline_rows: Vec::new(),
            inline_src: Vec::new(),
            left_marks: HashMap::new(),
            right_marks: HashMap::new(),
            scroll: UniformListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            stage: None,
            picked: BTreeSet::new(),
            anchor: None,
            hover_row: None,
            busy: false,
            outcome: None,
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
        self.old_text = old.to_string();
        self.new_text = new.to_string();
        self.rows = git::side_by_side(old, new);
        self.inline_rows = git::inline_rows(&self.rows);
        self.blocks = git::blocks(&self.rows);
        self.inline_src = inline_sources(&self.rows, &self.blocks);
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
        self.scroll.scroll_to_item(self.list_index(self.hunks[self.current]).saturating_sub(3), ScrollStrategy::Top);
        cx.notify();
    }

    /// The list item that shows side-by-side row `row` (its first inline row in the inline view).
    fn list_index(&self, row: usize) -> usize {
        if self.inline { self.inline_src.iter().position(|&r| r == row).unwrap_or(row) } else { row }
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

/// Width of the column where changed lines are picked for staging.
const PICK_W: f32 = 22.;
/// Height of a change's Stage / Unstage button drawn over its first line.
const BLOCK_BUTTON_H: f32 = 18.;

/// Sizes and colors one frame's rows share.
#[derive(Clone, Copy)]
struct RowStyle {
    p: ui::Palette,
    line_h: f32,
    num_w: f32,
    font_size: f32,
    /// The diff has the keyboard: the change under F7 is marked.
    focused: bool,
}

impl DiffView {
    /// How a whole row is read: both sides of a changed row.
    fn row_heard(&self, src: usize) -> String {
        match self.rows.get(src) {
            Some(DiffRow::Changed(a, b)) => {
                let mut parts = Vec::new();
                if let Some(a) = a {
                    parts.push(line_heard("-", Some(*a), self.left.get(*a)));
                }
                if let Some(b) = b {
                    parts.push(line_heard("+", Some(*b), self.right.get(*b)));
                }
                parts.join("; ")
            }
            Some(DiffRow::Same(_, b)) => line_heard(" ", Some(*b), self.right.get(*b)),
            None => String::new(),
        }
    }

    /// Whether the change `block` shows its button: under the pointer, or under F7 while the diff has the keyboard.
    fn block_active(&self, block: Option<usize>, focused: bool) -> bool {
        block.is_some() && (self.hover_row.and_then(|r| self.block_of(r)) == block || (focused && block == Some(self.current)))
    }

    /// The pick column of row `src` (`key`: its list item): a checkbox on a changed row, a mark on the change under F7.
    fn pick_cell(&self, src: usize, key: &str, st: RowStyle, cx: &mut Context<Self>) -> gpui::AnyElement {
        let p = st.p;
        let block = self.block_of(src);
        let current = st.focused && block.is_some() && block == Some(self.current);
        let cell = div()
            .w(px(PICK_W))
            .h(px(st.line_h))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .relative()
            .when(current, |d| d.border_l_2().border_color(c(p.accent)));
        if !matches!(self.rows.get(src), Some(DiffRow::Changed(..))) {
            return cell.into_any_element();
        }
        let picked = self.picked.contains(&src);
        let near = self.block_active(block, st.focused) || block.is_some_and(|b| self.picked.range(self.blocks[b].clone()).next().is_some());
        let unstage = self.unstages();
        let label = self.row_heard(src);
        let tick = div()
            .size(px(13.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(theme::radius::SM))
            .when(picked, |d| d.bg(c(p.accent)).child(icon("check", 11., p.on_accent)))
            .when(!picked, |d| d.border_1().border_color(if near { c(p.text_muted) } else { ca(p.text_muted, 0.3) }));
        cell.id(SharedString::from(format!("pick-{key}")))
            .cursor_pointer()
            .hover(|d| d.bg(c(p.surface_hover)))
            .child(crate::a11y_controls::control(
                crate::a11y_controls::Control::new(self.path.clone(), format!("pick-{key}"), accesskit::Role::CheckBox, label)
                    .description(if unstage { crate::i18n::t("Enter unstages the picked lines") } else { crate::i18n::t("Enter stages the picked lines") })
                    .toggled(picked)
                    .on_entity(cx.entity().downgrade(), move |this: &mut Self, _, cx| this.click_row(src, false, cx)),
            ))
            .child(tick)
            .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, _, cx| this.click_row(src, ev.modifiers().shift, cx)))
            .into_any_element()
    }

    /// Over the first line of change `block`: its Stage / Unstage button (shown while it is active) and, always, its
    /// control in the tree.
    fn block_button(&self, block: usize, st: RowStyle, cx: &mut Context<Self>) -> gpui::AnyElement {
        let p = st.p;
        let (glyph, label) = if self.unstages() { ("minus", crate::i18n::t("Unstage Change")) } else { ("plus", crate::i18n::t("Stage Change")) };
        let heard = crate::i18n::tf("{0}, change {1} of {2}", &[&label, &(block + 1), &self.blocks.len()]);
        let shown = self.block_active(Some(block), st.focused) && !self.busy;
        let control = crate::a11y_controls::control(
            crate::a11y_controls::Control::button(self.path.clone(), format!("block-{block}"), heard, |_, _| {})
                .on_entity(cx.entity().downgrade(), move |this: &mut Self, _, cx| this.apply_block(block, cx)),
        );
        if !shown {
            // Hidden, the button is still in the tree, framed by the change's first line.
            return div().absolute().top_0().left_0().size_full().child(control).into_any_element();
        }
        div()
            .id(SharedString::from(format!("diff-block-{block}")))
            .absolute()
            .top(px(((st.line_h - BLOCK_BUTTON_H) / 2.).max(0.)))
            .right(px(theme::SPACE[4]))
            .h(px(BLOCK_BUTTON_H))
            .child(control)
            .flex()
            .items_center()
            .gap(px(theme::SPACE[2]))
            .px(px(theme::SPACE[3]))
            .rounded(px(theme::radius::MD))
            .bg(c(p.surface_raised))
            .border_1()
            .border_color(c(p.border))
            .shadow_sm()
            .font_family(ui::UI_FONT)
            .text_size(px(text::XS.0))
            .text_color(c(p.text))
            .cursor_pointer()
            .hover(|d| d.bg(c(p.surface_hover)).border_color(c(p.accent)))
            .child(icon(glyph, 12., p.accent))
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.apply_block(block, cx);
            }))
            .into_any_element()
    }

    /// A list row of either view: the pick column, the hover that shows its change's button, and that button on the
    /// change's first row. `inner` is the row's content.
    /// `first`: the change this row is the first line of.
    #[allow(clippy::too_many_arguments)]
    fn stage_row(
        &self,
        id: SharedString,
        src: usize,
        key: &str,
        first: Option<usize>,
        inner: gpui::Div,
        st: RowStyle,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if !self.can_stage() {
            return inner.into_any_element();
        }
        div()
            .id(id)
            .w_full()
            .flex()
            .relative()
            .child(self.pick_cell(src, key, st, cx))
            .child(inner.flex_1().min_w_0())
            .children(first.map(|b| self.block_button(b, st, cx)))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.hover_row != Some(src) {
                    this.hover_row = Some(src);
                    cx.notify();
                } else if !*hovered && this.hover_row == Some(src) {
                    this.hover_row = None;
                    cx.notify();
                }
            }))
            .into_any_element()
    }

    fn num_cell(n: Option<usize>, st: RowStyle) -> gpui::Div {
        div().w(px(st.num_w)).flex_none().flex().justify_end().pr(px(theme::SPACE[4])).text_color(c(st.p.line_number)).children(n.map(|n| (n + 1).to_string()))
    }

    fn inline_row(&self, ix: usize, st: RowStyle, cx: &mut Context<Self>) -> gpui::AnyElement {
        let p = st.p;
        let src = self.inline_src.get(ix).copied().unwrap_or(ix);
        let picked = self.picked.contains(&src);
        let (old_n, new_n, text, sign, tint) = match self.inline_rows[ix] {
            git::InlineRow::Same(a, b) => (Some(a), Some(b), self.right.get(b), " ", None),
            git::InlineRow::Removed(a) => (Some(a), None, self.left.get(a), "-", Some(p.danger)),
            git::InlineRow::Added(b) => (None, Some(b), self.right.get(b), "+", Some(p.success)),
        };
        let heard = crate::a11y_controls::said(self.path.clone(), format!("i{ix}"), accesskit::Role::ListItem, line_heard(sign, old_n.or(new_n), text));
        let inner = div()
            .w_full()
            .h(px(st.line_h))
            .flex()
            .overflow_hidden()
            .relative()
            .child(heard)
            .when_some(tint, |d, t| d.bg(ca(t, if picked { 0.3 } else { 0.16 })))
            .child(Self::num_cell(old_n, st))
            .child(Self::num_cell(new_n, st))
            .child(div().w(px(st.font_size)).flex_none().text_color(c(tint.unwrap_or(p.text_muted))).child(sign))
            .child(div().flex_1().min_w_0().whitespace_nowrap().overflow_hidden().text_color(c(p.text)).children(match self.inline_rows[ix] {
                git::InlineRow::Removed(a) => code_cell(text, self.left_marks.get(&a), p.danger),
                git::InlineRow::Added(b) => code_cell(text, self.right_marks.get(&b), p.success),
                git::InlineRow::Same(..) => code_cell(text, None, p.text),
            }));
        let first = self.block_of(src).filter(|&b| ix == 0 || !self.blocks[b].contains(&self.inline_src[ix - 1]));
        self.stage_row(SharedString::from(format!("diff-irow-{ix}")), src, &format!("i{ix}"), first, inner, st, cx)
    }

    fn side_row(&self, ix: usize, st: RowStyle, cx: &mut Context<Self>) -> gpui::AnyElement {
        let p = st.p;
        let (l, r, changed) = match self.rows[ix] {
            DiffRow::Same(a, b) => (Some(a), Some(b), false),
            DiffRow::Changed(a, b) => (a, b, true),
        };
        let picked = self.picked.contains(&ix);
        let side = |n: Option<usize>, lines: &Vec<String>, marks: &HashMap<usize, Vec<Range<usize>>>, tint: u32| {
            let sign = match (changed, tint == p.danger) {
                (false, _) => " ",
                (true, true) => "-",
                (true, false) => "+",
            };
            let key = format!("{}{ix}", if tint == p.danger { "l" } else { "r" });
            let heard = n.map(|n| crate::a11y_controls::said(self.path.clone(), key, accesskit::Role::ListItem, line_heard(sign, Some(n), lines.get(n))));
            let bg = match (changed, n) {
                (true, Some(_)) => Some(ca(tint, if picked { 0.3 } else { 0.16 })),
                // Line with no counterpart: empty, muted like strikethrough.
                (true, None) => Some(ca(p.text_muted, if picked { 0.12 } else { 0.06 })),
                _ => None,
            };
            div()
                .flex_1()
                .min_w_0()
                .h(px(st.line_h))
                .flex()
                .overflow_hidden()
                .relative()
                .children(heard)
                .when_some(bg, |d, bg| d.bg(bg))
                .child(Self::num_cell(n, st))
                .child(div().flex_1().min_w_0().whitespace_nowrap().overflow_hidden().text_color(c(p.text)).children(code_cell(
                    n.and_then(|n| lines.get(n)),
                    n.and_then(|n| marks.get(&n)),
                    tint,
                )))
        };
        let inner = div()
            .w_full()
            .flex()
            .child(side(l, &self.left, &self.left_marks, p.danger))
            .child(div().w(px(1.)).h(px(st.line_h)).flex_none().bg(c(p.border)))
            .child(side(r, &self.right, &self.right_marks, p.success));
        let first = self.blocks.iter().position(|b| b.start == ix);
        self.stage_row(SharedString::from(format!("diff-row-{ix}")), ix, &ix.to_string(), first, inner, st, cx)
    }

    /// The header's staging part: what was done, what is picked and the button that stages it, or a hint.
    fn stage_status(&self, p: ui::Palette, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        self.stage.as_ref()?;
        let unstage = self.unstages();
        let surface = self.path.clone();
        let ctl = |key: &str, label: &str| crate::a11y_controls::Control::button(surface.clone(), key.to_string(), label, |_, _| {});
        let button = |id: &'static str, label: &str, primary: bool| {
            div()
                .id(id)
                .px(px(theme::SPACE[4]))
                .h(px(22.))
                .flex()
                .items_center()
                .rounded(px(theme::radius::MD))
                .cursor_pointer()
                .text_size(px(text::XS.0))
                .when(primary, |d| d.bg(c(p.accent)).text_color(c(p.on_accent)).hover(|d| d.opacity(0.9)))
                .when(!primary, |d| d.text_color(c(p.text)).hover(|d| d.bg(c(p.surface_hover))))
                .child(label.to_string())
        };
        let row = div().flex().items_center().gap(px(theme::SPACE[3])).min_w_0();
        let row = if self.busy {
            row.child(ui::spinner("diff-stage-busy", 12., p.text_muted)).child(div().text_color(c(p.text_muted)).child(if unstage {
                crate::i18n::t("Unstaging…")
            } else {
                crate::i18n::t("Staging…")
            }))
        } else if let Some((message, failed)) = &self.outcome {
            row.child(icon(if *failed { "alert" } else { "circle-check" }, 13., if *failed { p.danger } else { p.success }))
                .child(div().truncate().text_color(c(if *failed { p.danger } else { p.text })).child(message.clone()))
                .child(crate::a11y_controls::said(self.path.clone(), "outcome", accesskit::Role::Status, message.to_string()))
        } else if !self.picked.is_empty() {
            let n = self.picked.len();
            let apply = if unstage { crate::i18n::t("Unstage Lines") } else { crate::i18n::t("Stage Lines") };
            row.child(div().text_color(c(p.text_muted)).child(crate::i18n::tn("{} line picked", "{} lines picked", n, &[])))
                .child(
                    crate::a11y_controls::press_on(
                        button("diff-clear-picked", crate::i18n::t("Clear"), false),
                        ctl("clear", crate::i18n::t("Clear")),
                        cx,
                        |this, w, cx| this.clear_picked(&crate::diff_stage::ClearPicked, w, cx),
                    )
                    .tooltip(|_, cx| cx.new(|_| crate::workbench::Tooltip(crate::i18n::t("Clear the picked lines (Escape)").into())).into()),
                )
                .child(
                    crate::a11y_controls::press_on(button("diff-apply-picked", apply, true), ctl("apply", apply), cx, |this, _, cx| this.apply_selection(cx))
                        .tooltip(move |_, cx| cx.new(|_| crate::workbench::Tooltip(crate::i18n::tf("{} (Enter)", &[&apply]).into())).into()),
                )
        } else if !self.blocks.is_empty() {
            row.child(div().text_color(c(p.text_muted)).truncate().child(if unstage {
                crate::i18n::t("Pick lines to unstage them, or use a change's button")
            } else {
                crate::i18n::t("Pick lines to stage them, or use a change's button")
            }))
        } else {
            return None;
        };
        Some(row.into_any_element())
    }

    /// A Source Control diff with nothing left in it.
    fn empty_state(&self, p: ui::Palette) -> gpui::Div {
        let text = if self.unstages() { crate::i18n::t("Nothing in this file is staged") } else { crate::i18n::t("Every change in this file is staged") };
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(theme::SPACE[4]))
            .font_family(ui::UI_FONT)
            .text_size(px(text::SM.0))
            .text_color(c(p.text_muted))
            .child(icon("circle-check", 28., p.success))
            .child(div().relative().child(text).child(crate::a11y_controls::said(self.path.clone(), "empty", accesskit::Role::Label, text)))
    }
}

impl Render for DiffView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        let settings = cx.global::<Settings>();
        let (font_size, line_h) = (settings.editor_font_size, settings.editor_line_height);
        let font: SharedString = settings.editor_font_family.clone().map_or(MONO_FONT.into(), Into::into);
        let digits = self.left.len().max(self.right.len()).to_string().len().max(3) as f32;
        let num_w = digits * font_size * 0.62 + theme::SPACE[5];
        let st = RowStyle { p, line_h, num_w, font_size, focused: self.focus_handle.is_focused(window) };
        let (added, removed) = self.stats;
        let this_scroll = self.scroll.clone();
        let empty = self.can_stage() && self.blocks.is_empty() && !self.busy;
        let status = self.stage_status(p, cx);
        let root = div()
            .key_context("DiffView")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::next_change))
            .on_action(cx.listener(Self::prev_change))
            .on_action(cx.listener(Self::toggle_inline));
        self.register_stage_actions(root, cx)
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
                    .child(div().flex_none().text_color(c(p.text)).child(self.path.clone()))
                    .child(div().flex_none().child(self.label.clone()))
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
                    .when_some(status, |d, status| d.child(status).child(div().w(px(1.)).h(px(14.)).flex_none().bg(c(p.border))))
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
                    .when(self.hunks.is_empty() && !self.can_stage(), |d| d.child(crate::i18n::t("No changes"))),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .font_family(font.clone())
                    .text_size(px(font_size))
                    .line_height(px(line_h))
                    .when(empty, |d| d.child(self.empty_state(p)))
                    .when(self.inline && !empty, |d| {
                        d.child(
                            uniform_list(
                                "diff-inline-rows",
                                self.inline_rows.len(),
                                cx.processor(move |this, range: Range<usize>, _, cx| range.map(|ix| this.inline_row(ix, st, cx)).collect::<Vec<_>>()),
                            )
                            .track_scroll(this_scroll.clone())
                            .size_full(),
                        )
                    })
                    .when(!self.inline && !empty, |d| {
                        d.child(
                            uniform_list(
                                "diff-rows",
                                self.rows.len(),
                                cx.processor(move |this, range: Range<usize>, _, cx| range.map(|ix| this.side_row(ix, st, cx)).collect::<Vec<_>>()),
                            )
                            .track_scroll(self.scroll.clone())
                            .size_full(),
                        )
                    }),
            )
    }
}
