//! The Settings page, the Keyboard Shortcuts page and the Ports panel for assistive technology. Rows are bounded and
//! every action goes through the pointer's own path (the page's events, `forward_port`, `stop_forward`). An action names
//! its row by what it is (setting key, command, server port and its forward), never only by position, so an action from
//! an outdated tree does nothing when that row changed or went away.

use super::row_menus::{MenuRow, add_menu, menu_request};
use super::*;
use crate::settings_page::{AccessControl, AccessPage, SettingsEvent};
use crate::workbench::ports_glue::{RowState, port_rows};
use serde_json::Value;

const SETTINGS: NodeId = NodeId(54);
const SETTINGS_SEARCH: NodeId = NodeId(55);
const SETTINGS_CATEGORIES: NodeId = NodeId(56);
const SETTINGS_LIST: NodeId = NodeId(57);
const SETTINGS_EMPTY: NodeId = NodeId(58);
const SETTINGS_JSON: NodeId = NodeId(59);
const SETTINGS_KEYS: NodeId = NodeId(60);
const SETTINGS_ICONS: NodeId = NodeId(61);
const SHORTCUTS: NodeId = NodeId(62);
const SHORTCUTS_SEARCH: NodeId = NodeId(63);
const SHORTCUTS_LIST: NodeId = NodeId(64);
const PORTS: NodeId = NodeId(65);
const PORTS_FIELD: NodeId = NodeId(66);
const PORTS_EMPTY: NodeId = NodeId(67);
/// This module's fixed nodes, for the id layout check.
pub(in crate::workbench) const FIXED: [NodeId; 14] = [
    SETTINGS,
    SETTINGS_SEARCH,
    SETTINGS_CATEGORIES,
    SETTINGS_LIST,
    SETTINGS_EMPTY,
    SETTINGS_JSON,
    SETTINGS_KEYS,
    SETTINGS_ICONS,
    SHORTCUTS,
    SHORTCUTS_SEARCH,
    SHORTCUTS_LIST,
    PORTS,
    PORTS_FIELD,
    PORTS_EMPTY,
];

/// The block of this module's rows; inside it: categories, settings (`ROW_IDS` ids each: the control, Reset, then one
/// per option), shortcuts, ports (`PORT_IDS` ids each: the row, then its buttons).
pub(in crate::workbench) const PAGE_ROW: u64 = 0x160_0000;
const SET_CATEGORY: u64 = PAGE_ROW;
const SET_ROW: u64 = PAGE_ROW + 0x100;
const ROW_IDS: u64 = 16;
const SHORTCUT_ROW: u64 = PAGE_ROW + 0x2000;
const PORT_ROW: u64 = PAGE_ROW + 0x4000;
const PORT_IDS: u64 = 8;
/// Rows exposed at most (the counts say how many there are).
const MAX_SETTINGS: usize = 256;
const MAX_SHORTCUTS: usize = 2_000;
const MAX_PORTS: usize = 500;
const _: () = {
    assert!(SET_CATEGORY + 32 <= SET_ROW);
    assert!(SET_ROW + MAX_SETTINGS as u64 * ROW_IDS <= SHORTCUT_ROW);
    assert!(SHORTCUT_ROW + MAX_SHORTCUTS as u64 <= PORT_ROW);
    assert!(PORT_ROW + MAX_PORTS as u64 * PORT_IDS <= PAGE_ROW + BLOCK);
};

/// The Settings page in front, and which page it is (a reopened page is another one).
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct SettingsSnap {
    entity: gpui::EntityId,
    view: AccessPage,
}

/// The Keyboard Shortcuts page in front: search text, rows (command, key), how many match, search focused.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct ShortcutsSnap {
    entity: gpui::EntityId,
    query: String,
    rows: Vec<(String, Option<String>)>,
    total: usize,
    focused: bool,
}

/// A row of the Ports panel and the forward it shows (`None`: listening, not forwarded).
#[derive(Clone, PartialEq, Debug)]
struct PortSnap {
    port: u16,
    forward: Option<crate::port_forwards::ForwardId>,
    label: String,
    state: String,
    detail: String,
    url: Option<String>,
    address: Option<String>,
    listening_only: bool,
}

