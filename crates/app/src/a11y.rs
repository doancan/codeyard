//! Accessibility tree: exposes the window, the explorer trees, the editor tabs, the active editor (text field +
//! cursor/selection, find bar, suggestions), the terminal, pickers (command palette and the rest), notices (live region)
//! and the status bar text to VoiceOver through AccessKit.
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
use accesskit::{Action, ActionData, ActionRequest, Live, Node, NodeId, Role, TextPosition, TextSelection, Toggled, TreeId, TreeInfo, TreeUpdate};

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
const TABS: NodeId = NodeId(7);
const NOTICE: NodeId = NodeId(8);
const PICKER: NodeId = NodeId(9);
const PICKER_INPUT: NodeId = NodeId(10);
const PICKER_LIST: NodeId = NodeId(11);
const FIND: NodeId = NodeId(12);
const FIND_QUERY: NodeId = NodeId(13);
const FIND_REPLACE: NodeId = NodeId(14);
const FIND_STATUS: NodeId = NodeId(15);
const COMPLETION: NodeId = NodeId(16);
const TERMINAL: NodeId = NodeId(17);
const SCM: NodeId = NodeId(18);
const SCM_MESSAGE: NodeId = NodeId(19);
const SCM_COMMIT: NodeId = NodeId(20);
const SCM_BRANCH: NodeId = NodeId(21);
const SCM_RESULT: NodeId = NodeId(22);
const SCM_EMPTY: NodeId = NodeId(23);
const ABOUT_TAB: NodeId = NodeId(24);
const ABOUT: NodeId = NodeId(25);
const ABOUT_COPY: NodeId = NodeId(26);
const ABOUT_DETAIL: u64 = 0xa0_0000;
const STATUS_ITEM: u64 = 0x10_0000;
const UPDATE_BUTTON: u64 = 0x20_0000;
const TAB_BASE: u64 = 0x30_0000;
const PICK_OPTION: u64 = 0x40_0000;
const FIND_CONTROL: u64 = 0x50_0000;
const COMPLETION_OPTION: u64 = 0x60_0000;
/// Explorer trees of the added folders (the first folder's tree is `EXPLORER`).
const EXTRA_TREE: u64 = 0x70_0000;
const SCM_GROUP: u64 = 0x80_0000;
const SCM_FILE: u64 = 0x90_0000;
/// Ids per block (status items, update buttons, tabs, ...).
const BLOCK: u64 = 0x10_0000;
const ROW_BASE: u64 = 0x2_0000_0000;
const RUN_BASE: u64 = 0x4_0000_0000;
const TERM_RUN_BASE: u64 = 0x8_0000_0000;
const _: () = {
    let blocks = [STATUS_ITEM, UPDATE_BUTTON, TAB_BASE, PICK_OPTION, FIND_CONTROL, COMPLETION_OPTION, EXTRA_TREE, SCM_GROUP, SCM_FILE, ABOUT_DETAIL, ROW_BASE];
    let mut i = 1;
    while i < blocks.len() {
        assert!(blocks[i - 1] + BLOCK <= blocks[i]);
        i += 1;
    }
    // Rows are ROW_BASE | 32-bit hash; editor and terminal runs sit above them.
    assert!(ROW_BASE + 0xffff_ffff < RUN_BASE && RUN_BASE + 0xffff_ffff < TERM_RUN_BASE);
};
/// Window shown to AT in the editor: this many lines before/after the cursor line.
const WINDOW_BEFORE: usize = 100;
const WINDOW_AFTER: usize = 200;
/// Characters per text run (word_starts is u8, so at most 255).
const RUN_CHARS: usize = 200;
/// Max rows shown to AT in the explorer.
const MAX_ROWS: usize = 5000;
/// Max rows shown to AT in a picker or the suggestion list.
const MAX_OPTIONS: usize = 500;

fn row_id(path: &str) -> NodeId {
    // FNV-1a: ids of other rows must not change when a file is added or removed.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in path.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    NodeId(ROW_BASE | (h & 0xffff_ffff))
}

/// The explorer tree of one folder of the workspace.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct ExplorerSnap {
    pub name: String,
    pub rows: Vec<RowSnap>,
    pub rows_total: usize,
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

/// The find bar of the active editor.
#[derive(Clone, PartialEq, Debug)]
pub struct FindSnap {
    pub query: String,
    /// The replacement, while the replace row is shown.
    pub replacement: Option<String>,
    /// "3 of 12", "No results" or a pattern error.
    pub status: Option<String>,
    /// Options (with their state) and buttons, in visual order.
    pub controls: Vec<(String, Option<bool>)>,
}

