//! Unit tests for `buffer`.

use super::*;

fn select_all_text(b: &mut Buffer) {
    let n = b.len_chars();
    b.set_selection(0..n, false);
}

#[test]
fn reverse_and_dedupe_lines() {
    let mut b = Buffer::from_str("a\nb\nc\n");
    b.reverse_lines();
    assert_eq!(b.rope().to_string(), "a\nb\nc\n", "no-op without a selection");
    b.set_selection(0..5, false); // a\nb\nc
    b.reverse_lines();
    assert_eq!(b.rope().to_string(), "c\nb\na\n");

    let mut b = Buffer::from_str("x\ny\nx\nz\ny\n");
    b.delete_duplicate_lines();
    assert_eq!(b.rope().to_string(), "x\ny\nz\n", "whole file without a selection, first one stays");
    let mut b = Buffer::from_str("x\nx\nx\nx\n");
    b.set_selection(0..4, false); // first two lines only
    b.delete_duplicate_lines();
    assert_eq!(b.rope().to_string(), "x\nx\nx\n");
}

#[test]
fn duplicate_selection_and_line() {
    let mut b = Buffer::from_str("abc def\n");
    b.set_selection(4..7, false);
    b.duplicate_selection();
    assert_eq!(b.rope().to_string(), "abc defdef\n");
    assert_eq!(b.selected_text().as_deref(), Some("def"), "copy stays selected");
    let mut b = Buffer::from_str("one\ntwo\n");
    b.set_cursor(1, false);
    b.duplicate_selection();
    assert_eq!(b.rope().to_string(), "one\none\ntwo\n", "no selection: copy the line");
    select_all_text(&mut b);
}

#[test]
fn transpose_swaps_around_the_cursor() {
    let mut b = Buffer::from_str("abcd\n");
    b.set_cursor(2, false);
    b.transpose_chars();
    assert_eq!(b.rope().to_string(), "acbd\n");
    assert_eq!(b.cursor(), 3);
    // At line end: the previous two characters.
    let mut b = Buffer::from_str("ab\ncd");
    b.set_cursor(2, false);
    b.transpose_chars();
    assert_eq!(b.rope().to_string(), "ba\ncd");
    // At line start: nothing to do.
    let mut b = Buffer::from_str("ab");
    b.set_cursor(0, false);
    b.transpose_chars();
    assert_eq!(b.rope().to_string(), "ab");
}

#[test]
fn crlf_and_bom_roundtrip() {
    let src = "\u{feff}a\r\nb\r\n";
    let mut b = Buffer::from_str(src);
    assert_eq!(b.line_ending(), LineEnding::Crlf);
    assert_eq!(b.line(0), "a");
    assert_eq!(b.to_bytes(), src.as_bytes());
    b.set_cursor(1, false);
    b.newline();
    b.insert("x");
    assert_eq!(String::from_utf8(b.to_bytes()).unwrap(), "\u{feff}a\r\nx\r\nb\r\n");
}

#[test]
fn not_utf8_is_rejected() {
    assert!(Buffer::from_bytes(&[0xff, 0xfe, 0x00]).is_err());
}

#[test]
fn graphemes_move_as_one() {
    // ZWJ family emoji (5 chars) and combining "i̇" (i + U+0307).
    let mut b = Buffer::from_str("a👨‍👩‍👧b i̇z");
    b.set_cursor(1, false);
    b.move_right(false);
    assert_eq!(b.cursor(), 6);
    b.move_left(false);
    assert_eq!(b.cursor(), 1);
    b.set_cursor(6, false);
    b.backspace();
    assert_eq!(b.line(0), "ab i̇z");
    b.set_cursor(3, false);
    b.delete();
    assert_eq!(b.line(0), "ab z");
}

#[test]
fn vertical_keeps_preferred_column() {
    let mut b = Buffer::from_str("uzun satir\nk\nbir daha uzun");
    b.set_cursor(8, false);
    b.move_vertical(1, false);
    assert_eq!(b.cursor_line_col(), (1, 1));
    b.move_vertical(1, false);
    assert_eq!(b.cursor_line_col(), (2, 8));
    b.move_vertical(1, false);
    assert_eq!(b.cursor(), b.len_chars(), "down on the last line: end of document");
}

#[test]
fn typing_groups_into_one_undo() {
    let mut b = Buffer::from_str("");
    for c in "merhaba".chars() {
        b.insert(&c.to_string());
    }
    b.insert(" ");
    for c in "dünya".chars() {
        b.insert(&c.to_string());
    }
    assert_eq!(b.line(0), "merhaba dünya");
    b.undo();
    assert_eq!(b.line(0), "merhaba", "word starting with whitespace is a separate step");
    b.undo();
    assert_eq!(b.line(0), "");
    b.redo();
    b.redo();
    assert_eq!(b.line(0), "merhaba dünya");
}

