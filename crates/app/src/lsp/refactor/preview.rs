//! The review page of a refactoring ("Refactor Preview"): every file a language server wants to change with each change as
//! its old and new line, and every file operation (create, rename, delete) as a row of its own. Files and single changes
//! are selected with checkboxes; nothing is written before Apply Selected. The page only shows state and asks through
//! `RefactorEvent`; the workbench reads open editors, writes and undoes (`workbench_refactor.rs`).
//!
//! Keys: Up and Down move through the rows, Space selects, Enter opens the change in its file (or folds a file), Left and
//! Right fold and unfold, the modifier+Enter applies and Escape discards. Rows are a tree for assistive technology:
//! files and operations at level 1, their change lines at level 2.

use std::collections::HashSet;
use std::sync::Arc;

use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, HighlightStyle, KeyBinding, MouseButton, StyledText, UniformListScrollHandle, Window, div, prelude::*,
    px, uniform_list,
};

use super::plan::{Blocker, Chosen, Item, RefactorPlan, Resource};
use crate::replace_plan::Tri;
use crate::search_editor::{checkbox, window_around};
use crate::ui::{self, c, icon, layout, palette, text};

gpui::actions!(refactor_preview, [NextRow, PrevRow, ToggleRow, ActivateRow, ExpandRow, CollapseRow, ApplySelected, Discard]);

/// The keys of the review page (its view has the key contexts `RefactorPreview` and, for the list, `RefactorList`). `cyx` lists
/// them as built-in keys too (its `BINDING_SOURCES`).
pub fn key_bindings() -> [KeyBinding; 8] {
    let list = Some("RefactorList");
    let page = Some("RefactorPreview");
    [
        KeyBinding::new("down", NextRow, list),
        KeyBinding::new("up", PrevRow, list),
        KeyBinding::new("space", ToggleRow, list),
        KeyBinding::new("enter", ActivateRow, list),
        KeyBinding::new("right", ExpandRow, list),
        KeyBinding::new("left", CollapseRow, list),
        KeyBinding::new("secondary-enter", ApplySelected, page),
        KeyBinding::new("escape", Discard, page),
    ]
}

/// Unchanged lines shown around a change, at most.
const MAX_CONTEXT: u8 = 3;
const DEFAULT_CONTEXT: u8 = 1;
/// Characters of a changed text a screen reader is given (the page shows the rest).
const SPOKEN_CHARS: usize = 80;

pub enum RefactorEvent {
    /// Write the selection (the workbench checks that nothing went stale).
    Apply,
    /// Close the page without writing.
    Discard,
    /// Take back the text changes just written.
    Undo,
    /// Stop an apply between steps; what was done is taken back.
    Stop,
    /// Go to a change: absolute path, 1-based line, 0-based column, length (characters).
    Open { path: String, line: u32, column: u32, len: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Review,
    /// Selected deletes wait for a second, explicit go.
    Confirm,
    Applying,
    /// Applied; the report is what is left.
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    Item(usize),
    /// An unchanged line `k` lines before (k < 0) or after (k > 0) a change.
    Context(usize, usize, i8),
    Old(usize, usize),
    New(usize, usize),
}

pub struct RefactorPreview {
    title: String,
    plan: Arc<RefactorPlan>,
    chosen: Chosen,
    collapsed: HashSet<usize>,
    rows: Vec<Row>,
    context: u8,
    cursor: Option<usize>,
    scroll: UniformListScrollHandle,
    painted: crate::painted::WindowBounds<Frame>,
    focus: FocusHandle,
    list_focus: FocusHandle,
    phase: Phase,
    report: Vec<(String, bool)>,
    can_undo: bool,
}

impl EventEmitter<RefactorEvent> for RefactorPreview {}

impl Focusable for RefactorPreview {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        if self.phase == Phase::Done { self.focus.clone() } else { self.list_focus.clone() }
    }
}

