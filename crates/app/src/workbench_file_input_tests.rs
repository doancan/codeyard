use super::*;
use gpui::TestAppContext;

#[cfg(windows)]
#[gpui::test]
fn windows_native_terminal_file_paths_preserve_the_requested_line_and_column(cx: &mut TestAppContext) {
    for extended in [false, true] {
        let root = crate::qa_workbench_tests::fresh(&format!("native-file-link-{extended}"));
        let file = root.join("şablon.txt");
        std::fs::write(&file, "line one\nabcdef\nlast\n").unwrap();
        let path = if extended { file.to_string_lossy().into_owned() } else { crate::ui::local_path(&file).replace('/', "\\") };
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        wb.update_in(cx, |wb, window, cx| wb.open_link(crate::links::Link::File { path, line: Some(2), col: Some(4) }, window, cx));
        cx.run_until_parked();
        assert_eq!(wb.read_with(cx, |wb, cx| wb.active_path(cx)), Some(crate::ui::local_path(&file)));
        assert_eq!(wb.read_with(cx, |wb, cx| wb.active_cursor(cx)), Some((2, 4)), "native Windows paths must match the editor's reveal request");
    }
}

#[gpui::test]
fn pending_terminal_file_links_cannot_open_after_the_host_or_workspace_changes(cx: &mut TestAppContext) {
    check_pending_file_link(cx, true);
}

#[gpui::test]
fn pending_terminal_file_link_errors_cannot_replace_new_context_notices(cx: &mut TestAppContext) {
    check_pending_file_link(cx, false);
}

fn check_pending_file_link(cx: &mut TestAppContext, exists: bool) {
    for absolute in [false, true] {
        for change_host in [false, true] {
            let fixture = format!("pending-file-link-{exists}-{absolute}-{change_host}");
            let root = crate::qa_workbench_tests::fresh(&fixture);
            let other = crate::qa_workbench_tests::fresh(&format!("{fixture}-other"));
            let file = root.join("target.txt");
            if exists {
                std::fs::write(&file, "line one\nabcdef\nlast\n").unwrap();
            }
            let (wb, cx) = crate::qa_workbench_tests::setup(cx);
            wb.update_in(cx, |wb, window, cx| {
                wb.replace_root(crate::ui::local_path(&root), cx);
                wb.open_link(
                    crate::links::Link::File { path: if absolute { crate::ui::local_path(&file) } else { "target.txt".into() }, line: Some(2), col: Some(4) },
                    window,
                    cx,
                );
                if change_host {
                    wb.remote.host = Arc::new(host::local());
                } else {
                    wb.replace_root(crate::ui::local_path(&other), cx);
                }
                wb.notice = Some(Notice::Info("new context".into()));
            });
            cx.run_until_parked();
            assert!(wb.read_with(cx, |wb, _| wb.tabs.editors.is_empty()), "an old file lookup cannot open a location in the new context");
            assert!(
                wb.read_with(cx, |wb, _| matches!(&wb.notice, Some(Notice::Info(message)) if message.as_ref() == "new context")),
                "old file lookups cannot replace the new context's notice"
            );
        }
    }
}

#[gpui::test]
fn current_terminal_file_links_reveal_positions_and_report_missing_files(cx: &mut TestAppContext) {
    for absolute in [false, true] {
        let root = crate::qa_workbench_tests::fresh(&format!("current-file-link-{absolute}"));
        let file = root.join("target.txt");
        std::fs::write(&file, "line one\nabcdef\nlast\n").unwrap();
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        wb.update_in(cx, |wb, window, cx| {
            wb.replace_root(crate::ui::local_path(&root), cx);
            wb.open_link(
                crate::links::Link::File { path: if absolute { crate::ui::local_path(&file) } else { "target.txt".into() }, line: Some(2), col: Some(4) },
                window,
                cx,
            );
        });
        cx.run_until_parked();
        assert_eq!(wb.read_with(cx, |wb, cx| wb.active_path(cx)), Some(crate::ui::local_path(&file)));
        assert_eq!(wb.read_with(cx, |wb, cx| wb.active_cursor(cx)), Some((2, 4)));
        wb.update_in(cx, |wb, window, cx| wb.open_link(crate::links::Link::File { path: "missing.txt".into(), line: None, col: None }, window, cx));
        cx.run_until_parked();
        assert!(wb.read_with(cx, |wb, _| matches!(&wb.notice, Some(Notice::Info(message)) if message.contains("missing.txt"))));
        assert_eq!(wb.read_with(cx, |wb, _| wb.tabs.editors.len()), 1);
    }
}