#[test]
fn selection_replace_and_undo_restores_selection() {
    let mut b = Buffer::from_str("foo bar");
    b.set_selection(4..7, false);
    b.insert("baz");
    assert_eq!(b.line(0), "foo baz");
    b.undo();
    assert_eq!(b.line(0), "foo bar");
    assert_eq!(b.selection(), Some(4..7));
    // Letter-by-letter typing also forms one step with the letter that replaced the selection.
    for ch in "kuş".chars() {
        b.insert(&ch.to_string());
    }
    assert_eq!(b.line(0), "foo kuş");
    b.undo();
    assert_eq!(b.line(0), "foo bar");
    assert_eq!(b.selection(), Some(4..7));
}

#[test]
fn newline_keeps_indent_and_home_toggles() {
    let mut b = Buffer::from_str("    let x = 1;");
    b.move_end(false);
    b.newline();
    assert_eq!(b.line(1), "    ");
    b.insert("y");
    b.move_home(false);
    assert_eq!(b.cursor_line_col(), (1, 4), "ilk Home: girintinin sonu");
    b.move_home(false);
    assert_eq!(b.cursor_line_col(), (1, 0), "second Home: line start");
}

#[test]
fn byte_edits_only_when_tracked() {
    let mut b = Buffer::from_str("ağ\nb");
    b.insert("x");
    assert!(b.take_edits().is_empty(), "nothing accumulates while tracking is off");
    b.track_edits();
    b.set_cursor(3, false); // "xağ|"
    b.insert("ç");
    let e = b.take_edits();
    assert_eq!(e.len(), 1);
    // "xağ": x(1) a(1) ğ(2) = byte 4
    assert_eq!((e[0].start_byte, e[0].old_end_byte, e[0].new_end_byte), (4, 4, 6));
    assert_eq!((e[0].start_point, e[0].new_end_point), ((0, 4), (0, 6)));
    b.undo();
    let e = b.take_edits();
    assert_eq!((e[0].start_byte, e[0].old_end_byte, e[0].new_end_byte), (4, 6, 4));
}

#[test]
fn brackets_match_both_directions_and_nesting() {
    let b = Buffer::from_str("f(a[1], {x: (2)})");
    assert_eq!(b.matching_bracket(2), Some((1, 16)), "cursor after '('");
    assert_eq!(b.matching_bracket(17), Some((16, 1)), "cursor after the last ')'");
    assert_eq!(b.matching_bracket(16), Some((15, 8)), "between two brackets: the left one wins");
    assert_eq!(b.matching_bracket(4), Some((3, 5)));
    assert_eq!(b.matching_bracket(10), None, "no bracket");
    assert_eq!(Buffer::from_str("((").matching_bracket(1), None, "unpaired");
}

#[test]
fn word_at_cursor() {
    let b = Buffer::from_str("let şeker_2 = 1;");
    assert_eq!(b.word_at(6), Some(4..11));
    assert_eq!(b.word_at(11), Some(4..11), "right next to the word");
    assert_eq!(b.word_at(12), None);
}

#[test]
fn save_transforms_keep_cursor() {
    let mut b = Buffer::from_str("a  \nbc\t\nd");
    b.set_cursor(6, false); // "bc|"
    b.trim_trailing_whitespace();
    b.ensure_final_newline();
    assert_eq!(String::from_utf8(b.to_bytes()).unwrap(), "a\nbc\nd\n");
    assert_eq!(b.cursor_line_col(), (1, 2), "cursor must stay in place");
    b.undo();
    assert_eq!(b.line(2), "d");
}

#[test]
fn utf16_conversion() {
    let b = Buffer::from_str("a😀b");
    assert_eq!(b.char_to_utf16(2), 3);
    assert_eq!(b.utf16_to_char(3), 2);
}

#[test]
fn multi_cursor_typing_is_one_undo_step() {
    let mut b = Buffer::from_str("a\nb\nc");
    b.add_cursor_vertical(1);
    b.add_cursor_vertical(1);
    assert_eq!(b.cursors(), [0, 2, 4]);
    for ch in "şey".chars() {
        b.insert(&ch.to_string());
    }
    assert_eq!(b.rope().to_string(), "şeya\nşeyb\nşeyc");
    assert_eq!(b.cursors(), [3, 8, 13]);
    b.backspace();
    assert_eq!(b.rope().to_string(), "şea\nşeb\nşec");
    b.undo();
    assert_eq!(b.rope().to_string(), "şeya\nşeyb\nşeyc");
    b.undo();
    assert_eq!(b.rope().to_string(), "a\nb\nc", "typing at three cursors is one step");
    assert_eq!(b.cursors(), [0, 2, 4], "undo restores the cursors");
    b.redo();
    assert_eq!(b.rope().to_string(), "şeya\nşeyb\nşeyc");
    assert_eq!(b.cursors(), [3, 8, 13]);
}

