//! A saved editor layout that cannot be used must not stop the window from starting.

use gpui::TestAppContext;

use crate::workbench::notifications_glue::model::Severity;

use crate::qa_workbench_tests::{fresh, open, open_in, project, restart_after_kill, setup_persistent};

/// Layouts a damaged database or a newer/older version could leave behind.
const BROKEN_LAYOUTS: &[&str] = &[
    "",
    "not json at all",
    "{}",
    "null",
    r#"{"version":9,"grid":{},"views":[]}"#,
    // Valid shape, but the focused group is not in the tree and a view names a group that does not exist.
    r#"{"version":1,"grid":{"version":1,"root":{"Group":{"id":0}},"focused":7},"views":[{"group":3,"path":"src/main.rs","line":0,"column":0,"scroll_line":0}]}"#,
    "\u{0}\u{1}\u{2}",
];

#[gpui::test]
fn a_broken_saved_layout_falls_back_to_one_group_and_warns(cx: &mut TestAppContext) {
    let home = fresh("home-broken-layout");
    let dir = project("broken-layout");
    let (mut wb, cx) = {
        let (wb, cx) = setup_persistent(cx, &home);
        (wb, cx)
    };
    open(&wb, &dir, cx);
    open_in(&wb, &dir.join("src/main.rs"), cx);
    open_in(&wb, &dir.join("README.md"), cx);
    for (n, broken) in BROKEN_LAYOUTS.iter().enumerate() {
        super::fallback::forget_announcements();
        assert!(wb.update(cx, |wb, cx| wb.prepare_quit(cx)));
        let ws = wb.read_with(cx, |wb, _| wb.persist.workspace_id).unwrap_or_else(|| panic!("a workspace before layout {n}"));
        state::State::open(&home.join("state.db")).unwrap().set(&format!("editor_layout:{ws}"), broken).unwrap();
        wb = restart_after_kill(wb, cx, &home);
        let names: Vec<String> = wb.read_with(cx, |w, cx| w.open_paths(cx)).iter().map(|p| crate::ui::file_name(p).to_string()).collect();
        assert!(names.contains(&"main.rs".to_string()) && names.contains(&"README.md".to_string()), "layout {n} ({broken:?}): the tabs come back: {names:?}");
        assert_eq!(wb.read_with(cx, |w, _| w.tabs.layout.groups().len()), 1, "layout {n}: one group");
        let warned = wb.read_with(cx, |w, _| {
            w.notice_history().iter().any(|(severity, message)| *severity == Severity::Warning && message.contains("editor layout could not be read"))
        });
        assert!(warned, "layout {n} ({broken:?}): the warning");
        // The unreadable text is kept, in one place, before the next save overwrites the layout.
        let kept = state::State::open(&home.join("state.db")).unwrap().get(&super::fallback::corrupt_key(ws));
        assert_eq!(kept.as_deref(), Some(*broken), "layout {n}: a copy of what could not be read");
    }
}

#[gpui::test]
fn a_good_layout_does_not_warn(cx: &mut TestAppContext) {
    let home = fresh("home-good-layout");
    let dir = project("good-layout");
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir, cx);
    open_in(&wb, &dir.join("src/main.rs"), cx);
    assert!(wb.update(cx, |wb, cx| wb.prepare_quit(cx)));
    let wb = restart_after_kill(wb, cx, &home);
    assert!(!wb.read_with(cx, |w, _| w.notice_history().iter().any(|(_, m)| m.contains("editor layout"))));
}

#[gpui::test]
fn the_unreadable_layout_is_announced_once_per_session_and_one_copy_is_kept(cx: &mut TestAppContext) {
    let home = fresh("home-once-layout");
    let dir = project("once-layout");
    let (mut wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir, cx);
    open_in(&wb, &dir.join("src/main.rs"), cx);
    super::fallback::forget_announcements();
    let mut warnings = 0;
    for round in 0..3 {
        assert!(wb.update(cx, |wb, cx| wb.prepare_quit(cx)));
        let ws = wb.read_with(cx, |wb, _| wb.persist.workspace_id).expect("a workspace");
        state::State::open(&home.join("state.db")).unwrap().set(&format!("editor_layout:{ws}"), &format!("broken {round}")).unwrap();
        wb = restart_after_kill(wb, cx, &home);
        warnings += wb.read_with(cx, |w, _| w.notice_history().iter().filter(|(_, m)| m.contains("editor layout could not be read")).count());
        let kept = state::State::open(&home.join("state.db")).unwrap().get(&super::fallback::corrupt_key(ws));
        assert_eq!(kept, Some(format!("broken {round}")), "round {round}: the newest unreadable layout is the one copy");
    }
    assert_eq!(warnings, 1, "told once for this workspace in this session");
}
