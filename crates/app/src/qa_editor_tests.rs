//! QA (editor core): exploratory end-to-end tests that encode VS Code behaviour.
//! A failing test here is a reproduced defect; see the QA report for details.
//! Passing tests are kept as regression coverage.

use std::path::PathBuf;
use std::time::Instant;

use gpui::{Entity, EntityInputHandler, Focusable, Modifiers, MouseButton, TestAppContext, VisualTestContext};

use crate::editor::EditorView;
use crate::ui::ActiveTheme;
use crate::workbench::Workbench;

fn qa_dir(name: &str) -> PathBuf {
    let base = std::env::var("CODEYARD_QA_DIR").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("codeyard-qa-editor"));
    let dir = base.join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn file(name: &str, file_name: &str, content: &[u8]) -> PathBuf {
    let f = qa_dir(name).join(file_name);
    std::fs::write(&f, content).unwrap();
    f
}

fn setup(cx: &mut TestAppContext) -> (Entity<Workbench>, &mut VisualTestContext) {
    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(ActiveTheme(theme::DARK));
        cx.set_global(crate::settings::Settings { lsp_enabled: false, ..Default::default() });
    });
    let (wb, cx) = cx.add_window_view(|window, cx| Workbench::with_state(state::State::in_memory().ok(), None, window, cx));
    wb.update_in(cx, |wb, window, cx| window.focus(&wb.focus_handle(cx)));
    (wb, cx)
}

pub(crate) fn open(wb: &Entity<Workbench>, path: &std::path::Path, cx: &mut VisualTestContext) -> Entity<EditorView> {
    let p = path.to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_path(&p, window, cx));
    cx.run_until_parked();
    cx.run_until_parked();
    wb.read_with(cx, |wb, _| wb.active_editor_entity()).expect("editor opened")
}

fn text(ed: &Entity<EditorView>, cx: &mut VisualTestContext) -> String {
    ed.read_with(cx, |e, _| e.text())
}

fn sels(ed: &Entity<EditorView>, cx: &mut VisualTestContext) -> Vec<std::ops::Range<usize>> {
    ed.read_with(cx, |e, _| e.qa_selections())
}

fn click_count(cx: &mut VisualTestContext, at: gpui::Point<gpui::Pixels>, n: usize) {
    cx.simulate_event(gpui::MouseDownEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::none(), click_count: n, first_mouse: false });
    cx.simulate_event(gpui::MouseUpEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::none(), click_count: n });
}

// ---------------------------------------------------------------- clipboard

