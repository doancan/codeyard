//! The Keyboard Shortcuts page in a real workbench: recording keys (also on other layouts and as a sequence), no
//! shortcut acting while it listens, writing `keybindings.jsonc` with its comments and line endings, conflicts, removing
//! and resetting keys.

use std::path::{Path, PathBuf};

use gpui::{Entity, TestAppContext, VisualTestContext};

use super::rows::{Row, Source};
use super::{ShortcutsEvent, ShortcutsPage};
use crate::workbench::Workbench;

fn config(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("codeyard-shortcuts-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn setup<'a>(cx: &'a mut TestAppContext, dir: &Path) -> (Entity<Workbench>, &'a mut VisualTestContext) {
    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(crate::ui::ActiveTheme(theme::DARK));
        cx.set_global(crate::settings::Settings::default());
    });
    let dir = dir.to_path_buf();
    let (wb, cx) = cx.add_window_view(move |window, cx| Workbench::with_state(state::State::in_memory().ok(), Some(dir), window, cx));
    wb.update_in(cx, |wb, window, cx| window.focus(&gpui::Focusable::focus_handle(wb, cx)));
    (wb, cx)
}

fn open_page(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Entity<ShortcutsPage> {
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Preferences: Keyboard Shortcuts", window, cx));
    cx.run_until_parked();
    wb.read_with(cx, |wb, _| wb.shortcuts_page_for_test()).expect("the page opened")
}

fn row(page: &Entity<ShortcutsPage>, id: &str, cx: &mut VisualTestContext) -> Row {
    page.read_with(cx, |p, _| p.rows_for_test().iter().find(|r| r.id.as_deref() == Some(id)).cloned()).unwrap_or_else(|| panic!("no row for {id}"))
}

fn rows_of(page: &Entity<ShortcutsPage>, id: &str, cx: &mut VisualTestContext) -> Vec<Row> {
    page.read_with(cx, |p, _| p.rows_for_test().iter().filter(|r| r.id.as_deref() == Some(id)).cloned().collect())
}

fn record(page: &Entity<ShortcutsPage>, row: &Row, add: bool, cx: &mut VisualTestContext) {
    page.update_in(cx, |p, window, cx| p.start_recording(row, add, window, cx));
}

fn file(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("keybindings.jsonc")).unwrap_or_default()
}

const PALETTE: &str = "workbench.commandPalette";
const GO_TO_LINE: &str = "workbench.goToLine";

#[gpui::test]
fn preferences_keyboard_shortcuts_opens_the_page_with_every_command(cx: &mut TestAppContext) {
    let dir = config("opens");
    let (wb, cx) = setup(cx, &dir);
    let page = open_page(&wb, cx);
    assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), Some(("shortcuts", true)));
    let go = row(&page, GO_TO_LINE, cx);
    let binding = go.binding.expect("Go to Line has a key");
    assert_eq!(binding.source, Source::Default);
    assert!(page.read_with(cx, |p, _| p.rows_for_test().len()) > 200);
    // A command only keybindings.jsonc can name is listed too, by its id.
    assert!(page.read_with(cx, |p, _| p.rows_for_test().iter().any(|r| r.binding.is_none() && r.id.is_some())));
    assert!(!dir.join("keybindings.jsonc").exists(), "opening the page writes nothing");
}