impl RefactorPreview {
    pub fn new(title: String, plan: Arc<RefactorPlan>, cx: &mut Context<Self>) -> Self {
        let chosen = Chosen::initial(&plan);
        let mut this = RefactorPreview {
            title,
            plan,
            chosen,
            collapsed: HashSet::new(),
            rows: Vec::new(),
            context: DEFAULT_CONTEXT,
            cursor: None,
            scroll: UniformListScrollHandle::new(),
            painted: Default::default(),
            focus: cx.focus_handle(),
            list_focus: cx.focus_handle(),
            phase: Phase::Review,
            report: Vec::new(),
            can_undo: false,
        };
        this.rebuild_rows();
        this.cursor = (!this.rows.is_empty()).then_some(0);
        this
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn chosen(&self) -> &Chosen {
        &self.chosen
    }

    fn rebuild_rows(&mut self) {
        self.rows.clear();
        for (i, item) in self.plan.items.iter().enumerate() {
            self.rows.push(Row::Item(i));
            let Item::File(f) = item else { continue };
            if self.collapsed.contains(&i) || f.file.problem.is_some() {
                continue;
            }
            // A line is shown once: as a change, or as the context of the first change that reaches it. Context stops at a line
            // that is shown already, so the rows stay in the order of the file.
            let mut shown: HashSet<u32> = f.file.changes.iter().map(|c| c.line).collect();
            let changes = &f.file.changes;
            for ch in 0..changes.len() {
                // Several changes on one line: the context goes before the first and after the last.
                let (first, last) = (ch == 0 || changes[ch - 1].line != changes[ch].line, changes.get(ch + 1).is_none_or(|n| n.line != changes[ch].line));
                let mut before = Vec::new();
                for k in (1..=self.context as i8).take_while(|_| first) {
                    match f.file.neighbour_line(ch, -(k as isize)) {
                        Some((line, _)) if shown.insert(line) => before.push(Row::Context(i, ch, -k)),
                        _ => break,
                    }
                }
                self.rows.extend(before.into_iter().rev());
                self.rows.push(Row::Old(i, ch));
                self.rows.push(Row::New(i, ch));
                for k in (1..=self.context as i8).take_while(|_| last) {
                    match f.file.neighbour_line(ch, k as isize) {
                        Some((line, _)) if shown.insert(line) => self.rows.push(Row::Context(i, ch, k)),
                        _ => break,
                    }
                }
            }
        }
        if self.cursor.is_some_and(|c| c >= self.rows.len()) {
            self.cursor = self.rows.len().checked_sub(1);
        }
    }

    // --- Selection and phases ---

    /// (selected changes, files with a selected change, selected file operations).
    fn counts(&self) -> (usize, usize, usize) {
        let (mut changes, mut files, mut ops) = (0, 0, 0);
        for (i, item) in self.plan.items.iter().enumerate() {
            let n = self.chosen.item(i).iter().filter(|x| **x).count();
            match item {
                Item::File(_) if n > 0 => {
                    changes += n;
                    files += 1;
                }
                Item::Resource(_) => ops += n,
                Item::File(_) => {}
            }
        }
        (changes, files, ops)
    }

    fn apply_label(&self) -> String {
        crate::i18n::t("Apply Selected").to_string()
    }

    fn blocker(&self) -> Option<String> {
        match self.chosen.blocker(&self.plan)? {
            Blocker::NeedsOperation(path) => Some(crate::i18n::tf("{} needs a file operation that is not selected.", &[&self.display(&path)])),
            Blocker::Nothing => None,
        }
    }

    fn can_apply(&self) -> bool {
        self.phase == Phase::Review && self.chosen.counts().0 > 0 && self.chosen.blocker(&self.plan).is_none()
    }

    fn deletes(&self) -> Vec<String> {
        self.chosen
            .selected_resources(&self.plan)
            .into_iter()
            .filter_map(|r| match &r.op {
                Resource::Delete { path, .. } if !r.noop => Some(self.display(path)),
                _ => None,
            })
            .collect()
    }

    /// Apply was asked for: deletes need a second go, anything else is written.
    pub fn request_apply(&mut self, cx: &mut Context<Self>) {
        match self.phase {
            Phase::Review if self.can_apply() => {
                if self.deletes().is_empty() {
                    self.phase = Phase::Applying;
                    cx.emit(RefactorEvent::Apply);
                } else {
                    self.phase = Phase::Confirm;
                }
            }
            Phase::Confirm => {
                self.phase = Phase::Applying;
                cx.emit(RefactorEvent::Apply);
            }
            _ => {}
        }
        cx.notify();
    }

    /// The Undo button: the workbench takes back the text changes just written.
    pub fn request_undo(&mut self, cx: &mut Context<Self>) {
        if self.can_undo && self.phase == Phase::Done {
            cx.emit(RefactorEvent::Undo);
        }
    }

    /// Right: unfold the file the keyboard row belongs to.
    fn expand_cursor(&mut self, cx: &mut Context<Self>) {
        if let Some(ix) = self.cursor {
            self.fold_row(ix, false, cx);
        }
    }

    /// Left: on a file, fold it; on one of its lines, go to the file.
    fn collapse_cursor(&mut self, cx: &mut Context<Self>) {
        let Some(ix) = self.cursor else { return };
        match self.rows.get(ix).copied() {
            Some(Row::Item(i)) => self.fold(i, true, cx),
            Some(_) => {
                if let Some(i) = self.row_item(ix) {
                    self.cursor = self.rows.iter().position(|r| *r == Row::Item(i));
                    cx.notify();
                }
            }
            None => {}
        }
    }

    /// Fold or unfold the file a row belongs to.
    pub fn fold_row(&mut self, ix: usize, folded: bool, cx: &mut Context<Self>) {
        if let Some(i) = self.row_item(ix) {
            self.fold(i, folded, cx);
        }
    }

    pub fn request_discard(&mut self, cx: &mut Context<Self>) {
        match self.phase {
            Phase::Confirm => self.phase = Phase::Review,
            Phase::Applying => cx.emit(RefactorEvent::Stop),
            Phase::Review | Phase::Done => cx.emit(RefactorEvent::Discard),
        }
        cx.notify();
    }

    /// The workbench finished an apply (`applied`) or took everything back after a failure. `lines` say what happened.
    pub fn set_report(&mut self, lines: Vec<(String, bool)>, applied: bool, can_undo: bool, cx: &mut Context<Self>) {
        self.report = lines;
        self.can_undo = can_undo;
        self.phase = if applied { Phase::Done } else { Phase::Review };
        if applied {
            self.rows.clear();
            self.cursor = None;
        }
        cx.notify();
    }

    /// The text changes were taken back (or could not be): the report replaces the earlier one.
    pub fn set_undone(&mut self, lines: Vec<(String, bool)>, cx: &mut Context<Self>) {
        self.report = lines;
        self.can_undo = false;
        cx.notify();
    }

    /// Something the plan was made from went stale: nothing can be applied from this page any more.
    pub fn set_stale(&mut self, why: String, cx: &mut Context<Self>) {
        self.report = vec![(why, true)];
        self.phase = Phase::Done;
        self.can_undo = false;
        self.rows.clear();
        self.cursor = None;
        cx.notify();
    }

    fn toggle_item(&mut self, i: usize, cx: &mut Context<Self>) {
        if self.phase == Phase::Review {
            self.chosen.toggle_item(&self.plan, i);
            cx.notify();
        }
    }

    fn toggle_change(&mut self, i: usize, ch: usize, cx: &mut Context<Self>) {
        if self.phase == Phase::Review {
            self.chosen.toggle_part(&self.plan, i, ch);
            cx.notify();
        }
    }

    fn set_context(&mut self, n: u8, cx: &mut Context<Self>) {
        let n = n.min(MAX_CONTEXT);
        if n != self.context {
            self.context = n;
            self.rebuild_rows();
            cx.notify();
        }
    }

    fn fold(&mut self, i: usize, folded: bool, cx: &mut Context<Self>) {
        if matches!(self.plan.items.get(i), Some(Item::File(f)) if f.file.problem.is_none()) && self.collapsed.contains(&i) != folded {
            if folded {
                self.collapsed.insert(i);
            } else {
                self.collapsed.remove(&i);
            }
            self.rebuild_rows();
            cx.notify();
        }
    }

    // --- Keyboard ---

    fn move_cursor(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.rows.len();
        if n == 0 {
            return;
        }
        let next = match self.cursor {
            Some(i) => (i as isize + delta).clamp(0, n as isize - 1) as usize,
            None if delta < 0 => n - 1,
            None => 0,
        };
        self.select_row(next, cx);
    }

    pub fn select_row(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.rows.len() {
            self.cursor = Some(ix);
            self.scroll.scroll_to_item(ix, gpui::ScrollStrategy::Center);
            cx.notify();
        }
    }

    pub fn focus_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.list_focus);
        if self.cursor.is_none() && !self.rows.is_empty() {
            self.cursor = Some(0);
        }
        cx.notify();
    }

    pub fn toggle_row(&mut self, ix: usize, cx: &mut Context<Self>) {
        match self.rows.get(ix).copied() {
            Some(Row::Item(i)) => self.toggle_item(i, cx),
            Some(Row::Old(i, ch) | Row::New(i, ch)) => self.toggle_change(i, ch, cx),
            Some(Row::Context(..)) | None => {}
        }
    }

    /// Enter, a click or an accessibility click on a row: a file folds, a change opens its file, an operation is selected.
    pub fn activate_row(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.cursor = Some(ix);
        match self.rows.get(ix).copied() {
            Some(Row::Item(i)) => match self.plan.items.get(i) {
                Some(Item::File(f)) if f.file.problem.is_none() => self.fold(i, !self.collapsed.contains(&i), cx),
                Some(Item::File(f)) => cx.emit(RefactorEvent::Open { path: f.file.path.clone(), line: 1, column: 0, len: 0 }),
                Some(Item::Resource(_)) => self.toggle_item(i, cx),
                None => {}
            },
            Some(Row::Old(i, ch) | Row::New(i, ch)) => self.open_change(i, ch, cx),
            Some(Row::Context(i, ch, k)) => {
                if let Some(Item::File(f)) = self.plan.items.get(i)
                    && let Some((line, _)) = f.file.neighbour_line(ch, k as isize)
                {
                    cx.emit(RefactorEvent::Open { path: f.file.path.clone(), line, column: 0, len: 0 });
                }
            }
            None => {}
        }
        cx.notify();
    }

    fn open_change(&mut self, i: usize, ch: usize, cx: &mut Context<Self>) {
        let Some(Item::File(f)) = self.plan.items.get(i) else { return };
        let Some(change) = f.file.changes.get(ch) else { return };
        let (old, _, range, _) = f.file.line_diff(ch, &[]);
        let column = old.get(..range.start).map_or(0, |s| s.chars().count() as u32);
        cx.emit(RefactorEvent::Open { path: f.file.path.clone(), line: change.line, column, len: change.old.chars().count() as u32 });
    }

    fn row_item(&self, ix: usize) -> Option<usize> {
        match self.rows.get(ix)? {
            Row::Item(i) | Row::Context(i, ..) | Row::Old(i, _) | Row::New(i, _) => Some(*i),
        }
    }

    // --- Words ---

    /// A path as shown: relative to the folder when it is inside it.
    fn display(&self, path: &str) -> String {
        let root = self.plan.root.replace('\\', "/");
        let path = path.replace('\\', "/");
        path.strip_prefix(&format!("{}/", root.trim_end_matches('/'))).map(str::to_string).unwrap_or(path)
    }

    /// (label, description) of an item row.
    fn item_words(&self, i: usize) -> (String, String) {
        let state = |tri: Tri| match tri {
            Tri::All => crate::i18n::t("selected"),
            Tri::Some => crate::i18n::t("partly selected"),
            Tri::None => crate::i18n::t("not selected"),
        };
        match &self.plan.items[i] {
            Item::File(f) => {
                let detail = match &f.file.problem {
                    Some(p) => p.message(),
                    None => {
                        let on = self.chosen.item(i).iter().filter(|x| **x).count();
                        crate::i18n::tf("{0} of {1} changes", &[&on, &f.file.changes.len()])
                    }
                };
                (self.display(&f.file.path), detail)
            }
            Item::Resource(r) => {
                let label = match &r.op {
                    Resource::Create { path } => crate::i18n::tf("Create {}", &[&self.display(path)]),
                    Resource::Rename { from, to } => crate::i18n::tf("Rename {0} to {1}", &[&self.display(from), &self.display(to)]),
                    Resource::Delete { path, .. } => crate::i18n::tf("Delete {}", &[&self.display(path)]),
                };
                let detail = if let Some(why) = &r.problem {
                    why.clone()
                } else if r.noop {
                    crate::i18n::t("Nothing to do: it is already so.").to_string()
                } else {
                    match (&r.op, &r.resolved) {
                        (Resource::Delete { dir: true, .. }, _) => {
                            crate::i18n::t("Deletes the folder and everything in it for good; it is not moved to the trash.").to_string()
                        }
                        (Resource::Delete { .. }, _) => crate::i18n::t("Deletes the file for good; it is not moved to the trash.").to_string(),
                        (_, Some(real)) => crate::i18n::tf("Real location: {}", &[real]),
                        _ => state(self.chosen.state(i)).to_string(),
                    }
                };
                (label, detail)
            }
        }
    }

    /// What assistive technology reads for a row.
    pub fn row_words(&self, ix: usize) -> RowWords {
        let none = RowWords { label: String::new(), detail: None, level: 1, expanded: None, toggled: None };
        let Some(row) = self.rows.get(ix).copied() else { return none };
        match row {
            Row::Item(i) => {
                let (label, detail) = self.item_words(i);
                let usable = self.plan.items[i].usable();
                let foldable = matches!(&self.plan.items[i], Item::File(f) if f.file.problem.is_none());
                RowWords {
                    label,
                    detail: Some(detail),
                    level: 1,
                    expanded: foldable.then(|| !self.collapsed.contains(&i)),
                    toggled: usable.then(|| self.chosen.state(i)),
                }
            }
            Row::Old(i, ch) => {
                let Item::File(f) = &self.plan.items[i] else { return none };
                let Some(change) = f.file.changes.get(ch) else { return none };
                let (line, _, range, _) = f.file.line_diff(ch, &[]);
                let column = line.get(..range.start).map_or(0, |s| s.chars().count()) + 1;
                let short = |text: &str| {
                    let flat = text.replace("\r\n", "↵").replace(['\n', '\r'], "↵");
                    if flat.chars().count() > SPOKEN_CHARS { format!("{}…", flat.chars().take(SPOKEN_CHARS).collect::<String>()) } else { flat }
                };
                RowWords {
                    label: crate::i18n::tf("Line {0}, column {1}: “{2}” → “{3}”", &[&change.line, &column, &short(&change.old), &short(&change.new)]),
                    detail: Some(line.trim().to_string()),
                    level: 2,
                    expanded: None,
                    toggled: Some(if self.chosen.item(i).get(ch).copied().unwrap_or(false) { Tri::All } else { Tri::None }),
                }
            }
            Row::New(i, ch) => {
                let Item::File(f) = &self.plan.items[i] else { return none };
                RowWords {
                    label: crate::i18n::tf("Line {} after the change", &[&f.file.changes.get(ch).map_or(0, |c| c.line)]),
                    detail: None,
                    level: 2,
                    expanded: None,
                    toggled: None,
                }
            }
            Row::Context(i, ch, k) => {
                let Item::File(f) = &self.plan.items[i] else { return none };
                let (line, text) = f.file.neighbour_line(ch, k as isize).unwrap_or_default();
                RowWords {
                    label: crate::i18n::tf("Line {0}: {1}", &[&line, &text.trim()]),
                    detail: Some(crate::i18n::t("context").to_string()),
                    level: 2,
                    expanded: None,
                    toggled: None,
                }
            }
        }
    }

    /// What is selected, in words; or why the selection cannot be applied.
    fn status_text(&self) -> (String, bool) {
        if let Some(why) = self.blocker() {
            return (why, true);
        }
        let (changes, files, ops) = self.counts();
        let c = crate::i18n::tn("{} change", "{} changes", changes, &[]);
        let f = crate::i18n::tn("in {} file", "in {} files", files, &[]);
        let o = crate::i18n::tn("{} file operation", "{} file operations", ops, &[]);
        let text = match (changes > 0, ops > 0) {
            (true, false) => crate::i18n::tf("Selected: {0} {1}.", &[&c, &f]),
            (true, true) => crate::i18n::tf("Selected: {0} {1}, {2}.", &[&c, &f, &o]),
            (false, true) => crate::i18n::tf("Selected: {}.", &[&o]),
            (false, false) => crate::i18n::t("Nothing selected.").to_string(),
        };
        (text, false)
    }

    /// What the whole refactoring proposes, in words.
    fn summary(&self) -> String {
        let changes = self.plan.change_count();
        let files = self.plan.files().filter(|f| !f.file.changes.is_empty()).count();
        let ops = self.plan.resources().count();
        let c = crate::i18n::tn("{} change", "{} changes", changes, &[]);
        let f = crate::i18n::tn("in {} file", "in {} files", files, &[]);
        let o = crate::i18n::tn("{} file operation", "{} file operations", ops, &[]);
        match (changes > 0, ops > 0) {
            (true, false) => crate::i18n::tf("Proposed: {0} {1}.", &[&c, &f]),
            (true, true) => crate::i18n::tf("Proposed: {0} {1}, {2}.", &[&c, &f, &o]),
            (false, _) => crate::i18n::tf("Proposed: {}.", &[&o]),
        }
    }

    // --- Accessibility and tests ---

    /// What assistive technology sees: the status, the rows around the keyboard row (at most `max`) and the buttons.
    pub fn access_view(&self, max: usize, window: &Window) -> AccessView {
        let total = self.rows.len();
        let start = self.cursor.unwrap_or(0).saturating_sub(max / 2).min(total.saturating_sub(max));
        let rows: Vec<_> = (start..(start + max).min(total)).map(|i| self.row_words(i)).collect();
        let frame = |k: Frame| self.painted.physical(&k, window);
        let (status, bad) = self.status_text();
        AccessView {
            title: self.title.clone(),
            summary: self.summary(),
            status: if self.phase == Phase::Confirm { self.confirm_text() } else { status },
            problem: bad || self.phase == Phase::Confirm,
            report: self.report.iter().map(|(l, _)| l.clone()).collect(),
            bounds: AccessBounds {
                page: frame(Frame::Page),
                status: frame(Frame::Status),
                list: frame(Frame::List),
                rows: (start..start + rows.len()).map(|i| frame(Frame::Row(i))).collect(),
                apply: frame(Frame::Apply),
                discard: frame(Frame::Discard),
                undo: frame(Frame::Undo),
            },
            rows,
            first_row: start,
            rows_total: total,
            selected: self.cursor,
            apply: (self.phase != Phase::Done).then(|| {
                let (label, enabled) = match self.phase {
                    Phase::Confirm => (crate::i18n::t("Delete and Apply").to_string(), true),
                    Phase::Applying => (self.apply_label(), false),
                    _ => (self.apply_label(), self.can_apply()),
                };
                (label, enabled)
            }),
            discard_label: match self.phase {
                Phase::Confirm => crate::i18n::t("Back").to_string(),
                Phase::Applying => crate::i18n::t("Stop").to_string(),
                Phase::Done => crate::i18n::t("Close").to_string(),
                Phase::Review => crate::i18n::t("Discard").to_string(),
            },
            can_undo: self.can_undo && self.phase == Phase::Done,
            list_focused: self.list_focus.is_focused(window),
            busy: self.phase == Phase::Applying,
        }
    }

    fn confirm_text(&self) -> String {
        let names = self.deletes();
        crate::i18n::tn(
            "Delete {0} item for good? It is not moved to the trash: {1}",
            "Delete {0} items for good? They are not moved to the trash: {1}",
            names.len(),
            &[&names.join(", ")],
        )
    }

    #[cfg(test)]
    pub fn rows_for_test(&self) -> Vec<String> {
        (0..self.rows.len())
            .map(|i| {
                let w = self.row_words(i);
                let mark = match w.toggled {
                    Some(Tri::All) => "[x] ",
                    Some(Tri::Some) => "[-] ",
                    Some(Tri::None) => "[ ] ",
                    None => "",
                };
                format!("{}{mark}{}", "  ".repeat(w.level - 1), w.label)
            })
            .collect()
    }

    #[cfg(test)]
    pub fn row_words_for_test(&self, ix: usize) -> String {
        let w = self.row_words(ix);
        format!("{} {}", w.label, w.detail.unwrap_or_default())
    }

    #[cfg(test)]
    pub fn status_for_test(&self) -> String {
        self.status_text().0
    }

    #[cfg(test)]
    pub fn apply_label_for_test(&self) -> String {
        self.apply_label()
    }

    #[cfg(test)]
    pub fn report_for_test(&self) -> Vec<String> {
        self.report.iter().map(|(l, _)| l.clone()).collect()
    }

    #[cfg(test)]
    pub fn phase_for_test(&self) -> &'static str {
        match self.phase {
            Phase::Review => "review",
            Phase::Confirm => "confirm",
            Phase::Applying => "applying",
            Phase::Done => "done",
        }
    }

    #[cfg(test)]
    pub fn press_for_test(&mut self, key: &str, cx: &mut Context<Self>) {
        match key {
            "down" => self.move_cursor(1, cx),
            "up" => self.move_cursor(-1, cx),
            "space" => {
                if let Some(i) = self.cursor {
                    self.toggle_row(i, cx)
                }
            }
            "enter" => {
                if let Some(i) = self.cursor {
                    self.activate_row(i, cx)
                }
            }
            "right" => self.expand_cursor(cx),
            "left" => self.collapse_cursor(cx),
            "apply" => self.request_apply(cx),
            "escape" => self.request_discard(cx),
            other => panic!("unknown key {other}"),
        }
    }

    #[cfg(test)]
    pub fn deletes_for_test(&self) -> Vec<String> {
        self.deletes()
    }

    #[cfg(test)]
    pub fn cursor_for_test(&self) -> Option<usize> {
        self.cursor
    }

    #[cfg(test)]
    pub fn chosen_for_test(&self) -> Chosen {
        self.chosen.clone()
    }

    #[cfg(test)]
    pub fn set_context_for_test(&mut self, n: u8, cx: &mut Context<Self>) {
        self.set_context(n, cx);
    }
}