/// A list of options: picker rows or suggestions.
#[derive(Clone, PartialEq, Debug)]
pub struct ListSnap {
    pub rows: Vec<(String, Option<String>)>,
    pub rows_total: usize,
    pub selected: Option<usize>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct PickerSnap {
    pub label: String,
    pub message: Option<String>,
    pub query: String,
    pub list: ListSnap,
    pub empty: Option<String>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct TerminalSnap {
    pub label: String,
    pub runs: Vec<RunSnap>,
    /// Character index of the cursor, if it is on screen.
    pub cursor: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FocusSnap {
    #[default]
    Window,
    /// Index of the explorer tree.
    Explorer(usize),
    Editor,
    /// 0 query, 1 replacement.
    Find(usize),
    Picker,
    Terminal,
    ScmMessage,
    AboutCopy,
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

/// An editor tab of the focused group.
#[derive(Clone, PartialEq, Debug)]
pub struct TabSnap {
    pub name: String,
    pub dirty: bool,
}

/// The About tab and its visible contents.
#[derive(Clone, PartialEq, Debug)]
pub struct AboutSnap {
    pub active: bool,
    pub details: Vec<(String, String)>,
}

/// The notice shown in the status bar; errors interrupt the screen reader, info waits its turn.
#[derive(Clone, PartialEq, Debug)]
pub struct NoticeSnap {
    pub text: String,
    pub error: bool,
}

#[path = "a11y_scm.rs"]
mod scm_access;

#[derive(Clone, PartialEq, Debug)]
pub(crate) struct ScmSnap {
    generation: u64,
    pub view: crate::scm::AccessView,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct Snapshot {
    generation: u64,
    /// Physical landmark frames from the latest painted layout.
    pub geometry: Geometry,
    pub title: String,
    pub explorers: Vec<ExplorerSnap>,
    pub tabs: Vec<TabSnap>,
    /// Index into `tabs` of the tab in front (`None` while a diff, large file or page tab is in front).
    pub active_tab: Option<usize>,
    pub about: Option<AboutSnap>,
    pub editor: Option<EditorSnap>,
    pub find: Option<FindSnap>,
    pub completion: Option<ListSnap>,
    pub terminal: Option<TerminalSnap>,
    pub(crate) scm: Option<ScmSnap>,
    pub picker: Option<PickerSnap>,
    pub notice: Option<NoticeSnap>,
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
/// `base`: id of the first run.
fn position_of(runs: &[RunSnap], base: u64, ix: usize) -> Option<TextPosition> {
    let first = runs.first()?;
    if ix <= first.start {
        return Some(TextPosition { node: NodeId(base), character_index: 0 });
    }
    for (i, r) in runs.iter().enumerate() {
        if ix < r.start + r.chars {
            return Some(TextPosition { node: NodeId(base + i as u64), character_index: ix - r.start });
        }
    }
    let i = runs.len() - 1;
    Some(TextPosition { node: NodeId(base + i as u64), character_index: runs[i].text.chars().count() })
}

fn char_of(runs: &[RunSnap], p: TextPosition) -> Option<usize> {
    let i = p.node.0.checked_sub(RUN_BASE)? as usize;
    runs.get(i).map(|r| r.start + p.character_index.min(r.chars))
}

/// Text run nodes (one per run) with ids from `base`.
fn run_nodes(runs: &[RunSnap], base: u64, nodes: &mut Vec<(NodeId, Node)>) -> Vec<NodeId> {
    let mut ids = Vec::new();
    for (i, r) in runs.iter().enumerate() {
        let id = NodeId(base + i as u64);
        let mut run = Node::new(Role::TextRun);
        run.set_value(r.text.clone());
        run.set_character_lengths(r.text.chars().map(|c| c.len_utf8() as u8).collect::<Vec<u8>>());
        run.set_word_starts(word_starts(&r.text));
        ids.push(id);
        nodes.push((id, run));
    }
    ids
}

/// A list box with one option per row (ids from `base`); options can be focused and clicked.
fn list_nodes(list: &ListSnap, label: &str, base: u64, nodes: &mut Vec<(NodeId, Node)>) -> Node {
    let mut node = Node::new(Role::ListBox);
    node.set_label(label.to_string());
    let mut ids = Vec::new();
    for (i, (name, detail)) in list.rows.iter().enumerate() {
        let oid = NodeId(base + i as u64);
        let mut o = Node::new(Role::ListBoxOption);
        o.set_label(name.clone());
        if let Some(d) = detail.as_ref().filter(|d| !d.is_empty()) {
            o.set_description(d.clone());
        }
        o.set_selected(list.selected == Some(i));
        o.set_position_in_set(i + 1);
        o.set_size_of_set(list.rows_total);
        o.add_action(Action::Click);
        o.add_action(Action::Focus);
        ids.push(oid);
        nodes.push((oid, o));
    }
    node.set_children(ids);
    node
}

/// Id of the selected option of a list (if it is among the exposed rows).
fn selected_option(list: &ListSnap, base: u64) -> Option<NodeId> {
    list.selected.filter(|i| *i < list.rows.len()).map(|i| NodeId(base + i as u64))
}

fn explorer_id(i: usize) -> NodeId {
    if i == 0 { EXPLORER } else { NodeId(EXTRA_TREE + i as u64) }
}

impl Snapshot {
    pub fn to_tree(&self) -> TreeUpdate {
        let mut nodes: Vec<(NodeId, Node)> = Vec::new();
        let mut root = Node::new(Role::Window);
        self.geometry.apply(ROOT, &mut root);
        root.set_label(self.title.clone());
        let mut kids = Vec::new();

        // Explorer: one tree per folder of the workspace.
        for (k, ex) in self.explorers.iter().enumerate() {
            let mut tree = Node::new(Role::Tree);
            self.geometry.apply(explorer_id(k), &mut tree);
            tree.set_label(if self.explorers.len() > 1 { format!("{}: {}", t("Explorer"), ex.name) } else { t("Explorer").to_string() });
            tree.set_size_of_set(ex.rows_total);
            let mut row_ids = Vec::new();
            for (i, r) in ex.rows.iter().enumerate() {
                let mut n = Node::new(Role::TreeItem);
                n.set_label(r.name.clone());
                n.set_level(r.depth + 1);
                n.set_selected(r.selected);
                n.set_position_in_set(i + 1);
                n.set_size_of_set(ex.rows_total);
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
            nodes.push((explorer_id(k), tree));
            kids.push(explorer_id(k));
        }

        if let Some(scm) = &self.scm {
            scm.append_tree(&mut nodes);
            kids.push(SCM);
        }

        // Tabs
        if !self.tabs.is_empty() || self.about.is_some() {
            let mut list = Node::new(Role::TabList);
            list.set_label(t("Editor tabs"));
            let mut tab_ids = Vec::new();
            for (i, tab) in self.tabs.iter().enumerate() {
                let id = NodeId(TAB_BASE + i as u64);
                let mut n = Node::new(Role::Tab);
                n.set_label(if tab.dirty { tf("{} (modified)", &[&tab.name]) } else { tab.name.clone() });
                n.set_selected(self.active_tab == Some(i));
                n.set_position_in_set(i + 1);
                n.set_size_of_set(self.tabs.len() + usize::from(self.about.is_some()));
                n.add_action(Action::Click);
                n.add_action(Action::Focus);
                tab_ids.push(id);
                nodes.push((id, n));
            }
            if let Some(about) = &self.about {
                let mut tab = Node::new(Role::Tab);
                tab.set_label(t("About Codeyard"));
                tab.set_selected(about.active);
                tab.set_position_in_set(self.tabs.len() + 1);
                tab.set_size_of_set(self.tabs.len() + 1);
                tab.add_action(Action::Click);
                tab.add_action(Action::Focus);
                tab_ids.push(ABOUT_TAB);
                nodes.push((ABOUT_TAB, tab));
            }
            list.set_children(tab_ids);
            nodes.push((TABS, list));
            kids.push(TABS);
        }

        if let Some(about) = &self.about
            && about.active
        {
            let mut group = Node::new(Role::Group);
            group.set_label(t("About Codeyard"));
            let mut children = Vec::new();
            for (i, (label, value)) in about.details.iter().enumerate() {
                let id = NodeId(ABOUT_DETAIL + i as u64);
                let mut detail = Node::new(Role::Label);
                detail.set_label(format!("{label}: {value}"));
                children.push(id);
                nodes.push((id, detail));
            }
            let mut copy = Node::new(Role::Button);
            copy.set_label(t("Copy Version"));
            copy.add_action(Action::Click);
            copy.add_action(Action::Focus);
            children.push(ABOUT_COPY);
            nodes.push((ABOUT_COPY, copy));
            group.set_children(children);
            nodes.push((ABOUT, group));
            kids.push(ABOUT);
        }

        // Find bar (above the editor, as on screen)
        if let Some(f) = &self.find {
            let mut bar = Node::new(Role::Group);
            bar.set_label(t("Find"));
            let mut children = Vec::new();
            let mut query = Node::new(Role::SearchInput);
            query.set_label(t("Find"));
            query.set_value(f.query.clone());
            query.add_action(Action::Focus);
            children.push(FIND_QUERY);
            nodes.push((FIND_QUERY, query));
            if let Some(rep) = &f.replacement {
                let mut field = Node::new(Role::TextInput);
                field.set_label(t("Replace"));
                field.set_value(rep.clone());
                field.add_action(Action::Focus);
                children.push(FIND_REPLACE);
                nodes.push((FIND_REPLACE, field));
            }
            if let Some(st) = &f.status {
                // A live region: "3 of 12" is spoken while typing and moving between matches.
                let mut label = Node::new(Role::Status);
                label.set_label(st.clone());
                label.set_live(Live::Polite);
                children.push(FIND_STATUS);
                nodes.push((FIND_STATUS, label));
            }
            for (i, (name, on)) in f.controls.iter().enumerate() {
                let id = NodeId(FIND_CONTROL + i as u64);
                let mut b = Node::new(if on.is_some() { Role::CheckBox } else { Role::Button });
                b.set_label(name.clone());
                if let Some(on) = on {
                    b.set_toggled(Toggled::from(*on));
                }
                b.add_action(Action::Click);
                children.push(id);
                nodes.push((id, b));
            }
            bar.set_children(children);
            nodes.push((FIND, bar));
            kids.push(FIND);
        }

        // Editor
        if let Some(e) = &self.editor {
            let mut ed = Node::new(Role::MultilineTextInput);
            self.geometry.apply(EDITOR, &mut ed);
            ed.set_label(if e.dirty { tf("{} (modified)", &[&e.name]) } else { e.name.clone() });
            ed.set_description(tf("{0} lines, showing from line {1}", &[&e.total_lines, &(e.first_line + 1)]));
            ed.add_action(Action::Focus);
            ed.add_action(Action::SetTextSelection);
            let mut children = run_nodes(&e.runs, RUN_BASE, &mut nodes);
            if let (Some(a), Some(f)) = (position_of(&e.runs, RUN_BASE, e.selection.0), position_of(&e.runs, RUN_BASE, e.selection.1)) {
                ed.set_text_selection(TextSelection { anchor: a, focus: f });
            }
            // For clients that ask for AXValue (text range queries are still resolved from the runs).
            ed.set_value(e.runs.iter().map(|r| r.text.as_str()).collect::<String>());
            // Suggestions: the editor keeps the keyboard focus, the selected suggestion is its active descendant.
            if let Some(list) = &self.completion {
                let node = list_nodes(list, t("Suggestions"), COMPLETION_OPTION, &mut nodes);
                nodes.push((COMPLETION, node));
                children.push(COMPLETION);
                ed.set_controls(vec![COMPLETION]);
                if let Some(o) = selected_option(list, COMPLETION_OPTION) {
                    ed.set_active_descendant(o);
                }
            }
            ed.set_children(children);
            nodes.push((EDITOR, ed));
            kids.push(EDITOR);
        }

        // Terminal: the rows on screen, read-only, with the cursor as the caret.
        if let Some(term) = &self.terminal {
            let mut node = Node::new(Role::Terminal);
            node.set_label(term.label.clone());
            node.set_read_only();
            node.add_action(Action::Focus);
            node.set_children(run_nodes(&term.runs, TERM_RUN_BASE, &mut nodes));
            if let Some(p) = term.cursor.and_then(|c| position_of(&term.runs, TERM_RUN_BASE, c)) {
                node.set_text_selection(TextSelection { anchor: p, focus: p });
            }
            node.set_value(term.runs.iter().map(|r| r.text.as_str()).collect::<String>());
            nodes.push((TERMINAL, node));
            kids.push(TERMINAL);
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

        // Notice: a live region, so a screen reader speaks it when it appears or changes.
        if let Some(n) = &self.notice {
            let mut alert = Node::new(if n.error { Role::Alert } else { Role::Status });
            alert.set_label(n.text.clone());
            alert.set_live(if n.error { Live::Assertive } else { Live::Polite });
            nodes.push((NOTICE, alert));
            kids.push(NOTICE);
        }

        // Picker (command palette, go to file, themes, ...): a dialog with an editable combo box and its list.
        if let Some(pk) = &self.picker {
            let mut dialog = Node::new(Role::Dialog);
            dialog.set_label(pk.label.clone());
            if let Some(m) = &pk.message {
                dialog.set_description(m.clone());
            }
            dialog.set_modal();
            let mut input = Node::new(Role::EditableComboBox);
            input.set_label(pk.label.clone());
            input.set_value(pk.query.clone());
            input.set_expanded(true);
            input.set_controls(vec![PICKER_LIST]);
            input.add_action(Action::Focus);
            if let Some(o) = selected_option(&pk.list, PICK_OPTION) {
                input.set_active_descendant(o);
            }
            let mut list = list_nodes(&pk.list, &pk.label, PICK_OPTION, &mut nodes);
            if let Some(empty) = &pk.empty {
                list.set_description(empty.clone());
            }
            nodes.push((PICKER_INPUT, input));
            nodes.push((PICKER_LIST, list));
            dialog.set_children(vec![PICKER_INPUT, PICKER_LIST]);
            nodes.push((PICKER, dialog));
            kids.push(PICKER);
        }

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
            FocusSnap::Picker if self.picker.is_some() => PICKER_INPUT,
            FocusSnap::Find(0) if self.find.is_some() => FIND_QUERY,
            FocusSnap::Find(_) if self.find.as_ref().is_some_and(|f| f.replacement.is_some()) => FIND_REPLACE,
            FocusSnap::Editor if self.editor.is_some() => EDITOR,
            FocusSnap::Terminal if self.terminal.is_some() => TERMINAL,
            FocusSnap::ScmMessage if self.scm.as_ref().is_some_and(|s| s.view.root.is_some()) => SCM_MESSAGE,
            FocusSnap::AboutCopy if self.about.as_ref().is_some_and(|a| a.active) => ABOUT_COPY,
            FocusSnap::Explorer(k) if k < self.explorers.len() => self.explorers[k].rows.iter().find(|r| r.selected).map_or(explorer_id(k), |r| NodeId(r.id)),
            _ => ROOT,
        };
        TreeUpdate { nodes, tree: Some(TreeInfo::new(ROOT)), tree_id: TreeId::ROOT, focus }
    }
}

impl Workbench {
    /// Flat summary of the current view (the tree is only built while AT is active).
    pub(crate) fn a11y_snapshot(&self, window: &Window, cx: &App) -> Snapshot {
        let trees = if self.chrome.panel == Some(Panel::Explorer) { self.all_trees() } else { Vec::new() };
        let explorers = trees
            .iter()
            .map(|tree| {
                let tree = tree.read(cx);
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
                ExplorerSnap { name: tree.root().map_or(String::new(), |r| crate::ui::file_name(r).to_string()), rows, rows_total }
            })
            .collect();
        let tree = self.roots.primary.read(cx);
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
        let notice = match (&self.notice, &self.tabs.open_error) {
            (Some(Notice::Error(m)), _) => Some(NoticeSnap { text: m.to_string(), error: true }),
            (_, Some((_, m))) => Some(NoticeSnap { text: m.to_string(), error: true }),
            (Some(Notice::Info(m)), _) => Some(NoticeSnap { text: m.to_string(), error: false }),
            (None, None) => None,
        };
        let tabs = self.tabs.editors.iter().map(|e| e.read(cx)).map(|e| TabSnap { name: e.name.to_string(), dirty: e.is_dirty() }).collect();
        let active_tab = self.tabs.active.filter(|_| editor_visible);
        let about_view = match &self.tabs.page {
            Some(Page::About(view)) => Some(view),
            _ => None,
        };
        let about = about_view.map(|view| AboutSnap { active: self.tabs.page_active, details: view.read(cx).a11y_details() });
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

        let find_bar = ed.and_then(|e| e.read(cx).find_bar());
        let find = find_bar.as_ref().map(|b| {
            let b = b.read(cx);
            FindSnap {
                query: b.query(cx),
                replacement: b.show_replace.then(|| b.replacement(cx)),
                status: b.status_text().map(str::to_string),
                controls: b.a11y_controls().into_iter().map(|(l, on)| (l.to_string(), on)).collect(),
            }
        });
        let completion = ed.and_then(|e| e.read(cx).a11y_completion(MAX_OPTIONS)).map(|(rows, selected)| ListSnap {
            rows_total: rows.len(),
            selected: Some(selected),
            rows,
        });
        let term = self.active_terminal().filter(|_| self.chrome.panel_open && self.chrome.bottom == Bottom::Terminal);
        let terminal = term.as_ref().map(|term| {
            let (rows, cursor) = term.read(cx).a11y_view();
            let mut starts = Vec::with_capacity(rows.len());
            let mut at = 0;
            for row in &rows {
                starts.push(at);
                at += row.chars().count() + 1;
            }
            let total = rows.len();
            let cursor = cursor.and_then(|(row, col)| starts.get(row).map(|s| s + col.min(rows[row].chars().count())));
            TerminalSnap {
                label: tf("Terminal {0} of {1}", &[&(self.active_terminal + 1), &self.terminals.len()]),
                runs: runs_for(starts.into_iter().zip(rows).collect(), total, 0),
                cursor,
            }
        });
        let picker_entity = self.overlay.as_ref().map(|o| o.picker.clone());
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

        let scm = (self.chrome.panel == Some(Panel::Scm)).then(|| ScmSnap { generation: self.lsp.generation, view: self.scm.read(cx).a11y_view(window, cx) });
        let focus = if self.tabs.page_active && about_view.is_some_and(|view| view.focus_handle(cx).is_focused(window)) {
            FocusSnap::AboutCopy
        } else if let Some(k) = trees.iter().position(|t| t.read(cx).focus_handle(cx).is_focused(window)) {
            FocusSnap::Explorer(k)
        } else if scm.as_ref().is_some_and(|s| s.view.message_focused) {
            FocusSnap::ScmMessage
        } else if picker_entity.as_ref().is_some_and(|p| p.focus_handle(cx).is_focused(window)) {
            FocusSnap::Picker
        } else if let Some(field) = find_bar.as_ref().and_then(|b| b.read(cx).a11y_focus(window, cx)) {
            FocusSnap::Find(field)
        } else if ed.is_some_and(|e| e.read(cx).focus_handle(cx).is_focused(window)) {
            FocusSnap::Editor
        } else if term.as_ref().is_some_and(|t| t.focus_handle(cx).contains_focused(window, cx)) {
            FocusSnap::Terminal
        } else {
            FocusSnap::Window
        };
        Snapshot {
            generation: self.lsp.generation,
            geometry: Geometry::new(&self.chrome.a11y_bounds, window),
            title,
            explorers,
            tabs,
            active_tab,
            about,
            editor,
            find,
            completion,
            terminal,
            scm,
            picker,
            notice,
            status,
            dialog,
            focus,
        }
    }

    /// Action from AT (main thread, in the GPUI context).
    fn a11y_action(&mut self, req: &ActionRequest, last: &Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        if last.generation != self.lsp.generation {
            return;
        }
        if self.a11y_scm_action(req, last, window, cx) {
            cx.notify();
            return;
        }
        match (req.action, req.target_node) {
            (Action::Focus | Action::Click, ABOUT_TAB) if last.about.is_some() => {
                if let Some(Page::About(view)) = &self.tabs.page {
                    self.tabs.page_active = true;
                    self.tabs.diff_active = false;
                    self.tabs.large_active = false;
                    window.focus(&view.focus_handle(cx));
                    cx.notify();
                }
            }
            (Action::Focus | Action::Click, ABOUT_COPY) if last.about.as_ref().is_some_and(|a| a.active) && self.tabs.page_active => {
                if let Some(Page::About(view)) = &self.tabs.page {
                    window.focus(&view.focus_handle(cx));
                    if req.action == Action::Click {
                        view.update(cx, |view, cx| view.copy_version(cx));
                    }
                }
            }
            (Action::Focus, EDITOR) => {
                if let Some(e) = self.active_editor() {
                    window.focus(&e.read(cx).focus_handle(cx));
                }
            }
            (Action::Focus, id) if id == EXPLORER || (id.0 >= EXTRA_TREE && id.0 < EXTRA_TREE + BLOCK) => {
                if self.chrome.panel != Some(Panel::Explorer) {
                    return;
                }
                let k = if id == EXPLORER { 0 } else { (id.0 - EXTRA_TREE) as usize };
                if let Some(tree) = self.all_trees().get(k) {
                    window.focus(&tree.read(cx).focus_handle(cx));
                }
            }
            (Action::Focus | Action::Click, id) if id.0 & !0xffff_ffff == ROW_BASE => {
                if self.chrome.panel != Some(Panel::Explorer) {
                    return;
                }
                let Some((k, ix)) = last.explorers.iter().enumerate().find_map(|(k, ex)| ex.rows.iter().position(|r| r.id == id.0).map(|ix| (k, ix))) else {
                    return;
                };
                let Some(tree) = self.all_trees().get(k).cloned() else { return };
                window.focus(&tree.read(cx).focus_handle(cx));
                let path = &last.explorers[k].rows[ix].path;
                tree.update(cx, |t, cx| t.a11y_select(path, req.action == Action::Click, cx));
            }
            (Action::Focus, PICKER_INPUT) => {
                if let Some(o) = &self.overlay {
                    window.focus(&o.picker.focus_handle(cx));
                }
            }
            (Action::Focus | Action::Click, id) if id.0 >= PICK_OPTION && id.0 < PICK_OPTION + BLOCK => {
                let index = (id.0 - PICK_OPTION) as usize;
                let Some(picker) = self.overlay.as_ref().map(|o| o.picker.clone()) else { return };
                // The tree may be a quarter of a second old: act only if the row still has the same label.
                let label = last.picker.as_ref().and_then(|p| p.list.rows.get(index)).map(|(l, _)| l.clone());
                if label.is_none() || picker.read(cx).a11y_view(index + 1).rows.get(index).map(|(l, _)| l) != label.as_ref() {
                    return;
                }
                window.focus(&picker.focus_handle(cx));
                picker.update(cx, |p, cx| p.a11y_choose(index, req.action == Action::Click, cx));
            }
            (Action::Focus, FIND_QUERY | FIND_REPLACE) => {
                if let Some(bar) = self.active_editor().and_then(|e| e.read(cx).find_bar()) {
                    let bar = bar.read(cx);
                    if req.target_node == FIND_QUERY { bar.focus_query(window, cx) } else { bar.focus_replacement(window, cx) }
                }
            }
            (Action::Click, id) if id.0 >= FIND_CONTROL && id.0 < FIND_CONTROL + BLOCK => {
                let index = (id.0 - FIND_CONTROL) as usize;
                let Some(bar) = self.active_editor().and_then(|e| e.read(cx).find_bar()) else { return };
                let expected = last.find.as_ref().and_then(|f| f.controls.get(index)).map(|(l, _)| l.as_str());
                if expected.is_none() || bar.read(cx).a11y_controls().get(index).map(|(l, _)| *l) != expected {
                    return;
                }
                bar.update(cx, |b, cx| b.a11y_press(index, cx));
            }
            (Action::Focus | Action::Click, id) if id.0 >= COMPLETION_OPTION && id.0 < COMPLETION_OPTION + BLOCK => {
                let index = (id.0 - COMPLETION_OPTION) as usize;
                let Some(e) = self.active_editor().cloned() else { return };
                let expected = last.completion.as_ref().and_then(|c| c.rows.get(index)).map(|(l, _)| l.clone());
                if expected.is_none() || e.read(cx).a11y_completion(index + 1).and_then(|(rows, _)| rows.get(index).map(|(l, _)| l.clone())) != expected {
                    return;
                }
                window.focus(&e.read(cx).focus_handle(cx));
                e.update(cx, |e, cx| e.a11y_choose_completion(index, req.action == Action::Click, window, cx));
            }
            (Action::Focus, TERMINAL) => {
                if let Some(term) = self.active_terminal() {
                    window.focus(&term.focus_handle(cx));
                }
            }
            (Action::Focus | Action::Click, id) if id.0 >= TAB_BASE && id.0 < TAB_BASE + BLOCK => {
                let index = (id.0 - TAB_BASE) as usize;
                // Same check as the dialog buttons: the tree may be a quarter of a second old.
                let Some(snap) = last.tabs.get(index) else { return };
                let Some(ed) = self.tabs.editors.get(index).cloned().filter(|e| e.read(cx).name.as_ref() == snap.name) else { return };
                self.activate(index, window, cx);
                window.focus(&ed.read(cx).focus_handle(cx));
            }
            (Action::Focus | Action::Click, id) if id.0 >= UPDATE_BUTTON && id.0 < UPDATE_BUTTON + BLOCK => {
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

    #[cfg(test)]
    pub(crate) fn a11y_action_for_test(&mut self, req: &ActionRequest, last: &Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        self.a11y_action(req, last, window, cx);
    }
}

#[cfg(any(target_os = "macos", windows))]
pub use platform::attach;

/// The AccessKit adapter: `accesskit_macos` (NSAccessibility, VoiceOver) or `accesskit_windows` (UI Automation: NVDA,
/// JAWS, Narrator). Both subclass the native view or window; the tree and the actions are the same.
#[cfg(any(target_os = "macos", windows))]
mod platform {
    use super::*;
    use accesskit::{ActionHandler, ActivationHandler};
    #[cfg(target_os = "macos")]
    use accesskit_macos::SubclassingAdapter;
    #[cfg(windows)]
    use accesskit_windows::SubclassingAdapter;
    use futures::StreamExt as _;
    use futures::channel::mpsc::{UnboundedSender, unbounded};
    use gpui::{AsyncApp, WindowHandle};
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    struct Ctl {
        adapter: Option<SubclassingAdapter>,
        last: Snapshot,
        #[cfg(target_os = "macos")]
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

    /// Actions may arrive on another thread (UI Automation): they are sent to a task on the main thread.
    struct Actions(UnboundedSender<ActionRequest>);

    impl ActionHandler for Actions {
        fn do_action(&mut self, request: ActionRequest) {
            self.0.unbounded_send(request).ok();
        }
    }

    /// While active: take a snapshot, send it if changed, report window focus (Windows' adapter follows focus itself).
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
                #[cfg(target_os = "macos")]
                let focus_changed = first || c.focused != active;
                #[cfg(target_os = "macos")]
                {
                    c.focused = active;
                }
                #[cfg(windows)]
                let _ = active;
                let mut events = Vec::new();
                if let Some(ad) = c.adapter.as_mut() {
                    if changed {
                        events.extend(ad.update_if_active(|| snap.to_tree()));
                    }
                    #[cfg(target_os = "macos")]
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
    /// just a few hundred bytes and the subclass; the tree and loop start then. On Windows the window must not be
    /// visible yet (open it with `show: false` and activate it after this).
    pub fn attach(handle: WindowHandle<Workbench>, cx: &mut App) {
        // The GPUI test window has `unimplemented!()` for `window_handle()`; no real native window.
        if cfg!(test) || std::env::var_os("CODEYARD_NO_A11Y").is_some() {
            return;
        }
        let async_cx = cx.to_async();
        let ctl = Rc::new(RefCell::new(Ctl {
            adapter: None,
            last: Snapshot::default(),
            #[cfg(target_os = "macos")]
            focused: false,
        }));
        let started = Rc::new(AtomicBool::new(false));
        let raw = handle.update(cx, |_, window, _| {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            match HasWindowHandle::window_handle(window).ok()?.as_raw() {
                #[cfg(target_os = "macos")]
                RawWindowHandle::AppKit(h) => Some(h.ns_view.as_ptr()),
                #[cfg(windows)]
                RawWindowHandle::Win32(h) => Some(h.hwnd.get() as *mut std::ffi::c_void),
                _ => None,
            }
        });
        let Ok(Some(raw)) = raw else { return };
        #[cfg(windows)]
        {
            // `SubclassingAdapter::new` panics on a visible window; skip the adapter rather than the app.
            let visible = unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(windows::Win32::Foundation::HWND(raw)) };
            if visible.as_bool() {
                eprintln!("accessibility: the window was already visible, no UI Automation tree");
                return;
            }
        }
        let (tx, mut rx) = unbounded::<ActionRequest>();
        let activate = Activate { cx: async_cx.clone(), handle, ctl: ctl.clone(), started };
        // SAFETY: `raw` is the live NSView / HWND GPUI holds; the adapter drops with the window
        // (ctl is released with the task and closures when the window closes).
        #[cfg(target_os = "macos")]
        let adapter = unsafe { SubclassingAdapter::new(raw, activate, Actions(tx)) };
        #[cfg(windows)]
        let adapter = SubclassingAdapter::new(accesskit_windows::HWND(raw), activate, Actions(tx));
        ctl.borrow_mut().adapter = Some(adapter);
        let mut action_cx = async_cx.clone();
        async_cx
            .spawn(async move |_| {
                while let Some(request) = rx.next().await {
                    let last = ctl.borrow().last.clone();
                    let window = action_cx.update_window(handle.into(), |root, window, cx| {
                        if let Ok(wb) = root.downcast::<Workbench>() {
                            wb.update(cx, |wb, cx| wb.a11y_action(&request, &last, window, cx));
                        }
                    });
                    if window.is_err() {
                        return;
                    }
                }
            })
            .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> Snapshot {
        let lines = vec![(0, "fn main() {".to_string()), (12, "    println!(\"hé\");".to_string()), (31, "}".to_string())];
        let runs = runs_for(lines, 3, 0);
        Snapshot {
            generation: 0,
            geometry: Geometry::default(),
            title: "Codeyard \u{2014} demo".into(),
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
        s.picker = Some(PickerSnap {
            label: "Type a command".into(),
            message: None,
            query: ">sav".into(),
            list: list(&["File: Save", "File: Save As…"], 1),
            empty: None,
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
        s.explorers.push(ExplorerSnap { name: "e".into(), rows: vec![row], rows_total: 1 });
        s.focus = FocusSnap::Explorer(1);
        let tree = accesskit_consumer::Tree::new(s.to_tree(), true);
        let state = tree.state();
        let trees: Vec<_> = state.root().children().filter(|n| n.role() == Role::Tree).collect();
        assert_eq!(trees.len(), 2);
        assert_eq!(trees[1].label().as_deref(), Some("Explorer: e"));
        assert_eq!(state.focus().unwrap().label().as_deref(), Some("x.rs"));
    }
}

#[cfg(test)]
#[path = "a11y_geometry_tests.rs"]
mod geometry_tests;
