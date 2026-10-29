//! Find / replace within a file (⌘F, ⌥⌘F). The matcher is the same as file search
//! (`agent::search::compile`): Turkish i/İ/ı, whole word, regex.
//! The bar only holds the inputs; the editor computes and shows the matches.

use std::ops::Range;

use buffer::Buffer;
use gpui::{App, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton, SharedString, Subscription, Window, actions, div, prelude::*, px};
use regex::bytes::Regex;

use crate::input::{FieldEvent, TextField};
use crate::ui::{self, c, icon, palette, text};

actions!(find, [FindPrev, ReplaceAll, ToggleCase, ToggleWord, ToggleRegex, TogglePreserveCase, SwitchField, ToggleInSelection, SelectAllMatches]);

/// Matches beyond this count are not counted (a huge file is scanned on every keystroke).
pub const MAX_MATCHES: usize = 10_000;

pub enum FindEvent {
    /// The query or options changed: search again.
    Changed,
    Next,
    Prev,
    ReplaceOne,
    ReplaceAll,
    /// ⌥↵: select all matches (multi-cursor).
    SelectAll,
    Close,
}

pub struct FindBar {
    query: Entity<TextField>,
    replacement: Entity<TextField>,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    /// When replacing, preserve the match's letter case (`AB`, ⌥P).
    pub preserve_case: bool,
    /// Search only within the selection at the time the bar was opened (⌥⌘L).
    pub in_selection: bool,
    pub show_replace: bool,
    /// "3 of 12", "No results" or a pattern error (written by the editor).
    status: Option<(SharedString, bool)>,
    focus_handle: FocusHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<FindEvent> for FindBar {}

impl Focusable for FindBar {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl FindBar {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| TextField::new(crate::i18n::tr("Find"), cx));
        let replacement = cx.new(|cx| TextField::new(crate::i18n::tr("Replace"), cx));
        let subs = vec![
            cx.subscribe(&query, |_, _, ev: &FieldEvent, cx| {
                cx.emit(match ev {
                    FieldEvent::Changed => FindEvent::Changed,
                    FieldEvent::Submit => FindEvent::Next,
                    FieldEvent::Escape => FindEvent::Close,
                })
            }),
            cx.subscribe(&replacement, |_, _, ev: &FieldEvent, cx| match ev {
                FieldEvent::Changed => {}
                FieldEvent::Submit => cx.emit(FindEvent::ReplaceOne),
                FieldEvent::Escape => cx.emit(FindEvent::Close),
            }),
        ];
        FindBar {
            query,
            replacement,
            case_sensitive: false,
            whole_word: false,
            preserve_case: false,
            regex: false,
            in_selection: false,
            show_replace: false,
            status: None,
            focus_handle: cx.focus_handle(),
            _subs: subs,
        }
    }

    pub fn query(&self, cx: &App) -> String {
        self.query.read(cx).text().to_string()
    }

    pub fn replacement(&self, cx: &App) -> String {
        self.replacement.read(cx).text().to_string()
    }

    pub fn set_query(&mut self, text: &str, cx: &mut Context<Self>) {
        self.query.update(cx, |f, cx| f.set_text(text, cx));
    }

    #[cfg(test)]
    pub fn set_replacement(&mut self, text: &str, cx: &mut Context<Self>) {
        self.replacement.update(cx, |f, cx| f.set_text(text, cx));
    }

    pub fn focus_query(&self, window: &mut Window, cx: &App) {
        window.focus(&self.query.focus_handle(cx));
    }

    pub fn focus_replacement(&self, window: &mut Window, cx: &App) {
        window.focus(&self.replacement.focus_handle(cx));
    }

    pub fn set_status(&mut self, status: Option<(SharedString, bool)>, cx: &mut Context<Self>) {
        self.status = status;
        cx.notify();
    }

    pub fn status_text(&self) -> Option<&str> {
        self.status.as_ref().map(|(s, _)| s.as_ref())
    }