#[gpui::test]
fn old_explorer_context_menus_cannot_operate_after_the_host_or_workspace_changes(cx: &mut TestAppContext) {
    check_explorer_menu(cx, true);
}

#[gpui::test]
fn current_explorer_context_menus_keep_copy_duplicate_and_rename_working(cx: &mut TestAppContext) {
    check_explorer_menu(cx, false);
}

fn check_explorer_menu(cx: &mut TestAppContext, stale: bool) {
    for (action, label) in ["Duplicate", "Rename…", "Copy"].into_iter().enumerate() {
        for change_host in [false, true].into_iter().take(if stale { 2 } else { 1 }) {
            let fixture = format!("explorer-menu-{stale}-{action}-{change_host}");
            let root = crate::qa_workbench_tests::fresh(&fixture);
            let other = crate::qa_workbench_tests::fresh(&format!("{fixture}-other"));
            let victim = root.join("victim.txt");
            std::fs::write(&victim, "original\n").unwrap();
            let (wb, cx) = crate::qa_workbench_tests::setup(cx);
            wb.update_in(cx, |wb, window, cx| {
                wb.replace_root(crate::ui::local_path(&root), cx);
                wb.open_context_menu(MenuTarget { path: crate::ui::local_path(&victim), is_dir: false, tab: None }, gpui::point(px(0.), px(0.)), window, cx);
            });
            wb.update_in(cx, |wb, _, cx| {
                if stale {
                    if change_host {
                        wb.remote.host = Arc::new(host::local());
                    } else {
                        wb.replace_root(crate::ui::local_path(&other), cx);
                    }
                    wb.notice = Some(Notice::Info("new context".into()));
                }
                wb.select_menu_for_test(label, cx);
            });
            cx.run_until_parked();
            assert!(wb.read_with(cx, |wb, _| wb.menu.is_none()));
            if stale {
                assert!(!root.join("victim copy.txt").exists(), "an old menu cannot duplicate through the new context");
                assert!(wb.read_with(cx, |wb, _| wb.overlay.is_none()), "an old menu cannot open a rename form in the new context");
                assert!(wb.read_with(cx, |wb, _| wb.roots.file_clip.is_none()), "an old menu cannot replace the new workspace's file clipboard");
                assert!(wb.read_with(cx, |wb, _| matches!(&wb.notice, Some(Notice::Info(message)) if message.as_ref() == "new context")));
                assert_eq!(std::fs::read_to_string(&victim).unwrap(), "original\n");
            } else {
                match action {
                    0 => assert_eq!(std::fs::read_to_string(root.join("victim copy.txt")).unwrap(), "original\n"),
                    1 => {
                        wb.update_in(cx, |wb, window, cx| {
                            wb.set_overlay_query_for_test("renamed.txt".into(), cx);
                            wb.on_overlay_event(&PickerEvent::Confirm(Pick::Input), window, cx);
                        });
                        cx.run_until_parked();
                        assert_eq!(std::fs::read_to_string(root.join("renamed.txt")).unwrap(), "original\n");
                        assert!(!victim.exists());
                    }
                    _ => assert_eq!(wb.read_with(cx, |wb, _| wb.roots.file_clip.as_ref().unwrap().paths.clone()), vec![crate::ui::local_path(&victim)]),
                }
            }
        }
    }
}

#[gpui::test]
fn an_old_permanent_delete_fallback_cannot_delete_after_its_context_changes(cx: &mut TestAppContext) {
    for directory in [false, true] {
        for change_host in [false, true] {
            let fixture = format!("stale-permanent-delete-{directory}-{change_host}");
            let root = crate::qa_workbench_tests::fresh(&fixture);
            let other = crate::qa_workbench_tests::fresh(&format!("{fixture}-other"));
            let victim = root.join("victim");
            if directory {
                std::fs::create_dir(&victim).unwrap();
                std::fs::write(victim.join("keep.txt"), "keep\n").unwrap();
            } else {
                std::fs::write(&victim, "keep\n").unwrap();
            }
            let (wb, cx) = crate::qa_workbench_tests::setup(cx);
            wb.update_in(cx, |wb, window, cx| {
                wb.replace_root(crate::ui::local_path(&root), cx);
                wb.confirm_permanent_delete(crate::ui::local_path(&victim), "fixture trash unavailable".into(), window, cx);
            });
            cx.run_until_parked();
            wb.update(cx, |wb, cx| {
                if change_host {
                    wb.remote.host = Arc::new(host::local());
                } else {
                    wb.replace_root(crate::ui::local_path(&other), cx);
                }
                wb.notice = Some(Notice::Info("new context".into()));
            });
            cx.simulate_prompt_answer("Delete Permanently");
            cx.run_until_parked();
            assert!(victim.exists(), "an old trash fallback must not delete through a new context");
            assert_eq!(std::fs::read_to_string(if directory { victim.join("keep.txt") } else { victim }).unwrap(), "keep\n");
            assert!(wb.read_with(cx, |wb, _| matches!(&wb.notice, Some(Notice::Info(message)) if message.as_ref() == "new context")));
        }
    }
}