#[test]
fn next_occurrence_selects_word_then_matches_and_wraps() {
    let mut b = Buffer::from_str("foo bar foo\nfoo");
    b.set_cursor(9, false);
    assert!(b.select_next_occurrence(), "the word at the cursor first");
    assert_eq!(b.selections(), vec![8..11]);
    assert!(b.select_next_occurrence());
    assert_eq!(b.selections(), [8..11, 12..15]);
    assert!(b.select_next_occurrence(), "wraps around from the end");
    assert_eq!(b.selections(), [0..3, 8..11, 12..15]);
    assert!(!b.select_next_occurrence(), "all selected");
    b.insert("x");
    assert_eq!(b.rope().to_string(), "x bar x\nx");
    assert_eq!(b.cursor(), 1, "birincil: son eklenen");
    assert!(b.collapse_cursors());
    assert_eq!(b.cursors(), [7], "Esc stays at the original (first) cursor, like VS Code");
}

#[test]
fn cursors_merge_when_they_meet() {
    let mut b = Buffer::from_str("ab\ncd");
    b.set_cursor(1, false);
    b.toggle_cursor(2);
    assert_eq!(b.cursor_count(), 2);
    b.move_right(false);
    assert_eq!(b.cursors(), [2, 3]);
    b.move_left(false);
    b.move_left(false);
    b.move_left(false);
    assert_eq!(b.cursors(), [0], "those meeting at the start become a single cursor");
    b.toggle_cursor(4);
    b.toggle_cursor(4);
    assert_eq!(b.cursors(), [0], "alt+click at the same spot: remove");
}

#[test]
fn multi_paste_spreads_lines_and_newline_keeps_each_indent() {
    let mut b = Buffer::from_str("  a\n    b");
    b.set_cursor(3, false);
    b.toggle_cursor(9);
    b.insert_each(&["1".into(), "2".into()]);
    assert_eq!(b.rope().to_string(), "  a1\n    b2");
    b.newline();
    assert_eq!(b.rope().to_string(), "  a1\n  \n    b2\n    ");
    assert_eq!(b.selected_texts(), ["", ""]);
    b.select_all();
    assert_eq!(b.cursor_count(), 1);
}

#[test]
fn delete_and_save_transforms_with_many_cursors() {
    let mut b = Buffer::from_str("x  \ny  \nz");
    b.set_cursor(1, false);
    b.toggle_cursor(5);
    b.trim_trailing_whitespace();
    assert_eq!(b.rope().to_string(), "x\ny\nz");
    assert_eq!(b.cursors(), [1, 3], "cursors stay on their lines");
    b.move_left(false);
    b.delete();
    assert_eq!(b.rope().to_string(), "\n\nz");
}

#[test]
fn word_moves_and_word_delete() {
    let mut b = Buffer::from_str("let şeker_2 = foo(bar);");
    b.move_word_right(false);
    assert_eq!(b.cursor(), 3);
    b.move_word_right(false);
    assert_eq!(b.cursor(), 11, "Turkish letters and _ are part of the word");
    b.move_word_right(false);
    assert_eq!(b.cursor(), 13, "noktalama kendi grubu");
    b.set_cursor(b.len_chars(), false);
    b.move_word_left(false);
    assert_eq!(b.cursor(), 21, "\");\" birlikte");
    b.move_word_left(true);
    assert_eq!(b.selected_text().as_deref(), Some("bar"));
    b.set_cursor(11, false);
    b.delete_word(false);
    assert_eq!(b.line(0), "let  = foo(bar);");
    b.delete_word(true);
    assert_eq!(b.line(0), "let  foo(bar);", "whitespace + next group (like VS Code)");
    b.select_line_at(2);
    assert_eq!(b.selection(), Some(0..14));
}

#[test]
fn undo_back_to_saved_state_is_clean() {
    let mut b = Buffer::from_str("a");
    let saved = b.version();
    b.insert("x");
    b.insert("y");
    assert_ne!(b.version(), saved);
    let typed = b.version();
    b.undo();
    assert_eq!(b.version(), saved, "returned to the saved content");
    b.redo();
    assert_eq!(b.version(), typed);
    b.undo();
    b.insert("z");
    assert_ne!(b.version(), typed, "different content, different identity");
}

#[test]
fn lsp_changes_use_utf16_positions() {
    let mut b = Buffer::from_str("a😀b\nçok");
    b.track_lsp();
    b.set_cursor(3, false); // "a😀b|"
    b.insert("x");
    b.set_cursor(4, false); // end of the first line
    b.delete();
    let ch = b.take_lsp_changes();
    assert_eq!(ch[0], TextChange { start: (0, 4), end: (0, 4), text: "x".into() }, "😀 iki UTF-16 birimi");
    assert_eq!(ch[1], TextChange { start: (0, 5), end: (1, 0), text: String::new() }, "line ending deleted");
    assert_eq!(b.line(0), "a😀bxçok");
    assert_eq!(b.from_lsp_position(0, 3), 2, "after 😀");
    assert_eq!(b.from_lsp_position(0, 99), b.line_len_chars(0), "overflowing column is clamped");
    assert_eq!(b.lsp_position(4), (0, 5));
}

