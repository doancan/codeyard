//! Extension views' AccessKit nodes: every element with the role and label `ext_ui::Node::access` gives it, so authors
//! write no accessibility code. Layout containers and decorative icons are not announced (their children are). Rows of
//! lists, tables and trees are listed up to `MAX_OPTIONS` around the keyboard row, with their position in the whole
//! set. Actions are checked against the view on screen before they run.

use ext_ui::{Element, InputState, Key, KeyPath, Kind, PageToken, Role as UiRole};

use super::*;
use crate::workbench::ext_ui_glue::tree_rows;

/// Nodes one view may have in the tree (a view may have 50,000 elements; screen readers need far fewer).
const MAX_NODES: usize = 20_000;
/// Ids per view inside `EXT_VIEW_NODE`'s block.
const PER_VIEW: u64 = 0x4_0000;

/// What a node's action does.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum Act {
    None,
    /// Focus it (inputs, selects, row containers).
    Focus(KeyPath),
    Click(KeyPath),
    Toggle(KeyPath),
    Link(KeyPath),
    /// A row of the list, table or tree at the path.
    Row(KeyPath, Key),
    /// The "Load more" row of a tree level.
    More(KeyPath, Option<Key>, PageToken),
}

#[derive(Clone, PartialEq, Debug)]
pub(crate) struct NodeSnap {
    pub role: UiRole,
    pub label: String,
    pub description: Option<String>,
    pub children: Vec<usize>,
    pub act: Act,
    pub checked: Option<bool>,
    pub selected: Option<bool>,
    pub expanded: Option<bool>,
    pub value: Option<String>,
    /// Tree item level (1 = top), or a row's position and the size of its set.
    pub level: Option<usize>,
    pub position: Option<(usize, usize)>,
    pub percent: Option<u8>,
    pub disabled: bool,
    pub focused: bool,
}

impl NodeSnap {
    fn new(role: UiRole, label: String) -> Self {
        NodeSnap {
            role,
            label,
            description: None,
            children: Vec::new(),
            act: Act::None,
            checked: None,
            selected: None,
            expanded: None,
            value: None,
            level: None,
            position: None,
            percent: None,
            disabled: false,
            focused: false,
        }
    }
}

/// One view on screen: a group named by its title and publisher, then its elements (`nodes[0]` is the group).
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct ViewSnap {
    pub id: String,
    pub nodes: Vec<NodeSnap>,
}

impl ViewSnap {
    pub(crate) fn node_id(slot: usize, i: usize) -> NodeId {
        NodeId(EXT_VIEW_NODE + slot as u64 * PER_VIEW + i as u64)
    }

    /// Add this view's nodes; returns the id of its group.
    pub(super) fn append(&self, slot: usize, nodes: &mut Vec<(NodeId, Node)>) -> NodeId {
        for (i, n) in self.nodes.iter().enumerate() {
            let mut node = Node::new(role(n.role));
            if !n.label.is_empty() {
                node.set_label(n.label.clone());
            }
            if let Some(d) = &n.description {
                node.set_description(d.clone());
            }
            if let UiRole::Heading(level) = n.role {
                node.set_level(level as usize);
            }
            if let Some(level) = n.level {
                node.set_level(level);
            }
            if let Some((pos, size)) = n.position {
                node.set_position_in_set(pos);
                node.set_size_of_set(size);
            }
            if let Some(c) = n.checked {
                node.set_toggled(if c { Toggled::True } else { Toggled::False });
            }
            if let Some(s) = n.selected {
                node.set_selected(s);
            }
            if let Some(e) = n.expanded {
                node.set_expanded(e);
            }
            if let Some(v) = &n.value {
                node.set_value(v.clone());
            }
            if let Some(pc) = n.percent {
                node.set_numeric_value(pc as f64);
                node.set_min_numeric_value(0.);
                node.set_max_numeric_value(100.);
            }
            if n.disabled {
                node.set_disabled();
            }
            if n.role == UiRole::Alert {
                node.set_live(Live::Assertive);
            }
            match &n.act {
                Act::None => {}
                Act::Focus(_) => node.add_action(Action::Focus),
                _ if n.disabled => {}
                _ => {
                    node.add_action(Action::Click);
                    node.add_action(Action::Focus);
                }
            }
            node.set_children(n.children.iter().map(|&c| Self::node_id(slot, c)).collect::<Vec<_>>());
            nodes.push((Self::node_id(slot, i), node));
        }
        Self::node_id(slot, 0)
    }
}

