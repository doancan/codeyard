//! The Testing and Remote Explorer side views and the open context menu for assistive technology, with the Run and
//! Debug view (`a11y_debug.rs`) gathered in [`SideSnap`]. Rows are bounded; every action goes through the pointer's own
//! path (`run_test_item`, `debug_test`, `run_remote_menu`, `toggle_remote`, the menu's own selection), and names its
//! row by what it is (test file and id, host alias, menu and item), so an action from an outdated tree does nothing
//! when that row changed or went away.

use super::row_menus::{MenuRow, add_menu, menu_request};
use super::*;
use crate::testexp::{self, Kind, Outcome, Row};
use crate::workbench::a11y::{Geometry, RowFrame};
use crate::workbench::remote_glue::RemoteMenuCmd;
use crate::workbench::remote_glue::tree::{Group, HostStatus, RemoteRow};

#[path = "a11y_debug.rs"]
mod debug_access;
pub(in crate::workbench) use debug_access::{DEBUG_ROW, FIXED as DEBUG_FIXED};

const TESTING: NodeId = NodeId(89);
const TESTS_RUN: NodeId = NodeId(90);
const TESTS_RUN_FAILED: NodeId = NodeId(91);
const TESTS_ONLY_FAILED: NodeId = NodeId(92);
const TESTS_REFRESH: NodeId = NodeId(93);
const TESTS_STATUS: NodeId = NodeId(94);
const TESTS_TREE: NodeId = NodeId(95);
const REMOTE: NodeId = NodeId(96);
const MENU: NodeId = NodeId(97);
/// This module's fixed nodes, for the id layout check.
pub(in crate::workbench) const FIXED: [NodeId; 9] =
    [TESTING, TESTS_RUN, TESTS_RUN_FAILED, TESTS_ONLY_FAILED, TESTS_REFRESH, TESTS_STATUS, TESTS_TREE, REMOTE, MENU];

/// Test rows: `TEST_IDS` ids each (the row, Run, Debug).
pub(in crate::workbench) const TEST_ROW: u64 = 0x190_0000;
const TEST_IDS: u64 = 4;
/// Remote rows: `REMOTE_IDS` ids each (the row, Connect or Open Folder, Connect in New Window, Disconnect).
pub(in crate::workbench) const REMOTE_ROW: u64 = 0x1a0_0000;
const REMOTE_IDS: u64 = 4;
pub(in crate::workbench) const MENU_ITEM: u64 = 0x1b0_0000;
const MAX_TESTS: usize = 5_000;
const MAX_REMOTE: usize = 2_000;
const MAX_MENU: usize = 1_000;
const _: () = {
    assert!(MAX_TESTS as u64 * TEST_IDS <= BLOCK);
    assert!(MAX_REMOTE as u64 * REMOTE_IDS <= BLOCK);
};

/// A row of the Testing tree, named by its file (and test id).
#[derive(Clone, PartialEq, Debug)]
enum TestRow {
    File { rel: String, outcome: Outcome, open: bool, count: usize },
    Test { rel: String, id: String, name: String, outcome: Outcome, millis: Option<u64>, debuggable: bool },
    Message { rel: String, id: String, text: String },
}

/// The Testing view while it is shown.
#[derive(Clone, PartialEq, Debug)]
struct TestingSnap {
    running: bool,
    only_failed: bool,
    status: String,
    rows: Vec<TestRow>,
}

/// The open context menu: which menu it is and its items (label, shortcut, enabled, checked, submenu).
#[derive(Clone, PartialEq, Debug)]
struct MenuSnap {
    entity: gpui::EntityId,
    items: Vec<(String, Option<String>, bool, Option<bool>, bool)>,
    selected: Option<usize>,
}

/// The side views and the menu of this module, kept in `ViewsSnap`.
#[derive(Clone, PartialEq, Debug, Default)]
pub(crate) struct SideSnap {
    debug: debug_access::DebugSnap,
    testing: Option<TestingSnap>,
    remote: Option<Vec<RemoteRow>>,
    menu: Option<MenuSnap>,
}

fn outcome_text(o: Outcome) -> &'static str {
    match o {
        Outcome::Passed => t("passed"),
        Outcome::Failed => t("failed"),
        Outcome::Skipped => t("skipped"),
        Outcome::Running => t("running"),
        Outcome::NotRun => t("not run"),
    }
}

