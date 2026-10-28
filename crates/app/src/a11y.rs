//! Accessibility tree (spike): exposes the window, explorer rows, the active
//! editor (text field + cursor/selection) and the status bar text to VoiceOver through AccessKit.
//!
//! Submodule of `workbench` (`#[path]`), so it can read Workbench's private fields.
//! Design (details: docs/accessibility.md):
//!  - The adapter (`accesskit_macos::SubclassingAdapter`) dynamically subclasses GPUIView;
//!    no GPUI patch needed (`Window: HasWindowHandle` yields the NSView).
//!  - The tree is lazy: `ActivationHandler` is called when an assistive technology (VoiceOver, Accessibility Inspector, AXUIElement)
//!    first queries the window; until then there is no task,
//!    timer or tree. Once active, a 250 ms loop takes a snapshot and
//!    sends a `TreeUpdate` only if it changed.
//!  - Actions (focus, click, text selection) arrive on the main thread; they are moved into the GPUI context
//!    with `AsyncApp`.
use super::*;
use crate::i18n::{t, tf, tn};
use accesskit::{Action, ActionData, ActionRequest, Node, NodeId, Role, TextPosition, TextSelection, TreeId, TreeInfo, TreeUpdate};

#[path = "a11y_geometry.rs"]
mod geometry;
pub use geometry::Geometry;
pub(super) use geometry::{Landmark, PaintedBounds, landmark};

const ROOT: NodeId = NodeId(1);
const EXPLORER: NodeId = NodeId(2);
const EDITOR: NodeId = NodeId(3);
const STATUS: NodeId = NodeId(4);
const UPDATE_DIALOG: NodeId = NodeId(5);
const UPDATE_PROGRESS: NodeId = NodeId(6);
const STATUS_ITEM: u64 = 0x10_0000;
const UPDATE_BUTTON: u64 = 0x20_0000;
const ROW_BASE: u64 = 0x2_0000_0000;
const RUN_BASE: u64 = 0x4_0000_0000;
/// Window shown to AT in the editor: this many lines before/after the cursor line.
const WINDOW_BEFORE: usize = 100;
const WINDOW_AFTER: usize = 200;
/// Characters per text run (word_starts is u8, so at most 255).
const RUN_CHARS: usize = 200;
/// Max rows shown to AT in the explorer.
const MAX_ROWS: usize = 5000;