/// What a row says to assistive technology.
#[derive(Clone, Debug, PartialEq)]
pub struct RowWords {
    pub label: String,
    pub detail: Option<String>,
    /// 1: a file or an operation; 2: a line of a change.
    pub level: usize,
    /// `Some` for a row that folds.
    pub expanded: Option<bool>,
    /// `Some` for a row with a checkbox.
    pub toggled: Option<Tri>,
}

/// A part of the page whose painted bounds the accessibility tree reports.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Frame {
    Page,
    Status,
    List,
    Row(usize),
    Apply,
    Discard,
    Undo,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct AccessBounds {
    pub page: Option<accesskit::Rect>,
    pub status: Option<accesskit::Rect>,
    pub list: Option<accesskit::Rect>,
    pub rows: Vec<Option<accesskit::Rect>>,
    pub apply: Option<accesskit::Rect>,
    pub discard: Option<accesskit::Rect>,
    pub undo: Option<accesskit::Rect>,
}

/// What the accessibility tree shows of the page.
#[derive(Clone, Debug, PartialEq)]
pub struct AccessView {
    pub bounds: AccessBounds,
    pub title: String,
    pub summary: String,
    pub status: String,
    /// The status is a warning (a blocker, or the question before a delete).
    pub problem: bool,
    pub report: Vec<String>,
    pub rows: Vec<RowWords>,
    pub first_row: usize,
    pub rows_total: usize,
    pub selected: Option<usize>,
    /// The apply button's label and whether it can be pressed; `None` once applied.
    pub apply: Option<(String, bool)>,
    /// "Discard", or "Back" at the question before a delete, or "Stop" while writing.
    pub discard_label: String,
    pub can_undo: bool,
    pub list_focused: bool,
    pub busy: bool,
}

