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
#[path = "a11y_text.rs"]
mod text;
pub use geometry::Geometry;
pub(super) use geometry::{Landmark, MENU_BUTTON, PaintedBounds, Part, RowFrame, StatusPart, landmark, part_frame, part_frame_for, row_button, row_frame};
pub use text::RunFrame;
#[cfg(test)]
use text::runs_for;
use text::{char_of, position_of, run_nodes, runs_painted};

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
const ABOUT_TAB: NodeId = NodeId(29);
const ABOUT: NodeId = NodeId(30);
const ABOUT_COPY: NodeId = NodeId(31);
const SCM_REPOS: NodeId = NodeId(38);
/// The change list of Source Control (scrolls its exposed range).
const SCM_CHANGES: NodeId = NodeId(40);
const ABOUT_DETAIL: u64 = 0xa0_0000;
const EXT: NodeId = NodeId(41);
const EXT_TABS: NodeId = NodeId(42);
const EXT_TAB_INSTALLED: NodeId = NodeId(43);
const EXT_TAB_MARKET: NodeId = NodeId(44);
const EXT_SEARCH: NodeId = NodeId(45);
const EXT_STATUS: NodeId = NodeId(46);
const EXT_LIST: NodeId = NodeId(47);
const EXT_PAGE: NodeId = NodeId(48);
const EXT_PAGE_INSTALL: NodeId = NodeId(49);
const EXT_ROW: u64 = 0xd0_0000;
const EXT_PAGE_LINE: u64 = 0xe0_0000;
const INSPECTOR: NodeId = NodeId(39);
const INSPECTOR_LINE: u64 = 0x110_0000;
/// Elements of extension views (a11y_ext_views.rs).
const EXT_VIEW_NODE: u64 = 0x100_0000;
const SERVICES: NodeId = NodeId(24);
const SERVICES_INFO: NodeId = NodeId(25);
const SERVICES_STATUS: NodeId = NodeId(26);
const SERVICES_INSTALL: NodeId = NodeId(27);
const SERVICES_CANCEL: NodeId = NodeId(28);
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
/// Repositories of the Source Control inventory.
const SCM_REPO: u64 = 0xf0_0000;
/// Ids per block (status items, update buttons, tabs, ...).
const BLOCK: u64 = 0x10_0000;
const ROW_BASE: u64 = 0x2_0000_0000;
const RUN_BASE: u64 = 0x4_0000_0000;
const TERM_RUN_BASE: u64 = 0x8_0000_0000;
/// Lines of the editor shown to AT (see `window_start`).
const EDITOR_WINDOW: usize = 300;
/// Characters per text run (word_starts is u8, so at most 255).
const RUN_CHARS: usize = 200;
/// Rows of an explorer tree shown to AT (see `window_start`).
const EXPLORER_WINDOW: usize = 1000;
/// The "More Actions…" button of Explorer row `i` of tree `k` (`EXPLORER_MENU + k * EXPLORER_WINDOW + i`).
const EXPLORER_MENU: u64 = 0x1e0_0000;
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
    /// Index of the first row of `rows` in the whole tree.
    pub first: usize,
    pub focused: Option<u64>,
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
    /// Where the row was painted (`None`: scrolled out of view).
    pub frame: Option<accesskit::Rect>,
}

/// A text run: a slice of a single line (at most RUN_CHARS characters).
#[derive(Clone, PartialEq, Debug)]
pub struct RunSnap {
    /// Index of the first character in the document.
    pub start: usize,
    /// Character count including the line ending.
    pub chars: usize,
    pub text: String,
    /// Where it was painted (`None`: not on screen).
    pub frame: Option<RunFrame>,
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
    /// Where its parts were painted.
    pub frames: Vec<(crate::find::FindFrame, accesskit::Rect)>,
}

impl FindSnap {
    fn place(&self, part: crate::find::FindFrame, node: &mut Node) {
        if let Some((_, bounds)) = self.frames.iter().find(|(k, _)| *k == part) {
            node.set_bounds(*bounds);
        }
    }
}

/// A list of options: picker rows, suggestions or extensions.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct ListSnap {
    pub rows: Vec<(String, Option<String>)>,
    pub rows_total: usize,
    pub selected: Option<usize>,
    /// Where each row was painted (physical pixels; `None` or missing: not on screen).
    pub frames: Vec<Option<accesskit::Rect>>,
    /// Where the list itself was painted.
    pub bounds: Option<accesskit::Rect>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct PickerSnap {
    pub label: String,
    pub message: Option<String>,
    pub query: String,
    pub list: ListSnap,
    pub empty: Option<String>,
    /// Where the picker and its query box were painted in this window.
    pub bounds: Option<accesskit::Rect>,
    pub input: Option<accesskit::Rect>,
}