/// VS Code: ⌘C with an empty selection copies the whole line; ⌘V of such a
/// clipboard inserts it as a full line *above* the current line, the cursor stays.
#[gpui::test]
fn qa_copy_line_without_selection_pastes_as_whole_line_above(cx: &mut TestAppContext) {
    let f = file("copyline", "a.txt", b"alfa\nbeta\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("right right cmd-c down cmd-v");
    assert_eq!(text(&ed, cx), "alfa\nalfa\nbeta\n", "line pasted above, not spliced into 'be|ta'");
    assert_eq!(ed.read_with(cx, |e, _| e.cursor_position()), (3, 3));
}

/// Multi-cursor copy/paste with the same number of cursors: one line each (regression).
#[gpui::test]
fn qa_multi_cursor_copy_paste_distributes(cx: &mut TestAppContext) {
    let f = file("mcpaste", "a.txt", b"x1 y\nx2 y\nx3 y\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("alt-cmd-down alt-cmd-down alt-shift-right cmd-c end cmd-v");
    assert_eq!(text(&ed, cx), "x1 yx1\nx2 yx2\nx3 yx3\n");
}

// ---------------------------------------------------------------- multi-cursor

/// VS Code: ⌘D started from an empty selection searches whole-word + case-sensitive,
/// so "foo" does not pick the "foo" inside "foobar".
#[gpui::test]
fn qa_cmd_d_from_word_matches_whole_words_only(cx: &mut TestAppContext) {
    let f = file("cmdd-word", "a.txt", b"foo foobar foo\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("cmd-d cmd-d");
    assert_eq!(sels(&ed, cx), vec![0..3, 11..14], "second ⌘D must skip 'foo' inside 'foobar'");
}

/// Same for ⇧⌘L (select all occurrences of the word under the cursor).
#[gpui::test]
fn qa_select_all_occurrences_of_word_is_whole_word(cx: &mut TestAppContext) {
    let f = file("selall-word", "a.txt", b"ad adres ad\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("cmd-shift-l");
    assert_eq!(sels(&ed, cx), vec![0..2, 9..11]);
    cx.simulate_input("isim");
    assert_eq!(text(&ed, cx), "isim adres isim\n");
}

/// ⌥⌘↓ keeps the column across a short line (regression).
#[gpui::test]
fn qa_add_cursor_below_keeps_column_over_short_line(cx: &mut TestAppContext) {
    let f = file("addcursor", "a.txt", b"abcdef\nx\nabcdef\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("right right right right alt-cmd-down alt-cmd-down");
    assert_eq!(ed.read_with(cx, |e, _| e.cursors()), vec![4, 8, 13]);
}

// ---------------------------------------------------------------- IME / marked text

/// Dead key / IME composition (e.g. ⌥E then E → é): committing must be one undo step.
#[gpui::test]
fn qa_ime_composition_is_a_single_undo_step(cx: &mut TestAppContext) {
    let f = file("ime-undo", "a.txt", b"ab\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    ed.update_in(cx, |e, window, cx| e.replace_and_mark_text_in_range(None, "´", None, window, cx));
    ed.update_in(cx, |e, window, cx| e.replace_text_in_range(None, "é", window, cx));
    assert_eq!(text(&ed, cx), "éab\n");
    cx.simulate_keystrokes("cmd-z");
    assert_eq!(text(&ed, cx), "ab\n", "one ⌘Z removes the composed character, no intermediate '´' state");
}

/// VS Code composes in every cursor; codeyard collapses to one cursor on marked text.
#[gpui::test]
fn qa_ime_composition_with_multiple_cursors(cx: &mut TestAppContext) {
    let f = file("ime-multi", "a.txt", b"a\nb\nc\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("alt-cmd-down alt-cmd-down");
    assert_eq!(ed.read_with(cx, |e, _| e.cursor_count()), 3);
    ed.update_in(cx, |e, window, cx| e.replace_and_mark_text_in_range(None, "¨", None, window, cx));
    ed.update_in(cx, |e, window, cx| e.replace_text_in_range(None, "ü", window, cx));
    assert_eq!(text(&ed, cx), "üa\nüb\nüc\n");
    assert_eq!(ed.read_with(cx, |e, _| e.cursor_count()), 3);
}

/// Turkish characters typed with several cursors + undo (regression).
#[gpui::test]
fn qa_turkish_typing_multi_cursor_one_undo(cx: &mut TestAppContext) {
    let f = file("tr-multi", "a.txt", b"x\ny\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("alt-cmd-down");
    for ch in ["ğ", "ü", "ş", "İ", "ı", "ö", "ç"] {
        cx.simulate_input(ch);
    }
    assert_eq!(text(&ed, cx), "ğüşİıöçx\nğüşİıöçy\n");
    cx.simulate_keystrokes("cmd-z");
    assert_eq!(text(&ed, cx), "x\ny\n");
}

// ---------------------------------------------------------------- typing, undo

/// VS Code: consecutive Backspaces form one undo step.
#[gpui::test]
fn qa_consecutive_backspaces_undo_together(cx: &mut TestAppContext) {
    let f = file("bs-undo", "a.txt", b"hello\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("end backspace backspace backspace cmd-z");
    assert_eq!(text(&ed, cx), "hello\n");
}

/// VS Code: moving the cursor closes the current typing undo group.
#[gpui::test]
fn qa_cursor_move_breaks_undo_group(cx: &mut TestAppContext) {
    let f = file("move-undo", "a.txt", b"\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_input("ab");
    cx.simulate_keystrokes("left right");
    cx.simulate_input("cd");
    cx.simulate_keystrokes("cmd-z");
    assert_eq!(text(&ed, cx), "ab\n");
}

/// VS Code (editor.useTabStops): Backspace inside leading spaces deletes to the previous tab stop.
#[gpui::test]
fn qa_backspace_in_indentation_goes_to_tab_stop(cx: &mut TestAppContext) {
    let f = file("bs-indent", "a.rs", b"fn a() {\n        x\n}\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("down home backspace");
    assert_eq!(text(&ed, cx), "fn a() {\n    x\n}\n");
}

/// VS Code: Tab with no selection inserts spaces up to the next tab stop.
#[gpui::test]
fn qa_tab_inserts_to_next_tab_stop(cx: &mut TestAppContext) {
    let f = file("tab-stop", "a.rs", b"ab\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("end tab");
    cx.simulate_input("c");
    assert_eq!(text(&ed, cx), "ab  c\n");
}

/// VS Code (editor.trimAutoWhitespace): pressing Enter on an auto-indented empty line
/// leaves that line empty, not full of spaces.
#[gpui::test]
fn qa_enter_twice_trims_auto_indent(cx: &mut TestAppContext) {
    let f = file("trim-auto", "a.rs", b"fn a() {\n    x\n}\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("down end enter enter");
    assert_eq!(text(&ed, cx), "fn a() {\n    x\n\n    \n}\n");
}

/// VS Code (autoClosingOvertype: auto): only auto-inserted closers are typed over.
#[gpui::test]
fn qa_typing_closer_before_user_typed_closer_inserts(cx: &mut TestAppContext) {
    let f = file("overtype", "a.rs", b"x)\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("right");
    cx.simulate_input(")");
    assert_eq!(text(&ed, cx), "x))\n");
}

/// Auto-close + backspace of an empty pair + wrap selection (regression).
#[gpui::test]
fn qa_auto_close_pairs_regression(cx: &mut TestAppContext) {
    let f = file("pairs", "a.rs", b"word\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("shift-alt-right");
    cx.simulate_input("(");
    assert_eq!(text(&ed, cx), "(word)\n");
    cx.simulate_keystrokes("end");
    cx.simulate_input("[");
    assert_eq!(text(&ed, cx), "(word)[]\n");
    cx.simulate_keystrokes("backspace");
    assert_eq!(text(&ed, cx), "(word)\n");
}

// ---------------------------------------------------------------- line commands

/// VS Code: ⇧⌘K on the last line (no trailing newline) also removes the newline before it.
#[gpui::test]
fn qa_delete_last_line_without_trailing_newline(cx: &mut TestAppContext) {
    let f = file("del-last", "a.txt", b"a\nb");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("down cmd-shift-k");
    assert_eq!(text(&ed, cx), "a");
}

/// ⌘X with no selection on the last line (no trailing newline) – same as above in VS Code.
#[gpui::test]
fn qa_cut_last_line_without_trailing_newline(cx: &mut TestAppContext) {
    let f = file("cut-last", "a.txt", b"a\nb");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("down cmd-x");
    assert_eq!(text(&ed, cx), "a");
}

/// VS Code sorts with a locale-aware collator: "çay" sorts between "arı" and "dal".
#[gpui::test]
fn qa_sort_lines_uses_locale_collation(cx: &mut TestAppContext) {
    let f = file("sort-tr", "a.txt", "dal\nçay\narı\n".as_bytes());
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("cmd-a");
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Editor: Sort Lines Ascending", window, cx));
    cx.run_until_parked();
    assert_eq!(text(&ed, cx), "arı\nçay\ndal\n");
}

/// VS Code: ⌘/ in CSS toggles a block comment.
#[gpui::test]
fn qa_toggle_comment_in_css(cx: &mut TestAppContext) {
    let f = file("css-comment", "a.css", b"a { color: red; }\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("cmd-/");
    assert_eq!(text(&ed, cx), "/* a { color: red; } */\n");
}

/// VS Code: ⌘/ in HTML/Markdown toggles <!-- -->.
#[gpui::test]
fn qa_toggle_comment_in_html(cx: &mut TestAppContext) {
    let f = file("html-comment", "a.html", b"<p>x</p>\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("cmd-/");
    assert_eq!(text(&ed, cx), "<!-- <p>x</p> -->\n");
}

/// Move/copy lines with a multi-line selection; undo restores (regression).
#[gpui::test]
fn qa_move_and_copy_block_regression(cx: &mut TestAppContext) {
    let f = file("move-block", "a.txt", b"1\n2\n3\n4\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("shift-down shift-end alt-down");
    assert_eq!(text(&ed, cx), "3\n1\n2\n4\n");
    cx.simulate_keystrokes("shift-alt-down");
    assert_eq!(text(&ed, cx), "3\n1\n2\n1\n2\n4\n");
    cx.simulate_keystrokes("cmd-z cmd-z");
    assert_eq!(text(&ed, cx), "1\n2\n3\n4\n");
}

// ---------------------------------------------------------------- find / replace

/// VS Code: `$1_x` in a regex replacement is group 1 followed by "_x".
#[gpui::test]
fn qa_regex_replace_group_followed_by_word_char(cx: &mut TestAppContext) {
    let f = file("re-group", "a.txt", b"foo bar\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("cmd-f alt-r");
    let bar = ed.read_with(cx, |e, _| e.find_bar()).unwrap();
    bar.update(cx, |b, cx| {
        b.set_query(r"(\w+)", cx);
        b.set_replacement("$1_x", cx);
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("alt-cmd-f alt-cmd-enter");
    assert_eq!(text(&ed, cx), "foo_x bar_x\n");
}

/// Preserve case (⌥P): the replacement takes the letter case of each match.
#[gpui::test]
fn qa_replace_preserve_case(cx: &mut TestAppContext) {
    let f = file("re-preserve", "a.txt", b"foo Foo FOO\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("cmd-f alt-p");
    let bar = ed.read_with(cx, |e, _| e.find_bar()).unwrap();
    assert!(bar.read_with(cx, |b, _| b.preserve_case), "alt-p turns it on");
    bar.update(cx, |b, cx| {
        b.set_query("foo", cx);
        b.set_replacement("bar", cx);
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("alt-cmd-f alt-cmd-enter");
    assert_eq!(text(&ed, cx), "bar Bar BAR\n");
}

/// Replace All must replace every match, not only the first 10 000 (display cap).
#[gpui::test]
fn qa_replace_all_beyond_match_cap(cx: &mut TestAppContext) {
    let content = "ab\n".repeat(12_000);
    let f = file("replace-cap", "a.txt", content.as_bytes());
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("cmd-f");
    cx.simulate_input("ab");
    cx.simulate_keystrokes("alt-cmd-f");
    cx.simulate_input("cd");
    cx.simulate_keystrokes("alt-cmd-enter");
    let t = text(&ed, cx);
    assert_eq!(t.matches("ab").count(), 0, "{} matches left unreplaced", t.matches("ab").count());
}

/// Regex ^ anchors on every line (multi-line) (regression).
#[gpui::test]
fn qa_regex_caret_matches_each_line(cx: &mut TestAppContext) {
    let f = file("re-caret", "a.txt", b"foo\nfoo\nxfoo\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("cmd-f alt-r");
    cx.simulate_input("^foo");
    let bar = ed.read_with(cx, |e, _| e.find_bar()).unwrap();
    let status = bar.read_with(cx, |b, _| b.status_text().map(str::to_string));
    assert_eq!(status.as_deref(), Some("1 of 2"));
}

/// VS Code: Esc with the find widget open closes it even if a match is selected.
#[gpui::test]
fn qa_escape_closes_find_with_match_selected(cx: &mut TestAppContext) {
    let f = file("find-esc", "a.txt", b"one two one\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("cmd-f");
    cx.simulate_input("one");
    cx.simulate_keystrokes("enter");
    // Focus back to the editor (as after clicking into the text), match still selected.
    ed.update_in(cx, |e, window, cx| window.focus(&e.focus_handle(cx)));
    cx.simulate_keystrokes("escape");
    assert!(!ed.read_with(cx, |e, _| e.find_open()), "first Esc should close the find widget");
}

/// Find a match inside a folded region unfolds it (regression).
#[gpui::test]
fn qa_find_inside_fold_unfolds(cx: &mut TestAppContext) {
    let f = file("find-fold", "a.rs", b"fn a() {\n    gizli();\n}\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    ed.update(cx, |e, cx| e.toggle_fold_for_test(0, cx));
    cx.run_until_parked();
    assert_eq!(ed.read_with(cx, |e, _| e.row_counts()).0, 3);
    cx.simulate_keystrokes("cmd-f");
    cx.simulate_input("gizli");
    cx.run_until_parked();
    assert_eq!(ed.read_with(cx, |e, _| e.row_counts()).0, 4);
}

// ---------------------------------------------------------------- line breaks / encodings

/// A form feed (^L, common in C/Python/Elisp sources) is not a line break in VS Code.
#[gpui::test]
fn qa_form_feed_is_not_a_line_break(cx: &mut TestAppContext) {
    let f = file("formfeed", "a.c", b"a\x0cb\nc\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    assert_eq!(ed.read_with(cx, |e, _| e.line_count()), 3, "lines: 'a^Lb', 'c', ''");
    cx.simulate_keystrokes("end");
    cx.simulate_input("X");
    assert_eq!(text(&ed, cx), "a\x0cbX\nc\n");
}

/// U+2028 / NEL inside a line must not split it either (VS Code shows them inline).
#[gpui::test]
fn qa_unicode_line_separator_is_not_a_line_break(cx: &mut TestAppContext) {
    let f = file("u2028", "a.js", "s = \"a\u{2028}b\";\nnext\n".as_bytes());
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    assert_eq!(ed.read_with(cx, |e, _| e.line_count()), 3);
}

/// Classic-Mac CR-only file: VS Code treats lone CR as a line end; End stays on the line.
#[gpui::test]
fn qa_cr_only_line_endings(cx: &mut TestAppContext) {
    let f = file("cr-only", "a.txt", b"one\rtwo\r");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("end");
    cx.simulate_input("!");
    cx.simulate_keystrokes("cmd-s");
    cx.run_until_parked();
    assert_eq!(std::fs::read(&f).unwrap(), b"one!\rtwo\r");
    let _ = ed;
}

/// UTF-16 LE with BOM round-trips (regression).
#[gpui::test]
fn qa_utf16le_bom_roundtrip(cx: &mut TestAppContext) {
    let mut bytes = vec![0xFF, 0xFE];
    for u in "Işık\r\n".encode_utf16() {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    let f = file("utf16", "a.txt", &bytes);
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    assert_eq!(text(&ed, cx), "Işık\r\n");
    cx.simulate_input("ğ");
    cx.simulate_keystrokes("cmd-s");
    cx.run_until_parked();
    let mut want = vec![0xFF, 0xFE];
    for u in "ğIşık\r\n".encode_utf16() {
        want.extend_from_slice(&u.to_le_bytes());
    }
    assert_eq!(std::fs::read(&f).unwrap(), want);
}

/// Empty file: typing, undo back to clean, save keeps it empty (regression).
#[gpui::test]
fn qa_empty_file_edit_undo_clean(cx: &mut TestAppContext) {
    let f = file("empty", "a.txt", b"");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("up down cmd-up cmd-down home end backspace delete cmd-shift-k alt-up alt-down cmd-/ ctrl-j");
    assert_eq!(text(&ed, cx), "");
    cx.simulate_input("x");
    cx.simulate_keystrokes("cmd-z");
    assert!(!ed.read_with(cx, |e, _| e.is_dirty()));
}

// ---------------------------------------------------------------- mouse

/// VS Code: dragging past the bottom edge keeps scrolling and extending the selection.
#[gpui::test]
fn qa_drag_select_autoscrolls_past_viewport(cx: &mut TestAppContext) {
    let content: String = (0..400).map(|i| format!("satır {i}\n")).collect();
    let f = file("drag-scroll", "a.txt", content.as_bytes());
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    let (_, last_visible) = ed.read_with(cx, |e, _| e.qa_visible_lines()).unwrap();
    let at = ed.read_with(cx, |e, _| e.point_for(0)).unwrap();
    let last_start = (0..last_visible).map(|i| format!("satır {i}\n").chars().count()).sum::<usize>();
    let bottom = ed.read_with(cx, |e, _| e.point_for(last_start)).unwrap();
    cx.simulate_event(gpui::MouseDownEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::none(), click_count: 1, first_mouse: false });
    // Drag to the last visible line: selection reaches it.
    cx.simulate_event(gpui::MouseMoveEvent { position: bottom, pressed_button: Some(MouseButton::Left), modifiers: Modifiers::none() });
    cx.run_until_parked();
    assert_eq!(ed.read_with(cx, |e, _| e.cursor_position()).0, last_visible + 1, "drag inside the view works");
    // Keep the button down just below the text area (status bar): VS Code keeps scrolling.
    for i in 0..40 {
        let pos = gpui::point(bottom.x, bottom.y + gpui::px(30. + (i % 3) as f32));
        cx.simulate_event(gpui::MouseMoveEvent { position: pos, pressed_button: Some(MouseButton::Left), modifiers: Modifiers::none() });
        cx.run_until_parked();
    }
    let (line, _) = ed.read_with(cx, |e, _| e.cursor_position());
    assert!(line > last_visible + 10, "selection stuck at line {line} (last visible line {})", last_visible + 1);
}

/// VS Code: double-click then drag extends the selection word by word.
#[gpui::test]
fn qa_double_click_drag_extends_by_words(cx: &mut TestAppContext) {
    let f = file("dbl-drag", "a.txt", b"alpha beta gamma\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    let a = ed.read_with(cx, |e, _| e.point_for(2)).unwrap();
    let g = ed.read_with(cx, |e, _| e.point_for(13)).unwrap();
    cx.simulate_event(gpui::MouseDownEvent { button: MouseButton::Left, position: a, modifiers: Modifiers::none(), click_count: 2, first_mouse: false });
    cx.simulate_event(gpui::MouseMoveEvent { position: g, pressed_button: Some(MouseButton::Left), modifiers: Modifiers::none() });
    cx.simulate_event(gpui::MouseUpEvent { button: MouseButton::Left, position: g, modifiers: Modifiers::none(), click_count: 2 });
    assert_eq!(ed.read_with(cx, |e, _| e.selected_text()).as_deref(), Some("alpha beta gamma"));
}

/// Shift+click extends; triple click selects line (regression).
#[gpui::test]
fn qa_shift_click_and_triple_click(cx: &mut TestAppContext) {
    let f = file("clicks", "a.txt", b"first line\nsecond line\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    let p = ed.read_with(cx, |e, _| e.point_for(14)).unwrap();
    cx.simulate_click(p, Modifiers::shift());
    assert_eq!(ed.read_with(cx, |e, _| e.selected_text()).as_deref(), Some("first line\nsec"));
    click_count(cx, p, 3);
    assert_eq!(ed.read_with(cx, |e, _| e.selected_text()).as_deref(), Some("second line\n"));
}

// ---------------------------------------------------------------- keyboard navigation

/// VS Code: ⇧PageDown selects a page.
#[gpui::test]
fn qa_shift_page_down_selects(cx: &mut TestAppContext) {
    let content: String = (0..200).map(|i| format!("{i}\n")).collect();
    let f = file("shift-pgdn", "a.txt", content.as_bytes());
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("shift-pagedown");
    assert!(ed.read_with(cx, |e, _| e.selection_len()).is_some_and(|n| n > 10), "no selection after ⇧PageDown");
}

/// VS Code (macOS): ⌘⌫ deletes to the line start (no Linux / Windows binding).
#[cfg(target_os = "macos")]
#[gpui::test]
fn qa_cmd_backspace_deletes_to_line_start(cx: &mut TestAppContext) {
    let f = file("cmd-bs", "a.txt", b"hello world\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("alt-right alt-right cmd-backspace");
    assert_eq!(text(&ed, cx), "\n");
}

/// VS Code: ⌘↵ inserts a line below, ⇧⌘↵ above, wherever the cursor is.
#[gpui::test]
fn qa_cmd_enter_inserts_line_below(cx: &mut TestAppContext) {
    let f = file("cmd-enter", "a.txt", b"abc\nxyz\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("right cmd-enter");
    cx.simulate_input("N");
    assert_eq!(text(&ed, cx), "abc\nN\nxyz\n");
}

/// VS Code: ⌘] / ⌘[ indent / outdent the current line without a selection.
#[gpui::test]
fn qa_cmd_bracket_indents_line(cx: &mut TestAppContext) {
    let f = file("cmd-bracket", "a.rs", b"x\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("cmd-]");
    assert_eq!(text(&ed, cx), "    x\n");
}

/// VS Code: ⇧⌘\ jumps to the matching bracket.
#[gpui::test]
fn qa_jump_to_matching_bracket(cx: &mut TestAppContext) {
    let f = file("jump-bracket", "a.rs", b"fn a() { x }\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("end left cmd-shift-\\");
    assert_eq!(ed.read_with(cx, |e, _| e.cursors()), vec![7]);
}

/// VS Code: ⌘U undoes the last cursor operation (e.g. one ⌘D too many).
#[gpui::test]
fn qa_cursor_undo(cx: &mut TestAppContext) {
    let f = file("cursor-undo", "a.txt", b"x x x\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("cmd-d cmd-d cmd-d cmd-u");
    assert_eq!(ed.read_with(cx, |e, _| e.cursor_count()), 2);
}

/// VS Code: ⇧⌥I puts a cursor at the end of every selected line.
#[gpui::test]
fn qa_cursors_at_line_ends(cx: &mut TestAppContext) {
    let f = file("line-ends", "a.txt", b"a\nbb\nccc\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("cmd-a shift-alt-i");
    assert_eq!(ed.read_with(cx, |e, _| e.cursor_count()), 3);
}

/// VS Code: Esc after ⌥⌘↓⌥⌘↓ keeps the *original* (primary) cursor.
#[gpui::test]
fn qa_escape_keeps_original_cursor(cx: &mut TestAppContext) {
    let f = file("esc-primary", "a.txt", b"a\nb\nc\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("alt-cmd-down alt-cmd-down escape");
    assert_eq!(ed.read_with(cx, |e, _| e.cursor_position()), (1, 1));
}

/// VS Code: Go to Line accepts `line:column`.
#[gpui::test]
fn qa_go_to_line_with_column(cx: &mut TestAppContext) {
    let f = file("goto-col", "a.txt", b"abc\ndefgh\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("ctrl-g");
    cx.simulate_input("2:4");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(ed.read_with(cx, |e, _| e.cursor_position()), (2, 4));
}

/// Multi-cursor + auto-close (regression).
#[gpui::test]
fn qa_multi_cursor_auto_close(cx: &mut TestAppContext) {
    let f = file("mc-pairs", "a.rs", b"f\ng\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("alt-cmd-down end");
    cx.simulate_input("(");
    cx.simulate_input("1");
    cx.simulate_input(")");
    cx.simulate_input(";");
    assert_eq!(text(&ed, cx), "f(1);\ng(1);\n");
}

/// No trailing newline is preserved on save; CRLF paste into a CRLF file (regression).
#[gpui::test]
fn qa_no_final_newline_and_crlf_paste(cx: &mut TestAppContext) {
    let f = file("no-eol", "a.txt", b"x\r\ny");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("p\r\nq".into()));
    cx.simulate_keystrokes("cmd-down cmd-v cmd-s");
    cx.run_until_parked();
    assert_eq!(std::fs::read(&f).unwrap(), b"x\r\nyp\r\nq");
    let _ = ed;
}

/// VS Code whole-word search also works when the query ends in punctuation.
#[gpui::test]
fn qa_whole_word_with_punctuation_edge(cx: &mut TestAppContext) {
    let f = file("ww-punct", "a.txt", b"a = foo( 1)\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("cmd-f alt-w");
    cx.simulate_input("foo(");
    let bar = ed.read_with(cx, |e, _| e.find_bar()).unwrap();
    let status = bar.read_with(cx, |b, _| b.status_text().map(str::to_string));
    assert_eq!(status.as_deref(), Some("1 of 1"));
}

// ---------------------------------------------------------------- word wrap

/// VS Code: with word wrap on, End goes to the end of the *visual* row first.
#[gpui::test]
fn qa_end_on_wrapped_line_stops_at_row_end(cx: &mut TestAppContext) {
    let long = "kelime ".repeat(60);
    let f = file("wrap-end", "a.txt", format!("{long}\n").as_bytes());
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("alt-z");
    cx.run_until_parked();
    cx.run_until_parked();
    let (rows, lines) = ed.read_with(cx, |e, _| e.row_counts());
    assert!(rows > lines);
    cx.simulate_keystrokes("end");
    let (_, col) = ed.read_with(cx, |e, _| e.cursor_position());
    assert!(col < long.chars().count(), "End jumped to the end of the logical line (col {col})");
}

/// Shift+Down across wrapped rows selects one screen row at a time (regression).
#[gpui::test]
fn qa_shift_down_on_wrapped_rows(cx: &mut TestAppContext) {
    let long = "kelime ".repeat(60);
    let f = file("wrap-sel", "a.txt", format!("{long}\nson\n").as_bytes());
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("alt-z");
    cx.run_until_parked();
    cx.run_until_parked();
    cx.simulate_keystrokes("shift-down");
    let n = ed.read_with(cx, |e, _| e.selection_len()).unwrap_or(0);
    assert!(n > 0 && n < long.len(), "selected {n}");
}

/// Folding and word wrap together: a fold hides all wrapped rows of hidden lines (regression).
#[gpui::test]
fn qa_fold_with_wrap(cx: &mut TestAppContext) {
    let long = "kelime ".repeat(60);
    let f = file("fold-wrap", "a.rs", format!("fn a() {{\n    {long}\n}}\n").as_bytes());
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("alt-z");
    cx.run_until_parked();
    cx.run_until_parked();
    ed.update(cx, |e, cx| e.toggle_fold_for_test(0, cx));
    cx.run_until_parked();
    assert_eq!(ed.read_with(cx, |e, _| e.row_counts()).0, 3);
    cx.simulate_keystrokes("down");
    assert_eq!(ed.read_with(cx, |e, _| e.cursor_position()).0, 3);
}

// ---------------------------------------------------------------- folding edits

/// Enter at the very start of a folded header pushes the fold down with the header (regression).
#[gpui::test]
fn qa_enter_before_folded_header_keeps_fold(cx: &mut TestAppContext) {
    let f = file("fold-enter", "a.rs", b"fn a() {\n    x;\n    y;\n}\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    ed.update(cx, |e, cx| e.toggle_fold_for_test(0, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-up enter");
    cx.run_until_parked();
    assert_eq!(ed.read_with(cx, |e, _| e.fold_headers()), vec![1]);
    assert_eq!(ed.read_with(cx, |e, _| e.row_counts()), (4, 6));
}

/// Folds are per view; editing in the other group moves them along (regression).
#[gpui::test]
fn qa_fold_in_left_follows_edit_in_right(cx: &mut TestAppContext) {
    let f = file("fold-split", "a.rs", b"fn a() {\n    x;\n}\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    ed.update(cx, |e, cx| e.toggle_fold_for_test(0, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-\\");
    cx.run_until_parked();
    assert_eq!(ed.read_with(cx, |e, _| e.fold_headers()), vec![0], "split copies the view state (as VS Code)");
    cx.simulate_keystrokes("cmd-up");
    cx.simulate_input("// a\n// b\n");
    cx.simulate_keystrokes("cmd-1");
    cx.run_until_parked();
    assert_eq!(ed.read_with(cx, |e, _| e.fold_headers()), vec![2]);
}

// ---------------------------------------------------------------- same file in two groups

/// The other view's selection follows edits made in this view (regression).
#[gpui::test]
fn qa_other_view_selection_follows_edits(cx: &mut TestAppContext) {
    let f = file("two-sel", "a.txt", b"bir iki uc\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("alt-right alt-right alt-shift-left"); // "iki" selected on the left
    cx.simulate_keystrokes("cmd-\\ cmd-up");
    cx.simulate_input(">>>");
    cx.simulate_keystrokes("cmd-1");
    assert_eq!(ed.read_with(cx, |e, _| e.selected_text()).as_deref(), Some("iki"));
    // Undo from the left removes the right view's insertion (one undo stack per document).
    cx.simulate_keystrokes("cmd-z");
    assert_eq!(text(&ed, cx), "bir iki uc\n");
}

/// Replace All in one view while the other view's find bar is open: counts stay right.
#[gpui::test]
fn qa_find_bars_in_both_views_after_replace(cx: &mut TestAppContext) {
    let f = file("two-find", "a.txt", b"a a a\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("cmd-f");
    cx.simulate_input("a");
    cx.simulate_keystrokes("escape");
    ed.update_in(cx, |e, window, cx| window.focus(&e.focus_handle(cx)));
    cx.simulate_keystrokes("cmd-\\ cmd-f");
    cx.simulate_input("a");
    cx.simulate_keystrokes("alt-cmd-f");
    cx.simulate_input("b");
    cx.simulate_keystrokes("alt-cmd-enter");
    assert_eq!(text(&ed, cx), "b b b\n");
}

// ---------------------------------------------------------------- large / long

/// A 200 000-character single line: End, typing and Home stay responsive.
#[gpui::test]
fn qa_very_long_single_line(cx: &mut TestAppContext) {
    let long = "abcdefghij".repeat(20_000);
    let f = file("longline", "a.txt", format!("{long}\n").as_bytes());
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    let t = Instant::now();
    cx.simulate_keystrokes("end");
    cx.run_until_parked();
    for _ in 0..20 {
        cx.simulate_input("x");
        cx.run_until_parked();
    }
    cx.simulate_keystrokes("home");
    cx.run_until_parked();
    let ms = t.elapsed().as_millis();
    eprintln!("QA long line: end + 20 chars + home = {ms} ms");
    assert_eq!(ed.read_with(cx, |e, _| e.cursor_position()), (1, 1));
    assert!(ms < 3000 * u128::from(crate::test_sshd::timeout_scale()), "{ms} ms");
}

// ---------------------------------------------------------------- performance (50k lines)

fn big_rust(lines: usize) -> String {
    let mut s = String::new();
    let mut i = 0;
    while i < lines {
        s.push_str(&format!("fn f{i}() {{\n    let a = {i};\n    if a > 1 {{\n        x(a);\n    }}\n}}\n"));
        i += 6;
    }
    s
}

fn time(label: &str, cx: &mut VisualTestContext, n: usize, mut f: impl FnMut(&mut VisualTestContext)) -> f64 {
    let t = Instant::now();
    for _ in 0..n {
        f(cx);
        cx.run_until_parked();
    }
    let per = t.elapsed().as_secs_f64() * 1000. / n as f64;
    eprintln!("QA perf {label}: {per:.1} ms/op");
    per
}

#[gpui::test]
fn qa_perf_50k_lines(cx: &mut TestAppContext) {
    let content = big_rust(50_000);
    eprintln!("QA perf file: {} bytes, {} lines", content.len(), content.lines().count());
    let f = file("perf", "big.rs", content.as_bytes());
    let (wb, cx) = setup(cx);
    // Measured: the UI thread's work. With a language server on, run_until_parked would also wait for its
    // reply (in the background, ~20 ms, doesn't block typing); it is measured off.
    cx.update(|_, cx| cx.global_mut::<crate::settings::Settings>().lsp_enabled = false);
    let t = Instant::now();
    let ed = open(&wb, &f, cx);
    eprintln!("QA perf open: {} ms", t.elapsed().as_millis());
    let mut worst: Vec<(String, f64)> = Vec::new();
    let mut rec = |l: &str, v: f64| worst.push((l.to_string(), v));
    rec("down", time("down", cx, 30, |cx| cx.simulate_keystrokes("down")));
    rec("type", time("type char (top)", cx, 30, |cx| cx.simulate_input("x")));
    rec("enter", time("enter", cx, 10, |cx| cx.simulate_keystrokes("enter")));
    rec("cmd-down", time("cmd-down", cx, 3, |cx| cx.simulate_keystrokes("cmd-down cmd-up")));
    rec("alt-down", time("move line", cx, 10, |cx| cx.simulate_keystrokes("alt-down")));
    rec("undo", time("undo", cx, 10, |cx| cx.simulate_keystrokes("cmd-z")));
    rec("pagedown", time("pagedown", cx, 20, |cx| cx.simulate_keystrokes("pagedown")));
    cx.simulate_keystrokes("cmd-up");
    rec(
        "cmd-f",
        time("cmd-f + query", cx, 1, |cx| {
            cx.simulate_keystrokes("cmd-f");
            cx.simulate_input("let");
        }),
    );
    ed.update_in(cx, |e, window, cx| window.focus(&e.focus_handle(cx)));
    rec("type+find", time("type char with find bar open", cx, 20, |cx| cx.simulate_input("y")));
    cx.simulate_keystrokes("escape escape");
    rec(
        "cmd-shift-l",
        time("select all occurrences (word 'let')", cx, 1, |cx| {
            cx.simulate_keystrokes("cmd-up down right right right right right cmd-shift-l escape");
        }),
    );
    rec("fold-all", time("fold all", cx, 1, |cx| cx.simulate_keystrokes("cmd-k cmd-0")));
    rec("down-folded", time("down with everything folded", cx, 20, |cx| cx.simulate_keystrokes("down")));
    rec("type-folded", time("type with everything folded", cx, 20, |cx| cx.simulate_input("z")));
    cx.simulate_keystrokes("cmd-k cmd-j");
    rec("wrap", time("toggle wrap", cx, 1, |cx| cx.simulate_keystrokes("alt-z")));
    rec("type-wrap", time("type with wrap on", cx, 20, |cx| cx.simulate_input("w")));
    // Same size as plain text (no tree-sitter): isolates the syntax cost of a keystroke.
    // Scrolling is turned off: otherwise this measures scrolling, not syntax.
    cx.simulate_keystrokes("alt-z");
    let f = file("perf-txt", "big.txt", content.as_bytes());
    open(&wb, &f, cx);
    rec("type-txt", time("type char (plain .txt, same size)", cx, 30, |cx| cx.simulate_input("x")));
    // The budget is one frame (60 Hz) and only meaningful in release: debug builds are 10-20x
    // slower and affected by the load of parallel tests. `cargo test --release -- qa_perf`.
    if cfg!(debug_assertions) {
        eprintln!("SKIPPED the frame-budget assertion: debug build (run `cargo test --release -- qa_perf`)");
        return;
    }
    // Shared CI runners get their allowance (`CODEYARD_TEST_TIMEOUT_SCALE`), as the other wall-clock budgets do.
    let budget = 16.7 * crate::test_sshd::timeout_scale() as f64;
    let slow: Vec<_> = worst.iter().filter(|(l, v)| *v > budget && !matches!(l.as_str(), "fold-all" | "cmd-f" | "cmd-shift-l" | "wrap")).collect();
    assert!(slow.is_empty(), "per-keystroke ops slower than {budget} ms: {slow:?}");
}

// ---------------------------------------------------------------- robustness (random keys)

/// Pseudo-random editing keys over a small Rust file (multi-cursor, line commands,
/// folding, wrap, clipboard), then undo everything: no panic and the original text back;
/// redo everything: the edited text back.
fn random_session(seed: u64, steps: usize, split: bool, cx: &mut TestAppContext) {
    let original = "fn a() {\n    let x = (1, 2);\n    if x { y(\"s\"); }\n}\n\nfn b() {\n\tz();\n}\n";
    random_session_on(seed, steps, split, "a.rs", original, cx);
}

fn random_session_on(seed: u64, steps: usize, split: bool, name: &str, original: &str, cx: &mut TestAppContext) {
    const KEYS: &[&str] = &[
        "left",
        "right",
        "up",
        "down",
        "shift-left",
        "shift-right",
        "shift-up",
        "shift-down",
        "alt-left",
        "alt-right",
        "alt-shift-left",
        "alt-shift-right",
        "home",
        "end",
        "shift-home",
        "shift-end",
        "cmd-up",
        "cmd-down",
        "pageup",
        "pagedown",
        "cmd-d",
        "alt-cmd-down",
        "alt-cmd-up",
        "escape",
        "backspace",
        "delete",
        "enter",
        "tab",
        "shift-tab",
        "cmd-/",
        "alt-up",
        "alt-down",
        "shift-alt-up",
        "shift-alt-down",
        "cmd-shift-k",
        "cmd-l",
        "ctrl-j",
        "cmd-x",
        "cmd-c",
        "cmd-v",
        "cmd-shift-l",
        "alt-cmd-[",
        "alt-cmd-]",
        "alt-backspace",
        "alt-delete",
        "ctrl-shift-cmd-right",
        "ctrl-shift-cmd-left",
        "cmd-a",
        "alt-z",
        "cmd-k cmd-0",
        "cmd-k cmd-j",
    ];
    const INPUT: &[&str] = &["a", "(", "\"", "ğ", " ", "}", "{", "İ", "\t", "x\ny", "😀"];
    let f = file(&format!("random-{seed}"), name, original.as_bytes());
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    if split {
        cx.simulate_keystrokes("cmd-\\");
    }
    let mut state = seed;
    let mut next = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (state >> 33) as usize
    };
    for _ in 0..steps {
        let r = next();
        if split && r % 17 == 0 {
            cx.simulate_keystrokes(if next() % 2 == 0 { "cmd-1" } else { "cmd-2" });
        } else if r % 4 == 0 {
            cx.simulate_input(INPUT[next() % INPUT.len()]);
        } else {
            cx.simulate_keystrokes(KEYS[next() % KEYS.len()]);
        }
    }
    cx.run_until_parked();
    let edited = text(&ed, cx);
    for _ in 0..steps * 2 {
        cx.simulate_keystrokes("cmd-z");
    }
    assert_eq!(text(&ed, cx), original, "undo all must restore the original (seed {seed})");
    for _ in 0..steps * 2 {
        cx.simulate_keystrokes("cmd-shift-z");
    }
    assert_eq!(text(&ed, cx), edited, "redo all must restore the edited text (seed {seed})");
}

/// The same random session, with the commands added this round (column selection, fold levels,
/// toggle comment, delete to end of line, previous match, go to change) and on an HTML
/// file (linked tags, Emmet Tab). No panics, undo/redo restores the text.
fn random_session_new(seed: u64, steps: usize, name: &str, original: &str, cx: &mut TestAppContext) {
    const KEYS: &[&str] = &[
        "left",
        "right",
        "up",
        "down",
        "shift-left",
        "shift-right",
        "shift-down",
        "home",
        "end",
        "cmd-up",
        "cmd-down",
        "cmd-d",
        "escape",
        "backspace",
        "delete",
        "enter",
        "tab",
        "shift-tab",
        "cmd-/",
        "cmd-x",
        "cmd-c",
        "cmd-v",
        "shift-alt-cmd-down",
        "shift-alt-cmd-up",
        "shift-alt-cmd-left",
        "shift-alt-cmd-right",
        "cmd-k cmd-1",
        "cmd-k cmd-2",
        "cmd-k cmd-3",
        "cmd-k cmd-[",
        "cmd-k cmd-]",
        "cmd-k cmd-0",
        "cmd-k cmd-j",
        "cmd-k cmd-k",
        "cmd-k cmd-c",
        "cmd-k cmd-u",
        "cmd-k cmd-d",
        "alt-f3",
        "shift-alt-f3",
        "alt-z",
        "cmd-shift-\\",
    ];
    const INPUT: &[&str] = &["a", "<", ">", "/", "div", "ul>li*2", ".x", "(", "\"", "ğ", " ", "😀", "m10", "{", "}"];
    let f = file(&format!("random-new-{seed}"), name, original.as_bytes());
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    let mut state = seed;
    let mut next = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (state >> 33) as usize
    };
    for _ in 0..steps {
        let r = next();
        if r % 3 == 0 {
            cx.simulate_input(INPUT[next() % INPUT.len()]);
        } else {
            cx.simulate_keystrokes(KEYS[next() % KEYS.len()]);
        }
    }
    cx.run_until_parked();
    let edited = text(&ed, cx);
    for _ in 0..steps * 3 {
        cx.simulate_keystrokes("cmd-z");
    }
    assert_eq!(text(&ed, cx), original, "undo all must restore the original (seed {seed}, {name})");
    for _ in 0..steps * 3 {
        cx.simulate_keystrokes("cmd-shift-z");
    }
    assert_eq!(text(&ed, cx), edited, "redo all must restore the edited text (seed {seed}, {name})");
}

const RS_SRC: &str = "fn a() {\n    let x = (1, 2);\n    if x { y(\"s\"); }\n}\n\nfn b() {\n\tz();\n}\n";
const HTML_SRC: &str = "<div class=\"a\">\n  <ul>\n    <li>one</li>\n    <li>two</li>\n  </ul>\n  <p>x</p>\n</div>\n\n";

#[gpui::test]
fn qa_random_new_commands_rs_11(cx: &mut TestAppContext) {
    random_session_new(11, 2000, "a.rs", RS_SRC, cx);
}

#[gpui::test]
fn qa_random_new_commands_rs_12(cx: &mut TestAppContext) {
    random_session_new(12, 2000, "a.rs", RS_SRC, cx);
}

#[gpui::test]
fn qa_random_new_commands_html_13(cx: &mut TestAppContext) {
    random_session_new(13, 2000, "a.html", HTML_SRC, cx);
}

#[gpui::test]
fn qa_random_new_commands_html_14(cx: &mut TestAppContext) {
    random_session_new(14, 2000, "a.html", HTML_SRC, cx);
}

#[gpui::test]
fn qa_random_new_commands_css_15(cx: &mut TestAppContext) {
    random_session_new(15, 1500, "a.css", "a {\n  color: red;\n}\n\nb { m: 1 }\n", cx);
}

/// `CODEYARD_QA_SEEDS=40` runs many seeds (default 3); each seed on an rs, html and css file.
#[gpui::test]
fn qa_random_new_commands_many_seeds(cx: &mut TestAppContext) {
    let n: u64 = std::env::var("CODEYARD_QA_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(3);
    let base: u64 = std::env::var("CODEYARD_QA_SEED_BASE").ok().and_then(|v| v.parse().ok()).unwrap_or(100);
    for seed in base..base + n {
        random_session_new(seed, 600, "a.rs", RS_SRC, cx);
        random_session_new(seed + 1000, 600, "a.html", HTML_SRC, cx);
        random_session_new(seed + 2000, 400, "a.css", "a {\n  color: red;\n}\n", cx);
    }
}

/// Text that has broken editors elsewhere: bidirectional overrides, combining marks, joined emoji, every kind of line end,
/// NUL, a very long line and a file without a final newline. Random editing must not panic, and undo / redo restore it.
#[gpui::test]
fn qa_random_keys_on_hostile_text(cx: &mut TestAppContext) {
    let long = "x".repeat(50_000);
    let texts = [
        ("bidi.rs", "let s = \"\u{202e}gnirts\u{202c}\";\n// \u{2066}a\u{2069} \u{200f}b\n".to_string()),
        ("marks.txt", "e\u{301}\u{301}\u{301} a\u{20d7}\n\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467} \u{1f3f3}\u{fe0f}\u{200d}\u{1f308}\n".to_string()),
        // Mixed line ends are made uniform on purpose (see `buffer`), so this file keeps one kind, plus the Unicode separators.
        ("ends.txt", "a\r\nb\r\nc\r\nd\u{2028}e\u{85}f\r\n\r\n".to_string()),
        // NUL makes a file binary (it opens in the hex view), so only the other control characters are here.
        ("controls.txt", "a\u{1}b\u{8}\n\u{7f}\u{1b}[31m\u{c}\n".to_string()),
        ("long.txt", format!("{long}\n{long}")),
        ("tabs.py", "\t\t \tdef f():\n\t  \treturn\t1".to_string()),
    ];
    for (i, (name, text)) in texts.iter().enumerate() {
        random_session_on(500 + i as u64, 250, i % 2 == 1, name, text, cx);
    }
}

#[gpui::test]
fn qa_random_keys_seed_1(cx: &mut TestAppContext) {
    random_session(1, 3000, false, cx);
}

#[gpui::test]
fn qa_random_keys_seed_2(cx: &mut TestAppContext) {
    random_session(2, 3000, false, cx);
}

#[gpui::test]
fn qa_random_keys_seed_3(cx: &mut TestAppContext) {
    random_session(3, 3000, false, cx);
}

#[gpui::test]
fn qa_random_keys_split_seed_4(cx: &mut TestAppContext) {
    random_session(4, 3000, true, cx);
}

#[gpui::test]
fn qa_random_keys_split_seed_5(cx: &mut TestAppContext) {
    random_session(5, 3000, true, cx);
}

#[gpui::test]
fn qa_cursor_blinks_resets_on_typing_and_settles(cx: &mut TestAppContext) {
    let f = file("blink", "a.txt", b"abc\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    let on = |cx: &mut VisualTestContext| ed.update(cx, |e, cx| e.blink_for_test(true, cx));
    let tick = |cx: &mut VisualTestContext, ms: u64| {
        cx.executor().advance_clock(std::time::Duration::from_millis(ms));
        cx.run_until_parked();
    };
    assert!(on(cx));
    tick(cx, 540);
    assert!(!on(cx), "off at half a period");
    tick(cx, 540);
    assert!(on(cx), "on again after that");
    tick(cx, 540);
    assert!(!on(cx));
    // Typing resets the cycle: the cursor is visible at once.
    ed.update_in(cx, |e, window, cx| e.replace_text_in_range(None, "x", window, cx));
    assert!(on(cx), "visible while typing");
    // Left untouched for long it stays solid, and the timer ends too.
    tick(cx, 40_000);
    assert!(on(cx), "steady when idle");
    // Setting "solid": never fades.
    cx.update(|_, cx| cx.global_mut::<crate::settings::Settings>().cursor_blinking = false);
    ed.update_in(cx, |e, window, cx| e.replace_text_in_range(None, "y", window, cx));
    assert!(on(cx));
    tick(cx, 600);
    assert!(on(cx), "solid: stays on");
}

/// In a file over 4 MB the scan runs in the background; commands must still work with the current list
/// (a stale line number must not delete the wrong region).
#[gpui::test]
fn qa_conflicts_in_a_big_file_scan_off_thread_and_commands_stay_exact(cx: &mut TestAppContext) {
    let filler = "let x = 1; // dolgu satırı, işaret yok\n".repeat(130_000); // ~5 MB
    let content = format!("<<<<<<< HEAD\nbiz\n=======\nonlar\n>>>>>>> dal\n{filler}");
    let f = file("bigconflict", "big.txt", content.as_bytes());
    let (wb, cx) = setup(cx);
    cx.update(|_, cx| cx.global_mut::<crate::settings::Settings>().lsp_enabled = false);
    let ed = open(&wb, &f, cx);
    assert_eq!(ed.read_with(cx, |e, _| e.conflict_count()), 1, "scanned on open");
    // Add two lines above the block: still the right block even if the command runs before the background scan finishes.
    cx.simulate_keystrokes("cmd-up");
    cx.simulate_input("üst 1\nüst 2\n");
    cx.simulate_keystrokes("down");
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Merge Conflict: Accept Incoming", window, cx));
    cx.run_until_parked();
    let text = ed.read_with(cx, |e, _| e.text());
    assert!(text.starts_with("üst 1\nüst 2\nonlar\nlet x"), "{}", &text[..60]);
    assert_eq!(ed.read_with(cx, |e, _| e.conflict_count()), 0);
}

/// File without a language server: after two letters, words from the document are suggested.
#[gpui::test]
fn qa_word_based_suggestions_without_a_language_server(cx: &mut TestAppContext) {
    let f = file("words", "notlar.txt", "merhaba dünya\nmerdiven ve masa\n\n".as_bytes());
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    let labels = |cx: &mut VisualTestContext| ed.read_with(cx, |e, _| e.completion_labels());
    cx.simulate_keystrokes("cmd-down");
    cx.simulate_input("m");
    cx.run_until_parked();
    assert_eq!(labels(cx), None, "no list after one letter");
    cx.simulate_input("e");
    cx.run_until_parked();
    let l = labels(cx).expect("opens after two letters");
    assert!(l.contains(&"merhaba".to_string()) && l.contains(&"merdiven".to_string()), "{l:?}");
    assert!(!l.contains(&"masa".to_string()), "items not matching 'me' are filtered out: {l:?}");
    cx.simulate_input("rd");
    cx.run_until_parked();
    assert_eq!(labels(cx).unwrap()[0], "merdiven");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(ed.read_with(cx, |e, _| e.text()).ends_with("\nmerdiven"), "{:?}", ed.read_with(cx, |e, _| e.text()));
    assert_eq!(labels(cx), None);
    // A non-word character closes the list; with the setting off it never opens.
    cx.simulate_input(" ma");
    cx.run_until_parked();
    assert!(labels(cx).is_some());
    cx.simulate_input(" ");
    assert_eq!(labels(cx), None);
    cx.update(|_, cx| cx.global_mut::<crate::settings::Settings>().word_based_suggestions = false);
    cx.simulate_input("me");
    cx.run_until_parked();
    assert_eq!(labels(cx), None, "setting is off");
}

/// Find in selection (⌥⌘L) and selecting all matches with ⌥↵.
#[gpui::test]
fn qa_find_in_selection_and_select_all_matches(cx: &mut TestAppContext) {
    let f = file("insel", "a.txt", "x bir x\nx iki x\nx üç x\n".as_bytes());
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    // Select line 2, ⌘F, search "x": 6 in the file, 2 in the selection.
    cx.simulate_keystrokes("down cmd-l");
    cx.simulate_keystrokes("cmd-f");
    cx.simulate_input("x");
    cx.run_until_parked();
    let status = |cx: &mut VisualTestContext| ed.read_with(cx, |e, cx| e.find_bar().unwrap().read(cx).status_text().map(str::to_string)).unwrap_or_default();
    assert!(status(cx).ends_with("of 6"), "{}", status(cx));
    // While typing the selection moved to the first match; the scope is still the selection from opening.
    cx.simulate_keystrokes("alt-cmd-l");
    cx.run_until_parked();
    assert!(status(cx).ends_with("of 2"), "in selection: {}", status(cx));
    // Inserting text before the scope shifts it (the marks follow the edit).
    ed.update(cx, |e, cx| e.insert_at_for_test(0, "x\n", cx));
    cx.run_until_parked();
    assert!(status(cx).ends_with("of 2"), "after the shift: {}", status(cx));
    // ⌥↵: the two matches in scope are selected, focus in the editor; typing changes both.
    cx.simulate_keystrokes("alt-enter");
    cx.run_until_parked();
    cx.simulate_input("Y");
    assert_eq!(ed.read_with(cx, |e, _| e.text()), "x\nx bir x\nY iki Y\nx üç x\n");
    // Closing the scope gives the whole file. (⌘F puts the selected "Y" in the query, like VS Code; back to "x".)
    cx.simulate_keystrokes("cmd-f cmd-a");
    cx.simulate_input("x");
    cx.run_until_parked();
    assert_eq!(status(cx), "No results", "kapsamdaki x'ler Y oldu");
    cx.simulate_keystrokes("alt-cmd-l");
    cx.run_until_parked();
    assert!(status(cx).ends_with("of 5"), "scope off: {}", status(cx));
}

#[gpui::test]
fn column_select_with_keyboard_extends_a_box(cx: &mut TestAppContext) {
    let f = file("column", "c.txt", b"abcdef\nxy\nabcdef\nabcdef\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("right");
    // ⇧⌥⌘↓ twice + → twice: 3 lines, columns 1..3 (the short line is clipped).
    cx.simulate_keystrokes("shift-alt-cmd-down shift-alt-cmd-down shift-alt-cmd-right shift-alt-cmd-right");
    assert_eq!(ed.read_with(cx, |e, _| e.selected_text_all()), vec!["bc", "y", "bc"]);
    // Typing goes into every line; one undo reverts all of them.
    cx.simulate_input("-");
    assert_eq!(text(&ed, cx), "a-def\nx-\na-def\nabcdef\n");
    cx.simulate_keystrokes("cmd-z");
    assert_eq!(text(&ed, cx), "abcdef\nxy\nabcdef\nabcdef\n");
    // Any other movement drops the box; then ⇧⌥⌘↓ starts a new one.
    cx.simulate_keystrokes("escape down");
    cx.simulate_keystrokes("shift-alt-cmd-down");
    assert_eq!(ed.read_with(cx, |e, _| e.cursor_count()), 2);
}

#[gpui::test]
fn fold_levels_and_recursive_fold(cx: &mut TestAppContext) {
    // Level 1: mod a, mod b; level 2: fn x, fn y (inside a), fn z (inside b).
    let src = "mod a {\n    fn x() {\n        1;\n    }\n    fn y() {\n        2;\n    }\n}\nmod b {\n    fn z() {\n        3;\n    }\n}\n";
    let f = file("fold-levels", "m.rs", src.as_bytes());
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    let headers = |cx: &mut VisualTestContext| {
        ed.read_with(cx, |e, _| {
            let mut h = e.fold_headers();
            h.sort();
            h
        })
    };
    // Cursor on line 0 (inside mod a): at level 1 a is excluded, only b folds.
    cx.simulate_keystrokes("cmd-k cmd-1");
    assert_eq!(headers(cx), vec![8]);
    cx.simulate_keystrokes("cmd-k cmd-j");
    assert!(headers(cx).is_empty(), "unfold all");
    // Level 2: the cursor is inside a so a's fns aren't excluded (cursor line 0 = the mod a header,
    // not in the fn x/y regions); all three fns fold.
    cx.simulate_keystrokes("cmd-k cmd-2");
    assert_eq!(headers(cx), vec![1, 4, 9]);
    cx.simulate_keystrokes("cmd-k cmd-j");
    // Recursive: fold on the mod a header folds a, x, y; b is untouched.
    cx.simulate_keystrokes("cmd-k cmd-[");
    assert_eq!(headers(cx), vec![0, 1, 4]);
    cx.simulate_keystrokes("cmd-k cmd-]");
    assert!(headers(cx).is_empty());
}

#[gpui::test]
fn ten_more_editor_commands(cx: &mut TestAppContext) {
    let f = file("ten", "t.rs", b"fn a(x: u8) {\n    let y = x;\n}\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    // Add / remove line comment (not a toggle).
    cx.simulate_keystrokes("cmd-k cmd-c cmd-k cmd-c");
    assert!(text(&ed, cx).starts_with("// // fn a"), "{}", text(&ed, cx));
    cx.simulate_keystrokes("cmd-k cmd-u cmd-k cmd-u");
    assert!(text(&ed, cx).starts_with("fn a(x"));
    // ⌘K ⌘K: delete from the cursor to the end of the line.
    cx.simulate_keystrokes("right right right right right cmd-k cmd-k");
    assert_eq!(text(&ed, cx).lines().next(), Some("fn a("));
    // Select inside brackets (from the command palette).
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Editor: Select to Bracket", window, cx));
    assert_eq!(ed.read_with(cx, |e, _| e.selected_text()), None, "no pair (unclosed opener): leaves it alone");
    // Indentation conversion.
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Editor: Convert Indentation to Tabs", window, cx));
    assert!(text(&ed, cx).contains("\n\tlet y"), "{}", text(&ed, cx));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Editor: Convert Indentation to Spaces", window, cx));
    assert!(text(&ed, cx).contains("\n    let y"));
    // View toggles flip the setting.
    let was = cx.read(|cx| cx.global::<crate::settings::Settings>().minimap);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("View: Toggle Minimap", window, cx));
    assert_eq!(cx.read(|cx| cx.global::<crate::settings::Settings>().minimap), !was);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("View: Toggle Render Whitespace", window, cx));
    assert_eq!(cx.read(|cx| cx.global::<crate::settings::Settings>().render_whitespace.clone()), "all");
}

#[gpui::test]
fn editorconfig_drives_indent_and_save(cx: &mut TestAppContext) {
    let dir = qa_dir("ec-e2e");
    std::fs::write(dir.join(".editorconfig"), "root = true\n[*.txt]\nindent_style = space\nindent_size = 2\ntrim_trailing_whitespace = true\ninsert_final_newline = true\nend_of_line = crlf\nmax_line_length = 40\n").unwrap();
    let f = dir.join("a.txt");
    std::fs::write(&f, "x   \n").unwrap();
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("tab");
    assert!(text(&ed, cx).starts_with("  x"), "Tab inserts two spaces: {:?}", text(&ed, cx));
    cx.simulate_keystrokes("cmd-down");
    cx.simulate_input("end");
    cx.simulate_keystrokes("cmd-s");
    cx.run_until_parked();
    std::thread::sleep(std::time::Duration::from_millis(300));
    cx.run_until_parked();
    let saved = std::fs::read(&f).unwrap();
    assert_eq!(String::from_utf8(saved).unwrap(), "  x\r\nend\r\n", "trim + final newline + CRLF");
}

#[gpui::test]
fn language_mode_override_survives_and_associates(cx: &mut TestAppContext) {
    let f = file("langmode", "notes.zzz", b"fn main() {}\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    assert_eq!(ed.read_with(cx, |e, _| e.language()), "Plain Text");
    ed.update(cx, |e, cx| e.set_language(Some("rs"), cx));
    assert_eq!(ed.read_with(cx, |e, _| e.language()), "Rust");
    // Reloading after an external change keeps the language.
    std::fs::write(&f, b"fn other() {}\n").unwrap();
    ed.update(cx, |e, cx| e.reload(cx));
    cx.run_until_parked();
    std::thread::sleep(std::time::Duration::from_millis(200));
    cx.run_until_parked();
    assert_eq!(ed.read_with(cx, |e, _| e.language()), "Rust");
    ed.update(cx, |e, cx| e.set_language(None, cx));
    assert_eq!(ed.read_with(cx, |e, _| e.language()), "Plain Text");
    // The palette command opens the picker.
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Editor: Change Language Mode", window, cx));
    cx.run_until_parked();
}

#[gpui::test]
fn linked_tag_editing_mirrors_the_partner(cx: &mut TestAppContext) {
    let f = file("linked", "a.html", b"<div class=\"x\"><span>hi</span></div>\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    // Cursor at the end of the "div" name (opening tag): what is typed also goes to the closing tag.
    cx.simulate_keystrokes("right right right right");
    cx.simulate_input("x");
    assert_eq!(text(&ed, cx), "<divx class=\"x\"><span>hi</span></divx>\n");
    cx.simulate_keystrokes("backspace backspace");
    assert_eq!(text(&ed, cx), "<di class=\"x\"><span>hi</span></di>\n");
    // Typing outside the name doesn't affect its partner.
    cx.simulate_keystrokes("end");
    cx.simulate_input("z");
    assert_eq!(text(&ed, cx), "<di class=\"x\"><span>hi</span></di>z\n");
    // No linking in a .rs file.
    let g = file("linked-rs", "a.rs", b"<div></div>\n");
    let ed2 = open(&wb, &g, cx);
    cx.simulate_keystrokes("right right right right");
    cx.simulate_input("q");
    assert_eq!(text(&ed2, cx), "<divq></div>\n");
}

#[gpui::test]
fn emmet_expands_on_tab_in_html_and_css(cx: &mut TestAppContext) {
    let f = file("emmet", "e.html", b"<body>\n\n</body>\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("down");
    cx.simulate_input("ul>li.item*2");
    cx.simulate_keystrokes("tab");
    assert_eq!(text(&ed, cx), "<body>\n<ul>\n    <li class=\"item\"></li>\n    <li class=\"item\"></li>\n</ul>\n</body>\n");
    // Cursor inside the first empty li: typing goes there.
    cx.simulate_input("x");
    assert!(text(&ed, cx).contains("<li class=\"item\">x</li>\n    <li class=\"item\"></li>"), "{}", text(&ed, cx));
    // Plain word + Tab: not expanded (indent).
    let g = file("emmet-css", "e.css", b"a {\n  \n}\n");
    let ed2 = open(&wb, &g, cx);
    cx.simulate_keystrokes("down end");
    cx.simulate_input("m10-20");
    cx.simulate_keystrokes("tab");
    assert_eq!(text(&ed2, cx), "a {\n  margin: 10px 20px;\n}\n");
    let h = file("emmet-txt", "e.txt", b"");
    let ed3 = open(&wb, &h, cx);
    cx.simulate_input("div");
    cx.simulate_keystrokes("tab");
    assert!(!text(&ed3, cx).contains('<'), "only in markup files");
}

#[gpui::test]
fn linked_tag_edge_cases(cx: &mut TestAppContext) {
    // Typing at the start of the name is mirrored too (the mark sits on the "<" before the name).
    let f = file("linked-edge", "a.html", b"<div></div>\n");
    let (wb, cx) = setup(cx);
    let ed = open(&wb, &f, cx);
    cx.simulate_keystrokes("right");
    cx.simulate_input("x");
    assert_eq!(text(&ed, cx), "<xdiv></xdiv>\n");
    // A delete that swallows the partner doesn't duplicate the name.
    let g = file("linked-edge2", "b.html", b"<div></div>\n");
    let ed2 = open(&wb, &g, cx);
    cx.simulate_keystrokes("right right right right cmd-k cmd-k");
    assert_eq!(text(&ed2, cx), "<div\n");
    // Undo reverts both together; redo reapplies both together.
    let h = file("linked-edge3", "c.html", b"<p></p>\n");
    let ed3 = open(&wb, &h, cx);
    cx.simulate_keystrokes("right right");
    cx.simulate_input("q");
    assert_eq!(text(&ed3, cx), "<pq></pq>\n");
    cx.simulate_keystrokes("cmd-z");
    assert_eq!(text(&ed3, cx), "<p></p>\n");
    cx.simulate_keystrokes("cmd-shift-z");
    assert_eq!(text(&ed3, cx), "<pq></pq>\n");
}

#[gpui::test]
fn emmet_does_not_rewrite_ordinary_code(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx);
    // TSX: member access and console.log stay as they are (Tab writes indent).
    let f = file("emmet-tsx", "a.tsx", b"");
    let ed = open(&wb, &f, cx);
    cx.simulate_input("this.persist.state");
    cx.simulate_keystrokes("tab");
    assert!(text(&ed, cx).starts_with("this.persist.state"), "{:?}", text(&ed, cx));
    assert!(!text(&ed, cx).contains('<'));
    // A JSX abbreviation starting with a known tag expands.
    let g = file("emmet-tsx2", "b.tsx", b"");
    let ed2 = open(&wb, &g, cx);
    cx.simulate_input("div.box");
    cx.simulate_keystrokes("tab");
    assert!(text(&ed2, cx).starts_with("<div className=\"box\">"), "{:?}", text(&ed2, cx));
    // No expansion inside an HTML attribute value.
    let h = file("emmet-attr", "c.html", b"");
    let ed3 = open(&wb, &h, cx);
    cx.simulate_input("<a href=\"foo.bar");
    cx.simulate_keystrokes("tab");
    assert!(!text(&ed3, cx).contains("<foo"), "{:?}", text(&ed3, cx));
    // A bogus abbreviation is silently rejected, the text doesn't change.
    let i = file("emmet-big", "d.html", b"");
    let ed4 = open(&wb, &i, cx);
    cx.simulate_input("div*200>div*200>div*200");
    cx.simulate_keystrokes("tab");
    assert!(text(&ed4, cx).len() < 1000, "huge abbreviation was not expanded");
}