#[gpui::test]
fn a_current_permanent_delete_fallback_obeys_confirmation_and_cancellation(cx: &mut TestAppContext) {
    for cancel in [false, true] {
        let root = crate::qa_workbench_tests::fresh(&format!("current-permanent-delete-{cancel}"));
        let victim = root.join("victim.txt");
        std::fs::write(&victim, "keep\n").unwrap();
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        wb.update_in(cx, |wb, window, cx| {
            wb.replace_root(crate::ui::local_path(&root), cx);
            wb.confirm_permanent_delete(crate::ui::local_path(&victim), "fixture trash unavailable".into(), window, cx);
        });
        cx.run_until_parked();
        cx.simulate_prompt_answer(if cancel { "Cancel" } else { "Delete Permanently" });
        cx.run_until_parked();
        assert_eq!(victim.exists(), cancel);
        if cancel {
            assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep\n");
        }
    }
}

#[gpui::test]
fn late_save_as_results_cannot_change_a_new_host_or_workspace(cx: &mut TestAppContext) {
    check_late_save_results(cx, false);
}

#[gpui::test]
fn late_save_local_copy_results_cannot_change_a_new_host_or_workspace(cx: &mut TestAppContext) {
    check_late_save_results(cx, true);
}

fn check_late_save_results(cx: &mut TestAppContext, local_copy: bool) {
    for change_host in [false, true] {
        for fail in [false, true] {
            let fixture = format!("late-save-{local_copy}-{change_host}-{fail}");
            let root = crate::qa_workbench_tests::fresh(&fixture);
            let other = crate::qa_workbench_tests::fresh(&format!("{fixture}-other"));
            let from = crate::ui::local_path(&root.join("source.txt"));
            let target = root.join("copy.txt");
            std::fs::write(root.join("source.txt"), "original document\n").unwrap();
            if fail {
                std::fs::write(&target, "existing destination\n").unwrap();
            }
            let (wb, cx) = crate::qa_workbench_tests::setup(cx);
            wb.update_in(cx, |wb, window, cx| {
                wb.replace_root(crate::ui::local_path(&root), cx);
                wb.open_file(from.clone(), window, cx);
            });
            cx.run_until_parked();
            wb.update_in(cx, |wb, window, cx| {
                if local_copy {
                    wb.save_local_copy_to(from.clone(), crate::ui::local_path(&target), cx);
                } else {
                    wb.save_as_to(from.clone(), "copy.txt".into(), window, cx);
                }
                if change_host {
                    wb.remote.host = Arc::new(host::local());
                } else {
                    wb.replace_root(crate::ui::local_path(&other), cx);
                }
                wb.notice = Some(Notice::Info("new context".into()));
            });
            cx.run_until_parked();
            assert_eq!(
                std::fs::read_to_string(&target).unwrap(),
                if fail { "existing destination\n" } else { "original document\n" },
                "an already started save still finishes on its original destination"
            );
            assert!(
                wb.read_with(cx, |wb, cx| !wb.tabs.editors.iter().any(|editor| editor.read(cx).path == crate::ui::local_path(&target))),
                "an old save must not open its target on the new connection"
            );
            assert!(
                wb.read_with(cx, |wb, _| matches!(&wb.notice, Some(Notice::Info(message)) if message.as_ref() == "new context")),
                "old save results must not replace the new context's notice"
            );
            if change_host {
                assert!(
                    wb.read_with(cx, |wb, cx| wb.tabs.editors.iter().any(|editor| editor.read(cx).path == from)),
                    "old save callbacks cannot close tabs in the new context"
                );
            }
        }
    }
}

fn purpose(operation: u8, root: &std::path::Path) -> InputPurpose {
    let dir = crate::ui::local_path(root);
    match operation {
        0 | 1 => InputPurpose::Create { dir, is_dir: operation == 1 },
        2 => InputPurpose::NewFile { base: dir },
        _ => InputPurpose::Rename { path: crate::ui::local_path(&root.join("old.txt")) },
    }
}

#[gpui::test]
fn save_name_forms_cannot_use_a_different_connection(cx: &mut TestAppContext) {
    check_save_forms(cx, true);
}