#[test]
fn toggle_comment_on_lines() {
    let mut b = Buffer::from_str("fn a() {\n    let x = 1;\n\n    x\n}\n");
    b.set_selection(9..30, false); // from line 2 to line 4
    b.toggle_comment("//");
    assert_eq!(b.rope().to_string(), "fn a() {\n    // let x = 1;\n\n    // x\n}\n", "leaves empty lines alone, at the least indentation");
    b.toggle_comment("//");
    assert_eq!(b.rope().to_string(), "fn a() {\n    let x = 1;\n\n    x\n}\n", "removed if all are commented");
    b.undo();
    b.undo();
    assert_eq!(b.line(1), "    let x = 1;");
    // Cursor on a single line: that line; Python '#'.
    let mut b = Buffer::from_str("x = 1\n");
    b.set_cursor(3, false);
    b.toggle_comment("#");
    assert_eq!(b.line(0), "# x = 1");
    assert_eq!(b.cursor(), 5, "cursor shifts with the text");
}

#[test]
fn indent_outdent_and_line_moves() {
    let mut b = Buffer::from_str("a\nb\nc\n");
    b.set_selection(0..3, false); // a and b
    b.indent_lines("    ");
    assert_eq!(b.rope().to_string(), "    a\n    b\nc\n");
    b.outdent_lines(4);
    assert_eq!(b.rope().to_string(), "a\nb\nc\n");
    // ⌥↓: a and b move down together, selection with them.
    b.move_lines(false);
    assert_eq!(b.rope().to_string(), "c\na\nb\n");
    assert_eq!(b.selected_text().as_deref(), Some("a\nb"));
    b.move_lines(true);
    assert_eq!(b.rope().to_string(), "a\nb\nc\n");
    b.move_lines(true);
    assert_eq!(b.rope().to_string(), "a\nb\nc\n", "up on the first line: unchanged");
    // ⇧⌥↓: duplicate, cursor on the lower copy.
    let mut b = Buffer::from_str("satır\nson");
    b.set_cursor(2, false);
    b.copy_lines(true);
    assert_eq!(b.rope().to_string(), "satır\nsatır\nson");
    assert_eq!(b.cursor_line_col(), (1, 2));
    b.copy_lines(false);
    assert_eq!(b.rope().to_string(), "satır\nsatır\nsatır\nson");
    assert_eq!(b.cursor_line_col(), (1, 2), "↑: stays on the new upper copy");
    // Move the last line down (no line ending) and move onto the last line.
    let mut b = Buffer::from_str("bir\niki");
    b.move_lines(false);
    assert_eq!(b.rope().to_string(), "iki\nbir");
    assert_eq!(b.cursor_line_col(), (1, 0));
}

#[test]
fn auto_closing_pairs() {
    let mut b = Buffer::from_str("");
    b.type_char('(', true);
    assert_eq!((b.rope().to_string().as_str(), b.cursor()), ("()", 1));
    b.type_char('x', true);
    b.type_char(')', true);
    assert_eq!((b.rope().to_string().as_str(), b.cursor()), ("(x)", 3), "steps over the closer");
    b.set_cursor(1, false);
    b.backspace_pair();
    assert_eq!(b.rope().to_string(), "x)");
    // Backspace in an empty pair: both at once.
    let mut b = Buffer::from_str("");
    b.type_char('[', true);
    b.backspace_pair();
    assert_eq!(b.rope().to_string(), "");
    // Wrap the selection; no quote pair inside a word (don't); no bracket pair before a word.
    let mut b = Buffer::from_str("adı");
    b.select_all();
    b.type_char('"', true);
    assert_eq!(b.rope().to_string(), "\"adı\"");
    assert_eq!(b.selected_text().as_deref(), Some("adı"));
    let mut b = Buffer::from_str("don");
    b.set_cursor(3, false);
    b.type_char('\'', true);
    assert_eq!(b.rope().to_string(), "don'");
    let mut b = Buffer::from_str("x");
    b.type_char('(', true);
    assert_eq!(b.rope().to_string(), "(x", "sonraki karakter kelimeyse tek");
    // Multiple cursors: a pair at each.
    let mut b = Buffer::from_str("a\nb");
    b.set_cursor(1, false);
    b.toggle_cursor(3);
    b.type_char('{', true);
    assert_eq!(b.rope().to_string(), "a{}\nb{}");
    assert_eq!(b.cursors(), [2, 6]);
    // Disabled: plain typing.
    let mut b = Buffer::from_str("");
    b.type_char('(', false);
    assert_eq!(b.rope().to_string(), "(");
}

#[test]
fn smart_newline_indents_after_openers() {
    let mut b = Buffer::from_str("fn a() {}");
    b.set_cursor(8, false);
    b.smart_newline("    ", false);
    assert_eq!(b.rope().to_string(), "fn a() {\n    \n}");
    assert_eq!(b.cursor_line_col(), (1, 4), "cursor on the middle line");
    let mut b = Buffer::from_str("if x:");
    b.move_end(false);
    b.smart_newline("    ", true);
    assert_eq!(b.rope().to_string(), "if x:\n    ");
    let mut b = Buffer::from_str("  düz");
    b.move_end(false);
    b.smart_newline("    ", false);
    assert_eq!(b.rope().to_string(), "  düz\n  ", "keeps the indentation");
    b.undo();
    assert_eq!(b.rope().to_string(), "  düz");
}