fn role(r: UiRole) -> Role {
    match r {
        UiRole::Hidden | UiRole::Group => Role::Group,
        UiRole::Label => Role::Label,
        UiRole::Heading(_) => Role::Heading,
        UiRole::Image => Role::Image,
        UiRole::Button => Role::Button,
        UiRole::TextInput => Role::TextInput,
        UiRole::MultilineTextInput => Role::MultilineTextInput,
        UiRole::CheckBox => Role::CheckBox,
        UiRole::ComboBox => Role::ComboBox,
        UiRole::List => Role::List,
        UiRole::ListItem => Role::ListItem,
        UiRole::Tree => Role::Tree,
        UiRole::TreeItem => Role::TreeItem,
        UiRole::Table => Role::Table,
        UiRole::Row => Role::Row,
        UiRole::Cell => Role::Cell,
        UiRole::Status => Role::Status,
        UiRole::Alert => Role::Alert,
        UiRole::ProgressIndicator => Role::ProgressIndicator,
        UiRole::Document => Role::Document,
        UiRole::Code => Role::Code,
        UiRole::DescriptionList => Role::DescriptionList,
        UiRole::Link => Role::Link,
        UiRole::Separator => Role::Splitter,
    }
}

/// Builds one view's nodes.
struct Build<'a> {
    h: &'a super::ext_views_glue::HostedView,
    window: &'a Window,
    nodes: Vec<NodeSnap>,
}