/// The Ports panel while it is shown: its rows, or why there are none.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct PortsSnap {
    rows: Vec<PortSnap>,
    empty: Option<String>,
    field: Option<(String, bool)>,
}

fn button(nodes: &mut Vec<(NodeId, Node)>, id: NodeId, label: &str, description: Option<String>) -> NodeId {
    let mut b = Node::new(Role::Button);
    b.set_label(label.to_string());
    if let Some(d) = description {
        b.set_description(d);
    }
    b.add_action(Action::Click);
    nodes.push((id, b));
    id
}

/// The text of a `SetValue` request (none: the request changes nothing).
fn text_of(req: &ActionRequest) -> Option<&str> {
    match &req.data {
        Some(ActionData::Value(v)) => Some(v),
        _ => None,
    }
}

fn search_field(nodes: &mut Vec<(NodeId, Node)>, id: NodeId, label: &str, value: &str) -> NodeId {
    let mut n = Node::new(Role::SearchInput);
    n.set_label(label.to_string());
    n.set_value(value.to_string());
    n.add_action(Action::Focus);
    n.add_action(Action::SetValue);
    nodes.push((id, n));
    id
}

impl SettingsSnap {
    fn append_tree(&self, nodes: &mut Vec<(NodeId, Node)>) -> NodeId {
        let v = &self.view;
        let mut kids = vec![search_field(nodes, SETTINGS_SEARCH, t("Search settings"), &v.query)];
        let mut cats = Node::new(Role::TabList);
        cats.set_label(t("Categories"));
        let mut cat_ids = Vec::new();
        for (i, (label, count, selected)) in v.categories.iter().enumerate() {
            let id = NodeId(SET_CATEGORY + i as u64);
            let mut n = Node::new(Role::Tab);
            n.set_label(label.to_string());
            n.set_description(count.to_string());
            n.set_selected(*selected);
            n.add_action(Action::Click);
            nodes.push((id, n));
            cat_ids.push(id);
        }
        cats.set_children(cat_ids);
        nodes.push((SETTINGS_CATEGORIES, cats));
        kids.push(SETTINGS_CATEGORIES);
        if let Some(why) = v.empty {
            let mut empty = Node::new(Role::Status);
            empty.set_label(why.to_string());
            empty.set_live(Live::Polite);
            nodes.push((SETTINGS_EMPTY, empty));
            kids.push(SETTINGS_EMPTY);
        } else {
            let mut list = Node::new(Role::Group);
            list.set_label(v.heading.clone());
            let mut ids = Vec::new();
            for (i, row) in v.rows.iter().enumerate() {
                let base = SET_ROW + i as u64 * ROW_IDS;
                let mut n = Node::new(match row.control {
                    AccessControl::Theme(_) => Role::Button,
                    AccessControl::Toggle(_) => Role::CheckBox,
                    AccessControl::Choice(..) => Role::RadioGroup,
                    AccessControl::Field(_) => Role::TextInput,
                });
                n.set_label(row.title.clone());
                let mut about = vec![row.about.clone()];
                about.extend(row.source.map(str::to_string));
                if let Some(error) = &row.error {
                    about.push(error.clone());
                    n.set_invalid(accesskit::Invalid::True);
                }
                n.set_description(about.join(". "));
                match &row.control {
                    AccessControl::Theme(name) => {
                        n.set_value(name.clone());
                        n.add_action(Action::Click);
                    }
                    AccessControl::Toggle(on) => {
                        n.set_toggled(Toggled::from(*on));
                        n.add_action(Action::Click);
                    }
                    AccessControl::Choice(options, chosen) => {
                        let mut opts = Vec::new();
                        for (k, o) in options.iter().enumerate() {
                            let id = NodeId(base + 2 + k as u64);
                            let mut b = Node::new(Role::RadioButton);
                            b.set_label(o.to_string());
                            b.set_toggled(Toggled::from(chosen.as_deref() == Some(*o)));
                            b.add_action(Action::Click);
                            nodes.push((id, b));
                            opts.push(id);
                        }
                        n.set_children(opts);
                    }
                    AccessControl::Field(text) => {
                        n.set_value(text.clone());
                        n.add_action(Action::Focus);
                        n.add_action(Action::SetValue);
                    }
                }
                nodes.push((NodeId(base), n));
                ids.push(NodeId(base));
                if row.modified {
                    ids.push(button(nodes, NodeId(base + 1), t("Reset"), Some(row.title.clone())));
                }
            }
            list.set_children(ids);
            nodes.push((SETTINGS_LIST, list));
            kids.push(SETTINGS_LIST);
        }
        kids.push(button(nodes, SETTINGS_JSON, t("Open settings.jsonc"), None));
        kids.push(button(nodes, SETTINGS_KEYS, t("Keyboard Shortcuts"), None));
        kids.push(button(nodes, SETTINGS_ICONS, t("File icon theme…"), None));
        let mut page = Node::new(Role::Group);
        page.set_label(t("Settings"));
        page.set_children(kids);
        nodes.push((SETTINGS, page));
        SETTINGS
    }