#[test]
fn parked_selections_follow_edits_from_the_other_view() {
    let mut b = Buffer::from_str("alfa\nbeta\ngama\n");
    b.set_cursor(7, false); // "be|ta"
    b.switch_selections(0, 1); // new view starts at the same place
    assert_eq!(b.cursor(), 7);
    b.set_cursor(0, false);
    b.insert("XY"); // view 1 types at the start
    b.set_selection(12..16, false); // "gama"
    b.switch_selections(1, 0);
    assert_eq!(b.cursor(), 9, "view 0's cursor shifted");
    b.set_cursor(11, false);
    b.backspace(); // view 0 deletes before view 1's selection
    b.switch_selections(0, 1);
    assert_eq!(b.selection(), Some(11..15), "selection shifted too");
    b.undo(); // undo view 0's edit: the parked one shifts back too
    b.switch_selections(1, 0);
    assert_eq!(b.text_range(0..b.len_chars()), "XYalfa\nbeta\ngama\n");
    b.forget_selections(1);
    assert!(b.parked.iter().all(|(s, ..)| *s != 1));
}

#[test]
fn line_and_case_commands() {
    let mut b = Buffer::from_str("ali veli ali\n");
    b.set_cursor(1, false);
    b.select_all_occurrences();
    assert_eq!(b.selections(), vec![0..3, 9..12]);
    b.transform_selections(|t| t.to_uppercase());
    assert_eq!(b.text_range(0..b.len_chars()), "ALI veli ALI\n");
    assert_eq!(b.selections(), vec![0..3, 9..12], "selection preserved");

    let mut b = Buffer::from_str("fn a() {\n    x;   \n    y;\n}\n");
    b.set_cursor(10, false); // line 2
    b.join_lines();
    assert_eq!(b.text_range(0..b.len_chars()), "fn a() {\n    x; y;\n}\n");
    b.undo();
    b.set_selection(0..b.line_to_char(3), false);
    b.join_lines();
    assert_eq!(b.text_range(0..b.len_chars()), "fn a() { x; y;\n}\n");

    let mut b = Buffer::from_str("c\na\nb\n");
    b.set_selection(0..5, false);
    b.sort_lines(false);
    assert_eq!(b.text_range(0..b.len_chars()), "a\nb\nc\n");
    b.sort_lines(true);
    assert_eq!(b.text_range(0..b.len_chars()), "c\nb\na\n");
    assert_eq!(b.selections(), vec![0..5]);
}

#[test]
fn block_comments_round_trip() {
    let mut b = Buffer::from_str("  a { x }\n\n  b\n");
    b.select_all();
    b.toggle_block_comment_lines("/*", "*/");
    assert_eq!(b.text_range(0..b.len_chars()), "  /* a { x } */\n\n  /* b */\n");
    b.select_all();
    b.toggle_block_comment_lines("/*", "*/");
    assert_eq!(b.text_range(0..b.len_chars()), "  a { x }\n\n  b\n");
}

#[test]
fn next_occurrence_stays_fast_with_tens_of_thousands_of_selections() {
    let text = "ab ".repeat(30_000);
    let mut b = Buffer::from_str(&text);
    b.set_cursor(0, false);
    assert!(b.select_next_occurrence()); // selects the word `ab`
    b.select_all_occurrences();
    assert_eq!(b.selected_texts().len(), 30_000);
    let started = std::time::Instant::now();
    // Everything is selected: nothing is free, and finding that out must not take quadratic time.
    assert!(!b.select_next_occurrence());
    assert!(started.elapsed().as_secs() < 5, "{:?}", started.elapsed());
    // Correctness of the overlap test on a small case: cursors inside/at the edge of a match.
    let mut c = Buffer::from_str("foo foo foo");
    c.set_cursor(0, false);
    c.select_next_occurrence();
    c.select_next_occurrence();
    assert_eq!(c.selected_texts(), vec!["foo", "foo"]);
    c.select_next_occurrence();
    assert_eq!(c.selected_texts().len(), 3);
    assert!(!c.select_next_occurrence(), "all three are taken");
}

#[test]
fn vertical_movement_keeps_the_visual_column_across_tabs() {
    // Line 0 is indented with one tab (4 columns), line 1 with four spaces, line 2 with two tabs (8 columns).
    let mut b = Buffer::from_str("\tabc\n    xyz\n\t\tq\n");
    b.set_cursor(2, false); // after `a`: visual column 5
    b.move_vertical_visual(1, false, 4);
    assert_eq!(b.cursor_line_col(), (1, 5), "same screen column, one char further in the space-indented line");
    b.move_vertical_visual(1, false, 4);
    assert_eq!(b.cursor_line_col(), (2, 1), "column 5 is inside the second tab: the cursor snaps to its nearer edge (never inside a tab)");
    b.move_vertical_visual(-1, false, 4);
    assert_eq!(b.cursor_line_col(), (1, 5), "the remembered goal column (5) comes back");
    // The character-column variant still behaves as before.
    let mut c = Buffer::from_str("\tabc\n    xyz\n");
    c.set_cursor(2, false);
    c.move_vertical(1, false);
    assert_eq!(c.cursor_line_col(), (1, 2));
}