#[gpui::test]
fn a_recorded_key_is_written_with_comments_and_crlf_kept_and_works_at_once(cx: &mut TestAppContext) {
    let dir = config("record");
    let before = "// my keys\r\n[\r\n  // palette on a mouse-friendly key\r\n  { \"key\": \"ctrl-alt-shift-1\", \"command\": \"workbench.quickOpen\" }, // keep\r\n]\r\n";
    std::fs::write(dir.join("keybindings.jsonc"), before).unwrap();
    let (wb, cx) = setup(cx, &dir);
    let page = open_page(&wb, cx);
    let target = row(&page, PALETTE, cx);
    record(&page, &target, true, cx);
    cx.simulate_keystrokes("ctrl-alt-shift-8");
    assert_eq!(page.read_with(cx, |p, _| p.recording_for_test()).as_deref(), Some("ctrl-alt-shift-8"), "a key is shown but not saved before Enter");
    assert_eq!(file(&dir), before);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let after = file(&dir);
    assert!(after.starts_with("// my keys\r\n[\r\n  // palette on a mouse-friendly key\r\n") && after.contains("// keep"), "{after:?}");
    assert!(after.contains("\"key\": \"ctrl-alt-shift-8\", \"command\": \"workbench.commandPalette\""), "{after}");
    assert_eq!(after.matches('\n').count(), after.matches("\r\n").count(), "no bare LF: {after:?}");
    assert_eq!(page.read_with(cx, |p, _| p.recording_for_test()), None);
    // The key works without a restart, and the page lists it as the user's.
    cx.simulate_keystrokes("ctrl-alt-shift-8");
    assert!(wb.read_with(cx, |wb, _| wb.palette_open()));
    let mine = rows_of(&page, PALETTE, cx).into_iter().filter_map(|r| r.binding).find(|b| b.keys == "ctrl-alt-shift-8").expect("listed");
    assert_eq!(mine.source, Source::User);
    let status = page.read_with(cx, |p, _| p.status_for_test().cloned()).expect("said what happened");
    assert!(!status.problem, "{status:?}");
}

#[gpui::test]
fn escape_cancels_and_changes_nothing(cx: &mut TestAppContext) {
    let dir = config("escape");
    let (wb, cx) = setup(cx, &dir);
    let page = open_page(&wb, cx);
    let target = row(&page, PALETTE, cx);
    record(&page, &target, true, cx);
    cx.simulate_keystrokes("ctrl-alt-shift-8 escape");
    assert_eq!(page.read_with(cx, |p, _| p.recording_for_test()), None);
    assert!(!dir.join("keybindings.jsonc").exists());
    assert!(!wb.read_with(cx, |wb, _| wb.palette_open()));
}

#[gpui::test]
fn no_shortcut_acts_while_the_page_listens(cx: &mut TestAppContext) {
    let dir = config("global");
    let (wb, cx) = setup(cx, &dir);
    cx.update(|_, cx| cx.bind_keys([gpui::KeyBinding::new("ctrl-alt-shift-7", crate::workbench::CommandPalette, None)]));
    let page = open_page(&wb, cx);
    let target = row(&page, GO_TO_LINE, cx);
    record(&page, &target, true, cx);
    cx.simulate_keystrokes("ctrl-alt-shift-7");
    assert!(!wb.read_with(cx, |wb, _| wb.palette_open()), "the key was recorded, not run");
    assert_eq!(page.read_with(cx, |p, _| p.recording_for_test()).as_deref(), Some("ctrl-alt-shift-7"));
    // Built-in shortcuts too: the key for Quick Open is a key to record, not a command.
    let quick_open = cx.update(|_, cx| crate::commands::Command::action("", crate::workbench::QuickOpen).shortcut(cx)).expect("bound");
    cx.simulate_keystrokes("escape");
    record(&page, &target, true, cx);
    cx.simulate_keystrokes(if cfg!(target_os = "macos") { "cmd-p" } else { "ctrl-p" });
    assert!(!wb.read_with(cx, |wb, _| wb.palette_open()), "{quick_open} did not open Quick Open");
    assert!(page.read_with(cx, |p, _| p.recording_for_test()).is_some());
    // After recording ends the shortcut acts again.
    cx.simulate_keystrokes("escape");
    cx.simulate_keystrokes("ctrl-alt-shift-7");
    assert!(wb.read_with(cx, |wb, _| wb.palette_open()));
}

#[gpui::test]
fn keys_of_other_layouts_and_sequences_are_recorded_as_pressed(cx: &mut TestAppContext) {
    let dir = config("layout");
    let (wb, cx) = setup(cx, &dir);
    let page = open_page(&wb, cx);
    let target = row(&page, PALETTE, cx);
    record(&page, &target, true, cx);
    cx.simulate_keystrokes("ctrl-alt-ö enter");
    assert!(file(&dir).contains("\"key\": \"alt-ctrl-ö\"") || file(&dir).contains("\"key\": \"ctrl-alt-ö\""), "{}", file(&dir));
    cx.simulate_keystrokes("ctrl-alt-ö");
    assert!(wb.read_with(cx, |wb, _| wb.palette_open()), "the Turkish key runs the command");
    cx.simulate_keystrokes("escape");

    let target = row(&page, GO_TO_LINE, cx);
    record(&page, &target, true, cx);
    cx.simulate_keystrokes("ctrl-alt-k ctrl-alt-i enter");
    assert!(file(&dir).contains("ctrl-alt-k ctrl-alt-i"), "{}", file(&dir));
    let mine = rows_of(&page, GO_TO_LINE, cx).into_iter().filter_map(|r| r.binding).find(|b| b.source == Source::User).expect("the sequence is listed");
    assert!(mine.keys.contains(' '), "{mine:?}");
    cx.simulate_keystrokes("ctrl-alt-k ctrl-alt-i");
    assert!(wb.read_with(cx, |wb, _| wb.palette_open()), "the sequence runs the command");
}