    fn focus_node(&self) -> NodeId {
        match self.view.focused {
            Some(None) => SETTINGS_SEARCH,
            Some(Some(key)) => self.view.rows.iter().position(|r| r.key == key).map_or(SETTINGS, |i| NodeId(SET_ROW + i as u64 * ROW_IDS)),
            None => SETTINGS,
        }
    }
}

impl ShortcutsSnap {
    fn append_tree(&self, nodes: &mut Vec<(NodeId, Node)>) -> NodeId {
        let search = search_field(nodes, SHORTCUTS_SEARCH, t("Search commands or keys (e.g. “comment”, “⌘D”)"), &self.query);
        let mut list = Node::new(Role::List);
        list.set_label(tn("{} command", "{} commands", self.total, &[]));
        list.set_size_of_set(self.total);
        let mut ids = Vec::new();
        for (i, (label, keys)) in self.rows.iter().enumerate() {
            let id = NodeId(SHORTCUT_ROW + i as u64);
            let mut n = Node::new(Role::ListItem);
            n.set_label(label.clone());
            n.set_description(keys.clone().unwrap_or_else(|| t("No shortcut").to_string()));
            n.set_position_in_set(i + 1);
            n.set_size_of_set(self.total);
            n.add_action(Action::Click);
            nodes.push((id, n));
            ids.push(id);
        }
        list.set_children(ids);
        nodes.push((SHORTCUTS_LIST, list));
        let mut page = Node::new(Role::Group);
        page.set_label(t("Keyboard Shortcuts"));
        page.set_children(vec![search, SHORTCUTS_LIST]);
        nodes.push((SHORTCUTS, page));
        SHORTCUTS
    }
}

impl PortsSnap {
    fn append_tree(&self, nodes: &mut Vec<(NodeId, Node)>, geometry: &Geometry) -> NodeId {
        let mut kids = Vec::new();
        if let Some((text, _)) = &self.field {
            let mut field = Node::new(Role::TextInput);
            field.set_label(t("Forward a port:"));
            field.set_value(text.clone());
            field.add_action(Action::Focus);
            nodes.push((PORTS_FIELD, field));
            kids.push(PORTS_FIELD);
        }
        if let Some(why) = &self.empty {
            let mut empty = Node::new(Role::Status);
            empty.set_label(why.clone());
            empty.set_live(Live::Polite);
            nodes.push((PORTS_EMPTY, empty));
            kids.push(PORTS_EMPTY);
        }
        for (i, row) in self.rows.iter().enumerate() {
            let base = PORT_ROW + i as u64 * PORT_IDS;
            let mut n = Node::new(Role::ListItem);
            n.set_label(row.label.clone());
            n.set_value(row.state.clone());
            if !row.detail.is_empty() {
                n.set_description(row.detail.clone());
            }
            n.set_position_in_set(i + 1);
            n.set_size_of_set(self.rows.len());
            let mut buttons = Vec::new();
            let mut add = |k: u64, label: &str| buttons.push(button(nodes, NodeId(base + k), label, Some(row.label.clone())));
            if row.url.is_some() {
                add(1, t("Open in Browser"));
            }
            if row.listening_only {
                add(4, t("Forward Port"));
            } else {
                if row.address.is_some() {
                    add(2, t("Copy Address"));
                }
                add(3, t("Stop Forwarding"));
            }
            n.set_children(buttons);
            add_menu(nodes, &mut n, MenuRow::Port, i, &row.label);
            geometry.apply_row(RowFrame::Port, i, &mut n);
            nodes.push((NodeId(base), n));
        }
        let mut list = Node::new(Role::List);
        list.set_label(t("Ports"));
        kids.extend((0..self.rows.len()).map(|i| NodeId(PORT_ROW + i as u64 * PORT_IDS)));
        list.set_children(kids);
        nodes.push((PORTS, list));
        PORTS
    }
}