fn row_id(path: &str) -> NodeId {
    // FNV-1a: ids of other rows must not change when a file is added or removed.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in path.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    NodeId(ROW_BASE | (h & 0xffff_ffff))
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct RowSnap {
    pub id: u64,
    pub path: String,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
    pub selected: bool,
}

/// A text run: a slice of a single line (at most RUN_CHARS characters).
#[derive(Clone, PartialEq, Debug)]
pub struct RunSnap {
    /// Index of the first character in the document.
    pub start: usize,
    /// Character count including the line ending.
    pub chars: usize,
    pub text: String,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct EditorSnap {
    pub name: String,
    pub dirty: bool,
    pub runs: Vec<RunSnap>,
    /// (anchor, cursor) character indices.
    pub selection: (usize, usize),
    pub total_lines: usize,
    pub first_line: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FocusSnap {
    #[default]
    Window,
    Explorer,
    Editor,
}

/// The update dialog (or the first-run question) while it is open.
#[derive(Clone, PartialEq, Debug)]
pub struct DialogSnap {
    pub title: String,
    pub description: String,
    /// `Some(None)`: working, no end in sight; `Some(Some(f))`: `f` of 1 done.
    pub progress: Option<Option<f64>>,
    /// Button labels in tab order, and the focused one.
    pub buttons: Vec<String>,
    pub focused: Option<usize>,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct Snapshot {
    /// Physical landmark frames from the latest painted layout.
    pub geometry: Geometry,
    pub title: String,
    pub rows: Vec<RowSnap>,
    pub rows_total: usize,
    pub editor: Option<EditorSnap>,
    pub status: Vec<String>,
    pub dialog: Option<DialogSnap>,
    pub focus: FocusSnap,
}

/// Split into runs: each line into runs of at most RUN_CHARS characters, the line ending in the last run.
fn runs_for(lines: Vec<(usize, String)>, total_lines: usize, first_line: usize) -> Vec<RunSnap> {
    let mut runs = Vec::new();
    for (i, (start, line)) in lines.into_iter().enumerate() {
        let last_in_doc = first_line + i + 1 >= total_lines;
        let chars: Vec<char> = line.chars().collect();
        let n = chars.len();
        let mut off = 0;
        loop {
            let end = (off + RUN_CHARS).min(n);
            let mut text: String = chars[off..end].iter().collect();
            let mut count = end - off;
            if end == n && !last_in_doc {
                text.push('\n');
                count += 1;
            }
            runs.push(RunSnap { start: start + off, chars: count, text });
            if end == n {
                break;
            }
            off = end;
        }
    }
    runs
}

fn word_starts(text: &str) -> Vec<u8> {
    #[derive(PartialEq, Clone, Copy)]
    enum K {
        Word,
        Space,
        Punct,
    }
    let kind = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            K::Word
        } else if c.is_whitespace() {
            K::Space
        } else {
            K::Punct
        }
    };
    let mut out = Vec::new();
    let mut prev: Option<K> = None;
    for (i, c) in text.chars().enumerate() {
        let k = kind(c);
        // Each punctuation character is its own word; words and whitespace as clusters.
        if (prev != Some(k) || k == K::Punct) && i > 0 && i < 256 {
            out.push(i as u8);
        }
        prev = Some(k);
    }
    out
}

/// Convert a character index to a (run, char index within run) position; clamp to the edge if outside the window.
fn position_of(runs: &[RunSnap], ix: usize) -> Option<TextPosition> {
    let first = runs.first()?;
    if ix <= first.start {
        return Some(TextPosition { node: NodeId(RUN_BASE), character_index: 0 });
    }
    for (i, r) in runs.iter().enumerate() {
        if ix < r.start + r.chars {
            return Some(TextPosition { node: NodeId(RUN_BASE + i as u64), character_index: ix - r.start });
        }
    }
    let i = runs.len() - 1;
    Some(TextPosition { node: NodeId(RUN_BASE + i as u64), character_index: runs[i].text.chars().count() })
}

fn char_of(runs: &[RunSnap], p: TextPosition) -> Option<usize> {
    let i = p.node.0.checked_sub(RUN_BASE)? as usize;
    runs.get(i).map(|r| r.start + p.character_index.min(r.chars))
}

impl Snapshot {
    pub fn to_tree(&self) -> TreeUpdate {
        let mut nodes: Vec<(NodeId, Node)> = Vec::new();
        let mut root = Node::new(Role::Window);
        self.geometry.apply(ROOT, &mut root);
        root.set_label(self.title.clone());
        let mut kids = Vec::new();

        // Explorer
        let mut tree = Node::new(Role::Tree);
        self.geometry.apply(EXPLORER, &mut tree);
        tree.set_label(t("Explorer"));
        tree.set_size_of_set(self.rows_total);
        let mut row_ids = Vec::new();
        for (i, r) in self.rows.iter().enumerate() {
            let mut n = Node::new(Role::TreeItem);
            n.set_label(r.name.clone());
            n.set_level(r.depth + 1);
            n.set_selected(r.selected);
            n.set_position_in_set(i + 1);
            n.set_size_of_set(self.rows_total);
            if r.is_dir {
                n.set_expanded(r.expanded);
                n.set_description(t("folder"));
            }
            n.add_action(Action::Click);
            n.add_action(Action::Focus);
            row_ids.push(NodeId(r.id));
            nodes.push((NodeId(r.id), n));
        }
        tree.set_children(row_ids);
        tree.add_action(Action::Focus);
        nodes.push((EXPLORER, tree));
        kids.push(EXPLORER);

        // Editor
        if let Some(e) = &self.editor {
            let mut ed = Node::new(Role::MultilineTextInput);
            self.geometry.apply(EDITOR, &mut ed);
            ed.set_label(if e.dirty { tf("{} (modified)", &[&e.name]) } else { e.name.clone() });
            ed.set_description(tf("{0} lines, showing from line {1}", &[&e.total_lines, &(e.first_line + 1)]));
            ed.add_action(Action::Focus);
            ed.add_action(Action::SetTextSelection);
            let mut run_ids = Vec::new();
            for (i, r) in e.runs.iter().enumerate() {
                let id = NodeId(RUN_BASE + i as u64);
                let mut run = Node::new(Role::TextRun);
                run.set_value(r.text.clone());
                run.set_character_lengths(r.text.chars().map(|c| c.len_utf8() as u8).collect::<Vec<u8>>());
                run.set_word_starts(word_starts(&r.text));
                run_ids.push(id);
                nodes.push((id, run));
            }
            if let (Some(a), Some(f)) = (position_of(&e.runs, e.selection.0), position_of(&e.runs, e.selection.1)) {
                ed.set_text_selection(TextSelection { anchor: a, focus: f });
            }
            // For clients that ask for AXValue (text range queries are still resolved from the runs).
            ed.set_value(e.runs.iter().map(|r| r.text.as_str()).collect::<String>());
            ed.set_children(run_ids);
            nodes.push((EDITOR, ed));
            kids.push(EDITOR);
        }

        // Status bar
        let mut status = Node::new(Role::Status);
        self.geometry.apply(STATUS, &mut status);
        status.set_label(t("Status bar"));
        let mut items = Vec::new();
        for (i, t) in self.status.iter().enumerate() {
            let id = NodeId(STATUS_ITEM + i as u64);
            let mut l = Node::new(Role::Label);
            l.set_value(t.clone());
            items.push(id);
            nodes.push((id, l));
        }
        status.set_children(items);
        nodes.push((STATUS, status));
        kids.push(STATUS);

        // The update dialog is modal: while it is open it is the only thing a screen reader should land on.
        if let Some(d) = &self.dialog {
            let mut dialog = Node::new(Role::Dialog);
            dialog.set_label(d.title.clone());
            dialog.set_description(d.description.clone());
            dialog.set_modal();
            let mut children = Vec::new();
            if let Some(progress) = d.progress {
                let mut bar = Node::new(Role::ProgressIndicator);
                bar.set_label(t("Progress"));
                if let Some(fraction) = progress {
                    bar.set_min_numeric_value(0.);
                    bar.set_max_numeric_value(100.);
                    bar.set_numeric_value((fraction * 100.).round());
                }
                children.push(UPDATE_PROGRESS);
                nodes.push((UPDATE_PROGRESS, bar));
            }
            for (i, label) in d.buttons.iter().enumerate() {
                let id = NodeId(UPDATE_BUTTON + i as u64);
                let mut b = Node::new(Role::Button);
                b.set_label(label.clone());
                b.add_action(Action::Click);
                b.add_action(Action::Focus);
                children.push(id);
                nodes.push((id, b));
            }
            dialog.set_children(children);
            nodes.push((UPDATE_DIALOG, dialog));
            kids.push(UPDATE_DIALOG);
        }

        root.set_children(kids);
        nodes.push((ROOT, root));

        let focus = match self.focus {
            _ if self.dialog.is_some() => self.dialog.as_ref().and_then(|d| d.focused).map_or(UPDATE_DIALOG, |i| NodeId(UPDATE_BUTTON + i as u64)),
            FocusSnap::Editor if self.editor.is_some() => EDITOR,
            FocusSnap::Explorer => self.rows.iter().find(|r| r.selected).map_or(EXPLORER, |r| NodeId(r.id)),
            _ => ROOT,
        };
        TreeUpdate { nodes, tree: Some(TreeInfo::new(ROOT)), tree_id: TreeId::ROOT, focus }
    }
}

impl Workbench {
    /// Flat summary of the current view (the tree is only built while AT is active).
    pub(crate) fn a11y_snapshot(&self, window: &Window, cx: &App) -> Snapshot {
        let tree = self.roots.primary.read(cx);
        let (rows, selected) = tree.a11y_rows();
        let rows_total = rows.len();
        let rows = rows
            .into_iter()
            .take(MAX_ROWS)
            .enumerate()
            .map(|(i, (path, name, depth, is_dir, expanded))| RowSnap {
                id: row_id(&path).0,
                path,
                name,
                depth,
                is_dir,
                expanded,
                selected: selected == Some(i),
            })
            .collect();
        let root_name = tree.root().map(|r| crate::ui::file_name(r).to_string());
        let title = match root_name {
            Some(n) if !n.is_empty() => format!("Codeyard \u{2014} {n}"),
            _ => "Codeyard".to_string(),
        };

        let editor_visible = !self.tabs.diff_active && !self.tabs.large_active && !self.tabs.page_active;
        let ed = self.active_editor().filter(|_| editor_visible);
        let editor = ed.map(|e| {
            let e = e.read(cx);
            let line = e.cursor_position().0.saturating_sub(1);
            let first = line.saturating_sub(WINDOW_BEFORE);
            let (selection, total_lines, lines) = e.a11y_view(first, line + WINDOW_AFTER);
            EditorSnap { name: e.name.to_string(), dirty: e.is_dirty(), runs: runs_for(lines, total_lines, first), selection, total_lines, first_line: first }
        });

        // Same texts as the status bar (status_bar() elements; should move into a shared function later).
        let mut status = Vec::new();
        status.push(match &self.remote.conn {
            Conn::Local => t("Local").to_string(),
            Conn::Connecting { alias, .. } => tf("Connecting to {}", &[alias]),
            Conn::Connected { alias } => format!("ssh://{alias}"),
            Conn::Reconnecting { alias, attempt } => tf("Reconnecting to {0} (attempt {1})", &[alias, attempt]),
            Conn::Failed { alias, .. } => tf("{}: connection failed", &[alias]),
        });
        if let Some(b) = &self.vcs.branch {
            status.push(tf("Branch {}", &[b]));
        }
        let (errors, warnings) = self.problem_counts();
        status.push(tf("{0} errors, {1} warnings", &[&errors, &warnings]));
        if let Some(e) = ed {
            let e = e.read(cx);
            let (l, col) = e.cursor_position();
            status.push(match (e.cursor_count(), e.selection_len()) {
                (n, _) if n > 1 => tn("{} cursor", "{} cursors", n, &[]),
                (_, Some(n)) => tf("Ln {}, Col {} ({} selected)", &[&l, &col, &n]),
                _ => tf("Ln {}, Col {}", &[&l, &col]),
            });
            status.push(e.indent_label());
            status.push(e.line_ending().to_string());
            status.push(e.language_label().to_string());
            status.push(e.encoding().label().to_string());
        }
        if let Some(item) = self.update_status_item() {
            status.push(item.tip.to_string());
        }
        if let Some(Notice::Info(m)) = &self.notice {
            status.push(m.to_string());
        }
        let dialog = self.dialog_model(cx).map(|model| DialogSnap {
            description: model.text().split_once('\n').map_or(String::new(), |(_, rest)| rest.to_string()),
            progress: match &model.body {
                super::update_glue::Body::Progress { fraction, .. } => Some(fraction.map(f64::from)),
                super::update_glue::Body::Working(_) => Some(None),
                _ => None,
            },
            focused: self.update.focus.iter().position(|h| h.is_focused(window)).filter(|i| *i < model.buttons.len()),
            buttons: model.buttons.iter().map(|b| b.label.to_string()).collect(),
            title: model.title.to_string(),
        });

        let focus = if tree.focus_handle(cx).is_focused(window) {
            FocusSnap::Explorer
        } else if ed.is_some_and(|e| e.read(cx).focus_handle(cx).is_focused(window)) {
            FocusSnap::Editor
        } else {
            FocusSnap::Window
        };
        Snapshot { geometry: Geometry::new(&self.chrome.a11y_bounds, window), title, rows, rows_total, editor, status, dialog, focus }
    }

    /// Action from AT (main thread, in the GPUI context).
    fn a11y_action(&mut self, req: &ActionRequest, last: &Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        match (req.action, req.target_node) {
            (Action::Focus, EDITOR) => {
                if let Some(e) = self.active_editor() {
                    window.focus(&e.read(cx).focus_handle(cx));
                }
            }
            (Action::Focus, EXPLORER) => window.focus(&self.roots.primary.read(cx).focus_handle(cx)),
            (Action::Focus | Action::Click, id) if id.0 & !0xffff_ffff == ROW_BASE => {
                let Some(ix) = last.rows.iter().position(|r| r.id == id.0) else { return };
                let tree = self.roots.primary.clone();
                window.focus(&tree.read(cx).focus_handle(cx));
                if req.action == Action::Click {
                    tree.update(cx, |t, cx| t.a11y_activate(ix, cx));
                }
            }
            (Action::Focus | Action::Click, id) if id.0 >= UPDATE_BUTTON && id.0 < ROW_BASE => {
                let (Some(snap), Some(index)) = (&last.dialog, id.0.checked_sub(UPDATE_BUTTON).map(|i| i as usize)) else { return };
                // The tree may be a quarter of a second old: act only if that button is still there under the same name.
                let Some(model) = self.dialog_model(cx) else { return };
                let Some(button) = model.buttons.get(index).filter(|b| snap.buttons.get(index).is_some_and(|l| *l == b.label.as_ref())) else { return };
                match req.action {
                    Action::Click => self.update_action(button.action, window, cx),
                    _ => {
                        if let Some(handle) = self.update.focus.get(index) {
                            window.focus(handle);
                        }
                    }
                }
            }
            (Action::SetTextSelection, EDITOR) => {
                let (Some(ActionData::SetTextSelection(sel)), Some(snap)) = (&req.data, &last.editor) else { return };
                let (Some(a), Some(h)) = (char_of(&snap.runs, sel.anchor), char_of(&snap.runs, sel.focus)) else { return };
                if let Some(e) = self.active_editor().cloned() {
                    window.focus(&e.read(cx).focus_handle(cx));
                    e.update(cx, |e, cx| e.a11y_set_selection(a, h, cx));
                }
            }
            _ => {}
        }
        cx.notify();
    }
}

#[cfg(target_os = "macos")]
pub use mac::attach;

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use accesskit::{ActionHandler, ActivationHandler};
    use accesskit_macos::SubclassingAdapter;
    use gpui::{AsyncApp, WindowHandle};
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    struct Ctl {
        adapter: Option<SubclassingAdapter>,
        last: Snapshot,
        focused: bool,
    }

    struct Activate {
        cx: AsyncApp,
        handle: WindowHandle<Workbench>,
        ctl: Rc<RefCell<Ctl>>,
        started: Rc<AtomicBool>,
    }

    impl ActivationHandler for Activate {
        fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
            // The assistive technology asked for the first time: start the loop once. We can't build the tree here
            // (no GPUI context); the first TreeUpdate goes out on the next turn, without waiting for the timer.
            if !self.started.swap(true, Ordering::SeqCst) {
                let (cx, handle, ctl) = (self.cx.clone(), self.handle, self.ctl.clone());
                self.cx.spawn(async move |_| pump(cx, handle, ctl).await).detach();
            }
            None
        }
    }

    struct Actions {
        cx: AsyncApp,
        handle: WindowHandle<Workbench>,
        ctl: Rc<RefCell<Ctl>>,
    }

    impl ActionHandler for Actions {
        fn do_action(&mut self, request: ActionRequest) {
            let (mut cx, handle, ctl) = (self.cx.clone(), self.handle, self.ctl.clone());
            self.cx
                .spawn(async move |_| {
                    let last = ctl.borrow().last.clone();
                    cx.update_window(handle.into(), |root, window, cx| {
                        if let Ok(wb) = root.downcast::<Workbench>() {
                            wb.update(cx, |wb, cx| wb.a11y_action(&request, &last, window, cx));
                        }
                    })
                    .ok();
                })
                .detach();
        }
    }

    /// While active: take a snapshot, send it if changed, report window focus.
    async fn pump(mut cx: AsyncApp, handle: WindowHandle<Workbench>, ctl: Rc<RefCell<Ctl>>) {
        let mut first = true;
        loop {
            let got = cx.update_window(handle.into(), |root, window, cx| {
                let wb = root.downcast::<Workbench>().ok()?;
                Some((wb.read(cx).a11y_snapshot(window, cx), window.is_window_active()))
            });
            let Ok(Some((snap, active))) = got else { return };
            // Don't hold the borrow across the await (the action task reads `ctl`).
            let events: Vec<_> = {
                let mut c = ctl.borrow_mut();
                let changed = first || snap != c.last;
                if changed {
                    c.last = snap.clone();
                }
                let focus_changed = first || c.focused != active;
                c.focused = active;
                let mut events = Vec::new();
                if let Some(ad) = c.adapter.as_mut() {
                    if changed {
                        events.extend(ad.update_if_active(|| snap.to_tree()));
                    }
                    if focus_changed {
                        events.extend(ad.update_view_focus_state(active));
                    }
                }
                events
            };
            for ev in events {
                ev.raise();
            }
            first = false;
            cx.background_executor().timer(Duration::from_millis(250)).await;
        }
    }

    /// Attach the accessibility adapter to the window. Until an assistive technology queries, the cost is
    /// just a few hundred bytes and the ObjC subclass; the tree and loop start then.
    pub fn attach(handle: WindowHandle<Workbench>, cx: &mut App) {
        // The GPUI test window has `unimplemented!()` for `window_handle()`; no real NSView.
        if cfg!(test) || std::env::var_os("CODEYARD_NO_A11Y").is_some() {
            return;
        }
        let async_cx = cx.to_async();
        let ctl = Rc::new(RefCell::new(Ctl { adapter: None, last: Snapshot::default(), focused: false }));
        let started = Rc::new(AtomicBool::new(false));
        let view = handle.update(cx, |_, window, _| {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            match HasWindowHandle::window_handle(window).ok()?.as_raw() {
                RawWindowHandle::AppKit(h) => Some(h.ns_view.as_ptr()),
                _ => None,
            }
        });
        let Ok(Some(view)) = view else { return };
        // SAFETY: `view` is the live NSView GPUI holds; the adapter drops with the window
        // (ctl is released with the task and closures when the window closes).
        let adapter = unsafe {
            SubclassingAdapter::new(
                view,
                Activate { cx: async_cx.clone(), handle, ctl: ctl.clone(), started },
                Actions { cx: async_cx, handle, ctl: ctl.clone() },
            )
        };
        ctl.borrow_mut().adapter = Some(adapter);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> Snapshot {
        let lines = vec![(0, "fn main() {".to_string()), (12, "    println!(\"hé\");".to_string()), (31, "}".to_string())];
        let runs = runs_for(lines, 3, 0);
        Snapshot {
            geometry: Geometry::default(),
            title: "Codeyard \u{2014} demo".into(),
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
            editor: Some(EditorSnap { name: "main.rs".into(), dirty: false, runs, selection: (3, 7), total_lines: 3, first_line: 0 }),
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
    fn selection_round_trips_through_runs() {
        let s = snap();
        let runs = &s.editor.as_ref().unwrap().runs;
        for ix in [0, 3, 11, 12, 20, 31, 32] {
            let p = position_of(runs, ix).unwrap();
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
}

#[cfg(test)]
#[path = "a11y_geometry_tests.rs"]
mod geometry_tests;
