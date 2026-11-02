//! Text field (search query, globs; commit message in multi-line mode). IME-aware.

use std::ops::Range;

use gpui::{
    App, ClipboardItem, Context, ElementInputHandler, EntityInputHandler, EventEmitter, FocusHandle, Focusable, Pixels, Point, SharedString, UTF16Selection,
    Window, actions, canvas, div, prelude::*, px,
};

use crate::ui::{c, palette, text};

actions!(
    text_field,
    [FieldLeft, FieldRight, FieldHome, FieldEnd, FieldBackspace, FieldDelete, FieldSelectAll, FieldPaste, FieldCopy, FieldSubmit, FieldEscape, FieldNewline]
);

pub enum FieldEvent {
    Changed,
    Submit,
    Escape,
}

pub struct TextField {
    text: String,
    /// Byte offset.
    cursor: usize,
    all_selected: bool,
    marked: Option<Range<usize>>,
    placeholder: SharedString,
    focus_handle: FocusHandle,
    /// Line breaks are kept (⇧↵ new line, pasted lines stay).
    multiline: bool,
}

impl EventEmitter<FieldEvent> for TextField {}

impl Focusable for TextField {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TextField {
    pub fn new(placeholder: &str, cx: &mut Context<Self>) -> Self {
        TextField {
            text: String::new(),
            cursor: 0,
            all_selected: false,
            marked: None,
            placeholder: placeholder.to_string().into(),
            focus_handle: cx.focus_handle(),
            multiline: false,
        }
    }

    pub fn multiline(mut self) -> Self {
        self.multiline = true;
        self
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Shown while the field is empty (a source control provider sets its own).
    pub fn set_placeholder(&mut self, placeholder: &str) {
        if self.placeholder != placeholder {
            self.placeholder = placeholder.to_string().into();
        }
    }

    /// Replace the text and leave all of it selected (so typing replaces it).
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.text = if self.multiline { text.to_string() } else { text.replace('\n', " ") };
        self.cursor = self.text.len();
        self.all_selected = !self.text.is_empty();
        self.marked = None;
        cx.emit(FieldEvent::Changed);
        cx.notify();
    }

    fn edit(&mut self, range: Range<usize>, new: &str, cx: &mut Context<Self>) {
        let range = if self.all_selected { 0..self.text.len() } else { range };
        // Single-line field: a line break becomes a space, other control characters (Tab) are rejected.
        let multiline = self.multiline;
        let new: String = new
            .replace("\r\n", "\n")
            .chars()
            .filter_map(|ch| match ch {
                '\n' if multiline => Some('\n'),
                '\n' => Some(' '),
                _ => (!ch.is_control()).then_some(ch),
            })
            .collect();
        if new.is_empty() && range.is_empty() {
            return;
        }
        self.text.replace_range(range.clone(), &new);
        self.cursor = range.start + new.len();
        self.all_selected = false;
        cx.emit(FieldEvent::Changed);
        cx.notify();
    }

    fn prev_char(&self) -> usize {
        self.text[..self.cursor].char_indices().next_back().map_or(0, |(i, _)| i)
    }

    fn next_char(&self) -> usize {
        self.text[self.cursor..].chars().next().map_or(self.cursor, |c| self.cursor + c.len_utf8())
    }

    fn move_to(&mut self, at: usize, cx: &mut Context<Self>) {
        self.cursor = at;
        self.all_selected = false;
        cx.notify();
    }

    fn left(&mut self, _: &FieldLeft, _: &mut Window, cx: &mut Context<Self>) {
        let at = if self.all_selected { 0 } else { self.prev_char() };
        self.move_to(at, cx);
    }
    fn right(&mut self, _: &FieldRight, _: &mut Window, cx: &mut Context<Self>) {
        let at = if self.all_selected { self.text.len() } else { self.next_char() };
        self.move_to(at, cx);
    }
    fn home(&mut self, _: &FieldHome, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }
    fn end(&mut self, _: &FieldEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.text.len(), cx);
    }
    fn backspace(&mut self, _: &FieldBackspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.all_selected || self.cursor > 0 {
            self.edit(self.prev_char()..self.cursor, "", cx);
        }
    }
    fn delete(&mut self, _: &FieldDelete, _: &mut Window, cx: &mut Context<Self>) {
        if self.all_selected || self.cursor < self.text.len() {
            self.edit(self.cursor..self.next_char(), "", cx);
        }
    }
    fn select_all(&mut self, _: &FieldSelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.all_selected = !self.text.is_empty();
        cx.notify();
    }
    fn paste(&mut self, _: &FieldPaste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = cx.read_from_clipboard().and_then(|i| i.text()) {
            self.edit(self.cursor..self.cursor, &t, cx);
        }
    }
    fn copy(&mut self, _: &FieldCopy, _: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.text.clone()));
    }
    fn submit(&mut self, _: &FieldSubmit, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(FieldEvent::Submit);
    }
    fn newline(&mut self, _: &FieldNewline, _: &mut Window, cx: &mut Context<Self>) {
        if self.multiline {
            let at = self.cursor;
            self.edit(at..at, "\n", cx);
        }
    }

    fn escape(&mut self, _: &FieldEscape, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(FieldEvent::Escape);
    }

    fn byte_of_utf16(&self, cu: usize) -> usize {
        let mut units = 0;
        for (b, ch) in self.text.char_indices() {
            if units >= cu {
                return b;
            }
            units += ch.len_utf16();
        }
        self.text.len()
    }

    fn utf16_of(&self, byte: usize) -> usize {
        self.text[..byte.min(self.text.len())].encode_utf16().count()
    }
}