#[test]
fn mixed_line_endings_are_detected_when_the_file_is_opened() {
    for (text, mixed) in [
        ("a\nb\n", false),
        ("a\r\nb\r\n", false),
        ("a\rb\r", false),
        ("a\nb\r\nc", true),
        ("a\r\nb\nc\r\n", true),
        ("a\nb\rc", true),
        ("no newline at all", false),
        ("", false),
    ] {
        assert_eq!(Buffer::from_str(text).has_mixed_line_endings(), mixed, "{text:?}");
    }
    // Normalisation is unchanged: the first line decides.
    assert_eq!(Buffer::from_str("a\nb\r\nc").to_bytes(), b"a\nb\nc");
}

#[test]
fn no_op_transforms_leave_no_undo_step_and_combining_marks_stay_in_words() {
    let mut b = Buffer::from_str("b\na\nc\n");
    b.select_all();
    b.sort_lines(false);
    assert!(b.undo(), "an actual sort is one undo step");
    assert!(!b.undo(), "and nothing else");
    let mut sorted = Buffer::from_str("a\nb\nc\n");
    sorted.select_all();
    sorted.sort_lines(false);
    assert!(!sorted.undo(), "already sorted: no history entry, buffer stays clean");
    let mut upper = Buffer::from_str("ABC\n");
    upper.select_all();
    upper.transform_selections(|s| s.to_uppercase());
    assert!(!upper.undo(), "uppercasing uppercase text changes nothing");
    // NFD "café" (e + U+0301) is one word for word movement and selection.
    let b = Buffer::from_str("x cafe\u{301} y");
    let w = b.word_at(5).expect("a word");
    assert_eq!(b.text_range(w), "cafe\u{301}");
}

#[test]
fn undo_history_is_bounded_and_the_newest_steps_survive() {
    let mut b = Buffer::from_str("");
    // Many small steps: only the newest MAX_UNDO_STEPS remain undoable, the text is untouched by the trimming.
    for i in 0..(MAX_UNDO_STEPS + 50) {
        b.replace(b.len_chars()..b.len_chars(), &format!("{}\n", i % 10), false);
    }
    assert_eq!(b.len_lines(), MAX_UNDO_STEPS + 51);
    let mut steps = 0;
    while b.undo() {
        steps += 1;
    }
    // Trimming happens in batches: at least MAX_UNDO_STEPS remain, never more than the limit plus the slack.
    assert!((MAX_UNDO_STEPS..=MAX_UNDO_STEPS + UNDO_SLACK).contains(&steps), "{steps}");
    assert_eq!(b.len_lines(), MAX_UNDO_STEPS + 51 - steps, "undoing everything undoable leaves the dropped steps' lines");
    // Large steps: the byte cap keeps memory bounded but always keeps at least the latest step.
    let mut big = Buffer::from_str(&"x".repeat(1024 * 1024));
    for _ in 0..200 {
        let n = big.len_chars();
        big.replace(0..n, &"y".repeat(1024 * 1024), false);
    }
    let mut kept = 0;
    while big.undo() {
        kept += 1;
    }
    assert!((1..=100).contains(&kept), "{kept} steps of ~2 MB each stay near the 64 MB cap");
}

#[test]
fn moving_lines_never_swaps_with_the_phantom_last_line() {
    // "a\nb\n": ↓ on b used to give "a\n\nb" (blank line, final newline lost).
    let mut b = Buffer::from_str("a\nb\n");
    b.set_cursor(2, false); // on `b`
    b.move_lines(false);
    assert_eq!(b.text_range(0..b.len_chars()), "a\nb\n", "the last real line cannot move down");
    // ↑ with the cursor on the empty line after the final newline does nothing either.
    b.set_cursor(4, false);
    b.move_lines(true);
    assert_eq!(b.text_range(0..b.len_chars()), "a\nb\n");
    // Without a final newline the last line is real and moves normally.
    let mut c = Buffer::from_str("a\nb");
    c.set_cursor(0, false);
    c.move_lines(false);
    assert_eq!(c.text_range(0..c.len_chars()), "b\na");
    // Middle lines of a file with a final newline still move both ways.
    let mut d = Buffer::from_str("a\nb\nc\n");
    d.set_cursor(2, false);
    d.move_lines(false);
    assert_eq!(d.text_range(0..d.len_chars()), "a\nc\nb\n");
    d.move_lines(true);
    assert_eq!(d.text_range(0..d.len_chars()), "a\nb\nc\n");
}