impl Render for RefactorPreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        self.painted.begin(window);
        let (status, status_bad) = self.status_text();
        let confirming = self.phase == Phase::Confirm;
        let button = |id: &'static str, label: String, enabled: bool, primary: bool, danger: bool| {
            let (bg, fg) = match (enabled, primary) {
                (true, true) => (if danger { p.danger } else { p.accent }, p.on_accent),
                (true, false) => (p.surface_hover, p.text),
                (false, _) => (p.surface, p.text_muted),
            };
            div()
                .id(id)
                .relative()
                .flex_none()
                .h(px(24.))
                .px(px(theme::SPACE[4]))
                .flex()
                .items_center()
                .rounded(px(theme::radius::SM))
                .bg(c(bg))
                .text_color(c(fg))
                .text_size(px(text::XS.0))
                .when(enabled, |d| d.cursor_pointer())
                .when(!enabled, |d| d.border_1().border_color(c(p.border)))
                .child(label)
        };
        let modifier = if cfg!(target_os = "macos") { "⌘" } else { "Ctrl+" };
        let apply_label = if confirming { crate::i18n::t("Delete and Apply").to_string() } else { self.apply_label() };
        let apply_enabled = confirming || self.can_apply();
        let discard_label = match self.phase {
            Phase::Confirm => crate::i18n::t("Back"),
            Phase::Applying => crate::i18n::t("Stop"),
            _ => crate::i18n::t("Discard"),
        };
        let rows = self.rows.len();

        div()
            .id("refactor-preview")
            .relative()
            .child(self.painted.probe(Frame::Page))
            .key_context("RefactorPreview")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ApplySelected, _, cx| this.request_apply(cx)))
            .on_action(cx.listener(|this, _: &Discard, _, cx| this.request_discard(cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(c(p.bg))
            .text_size(px(text::SM.0))
            .text_color(c(p.text))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(theme::SPACE[3]))
                    .px(px(theme::SPACE[6]))
                    .py(px(theme::SPACE[4]))
                    .border_b_1()
                    .border_color(c(p.border))
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(self.title.clone()))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(theme::SPACE[3]))
                            .text_size(px(text::XS.0))
                            .text_color(c(p.text_muted))
                            .child(self.summary())
                            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(format!("· {} · {}", self.plan.host, self.plan.root))),
                    )
                    .when(self.phase != Phase::Done, |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(px(theme::SPACE[3]))
                                .child(
                                    button("rp-apply", apply_label, apply_enabled && self.phase != Phase::Applying, true, confirming)
                                        .child(self.painted.probe(Frame::Apply))
                                        .tooltip(move |_, cx| {
                                            cx.new(|_| {
                                                crate::workbench::Tooltip(
                                                    crate::i18n::tf("Write the selected changes; nothing else is touched ({}Enter)", &[&modifier]).into(),
                                                )
                                            })
                                            .into()
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| this.request_apply(cx))),
                                )
                                .child(
                                    button("rp-discard", discard_label.to_string(), true, false, false)
                                        .child(self.painted.probe(Frame::Discard))
                                        .on_click(cx.listener(|this, _, _, cx| this.request_discard(cx))),
                                )
                                .when(self.phase == Phase::Review, |d| {
                                    d.child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(theme::SPACE[2]))
                                            .text_size(px(text::XS.0))
                                            .text_color(c(p.text_muted))
                                            .child(crate::i18n::tn("Context: {} line", "Context: {} lines", self.context as usize, &[]))
                                            .child(ui::icon_button("rp-context-less", "minus", crate::i18n::t("Fewer context lines"), &p).on_click(cx.listener(
                                                |this, _, _, cx| {
                                                    let n = this.context.saturating_sub(1);
                                                    this.set_context(n, cx)
                                                },
                                            )))
                                            .child(ui::icon_button("rp-context-more", "plus", crate::i18n::t("More context lines"), &p).on_click(cx.listener(
                                                |this, _, _, cx| {
                                                    let n = this.context + 1;
                                                    this.set_context(n, cx)
                                                },
                                            ))),
                                    )
                                }),
                        )
                    }),
            )
            .when(self.phase == Phase::Review || confirming, |d| {
                d.child(
                    div()
                        .id("rp-status")
                        .relative()
                        .child(self.painted.probe(Frame::Status))
                        .flex_none()
                        .px(px(theme::SPACE[6]))
                        .py(px(theme::SPACE[2]))
                        .text_size(px(text::XS.0))
                        .text_color(c(if status_bad || confirming { p.warning } else { p.text_muted }))
                        .child(if confirming { self.confirm_text() } else { status }),
                )
            })
            .children(self.plan.skipped.iter().take(5).map(|s| {
                div().flex_none().px(px(theme::SPACE[6])).py(px(theme::SPACE[1])).text_size(px(text::XS.0)).text_color(c(p.warning)).child(s.clone())
            }))
            .when(!self.report.is_empty(), |d| {
                d.child(
                    div()
                        .id("rp-report")
                        .flex_none()
                        .flex()
                        .flex_col()
                        .gap(px(theme::SPACE[1]))
                        .px(px(theme::SPACE[6]))
                        .py(px(theme::SPACE[3]))
                        .border_b_1()
                        .border_color(c(p.border))
                        .text_size(px(text::XS.0))
                        .children(self.report.iter().take(50).map(|(l, bad)| div().text_color(c(if *bad { p.warning } else { p.success })).child(l.clone())))
                        .when(self.phase == Phase::Done, |d| {
                            d.child(
                                div()
                                    .flex()
                                    .gap(px(theme::SPACE[3]))
                                    .when(self.can_undo, |d| {
                                        d.child(
                                            button("rp-undo", crate::i18n::t("Undo").to_string(), true, false, false)
                                                .child(self.painted.probe(Frame::Undo))
                                                .on_click(cx.listener(|this, _, _, cx| this.request_undo(cx))),
                                        )
                                    })
                                    .child(
                                        button("rp-close", crate::i18n::t("Close").to_string(), true, false, false)
                                            .child(self.painted.probe(Frame::Discard))
                                            .on_click(cx.listener(|this, _, _, cx| this.request_discard(cx))),
                                    ),
                            )
                        }),
                )
            })
            .when(self.phase != Phase::Done, |d| {
                d.child(
                    div()
                        .id("rp-list")
                        .relative()
                        .child(self.painted.probe(Frame::List))
                        .key_context("RefactorList")
                        .track_focus(&self.list_focus)
                        .on_action(cx.listener(|this, _: &NextRow, _, cx| this.move_cursor(1, cx)))
                        .on_action(cx.listener(|this, _: &PrevRow, _, cx| this.move_cursor(-1, cx)))
                        .on_action(cx.listener(|this, _: &ToggleRow, _, cx| {
                            if let Some(i) = this.cursor {
                                this.toggle_row(i, cx)
                            }
                        }))
                        .on_action(cx.listener(|this, _: &ActivateRow, _, cx| {
                            if let Some(i) = this.cursor {
                                this.activate_row(i, cx)
                            }
                        }))
                        .on_action(cx.listener(|this, _: &ExpandRow, _, cx| this.expand_cursor(cx)))
                        .on_action(cx.listener(|this, _: &CollapseRow, _, cx| this.collapse_cursor(cx)))
                        .flex_1()
                        .min_h_0()
                        .child(
                            uniform_list(
                                "rp-rows",
                                rows,
                                cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                                    let focused = this.list_focus.is_focused(window);
                                    range.map(|ix| this.render_row(ix, focused, &p, cx)).collect::<Vec<_>>()
                                }),
                            )
                            .track_scroll(self.scroll.clone())
                            .size_full(),
                        ),
                )
            })
            .when(self.phase == Phase::Review && rows > 0, |d| {
                d.child(
                    div()
                        .flex_none()
                        .px(px(theme::SPACE[6]))
                        .py(px(theme::SPACE[2]))
                        .border_t_1()
                        .border_color(c(p.border))
                        .text_size(px(text::XXS.0))
                        .text_color(c(p.text_muted))
                        .child(crate::i18n::tf(
                            "Up and Down move, Space selects, Enter opens, Left and Right fold, {}Enter applies, Escape discards. Nothing is written before Apply Selected.",
                            &[&modifier],
                        )),
                )
            })
    }
}