impl EntityInputHandler for TextField {
    fn text_for_range(&mut self, r: Range<usize>, actual: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        let (a, b) = (self.byte_of_utf16(r.start), self.byte_of_utf16(r.end));
        actual.replace(self.utf16_of(a)..self.utf16_of(b));
        Some(self.text[a..b].to_string())
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let range = if self.all_selected { 0..self.utf16_of(self.text.len()) } else { self.utf16_of(self.cursor)..self.utf16_of(self.cursor) };
        Some(UTF16Selection { range, reversed: false })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|m| self.utf16_of(m.start)..self.utf16_of(m.end))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(&mut self, range: Option<Range<usize>>, new: &str, _: &mut Window, cx: &mut Context<Self>) {
        let r = range.map(|r| self.byte_of_utf16(r.start)..self.byte_of_utf16(r.end)).or(self.marked.take()).unwrap_or(self.cursor..self.cursor);
        self.marked = None;
        self.edit(r, new, cx);
    }

    fn replace_and_mark_text_in_range(&mut self, range: Option<Range<usize>>, new: &str, _: Option<Range<usize>>, _: &mut Window, cx: &mut Context<Self>) {
        let r = range.map(|r| self.byte_of_utf16(r.start)..self.byte_of_utf16(r.end)).or(self.marked.clone()).unwrap_or(self.cursor..self.cursor);
        let start = if self.all_selected { 0 } else { r.start };
        self.edit(r, new, cx);
        self.marked = (!new.is_empty()).then(|| start..start + new.len());
    }

    fn bounds_for_range(&mut self, _: Range<usize>, b: gpui::Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<gpui::Bounds<Pixels>> {
        Some(b)
    }

    fn character_index_for_point(&mut self, _: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

impl Render for TextField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        let focused = self.focus_handle.is_focused(window);
        let entity = cx.entity();
        let focus = self.focus_handle.clone();
        let caret = || div().flex_none().w(px(1.5)).h(px(15.)).when(focused, |d| d.bg(c(p.accent)));
        div()
            .key_context(if self.multiline { "TextField multiline" } else { "TextField" })
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::escape))
            .on_action(cx.listener(Self::newline))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus_handle);
                    cx.notify();
                }),
            )
            .relative()
            .flex_1()
            .min_w_0()
            .when(!self.multiline, |d| d.h(px(26.)))
            .when(self.multiline, |d| d.min_h(px(26.)).py(px(4.)))
            .px(px(theme::SPACE[3]))
            .flex()
            .items_center()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_size(px(text::SM.0))
            .map(|d| {
                if self.multiline && self.text.contains('\n') {
                    // Line by line; the cursor is on its own line.
                    let mut start = 0;
                    let lines: Vec<gpui::AnyElement> = self
                        .text
                        .split('\n')
                        .map(|line| {
                            let (a, b) = (start, start + line.len());
                            start = b + 1;
                            let row = div().h(px(18.)).flex().items_center().text_color(c(p.text));
                            if self.all_selected {
                                row.child(div().bg(c(p.selection)).child(line.to_string())).into_any_element()
                            } else if (a..=b).contains(&self.cursor) {
                                let at = self.cursor - a;
                                row.child(line[..at].to_string()).child(caret()).child(line[at..].to_string()).into_any_element()
                            } else {
                                row.child(line.to_string()).into_any_element()
                            }
                        })
                        .collect();
                    d.items_start().child(div().flex().flex_col().children(lines))
                } else if self.text.is_empty() {
                    d.child(caret()).child(div().text_color(c(p.text_muted)).child(crate::i18n::t(&self.placeholder).to_string()))
                } else if self.all_selected {
                    d.child(div().bg(c(p.selection)).text_color(c(p.text)).child(self.text.clone()))
                } else {
                    // The cursor is between two segments: the layout computes its position.
                    d.text_color(c(p.text)).child(self.text[..self.cursor].to_string()).child(caret()).child(self.text[self.cursor..].to_string())
                }
            })
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        window.handle_input(&focus, ElementInputHandler::new(bounds, entity), cx);
                    },
                )
                .absolute()
                .size_full(),
            )
    }
}