#[test]
fn trimming_is_one_undo_step_and_the_final_newline_keeps_the_cursor() {
    let text: String = (0..500).map(|i| format!("line {i}   \n")).collect();
    let mut b = Buffer::from_str(&text);
    b.set_cursor(3, false);
    b.trim_trailing_whitespace();
    assert!(!b.text_range(0..b.len_chars()).contains("  \n"));
    assert_eq!(b.cursor_line_col(), (0, 3), "the cursor stays where it was");
    assert!(b.undo(), "one step");
    assert_eq!(b.text_range(0..b.len_chars()), text, "a single undo restores all 500 lines");
    // Final newline: a cursor at the very end stays on its line.
    let mut c = Buffer::from_str("abc");
    c.set_cursor(3, false);
    c.ensure_final_newline();
    assert_eq!(c.text_range(0..c.len_chars()), "abc\n");
    assert_eq!(c.cursor_line_col(), (0, 3), "saving does not jump to a new empty line");
    // Nothing to do: no undo step is recorded.
    let mut d = Buffer::from_str("abc\n");
    d.trim_trailing_whitespace();
    d.ensure_final_newline();
    assert!(!d.undo());
}

#[test]
fn block_comment_toggle_survives_overlapping_delimiters() {
    // `/*/` starts with the opener and ends with the closer, but is not a wrapped line.
    for (text, open, close) in [("/*/\n", "/*", "*/"), ("<!-->\n", "<!--", "-->"), ("*/\n", "/*", "*/")] {
        let mut b = Buffer::from_str(text);
        b.select_all();
        b.toggle_block_comment_lines(open, close);
        let once = b.text_range(0..b.len_chars());
        assert!(once.contains(open) && once.contains(close), "{text:?} -> {once:?}");
    }
    let mut b = Buffer::from_str("/**/\n");
    b.select_all();
    b.toggle_block_comment_lines("/*", "*/");
    assert_eq!(b.text_range(0..b.len_chars()), "\n", "an empty block comment unwraps to nothing");
}

#[test]
fn column_selection_is_a_box_clamped_to_short_lines() {
    let mut b = Buffer::from_str("abcdef\nxy\nabcdef\n");
    b.select_columns((0, 1), (2, 4), 4);
    assert_eq!(b.selected_texts(), vec!["bcd", "y", "bcd"]);
    assert_eq!(b.cursor_count(), 3);
    // Same column: an empty cursor on every line; typing goes into all, one undo.
    b.select_columns((0, 2), (2, 2), 4);
    assert!(b.selected_texts().iter().all(String::is_empty));
    b.insert("-");
    assert_eq!(b.rope().to_string(), "ab-cdef\nxy-\nab-cdef\n");
    b.undo();
    assert_eq!(b.rope().to_string(), "abcdef\nxy\nabcdef\n");
    // Leftward (edge left of the anchor) and upward give the same box.
    b.select_columns((2, 4), (0, 1), 4);
    assert_eq!(b.selected_texts(), vec!["bcd", "y", "bcd"]);
    // Delete the text in the column.
    b.select_columns((0, 1), (2, 3), 4);
    b.backspace();
    assert_eq!(b.rope().to_string(), "adef\nx\nadef\n");
}

#[test]
fn delete_to_line_end_and_join_at_end() {
    let mut b = Buffer::from_str("abc def\nxyz\n");
    b.set_cursor(3, false);
    b.delete_to_line_end();
    assert_eq!(b.rope().to_string(), "abc\nxyz\n");
    b.delete_to_line_end(); // at line end: joins with the next line
    assert_eq!(b.rope().to_string(), "abcxyz\n");
}

#[test]
fn add_and_remove_line_comment_do_not_toggle() {
    let mut b = Buffer::from_str("a\n// b\n");
    select_all_text(&mut b);
    b.comment_lines("//", Some(true));
    assert_eq!(b.rope().to_string(), "// a\n// // b\n", "also added to commented lines");
    select_all_text(&mut b);
    b.comment_lines("//", Some(false));
    assert_eq!(b.rope().to_string(), "a\n// b\n", "only one layer is removed");
    select_all_text(&mut b);
    b.comment_lines("//", Some(false));
    select_all_text(&mut b);
    b.comment_lines("//", Some(false));
    assert_eq!(b.rope().to_string(), "a\nb\n", "leaves uncommented lines alone");
}

#[test]
fn select_to_bracket_inside_and_enclosing() {
    let mut b = Buffer::from_str("f(a, [b, c], d)");
    b.set_cursor(3, false); // after "a": enclosing ( )
    assert!(b.select_to_bracket());
    assert_eq!(b.selected_text().as_deref(), Some("a, [b, c], d"));
    b.set_cursor(11, false); // adjacent to "]"
    assert!(b.select_to_bracket());
    assert_eq!(b.selected_text().as_deref(), Some("b, c"));
    let mut none = Buffer::from_str("düz metin");
    none.set_cursor(3, false);
    assert!(!none.select_to_bracket());
}

#[test]
fn select_previous_occurrence_walks_backwards_and_wraps() {
    let mut b = Buffer::from_str("ab x ab y ab");
    b.set_cursor(11, false);
    assert!(b.select_previous_occurrence()); // word: the trailing ab
    assert!(b.select_previous_occurrence());
    assert!(b.select_previous_occurrence());
    assert_eq!(b.selected_texts(), vec!["ab", "ab", "ab"]);
    assert!(!b.select_previous_occurrence(), "all selected");
}