fn button(nodes: &mut Vec<(NodeId, Node)>, id: u64, label: &str, row: &str) -> NodeId {
    let mut b = Node::new(Role::Button);
    b.set_label(label.to_string());
    b.set_description(row.to_string());
    b.add_action(Action::Click);
    nodes.push((NodeId(id), b));
    NodeId(id)
}

/// A tree item that opens and closes.
fn expandable(n: &mut Node, open: bool) {
    n.set_expanded(open);
    n.add_action(Action::Click);
    n.add_action(if open { Action::Collapse } else { Action::Expand });
}

/// Whether `action` asks for what a click on a row that is `open` does (Expand on an open row does nothing).
fn wants_toggle(action: Action, open: bool) -> bool {
    match action {
        Action::Click => true,
        Action::Expand => !open,
        Action::Collapse => open,
        _ => false,
    }
}

impl TestingSnap {
    fn append_tree(&self, nodes: &mut Vec<(NodeId, Node)>, geometry: &Geometry) -> NodeId {
        let mut kids = Vec::new();
        let mut push = |nodes: &mut Vec<(NodeId, Node)>, id: NodeId, label: &str| {
            let mut b = Node::new(Role::Button);
            b.set_label(label.to_string());
            b.add_action(Action::Click);
            nodes.push((id, b));
            kids.push(id);
        };
        push(nodes, TESTS_RUN, if self.running { t("Stop") } else { t("Run All Tests") });
        push(nodes, TESTS_RUN_FAILED, t("Run Failed Tests"));
        push(nodes, TESTS_REFRESH, t("Refresh Tests"));
        let mut only = Node::new(Role::Switch);
        only.set_label(t("Show Only Failed Tests"));
        only.set_toggled(Toggled::from(self.only_failed));
        only.add_action(Action::Click);
        nodes.push((TESTS_ONLY_FAILED, only));
        kids.push(TESTS_ONLY_FAILED);
        let mut status = Node::new(Role::Status);
        status.set_label(self.status.clone());
        status.set_live(Live::Polite);
        nodes.push((TESTS_STATUS, status));
        kids.push(TESTS_STATUS);
        let mut tree = Node::new(Role::Tree);
        tree.set_label(t("Testing"));
        let mut ids = Vec::new();
        for (i, row) in self.rows.iter().enumerate() {
            let base = TEST_ROW + i as u64 * TEST_IDS;
            let mut n = Node::new(Role::TreeItem);
            match row {
                TestRow::File { rel, outcome, open, count } => {
                    let (name, dir) = rel.rsplit_once('/').map_or((rel.as_str(), ""), |(d, n)| (n, d));
                    n.set_label(name.to_string());
                    n.set_value(outcome_text(*outcome).to_string());
                    n.set_description(if dir.is_empty() {
                        tn("{} test", "{} tests", *count, &[])
                    } else {
                        format!("{dir}, {}", tn("{} test", "{} tests", *count, &[]))
                    });
                    n.set_level(1);
                    expandable(&mut n, *open);
                    n.set_children(vec![button(nodes, base + 1, t("Run Tests in File"), name)]);
                }
                TestRow::Test { name, outcome, millis, debuggable, .. } => {
                    n.set_label(name.clone());
                    let mut value = outcome_text(*outcome).to_string();
                    if let Some(ms) = millis {
                        value.push_str(&if *ms >= 1000 { format!(", {:.1} s", *ms as f64 / 1000.) } else { format!(", {ms} ms") });
                    }
                    n.set_value(value);
                    n.set_level(2);
                    n.add_action(Action::Click);
                    let mut buttons = vec![button(nodes, base + 1, t("Run Test"), name)];
                    if *debuggable {
                        buttons.push(button(nodes, base + 2, t("Debug Test"), name));
                    }
                    n.set_children(buttons);
                }
                TestRow::Message { text, .. } => {
                    n.set_label(text.clone());
                    n.set_level(3);
                    n.add_action(Action::Click);
                }
            }
            geometry.apply_row(RowFrame::Test, i, &mut n);
            nodes.push((NodeId(base), n));
            ids.push(NodeId(base));
        }
        tree.set_children(ids);
        nodes.push((TESTS_TREE, tree));
        kids.push(TESTS_TREE);
        let mut view = Node::new(Role::Group);
        view.set_label(t("Testing"));
        view.set_children(kids);
        nodes.push((TESTING, view));
        TESTING
    }
}