impl PickerSnap {
    /// The picker as drawn in `window` (no bounds when it was drawn in another window).
    pub(super) fn of(picker: &crate::picker::Picker, window: &Window) -> Self {
        let v = picker.a11y_view(MAX_OPTIONS, window);
        PickerSnap {
            label: v.placeholder,
            message: v.message,
            query: v.query,
            list: ListSnap { rows: v.rows, rows_total: v.rows_total, selected: v.selected, frames: v.frames, bounds: v.list },
            empty: v.empty,
            bounds: v.bounds,
            input: v.input,
        }
    }
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
    /// The change list of Source Control (its cursor row is the focused node).
    ScmChanges,
    /// The repository list of Source Control (the selected repository is the focused option).
    ScmRepos,
    AboutCopy,
    ExtSearch,
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

#[path = "a11y_chrome.rs"]
mod chrome_access;
#[path = "a11y_extensions.rs"]
mod ext_access;
#[path = "a11y_ext_views.rs"]
mod ext_views_access;
#[path = "a11y_ids.rs"]
mod ids;
#[path = "a11y_inspector.rs"]
mod inspector_access;
pub(in crate::workbench) use chrome_access::view_part;
#[path = "a11y_popups.rs"]
mod popups_access;
#[path = "a11y_problems.rs"]
mod problems_access;
#[path = "a11y_scm.rs"]
mod scm_access;
#[path = "a11y_search_editor.rs"]
mod search_access;
#[path = "a11y_language_services.rs"]
mod services_access;
#[cfg(test)]
pub(crate) use ext_views_access::Act as ExtViewAct;
#[path = "a11y_term_tabs.rs"]
mod term_tab_access;
#[path = "a11y_windows.rs"]
pub(in crate::workbench) mod windows_access;
use search_access::SEARCH_ED_TAB;
pub(crate) use search_access::{SearchEdSnap, SearchTabSnap};
pub(crate) use term_tab_access::TermTabSnap;

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
    /// Where the views are (side bars, panel dock): the window's description.
    pub(crate) layout: String,
    pub explorers: Vec<ExplorerSnap>,
    pub tabs: Vec<TabSnap>,
    /// Index into `tabs` of the tab in front (`None` while a diff, large file or page tab is in front).
    pub active_tab: Option<usize>,
    pub about: Option<AboutSnap>,
    pub editor: Option<EditorSnap>,
    pub find: Option<FindSnap>,
    pub completion: Option<ListSnap>,
    /// The editor's hover and signature popups.
    pub(crate) popups: Option<crate::editor::PopupsA11y>,
    /// The activity bar, the title bar, the caption buttons and the side views' header buttons.
    pub(crate) chrome: chrome_access::ChromeSnap,
    /// Controls the views declared where they are drawn (`a11y_controls.rs`), in this window's last frame.
    pub(crate) controls: Vec<crate::a11y_controls::ControlSnap>,
    pub terminal: Option<TerminalSnap>,
    pub(crate) scm: Option<ScmSnap>,
    pub(crate) views: problems_access::ViewsSnap,
    pub(crate) services: Option<crate::language_services_page::AccessView>,
    pub(crate) exts: Option<ext_access::ExtSnap>,
    pub(crate) ext_page: Option<ext_access::ExtPageSnap>,
    pub(crate) inspector: Option<inspector_access::InspectorSnap>,
    pub(crate) ext_views: Vec<ext_views_access::ViewSnap>,
    /// The Search Editor in front, and every Search Editor tab of the focused group.
    pub(crate) search_editor: Option<SearchEdSnap>,
    pub(crate) search_tabs: Vec<SearchTabSnap>,
    /// The terminal tabs of the focused group.
    pub(crate) term_tabs: Vec<TermTabSnap>,
    pub picker: Option<PickerSnap>,
    pub notice: Option<NoticeSnap>,
    pub status: Vec<String>,
    /// What each status item is (the same order as `status`).
    pub(in crate::workbench) status_parts: Vec<StatusPart>,
    pub dialog: Option<DialogSnap>,
    pub focus: FocusSnap,
}

/// First of the `len` items (explorer rows, editor lines) of `total` shown to AT, so snapshots stay bounded: centred on
/// `focus` (selection, caret) while it is on screen, otherwise on the `visible` range (after a scroll).
pub(crate) fn window_start(total: usize, len: usize, focus: Option<usize>, visible: Option<(usize, usize)>) -> usize {
    let centre = match (focus, visible) {
        (Some(f), Some((a, b))) if (a..=b).contains(&f) => f,
        (_, Some((a, b))) => (a + b) / 2,
        (f, None) => f.unwrap_or(0),
    };
    centre.saturating_sub(len / 2).min(total.saturating_sub(len))
}

