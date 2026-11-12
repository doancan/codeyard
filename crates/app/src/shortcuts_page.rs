//! "Keyboard Shortcuts": every command with the keys it has now, where each key comes from, and the keys several commands
//! share. A key is recorded with "Change" / "Add" (the page takes the next key presses itself, so no shortcut acts while
//! it listens) and written to `keybindings.jsonc` by the workbench (`workbench_shortcuts.rs`); "Remove" and "Reset" undo
//! it again.

use gpui::{App, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent, Keystroke, Subscription, Window, div, prelude::*, px};

use crate::input::TextField;
use crate::ui::{self, c, palette, text};

#[path = "shortcuts_edit.rs"]
pub mod edit;
#[path = "shortcuts_record.rs"]
mod record;
#[path = "shortcuts_rows.rs"]
pub mod rows;

#[cfg(test)]
#[path = "shortcuts_page_tests.rs"]
mod tests;

use record::{Recorder, Step};
use rows::{Binding, Row};

/// What the page asks the workbench to do. The workbench owns the file and the keymap.
pub enum ShortcutsEvent {
    /// The command's label: run it.
    Run(String),
    /// Give the command `keys`: in place of the key of `row` (`add` false) or as one more (`add` true).
    Set { row: Row, keys: String, add: bool },
    /// Drop the key of the row.
    Remove(Row),
    /// Undo everything `keybindings.jsonc` says about the row's command.
    Reset(Row),
    /// Open `keybindings.jsonc` in an editor.
    OpenFile,
}

/// The line under the header: what the last change did, or why it did not work.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub text: String,
    pub problem: bool,
}

/// A row as the accessibility tree shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct AccessRow {
    pub label: String,
    /// Keys, where they come from, their context and who else uses them, in one sentence.
    pub description: String,
    pub selected: bool,
}

pub struct AccessView {
    pub query: String,
    pub rows: Vec<AccessRow>,
    pub total: usize,
    pub search_focused: bool,
    /// What a screen reader should say now: the recording prompt, or the last change's result.
    pub status: Option<Status>,
}

/// Which row is meant, so a selection survives the list being rebuilt after a change.
type RowKey = (String, Option<String>, Option<String>);

fn key_of(row: &Row) -> RowKey {
    (row.label.clone(), row.binding.as_ref().map(|b| b.keys.clone()), row.binding.as_ref().and_then(|b| b.when.clone()))
}

struct Recording {
    row: RowKey,
    add: bool,
    recorder: Recorder,
    _listen: Subscription,
    _blur: Subscription,
}

pub struct ShortcutsPage {
    rows: Vec<Row>,
    query: Entity<TextField>,
    selected: Option<RowKey>,
    recording: Option<Recording>,
    status: Option<Status>,
    /// There is a settings folder to write to (not in tests that have none).
    editable: bool,
    /// Where each shown row (by its place in the list) was painted in the last frame (accessibility).
    bounds: crate::painted::RowBounds<usize>,
    focus_handle: FocusHandle,
    _sub: Subscription,
}

impl EventEmitter<ShortcutsEvent> for ShortcutsPage {}

impl Focusable for ShortcutsPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// A list narrower than this (in pixels) puts each row's key, source and context on a second line.
const NARROW: f32 = 700.;
/// The key `bounds` records the list's own frame under.
const LIST_FRAME: usize = usize::MAX;
const ROW_H: f32 = 34.;
const ROW_H_NARROW: f32 = 46.;
const KEYS_W: f32 = 190.;
const WHEN_W: f32 = 120.;
const SOURCE_W: f32 = 64.;
/// Room for the five buttons, so the columns line up whether a row shows them or not.
const BUTTONS_W: f32 = 130.;