impl Build<'_> {
    fn push(&mut self, n: NodeSnap) -> Option<usize> {
        (self.nodes.len() < MAX_NODES).then(|| {
            self.nodes.push(n);
            self.nodes.len() - 1
        })
    }

    fn focused(&self, path: &KeyPath) -> bool {
        self.h.focus.get(path).is_some_and(|f| f.is_focused(self.window))
    }

    /// `n` and what is below it; returns the indices to attach to the parent (a hidden node passes its children up).
    fn node(&mut self, n: &ext_ui::Node, path: &KeyPath, ancestors: &mut Vec<Kind>) -> Vec<usize> {
        let mut path = path.clone();
        if let Some(k) = &n.key {
            path.push(k.clone());
        }
        let a = n.access(ancestors);
        let st = self.h.state.get(&path);
        let mut s = NodeSnap::new(a.role, a.label);
        s.description = a.description;
        s.focused = self.focused(&path);
        let keyed = n.key.is_some();
        match &n.element {
            Element::Button { disabled, .. } if keyed => {
                s.act = Act::Click(path.clone());
                s.disabled = *disabled;
            }
            Element::TextInput { disabled, .. } if keyed => {
                s.value = st.map(|x| x.text.clone());
                s.act = Act::Focus(path.clone());
                s.disabled = *disabled;
            }
            Element::Checkbox { disabled, .. } if keyed => {
                s.checked = st.map(|x| x.checked);
                s.act = Act::Toggle(path.clone());
                s.disabled = *disabled;
            }
            Element::Select { options, disabled, .. } if keyed => {
                let chosen = st.and_then(|x| x.choice.as_ref());
                s.value = chosen.and_then(|k| options.iter().find(|o| &o.key == k)).map(|o| o.label.clone());
                s.act = Act::Focus(path.clone());
                s.disabled = *disabled;
            }
            Element::Link { .. } if keyed => s.act = Act::Link(path.clone()),
            Element::Progress { percent, .. } => s.percent = *percent,
            _ => {}
        }
        if n.kind().has_rows() && ancestors.last() != Some(&Kind::Tree) {
            s.act = Act::Focus(path.clone());
            let Some(me) = self.push(s) else { return Vec::new() };
            let rows = self.rows(n, &path, st, ancestors);
            self.nodes[me].children = rows;
            return vec![me];
        }
        if a.role == UiRole::Hidden && n.children.is_empty() {
            return Vec::new();
        }
        let hidden = a.role == UiRole::Hidden;
        let me = if hidden { None } else { self.push(s) };
        ancestors.push(n.kind());
        let mut kids = Vec::new();
        for c in &n.children {
            kids.extend(self.node(c, &path, ancestors));
        }
        ancestors.pop();
        match me {
            Some(me) => {
                self.nodes[me].children = kids;
                vec![me]
            }
            None => kids,
        }
    }

    /// The rows of a list, table or tree, at most `MAX_OPTIONS` of them around the keyboard row.
    fn rows(&mut self, n: &ext_ui::Node, path: &KeyPath, st: Option<&InputState>, ancestors: &mut Vec<Kind>) -> Vec<usize> {
        let cursor = self.h.cursor.get(path).copied().unwrap_or(0);
        let mut out = Vec::new();
        ancestors.push(n.kind());
        if n.kind() == Kind::Tree {
            let rows = tree_rows(n, st);
            let start = cursor.saturating_sub(MAX_OPTIONS / 2).min(rows.len().saturating_sub(MAX_OPTIONS));
            for (i, r) in rows.iter().enumerate().skip(start).take(MAX_OPTIONS) {
                let mut s = match (&r.more, n.get(&r.at)) {
                    (Some((item, page)), _) => {
                        let mut s = NodeSnap::new(UiRole::Button, t("Load more").to_string());
                        s.act = Act::More(path.clone(), item.clone(), page.clone());
                        s
                    }
                    (None, Some(item)) => {
                        let a = item.access(&[Kind::Tree]);
                        let mut s = NodeSnap::new(UiRole::TreeItem, a.label);
                        s.description = a.description;
                        let key = item.key.clone().unwrap_or_else(|| Key::new(""));
                        let expandable = !item.children.is_empty() || matches!(item.element, Element::Tree { more: Some(_), .. });
                        if expandable {
                            s.expanded = Some(st.is_some_and(|x| x.expanded.contains(&key)));
                        }
                        s.selected = Some(st.is_some_and(|x| x.selected.contains(&key)));
                        s.act = Act::Row(path.clone(), key);
                        s
                    }
                    (None, None) => continue,
                };
                s.level = Some(r.depth + 1);
                s.position = Some((i + 1, rows.len()));
                s.focused = i == cursor && self.focused(path);
                if let Some(ix) = self.push(s) {
                    out.push(ix);
                }
            }
        } else {
            let total = n.children.len();
            let start = cursor.saturating_sub(MAX_OPTIONS / 2).min(total.saturating_sub(MAX_OPTIONS));
            for (i, row) in n.children.iter().enumerate().skip(start).take(MAX_OPTIONS) {
                let a = row.access(ancestors);
                let key = row.key.clone().unwrap_or_else(|| Key::new(""));
                let mut s = NodeSnap::new(a.role, a.label);
                s.selected = Some(st.is_some_and(|x| x.selected.contains(&key)));
                s.position = Some((i + 1, total));
                s.focused = i == cursor && self.focused(path);
                s.act = Act::Row(path.clone(), key.clone());
                let Some(me) = self.push(s) else { break };
                if n.kind() == Kind::Table {
                    ancestors.push(Kind::Row);
                    let cells: Vec<usize> = row.children.iter().filter_map(|c| self.push(NodeSnap::new(UiRole::Cell, c.access(ancestors).label))).collect();
                    ancestors.pop();
                    self.nodes[me].children = cells;
                }
                out.push(me);
            }
        }
        ancestors.pop();
        out
    }
}