fn remote_tree(rows: &[RemoteRow], nodes: &mut Vec<(NodeId, Node)>, geometry: &Geometry) -> NodeId {
    let mut ids = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let base = REMOTE_ROW + i as u64 * REMOTE_IDS;
        let mut n = Node::new(Role::TreeItem);
        match row {
            RemoteRow::Group { group, count, expanded } => {
                n.set_label(match group {
                    Group::Ssh => t("SSH Hosts"),
                    Group::Containers => t("Containers"),
                    Group::Wsl => t("WSL Targets"),
                });
                n.set_description(count.to_string());
                n.set_level(1);
                expandable(&mut n, *expanded);
            }
            RemoteRow::Host { alias, detail, status, expanded, .. } => {
                n.set_label(alias.clone());
                n.set_value(match status {
                    HostStatus::Connected => t("Connected").to_string(),
                    HostStatus::Connecting | HostStatus::Reconnecting => t("Connecting…").to_string(),
                    HostStatus::Failed(why) => why.clone(),
                    HostStatus::Reachable(_) => t("Connected before").to_string(),
                    HostStatus::Unknown => t("Not connected yet").to_string(),
                });
                if let Some(d) = detail {
                    n.set_description(d.clone());
                }
                n.set_level(2);
                expandable(&mut n, *expanded);
                let connected = *status == HostStatus::Connected;
                let mut buttons = vec![
                    button(nodes, base + 1, if connected { t("Open Folder…") } else { t("Connect in Current Window") }, alias),
                    button(nodes, base + 2, t("Connect in New Window"), alias),
                ];
                if connected {
                    buttons.push(button(nodes, base + 3, t("Disconnect"), alias));
                }
                n.set_children(buttons);
                add_menu(nodes, &mut n, MenuRow::Host, i, alias);
            }
            RemoteRow::Folder { path, current, .. } => {
                n.set_label(crate::ui::file_name(path).to_string());
                n.set_description(path.clone());
                n.set_selected(*current);
                n.set_level(3);
                n.add_action(Action::Click);
            }
            RemoteRow::NoFolders { .. } => {
                n.set_label(t("No recent folders"));
                n.set_level(3);
            }
            RemoteRow::Empty { group, message } => {
                n.set_label(match (message, group) {
                    (Some(m), _) => m.clone(),
                    (None, Group::Ssh) => t("No hosts in ~/.ssh/config. Add one with +.").into(),
                    (None, Group::Containers) => t("No running containers.").into(),
                    (None, Group::Wsl) => t("No WSL distributions found.").into(),
                });
                n.set_level(2);
            }
            RemoteRow::Loading { .. } => {
                n.set_label(t("Loading…"));
                n.set_level(2);
            }
            RemoteRow::PortsHeader { count } => {
                n.set_label(t("Forwarded Ports"));
                n.set_description(count.to_string());
                n.set_level(1);
                n.add_action(Action::Click);
            }
            RemoteRow::Port(r) => {
                n.set_label(match &r.label {
                    Some(l) => tf("Port {0}, {1}", &[&r.port, l]),
                    None => tf("Port {}", &[&r.port]),
                });
                if let Some(a) = &r.address {
                    n.set_value(a.clone());
                }
                n.set_level(2);
            }
        }
        geometry.apply_row(RowFrame::Remote, i, &mut n);
        nodes.push((NodeId(base), n));
        ids.push(NodeId(base));
    }
    let mut tree = Node::new(Role::Tree);
    tree.set_label(t("Remote Explorer"));
    tree.set_children(ids);
    nodes.push((REMOTE, tree));
    REMOTE
}

impl MenuSnap {
    fn append_tree(&self, nodes: &mut Vec<(NodeId, Node)>) -> NodeId {
        let mut ids = Vec::new();
        for (i, (label, shortcut, enabled, checked, submenu)) in self.items.iter().enumerate() {
            let id = NodeId(MENU_ITEM + i as u64);
            let mut n = Node::new(if checked.is_some() { Role::MenuItemCheckBox } else { Role::MenuItem });
            n.set_label(label.clone());
            if let Some(k) = shortcut {
                n.set_description(k.clone());
            }
            if let Some(on) = checked {
                n.set_toggled(Toggled::from(*on));
            }
            if *submenu {
                n.set_has_popup(accesskit::HasPopup::Menu);
            }
            n.set_position_in_set(i + 1);
            n.set_size_of_set(self.items.len());
            if *enabled {
                n.add_action(Action::Click);
            } else {
                n.set_disabled();
            }
            nodes.push((id, n));
            ids.push(id);
        }
        let mut menu = Node::new(Role::Menu);
        menu.set_label(t("Context menu"));
        menu.set_children(ids);
        nodes.push((MENU, menu));
        MENU
    }
}