/// The node of row `index` of the Ports list.
pub(in crate::workbench) fn port_node(index: usize) -> Option<NodeId> {
    (index < MAX_PORTS).then(|| NodeId(PORT_ROW + index as u64 * PORT_IDS))
}

/// The pages and panel of this module in one place, kept in [`ViewsSnap`].
#[derive(Clone, PartialEq, Debug, Default)]
pub(crate) struct PagesSnap {
    settings: Option<SettingsSnap>,
    shortcuts: Option<ShortcutsSnap>,
    ports: Option<PortsSnap>,
}

impl PagesSnap {
    pub(super) fn append_tree(&self, nodes: &mut Vec<(NodeId, Node)>, kids: &mut Vec<NodeId>, geometry: &Geometry) {
        kids.extend(self.settings.as_ref().map(|s| s.append_tree(nodes)));
        kids.extend(self.shortcuts.as_ref().map(|s| s.append_tree(nodes)));
        kids.extend(self.ports.as_ref().map(|p| p.append_tree(nodes, geometry)));
    }

    /// The focused field of these views, if one has focus.
    pub(super) fn focus_node(&self) -> Option<NodeId> {
        let settings = self.settings.as_ref().filter(|s| s.view.focused.is_some()).map(SettingsSnap::focus_node);
        let shortcuts = self.shortcuts.as_ref().filter(|s| s.focused).map(|_| SHORTCUTS_SEARCH);
        let ports = self.ports.as_ref().filter(|p| p.field.as_ref().is_some_and(|f| f.1)).map(|_| PORTS_FIELD);
        settings.or(shortcuts).or(ports)
    }
}

impl Workbench {
    pub(super) fn a11y_pages(&self, window: &Window, cx: &App) -> PagesSnap {
        let page = self.tabs.page.as_ref().filter(|_| self.tabs.page_active);
        let settings = match page {
            Some(Page::Settings(v)) => Some(SettingsSnap { entity: v.entity_id(), view: v.read(cx).access_view(MAX_SETTINGS, window, cx) }),
            _ => None,
        };
        let shortcuts = match page {
            Some(Page::Shortcuts(v)) => {
                let (query, rows, total, focused) = v.read(cx).access_rows(MAX_SHORTCUTS, window, cx);
                Some(ShortcutsSnap { entity: v.entity_id(), query, rows, total, focused })
            }
            _ => None,
        };
        let ports = (self.chrome.panel_open && self.chrome.bottom == Bottom::Ports).then(|| self.a11y_ports(window, cx));
        PagesSnap { settings, shortcuts, ports }
    }

    fn a11y_ports(&self, window: &Window, cx: &App) -> PortsSnap {
        let Some(alias) = self.ssh_alias() else {
            return PortsSnap { rows: Vec::new(), empty: Some(t("Port forwarding works while connected to an SSH host.").into()), field: None };
        };
        let field = Some((self.ports.field.read(cx).text().to_string(), self.ports.field.focus_handle(cx).is_focused(window)));
        let all = port_rows(&self.ports.set, &self.ports.listening, &alias);
        let empty = all.is_empty().then(|| {
            let mut why = t("Nothing is listening on the server yet.").to_string();
            if let Some(e) = &self.ports.scan_error {
                why = format!("{why} {e}");
            }
            why
        });
        let rows = all
            .iter()
            .take(MAX_PORTS)
            .map(|r| {
                let state = match &r.state {
                    RowState::Active => r.address.clone().unwrap_or_default(),
                    RowState::Pending => t("forwarding…").into(),
                    RowState::Interrupted => t("waiting for the connection").into(),
                    RowState::Failed(why) => why.clone(),
                    RowState::Listening => t("Not forwarded").into(),
                };
                let origin = match r.origin {
                    Some(crate::port_forwards::Origin::Auto) => Some(t("Auto Forwarded").to_string()),
                    Some(crate::port_forwards::Origin::User) => Some(t("User Forwarded").to_string()),
                    None => None,
                };
                PortSnap {
                    port: r.port,
                    forward: self.ports.set.by_remote(&alias, r.port).map(|f| f.id),
                    label: match &r.label {
                        Some(l) => tf("Port {0}, {1}", &[&r.port, l]),
                        None => tf("Port {}", &[&r.port]),
                    },
                    state,
                    detail: r.process.iter().cloned().chain(origin).collect::<Vec<_>>().join(", "),
                    url: r.url(),
                    address: r.address.clone().filter(|_| r.state == RowState::Active),
                    listening_only: r.state == RowState::Listening,
                }
            })
            .collect();
        PortsSnap { rows, empty, field }
    }