#[gpui::test]
fn a_key_another_command_uses_is_reported_with_its_name_on_both_rows(cx: &mut TestAppContext) {
    let dir = config("conflict");
    std::fs::write(dir.join("keybindings.jsonc"), "[ { \"key\": \"ctrl-alt-shift-5\", \"command\": \"workbench.commandPalette\" } ]").unwrap();
    let (wb, cx) = setup(cx, &dir);
    let page = open_page(&wb, cx);
    let go = row(&page, GO_TO_LINE, cx);
    record(&page, &go, true, cx);
    cx.simulate_keystrokes("ctrl-alt-shift-5 enter");
    let status = page.read_with(cx, |p, _| p.status_for_test().cloned()).unwrap();
    assert!(status.problem && status.text.contains("also used by"), "{status:?}");
    let other =
        rows_of(&page, PALETTE, cx).into_iter().filter_map(|r| r.binding).find(|b| b.keys == "alt-ctrl-shift-5" || b.keys == "ctrl-alt-shift-5").unwrap();
    assert_eq!(other.conflicts.len(), 1, "{other:?}");
    let mine = rows_of(&page, GO_TO_LINE, cx).into_iter().filter_map(|r| r.binding).find(|b| b.source == Source::User).unwrap();
    assert_eq!(mine.conflicts, [other_label(&page, PALETTE, cx)]);
    // The page's own search finds the clash.
    page.update(cx, |p, cx| p.set_query("conflict", cx));
    let found = page.read_with(cx, |p, cx| p.visible_for_test(cx));
    let (a, b) = (row(&page, GO_TO_LINE, cx).label, row(&page, PALETTE, cx).label);
    assert!(found.contains(&a) && found.contains(&b), "{found:?}");
}

fn other_label(page: &Entity<ShortcutsPage>, id: &str, cx: &mut VisualTestContext) -> String {
    row(page, id, cx).label
}

#[gpui::test]
fn changing_a_default_key_turns_it_off_and_reset_brings_it_back(cx: &mut TestAppContext) {
    let dir = config("default");
    let (wb, cx) = setup(cx, &dir);
    let page = open_page(&wb, cx);
    let go = row(&page, GO_TO_LINE, cx);
    let old = go.binding.clone().unwrap();
    record(&page, &go, false, cx);
    cx.simulate_keystrokes("ctrl-alt-shift-6 enter");
    let text = file(&dir);
    assert!(text.find("\"command\": null").is_some_and(|off| off < text.find("workbench.goToLine").unwrap()), "the old key is turned off first: {text}");
    let now: Vec<_> = rows_of(&page, GO_TO_LINE, cx).into_iter().filter_map(|r| r.binding).collect();
    assert!(now.iter().any(|b| b.keys == old.keys && b.disabled), "the old key is listed as off: {now:?}");
    assert!(now.iter().any(|b| b.source == Source::User && !b.disabled));
    // Reset removes both entries.
    let any = rows_of(&page, GO_TO_LINE, cx).remove(0);
    page.update(cx, |_, cx| cx.emit(ShortcutsEvent::Reset(any)));
    cx.run_until_parked();
    let back: Vec<_> = rows_of(&page, GO_TO_LINE, cx).into_iter().filter_map(|r| r.binding).collect();
    assert_eq!(back.len(), 1, "{back:?}");
    assert_eq!((back[0].keys.clone(), back[0].source, back[0].disabled), (old.keys, Source::Default, false));
    assert!(crate::shortcuts_page::edit::read_entries(&file(&dir)).unwrap().is_empty(), "{}", file(&dir));
}