/// The node of row `index` of a Run and Debug, Testing or Remote Explorer list.
pub(in crate::workbench) fn row_node(kind: RowFrame, index: usize) -> Option<NodeId> {
    match kind {
        RowFrame::Test if index < MAX_TESTS => Some(NodeId(TEST_ROW + index as u64 * TEST_IDS)),
        RowFrame::Remote if index < MAX_REMOTE => Some(NodeId(REMOTE_ROW + index as u64 * REMOTE_IDS)),
        _ => debug_access::row_node(kind, index),
    }
}

impl SideSnap {
    pub(super) fn append_tree(&self, nodes: &mut Vec<(NodeId, Node)>, kids: &mut Vec<NodeId>, geometry: &Geometry) {
        self.debug.append_tree(nodes, kids, geometry);
        kids.extend(self.testing.as_ref().map(|s| s.append_tree(nodes, geometry)));
        kids.extend(self.remote.as_ref().map(|rows| remote_tree(rows, nodes, geometry)));
        kids.extend(self.menu.as_ref().map(|m| m.append_tree(nodes)));
    }

    /// While a menu is open it has the focus: its highlighted item, else the menu.
    pub(super) fn focus_node(&self) -> Option<NodeId> {
        self.menu.as_ref().map(|m| m.selected.filter(|i| *i < m.items.len()).map_or(MENU, |i| NodeId(MENU_ITEM + i as u64)))
    }
}

impl Workbench {
    pub(super) fn a11y_side(&self, cx: &App) -> SideSnap {
        let testing = (self.chrome.panel == Some(Panel::Testing)).then(|| self.a11y_testing(cx));
        let remote = (self.chrome.panel == Some(Panel::Remote)).then(|| {
            let mut rows = self.remote_rows(cx);
            rows.truncate(MAX_REMOTE);
            rows
        });
        SideSnap { debug: self.a11y_debug(cx), testing, remote, menu: self.a11y_menu(cx) }
    }

    fn a11y_menu(&self, cx: &App) -> Option<MenuSnap> {
        let state = self.menu.as_ref()?;
        let menu = state.menu.read(cx);
        let items = menu
            .items()
            .iter()
            .take(MAX_MENU)
            .map(|i| (i.label.to_string(), i.shortcut.as_ref().map(|s| s.to_string()), i.enabled, i.checked, i.submenu))
            .collect();
        Some(MenuSnap { entity: state.menu.entity_id(), items, selected: menu.selected() })
    }

    fn a11y_testing(&self, cx: &App) -> TestingSnap {
        let t = &self.jobs.tests;
        let filter = t.filter.as_ref().map(|f| f.read(cx).text().to_string()).unwrap_or_default();
        let running = t.run.is_some();
        let (passed, failed, skipped, total) = testexp::summary(&t.files, &t.results);
        let status = if t.scanning && t.files.is_empty() {
            crate::i18n::t("Looking for tests…").to_string()
        } else if self.root(cx).is_none() {
            crate::i18n::t("Open a folder to find its tests.").to_string()
        } else if t.files.is_empty() {
            crate::i18n::t("No tests found. Tests are read from Rust, Go, Python and JavaScript test files.").to_string()
        } else {
            // The same words as the view's summary line.
            let mut text = tf("{0}/{1} passed", &[&passed, &total]);
            if failed > 0 {
                text.push_str(&format!(" · {}", tf("{} failed", &[&failed])));
            }
            if skipped > 0 {
                text.push_str(&format!(" · {}", tf("{} skipped", &[&skipped])));
            }
            text
        };
        let rows = testexp::rows(&t.files, &t.results, &t.expanded, &filter, t.only_failed)
            .into_iter()
            .take(MAX_TESTS)
            .filter_map(|row| match row {
                Row::File { file, outcome, open } => {
                    let f = t.files.get(file)?;
                    Some(TestRow::File { rel: f.rel.clone(), outcome, open, count: f.tests.len() })
                }
                Row::Test { file, test, outcome, millis } => {
                    let f = t.files.get(file)?;
                    let item = f.tests.get(test)?;
                    Some(TestRow::Test { rel: f.rel.clone(), id: item.id.clone(), name: item.name.clone(), outcome, millis, debuggable: f.kind != Kind::Js })
                }
                Row::Message { file, test, text } => {
                    let f = t.files.get(file)?;
                    Some(TestRow::Message { rel: f.rel.clone(), id: f.tests.get(test)?.id.clone(), text })
                }
            })
            .collect();
        TestingSnap { running, only_failed: t.only_failed, status, rows }
    }

