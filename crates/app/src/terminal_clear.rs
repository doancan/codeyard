//! Clearing the screen the way programs that redraw in place expect it.
//!
//! The emulator moves what is on the screen into the history when a program clears it (`CSI 2 J`). A program that clears
//! and redraws a frame each time (a coding assistant, a progress display) would then leave a copy of every old frame
//! and a run of blank lines in the history, and scrolling up shows the leftovers instead of what was printed before.
//! The screen is erased in place instead, as the other terminals of this kind do.

use super::*;

/// The sequence for "erase the whole screen".
const CLEAR_SCREEN: &[u8] = b"\x1b[2J";

/// "Erase the saved lines": the history.
const CLEAR_HISTORY: &[u8] = b"\x1b[3J";

/// `data` without the sequences that erase the history.
fn without_history_clears(data: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    if !data.windows(CLEAR_HISTORY.len()).any(|w| w == CLEAR_HISTORY) {
        return std::borrow::Cow::Borrowed(data);
    }
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i..].starts_with(CLEAR_HISTORY) {
            i += CLEAR_HISTORY.len();
        } else {
            out.push(data[i]);
            i += 1;
        }
    }
    std::borrow::Cow::Owned(out)
}

/// Where the next full-screen clear starts in `data`.
fn find_clear(data: &[u8]) -> Option<usize> {
    data.windows(CLEAR_SCREEN.len()).position(|w| w == CLEAR_SCREEN)
}

/// How many bytes at the end of `data` start an erase sequence (`ESC [`, `ESC [ 2`, `ESC [ 3`) that the next read completes.
/// A lone ESC is not held: an incomplete sequence is held by the parser anyway, so nothing shows later for it.
fn partial_erase_at_end(data: &[u8]) -> usize {
    match data {
        [.., 0x1b, b'[', b'2' | b'3'] => 3,
        [.., 0x1b, b'['] => 2,
        _ => 0,
    }
}

/// Erase the screen in place, with the cursor where it is: home, erase below, back (inside a synchronized update, where the
/// parser applies it in order).
const CLEAR_IN_PLACE: &[u8] = b"\x1b7\x1b[H\x1b[J\x1b8";

impl TerminalView {
    /// Bytes from the program: full-screen clears are carried out here, the rest goes to the parser (`feed_chunk`).
    pub(super) fn feed(&mut self, data: &[u8]) {
        let mut joined;
        let mut data = data;
        if !self.clear_carry.is_empty() {
            joined = std::mem::take(&mut self.clear_carry);
            joined.extend_from_slice(data);
            data = &joined;
        }
        let held = partial_erase_at_end(data);
        if held > 0 {
            self.clear_carry = data[data.len() - held..].to_vec();
            data = &data[..data.len() - held];
        }
        let kept;
        let data = if self.allow_clear_history {
            data
        } else {
            kept = without_history_clears(data);
            &kept
        };
        if find_clear(data).is_none() || self.term.mode().contains(TermMode::ALT_SCREEN) {
            return self.feed_chunk(data);
        }
        let mut rest = data;
        while let Some(at) = find_clear(rest) {
            self.feed_chunk(&rest[..at]);
            // Inside a synchronized update the parser holds what came before back: erasing now would run ahead of it, so
            // the parser gets an erase that leaves the history alone, in order with the rest.
            if self.parser.sync_timeout().sync_timeout().is_some() {
                self.feed_chunk(CLEAR_IN_PLACE);
            } else {
                self.erase_screen();
            }
            rest = &rest[at + CLEAR_SCREEN.len()..];
        }
        self.feed_chunk(rest);
    }