impl ShortcutsPage {
    pub fn new(rows: Vec<Row>, editable: bool, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| TextField::new(crate::i18n::tr("Search commands or keys (e.g. “comment”, “⌘D”)"), cx));
        let sub = cx.observe(&query, |_, _, cx| cx.notify());
        ShortcutsPage {
            rows,
            query,
            selected: None,
            recording: None,
            status: None,
            editable,
            bounds: Default::default(),
            focus_handle: cx.focus_handle(),
            _sub: sub,
        }
    }

    pub fn focus_search(&self, window: &mut Window, cx: &App) {
        window.focus(&self.query.focus_handle(cx));
    }

    /// The list after a change: the selection stays on the same row when it is still there, and `status` says what happened.
    pub fn set_rows(&mut self, rows: Vec<Row>, status: Option<Status>, cx: &mut Context<Self>) {
        self.rows = rows;
        if self.selected.as_ref().is_some_and(|k| !self.rows.iter().any(|r| &key_of(r) == k)) {
            self.selected = None;
        }
        self.recording = None;
        self.status = status;
        cx.notify();
    }

    /// The list after the keymap changed under an open page (another window, a hand edit, a profile switch): recording and
    /// the last message stay.
    pub fn refresh_rows(&mut self, rows: Vec<Row>, cx: &mut Context<Self>) {
        self.rows = rows;
        if self.selected.as_ref().is_some_and(|k| !self.rows.iter().any(|r| &key_of(r) == k)) {
            self.selected = None;
        }
        cx.notify();
    }

    pub fn set_status(&mut self, status: Status, cx: &mut Context<Self>) {
        self.status = Some(status);
        cx.notify();
    }

    fn visible(&self, cx: &App) -> Vec<&Row> {
        let q = self.query.read(cx).text().to_string();
        self.rows.iter().filter(|r| rows::matches(r, &q)).collect()
    }

    fn can_change(&self, row: &Row) -> bool {
        self.editable && row.id.is_some()
    }

    /// Filters the list as typing `q` into the search field does.
    pub fn set_query(&mut self, q: &str, cx: &mut Context<Self>) {
        self.query.update(cx, |f, cx| f.set_text(q, cx));
    }

    // --- Selection ---

    pub fn select(&mut self, row: &Row, window: &mut Window, cx: &mut Context<Self>) {
        if self.recording.as_ref().is_some_and(|r| r.row != key_of(row)) {
            self.recording = None;
        }
        self.selected = Some(key_of(row));
        window.focus(&self.focus_handle);
        cx.notify();
    }

    /// Selects the `index`th shown row (accessibility actions and the keyboard name rows by place).
    pub fn select_visible(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.visible(cx).get(index).map(|r| (*r).clone()) {
            self.select(&row, window, cx);
        }
    }

    fn move_selection(&mut self, by: isize, cx: &mut Context<Self>) {
        let keys: Vec<RowKey> = self.visible(cx).iter().map(|r| key_of(r)).collect();
        if keys.is_empty() {
            return;
        }
        let at = self.selected.as_ref().and_then(|s| keys.iter().position(|k| k == s));
        let next = match at {
            None => {
                if by > 0 {
                    0
                } else {
                    keys.len() - 1
                }
            }
            Some(i) => (i as isize + by).clamp(0, keys.len() as isize - 1) as usize,
        };
        self.selected = Some(keys[next].clone());
        self.recording = None;
        cx.notify();
    }

    fn selected_row(&self) -> Option<Row> {
        let key = self.selected.as_ref()?;
        self.rows.iter().find(|r| &key_of(r) == key).cloned()
    }

    // --- Recording ---

    /// Starts listening for the keys of `row`: `add` gives the command one more key, otherwise the key of the row is
    /// replaced. Until Enter or Escape every key press goes to the page; nothing else reacts to it.
    pub fn start_recording(&mut self, row: &Row, add: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_change(row) {
            return;
        }
        self.select(row, window, cx);
        let me = cx.weak_entity();
        let focus = self.focus_handle.clone();
        let listen = cx.intercept_keystrokes(move |event, window, cx| {
            // A key typed in another window, or after the page lost focus, is not ours: recording ends and the key acts as usual.
            if !focus.contains_focused(window, cx) {
                me.update(cx, |page, cx| page.cancel_recording(cx)).ok();
                return;
            }
            cx.stop_propagation();
            me.update(cx, |page, cx| page.record_press(&event.keystroke, cx)).ok();
        });
        // Focus going elsewhere ends the recording and clears the prompt, as Escape does.
        let blur = cx.on_focus_out(&self.focus_handle, window, |page, _, _, cx| page.cancel_recording(cx));
        self.recording = Some(Recording { row: key_of(row), add, recorder: Recorder::default(), _listen: listen, _blur: blur });
        self.status = None;
        cx.notify();
    }

    pub fn cancel_recording(&mut self, cx: &mut Context<Self>) {
        if self.recording.take().is_some() {
            cx.notify();
        }
    }

    #[cfg(test)]
    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    /// One key press while recording.
    pub fn record_press(&mut self, key: &Keystroke, cx: &mut Context<Self>) {
        let Some(recording) = self.recording.as_mut() else { return };
        match recording.recorder.press(key) {
            Step::Continue => self.status = None,
            Step::Cancel => self.recording = None,
            Step::Refused => {
                let text = crate::i18n::t("A key that types text cannot be a shortcut on its own. Add Ctrl, Alt or Cmd, or press a function key.");
                self.status = Some(Status { text: text.to_string(), problem: true });
            }
            Step::Done(keys) => {
                let (add, key) = (recording.add, recording.row.clone());
                self.recording = None;
                if let Some(row) = self.rows.iter().find(|r| key_of(r) == key).cloned() {
                    cx.emit(ShortcutsEvent::Set { row, keys, add });
                }
            }
        }
        cx.notify();
    }

    /// The line under the header: while recording, a refused key's reason, else the prompt; otherwise the last result.
    fn shown_status(&self) -> Option<Status> {
        match (self.recording_prompt(), &self.status) {
            (Some(_), Some(s)) if s.problem => Some(s.clone()),
            (Some(prompt), _) => Some(Status { text: prompt, problem: false }),
            (None, status) => status.clone(),
        }
    }

    /// The prompt while listening: what to press, and the chords so far.
    fn recording_prompt(&self) -> Option<String> {
        let r = self.recording.as_ref()?;
        let name = &r.row.0;
        Some(if r.recorder.is_empty() {
            crate::i18n::tf("Press the keys for “{}”. Escape cancels.", &[name])
        } else {
            crate::i18n::tf("{0} for “{1}”: press Enter to save, or the next key of a sequence. Escape cancels.", &[&r.recorder.display(), name])
        })
    }

    // --- Accessibility and tests ---

    /// For assistive technology: the search text, the first `max` shown rows, how many are shown, whether the search field
    /// has focus, and what is to be announced.
    pub fn access_view(&self, max: usize, window: &Window, cx: &App) -> AccessView {
        let query = self.query.read(cx).text().to_string();
        let visible = self.visible(cx);
        let rows = visible
            .iter()
            .take(max)
            .map(|r| AccessRow { label: r.label.clone(), description: describe(r), selected: self.selected.as_ref() == Some(&key_of(r)) })
            .collect();
        let status = self.shown_status();
        AccessView { query, rows, total: visible.len(), search_focused: self.query.focus_handle(cx).is_focused(window), status }
    }

    /// The painted, visible bounds of the first `count` shown rows (physical pixels; `None` while scrolled out of view).
    pub fn access_row_bounds(&self, count: usize, window: &Window) -> Vec<Option<accesskit::Rect>> {
        (0..count).map(|i| crate::painted::physical(&self.bounds, &i, window)).collect()
    }

    #[cfg(test)]
    pub fn visible_for_test(&self, cx: &App) -> Vec<String> {
        self.visible(cx).iter().map(|r| r.label.clone()).collect()
    }

    #[cfg(test)]
    pub fn rows_for_test(&self) -> &[Row] {
        &self.rows
    }

    #[cfg(test)]
    pub fn shown_status_for_test(&self) -> Option<Status> {
        self.shown_status()
    }

    #[cfg(test)]
    pub fn status_for_test(&self) -> Option<&Status> {
        self.status.as_ref()
    }

    #[cfg(test)]
    pub fn recording_for_test(&self) -> Option<String> {
        self.recording.as_ref().map(|r| r.recorder.keys())
    }

    #[cfg(test)]
    pub fn selected_for_test(&self) -> Option<Row> {
        self.selected_row()
    }
}