/// Scroll position of a windowed node, in items (rows, lines), and its scroll actions.
fn set_scroll(node: &mut Node, first: usize, shown: usize, total: usize) {
    node.set_scroll_y(first as f64);
    node.set_scroll_y_min(0.);
    node.set_scroll_y_max(total.saturating_sub(shown) as f64);
    node.add_action(Action::ScrollUp);
    node.add_action(Action::ScrollDown);
}

impl EditorSnap {
    /// The lines around the caret, or the visible ones after a scroll; run offsets are document-relative. Runs on lines
    /// painted in the editor's last frame (drawn at `scale`) carry where their characters are.
    pub(super) fn of(e: &crate::editor::EditorView, scale: f32) -> EditorSnap {
        let line = e.cursor_position().0.saturating_sub(1);
        let first = window_start(e.line_count(), EDITOR_WINDOW, Some(line), e.visible_lines());
        let (selection, total_lines, lines) = e.a11y_view(first, first + EDITOR_WINDOW);
        EditorSnap {
            name: e.name.to_string(),
            dirty: e.is_dirty(),
            runs: runs_painted(lines, total_lines, first, &e.a11y_layout(), scale),
            selection,
            total_lines,
            first_line: first,
        }
    }
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
        if let Some(b) = list.frames.get(i).copied().flatten() {
            o.set_bounds(b);
        }
        o.set_position_in_set(i + 1);
        o.set_size_of_set(list.rows_total);
        o.add_action(Action::Click);
        o.add_action(Action::Focus);
        ids.push(oid);
        nodes.push((oid, o));
    }
    node.set_children(ids);
    if let Some(b) = list.bounds {
        node.set_bounds(b);
    }
    node
}