    /// For the accessibility tree (a11y.rs): the bar's controls in visual order, with the state of each option
    /// (`None` for a button).
    pub fn a11y_controls(&self) -> Vec<(&'static str, Option<bool>)> {
        let t = crate::i18n::t;
        let mut out = vec![
            (t("Match case (⌥C)"), Some(self.case_sensitive)),
            (t("Whole word (⌥W)"), Some(self.whole_word)),
            (t("Regular expression (⌥R)"), Some(self.regex)),
            (t("Previous match (⇧↵)"), None),
            (t("Next match (↵)"), None),
            (t("Find in selection (⌥⌘L)"), Some(self.in_selection)),
            (t("Select all matches (⌥↵)"), None),
            (t("Close (Esc)"), None),
        ];
        if self.show_replace {
            out.extend([(t("Preserve case (⌥P)"), Some(self.preserve_case)), (t("Replace (↵)"), None), (t("Replace all (⌥⌘↵)"), None)]);
        }
        out
    }

    /// Accessibility action: press control `i` of `a11y_controls` (toggle an option or run a button).
    pub fn a11y_press(&mut self, i: usize, cx: &mut Context<Self>) {
        let replace = self.show_replace;
        let option = match i {
            0 => Some(&mut self.case_sensitive),
            1 => Some(&mut self.whole_word),
            2 => Some(&mut self.regex),
            5 => Some(&mut self.in_selection),
            8 if replace => Some(&mut self.preserve_case),
            _ => None,
        };
        let event = match (option, i) {
            (Some(on), _) => {
                *on = !*on;
                FindEvent::Changed
            }
            (None, 3) => FindEvent::Prev,
            (None, 4) => FindEvent::Next,
            (None, 6) => FindEvent::SelectAll,
            (None, 7) => FindEvent::Close,
            (None, 9) if replace => FindEvent::ReplaceOne,
            (None, 10) if replace => FindEvent::ReplaceAll,
            _ => return,
        };
        cx.emit(event);
        cx.notify();
    }

    /// For the accessibility tree (a11y.rs): which field has focus (0 query, 1 replacement).
    pub fn a11y_focus(&self, window: &Window, cx: &App) -> Option<usize> {
        if self.query.focus_handle(cx).is_focused(window) {
            Some(0)
        } else if self.replacement.focus_handle(cx).is_focused(window) {
            Some(1)
        } else {
            None
        }
    }

    /// Compile with the current options. Empty query: `Ok(None)`.
    pub fn compile(&self, cx: &App) -> Result<Option<Regex>, String> {
        let pattern = self.query(cx);
        if pattern.is_empty() {
            return Ok(None);
        }
        let q = proto::SearchQuery {
            root: String::new(),
            pattern,
            regex: self.regex,
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
            include: Vec::new(),
            exclude: Vec::new(),
            max_results: 0,
        };
        agent::search::compile(&q).map(Some).map_err(|e| e.to_string())
    }

    fn toggle_case(&mut self, _: &ToggleCase, _: &mut Window, cx: &mut Context<Self>) {
        self.case_sensitive = !self.case_sensitive;
        cx.emit(FindEvent::Changed);
        cx.notify();
    }
    fn toggle_word(&mut self, _: &ToggleWord, _: &mut Window, cx: &mut Context<Self>) {
        self.whole_word = !self.whole_word;
        cx.emit(FindEvent::Changed);
        cx.notify();
    }
    fn toggle_regex(&mut self, _: &ToggleRegex, _: &mut Window, cx: &mut Context<Self>) {
        self.regex = !self.regex;
        cx.emit(FindEvent::Changed);
        cx.notify();
    }
    fn toggle_preserve_case(&mut self, _: &TogglePreserveCase, _: &mut Window, cx: &mut Context<Self>) {
        self.preserve_case = !self.preserve_case;
        cx.notify();
    }
    fn toggle_in_selection(&mut self, _: &ToggleInSelection, _: &mut Window, cx: &mut Context<Self>) {
        self.in_selection = !self.in_selection;
        cx.emit(FindEvent::Changed);
        cx.notify();
    }
    fn select_all_matches(&mut self, _: &SelectAllMatches, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(FindEvent::SelectAll);
    }
    fn prev(&mut self, _: &FindPrev, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(FindEvent::Prev);
    }
    /// Tab: switch between the find and replace fields.
    fn switch_field(&mut self, _: &SwitchField, window: &mut Window, cx: &mut Context<Self>) {
        if self.show_replace && self.query.focus_handle(cx).is_focused(window) {
            self.focus_replacement(window, cx);
        } else {
            self.focus_query(window, cx);
        }
    }
    fn replace_all(&mut self, _: &ReplaceAll, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(FindEvent::ReplaceAll);
    }

