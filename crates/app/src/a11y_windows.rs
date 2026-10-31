//! Accessibility trees of the workbench's other windows: an editor's own window (window title, the editor as a text
//! field with its caret and selection, its find bar and suggestions, and the picker opened there) and a terminal's own
//! window (the terminal's text and cursor). They use the same node ids and tree builder as the main window, so a
//! screen reader finds the editor and the terminal where it does there. A side view's own window holds that view's
//! part of the main tree (Explorer, Source Control, Search, Extensions, Run and Debug, Testing, Remote Explorer), which
//! the main window's tree then leaves out. Actions go to the workbench with that window, after making it the one
//! commands act on.

use super::*;
use crate::workbench::editor_windows_glue::EditorWindow;
use crate::workbench::view_windows_glue::{ViewRef, ViewWindow};
use crate::workbench::views_glue::DetachedTerminal;

/// What a window's tree is made of: how its snapshot is taken and how its actions are carried out.
#[derive(Clone, Copy)]
pub(in crate::workbench) struct Hooks {
    pub(in crate::workbench) snapshot: fn(gpui::AnyView, &mut Window, &mut App) -> Option<Snapshot>,
    pub(in crate::workbench) action: fn(gpui::AnyView, &ActionRequest, &Snapshot, &mut Window, &mut App),
}

/// The main window (without the views drawn in windows of their own).
pub(in crate::workbench) const WORKBENCH: Hooks = Hooks {
    snapshot: |root, window, cx| {
        let wb = root.downcast::<Workbench>().ok()?;
        let wb = wb.read(cx);
        let mut snap = wb.a11y_snapshot(window, cx);
        for entry in &wb.chrome.view_windows {
            take_view(&mut snap, &entry.view);
        }
        Some(snap)
    },
    action: |root, req, last, window, cx| {
        if let Ok(wb) = root.downcast::<Workbench>() {
            wb.update(cx, |wb, cx| wb.a11y_action(req, last, window, cx));
        }
    },
};

/// An editor's own window.
pub(in crate::workbench) const EDITOR_WINDOW: Hooks = Hooks {
    snapshot: |root, window, cx| Some(editor_window_snapshot(&root.downcast::<EditorWindow>().ok()?, window, cx)),
    action: |root, req, last, window, cx| {
        let Ok(root) = root.downcast::<EditorWindow>() else { return };
        let (_, slot, home) = root.read(cx).parts();
        if let Some(wb) = home.upgrade() {
            wb.update(cx, |wb, cx| {
                wb.use_editor_window(slot, cx);
                wb.a11y_action(req, last, window, cx);
            });
        }
    },
};

/// A side view's own window.
pub(in crate::workbench) const VIEW_WINDOW: Hooks = Hooks {
    snapshot: |root, window, cx| Some(view_window_snapshot(&root.downcast::<ViewWindow>().ok()?, window, cx)),
    action: |root, req, last, window, cx| {
        let Ok(root) = root.downcast::<ViewWindow>() else { return };
        let (_, home) = root.read(cx).parts();
        if let Some(wb) = home.upgrade() {
            wb.update(cx, |wb, cx| {
                wb.use_view_window(window.window_handle(), cx);
                wb.a11y_action(req, last, window, cx);
            });
        }
    },
};

/// Moves the part of `snap` that belongs to `view` into a snapshot of its own (Run and Debug, Testing and Remote
/// Explorer share one part, which stays in both).
fn take_view(snap: &mut Snapshot, view: &ViewRef) -> Snapshot {
    let mut part = Snapshot::default();
    match view.panel {
        Panel::Explorer => {
            part.explorers = std::mem::take(&mut snap.explorers);
            if matches!(snap.focus, FocusSnap::Explorer(_)) {
                part.focus = std::mem::replace(&mut snap.focus, FocusSnap::Window);
            }
        }
        Panel::Scm => {
            part.scm = snap.scm.take();
            if matches!(snap.focus, FocusSnap::ScmMessage | FocusSnap::ScmRepos) {
                part.focus = std::mem::replace(&mut snap.focus, FocusSnap::Window);
            }
        }
        Panel::Search => part.views.search = snap.views.search.take(),
        Panel::Extensions => {
            part.exts = snap.exts.take();
            if matches!(snap.focus, FocusSnap::ExtSearch) {
                part.focus = std::mem::replace(&mut snap.focus, FocusSnap::Window);
            }
        }
        Panel::Debug | Panel::Testing | Panel::Remote => {
            part.views.side = snap.views.side.clone();
            part.views.row_focus = snap.views.row_focus;
        }
        Panel::ExtView => {
            let (mine, rest) = std::mem::take(&mut snap.ext_views).into_iter().partition(|v| Some(&v.id) == view.ext.as_ref());
            part.ext_views = mine;
            snap.ext_views = rest;
        }
        Panel::Containers => {}
    }
    part
}