impl RefactorPreview {
    fn render_row(&self, ix: usize, list_focused: bool, p: &theme::Palette, cx: &mut Context<Self>) -> gpui::AnyElement {
        let p = *p;
        let current = self.cursor == Some(ix);
        let base = div()
            .id(("rp-row", ix))
            .relative()
            .child(self.painted.probe(Frame::Row(ix)))
            .debug_selector(move || format!("rp-row-{ix}"))
            .w_full()
            .h(px(layout::ROW_H))
            .flex()
            .items_center()
            .gap(px(theme::SPACE[3]))
            .px(px(theme::SPACE[6]))
            .whitespace_nowrap()
            .overflow_hidden()
            .cursor_pointer()
            .when(current, |d| d.bg(c(if list_focused { ui::focused_selection(&p) } else { p.surface_active })))
            .when(!current, |d| d.hover(|d| d.bg(c(p.surface_hover))))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    window.focus(&this.list_focus);
                    this.activate_row(ix, cx);
                }),
            );
        let line_no = |n: u32| div().flex_none().w(px(44.)).text_size(px(text::XS.0)).text_color(c(p.line_number)).child(n.to_string());
        let Some(row) = self.rows.get(ix).copied() else { return base.into_any_element() };
        match row {
            Row::Item(i) => {
                let usable = self.plan.items[i].usable();
                let (label, detail) = self.item_words(i);
                let problem = match &self.plan.items[i] {
                    Item::File(f) => f.file.problem.is_some(),
                    Item::Resource(r) => r.problem.is_some(),
                };
                let check = checkbox(("rp-check", i), self.chosen.state(i), usable, &p).on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle_item(i, cx)
                }));
                match &self.plan.items[i] {
                    Item::File(f) => {
                        let folded = self.collapsed.contains(&i);
                        let (dir, name) = label.rsplit_once('/').map_or(("", label.as_str()), |(d, n)| (d, n));
                        base.child(icon(
                            if f.file.problem.is_some() {
                                "alert"
                            } else if folded {
                                "chevron-right"
                            } else {
                                "chevron-down"
                            },
                            layout::ICON,
                            p.text_muted,
                        ))
                        .child(check)
                        .child(crate::icon_theme::file_icon_el(ui::file_name(&f.file.path), layout::ICON, &p))
                        .child(div().flex_none().child(name.to_string()))
                        .child(div().flex_none().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(dir.to_string()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_size(px(text::XS.0))
                                .text_color(c(if problem { p.warning } else { p.text_muted }))
                                .child(detail),
                        )
                        .into_any_element()
                    }
                    Item::Resource(r) => {
                        let glyph = match r.op {
                            Resource::Create { .. } => "file-plus",
                            Resource::Rename { .. } => "pencil",
                            Resource::Delete { .. } => "trash",
                        };
                        let danger = matches!(r.op, Resource::Delete { .. });
                        base.child(div().flex_none().w(px(layout::ICON)))
                            .child(check)
                            .child(icon(glyph, layout::ICON, if danger && !problem { p.danger } else { p.text_muted }))
                            .child(div().flex_none().child(label))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_size(px(text::XS.0))
                                    .text_color(c(if problem || danger { p.warning } else { p.text_muted }))
                                    .child(detail),
                            )
                            .into_any_element()
                    }
                }
            }
            Row::Context(i, ch, k) => {
                let Item::File(f) = &self.plan.items[i] else { return base.into_any_element() };
                let Some((line, text)) = f.file.neighbour_line(ch, k as isize) else { return base.into_any_element() };
                base.pl(px(theme::SPACE[6] + layout::ICON + 16.))
                    .child(line_no(line))
                    .child(div().flex_none().w(px(10.)))
                    .child(div().min_w_0().text_color(c(p.text_muted)).child(text))
                    .into_any_element()
            }
            Row::Old(i, ch) => {
                let Item::File(f) = &self.plan.items[i] else { return base.into_any_element() };
                let sel = self.chosen.item(i);
                let on = sel.get(ch).copied().unwrap_or(false);
                let (old, _, range, _) = f.file.line_diff(ch, sel);
                let (old, range) = visible_breaks(&old, range);
                let (text, r) = window_around(&old, range);
                let hl = HighlightStyle {
                    background_color: Some(c(p.diff_removed).into()),
                    strikethrough: on.then(|| gpui::StrikethroughStyle { thickness: px(1.), color: Some(c(p.text).into()) }),
                    ..Default::default()
                };
                base.pl(px(theme::SPACE[6] + layout::ICON))
                    .child(checkbox(("rp-change-check", ix), if on { Tri::All } else { Tri::None }, f.file.problem.is_none(), &p).on_click(cx.listener(
                        move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle_change(i, ch, cx)
                        },
                    )))
                    .child(line_no(f.file.changes.get(ch).map_or(0, |c| c.line)))
                    .child(div().flex_none().text_color(c(p.danger)).child("−"))
                    .child(div().min_w_0().child(StyledText::new(text).with_highlights([(r, hl)])))
                    .into_any_element()
            }
            Row::New(i, ch) => {
                let Item::File(f) = &self.plan.items[i] else { return base.into_any_element() };
                let sel = self.chosen.item(i);
                let on = sel.get(ch).copied().unwrap_or(false);
                let (_, new, _, range) = f.file.line_diff(ch, sel);
                let (new, range) = visible_breaks(&new, range);
                let (text, r) = window_around(&new, range);
                let hl = HighlightStyle { background_color: Some(c(if on { p.diff_added } else { p.surface_hover }).into()), ..Default::default() };
                base.pl(px(theme::SPACE[6] + layout::ICON + 14. + theme::SPACE[3] + 44. + theme::SPACE[3]))
                    .child(div().flex_none().text_color(c(if on { p.success } else { p.text_muted })).child(if on { "+" } else { "=" }))
                    .child(div().min_w_0().text_color(c(if on { p.text } else { p.text_muted })).child(StyledText::new(text).with_highlights([(r, hl)])))
                    .into_any_element()
            }
        }
    }
}

/// A line shown on one row: its inner line breaks (a change over several lines) become "↵", and the range follows.
fn visible_breaks(line: &str, range: std::ops::Range<usize>) -> (String, std::ops::Range<usize>) {
    if !line.contains(['\n', '\r']) {
        return (line.to_string(), range);
    }
    let (mut out, mut start, mut end) = (String::with_capacity(line.len()), range.start, range.end);
    let mut chars = line.char_indices().peekable();
    while let Some((at, ch)) = chars.next() {
        if ch == '\r' || ch == '\n' {
            // "\r\n" is one break.
            let width = if ch == '\r' && chars.peek().is_some_and(|(_, n)| *n == '\n') { chars.next().map_or(1, |_| 2) } else { 1 };
            let grew = '↵'.len_utf8() as isize - width as isize;
            if at < range.start {
                start = (start as isize + grew) as usize;
            }
            if at < range.end {
                end = (end as isize + grew) as usize;
            }
            out.push('↵');
        } else {
            out.push(ch);
        }
    }
    (out, start..end)
}

#[cfg(test)]
#[path = "preview_tests.rs"]
mod tests;