    /// An action on a node of this module; `false` if the node is not one of them.
    pub(super) fn a11y_pages_action(&mut self, req: &ActionRequest, last: &PagesSnap, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let id = req.target_node;
        let settings_node = [SETTINGS_SEARCH, SETTINGS_JSON, SETTINGS_KEYS, SETTINGS_ICONS].contains(&id) || (SET_CATEGORY..SHORTCUT_ROW).contains(&id.0);
        if settings_node {
            self.a11y_settings_action(req, last.settings.as_ref(), window, cx);
        } else if id == SHORTCUTS_SEARCH || (SHORTCUT_ROW..PORT_ROW).contains(&id.0) {
            self.a11y_shortcuts_action(req, last.shortcuts.as_ref(), window, cx);
        } else if id == PORTS_FIELD || (PORT_ROW..PAGE_ROW + BLOCK).contains(&id.0) || menu_request(req, MenuRow::Port, |_| None).is_some() {
            self.a11y_ports_action(req, last.ports.as_ref(), window, cx);
        } else {
            return false;
        }
        true
    }

    fn a11y_settings_action(&mut self, req: &ActionRequest, last: Option<&SettingsSnap>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Page::Settings(page)) = self.tabs.page.clone().filter(|_| self.tabs.page_active) else { return };
        let Some(last) = last.filter(|s| s.entity == page.entity_id()) else { return };
        let (id, value) = (req.target_node, text_of(req));
        let emit = |event: SettingsEvent, cx: &mut Context<Self>| page.update(cx, |_, cx| cx.emit(event));
        match (req.action, id) {
            (Action::Focus, SETTINGS_SEARCH) => page.read(cx).focus_search(window, cx),
            (Action::SetValue, SETTINGS_SEARCH) if let Some(text) = value => page.update(cx, |p, cx| p.set_query(text, cx)),
            (Action::Click, SETTINGS_JSON) => emit(SettingsEvent::OpenJson, cx),
            (Action::Click, SETTINGS_KEYS) => emit(SettingsEvent::OpenKeybindings, cx),
            (Action::Click, SETTINGS_ICONS) => emit(SettingsEvent::PickIconTheme, cx),
            (Action::Click, id) if (SET_CATEGORY..SET_ROW).contains(&id.0) => page.update(cx, |p, cx| p.select_category((id.0 - SET_CATEGORY) as usize, cx)),
            (action, id) if id.0 >= SET_ROW => {
                let (index, part) = (((id.0 - SET_ROW) / ROW_IDS) as usize, (id.0 - SET_ROW) % ROW_IDS);
                let Some(old) = last.view.rows.get(index) else { return };
                // The same setting must still be on the page, showing what the old tree showed.
                let now = page.read(cx).access_view(MAX_SETTINGS, window, cx);
                let Some(row) = now.rows.iter().find(|r| r.key == old.key) else { return };
                let key = row.key.to_string();
                match (action, part, &row.control) {
                    (Action::Click, 1, _) if row.modified => emit(SettingsEvent::Write(key, Value::Null), cx),
                    (Action::Click, 0, AccessControl::Theme(_)) => emit(SettingsEvent::PickTheme, cx),
                    (Action::Click, 0, AccessControl::Toggle(on)) if old.control == row.control => emit(SettingsEvent::Write(key, Value::Bool(!on)), cx),
                    (Action::Click, k, AccessControl::Choice(options, _)) if k >= 2 && matches!(&old.control, AccessControl::Choice(o, _) if o == options) => {
                        if let Some(o) = options.get(k as usize - 2) {
                            emit(SettingsEvent::Write(key, Value::String(o.to_string())), cx);
                        }
                    }
                    (Action::Focus, 0, AccessControl::Field(_)) => page.read(cx).focus_field(&key, window, cx),
                    (Action::SetValue, 0, AccessControl::Field(_)) if let Some(text) = value => page.update(cx, |p, cx| p.submit_field(&key, text, cx)),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn a11y_shortcuts_action(&mut self, req: &ActionRequest, last: Option<&ShortcutsSnap>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Page::Shortcuts(page)) = self.tabs.page.clone().filter(|_| self.tabs.page_active) else { return };
        let Some(last) = last.filter(|s| s.entity == page.entity_id()) else { return };
        match (req.action, req.target_node) {
            (Action::Focus, SHORTCUTS_SEARCH) => page.read(cx).focus_search(window, cx),
            (Action::SetValue, SHORTCUTS_SEARCH) if let Some(text) = text_of(req) => page.update(cx, |p, cx| p.set_query(text, cx)),
            (Action::Click, id) if id.0 >= SHORTCUT_ROW => {
                // Run only a command the page still lists (the same one, not whatever moved into its place).
                let Some((label, _)) = last.rows.get((id.0 - SHORTCUT_ROW) as usize) else { return };
                let (_, now, _, _) = page.read(cx).access_rows(usize::MAX, window, cx);
                if now.iter().any(|(l, _)| l == label) {
                    let label = label.clone();
                    page.update(cx, |_, cx| cx.emit(crate::shortcuts_page::ShortcutsEvent::Run(label)));
                }
            }
            _ => {}
        }
    }

    fn a11y_ports_action(&mut self, req: &ActionRequest, last: Option<&PortsSnap>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(last) = last else { return };
        if !(self.chrome.panel_open && self.chrome.bottom == Bottom::Ports) {
            return;
        }
        if req.target_node == PORTS_FIELD {
            if req.action == Action::Focus && self.ssh_alias().is_some() {
                window.focus(&self.ports.field.focus_handle(cx));
            }
            return;
        }
        let row_index = |id: u64| (id >= PORT_ROW && (id - PORT_ROW).is_multiple_of(PORT_IDS)).then(|| ((id - PORT_ROW) / PORT_IDS) as usize);
        if let Some(i) = menu_request(req, MenuRow::Port, row_index) {
            self.a11y_port_menu(last.rows.get(i), window, cx);
            return;
        }
        let (index, part) = (((req.target_node.0 - PORT_ROW) / PORT_IDS) as usize, (req.target_node.0 - PORT_ROW) % PORT_IDS);
        let Some(old) = last.rows.get(index) else { return };
        // The row of the same server port, still showing the same forward.
        let now = self.a11y_ports(window, cx);
        let Some(row) = now.rows.iter().find(|r| r.port == old.port && r.forward == old.forward) else { return };
        if req.action != Action::Click {
            return;
        }
        match part {
            1 if row.url.is_some() && row.url == old.url => cx.open_url(row.url.as_deref().unwrap_or_default()),
            2 if row.address.is_some() && row.address == old.address => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(row.address.clone().unwrap_or_default()))
            }
            3 if row.forward.is_some() => self.stop_forward(row.port, cx),
            4 if row.listening_only => self.forward_port(row.port, cx),
            _ => {}
        }
    }

    /// The row's menu, as a right click on it opens it: only for the same server port, still showing the same forward
    /// in the same state (the menu's items depend on it).
    fn a11y_port_menu(&mut self, old: Option<&PortSnap>, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(old), Some(alias)) = (old, self.ssh_alias()) else { return };
        let now = self.a11y_ports(window, cx);
        let Some(i) = now.rows.iter().position(|r| r == old) else { return };
        let rows = port_rows(&self.ports.set, &self.ports.listening, &alias);
        if let Some(row) = rows.get(i).filter(|r| r.port == old.port) {
            let at = self.row_menu_anchor(MenuRow::Port, i, window);
            self.open_port_menu(row, at, window, cx);
        }
    }
}

#[cfg(test)]
#[path = "a11y_pages_tests.rs"]
mod tests;