/// The view in its own window: its part of the workbench's tree, taken with this window (focus and row frames are
/// this window's), the window's name, and the picker opened there.
pub(super) fn view_window_snapshot(root: &Entity<ViewWindow>, window: &Window, cx: &mut App) -> Snapshot {
    let (view, home) = root.read(cx).parts();
    let Some(wb) = home.upgrade() else { return Snapshot::default() };
    let wb = wb.read(cx);
    let mut full = wb.a11y_snapshot(window, cx);
    let mut snap = take_view(&mut full, &view);
    snap.generation = full.generation;
    snap.geometry = Geometry::whole_window(window);
    snap.title = wb.view_window_title(&view);
    if wb.chrome.view_host == Some(window.window_handle()) {
        snap.picker = full.picker.take();
        if full.focus == FocusSnap::Picker {
            snap.focus = FocusSnap::Picker;
        }
    }
    snap
}

/// The node of element `i` of the extension view at `slot` (tests).
#[cfg(test)]
pub(in crate::workbench) fn ext_view_node(slot: usize, i: usize) -> NodeId {
    ext_views_access::ViewSnap::node_id(slot, i)
}

/// A terminal's own window.
pub(in crate::workbench) const TERMINAL_WINDOW: Hooks = Hooks {
    snapshot: |root, window, cx| Some(terminal_window_snapshot(&root.downcast::<DetachedTerminal>().ok()?, window, cx)),
    action: |root, req, _, window, cx| {
        let Ok(root) = root.downcast::<DetachedTerminal>() else { return };
        if matches!((req.action, req.target_node), (Action::Focus, TERMINAL) | (Action::Focus, ROOT)) {
            let focus = root.read(cx).term().read(cx).focus_handle(cx);
            window.focus(&focus);
        }
    },
};

/// The editor's view in that window: text around the caret, caret and selection, find bar, suggestions, and the picker
/// when it was opened there.
pub(super) fn editor_window_snapshot(root: &Entity<EditorWindow>, window: &Window, cx: &mut App) -> Snapshot {
    let (ed, slot, home) = root.read(cx).parts();
    let (editor, find_bar, completion) = ed.update(cx, |e, _| {
        e.in_slot(slot, |e| {
            let editor = EditorSnap::of(e);
            let completion = e.a11y_completion(MAX_OPTIONS).map(|(rows, selected)| ListSnap { rows_total: rows.len(), selected: Some(selected), rows });
            (editor, e.find_bar(), completion)
        })
    });
    let find = find_bar.as_ref().map(|b| {
        let b = b.read(cx);
        FindSnap {
            query: b.query(cx),
            replacement: b.show_replace.then(|| b.replacement(cx)),
            status: b.status_text().map(str::to_string),
            controls: b.a11y_controls().into_iter().map(|(l, on)| (l.to_string(), on)).collect(),
        }
    });
    // The picker belongs to this window while it is the one in use.
    let picker_entity = home.upgrade().and_then(|wb| {
        let wb = wb.read(cx);
        (wb.chrome.editor_host == Some(slot)).then(|| wb.overlay.as_ref().map(|o| o.picker.clone())).flatten()
    });
    let picker = picker_entity.as_ref().map(|p| {
        let v = p.read(cx).a11y_view(MAX_OPTIONS);
        PickerSnap {
            label: v.placeholder,
            message: v.message,
            query: v.query,
            list: ListSnap { rows: v.rows, rows_total: v.rows_total, selected: v.selected },
            empty: v.empty,
        }
    });
    let focused = ed.read(cx).slot_focus(slot).is_some_and(|f| f.is_focused(window));
    let focus = if picker_entity.as_ref().is_some_and(|p| p.focus_handle(cx).is_focused(window)) {
        FocusSnap::Picker
    } else if let Some(field) = find_bar.as_ref().and_then(|b| b.read(cx).a11y_focus(window, cx)) {
        FocusSnap::Find(field)
    } else if focused {
        FocusSnap::Editor
    } else {
        FocusSnap::Window
    };
    let (line, col) = ed.update(cx, |e, _| e.in_slot(slot, |e| e.cursor_position()));
    Snapshot {
        // Actions are checked against the workbench's generation, as in the main window.
        generation: home.upgrade().map_or(0, |wb| wb.read(cx).lsp.generation),
        geometry: Geometry::whole_window(window),
        title: format!("{}{}", if editor.dirty { "● " } else { "" }, editor.name),
        status: vec![tf("Ln {}, Col {}", &[&line, &col])],
        editor: Some(editor),
        find,
        completion,
        picker,
        focus,
        ..Default::default()
    }
}

/// The terminal in its own window: its text, cursor and name.
pub(super) fn terminal_window_snapshot(root: &Entity<DetachedTerminal>, window: &Window, cx: &mut App) -> Snapshot {
    let term = root.read(cx).term();
    let focused = term.read(cx).focus_handle(cx).contains_focused(window, cx);
    Snapshot {
        geometry: Geometry::whole_window(window),
        title: term.read(cx).title.to_string(),
        terminal: Some(term_tab_access::terminal_snap(&term, 0, 1, cx)),
        focus: if focused { FocusSnap::Terminal } else { FocusSnap::Window },
        ..Default::default()
    }
}

#[cfg(test)]
#[path = "a11y_windows_tests.rs"]
mod tests;