#[gpui::test]
fn removing_a_key_turns_a_default_off_and_deletes_a_users_entry(cx: &mut TestAppContext) {
    let dir = config("remove");
    std::fs::write(dir.join("keybindings.jsonc"), "[\n  // mine\n  { \"key\": \"ctrl-alt-shift-4\", \"command\": \"workbench.commandPalette\" },\n]\n")
        .unwrap();
    let (wb, cx) = setup(cx, &dir);
    let page = open_page(&wb, cx);
    let mine = rows_of(&page, PALETTE, cx).into_iter().find(|r| r.binding.as_ref().is_some_and(|b| b.source == Source::User)).unwrap();
    page.update(cx, |_, cx| cx.emit(ShortcutsEvent::Remove(mine)));
    cx.run_until_parked();
    let text = file(&dir);
    assert!(text.contains("// mine") && !text.contains("ctrl-alt-shift-4"), "{text}");

    let go = row(&page, GO_TO_LINE, cx);
    let key = go.binding.clone().unwrap().keys;
    page.update(cx, |_, cx| cx.emit(ShortcutsEvent::Remove(go)));
    cx.run_until_parked();
    assert!(rows_of(&page, GO_TO_LINE, cx).iter().all(|r| r.binding.as_ref().is_none_or(|b| b.disabled)), "the default key is off");
    assert!(file(&dir).contains(&format!("\"key\": \"{key}\"")) && file(&dir).contains("\"command\": null"), "{}", file(&dir));
}

#[gpui::test]
fn a_file_that_does_not_parse_is_never_written_over(cx: &mut TestAppContext) {
    let dir = config("broken");
    let broken = "[ { \"key\": \"ctrl-1\", ";
    std::fs::write(dir.join("keybindings.jsonc"), broken).unwrap();
    let (wb, cx) = setup(cx, &dir);
    let page = open_page(&wb, cx);
    let target = row(&page, PALETTE, cx);
    record(&page, &target, true, cx);
    cx.simulate_keystrokes("ctrl-alt-shift-3 enter");
    assert_eq!(file(&dir), broken);
    let status = page.read_with(cx, |p, _| p.status_for_test().cloned()).unwrap();
    assert!(status.problem && status.text.contains("keybindings.jsonc"), "{status:?}");
}

#[gpui::test]
fn without_a_settings_folder_nothing_can_be_recorded(cx: &mut TestAppContext) {
    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(crate::ui::ActiveTheme(theme::DARK));
        cx.set_global(crate::settings::Settings::default());
    });
    let (wb, cx) = cx.add_window_view(|window, cx| Workbench::with_state(state::State::in_memory().ok(), None, window, cx));
    let page = open_page(&wb, cx);
    let target = row(&page, PALETTE, cx);
    record(&page, &target, true, cx);
    assert_eq!(page.read_with(cx, |p, _| p.recording_for_test()), None);
}

#[gpui::test]
fn the_search_finds_commands_by_name_and_keys_and_the_keyboard_moves_the_selection(cx: &mut TestAppContext) {
    let dir = config("search");
    let (wb, cx) = setup(cx, &dir);
    let page = open_page(&wb, cx);
    page.update(cx, |p, cx| p.set_query("go: line", cx));
    assert!(page.read_with(cx, |p, cx| p.visible_for_test(cx)).contains(&"Go: Line…".to_string()));
    page.update(cx, |p, cx| p.set_query("workbench.goToLine", cx));
    assert!(page.read_with(cx, |p, cx| p.visible_for_test(cx)).contains(&"Go: Line…".to_string()), "the id of the command finds it");
    page.update(cx, |p, cx| p.set_query("nothing like this", cx));
    assert!(page.read_with(cx, |p, cx| p.visible_for_test(cx)).is_empty());
    page.update(cx, |p, cx| p.set_query("", cx));
    page.update_in(cx, |p, window, cx| p.select_visible(2, window, cx));
    let first = page.read_with(cx, |p, _| p.selected_for_test()).unwrap();
    cx.simulate_keystrokes("down");
    let second = page.read_with(cx, |p, _| p.selected_for_test()).unwrap();
    assert_ne!(first.label, second.label);
    cx.simulate_keystrokes("up");
    assert_eq!(page.read_with(cx, |p, _| p.selected_for_test()).unwrap().label, first.label);
    cx.simulate_keystrokes("enter");
    assert!(page.read_with(cx, |p, _| p.is_recording()), "Enter on a row starts recording its key");
}