impl Workbench {
    /// The extension views on screen: the side bar's (primary or secondary), those in windows of their own, the bottom
    /// panel's and the editor page's.
    pub(in crate::workbench) fn a11y_ext_views(&self, window: &Window) -> Vec<ViewSnap> {
        let views = &self.cfg.exts.views;
        let mut shown = Vec::new();
        if self.chrome.panel == Some(Panel::ExtView) || self.chrome.secondary == Some(Panel::ExtView) {
            shown.extend(views.sidebar.clone());
        }
        shown.extend(self.ext_views_in_windows());
        if self.chrome.panel_open && self.chrome.bottom == Bottom::ExtView {
            shown.extend(views.bottom.clone());
        }
        if let (Some(Page::ExtView(id)), true) = (&self.tabs.page, self.tabs.page_active) {
            shown.push(id.clone());
        }
        shown
            .into_iter()
            .filter_map(|id| {
                let h = self.ext_view(&id)?;
                let d = &h.decl;
                let mut b = Build { h, window, nodes: Vec::new() };
                b.push(NodeSnap::new(UiRole::Group, tf("{0}, by {1}", &[&d.title, &d.publisher()])));
                let kids = match (&h.error, &h.tree) {
                    (Some(e), _) => b.push(NodeSnap::new(UiRole::Alert, tf("{0} could not draw this view: {1}", &[&d.extension, e]))).into_iter().collect(),
                    (None, None) => b.push(NodeSnap::new(UiRole::Status, t("Loading…").to_string())).into_iter().collect(),
                    (None, Some(tree)) => b.node(tree, &Vec::new(), &mut Vec::new()),
                };
                b.nodes[0].children = kids;
                Some(ViewSnap { id, nodes: b.nodes })
            })
            .collect()
    }

    /// An action on an extension view's node. True when the node is one of theirs.
    pub(super) fn a11y_ext_view_action(&mut self, req: &ActionRequest, last: &Snapshot, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let id = req.target_node.0;
        if !(EXT_VIEW_NODE..EXT_VIEW_NODE + BLOCK).contains(&id) {
            return false;
        }
        let (slot, i) = (((id - EXT_VIEW_NODE) / PER_VIEW) as usize, ((id - EXT_VIEW_NODE) % PER_VIEW) as usize);
        let Some(view) = last.ext_views.get(slot) else { return true };
        let Some(was) = view.nodes.get(i) else { return true };
        // The node must still be there, saying the same thing.
        let now = self.a11y_ext_views(window);
        if now.iter().find(|v| v.id == view.id).and_then(|v| v.nodes.get(i)).map(|n| (&n.label, &n.act)) != Some((&was.label, &was.act)) {
            return true;
        }
        let vid = view.id.clone();
        let focus = |this: &Self, path: &KeyPath, window: &mut Window| {
            if let Some(f) = this.ext_view(&vid).and_then(|h| h.focus.get(path)) {
                window.focus(f);
            }
        };
        match (req.action, was.act.clone()) {
            (Action::Focus, Act::Focus(p) | Act::Click(p) | Act::Toggle(p) | Act::Link(p)) => focus(self, &p, window),
            (Action::Click, Act::Focus(p)) => focus(self, &p, window),
            (Action::Click, Act::Click(p)) => self.ext_view_click(&vid, p, window, cx),
            (Action::Click, Act::Toggle(p)) => self.ext_view_toggle(&vid, p, window, cx),
            (Action::Click, Act::Link(p)) => self.ext_view_link(&vid, p, window, cx),
            (Action::Click | Action::Focus, Act::Row(p, key)) => {
                focus(self, &p, window);
                if req.action == Action::Click {
                    self.ext_view_select_row(&vid, p, key, false, window, cx);
                }
            }
            (Action::Click, Act::More(p, item, page)) => self.ext_view_load_more(&vid, p, item, page, window, cx),
            _ => {}
        }
        true
    }
}