#[gpui::test]
fn current_save_name_forms_still_write_the_original_document(cx: &mut TestAppContext) {
    check_save_forms(cx, false);
}

fn check_save_forms(cx: &mut TestAppContext, change_host: bool) {
    for local_copy in [false, true] {
        let root = crate::qa_workbench_tests::fresh(&format!("save-input-{local_copy}-{change_host}"));
        let from = crate::ui::local_path(&root.join("source.txt"));
        std::fs::write(root.join("source.txt"), "original document\n").unwrap();
        let target = root.join("copy.txt");
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        wb.update_in(cx, |wb, window, cx| {
            wb.replace_root(crate::ui::local_path(&root), cx);
            wb.open_file(from.clone(), window, cx);
        });
        cx.run_until_parked();
        assert!(wb.read_with(cx, |wb, cx| wb.tabs.editors.iter().any(|editor| editor.read(cx).path == from)));
        wb.update_in(cx, |wb, window, cx| {
            let purpose = if local_copy { InputPurpose::SaveLocalCopy { from: from.clone() } } else { InputPurpose::SaveAs { from: from.clone() } };
            wb.show_input(purpose, "Save", "File name", "", window, cx);
            wb.set_overlay_query_for_test(if local_copy { crate::ui::local_path(&target) } else { "copy.txt".into() }, cx);
            if change_host {
                wb.remote.host = Arc::new(host::local());
            }
            wb.on_overlay_event(&PickerEvent::Confirm(Pick::Input), window, cx);
        });
        cx.run_until_parked();
        if change_host {
            assert!(!target.exists(), "an old save form must not write after its connection changes (local copy {local_copy})");
        } else {
            assert_eq!(std::fs::read_to_string(&target).unwrap(), "original document\n");
        }
        assert_eq!(std::fs::read_to_string(root.join("source.txt")).unwrap(), "original document\n");
    }
}

#[gpui::test]
fn old_file_name_forms_cannot_mutate_after_the_host_or_workspace_changes(cx: &mut TestAppContext) {
    for change_host in [false, true] {
        for operation in 0..4 {
            let root = crate::qa_workbench_tests::fresh(&format!("stale-file-input-{change_host}-{operation}"));
            let other = crate::qa_workbench_tests::fresh(&format!("stale-file-input-other-{change_host}-{operation}"));
            std::fs::write(root.join("old.txt"), "keep this file\n").unwrap();
            let (wb, cx) = crate::qa_workbench_tests::setup(cx);
            wb.update(cx, |wb, cx| wb.replace_root(crate::ui::local_path(&root), cx));
            cx.run_until_parked();
            wb.update_in(cx, |wb, window, cx| {
                wb.show_input(purpose(operation, &root), "Name", "File name", "", window, cx);
                wb.set_overlay_query_for_test("created.txt".into(), cx);
                if change_host {
                    wb.remote.host = Arc::new(host::local());
                } else {
                    wb.replace_root(crate::ui::local_path(&other), cx);
                }
                wb.on_overlay_event(&PickerEvent::Confirm(Pick::Input), window, cx);
            });
            cx.run_until_parked();
            assert!(
                !root.join("created.txt").exists(),
                "the old form must not mutate its previous directory (operation {operation}, host change {change_host})"
            );
            assert!(!other.join("created.txt").exists());
            assert_eq!(std::fs::read_to_string(root.join("old.txt")).unwrap(), "keep this file\n");
            assert!(wb.read_with(cx, |wb, _| wb.overlay.is_none()));
        }
    }
}

#[gpui::test]
fn current_file_name_forms_work_without_a_git_repository(cx: &mut TestAppContext) {
    for operation in 0..4 {
        let root = crate::qa_workbench_tests::fresh(&format!("current-file-input-{operation}"));
        std::fs::write(root.join("old.txt"), "keep this file\n").unwrap();
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        wb.update(cx, |wb, cx| wb.replace_root(crate::ui::local_path(&root), cx));
        cx.run_until_parked();
        wb.update_in(cx, |wb, window, cx| {
            wb.show_input(purpose(operation, &root), "Name", "File name", "", window, cx);
            wb.set_overlay_query_for_test("created.txt".into(), cx);
            wb.on_overlay_event(&PickerEvent::Confirm(Pick::Input), window, cx);
        });
        cx.run_until_parked();
        assert!(root.join("created.txt").exists());
        if operation == 1 {
            assert!(root.join("created.txt").is_dir());
        } else {
            assert!(root.join("created.txt").is_file());
        }
        if operation == 3 {
            assert!(!root.join("old.txt").exists());
            assert_eq!(std::fs::read_to_string(root.join("created.txt")).unwrap(), "keep this file\n");
        }
    }
}
