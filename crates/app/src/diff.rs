//! Side-by-side diff (S07): old version on the left (HEAD or index), new on the right.
//! Lines aligned, changed blocks colored, virtualized list. Read-only, except that Source Control's diffs stage and
//! unstage picked lines (`diff_stage`).

#[path = "diff_chrome.rs"]
mod chrome;
#[path = "diff_syntax.rs"]
pub(crate) mod colors;
#[path = "diff_fold.rs"]
pub(crate) mod fold;
#[path = "diff_guard.rs"]
pub(crate) mod guard;
#[path = "diff_ruler.rs"]
pub(crate) mod ruler;
#[cfg(test)]
#[path = "diff_view_tests.rs"]
mod view_tests;
#[path = "diff_ws.rs"]
pub(crate) mod whitespace;

use std::collections::{BTreeSet, HashMap};
use std::ops::Range;

use gpui::{
    App, Context, FocusHandle, Focusable, HighlightStyle, ScrollStrategy, SharedString, StyledText, UniformListScrollHandle, Window, actions, div, prelude::*,
    px, uniform_list,
};

use self::colors::Runs;
use self::fold::Item;
use self::guard::Unsupported;
use self::ruler::Span;
use self::whitespace::Whitespace;
use crate::git::{self, DiffRow};
use crate::settings::Settings;
use crate::ui::{self, MONO_FONT, c, ca, icon, palette, text};

actions!(diff, [NextChange, PrevChange, ToggleInline, ToggleCollapseUnchanged, ToggleIgnoreWhitespace]);