    /// The file and test indexes of a test the view still lists.
    fn test_index(&self, rel: &str, id: Option<&str>) -> Option<(usize, Option<usize>)> {
        let files = &self.jobs.tests.files;
        let fi = files.iter().position(|f| f.rel == rel)?;
        match id {
            None => Some((fi, None)),
            Some(id) => Some((fi, Some(files[fi].tests.iter().position(|x| x.id == id)?))),
        }
    }

    /// An action on a node of this module; `false` if the node is not one of them.
    pub(super) fn a11y_side_action(&mut self, req: &ActionRequest, last: &SideSnap, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.a11y_debug_action(req, &last.debug, window, cx) {
            return true;
        }
        let id = req.target_node;
        if FIXED[..7].contains(&id) || (TEST_ROW..TEST_ROW + BLOCK).contains(&id.0) {
            self.a11y_testing_action(req, last.testing.as_ref(), window, cx);
        } else if id == REMOTE || (REMOTE_ROW..REMOTE_ROW + BLOCK).contains(&id.0) || menu_request(req, MenuRow::Host, |_| None).is_some() {
            self.a11y_remote_action(req, last.remote.as_deref(), window, cx);
        } else if id == MENU || (MENU_ITEM..MENU_ITEM + BLOCK).contains(&id.0) {
            self.a11y_menu_action(req, last.menu.as_ref(), cx);
        } else {
            return false;
        }
        true
    }

