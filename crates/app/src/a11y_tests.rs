//! The tree builder: trees AccessKit accepts, the selection mapping and long lines.

use super::*;

fn snap() -> Snapshot {
    let lines = vec![(0, "fn main() {".to_string()), (12, "    println!(\"hé\");".to_string()), (31, "}".to_string())];
    let runs = runs_for(lines, 3, 0);
    Snapshot {
        generation: 0,
        services: None,
        exts: None,
        ext_page: None,
        inspector: None,
        ext_views: Vec::new(),
        geometry: Geometry::default(),
        title: "Codeyard \u{2014} demo".into(),
        layout: String::new(),
        explorers: vec![ExplorerSnap {
            name: "d".into(),
            rows: vec![
                RowSnap { id: row_id("/d/src").0, path: "/d/src".into(), name: "src".into(), depth: 0, is_dir: true, expanded: true, selected: false },
                RowSnap {
                    id: row_id("/d/src/main.rs").0,
                    path: "/d/src/main.rs".into(),
                    name: "main.rs".into(),
                    depth: 1,
                    is_dir: false,
                    expanded: false,
                    selected: true,
                },
            ],
            rows_total: 2,
            first: 0,
            focused: Some(row_id("/d/src/main.rs").0),
        }],
        tabs: vec![TabSnap { name: "lib.rs".into(), dirty: true }, TabSnap { name: "main.rs".into(), dirty: false }],
        active_tab: Some(1),
        about: None,
        notice: None,
        editor: Some(EditorSnap { name: "main.rs".into(), dirty: false, runs, selection: (3, 7), total_lines: 3, first_line: 0 }),
        find: None,
        completion: None,
        terminal: None,
        scm: None,
        views: Default::default(),
        search_editor: None,
        search_tabs: Vec::new(),
        term_tabs: Vec::new(),
        picker: None,
        status: vec!["Local".into(), "Ln 1, Col 4 (4 selected)".into()],
        dialog: None,
        focus: FocusSnap::Editor,
    }
}

/// AccessKit's own consumer accepts the tree (ids, parent-child, text selection valid).
#[test]
fn tree_is_valid_for_accesskit_consumer() {
    let s = snap();
    let update = s.to_tree();
    // Tree::new panics: no root, missing child node, focus not in the tree, etc.
    let tree = accesskit_consumer::Tree::new(update, true);
    let state = tree.state();
    assert_eq!(state.root().role(), Role::Window);
    assert_eq!(state.focus().unwrap().role(), Role::MultilineTextInput);
    let focus = state.focus().unwrap();
    let sel = focus.text_selection().unwrap();
    assert_eq!(sel.start().to_global_utf16_index(), 3);
    assert_eq!(sel.end().to_global_utf16_index(), 7);
    assert_eq!(sel.text(), "main");
}

#[test]
fn about_tree_has_a_valid_focused_copy_button() {
    let mut snapshot = snap();
    snapshot.editor = None;
    snapshot.active_tab = None;
    snapshot.about = Some(AboutSnap { active: true, details: vec![("Version".into(), "0.5.0".into())] });
    snapshot.focus = FocusSnap::AboutCopy;
    let tree = accesskit_consumer::Tree::new(snapshot.to_tree(), true);
    let focused = tree.state().focus().unwrap();
    assert_eq!(focused.role(), Role::Button);
    assert_eq!(focused.label().as_deref(), Some("Copy Version"));
}

#[test]
fn selection_round_trips_through_runs() {
    let s = snap();
    let runs = &s.editor.as_ref().unwrap().runs;
    for ix in [0, 3, 11, 12, 20, 31, 32] {
        let p = position_of(runs, RUN_BASE, ix).unwrap();
        assert_eq!(char_of(runs, p), Some(ix), "ix {ix}");
    }
}

#[test]
fn long_lines_split_and_words_fit_u8() {
    let line = "abc ".repeat(200); // 800 characters
    let runs = runs_for(vec![(0, line)], 2, 0);
    assert!(runs.len() >= 4);
    assert!(runs.iter().all(|r| r.text.chars().count() <= RUN_CHARS + 1));
    assert!(runs.last().unwrap().text.ends_with('\n'));
    assert_eq!(runs.iter().map(|r| r.chars).sum::<usize>(), 801);
}

#[test]
fn tabs_are_a_tab_list_with_the_front_tab_selected() {
    let tree = accesskit_consumer::Tree::new(snap().to_tree(), true);
    let state = tree.state();
    let list = state.root().children().find(|n| n.role() == Role::TabList).unwrap();
    assert_eq!(list.role(), Role::TabList);
    let tabs: Vec<_> = list.children().collect();
    assert_eq!(tabs.len(), 2);
    assert!(tabs.iter().all(|t| t.role() == Role::Tab));
    assert_eq!(tabs[0].label().as_deref(), Some("lib.rs (modified)"));
    assert_eq!(tabs.iter().map(|t| t.is_selected()).collect::<Vec<_>>(), [Some(false), Some(true)]);
}

#[test]
fn notices_are_live_regions_and_errors_interrupt() {
    let mut s = snap();
    assert!(s.to_tree().nodes.iter().all(|(id, _)| *id != NOTICE));
    s.notice = Some(NoticeSnap { text: "Saved".into(), error: false });
    let tree = accesskit_consumer::Tree::new(s.to_tree(), true);
    let n = tree.state().root().children().last().unwrap();
    assert_eq!((n.role(), n.live(), n.label().as_deref()), (Role::Status, Live::Polite, Some("Saved")));
    s.notice = Some(NoticeSnap { text: "Could not save".into(), error: true });
    let tree = accesskit_consumer::Tree::new(s.to_tree(), true);
    let n = tree.state().root().children().last().unwrap();
    assert_eq!((n.role(), n.live()), (Role::Alert, Live::Assertive));
}