    fn toggle(&self, id: &'static str, label: &'static str, on: bool, tip: &'static str, cx: &mut Context<Self>, set: fn(&mut Self)) -> impl IntoElement {
        let p = *palette(cx);
        ui::option_toggle(id, label, on, tip, &p, p.bg).on_click(cx.listener(move |this, _, _, cx| {
            set(this);
            cx.emit(FindEvent::Changed);
            cx.notify();
        }))
    }

    fn tool(&self, id: &'static str, glyph: &'static str, tip: &'static str, event: fn() -> FindEvent, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        let is_icon = matches!(glyph, "arrow-up" | "arrow-down" | "x");
        let content = if is_icon { icon(glyph, 14., p.text_muted).into_any_element() } else { div().child(glyph).into_any_element() };
        div()
            .id(id)
            .debug_selector(move || id.to_string())
            .flex_none()
            .h(px(22.))
            .when(is_icon, |d| d.w(px(22.)))
            .when(!is_icon, |d| d.px(px(theme::SPACE[3])))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(theme::radius::SM))
            .text_size(px(text::XS.0))
            .text_color(c(p.text_muted))
            .cursor_pointer()
            .hover(|d| d.bg(c(p.surface_hover)))
            .child(content)
            .tooltip(move |_, cx| cx.new(|_| crate::workbench::Tooltip(tip.into())).into())
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(event())))
    }
}

