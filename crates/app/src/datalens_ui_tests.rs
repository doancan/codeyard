//! Data lens in a window: convert into a new tab or in place, the live query bar, the path under the cursor.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{Entity, Focusable, TestAppContext, VisualTestContext};

use crate::datalens::Format;
use crate::editor::EditorView;
use crate::ui::ActiveTheme;
use crate::workbench::{DataCopyPath, DataQuery, Workbench};

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-data-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn setup<'a>(cx: &'a mut TestAppContext, config: &std::path::Path) -> (Entity<Workbench>, &'a mut VisualTestContext) {
    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(ActiveTheme(theme::DARK));
        cx.set_global(crate::settings::Settings::default());
    });
    let config = config.to_path_buf();
    let (wb, cx) = cx.add_window_view(move |window, cx| Workbench::with_state(state::State::in_memory().ok(), Some(config), window, cx));
    wb.update_in(cx, |wb, window, cx| window.focus(&wb.focus_handle(cx)));
    (wb, cx)
}

fn open(wb: &Entity<Workbench>, path: &std::path::Path, cx: &mut VisualTestContext) -> Entity<EditorView> {
    let p = path.to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_path(&p, window, cx));
    cx.run_until_parked();
    cx.run_until_parked();
    wb.read_with(cx, |wb, _| wb.active_editor_entity()).expect("editor opened")
}

fn text(ed: &Entity<EditorView>, cx: &mut VisualTestContext) -> String {
    ed.read_with(cx, |e, _| e.text())
}

#[gpui::test]
fn convert_opens_a_tab_or_replaces_in_place(cx: &mut TestAppContext) {
    let d = dir("convert");
    let file = d.join("app.json");
    std::fs::write(&file, "{\"name\": \"x\", \"ports\": [80, 443]}\n").unwrap();
    let (wb, cx) = setup(cx, &d.join("config"));
    let ed = open(&wb, &file, cx);

    wb.update_in(cx, |wb, window, cx| wb.convert_for_test(Format::Yaml, false, window, cx));
    cx.run_until_parked();
    cx.run_until_parked();
    let path = wb.read_with(cx, |wb, cx| wb.active_path(cx)).unwrap();
    assert!(path.ends_with("scratches/app.yaml"), "{path}");
    let tab = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    assert_eq!(text(&tab, cx), "name: x\nports:\n  - 80\n  - 443\n");
    assert_eq!(text(&ed, cx), "{\"name\": \"x\", \"ports\": [80, 443]}\n", "the source is untouched");

    open(&wb, &file, cx);
    wb.update_in(cx, |wb, window, cx| wb.convert_for_test(Format::Toml, true, window, cx));
    cx.run_until_parked();
    assert_eq!(text(&ed, cx), "name = \"x\"\nports = [80, 443]\n");
}

#[gpui::test]
fn a_refused_conversion_leaves_the_text_and_says_why(cx: &mut TestAppContext) {
    let d = dir("refused");
    let file = d.join("a.json");
    std::fs::write(&file, "{\n  \"a\": null\n}\n").unwrap();
    let (wb, cx) = setup(cx, &d.join("config"));
    let ed = open(&wb, &file, cx);
    wb.update_in(cx, |wb, window, cx| wb.convert_for_test(Format::Toml, true, window, cx));
    cx.run_until_parked();
    assert_eq!(text(&ed, cx), "{\n  \"a\": null\n}\n");
    let notice = wb.read_with(cx, |wb, _| wb.notice_for_test());
    assert!(notice.as_ref().is_some_and(|(error, n)| *error && n.contains(".a") && n.contains("no null")), "{notice:?}");
}

#[gpui::test]
fn the_query_bar_fills_a_results_tab_as_you_type(cx: &mut TestAppContext) {
    let d = dir("query");
    let file = d.join("items.json");
    std::fs::write(&file, "{\"items\": [{\"id\": 1, \"n\": \"a\"}, {\"id\": 3, \"n\": \"b\"}]}\n").unwrap();
    let (wb, cx) = setup(cx, &d.join("config"));
    open(&wb, &file, cx);

    cx.dispatch_action(DataQuery);
    cx.run_until_parked();
    assert!(wb.read_with(cx, |wb, _| wb.query_status_for_test()).is_some(), "the bar is open");

    let query = |cx: &mut VisualTestContext, q: &str| {
        wb.update(cx, |wb, cx| wb.query_text_for_test(q, cx));
        cx.executor().advance_clock(Duration::from_millis(150));
        for _ in 0..20 {
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(60));
        }
    };
    query(cx, ".items[] | select(.id == 3)");
    assert_eq!(wb.read_with(cx, |wb, _| wb.query_status_for_test()).as_deref(), Some("1 results"));
    let path = wb.read_with(cx, |wb, cx| wb.active_path(cx)).unwrap();
    assert!(path.ends_with("scratches/query.json"), "{path}");
    let tab = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    assert_eq!(text(&tab, cx), "{\n  \"id\": 3,\n  \"n\": \"b\"\n}\n");

    // A second query reuses the same tab.
    query(cx, ".items | map(.n)");
    assert_eq!(wb.read_with(cx, |wb, cx| wb.active_path(cx)).unwrap(), path);
    assert_eq!(text(&tab, cx), "[\n  \"a\",\n  \"b\"\n]\n");

    // A syntax error is shown in the bar with its column and leaves the results alone.
    query(cx, ".items[");
    let status = wb.read_with(cx, |wb, _| wb.query_status_for_test()).unwrap();
    assert!(status.starts_with("failed Some(8)"), "{status}");
    assert_eq!(text(&tab, cx), "[\n  \"a\",\n  \"b\"\n]\n");
}

#[gpui::test]
fn the_path_at_the_cursor_is_copied_and_shown(cx: &mut TestAppContext) {
    let d = dir("path");
    let file = d.join("c.yaml");
    std::fs::write(&file, "servers:\n  - host: a\n  - host: ğüş\n").unwrap();
    let (wb, cx) = setup(cx, &d.join("config"));
    let ed = open(&wb, &file, cx);
    ed.update(cx, |e, cx| e.set_cursor_line_col_for_test(2, 12, cx));
    assert_eq!(wb.read_with(cx, |wb, cx| wb.data_path_label(cx)).as_deref(), Some(".servers[1].host"));
    cx.dispatch_action(DataCopyPath);
    cx.run_until_parked();
    assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some(".servers[1].host"));
}