    /// Every visible row becomes blank (in the current background); the cursor, the history and the modes stay. What pointed
    /// at the rows (a selection, the prompt marks) goes with them, as it did when the rows moved into the history.
    fn erase_screen(&mut self) {
        let template = self.term.grid().cursor.template.clone();
        let rows = self.term.screen_lines();
        let first_visible = self.term.grid().history_size() as i64 + self.marks.shift;
        self.marks.list.retain(|m| m.prompt < first_visible);
        self.term.selection = None;
        let grid = self.term.grid_mut();
        for line in 0..rows {
            grid[Line(line as i32)].reset(&template);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[test]
    fn finds_the_clear_sequence_anywhere_in_the_data() {
        assert_eq!(find_clear(b"abc\x1b[2Jdef"), Some(3));
        assert_eq!(find_clear(b"\x1b[2J"), Some(0));
        assert_eq!(find_clear(b"\x1b[J\x1b[3J\x1b[1J"), None, "other erase sequences are not a full clear");
        assert_eq!(find_clear(b""), None);
    }

    #[test]
    fn history_clears_are_cut_out_and_nothing_else() {
        assert_eq!(without_history_clears(b"a\x1b[3Jb").as_ref(), b"ab");
        assert_eq!(without_history_clears(b"\x1b[H\x1b[2J\x1b[3J").as_ref(), b"\x1b[H\x1b[2J");
        assert_eq!(without_history_clears(b"\x1b[3J\x1b[3J").as_ref(), b"");
        assert!(matches!(without_history_clears(b"plain \x1b[1m"), std::borrow::Cow::Borrowed(_)));
    }

    #[gpui::test]
    fn a_program_cannot_erase_the_history_unless_the_setting_allows_it(cx: &mut TestAppContext) {
        let (term, cx) = setup(cx);
        let rows = term.read_with(cx, |t, _| t.term.screen_lines());
        term.update(cx, |t, _| t.feed("line\r\n".repeat(rows + 20).as_bytes()));
        let kept = term.update(cx, |t, _| {
            t.feed(b"\x1b[3J");
            t.a11y_scroll().1
        });
        assert!(kept >= 20, "the history survived: {kept}");
        term.update(cx, |t, _| t.allow_clear_history = true);
        let erased = term.update(cx, |t, _| {
            t.feed(b"\x1b[3J");
            t.a11y_scroll().1
        });
        assert_eq!(erased, 0, "with the setting on a program may erase it");
    }

    fn setup(cx: &mut TestAppContext) -> (gpui::Entity<TerminalView>, &mut gpui::VisualTestContext) {
        crate::qa_workbench_tests::globals(cx);
        cx.add_window_view(|_, cx| TerminalView::output_only(Arc::new(host::local()), "test".into(), cx))
    }

    fn frame(n: usize) -> String {
        (1..=n).map(|i| format!("frame {i}")).collect::<Vec<_>>().join("\r\n")
    }

    #[gpui::test]
    fn redrawing_after_a_clear_does_not_fill_the_history_with_old_frames(cx: &mut TestAppContext) {
        let (term, cx) = setup(cx);
        let history = term.update(cx, |t, _| {
            t.feed(b"before\r\n");
            for _ in 0..20 {
                t.feed(format!("\x1b[2J\x1b[H{}", frame(5)).as_bytes());
            }
            t.a11y_scroll().1
        });
        assert_eq!(history, 0, "nothing was pushed up: the frames replaced each other");
        let lines = term.read_with(cx, |t, _| t.screen_text());
        assert!(lines.starts_with("frame 1\nframe 2"), "{lines:?}");
        assert!(!lines.contains("before"), "the clear wiped the screen");
    }

    #[gpui::test]
    fn the_cursor_stays_where_it_was_after_a_clear(cx: &mut TestAppContext) {
        let (term, cx) = setup(cx);
        term.update(cx, |t, _| t.feed(b"one\r\ntwo\x1b[2Jx"));
        let lines = term.read_with(cx, |t, _| t.screen_text());
        assert_eq!(lines.lines().nth(1), Some("   x"), "ED 2 does not move the cursor: {lines:?}");
    }

    #[gpui::test]
    fn old_lines_that_already_scrolled_up_stay_in_the_history(cx: &mut TestAppContext) {
        let (term, cx) = setup(cx);
        let rows = term.read_with(cx, |t, _| t.term.screen_lines());
        let history = term.update(cx, |t, _| {
            t.feed("old\r\n".repeat(rows + 30).as_bytes());
            let before = t.a11y_scroll().1;
            t.feed(b"\x1b[2J\x1b[Hnew");
            (before, t.a11y_scroll().1)
        });
        assert_eq!(history.0, history.1, "a clear keeps what had scrolled off already");
        assert!(history.0 >= 30);
    }

    #[gpui::test]
    fn a_clear_inside_a_synchronized_update_still_applies_in_order(cx: &mut TestAppContext) {
        let (term, cx) = setup(cx);
        term.update(cx, |t, _| {
            t.feed(b"stale\r\n");
            t.feed(format!("\x1b[?2026h\x1b[2J\x1b[H{}\x1b[?2026l", frame(3)).as_bytes());
        });
        let lines = term.read_with(cx, |t, _| t.screen_text());
        assert!(lines.starts_with("frame 1\nframe 2\nframe 3"), "{lines:?}");
        assert!(!lines.contains("stale"));
    }

    #[test]
    fn an_incomplete_erase_at_the_end_of_a_read_is_held_back() {
        assert_eq!(partial_erase_at_end(b"abc\x1b["), 2);
        assert_eq!(partial_erase_at_end(b"abc\x1b[3"), 3);
        assert_eq!(partial_erase_at_end(b"\x1b[2"), 3);
        assert_eq!(partial_erase_at_end(b"abc\x1b"), 0, "a lone ESC is passed on");
        assert_eq!(partial_erase_at_end(b"\x1b[1"), 0, "another sequence is not ours");
        assert_eq!(partial_erase_at_end(b"abc"), 0);
    }

    #[gpui::test]
    fn an_erase_split_between_two_reads_still_counts(cx: &mut TestAppContext) {
        let (term, cx) = setup(cx);
        let rows = term.read_with(cx, |t, _| t.term.screen_lines());
        term.update(cx, |t, _| t.feed("line\r\n".repeat(rows + 20).as_bytes()));
        let history = term.update(cx, |t, _| {
            t.feed(b"\x1b[");
            t.feed(b"3J");
            t.a11y_scroll().1
        });
        assert!(history >= 20, "the history clear split in two is still refused: {history}");
        let history = term.update(cx, |t, _| {
            t.feed(b"\x1b[2");
            t.feed(b"Jx");
            t.a11y_scroll().1
        });
        assert!(history >= 20, "and a split screen clear does not push the screen up either: {history}");
        let lines = term.read_with(cx, |t, _| t.screen_text());
        assert!(lines.contains('x') && !lines.contains("line"), "{lines:?}");
    }

    #[gpui::test]
    fn a_synchronized_clear_leaves_the_history_alone(cx: &mut TestAppContext) {
        let (term, cx) = setup(cx);
        let history = term.update(cx, |t, _| {
            t.feed(b"stale\r\n");
            for _ in 0..10 {
                t.feed(format!("\x1b[?2026h\x1b[2J\x1b[H{}\x1b[?2026l", frame(5)).as_bytes());
            }
            t.a11y_scroll().1
        });
        assert_eq!(history, 0, "ten synchronized redraws added nothing to the history");
    }

    #[gpui::test]
    fn a_clear_drops_the_selection_and_the_marks_of_the_rows_it_erased(cx: &mut TestAppContext) {
        let (term, cx) = setup(cx);
        term.update_in(cx, |t, window, cx| {
            t.feed(b"\x1b]133;A\x07$ ls\r\n\x1b]133;C\x07out\r\n\x1b]133;D;0\x07");
            t.select_all(&TermSelectAll, window, cx);
        });
        assert!(term.read_with(cx, |t, _| t.selected_text()).is_some(), "something is selected before the clear");
        assert_eq!(term.read_with(cx, |t, _| t.command_count_for_test()), 1);
        term.update(cx, |t, _| t.feed(b"\x1b[2J"));
        assert_eq!(term.read_with(cx, |t, _| t.command_count_for_test()), 0, "the erased prompt is not marked any more");
        assert_eq!(term.read_with(cx, |t, _| t.selected_text()), None, "nothing stays selected over blank rows");
    }

    #[gpui::test]
    fn resetting_an_output_terminal_leaves_no_history_behind(cx: &mut TestAppContext) {
        let (term, cx) = setup(cx);
        let rows = term.read_with(cx, |t, _| t.term.screen_lines());
        term.update(cx, |t, _| t.feed("run one\r\n".repeat(rows + 10).as_bytes()));
        let history = term.update_in(cx, |t, _, cx| {
            t.reset_output(cx);
            t.a11y_scroll().1
        });
        assert_eq!(history, 0, "a new run starts with an empty history");
    }

    #[gpui::test]
    fn the_alternate_screen_is_left_to_the_parser(cx: &mut TestAppContext) {
        let (term, cx) = setup(cx);
        term.update(cx, |t, _| {
            t.feed(b"shell text\r\n\x1b[?1049h\x1b[2J\x1b[Hfull screen app\x1b[?1049l");
        });
        let lines = term.read_with(cx, |t, _| t.screen_text());
        assert!(lines.contains("shell text"), "leaving the alternate screen brings the shell back: {lines:?}");
    }
}