/// One sentence for a reader of the tree: the key, where it comes from, its context and who else uses it.
fn describe(row: &Row) -> String {
    let Some(b) = &row.binding else { return crate::i18n::t("No shortcut").to_string() };
    let mut parts =
        vec![if b.disabled { crate::i18n::tf("{} (turned off)", &[&b.display]) } else { b.display.clone() }, rows::source_name(b.source).to_string()];
    parts.extend(b.when.as_ref().map(|w| crate::i18n::tf("when {}", &[w])));
    if !b.conflicts.is_empty() {
        parts.push(conflict_text(b));
    }
    parts.join(", ")
}

fn conflict_text(b: &Binding) -> String {
    crate::i18n::tf("Also used by {}", &[&b.conflicts.join(", ")])
}

impl Render for ShortcutsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        // The list's width as painted in the last frame (the window's, less what is beside the page, before the first one).
        let list_width = self.bounds.borrow().get(&LIST_FRAME).map_or((window.viewport_size().width / px(1.)) - 320., |b| b.size.width / px(1.));
        let narrow = list_width < NARROW;
        let rows: Vec<Row> = self.visible(cx).into_iter().cloned().collect();
        let n = rows.len();
        let conflicts = rows::conflict_count(&rows);
        // Rows record where they are painted in this frame.
        let list_frame = self.bounds.borrow().get(&LIST_FRAME).copied();
        self.bounds.borrow_mut().clear();
        let bounds = self.bounds.clone();
        // A resize changes the width after this frame was laid out: draw once more when that moved the layout across the limit.
        let me = cx.weak_entity();
        window.on_next_frame(move |_, cx| {
            me.update(cx, |page, cx| {
                let now = page.bounds.borrow().get(&LIST_FRAME).map(|b| b.size.width / px(1.));
                if now.is_some_and(|w| (w < NARROW) != (list_frame.is_none_or(|b| (b.size.width / px(1.)) < NARROW))) {
                    cx.notify();
                }
            })
            .ok();
        });
        let surface = crate::i18n::t("Keyboard Shortcuts");
        let status = self.shown_status();
        div()
            .id("shortcuts-page")
            .key_context("ShortcutsPage")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| this.on_key(e, window, cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(c(p.bg))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(theme::SPACE[5]))
                    .px(px(theme::SPACE[9]))
                    .py(px(theme::SPACE[5]))
                    .border_b_1()
                    .border_color(c(p.border))
                    .child(ui::icon("keyboard", 20., p.accent))
                    .child(div().text_size(px(text::LG.0)).font_weight(gpui::FontWeight::SEMIBOLD).child(crate::i18n::t("Keyboard Shortcuts")))
                    .child(
                        crate::ui::input_box("shortcuts-search-box", &p, p.surface, self.query.focus_handle(cx).is_focused(window))
                            .w(px(400.))
                            .max_w_full()
                            .min_w_0()
                            .child(crate::ui::search_icon(&p))
                            .child(div().flex_1().min_w_0().child(self.query.clone())),
                    )
                    .child(div().flex_none().text_color(c(p.text_muted)).text_size(px(text::XS.0)).child(crate::i18n::tn("{} command", "{} commands", n, &[])))
                    .child(div().flex_1().min_w_0())
                    .child(self.open_file_button(&p, surface, cx)),
            )
            .when_some(status, |d, s| {
                let color = if s.problem {
                    p.danger
                } else if self.recording.is_some() {
                    p.accent
                } else {
                    p.text_muted
                };
                d.child(
                    div()
                        .id("shortcuts-status")
                        .flex_none()
                        .px(px(theme::SPACE[9]))
                        .py(px(theme::SPACE[3]))
                        .text_size(px(text::XS.0))
                        .text_color(c(color))
                        .child(s.text),
                )
            })
            .when(conflicts > 0 && self.status.is_none() && self.recording.is_none(), |d| {
                d.child(
                    div()
                        .id("shortcuts-conflicts")
                        .flex_none()
                        .px(px(theme::SPACE[9]))
                        .py(px(theme::SPACE[3]))
                        .text_size(px(text::XS.0))
                        .text_color(c(p.warning))
                        .child(crate::i18n::tn(
                            "{} shortcut shares its key with another command",
                            "{} shortcuts share their key with another command",
                            conflicts,
                            &[],
                        )),
                )
            })
            .when(!narrow, |d| d.child(self.column_header(&p)))
            .child(
                div()
                    .id("shortcut-list")
                    .relative()
                    .child(crate::painted::probe(&bounds, LIST_FRAME))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(theme::SPACE[9]))
                    .py(px(theme::SPACE[4]))
                    .children(rows.into_iter().enumerate().map(|(i, row)| self.render_row(i, row, narrow, &bounds, &p, cx))),
            )
    }
}