#[gpui::test]
fn a_printable_key_on_its_own_is_refused_with_a_reason_and_recording_goes_on(cx: &mut TestAppContext) {
    let dir = config("bare");
    let (wb, cx) = setup(cx, &dir);
    let page = open_page(&wb, cx);
    let target = row(&page, PALETTE, cx);
    record(&page, &target, true, cx);
    cx.simulate_keystrokes("a");
    let status = page.read_with(cx, |p, _| p.shown_status_for_test()).expect("the reason is said");
    assert!(status.problem && status.text.contains("types text"), "{status:?}");
    assert_eq!(page.read_with(cx, |p, _| p.recording_for_test()).as_deref(), Some(""), "still listening, nothing taken");
    cx.simulate_keystrokes("ctrl-alt-shift-8 enter");
    let text = file(&dir);
    assert!(text.contains("ctrl-alt-shift-8") && !text.contains("\"key\": \"a\""), "{text}");
}

#[gpui::test]
fn changing_a_key_to_one_a_later_null_turns_off_moves_the_entry_to_the_end(cx: &mut TestAppContext) {
    let dir = config("dead");
    let before = "[ { \"key\": \"ctrl-alt-shift-1\", \"command\": \"workbench.commandPalette\" }, { \"key\": \"ctrl-alt-shift-2\", \"command\": null } ]";
    std::fs::write(dir.join("keybindings.jsonc"), before).unwrap();
    let (wb, cx) = setup(cx, &dir);
    let page = open_page(&wb, cx);
    let mine = rows_of(&page, PALETTE, cx).into_iter().find(|r| r.binding.as_ref().is_some_and(|b| b.source == Source::User)).unwrap();
    record(&page, &mine, false, cx);
    cx.simulate_keystrokes("ctrl-alt-shift-2 enter");
    let entries = crate::shortcuts_page::edit::read_entries(&file(&dir)).unwrap();
    let last = entries.last().unwrap();
    assert_eq!((last.command.as_deref(), last.keys.as_str()), (Some("workbench.commandPalette"), "ctrl-alt-shift-2"), "{}", file(&dir));
    cx.simulate_keystrokes("ctrl-alt-shift-2");
    assert!(wb.read_with(cx, |wb, _| wb.palette_open()), "the key works");
}

#[gpui::test]
fn the_open_page_follows_a_hand_edit_of_the_file(cx: &mut TestAppContext) {
    let dir = config("follow");
    let (wb, cx) = setup(cx, &dir);
    let page = open_page(&wb, cx);
    assert!(rows_of(&page, PALETTE, cx).iter().all(|r| r.binding.as_ref().is_none_or(|b| b.source != Source::User)));
    std::fs::write(dir.join("keybindings.jsonc"), "[ { \"key\": \"ctrl-alt-shift-3\", \"command\": \"workbench.commandPalette\" } ]").unwrap();
    cx.executor().advance_clock(std::time::Duration::from_secs(2));
    cx.run_until_parked();
    assert!(rows_of(&page, PALETTE, cx).iter().any(|r| r.binding.as_ref().is_some_and(|b| b.source == Source::User)), "a hand edit shows");
}

#[gpui::test]
fn removing_an_entry_with_args_says_which_entry_and_that_its_args_went(cx: &mut TestAppContext) {
    let dir = config("args");
    std::fs::write(dir.join("keybindings.jsonc"), "[ { \"key\": \"ctrl-alt-shift-4\", \"command\": \"workbench.commandPalette\", \"args\": { \"x\": 1 } } ]")
        .unwrap();
    let (wb, cx) = setup(cx, &dir);
    let page = open_page(&wb, cx);
    let mine = rows_of(&page, PALETTE, cx).into_iter().find(|r| r.binding.as_ref().is_some_and(|b| b.source == Source::User)).unwrap();
    page.update(cx, |_, cx| cx.emit(ShortcutsEvent::Remove(mine)));
    cx.run_until_parked();
    let status = page.read_with(cx, |p, _| p.status_for_test().cloned()).unwrap();
    assert!(status.text.contains("args") && status.text.contains("Removed"), "{status:?}");
    assert!(!file(&dir).contains("ctrl-alt-shift-4"));
}