fn list(rows: &[&str], selected: usize) -> ListSnap {
    ListSnap { rows: rows.iter().map(|r| (r.to_string(), None)).collect(), rows_total: rows.len(), selected: Some(selected) }
}

#[test]
fn picker_is_a_combo_box_whose_active_descendant_is_the_selected_row() {
    let mut s = snap();
    s.picker =
        Some(PickerSnap {
            label: "Type a command".into(), message: None, query: ">sav".into(), list: list(&["File: Save", "File: Save As…"], 1), empty: None
        });
    s.focus = FocusSnap::Picker;
    let tree = accesskit_consumer::Tree::new(s.to_tree(), true);
    let state = tree.state();
    // The focus lands on the selected option through the combo box's active descendant.
    let focus = state.focus().unwrap();
    assert_eq!((focus.role(), focus.label().as_deref(), focus.is_selected()), (Role::ListBoxOption, Some("File: Save As…"), Some(true)));
    let dialog = state.root().children().find(|n| n.role() == Role::Dialog).unwrap();
    let input = dialog.children().next().unwrap();
    assert_eq!((input.role(), input.value().as_deref()), (Role::EditableComboBox, Some(">sav")));
}

#[test]
fn find_bar_has_fields_live_status_and_option_states() {
    let mut s = snap();
    s.find = Some(FindSnap {
        query: "main".into(),
        replacement: Some("start".into()),
        status: Some("1 of 1".into()),
        controls: vec![("Match case (⌥C)".into(), Some(true)), ("Next match (↵)".into(), None)],
    });
    s.focus = FocusSnap::Find(1);
    let tree = accesskit_consumer::Tree::new(s.to_tree(), true);
    let state = tree.state();
    let focus = state.focus().unwrap();
    assert_eq!((focus.role(), focus.value().as_deref()), (Role::TextInput, Some("start")));
    let bar = state.root().children().find(|n| n.role() == Role::Group).unwrap();
    let kids: Vec<_> = bar.children().collect();
    assert_eq!(kids.iter().map(|k| k.role()).collect::<Vec<_>>(), [Role::SearchInput, Role::TextInput, Role::Status, Role::CheckBox, Role::Button]);
    assert_eq!((kids[2].live(), kids[2].label().as_deref()), (Live::Polite, Some("1 of 1")));
    assert_eq!(kids[3].toggled(), Some(Toggled::True));
}

#[test]
fn selected_suggestion_is_the_editors_active_descendant() {
    let mut s = snap();
    s.completion = Some(list(&["println!", "print!"], 0));
    let tree = accesskit_consumer::Tree::new(s.to_tree(), true);
    let focus = tree.state().focus().unwrap();
    assert_eq!((focus.role(), focus.label().as_deref()), (Role::ListBoxOption, Some("println!")));
    assert_eq!(focus.parent().unwrap().role(), Role::ListBox);
}

#[test]
fn terminal_rows_are_text_with_the_cursor_as_caret() {
    let mut s = snap();
    let rows = ["$ ls".to_string(), "a  b".to_string(), "$ ".to_string()];
    s.terminal = Some(TerminalSnap {
        label: "Terminal 1 of 1".into(),
        runs: runs_for(vec![(0, rows[0].clone()), (5, rows[1].clone()), (10, rows[2].clone())], 3, 0),
        cursor: Some(12),
    });
    s.focus = FocusSnap::Terminal;
    let tree = accesskit_consumer::Tree::new(s.to_tree(), true);
    let focus = tree.state().focus().unwrap();
    assert_eq!(focus.role(), Role::Terminal);
    assert!(focus.is_read_only());
    let sel = focus.text_selection().unwrap();
    assert!(sel.is_degenerate());
    assert_eq!(sel.start().to_global_utf16_index(), 12);
}

#[test]
fn every_folder_of_the_workspace_has_its_own_tree() {
    let mut s = snap();
    let row = RowSnap { id: row_id("/e/x.rs").0, path: "/e/x.rs".into(), name: "x.rs".into(), depth: 0, is_dir: false, expanded: false, selected: true };
    s.explorers.push(ExplorerSnap { name: "e".into(), focused: Some(row.id), rows: vec![row], rows_total: 1, first: 0 });
    s.focus = FocusSnap::Explorer(1);
    let tree = accesskit_consumer::Tree::new(s.to_tree(), true);
    let state = tree.state();
    let trees: Vec<_> = state.root().children().filter(|n| n.role() == Role::Tree).collect();
    assert_eq!(trees.len(), 2);
    assert_eq!(trees[1].label().as_deref(), Some("Explorer: e"));
    assert_eq!(state.focus().unwrap().label().as_deref(), Some("x.rs"));
}

#[test]
fn the_range_centres_on_the_focus_while_it_is_on_screen() {
    assert_eq!(window_start(10_000, 1_000, Some(5_000), Some((4_990, 5_010))), 4_500);
    assert_eq!(window_start(10_000, 1_000, Some(5_000), Some((100, 120))), 0, "scrolled away: the visible rows");
    assert_eq!(window_start(10_000, 1_000, Some(5_000), Some((8_000, 8_020))), 7_510);
    assert_eq!(window_start(10_000, 1_000, Some(9_990), None), 9_000, "clamped at the end");
    assert_eq!(window_start(50, 1_000, Some(40), None), 0, "everything fits");
}