#[test]
fn column_selection_uses_visual_columns_with_tabs() {
    let mut b = Buffer::from_str("\tx = 1\n    y = 2\nz\n");
    // Visual columns 4..6: "x " after the tab in the first line, "y " in the second.
    b.select_columns((0, 4), (1, 6), 4);
    assert_eq!(b.selected_texts(), vec!["x ", "y "]);
    // A tab's middle rounds to the nearer edge (column 1 → tab start, 3 → its end).
    assert_eq!(b.char_at_visual_col(0, 1, 4), 0);
    assert_eq!(b.char_at_visual_col(0, 3, 4), 1);
    assert_eq!(b.char_at_visual_col(0, 99, 4), 6);
    assert_eq!(b.visual_col(0, 1, 4), 4);
    assert_eq!(b.visual_col(1, 4, 4), 4);
    // Very wide column: clamped to the line end, no panic.
    b.select_columns((0, 0), (9, 500), 4);
    assert!(b.cursor_count() >= 3);
}

/// Inserted text brings its own line breaks (`\r\n` from an LSP edit, stray `\r` in a file): they are stored as `\n`, and no cursor
/// may be left behind the end of the text because it was placed by the length the caller wrote.
#[test]
fn cursors_stay_inside_the_text_after_edits_with_crlf() {
    let mut b = Buffer::from_str("a\nb");
    b.set_cursor(3, false);
    b.apply_edits(vec![(0..1, "x\r\ny".to_string())]);
    assert_eq!(b.text_range(0..b.len_chars()), "x\ny\nb");
    assert!(b.cursor() <= b.len_chars(), "cursor {} past len {}", b.cursor(), b.len_chars());
    // Everything that looks up the cursor's line works.
    b.move_left(false);
    b.move_vertical(-1, false);
    b.join_lines();
    b.toggle_comment("//");
    b.indent_lines("  ");
    b.newline();
    b.backspace();
}

#[test]
fn line_commands_on_stray_carriage_returns_do_not_leave_cursors_past_the_end() {
    for text in ["b\r\r\na\r\r\nc", "b\rx\na\ry\n"] {
        for op in 0..3 {
            let mut b = Buffer::from_str(text);
            select_all_text(&mut b);
            match op {
                0 => b.reverse_lines(),
                1 => b.sort_lines(false),
                _ => b.join_lines(),
            }
            assert!(b.cursor() <= b.len_chars(), "{text:?} op {op}: cursor {} past len {}", b.cursor(), b.len_chars());
            b.move_vertical(-1, false);
            b.move_left(false);
            b.toggle_comment("//");
        }
    }
}

#[test]
fn mapped_cursors_count_a_crlf_in_the_new_text_once() {
    let mut b = Buffer::from_str("ab\ncd");
    b.set_cursor(4, false);
    b.apply_edits(vec![(0..0, "1\r\n2\r\n".to_string())]);
    // "1\n2\nab\ncd": the cursor kept its place relative to the old text ("c|d").
    assert_eq!(b.text_range(0..b.len_chars()), "1\n2\nab\ncd");
    assert_eq!(b.cursor(), 8);
}

/// Editing a closed file must not rewrite the lines nobody touched, whatever mix of endings they have.
#[test]
fn editing_bytes_keeps_the_endings_of_untouched_lines() {
    let edit = |bytes: &[u8], from: usize, to: usize, new: &str| {
        Buffer::edit_bytes(bytes, |_| vec![(from..to, new.to_string())]).unwrap().map(|b| String::from_utf8(b).unwrap())
    };
    // Mixed: the first line ends with LF, the others with CRLF. `x` -> `y` touches only the first.
    assert_eq!(edit(b"x = 1\nfoo\r\nbar\r\n", 0, 1, "y").unwrap(), "y = 1\nfoo\r\nbar\r\n");
    // An edit after CRLF lines is placed by the buffer's count (a CRLF is one char): `bar` is chars 10..13 there.
    assert_eq!(edit(b"x = 1\nfoo\r\nbar\r\n", 10, 13, "baz").unwrap(), "x = 1\nfoo\r\nbaz\r\n");
    // New text brings `\n`: it takes the file's own ending (CRLF here, the first break decides), the rest stays.
    assert_eq!(edit(b"a\r\nb\nc\r\n", 0, 1, "x\ny").unwrap(), "x\r\ny\r\nb\nc\r\n");
    // BOM and multibyte characters before the edit.
    assert_eq!(edit("\u{feff}ş\r\nğ\nü".as_bytes(), 4, 5, "u").unwrap(), "\u{feff}ş\r\nğ\nu");
    // Not mixed: through the buffer, as before, CRLF intact.
    assert_eq!(edit(b"a\r\nb\r\n", 0, 1, "z").unwrap(), "z\r\nb\r\n");
    // No edits, no bytes.
    assert!(Buffer::edit_bytes(b"a\nb\r\n", |_| Vec::new()).unwrap().is_none());
    assert!(Buffer::edit_bytes(&[0xff, 0xfe], |_| Vec::new()).is_err());
}