    fn a11y_testing_action(&mut self, req: &ActionRequest, last: Option<&TestingSnap>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(old) = last.filter(|_| self.chrome.panel == Some(Panel::Testing)) else { return };
        let now = self.a11y_testing(cx);
        let click = req.action == Action::Click;
        match req.target_node {
            // Run All and Stop share the button: it does what the tree showed, or nothing.
            TESTS_RUN if click && now.running == old.running => {
                if now.running {
                    self.stop_tests(cx)
                } else {
                    self.run_all_tests(window, cx)
                }
            }
            TESTS_RUN_FAILED if click => self.run_failed_tests(window, cx),
            TESTS_REFRESH if click => self.refresh_tests(cx),
            TESTS_ONLY_FAILED if click && now.only_failed == old.only_failed => {
                self.jobs.tests.only_failed = !self.jobs.tests.only_failed;
                cx.notify();
            }
            NodeId(n) if n >= TEST_ROW => {
                let (index, part) = (((n - TEST_ROW) / TEST_IDS) as usize, (n - TEST_ROW) % TEST_IDS);
                let Some(row) = old.rows.get(index) else { return };
                match row {
                    TestRow::File { rel, open, .. } if part == 0 && wants_toggle(req.action, *open) => {
                        // Still listed, and still open or closed as the tree said.
                        if now.rows.iter().any(|r| matches!(r, TestRow::File { rel: r2, open: o, .. } if r2 == rel && o == open)) {
                            let expanded = &mut self.jobs.tests.expanded;
                            if !expanded.remove(rel) {
                                expanded.insert(rel.clone());
                            }
                            cx.notify();
                        }
                    }
                    TestRow::File { rel, .. } if part == 1 && click => {
                        if let Some((fi, _)) = self.test_index(rel, None) {
                            self.run_test_item(fi, None, window, cx);
                        }
                    }
                    TestRow::Test { rel, id, debuggable, .. } if click => {
                        let Some((fi, Some(ti))) = self.test_index(rel, Some(id)) else { return };
                        match part {
                            0 => self.open_test(fi, ti, false, window, cx),
                            1 => self.run_test_item(fi, Some(ti), window, cx),
                            2 if *debuggable => {
                                let t = &self.jobs.tests;
                                let (Some(root), Some(f)) = (t.root.clone(), t.files.get(fi).cloned()) else { return };
                                let Some(item) = f.tests.get(ti).cloned() else { return };
                                self.debug_test(f, item, root, window, cx);
                            }
                            _ => {}
                        }
                    }
                    TestRow::Message { rel, id, .. } if click && part == 0 => {
                        if let Some((fi, Some(ti))) = self.test_index(rel, Some(id)) {
                            self.open_test(fi, ti, true, window, cx);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn a11y_remote_action(&mut self, req: &ActionRequest, last: Option<&[RemoteRow]>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(old) = last.filter(|_| self.chrome.panel == Some(Panel::Remote)) else { return };
        let row_index =
            |id: u64| (REMOTE_ROW..REMOTE_ROW + BLOCK).contains(&id).then(|| (id - REMOTE_ROW) / REMOTE_IDS).filter(|i| i * REMOTE_IDS + REMOTE_ROW == id);
        if let Some(i) = menu_request(req, MenuRow::Host, |id| row_index(id).map(|i| i as usize)) {
            // The same host at the same place, still connected or not as the tree showed it (the menu offers Disconnect
            // only while connected).
            let now = self.remote_rows(cx);
            if let Some(RemoteRow::Host { alias, group, .. }) = old.get(i).filter(|r| now.get(i) == Some(*r)) {
                let at = self.row_menu_anchor(MenuRow::Host, i, window);
                self.open_host_menu(alias, *group, at, window, cx);
            }
            return;
        }
        if req.target_node.0 < REMOTE_ROW {
            return;
        }
        let n = req.target_node.0 - REMOTE_ROW;
        let (index, part) = ((n / REMOTE_IDS) as usize, n % REMOTE_IDS);
        let Some(row) = old.get(index) else { return };
        let now = self.remote_rows(cx);
        let click = req.action == Action::Click;
        match row {
            RemoteRow::Group { group, expanded, .. } if part == 0 && wants_toggle(req.action, *expanded) => {
                if now.iter().any(|r| matches!(r, RemoteRow::Group { group: g, expanded: e, .. } if g == group && e == expanded)) {
                    self.toggle_remote(group.key().to_string(), cx);
                }
            }
            RemoteRow::Host { alias, expanded, status, .. } => {
                // The same host, still connected or not as the tree showed it.
                let connected = *status == HostStatus::Connected;
                let Some(RemoteRow::Host { expanded: open, status: s, .. }) = now.iter().find(|r| matches!(r, RemoteRow::Host { alias: a, .. } if a == alias))
                else {
                    return;
                };
                if (*s == HostStatus::Connected) != connected || connected != self.connected_to(alias) {
                    return;
                }
                match part {
                    0 if open == expanded && wants_toggle(req.action, *expanded) => self.toggle_remote(remote_glue::tree::host_key(alias), cx),
                    1 if click => self.run_remote_menu(RemoteMenuCmd::Connect, alias.clone(), window, cx),
                    2 if click => self.run_remote_menu(RemoteMenuCmd::ConnectNewWindow, alias.clone(), window, cx),
                    3 if click && connected => self.run_remote_menu(RemoteMenuCmd::Disconnect, alias.clone(), window, cx),
                    _ => {}
                }
            }
            RemoteRow::Folder { alias, path, last_opened, .. } if click && part == 0 => {
                if now.iter().any(|r| matches!(r, RemoteRow::Folder { alias: a, path: p, .. } if a == alias && p == path)) {
                    let recent = state::RecentWorkspace { host: Some(alias.clone()), path: path.clone(), last_opened_at: *last_opened };
                    self.open_recent(recent, window, cx);
                }
            }
            RemoteRow::PortsHeader { .. } if click && part == 0 => self.show_ports(&ShowPorts, window, cx),
            _ => {}
        }
    }

    /// A menu item: chosen through the menu itself, only in the same menu and only if the item is still the same.
    fn a11y_menu_action(&mut self, req: &ActionRequest, last: Option<&MenuSnap>, cx: &mut Context<Self>) {
        let (Some(old), Some(state)) = (last, self.menu.as_ref()) else { return };
        let menu = state.menu.clone();
        if req.target_node.0 < MENU_ITEM || menu.entity_id() != old.entity || req.action != Action::Click {
            return;
        }
        let i = (req.target_node.0 - MENU_ITEM) as usize;
        let Some(item) = old.items.get(i) else { return };
        let same = self.a11y_menu(cx).is_some_and(|now| now.items.get(i) == Some(item));
        if same {
            menu.update(cx, |m, cx| m.activate(i, cx));
        }
    }
}

#[cfg(test)]
#[path = "a11y_side_views_tests.rs"]
mod tests;