impl Render for FindBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        let (q_focus, r_focus) = (self.query.focus_handle(cx).is_focused(window), self.replacement.focus_handle(cx).is_focused(window));
        let field_box = |id: &'static str, focused: bool, child: gpui::AnyElement| {
            crate::ui::input_box(id, &p, p.bg, focused)
                .debug_selector(move || id.to_string())
                .w(px(320.))
                .max_w(gpui::relative(1.))
                .h_auto()
                .min_h(px(28.))
                .child(child)
        };
        let status = self.status.clone();
        let show_replace = self.show_replace;
        div()
            .id("find-bar")
            .debug_selector(|| "find-bar".into())
            .key_context("FindBar")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::toggle_case))
            .on_action(cx.listener(Self::toggle_word))
            .on_action(cx.listener(Self::toggle_regex))
            .on_action(cx.listener(Self::toggle_preserve_case))
            .on_action(cx.listener(Self::toggle_in_selection))
            .on_action(cx.listener(Self::select_all_matches))
            .on_action(cx.listener(Self::prev))
            .on_action(cx.listener(Self::replace_all))
            .on_action(cx.listener(Self::switch_field))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .w(px(620.))
            .max_w(gpui::relative(1.))
            .flex()
            .gap(px(theme::SPACE[2]))
            .p(px(theme::SPACE[2]))
            .bg(c(p.surface_raised))
            .border_1()
            .border_color(c(p.border))
            .rounded(px(theme::radius::MD))
            .font_family(ui::UI_FONT)
            .text_size(px(text::SM.0))
            .child(
                div()
                    .id("find-expand")
                    .flex_none()
                    .w(px(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(theme::radius::SM))
                    .cursor_pointer()
                    .hover(|d| d.bg(c(p.surface_hover)))
                    .child(icon(if show_replace { "chevron-down" } else { "chevron-right" }, 14., p.text_muted))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_replace = !this.show_replace;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .flex_col()
                    .gap(px(theme::SPACE[2]))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(theme::SPACE[2]))
                            .child(field_box(
                                "find-query-box",
                                q_focus,
                                div()
                                    .flex()
                                    .flex_1()
                                    .min_w_0()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(theme::SPACE[2]))
                                    .child(crate::ui::search_icon(&p))
                                    .child(div().flex_1().min_w(px(64.)).child(self.query.clone()))
                                    .child(self.toggle("f-case", "Aa", self.case_sensitive, crate::i18n::t("Match case (⌥C)"), cx, |s| {
                                        s.case_sensitive = !s.case_sensitive
                                    }))
                                    .child(
                                        self.toggle("f-word", "ab", self.whole_word, crate::i18n::t("Whole word (⌥W)"), cx, |s| s.whole_word = !s.whole_word),
                                    )
                                    .child(self.toggle("f-regex", ".*", self.regex, crate::i18n::t("Regular expression (⌥R)"), cx, |s| s.regex = !s.regex))
                                    .into_any_element(),
                            ))
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(96.))
                                    .text_size(px(text::XS.0))
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .when_some(status, |d, (s, err)| d.text_color(c(if err { p.danger } else { p.text_muted })).child(s)),
                            )
                            .child(self.tool("f-prev", "arrow-up", crate::i18n::t("Previous match (⇧↵)"), || FindEvent::Prev, cx))
                            .child(self.tool("f-next", "arrow-down", crate::i18n::t("Next match (↵)"), || FindEvent::Next, cx))
                            .child(self.toggle("f-insel", "⊂", self.in_selection, crate::i18n::t("Find in selection (⌥⌘L)"), cx, |s| {
                                s.in_selection = !s.in_selection
                            }))
                            .child(self.tool("f-all", "⌥↵", crate::i18n::t("Select all matches (⌥↵)"), || FindEvent::SelectAll, cx))
                            .child(self.tool("f-close", "x", crate::i18n::t("Close (Esc)"), || FindEvent::Close, cx)),
                    )
                    .when(show_replace, |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(px(theme::SPACE[2]))
                                .child(field_box(
                                    "find-replace-box",
                                    r_focus,
                                    div()
                                        .flex()
                                        .flex_1()
                                        .min_w_0()
                                        .items_center()
                                        .child(div().flex_1().min_w_0().child(self.replacement.clone()))
                                        .child(self.toggle("f-preserve", "AB", self.preserve_case, crate::i18n::t("Preserve case (⌥P)"), cx, |s| {
                                            s.preserve_case = !s.preserve_case
                                        }))
                                        .into_any_element(),
                                ))
                                .child(self.tool("f-replace", crate::i18n::t("Replace"), crate::i18n::t("Replace (↵)"), || FindEvent::ReplaceOne, cx))
                                .child(self.tool(
                                    "f-replace-all",
                                    crate::i18n::t("Replace All"),
                                    crate::i18n::t("Replace all (⌥⌘↵)"),
                                    || FindEvent::ReplaceAll,
                                    cx,
                                )),
                        )
                    }),
            )
    }
}

/// All matches (character ranges); empty matches are skipped.
pub fn find_all(buffer: &Buffer, re: &Regex) -> Vec<Range<usize>> {
    let text = buffer.rope().to_string();
    let rope = buffer.rope();
    re.find_iter(text.as_bytes()).filter(|m| !m.is_empty()).take(MAX_MATCHES).map(|m| rope.byte_to_char(m.start())..rope.byte_to_char(m.end())).collect()
}

/// Replacement syntax → the regex crate's: `$1_x` means group 1 followed by "_x" (the crate
/// takes it for a group named "1_x"), `$&` is the whole match; `\n`, `\t` are expanded.
pub(crate) fn regex_template(template: &str) -> String {
    let t = template.replace("\\n", "\n").replace("\\t", "\t").replace("$&", "${0}");
    let mut out = String::with_capacity(t.len() + 8);
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' && chars.peek().is_some_and(|d| d.is_ascii_digit()) {
            out.push_str("${");
            while let Some(d) = chars.peek().copied().filter(char::is_ascii_digit) {
                out.push(d);
                chars.next();
            }
            out.push('}');
        } else {
            out.push(c);
        }
    }
    out
}

/// All matches and their replacements (unbounded; for Replace All). The text is read once.
pub fn replace_all_parts(buffer: &Buffer, re: &Regex, template: &str, regex: bool) -> Vec<(Range<usize>, String)> {
    let text = buffer.rope().to_string();
    let rope = buffer.rope();
    let expanded = regex_template(template);
    re.captures_iter(text.as_bytes())
        .filter_map(|caps| {
            let m = caps.get(0).filter(|m| !m.is_empty())?;
            let new = if regex {
                let mut out = Vec::new();
                caps.expand(expanded.as_bytes(), &mut out);
                String::from_utf8_lossy(&out).into_owned()
            } else {
                template.to_string()
            };
            Some((rope.byte_to_char(m.start())..rope.byte_to_char(m.end()), new))
        })
        .collect()
}