impl ShortcutsPage {
    fn open_file_button(&self, p: &theme::Palette, surface: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let label = crate::i18n::t("Open keybindings.jsonc");
        let el = div()
            .id("shortcuts-open-file")
            .h(px(26.))
            .px(px(theme::SPACE[4]))
            .flex()
            .flex_none()
            .items_center()
            .rounded(px(theme::radius::SM))
            .bg(c(p.surface_hover))
            .cursor_pointer()
            .hover(|d| d.bg(c(p.surface_active)))
            .text_size(px(text::XS.0))
            .child(label);
        crate::a11y_controls::press_on(el, crate::a11y_controls::Control::button(surface, "open-file", label, |_, _| {}), cx, |_, _, cx| {
            cx.emit(ShortcutsEvent::OpenFile)
        })
    }

    /// Names of the columns, at the places of the row's cells.
    fn column_header(&self, p: &theme::Palette) -> impl IntoElement {
        let name = |label: &'static str, width: Option<f32>| {
            let cell = div().flex_none().overflow_hidden().whitespace_nowrap().child(crate::i18n::t(label));
            match width {
                Some(w) => cell.w(px(w)),
                None => cell.flex_1().min_w_0(),
            }
        };
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(theme::SPACE[5]))
            .mx(px(theme::SPACE[9]))
            .px(px(theme::SPACE[4]))
            .py(px(theme::SPACE[2]))
            .border_b_1()
            .border_color(c(p.border))
            .text_size(px(text::XS.0))
            .text_color(c(p.text_muted))
            .child(name(crate::i18n::tr("Command"), None))
            .child(name(crate::i18n::tr("Keybinding"), Some(KEYS_W)))
            .child(name(crate::i18n::tr("When"), Some(WHEN_W)))
            .child(name(crate::i18n::tr("Source"), Some(SOURCE_W)))
            .child(div().flex_none().w(px(BUTTONS_W)))
    }

    fn on_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // While recording the interceptor has the keys; this handler only sees what it let through.
        if self.recording.is_some() {
            return;
        }
        let search = self.query.focus_handle(cx).is_focused(window);
        let key = e.keystroke.key.as_str();
        let plain = !e.keystroke.modifiers.modified();
        match (key, plain) {
            // From the search field the arrows go to the list, so Enter and Delete then act on the row.
            ("down", true) | ("up", true) => {
                self.move_selection(if key == "down" { 1 } else { -1 }, cx);
                window.focus(&self.focus_handle);
            }
            ("enter", true) if !search => {
                if let Some(row) = self.selected_row() {
                    self.start_recording(&row, false, window, cx);
                }
            }
            ("delete" | "backspace", true) if !search => {
                if let Some(row) = self.selected_row().filter(|r| r.binding.is_some() && self.can_change(r)) {
                    cx.emit(ShortcutsEvent::Remove(row));
                }
            }
            // Typing while the list has focus goes on in the search field.
            _ if !search && plain && !e.keystroke.modifiers.platform => {
                let Some(typed) = e.keystroke.key_char.clone().filter(|t| !t.chars().any(char::is_control)) else { return };
                let text = format!("{}{typed}", self.query.read(cx).text());
                self.query.update(cx, |field, cx| field.set_text(&text, cx));
                window.focus(&self.query.focus_handle(cx));
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    fn render_row(
        &self,
        i: usize,
        row: Row,
        narrow: bool,
        bounds: &crate::painted::RowBounds<usize>,
        p: &theme::Palette,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = *p;
        let surface = crate::i18n::t("Keyboard Shortcuts");
        let key = key_of(&row);
        let selected = self.selected.as_ref() == Some(&key);
        let recording = self.recording.as_ref().is_some_and(|r| r.row == key);
        let can_change = self.can_change(&row);
        let has_key = row.binding.is_some();
        let changed = row.changed;
        let group: gpui::SharedString = format!("shortcut-{i}").into();
        let id_key = format!("{}|{}|{}", key.0, key.1.clone().unwrap_or_default(), key.2.clone().unwrap_or_default());
        let (when, source, conflict, disabled) = match &row.binding {
            Some(b) => (b.when.clone(), Some(b.source), (!b.conflicts.is_empty()).then(|| conflict_text(b)), b.disabled),
            None => (None, None, None, false),
        };
        let keys_cell = || {
            let cell = div().flex_none().flex().items_center().min_w_0();
            let cell = if narrow { cell } else { cell.w(px(KEYS_W)) };
            if recording {
                let shown = self.recording.as_ref().map(|r| r.recorder.display()).unwrap_or_default();
                let shown = if shown.is_empty() { crate::i18n::t("Press keys…").to_string() } else { format!("{shown} …") };
                cell.child(
                    div()
                        .px(px(theme::SPACE[2]))
                        .border_1()
                        .border_color(c(p.accent))
                        .rounded(px(theme::radius::SM))
                        .text_size(px(text::XS.0))
                        .text_color(c(p.accent))
                        .child(shown),
                )
            } else if let Some(b) = &row.binding {
                cell.child(
                    div()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .when(disabled, |d| d.line_through().opacity(0.6))
                        .child(crate::workbench::kbd(b.display.clone(), &p)),
                )
            } else {
                cell
            }
        };
        let button = |action: &'static str, glyph: &'static str, label: &'static str| {
            let described = format!("{}: {}", row.label, describe(&row));
            let el = ui::icon_button((action, i), glyph, label, &p);
            let el = if selected || recording { el } else { el.invisible().group_hover(group.clone(), |s| s.visible()) };
            (el, crate::a11y_controls::Control::button(surface, format!("{action}:{id_key}"), label, |_, _| {}).description(described))
        };
        let act = |action: &'static str, glyph: &'static str, label: &'static str, f: fn(&mut Self, &Row, &mut Window, &mut Context<Self>)| {
            let (el, ctl) = button(action, glyph, label);
            let row = row.clone();
            crate::a11y_controls::press_on(el, ctl, cx, move |this, w, cx| f(this, &row, w, cx))
        };
        let buttons = div().flex_none().w(px(BUTTONS_W)).flex().items_center().justify_end().gap(px(theme::SPACE[1])).when(can_change || has_key, |d| {
            d.when(can_change, |d| {
                d.child(act("change", "pencil", crate::i18n::t("Change key"), |this, row, w, cx| this.start_recording(row, false, w, cx))).child(act(
                    "add",
                    "plus",
                    crate::i18n::t("Add another key"),
                    |this, row, w, cx| this.start_recording(row, true, w, cx),
                ))
            })
            .when(can_change && has_key && !disabled, |d| {
                d.child(act("remove", "x", crate::i18n::t("Remove key"), |_, row, _, cx| cx.emit(ShortcutsEvent::Remove(row.clone()))))
            })
            .when(can_change && changed, |d| {
                d.child(act("reset", "rotate-ccw", crate::i18n::t("Reset to default"), |_, row, _, cx| cx.emit(ShortcutsEvent::Reset(row.clone()))))
            })
            .child(act("run", "play", crate::i18n::t("Run command"), |_, row, _, cx| cx.emit(ShortcutsEvent::Run(row.label.clone()))))
        });
        let label_cell = {
            let name = div().overflow_hidden().whitespace_nowrap().text_ellipsis().child(row.label.clone());
            let sub = narrow.then(|| {
                let mut bits: Vec<String> = Vec::new();
                bits.extend(source.map(|s| rows::source_name(s).to_string()));
                bits.extend(when.clone());
                bits.join(" · ")
            });
            let badge = || {
                conflict.clone().map(|text| {
                    div()
                        .flex_none()
                        .max_w(px(if narrow { 160. } else { 260. }))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .px(px(theme::SPACE[3]))
                        .rounded(px(theme::radius::SM))
                        .border_1()
                        .border_color(c(p.warning))
                        .text_color(c(p.warning))
                        .text_size(px(text::XS.0))
                        .child(text)
                })
            };
            let first = div()
                .flex()
                .items_center()
                .gap(px(theme::SPACE[3]))
                .min_w_0()
                .child(div().min_w_0().flex_shrink().child(name))
                .children((!narrow).then(badge).flatten());
            let second = narrow.then(|| {
                let meta = sub
                    .clone()
                    .filter(|s| !s.is_empty())
                    .map(|s| div().overflow_hidden().whitespace_nowrap().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(s));
                div().flex().items_center().gap(px(theme::SPACE[3])).min_w_0().child(keys_cell()).children(meta).children(badge())
            });
            div().flex_1().min_w_0().flex().flex_col().child(first).children(second)
        };
        div()
            .id(("shortcut", i))
            .group(group.clone())
            .relative()
            .child(crate::painted::probe(bounds, i))
            .h(px(if narrow { ROW_H_NARROW } else { ROW_H }))
            .flex()
            .items_center()
            .gap(px(theme::SPACE[5]))
            .px(px(theme::SPACE[4]))
            .rounded(px(theme::radius::SM))
            .cursor_pointer()
            .when(selected, |d| d.bg(c(p.surface_active)))
            .when(!selected, |d| d.hover(|d| d.bg(c(p.surface_hover))))
            .child(label_cell)
            .when(!narrow, |d| d.child(keys_cell()))
            .when(!narrow, |d| {
                d.child(
                    div()
                        .flex_none()
                        .w(px(WHEN_W))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(text::XS.0))
                        .text_color(c(p.text_muted))
                        .child(when.clone().unwrap_or_default()),
                )
                .child(
                    div()
                        .flex_none()
                        .w(px(SOURCE_W))
                        .text_size(px(text::XS.0))
                        .text_color(c(p.text_muted))
                        .child(source.map(|s| rows::source_name(s).to_string()).unwrap_or_default()),
                )
            })
            .child(buttons)
            .on_click({
                let row = row.clone();
                cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                    this.select(&row, window, cx);
                    if e.click_count() >= 2 {
                        this.start_recording(&row, false, window, cx);
                    }
                })
            })
    }
}
