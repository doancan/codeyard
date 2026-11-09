//! Tab shortcuts, visibility switches, encodings and the recent-file list.

use super::*;
use crate::encoding::Encoding;
use crate::qa_workbench_tests::{open, project, setup};
use gpui::TestAppContext;

fn text_of(wb: &Entity<Workbench>, cx: &mut gpui::VisualTestContext) -> String {
    wb.read_with(cx, |wb, cx| wb.active_editor().map(|e| e.read(cx).text()).unwrap_or_default())
}

fn active_file(wb: &Entity<Workbench>, cx: &mut gpui::VisualTestContext) -> String {
    wb.read_with(cx, |wb, cx| wb.active_path(cx)).map(|p| crate::ui::file_name(&p).to_string()).unwrap_or_default()
}

#[gpui::test]
fn control_digits_go_to_the_tab_at_that_place_and_nine_to_the_last(cx: &mut TestAppContext) {
    let dir = project("tab-index");
    let (wb, cx) = setup(cx);
    for file in ["README.md", "src/main.rs", "src/lib.rs"] {
        open(&wb, &dir.join(file), cx);
    }
    let tabs: Vec<String> = wb.read_with(cx, |wb, cx| wb.open_paths(cx)).iter().map(|p| crate::ui::file_name(p).to_string()).collect();
    assert!(tabs.len() >= 2, "the files are open as tabs (a preview tab gives way to the next file): {tabs:?}");
    let name = |i: usize| tabs[i].clone();
    for index in [0, 1] {
        cx.dispatch_action(OpenEditorAt { index });
        assert_eq!(active_file(&wb, cx), name(index), "⌃{} goes to tab {}", index + 1, index + 1);
    }
    cx.dispatch_action(OpenEditorAt { index: usize::MAX });
    assert_eq!(active_file(&wb, cx), name(tabs.len() - 1), "past the last tab means the last tab");
    cx.dispatch_action(OpenEditorAt { index: 0 });
    cx.dispatch_action(OpenEditorAt { index: 40 });
    assert_eq!(active_file(&wb, cx), name(tabs.len() - 1), "a place that is not there stays on the last tab");
}

#[gpui::test]
fn the_visibility_and_tab_focus_switches_flip_their_settings(cx: &mut TestAppContext) {
    let (_wb, cx) = setup(cx);
    let read = |cx: &mut gpui::VisualTestContext| {
        cx.update(|_, cx| (cx.global::<Settings>().status_bar, cx.global::<Settings>().activity_bar, cx.global::<Settings>().tab_focus_mode))
    };
    assert_eq!(read(cx), (true, true, false));
    cx.dispatch_action(ToggleStatusBar);
    assert_eq!(read(cx), (false, true, false));
    cx.dispatch_action(ToggleActivityBar);
    assert_eq!(read(cx), (false, false, false));
    cx.dispatch_action(ToggleTabFocusMode);
    assert_eq!(read(cx), (false, false, true));
    cx.dispatch_action(ToggleStatusBar);
    cx.dispatch_action(ToggleActivityBar);
    cx.dispatch_action(ToggleTabFocusMode);
    assert_eq!(read(cx), (true, true, false), "each switch flips back");
}

#[gpui::test]
fn a_file_reopens_in_another_encoding_and_saves_in_a_third(cx: &mut TestAppContext) {
    let dir = project("encoding");
    let file = dir.join("legacy.txt");
    std::fs::write(&file, b"a\xFE\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    assert_eq!(text_of(&wb, cx), "aş\n", "not UTF-8: read with the fallback encoding");
    let editor = wb.read_with(cx, |wb, _| wb.active_editor().cloned().unwrap());

    editor.update(cx, |e, cx| e.reopen_with_encoding(Encoding::Iso8859_1, cx));
    cx.run_until_parked();
    assert_eq!(text_of(&wb, cx), "aþ\n", "the same bytes, read as Latin-1");
    assert_eq!(editor.read_with(cx, |e, _| e.encoding()), Encoding::Iso8859_1);

    // UTF-8 cannot read the byte: the file stays as it is and says why.
    editor.update(cx, |e, cx| e.reopen_with_encoding(Encoding::Utf8, cx));
    cx.run_until_parked();
    assert_eq!(text_of(&wb, cx), "aþ\n");
    assert!(editor.read_with(cx, |e, _| e.error_for_test().is_some()), "a message explains it");

    editor.update(cx, |e, cx| e.save_with_encoding(Encoding::Utf8, cx));
    cx.run_until_parked();
    assert_eq!(std::fs::read(&file).unwrap(), "aþ\n".as_bytes(), "written as UTF-8 now");
    assert_eq!(editor.read_with(cx, |e, _| e.encoding()), Encoding::Utf8);
}

#[gpui::test]
fn saving_with_an_encoding_that_cannot_hold_the_text_changes_nothing(cx: &mut TestAppContext) {
    let dir = project("encoding-refuse");
    let file = dir.join("wide.txt");
    std::fs::write(&file, "ğ😀\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    let editor = wb.read_with(cx, |wb, _| wb.active_editor().cloned().unwrap());
    editor.update(cx, |e, cx| e.save_with_encoding(Encoding::Iso8859_1, cx));
    cx.run_until_parked();
    assert_eq!(std::fs::read(&file).unwrap(), "ğ😀\n".as_bytes(), "the file is untouched");
    assert_eq!(editor.read_with(cx, |e, _| e.encoding()), Encoding::Utf8, "and keeps its encoding");
    assert!(editor.read_with(cx, |e, _| e.error_for_test().is_some()));
}

#[gpui::test]
fn a_utf8_file_with_a_bom_saves_in_another_encoding_without_the_bom_character(cx: &mut TestAppContext) {
    let dir = project("encoding-bom");
    let file = dir.join("bom.txt");
    std::fs::write(&file, "\u{feff}caf\u{e9}\n").unwrap();
    let (wb, cx) = setup(cx);
    open(&wb, &file, cx);
    let editor = wb.read_with(cx, |wb, _| wb.active_editor().cloned().unwrap());
    editor.update(cx, |e, cx| e.save_with_encoding(Encoding::Iso8859_1, cx));
    cx.run_until_parked();
    assert_eq!(std::fs::read(&file).unwrap(), b"caf\xE9\n", "a single-byte encoding has no BOM, and the text is written");
    assert!(editor.read_with(cx, |e, _| e.error_for_test().is_none()));
    editor.update(cx, |e, cx| e.save_with_encoding(Encoding::Utf16Le, cx));
    cx.run_until_parked();
    let bytes = std::fs::read(&file).unwrap();
    assert_eq!(&bytes[..4], [0xFF, 0xFE, b'c', 0], "UTF-16 writes its own BOM once");
}

#[gpui::test]
fn the_recent_file_list_leaves_out_the_file_in_front(cx: &mut TestAppContext) {
    let dir = project("recent-files");
    let (wb, cx) = setup(cx);
    open(&wb, &dir.join("README.md"), cx);
    open(&wb, &dir.join("src/lib.rs"), cx);
    cx.dispatch_action(OpenRecentFile);
    let labels = wb.read_with(cx, |wb, cx| wb.overlay_labels(cx)).expect("the list is open");
    assert!(labels.iter().any(|l| l == "README.md"), "{labels:?}");
    assert!(!labels.iter().any(|l| l == "lib.rs"), "the file in front is not offered: {labels:?}");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(active_file(&wb, cx), "README.md");
}