/// The editor's suggestions, with where they were painted (at `scale`) in the editor's last frame.
fn completion_snap(e: &crate::editor::EditorView, scale: f32) -> Option<ListSnap> {
    let (rows, selected) = e.a11y_completion(MAX_OPTIONS)?;
    let (bounds, frames) = e.a11y_completion_frames(rows.len(), scale);
    Some(ListSnap { rows_total: rows.len(), selected: Some(selected), rows, frames, bounds })
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
        root.set_description(self.layout.clone());
        let mut kids = Vec::new();
        self.chrome.append_tree(&mut nodes, &mut kids, &self.geometry);
        // Painted controls by surface; the status bar's join its items below.
        let (mut status_controls, mut tab_controls) = (Vec::new(), Vec::new());
        for (surface, ids) in crate::a11y_controls::append_nodes(&self.controls, &mut nodes) {
            if surface == t("Status bar") {
                status_controls = ids;
            } else if surface == t("Editor tabs") {
                tab_controls = ids;
            } else {
                kids.push(crate::a11y_controls::group(surface, ids, &mut nodes));
            }
        }
        if let Some(view) = &self.services {
            services_access::append_tree(view, &mut nodes);
            kids.push(SERVICES);
        }

        // Explorer: one tree per folder of the workspace.
        for (k, ex) in self.explorers.iter().enumerate() {
            let mut tree = Node::new(Role::Tree);
            self.geometry.apply(explorer_id(k), &mut tree);
            tree.set_label(if self.explorers.len() > 1 { format!("{}: {}", t("Explorer"), ex.name) } else { t("Explorer").to_string() });
            tree.set_size_of_set(ex.rows_total);
            tree.set_multiselectable();
            set_scroll(&mut tree, ex.first, ex.rows.len(), ex.rows_total);
            let mut row_ids = Vec::new();
            for (i, r) in ex.rows.iter().enumerate() {
                let mut n = Node::new(Role::TreeItem);
                n.set_label(r.name.clone());
                n.set_level(r.depth + 1);
                n.set_selected(r.selected);
                n.set_position_in_set(ex.first + i + 1);
                n.set_size_of_set(ex.rows_total);
                if let Some(bounds) = r.frame {
                    n.set_bounds(bounds);
                }
                if r.is_dir {
                    n.set_expanded(r.expanded);
                    n.set_description(t("folder"));
                }
                n.add_action(Action::Click);
                n.add_action(Action::Focus);
                // Its menu, as Shift+F10 opens it: the action, and a button for the adapters that do not forward it.
                n.add_action(Action::ShowContextMenu);
                if i < EXPLORER_WINDOW {
                    let id = NodeId(EXPLORER_MENU + (k * EXPLORER_WINDOW + i) as u64);
                    let mut more = Node::new(Role::Button);
                    more.set_label(t("More Actions…"));
                    more.set_description(r.name.clone());
                    more.set_has_popup(accesskit::HasPopup::Menu);
                    more.add_action(Action::Click);
                    nodes.push((id, more));
                    n.set_children(vec![id]);
                }
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
        self.views.append_tree(&mut nodes, &mut kids, &self.geometry);

        if let Some(view) = &self.exts {
            ext_access::append_tree(view, &mut nodes);
            kids.push(EXT);
        }
        if let Some(page) = &self.ext_page {
            ext_access::append_page(page, &mut nodes);
            kids.push(EXT_PAGE);
        }
        if let Some(snap) = &self.inspector {
            inspector_access::append_tree(snap, &mut nodes);
            kids.push(INSPECTOR);
        }
        for (slot, view) in self.ext_views.iter().enumerate() {
            kids.push(view.append(slot, &mut nodes));
        }

        // Tabs
        if !self.tabs.is_empty() || self.about.is_some() || !self.search_tabs.is_empty() || !self.term_tabs.is_empty() || !tab_controls.is_empty() {
            let total = self.tabs.len() + usize::from(self.about.is_some()) + self.search_tabs.len() + self.term_tabs.len();
            let mut list = Node::new(Role::TabList);
            list.set_label(t("Editor tabs"));
            let mut tab_ids = Vec::new();
            for (i, tab) in self.tabs.iter().enumerate() {
                let id = NodeId(TAB_BASE + i as u64);
                let mut n = Node::new(Role::Tab);
                n.set_label(if tab.dirty { tf("{} (modified)", &[&tab.name]) } else { tab.name.clone() });
                n.set_selected(self.active_tab == Some(i));
                n.set_position_in_set(i + 1);
                n.set_size_of_set(total);
                if let Some(bounds) = self.geometry.part(Part::EditorTab(i)) {
                    n.set_bounds(bounds);
                }
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
                tab.set_size_of_set(total);
                tab.add_action(Action::Click);
                tab.add_action(Action::Focus);
                tab_ids.push(ABOUT_TAB);
                nodes.push((ABOUT_TAB, tab));
            }
            let before = self.tabs.len() + usize::from(self.about.is_some());
            for (i, s) in self.search_tabs.iter().enumerate() {
                let id = NodeId(SEARCH_ED_TAB + i as u64);
                let mut tab = Node::new(Role::Tab);
                tab.set_label(s.title.clone());
                tab.set_selected(s.active);
                tab.set_position_in_set(before + i + 1);
                tab.set_size_of_set(total);
                tab.add_action(Action::Click);
                tab.add_action(Action::Focus);
                self.geometry.apply_part(Part::SearchTab(i), &mut tab);
                tab_ids.push(id);
                nodes.push((id, tab));
            }
            term_tab_access::append_tabs(&self.term_tabs, before + self.search_tabs.len(), total, &mut tab_ids, &mut nodes, &self.geometry);
            // The large-file, diff and page tabs, as painted.
            tab_ids.extend(tab_controls);
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

        if let Some(se) = &self.search_editor {
            se.append_tree(&mut nodes);
            kids.push(SearchEdSnap::id());
        }

        // Editor
        if let Some(e) = &self.editor {
            let mut ed = Node::new(Role::MultilineTextInput);
            self.geometry.apply(EDITOR, &mut ed);
            ed.set_label(if e.dirty { tf("{} (modified)", &[&e.name]) } else { e.name.clone() });
            let last = e.total_lines.min(e.first_line + EDITOR_WINDOW).max(e.first_line + 1);
            ed.set_description(tf("{0} lines, showing lines {1}–{2}", &[&e.total_lines, &(e.first_line + 1), &last]));
            set_scroll(&mut ed, e.first_line, last - e.first_line, e.total_lines);
            ed.add_action(Action::Focus);
            ed.add_action(Action::SetTextSelection);
            ed.add_action(Action::ShowTooltip);
            if self.popups.as_ref().is_some_and(|p| p.hover.is_some() || p.signature.is_some()) {
                ed.add_action(Action::HideTooltip);
            }
            let mut children = run_nodes(&e.runs, RUN_BASE, &mut nodes);
            if let (Some(a), Some(f)) = (position_of(&e.runs, RUN_BASE, e.selection.0), position_of(&e.runs, RUN_BASE, e.selection.1)) {
                ed.set_text_selection(TextSelection { anchor: a, focus: f });
            }
            // For clients that ask for AXValue (text range queries are still resolved from the runs).
            ed.set_value(e.runs.iter().map(|r| r.text.as_str()).collect::<String>());
            if let Some(popups) = &self.popups {
                popups_access::append(popups, &mut nodes, &mut children);
            }
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

        // Find bar: drawn over the top of the editor, so it comes after it (hit testing takes the last sibling as the
        // one on top); the screen reader reaches it from the editor or through its focus.
        if let Some(f) = &self.find {
            let mut bar = Node::new(Role::Group);
            bar.set_label(t("Find"));
            let mut children = Vec::new();
            let mut query = Node::new(Role::SearchInput);
            query.set_label(t("Find"));
            query.set_value(f.query.clone());
            query.add_action(Action::Focus);
            f.place(crate::find::FindFrame::Query, &mut query);
            children.push(FIND_QUERY);
            nodes.push((FIND_QUERY, query));
            if let Some(rep) = &f.replacement {
                let mut field = Node::new(Role::TextInput);
                field.set_label(t("Replace"));
                field.set_value(rep.clone());
                field.add_action(Action::Focus);
                f.place(crate::find::FindFrame::Replace, &mut field);
                children.push(FIND_REPLACE);
                nodes.push((FIND_REPLACE, field));
            }
            if let Some(st) = &f.status {
                // A live region: "3 of 12" is spoken while typing and moving between matches.
                let mut label = Node::new(Role::Status);
                label.set_label(st.clone());
                label.set_live(Live::Polite);
                f.place(crate::find::FindFrame::Status, &mut label);
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
                f.place(crate::find::FindFrame::Control(i), &mut b);
                children.push(id);
                nodes.push((id, b));
            }
            bar.set_children(children);
            nodes.push((FIND, bar));
            kids.push(FIND);
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
            // Every item but the encoding opens something when clicked: a button that does the same.
            let pressable = self.status_parts.get(i).is_some_and(|p| *p != StatusPart::Encoding);
            let mut l = Node::new(if pressable { Role::Button } else { Role::Label });
            l.set_value(t.clone());
            if pressable {
                l.set_label(t.clone());
                l.add_action(Action::Click);
            }
            if let Some(bounds) = self.geometry.status_item(i) {
                l.set_bounds(bounds);
            }
            items.push(id);
            nodes.push((id, l));
        }
        items.extend(status_controls);
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
            if let Some(b) = pk.bounds {
                dialog.set_bounds(b);
            }
            let mut input = Node::new(Role::EditableComboBox);
            if let Some(b) = pk.input {
                input.set_bounds(b);
            }
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
            self.geometry.apply_part(Part::UpdateDialog, &mut dialog);
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
                self.geometry.apply_part(Part::DialogButton(i), &mut b);
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
            _ if self.services.as_ref().is_some_and(|view| view.focused) => services_access::focus_node(self.services.as_ref().expect("focused page")),
            _ if self.search_editor.as_ref().and_then(SearchEdSnap::focus_node).is_some() => {
                self.search_editor.as_ref().and_then(SearchEdSnap::focus_node).expect("focused search editor")
            }
            _ if self.views.focus_node().is_some() => self.views.focus_node().expect("focused view"),
            _ if crate::a11y_controls::focus_node(&self.controls).is_some() => crate::a11y_controls::focus_node(&self.controls).expect("focused control"),
            _ if self.scm.as_ref().and_then(ScmSnap::history_focus).is_some() => self.scm.as_ref().and_then(ScmSnap::history_focus).expect("graph search"),
            FocusSnap::Picker if self.picker.is_some() => PICKER_INPUT,
            FocusSnap::Find(0) if self.find.is_some() => FIND_QUERY,
            FocusSnap::Find(_) if self.find.as_ref().is_some_and(|f| f.replacement.is_some()) => FIND_REPLACE,
            FocusSnap::Editor if self.editor.is_some() => EDITOR,
            FocusSnap::Terminal if self.terminal.is_some() => TERMINAL,
            FocusSnap::ScmMessage if self.scm.as_ref().is_some_and(|s| s.view.root.is_some()) => SCM_MESSAGE,
            FocusSnap::ScmChanges => self.scm.as_ref().map_or(ROOT, ScmSnap::focus_node),
            FocusSnap::ScmRepos => match self.scm.as_ref().map(|s| &s.view.repos).filter(|r| !r.is_empty()) {
                Some(repos) => repos.iter().position(|r| r.selected).map_or(SCM_REPOS, |i| NodeId(SCM_REPO + i as u64)),
                None => ROOT,
            },
            FocusSnap::AboutCopy if self.about.as_ref().is_some_and(|a| a.active) => ABOUT_COPY,
            FocusSnap::ExtSearch if self.exts.is_some() => EXT_SEARCH,
            FocusSnap::Explorer(k) if k < self.explorers.len() => self.explorers[k].focused.map_or(explorer_id(k), NodeId),
            _ => ROOT,
        };
        TreeUpdate { nodes, tree: Some(TreeInfo::new(ROOT)), tree_id: TreeId::ROOT, focus }
    }
}

impl Workbench {
    /// Flat summary of the current view (the tree is only built while AT is active).
    pub(crate) fn a11y_snapshot(&self, window: &Window, cx: &App) -> Snapshot {
        let trees = if self.view_shown(Panel::Explorer) { self.all_trees() } else { Vec::new() };
        let explorers = trees
            .iter()
            .map(|tree| {
                let tree = tree.read(cx);
                let (rows, selected) = tree.a11y_rows();
                let rows_total = rows.len();
                let first = window_start(rows_total, EXPLORER_WINDOW, selected, tree.visible_rows());
                let shown = first..first + EXPLORER_WINDOW;
                let focused = selected.filter(|i| shown.contains(i)).and_then(|i| rows.get(i)).map(|r| row_id(&r.0).0);
                let rows = rows
                    .into_iter()
                    .skip(first)
                    .take(EXPLORER_WINDOW)
                    .map(|(path, name, depth, is_dir, expanded)| RowSnap {
                        id: row_id(&path).0,
                        selected: tree.path_is_selected(&path),
                        frame: tree.a11y_row_frame(&path, window),
                        path,
                        name,
                        depth,
                        is_dir,
                        expanded,
                    })
                    .collect();
                ExplorerSnap { name: tree.root().map_or(String::new(), |r| crate::ui::file_name(r).to_string()), rows, rows_total, first, focused }
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
        let mut editor = ed.map(|e| EditorSnap::of(e.read(cx), window.scale_factor()));

        // Same texts as the status bar (status_bar() elements; should move into a shared function later).
        let mut status = Vec::new();
        let mut status_parts = vec![StatusPart::Remote];
        status.push(match &self.remote.conn {
            Conn::Local => t("Local").to_string(),
            Conn::Connecting { alias, .. } => tf("Connecting to {}", &[alias]),
            Conn::Connected { alias } => format!("ssh://{alias}"),
            Conn::Reconnecting { alias, attempt } => tf("Reconnecting to {0} (attempt {1})", &[alias, attempt]),
            Conn::Failed { alias, .. } => tf("{}: connection failed", &[alias]),
        });
        if let Some(b) = &self.vcs.branch {
            status.push(tf("Branch {}", &[b]));
            status_parts.push(StatusPart::Branch);
        }
        let (errors, warnings) = self.problem_counts();
        status.push(tf("{0} errors, {1} warnings", &[&errors, &warnings]));
        status_parts.push(StatusPart::Problems);
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
            status_parts.extend([StatusPart::Position, StatusPart::Indent, StatusPart::LineEnding, StatusPart::Language, StatusPart::Encoding]);
        }
        if let Some(item) = self.update_status_item() {
            status.push(item.tip.to_string());
            status_parts.push(StatusPart::Update);
        }
        let mut geometry = Geometry::new(&self.chrome.a11y_bounds, window);
        geometry.set_status_items(&status_parts);
        // Character positions are those of the editor's last paint: only while it is drawn in this window's frame.
        if !geometry.editor_painted() {
            editor.iter_mut().flat_map(|e| e.runs.iter_mut()).for_each(|r| r.frame = None);
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
                frames: b.a11y_frames(window),
            }
        });
        let mut completion = ed.and_then(|e| completion_snap(e.read(cx), window.scale_factor()));
        let mut popups = ed.map(|e| e.read(cx).a11y_popups(window.scale_factor()));
        // Painted positions only while the editor was drawn in this window's last frame.
        if !geometry.editor_painted() {
            if let Some(list) = completion.as_mut() {
                (list.frames, list.bounds) = (Vec::new(), None);
            }
            if let Some(p) = popups.as_mut() {
                p.forget_frames();
            }
        }
        let shown = self.a11y_terminal(window, cx);
        let term = shown.as_ref().map(|(t, _, _)| t.clone());
        let terminal = shown.as_ref().map(|(term, ix, count)| term_tab_access::terminal_snap(term, *ix, *count, window.scale_factor(), cx));
        let picker_entity = self.overlay.as_ref().map(|o| o.picker.clone());
        let picker = picker_entity.as_ref().map(|p| PickerSnap::of(p.read(cx), window));

        let (search_editor, search_tabs) = self.a11y_search_snap(window, cx);
        let scm = (self.view_shown(Panel::Scm)).then(|| ScmSnap { generation: self.lsp.generation, view: self.scm.read(cx).a11y_view(window, cx) });
        let exts = self.a11y_extensions(window, cx);
        let focus = if self.tabs.page_active && about_view.is_some_and(|view| view.focus_handle(cx).is_focused(window)) {
            FocusSnap::AboutCopy
        } else if let Some(k) = trees.iter().position(|t| t.read(cx).focus_handle(cx).is_focused(window)) {
            FocusSnap::Explorer(k)
        } else if scm.as_ref().is_some_and(|s| s.view.message_focused) {
            FocusSnap::ScmMessage
        } else if scm.as_ref().is_some_and(|s| s.view.list_focused && s.view.root.is_some()) {
            FocusSnap::ScmChanges
        } else if exts.as_ref().is_some_and(|e| e.search_focused) {
            FocusSnap::ExtSearch
        } else if scm.as_ref().is_some_and(|s| s.view.repos_focused) {
            FocusSnap::ScmRepos
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
            services: self.tabs.page.as_ref().filter(|_| self.tabs.page_active).and_then(|page| match page {
                Page::LanguageServices(page) => Some(page.read(cx).access_view(window, cx)),
                _ => None,
            }),
            geometry,
            title,
            layout: self.a11y_layout(),
            explorers,
            tabs,
            active_tab,
            about,
            editor,
            find,
            completion,
            popups,
            chrome: self.a11y_chrome(window, cx),
            controls: crate::a11y_controls::snapshot(window, cx),
            terminal,
            scm,
            views: self.a11y_views(window, cx),
            exts,
            ext_page: self.a11y_extension_page(),
            inspector: self.a11y_inspector(),
            ext_views: self.a11y_ext_views(window),
            search_editor,
            search_tabs,
            term_tabs: self.a11y_term_tabs(cx),
            picker,
            notice,
            status,
            status_parts,
            dialog,
            focus,
        }
    }

    /// Action from AT (main thread, in the GPUI context).
    fn a11y_action(&mut self, req: &ActionRequest, last: &Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        if last.generation != self.lsp.generation {
            return;
        }
        if self.a11y_services_action(req, last, window, cx) {
            return;
        }
        if self.a11y_chrome_action(req, &last.chrome, window, cx) || self.a11y_control_action(req, &last.controls, window, cx) {
            return;
        }
        if self.a11y_scm_action(req, last, window, cx) {
            cx.notify();
            return;
        }
        if self.a11y_views_action(req, last, window, cx) {
            cx.notify();
            return;
        }
        if self.a11y_ext_action(req, last, window, cx) || self.a11y_ext_view_action(req, last, window, cx) || self.a11y_inspector_action(req, last, cx) {
            cx.notify();
            return;
        }
        if self.a11y_search_action(req, last, window, cx) || self.a11y_term_tab_action(req, last, window, cx) || self.a11y_popup_action(req, last, window, cx) {
            return;
        }
        match (req.action, req.target_node) {
            (Action::Click, NodeId(n)) if (STATUS_ITEM..STATUS_ITEM + 32).contains(&n) => {
                // The same item, still in the status bar: what a click on it does.
                let Some(part) = last.status_parts.get((n - STATUS_ITEM) as usize).copied() else { return };
                if !self.a11y_snapshot(window, cx).status_parts.contains(&part) {
                    return;
                }
                match part {
                    StatusPart::Remote => self.show_remote(&ShowRemote, window, cx),
                    StatusPart::Branch => self.show_branch_picker(window, cx),
                    StatusPart::Problems => self.show_problems(&ShowProblems, window, cx),
                    StatusPart::Position => self.go_to_line(&GoToLine, window, cx),
                    StatusPart::Indent => self.show_indent_picker(window, cx),
                    StatusPart::LineEnding => self.show_eol_picker(window, cx),
                    StatusPart::Language => self.show_language_picker(window, cx),
                    StatusPart::Update => self.open_update_dialog(window, cx),
                    StatusPart::Encoding => {}
                }
            }
            (Action::Focus | Action::Click, ABOUT_TAB) if last.about.is_some() => {
                if let Some(Page::About(view)) = self.tabs.page.clone() {
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
                if !self.view_shown(Panel::Explorer) {
                    return;
                }
                let k = if id == EXPLORER { 0 } else { (id.0 - EXTRA_TREE) as usize };
                if let Some(tree) = self.all_trees().get(k) {
                    window.focus(&tree.read(cx).focus_handle(cx));
                }
            }
            (Action::Focus | Action::Click | Action::ShowContextMenu, id)
                if id.0 & !0xffff_ffff == ROW_BASE || (EXPLORER_MENU..EXPLORER_MENU + BLOCK).contains(&id.0) =>
            {
                if !self.view_shown(Panel::Explorer) {
                    return;
                }
                let menu = (EXPLORER_MENU..EXPLORER_MENU + BLOCK).contains(&id.0);
                let found = if menu {
                    let n = (id.0 - EXPLORER_MENU) as usize;
                    Some((n / EXPLORER_WINDOW, n % EXPLORER_WINDOW)).filter(|(k, ix)| last.explorers.get(*k).is_some_and(|ex| *ix < ex.rows.len()))
                } else {
                    last.explorers.iter().enumerate().find_map(|(k, ex)| ex.rows.iter().position(|r| r.id == id.0).map(|ix| (k, ix)))
                };
                let Some((k, ix)) = found else { return };
                let Some(tree) = self.all_trees().get(k).cloned() else { return };
                window.focus(&tree.read(cx).focus_handle(cx));
                let path = &last.explorers[k].rows[ix].path;
                if menu || req.action == Action::ShowContextMenu {
                    // Under the row, with the keyboard; closing it gives the keyboard back to the tree on that row.
                    tree.update(cx, |t, cx| t.a11y_context_menu(path, window, cx));
                } else {
                    tree.update(cx, |t, cx| t.a11y_select(path, req.action == Action::Click, cx));
                }
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
                if label.is_none() || picker.read(cx).a11y_view(index + 1, window).rows.get(index).map(|(l, _)| l) != label.as_ref() {
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
                if let Some(term) = self.a11y_terminal(window, cx).map(|(t, _, _)| t).or_else(|| self.active_terminal()) {
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
            (Action::ScrollUp | Action::ScrollDown, EDITOR) => {
                let pages = if req.action == Action::ScrollUp { -1 } else { 1 };
                if let Some(e) = self.active_editor().cloned() {
                    e.update(cx, |e, cx| e.a11y_scroll(pages, cx));
                }
            }
            (Action::ScrollUp | Action::ScrollDown, id) if id == EXPLORER || (id.0 >= EXTRA_TREE && id.0 < EXTRA_TREE + BLOCK) => {
                let k = if id == EXPLORER { 0 } else { (id.0 - EXTRA_TREE) as usize };
                let pages = if req.action == Action::ScrollUp { -1 } else { 1 };
                if let Some(tree) = self.all_trees().get(k).filter(|_| self.view_shown(Panel::Explorer)) {
                    tree.update(cx, |t, cx| t.a11y_scroll(pages, cx));
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
pub(in crate::workbench) mod platform {
    use super::*;
    use accesskit::{ActionHandler, ActivationHandler};
    #[cfg(target_os = "macos")]
    use accesskit_macos::SubclassingAdapter;
    #[cfg(windows)]
    use accesskit_windows::SubclassingAdapter;
    use futures::StreamExt as _;
    use futures::channel::mpsc::{UnboundedSender, unbounded};
    use gpui::{AnyWindowHandle, AsyncApp, WindowHandle};
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
        handle: AnyWindowHandle,
        hooks: windows_access::Hooks,
        ctl: Rc<RefCell<Ctl>>,
        started: Rc<AtomicBool>,
    }

    impl ActivationHandler for Activate {
        fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
            // The assistive technology asked for the first time: start the loop once. We can't build the tree here
            // (no GPUI context); the first TreeUpdate goes out on the next turn, without waiting for the timer.
            if !self.started.swap(true, Ordering::SeqCst) {
                let (cx, handle, hooks, ctl) = (self.cx.clone(), self.handle, self.hooks, self.ctl.clone());
                self.cx.spawn(async move |_| pump(cx, handle, hooks, ctl).await).detach();
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
    async fn pump(mut cx: AsyncApp, handle: AnyWindowHandle, hooks: windows_access::Hooks, ctl: Rc<RefCell<Ctl>>) {
        let mut first = true;
        loop {
            let got = cx.update_window(handle, |root, window, cx| Some(((hooks.snapshot)(root, window, cx)?, window.is_window_active())));
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
        attach_with(handle.into(), windows_access::WORKBENCH, cx);
    }

    /// Attach the adapter to any window whose tree `hooks` builds (an editor's or a terminal's own window).
    pub(in crate::workbench) fn attach_with(handle: AnyWindowHandle, hooks: windows_access::Hooks, cx: &mut App) {
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
        let raw = cx.update_window(handle, |_, window, _| {
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
        let activate = Activate { cx: async_cx.clone(), handle, hooks, ctl: ctl.clone(), started };
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
                    let window = action_cx.update_window(handle, |root, window, cx| (hooks.action)(root, &request, &last, window, cx));
                    if window.is_err() {
                        return;
                    }
                }
            })
            .detach();
    }
}

#[cfg(test)]
#[path = "a11y_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "a11y_geometry_tests.rs"]
mod geometry_tests;

#[cfg(test)]
#[path = "a11y_frames_tests.rs"]
mod frames_tests;

#[cfg(test)]
#[path = "a11y_row_frames_tests.rs"]
mod row_frames_tests;

#[cfg(test)]
#[path = "a11y_list_frames_tests.rs"]
mod list_frames_tests;

#[cfg(test)]
#[path = "a11y_views_tests.rs"]
mod views_tests;

#[cfg(test)]
#[path = "a11y_chrome_tests.rs"]
mod chrome_tests;

#[cfg(test)]
#[path = "a11y_side_controls_tests.rs"]
mod side_controls_tests;

#[cfg(test)]
#[path = "a11y_confirm_tests.rs"]
mod confirm_tests;

#[cfg(test)]
#[path = "a11y_range_tests.rs"]
mod range_tests;

#[cfg(test)]
#[path = "a11y_scm_tests.rs"]
mod scm_tests;

#[cfg(test)]
#[path = "a11y_visibility_tests.rs"]
mod visibility_tests;