/// The diff's keys: changes (F7), the inline view, and picking lines to stage (`diff_stage`). `cyx` lists them as
/// built-in keys too (its `BINDING_SOURCES`).
pub fn key_bindings() -> [gpui::KeyBinding; 9] {
    use crate::diff_stage::{ApplyPicked, ClearPicked, PickChange};
    use gpui::KeyBinding;
    let at = Some("DiffView");
    [
        KeyBinding::new("alt-f5", PrevChange, at),
        KeyBinding::new("f7", NextChange, at),
        KeyBinding::new("shift-f7", PrevChange, at),
        KeyBinding::new("secondary-k secondary-i", ToggleInline, at),
        KeyBinding::new("secondary-k secondary-o", ToggleCollapseUnchanged, at),
        KeyBinding::new("secondary-k secondary-h", ToggleIgnoreWhitespace, at),
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
    pub(crate) left: Vec<String>,
    pub(crate) right: Vec<String>,
    pub(crate) rows: Vec<DiffRow>,
    /// The texts shown (`old` on the left), as given.
    pub(crate) old_text: String,
    pub(crate) new_text: String,
    /// First lines of the changed blocks (for navigation).
    pub(crate) hunks: Vec<usize>,
    /// Row ranges of the changed blocks.
    pub(crate) blocks: Vec<Range<usize>>,
    /// The block navigated with F7 / ⇧F7.
    pub(crate) current: usize,
    pub(crate) stats: (usize, usize),
    /// Single-column (inline) view.
    pub(crate) inline: bool,
    pub(crate) inline_rows: Vec<git::InlineRow>,
    /// The side-by-side row each inline row shows.
    pub(crate) inline_src: Vec<usize>,
    /// Inline (character-level) diffs: line number → byte ranges in the tab-expanded text.
    left_marks: HashMap<usize, Vec<Range<usize>>>,
    right_marks: HashMap<usize, Vec<Range<usize>>>,
    /// Syntax colors of each line of either side (empty: not colored).
    syn_left: Vec<Runs>,
    syn_right: Vec<Runs>,
    /// What the list shows: rows, and folds standing in for runs of unchanged ones.
    pub(crate) items: Vec<Item>,
    /// Foldable side-by-side row ranges, and the ones opened (by first row).
    folds: Vec<Range<usize>>,
    pub(crate) expanded: BTreeSet<usize>,
    /// Unchanged regions fold (otherwise the whole file shows).
    pub(crate) collapse: bool,
    pub(crate) ws: Whitespace,
    /// Why the texts are not lined up, if they are not.
    pub(crate) unsupported: Option<Unsupported>,
    /// A side had bytes that are not valid UTF-8 (shown as replacement characters).
    pub(crate) invalid_bytes: bool,
    /// What each side is called ("HEAD", "Working Tree"), from the label.
    pub(crate) titles: Option<(SharedString, SharedString)>,
    /// The overview ruler's markers, over `items`.
    pub(crate) ruler_spans: Vec<Span>,
    pub(crate) ruler_drag: bool,
    /// The path was just copied (the button says so for a moment).
    pub(crate) copied: bool,
    pub(crate) scroll: UniformListScrollHandle,
    /// How far the code of both sides is scrolled to the right (px), and the longest line in characters.
    pub(crate) h_scroll: f32,
    longest: usize,
    /// Width of the list at the last paint: how much of a line fits.
    view_w: std::rc::Rc<std::cell::Cell<f32>>,
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

/// The two sides' names from a label like "HEAD ↔ Working Tree".
fn titles_of(label: &str) -> Option<(SharedString, SharedString)> {
    label.split_once(" ↔ ").map(|(left, right)| (left.to_string().into(), right.to_string().into()))
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
    let mut upcoming = blocks.iter().peekable();
    let mut i = 0;
    while i < rows.len() {
        match upcoming.next_if(|b| b.start == i) {
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

/// Code cell: syntax colors from `runs`, and changed characters on a darker shade of `tint`.
fn code_cell(text: Option<&String>, runs: Option<&Runs>, marks: Option<&Vec<Range<usize>>>, tint: u32, p: &ui::Palette) -> Option<StyledText> {
    let text = expand_tabs(text?);
    let segments = colors::segments(text.len(), runs.map_or(&[][..], Vec::as_slice), marks.map_or(&[][..], Vec::as_slice));
    let styled = segments.into_iter().filter(|s| s.role.is_some() || s.marked).map(|s| {
        let style = HighlightStyle {
            color: s.role.map(|r| c(crate::editor::role_color(Some(r), p)).into()),
            background_color: s.marked.then(|| ca(tint, 0.4).into()),
            ..Default::default()
        };
        (s.range, style)
    });
    Some(StyledText::new(text).with_highlights(styled))
}

/// The farthest the code can scroll right: the longest line's end, plus a little room, reaches the right edge.
/// Nothing scrolls before the list's width is known.
pub(crate) fn h_scroll_max(longest: usize, char_w: f32, visible_w: f32) -> f32 {
    if visible_w <= 0. { 0. } else { ((longest + 2) as f32 * char_w - visible_w).max(0.) }
}

/// The code column of a row: clipped, and moved by the shared horizontal scroll.
fn code_column(text: Option<StyledText>, color: u32, h_scroll: f32) -> gpui::Div {
    div().flex_1().min_w_0().whitespace_nowrap().overflow_hidden().text_color(c(color)).child(div().relative().left(px(-h_scroll)).children(text))
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
            syn_left: Vec::new(),
            syn_right: Vec::new(),
            items: Vec::new(),
            folds: Vec::new(),
            expanded: BTreeSet::new(),
            collapse: true,
            ws: Whitespace::default(),
            unsupported: None,
            invalid_bytes: false,
            titles: titles_of(label),
            ruler_spans: Vec::new(),
            ruler_drag: false,
            copied: false,
            scroll: UniformListScrollHandle::new(),
            h_scroll: 0.,
            longest: 0,
            view_w: Default::default(),
            focus_handle: cx.focus_handle(),
            stage: None,
            picked: BTreeSet::new(),
            anchor: None,
            hover_row: None,
            busy: false,
            outcome: None,
        };
        view.set_texts(old, new);
        view.scroll_to_change(0);
        view
    }

    /// The description in the title (instead of "HEAD ↔ Staged", e.g. a commit or comparison).
    pub fn with_label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = label.into();
        self.titles = titles_of(&self.label);
        self
    }

    pub fn set_texts(&mut self, old: &str, new: &str) {
        let split = |s: &str| git::split_lines(s).into_iter().map(str::to_string).collect::<Vec<_>>();
        self.old_text = old.to_string();
        self.new_text = new.to_string();
        self.unsupported = guard::check(old, new);
        self.invalid_bytes = guard::has_invalid_bytes(old, new);
        let shown = self.unsupported.is_none();
        self.left = if shown { split(old) } else { Vec::new() };
        self.right = if shown { split(new) } else { Vec::new() };
        // The comparison's title can carry more than the file name ("a.rs ↔ Clipboard").
        let language_path = self.path.split(" ↔ ").next().unwrap_or(&self.path);
        self.syn_left = if shown { colors::highlight(language_path, old).unwrap_or_default() } else { Vec::new() };
        self.syn_right = if shown { colors::highlight(language_path, new).unwrap_or_default() } else { Vec::new() };
        self.longest = self.left.iter().chain(&self.right).map(|l| expand_tabs(l).chars().count()).max().unwrap_or(0);
        self.lay_out();
    }

    /// Line up the texts as they are (`self.ws` decides what counts as a difference): rows, changes, word marks, stats and
    /// the list. Colors do not depend on any of it, so a change of the whitespace option comes here and not to `set_texts`.
    fn lay_out(&mut self) {
        self.rows = if self.unsupported.is_none() { whitespace::side_by_side(&self.old_text, &self.new_text, self.ws) } else { Vec::new() };
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
        self.rebuild_items();
    }

    /// The side-by-side row list item `ix` shows.
    pub(crate) fn src_of(&self, ix: usize) -> usize {
        if self.inline { self.inline_src.get(ix).copied().unwrap_or(ix) } else { ix }
    }

    /// The list (rows and folds) and the ruler's markers over it, after the texts, the view or the folds changed.
    pub(crate) fn rebuild_items(&mut self) {
        self.folds = if self.collapse { fold::fold_ranges(&self.blocks, self.rows.len(), fold::CONTEXT) } else { Vec::new() };
        // Folds the reader opened stay open while they still exist (after a stage reloads the texts).
        self.expanded.retain(|start| self.folds.iter().any(|f| f.start == *start));
        self.items = if self.inline {
            fold::items(self.inline_rows.len(), |ix| self.inline_src[ix], &self.folds, &self.expanded)
        } else {
            fold::items(self.rows.len(), |ix| ix, &self.folds, &self.expanded)
        };
        let kind = |item: &Item| match item {
            Item::Fold { .. } => None,
            Item::Row(ix) if self.inline => ruler::kind_of_inline(&self.inline_rows[*ix]),
            Item::Row(ix) => ruler::kind_of_row(&self.rows[*ix]),
        };
        self.ruler_spans = ruler::spans(self.items.iter().map(kind));
    }

    /// How many lines of each side have syntax colors.
    #[cfg(test)]
    pub(crate) fn colored_lines(&self) -> (usize, usize) {
        let colored = |lines: &Vec<Runs>| lines.iter().filter(|runs| !runs.is_empty()).count();
        (colored(&self.syn_left), colored(&self.syn_right))
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
        self.scroll_to_change(self.current);
        cx.notify();
    }

    /// The list item to scroll to for change `n`: a few lines of context above it, and the fold above those if there is one.
    pub(crate) fn change_scroll_target(&self, n: usize) -> Option<usize> {
        let row = *self.hunks.get(n)?;
        let at = self.list_index(row).saturating_sub(fold::CONTEXT);
        Some(if at > 0 && matches!(self.items.get(at - 1), Some(Item::Fold { .. })) { at - 1 } else { at })
    }

    pub(crate) fn scroll_to_change(&mut self, n: usize) {
        if let Some(at) = self.change_scroll_target(n) {
            self.scroll.scroll_to_item(at, ScrollStrategy::Top);
        }
    }

    /// The list item that shows side-by-side row `row` (its first inline row in the inline view).
    pub(crate) fn list_index(&self, row: usize) -> usize {
        self.items.iter().position(|item| matches!(item, Item::Row(ix) if self.src_of(*ix) == row)).unwrap_or(0)
    }

    pub(crate) fn toggle_inline(&mut self, _: &ToggleInline, _: &mut Window, cx: &mut Context<Self>) {
        self.inline = !self.inline;
        self.rebuild_items();
        self.scroll_to_change(self.current);
        cx.notify();
    }

    pub(crate) fn toggle_collapse(&mut self, _: &ToggleCollapseUnchanged, _: &mut Window, cx: &mut Context<Self>) {
        self.collapse = !self.collapse;
        self.expanded.clear();
        self.rebuild_items();
        self.scroll_to_change(self.current);
        cx.notify();
    }

    /// Open the fold that starts at side-by-side row `start`.
    pub(crate) fn expand_fold(&mut self, start: usize, cx: &mut Context<Self>) {
        if self.expanded.insert(start) {
            self.rebuild_items();
            cx.notify();
        }
    }

    /// Exact → trailing → all: the diff is computed again, picks are dropped (staging is off while whitespace is ignored).
    pub(crate) fn toggle_whitespace(&mut self, _: &ToggleIgnoreWhitespace, _: &mut Window, cx: &mut Context<Self>) {
        self.ws = self.ws.next();
        self.picked.clear();
        self.anchor = None;
        self.outcome = None;
        self.expanded.clear();
        self.lay_out();
        self.current = 0;
        self.scroll_to_change(0);
        cx.notify();
    }

    #[cfg(test)]
    pub fn is_inline(&self) -> bool {
        self.inline
    }

    #[cfg(test)]
    pub fn set_inline_for_test(&mut self, v: bool) {
        self.inline = v;
        self.rebuild_items();
    }

    pub(crate) fn next_change(&mut self, _: &NextChange, _: &mut Window, cx: &mut Context<Self>) {
        self.jump(true, cx);
    }
    pub(crate) fn prev_change(&mut self, _: &PrevChange, _: &mut Window, cx: &mut Context<Self>) {
        self.jump(false, cx);
    }
}

/// Width of the column where changed lines are picked for staging.
pub(crate) const PICK_W: f32 = 22.;
/// Height of a change's Stage / Unstage button drawn over its first line.
const BLOCK_BUTTON_H: f32 = 18.;

/// Sizes and colors one frame's rows share.
#[derive(Clone, Copy)]
pub(crate) struct RowStyle {
    pub(crate) p: ui::Palette,
    pub(crate) line_h: f32,
    pub(crate) num_w: f32,
    pub(crate) font_size: f32,
    /// The diff has the keyboard: the change under F7 is marked.
    pub(crate) focused: bool,
    pub(crate) h_scroll: f32,
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

    /// A line number; on a changed line the gutter takes a stronger shade of the change's color.
    fn num_cell(n: Option<usize>, tint: Option<u32>, st: RowStyle) -> gpui::Div {
        div()
            .w(px(st.num_w))
            .h_full()
            .flex_none()
            .flex()
            .justify_end()
            .pr(px(theme::SPACE[4]))
            .text_color(c(st.p.line_number))
            .when_some(tint.filter(|_| n.is_some()), |d, t| d.bg(ca(t, 0.14)).text_color(c(ui::mix(st.p.line_number, t, 0.55))))
            .children(n.map(|n| (n + 1).to_string()))
    }

    /// The `+` / `-` column.
    fn sign_cell(sign: &'static str, tint: Option<u32>, st: RowStyle) -> gpui::Div {
        div().w(px(st.font_size)).flex_none().flex().justify_center().text_color(c(tint.unwrap_or(st.p.text_muted))).child(sign)
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
            .child(Self::num_cell(old_n, tint, st))
            .child(Self::num_cell(new_n, tint, st))
            .child(Self::sign_cell(sign, tint, st))
            .child(code_column(
                match self.inline_rows[ix] {
                    git::InlineRow::Removed(a) => code_cell(text, self.syn_left.get(a), self.left_marks.get(&a), p.danger, &p),
                    git::InlineRow::Added(b) => code_cell(text, self.syn_right.get(b), self.right_marks.get(&b), p.success, &p),
                    git::InlineRow::Same(_, b) => code_cell(text, self.syn_right.get(b), None, p.text, &p),
                },
                p.text,
                st.h_scroll,
            ));
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
        let side = |n: Option<usize>, lines: &Vec<String>, syn: &Vec<Runs>, marks: &HashMap<usize, Vec<Range<usize>>>, tint: u32| {
            let sign = match (changed, n, tint == p.danger) {
                (false, ..) | (true, None, _) => " ",
                (true, Some(_), true) => "-",
                (true, Some(_), false) => "+",
            };
            let row_tint = (changed && n.is_some()).then_some(tint);
            let key = format!("{}{ix}", if tint == p.danger { "l" } else { "r" });
            let heard = n.map(|n| crate::a11y_controls::said(self.path.clone(), key, accesskit::Role::ListItem, line_heard(sign, Some(n), lines.get(n))));
            let bg: Option<gpui::Background> = match (changed, n) {
                (true, Some(_)) => Some(ca(tint, if picked { 0.3 } else { 0.16 }).into()),
                // No counterpart on this side: hatched, so it reads as "nothing here" and not as an empty line.
                (true, None) => Some(gpui::pattern_slash(ca(p.text_muted, if picked { 0.22 } else { 0.12 }).into(), 1., 6.)),
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
                .child(Self::num_cell(n, row_tint, st))
                .child(Self::sign_cell(sign, row_tint, st))
                .child(code_column(
                    code_cell(n.and_then(|n| lines.get(n)), n.and_then(|n| syn.get(n)), n.and_then(|n| marks.get(&n)), tint, &p),
                    p.text,
                    st.h_scroll,
                ))
        };
        let inner = div()
            .w_full()
            .flex()
            .child(side(l, &self.left, &self.syn_left, &self.left_marks, p.danger))
            .child(div().w(px(1.)).h(px(st.line_h)).flex_none().bg(c(p.border)))
            .child(side(r, &self.right, &self.syn_right, &self.right_marks, p.success));
        let first = self.blocks.iter().position(|b| b.start == ix);
        self.stage_row(SharedString::from(format!("diff-row-{ix}")), ix, &ix.to_string(), first, inner, st, cx)
    }

    /// The header's staging part: what was done, what is picked and the button that stages it, or a hint.
    pub(crate) fn stage_status(&self, p: ui::Palette, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        self.stage.as_ref()?;
        let why_not = if self.ws.ignores() {
            Some(crate::i18n::t("Staging is off while whitespace is ignored"))
        } else if self.invalid_bytes {
            Some(crate::i18n::t("Staging is off: the file is not valid UTF-8"))
        } else {
            None
        };
        if let Some(why_not) = why_not {
            return Some(div().truncate().text_color(c(p.text_muted)).child(why_not).into_any_element());
        }
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

impl DiffView {
    /// How wide one side's code column is (the whole column in the inline view), from the list width last painted.
    fn code_w(&self, num_w: f32, sign_w: f32) -> f32 {
        let pick = if self.can_stage() { PICK_W } else { 0. };
        let list = (self.view_w.get() - pick).max(0.);
        if self.inline { list - 2. * num_w - sign_w } else { (list - 1.) / 2. - num_w - sign_w }
    }

    /// List item `i`: a row of the current view, or a fold.
    fn list_item(&self, i: usize, st: RowStyle, cx: &mut Context<Self>) -> gpui::AnyElement {
        match self.items.get(i) {
            Some(&Item::Row(ix)) if self.inline => self.inline_row(ix, st, cx),
            Some(&Item::Row(ix)) => self.side_row(ix, st, cx),
            Some(&Item::Fold { start, len }) => self.fold_row(start, len, st.line_h, st.p, cx),
            None => div().into_any_element(),
        }
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
        let font_id = window.text_system().resolve_font(&gpui::font(font.clone()));
        let char_w = window.text_system().advance(font_id, px(font_size), 'm').map_or(font_size * 0.6, |s| f32::from(s.width));
        let h_max = h_scroll_max(self.longest, char_w, self.code_w(num_w, font_size));
        // The window or the texts changed: what was scrolled past may now fit.
        self.h_scroll = self.h_scroll.clamp(0., h_max);
        let st = RowStyle { p, line_h, num_w, font_size, focused: self.focus_handle.is_focused(window), h_scroll: self.h_scroll };
        let empty = self.can_stage() && self.blocks.is_empty() && !self.busy;
        let status = self.stage_status(p, cx);
        let root = div()
            .key_context("DiffView")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::next_change))
            .on_action(cx.listener(Self::prev_change))
            .on_action(cx.listener(Self::toggle_inline))
            .on_action(cx.listener(Self::toggle_collapse))
            .on_action(cx.listener(Self::toggle_whitespace));
        let rows = self.items.len();
        let list = if self.inline {
            uniform_list(
                "diff-inline-rows",
                rows,
                cx.processor(move |this, range: Range<usize>, _, cx| range.map(|ix| this.list_item(ix, st, cx)).collect::<Vec<_>>()),
            )
        } else {
            uniform_list(
                "diff-rows",
                rows,
                cx.processor(move |this, range: Range<usize>, _, cx| range.map(|ix| this.list_item(ix, st, cx)).collect::<Vec<_>>()),
            )
        };
        let shows_list = !empty && self.unsupported.is_none();
        self.register_stage_actions(root, cx)
            .size_full()
            .flex()
            .flex_col()
            .bg(c(p.bg))
            .child(self.header(status, p, cx))
            .children(self.titles_band(st))
            .children(self.notice_strip(p))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .font_family(font.clone())
                            .text_size(px(font_size))
                            .line_height(px(line_h))
                            // The list scrolls first; the ruler's thumb follows once it has. Sideways, both sides' code moves
                            // together while the line numbers stay.
                            .on_scroll_wheel(cx.listener(move |this, ev: &gpui::ScrollWheelEvent, _, cx| {
                                let d = ev.delta.pixel_delta(px(line_h));
                                let (dx, dy) = (f32::from(d.x), f32::from(d.y));
                                // Only a mostly sideways move scrolls sideways: a vertical swipe carries a little x too.
                                let dx = if ev.modifiers.shift && dx == 0. {
                                    dy
                                } else if dx.abs() > dy.abs() {
                                    dx
                                } else {
                                    0.
                                };
                                this.h_scroll = (this.h_scroll - dx).clamp(0., h_max);
                                cx.notify();
                            }))
                            .child({
                                let view_w = self.view_w.clone();
                                gpui::canvas(move |bounds, _, _| view_w.set(f32::from(bounds.size.width)), |_, _, _, _| {}).absolute().size_full()
                            })
                            .when(empty, |d| d.child(self.empty_state(p)))
                            .when_some(self.unsupported, |d, why| d.child(self.unsupported_state(why, p)))
                            .when(shows_list, |d| {
                                d.child(list.track_scroll(self.scroll.clone()).size_full().map(|mut l| {
                                    // A sideways swipe scrolls the code, not the list.
                                    l.style().restrict_scroll_to_axis = Some(true);
                                    l
                                }))
                            }),
                    )
                    .when(shows_list && !self.items.is_empty(), |d| d.child(self.ruler(line_h, p, cx))),
            )
    }
}