/// The replacement text for a match. In regex mode `$1`, `${name}`, `\n`, `\t`
/// are expanded; in plain search the text is used as is.
pub fn replacement_for(buffer: &Buffer, re: &Regex, m: &Range<usize>, template: &str, regex: bool) -> String {
    if !regex {
        return template.to_string();
    }
    let rope = buffer.rope();
    let (start, end) = (rope.char_to_byte(m.start), rope.char_to_byte(m.end));
    let text = rope.to_string();
    let template = regex_template(template);
    let mut out = Vec::new();
    match re.captures_at(text.as_bytes(), start).filter(|c| c.get(0).is_some_and(|g| g.end() == end)) {
        Some(caps) => caps.expand(template.as_bytes(), &mut out),
        None => out.extend_from_slice(template.as_bytes()),
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// "Preserve case": adapt the replacement text to the matched text's letter case (VS Code rules).
/// UPPER → UPPER, lower → lower, Capitalized → Capitalized; for matches split by `_`, `-` or `.`
/// each part follows its own case (`Foo_Bar` → `Baz_Qux`).
/// Turkish text (a Turkish-only letter in the match or the replacement) pairs i/İ and ı/I; other text is unchanged.
pub fn preserve_case(matched: &str, replacement: &str) -> String {
    use crate::textcase::{CaseLocale, lower_locale, upper};
    let has_letter = matched.chars().any(char::is_alphabetic);
    if !has_letter || replacement.is_empty() {
        return replacement.to_string();
    }
    let turkish = |s: &str| s.chars().any(|c| matches!(c, 'ı' | 'İ' | 'ş' | 'Ş' | 'ğ' | 'Ğ'));
    let loc = if turkish(matched) || turkish(replacement) { CaseLocale::Turkish } else { CaseLocale::Invariant };
    if upper(matched, loc) == matched {
        return upper(replacement, loc);
    }
    if lower_locale(matched, loc) == matched {
        return lower_locale(replacement, loc);
    }
    for sep in ['_', '-', '.'] {
        if matched.contains(sep) && replacement.contains(sep) {
            let (ms, rs): (Vec<&str>, Vec<&str>) = (matched.split(sep).collect(), replacement.split(sep).collect());
            if ms.len() == rs.len() || ms.len() < rs.len() {
                return rs
                    .iter()
                    .enumerate()
                    .map(|(i, r)| preserve_case(ms.get(i).copied().unwrap_or_else(|| ms.last().copied().unwrap_or("")), r))
                    .collect::<Vec<_>>()
                    .join(&sep.to_string());
            }
        }
    }
    let first = matched.chars().next().unwrap_or(' ');
    if first.is_uppercase() {
        let mut it = replacement.chars();
        return it.next().map(|f| upper(&f.to_string(), loc) + it.as_str()).unwrap_or_default();
    }
    replacement.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserve_case_follows_turkish_letters_and_leaves_other_text_alone() {
        assert_eq!(preserve_case("İSTANBUL", "izmir"), "İZMİR");
        assert_eq!(preserve_case("ŞEHİR", "ılık"), "ILIK");
        assert_eq!(preserve_case("Şehir", "izmir"), "İzmir");
        assert_eq!(preserve_case("ılık", "IŞIK"), "ışık");
        // No Turkish letters: the usual rules, unchanged.
        assert_eq!(preserve_case("FILE", "item"), "ITEM");
        assert_eq!(preserve_case("File", "item"), "Item");
        assert_eq!(preserve_case("file", "ITEM"), "item");
        assert_eq!(preserve_case("Old_Name", "new_name"), "New_Name");
    }

    #[gpui::test]
    fn clicking_the_replacement_fields_empty_space_focuses_replacement(cx: &mut gpui::TestAppContext) {
        crate::qa_workbench_tests::globals(cx);
        let (bar, cx) = cx.add_window_view(|_, cx| FindBar::new(cx));
        bar.update_in(cx, |bar, window, cx| {
            bar.show_replace = true;
            bar.set_query("production", cx);
            bar.focus_query(window, cx);
            cx.notify();
        });
        cx.run_until_parked();
        let bounds = cx.debug_bounds("find-replace-box").unwrap();
        let at = gpui::point(bounds.left() + bounds.size.width * 0.75, bounds.center().y);
        cx.simulate_click(at, gpui::Modifiers::none());
        cx.run_until_parked();
        assert!(
            bar.update_in(cx, |bar, window, cx| bar.replacement.focus_handle(cx).is_focused(window)),
            "clicks inside the replacement field must not keep the search input focused"
        );
        cx.simulate_input("release");
        assert_eq!(bar.read_with(cx, |bar, cx| bar.replacement(cx)), "release");
        assert_eq!(bar.read_with(cx, |bar, cx| bar.query(cx)), "production");
    }

    #[gpui::test]
    fn find_controls_fit_narrow_editor_widths(cx: &mut gpui::TestAppContext) {
        crate::qa_workbench_tests::globals(cx);
        let (bar, cx) = cx.add_window_view(|_, cx| FindBar::new(cx));
        bar.update(cx, |bar, cx| {
            bar.show_replace = true;
            cx.notify();
        });
        for width in [480., 320., 240.] {
            cx.simulate_resize(gpui::size(px(width), px(600.)));
            cx.run_until_parked();
            for id in ["find-query-box", "find-replace-box", "f-close", "f-replace-all"] {
                let bounds = cx.debug_bounds(id).unwrap();
                assert!(bounds.left() >= px(0.) && bounds.right() <= px(width), "{id} exceeds {width}px: {bounds:?}");
            }
        }
    }

    #[test]
    fn preserve_case_follows_the_match() {
        assert_eq!(preserve_case("FOO", "bar"), "BAR");
        assert_eq!(preserve_case("foo", "BAR"), "bar");
        assert_eq!(preserve_case("Foo", "bar"), "Bar");
        assert_eq!(preserve_case("fOO", "bar"), "bar", "mixed: left as is");
        assert_eq!(preserve_case("Foo_Bar", "baz_qux"), "Baz_Qux");
        assert_eq!(preserve_case("foo-bar", "BAZ-QUX"), "baz-qux");
        assert_eq!(preserve_case("123", "Bar"), "Bar", "leave it alone when there are no letters");
        assert_eq!(preserve_case("Foo", ""), "");
    }

    fn re(pattern: &str, regex: bool, case: bool, word: bool) -> Regex {
        let q = proto::SearchQuery {
            root: String::new(),
            pattern: pattern.into(),
            regex,
            case_sensitive: case,
            whole_word: word,
            include: Vec::new(),
            exclude: Vec::new(),
            max_results: 0,
        };
        agent::search::compile(&q).unwrap()
    }

    #[test]
    fn finds_turkish_case_insensitive_and_whole_words() {
        let b = Buffer::from_str("İğne iğne\nliste IĞNE");
        assert_eq!(find_all(&b, &re("iğne", false, false, false)), [0..4, 5..9, 16..20]);
        assert_eq!(find_all(&b, &re("iğne", false, true, false)), vec![5..9]);
        let b = Buffer::from_str("foo food foo_bar foo");
        assert_eq!(find_all(&b, &re("foo", false, false, true)), [0..3, 17..20]);
    }

    #[test]
    fn regex_replacement_expands_groups() {
        let b = Buffer::from_str("let a = 1;\nlet bb = 22;");
        let r = re(r"let (\w+) = (\d+)", true, false, false);
        let ms = find_all(&b, &r);
        assert_eq!(ms.len(), 2);
        assert_eq!(replacement_for(&b, &r, &ms[1], "const $1: u8 = $2", true), "const bb: u8 = 22");
        assert_eq!(replacement_for(&b, &r, &ms[0], "$1\\n", true), "a\n");
        assert_eq!(replacement_for(&b, &r, &ms[0], "$1", false), "$1", "$ is not expanded in plain search");
    }
}
