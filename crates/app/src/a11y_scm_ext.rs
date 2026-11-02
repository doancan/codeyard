//! Source Control's AccessKit nodes for folders an extension's provider claims (#144): per folder a group with its
//! head, input box, accept button and message, then its groups of files, each file a button with one button per
//! action, as git's rows are. Actions go through the same panel methods as clicks.

use super::*;
use crate::scm::AccessExt;

/// Ids of the providers' sections: 16 folders of 0x1_0000 ids each.
pub(in crate::workbench) const SCM_EXT: u64 = 0x140_0000;
const PER_SOURCE: u64 = 0x1_0000;
const SOURCES: usize = (BLOCK / PER_SOURCE) as usize;
const HEAD: u64 = 1;
const MESSAGE: u64 = 2;
const ACCEPT: u64 = 3;
const NOTE: u64 = 4;
/// Group `g`: its section at `GROUP + 2g`, its header button one above.
const GROUP: u64 = 0x10;
const MAX_GROUPS: usize = 0x70;
/// File `k` of the folder: its button at `FILE + 4k`, its first three actions above it.
const FILE: u64 = 0x100;
const ACTIONS: usize = 3;
const MAX_FILES: usize = ((PER_SOURCE - FILE) / 4) as usize;

fn base(s: usize) -> u64 {
    SCM_EXT + s as u64 * PER_SOURCE
}

/// What a node of the block is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Message,
    Accept,
    Group(usize),
    /// File index over the folder's shown groups, and 0 to open it or 1.. for an action.
    File(usize, usize),
    Other,
}

fn part(id: NodeId) -> Option<(usize, Part)> {
    let off = id.0.checked_sub(SCM_EXT).filter(|o| *o < BLOCK)?;
    let (s, n) = ((off / PER_SOURCE) as usize, off % PER_SOURCE);
    let p = match n {
        MESSAGE => Part::Message,
        ACCEPT => Part::Accept,
        n if (GROUP..FILE).contains(&n) && n % 2 == 1 => Part::Group(((n - GROUP) / 2) as usize),
        n if n >= FILE => Part::File(((n - FILE) / 4) as usize, ((n - FILE) % 4) as usize),
        _ => Part::Other,
    };
    Some((s, p))
}

pub(super) fn append(list: &[AccessExt], nodes: &mut Vec<(NodeId, Node)>, children: &mut Vec<NodeId>) {
    for (s, src) in list.iter().take(SOURCES).enumerate() {
        let b = base(s);
        let mut section = Node::new(Role::Group);
        section.set_label(src.title.clone());
        if src.busy {
            section.set_busy();
        }
        let mut kids = Vec::new();
        let state = src.state.as_deref();
        if let Some(head) = state.and_then(|st| st.head.clone()) {
            let mut n = Node::new(Role::Label);
            n.set_value(head);
            nodes.push((NodeId(b + HEAD), n));
            kids.push(NodeId(b + HEAD));
        }
        if let Some(accept) = state.and_then(|st| st.input.accept.as_ref()) {
            let mut message = Node::new(Role::MultilineTextInput);
            let placeholder = state.map(|st| st.input.placeholder.clone()).unwrap_or_default();
            message.set_label(if placeholder.is_empty() { t("Message").to_string() } else { placeholder });
            message.set_value(src.message.clone());
            message.add_action(Action::Focus);
            if src.busy {
                message.set_disabled();
            } else {
                message.add_action(Action::SetValue);
            }
            nodes.push((NodeId(b + MESSAGE), message));
            kids.push(NodeId(b + MESSAGE));
            let mut button = Node::new(Role::Button);
            button.set_label(accept.title.clone());
            if src.busy || src.validation.is_some() {
                button.set_disabled();
            } else {
                button.add_action(Action::Click);
            }
            nodes.push((NodeId(b + ACCEPT), button));
            kids.push(NodeId(b + ACCEPT));
        }
        let note = src.validation.clone().or(src.error.clone()).or_else(|| state.is_none().then(|| t("Asking the source control provider…").to_string()));
        if let Some(text) = note {
            let mut n = Node::new(if src.error.is_some() { Role::Alert } else { Role::Status });
            n.set_label(text);
            n.set_live(Live::Polite);
            nodes.push((NodeId(b + NOTE), n));
            kids.push(NodeId(b + NOTE));
        }
        let mut file = 0;
        for (g, group) in state.iter().flat_map(|st| crate::scm::ext_shown_groups(st)).take(MAX_GROUPS).enumerate() {
            let id = NodeId(b + GROUP + g as u64 * 2);
            let mut sec = Node::new(Role::Group);
            sec.set_label(group.label.clone());
            let mut header = Node::new(Role::Button);
            header.set_label(format!("{} ({})", group.label, group.resources.len()));
            let open = src.open.contains(&group.id);
            header.set_expanded(open);
            header.add_action(Action::Click);
            nodes.push((NodeId(id.0 + 1), header));
            let mut rows = vec![NodeId(id.0 + 1)];
            if open {
                for (i, r) in group.resources.iter().enumerate() {
                    if file >= MAX_FILES {
                        break;
                    }
                    let fid = b + FILE + file as u64 * 4;
                    file += 1;
                    let mut row = Node::new(Role::Button);
                    row.set_label(crate::scm::ext_relative(&src.root, &r.uri));
                    row.set_description(match &r.detail {
                        Some(d) => format!("{} · {d}", crate::scm::describe_ext_state(r.state)),
                        None => crate::scm::describe_ext_state(r.state).to_string(),
                    });
                    row.set_position_in_set(i + 1);
                    row.set_size_of_set(group.resources.len());
                    row.add_action(Action::Click);
                    nodes.push((NodeId(fid), row));
                    rows.push(NodeId(fid));
                    for (k, a) in group.resource_actions.iter().take(ACTIONS).enumerate() {
                        let mut button = Node::new(Role::Button);
                        button.set_label(format!("{} {}", a.title, crate::scm::ext_relative(&src.root, &r.uri)));
                        if src.busy {
                            button.set_disabled();
                        } else {
                            button.add_action(Action::Click);
                        }
                        nodes.push((NodeId(fid + 1 + k as u64), button));
                        rows.push(NodeId(fid + 1 + k as u64));
                    }
                }
            }
            sec.set_children(rows);
            nodes.push((id, sec));
            kids.push(id);
        }
        section.set_children(kids);
        nodes.push((NodeId(b), section));
        children.push(NodeId(b));
    }
}

impl Workbench {
    /// An action on a provider's node: applied only while that folder shows the same state as when the tree was read.
    pub(super) fn a11y_scm_ext_action(&mut self, req: &ActionRequest, last: &Snapshot, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((s, part)) = part(req.target_node) else { return false };
        let Some(snap) = &last.scm else { return true };
        let Some(was) = snap.view.ext.get(s) else { return true };
        if !self.view_shown(Panel::Scm) || snap.generation != self.lsp.generation {
            return true;
        }
        let now = self.scm.read(cx).a11y_view(window, cx);
        let Some(src) = now.ext.iter().find(|e| e.root == was.root) else { return true };
        let (Some(state), true) = (src.state.clone(), src.state == was.state) else { return true };
        let root = src.root.clone();
        match (req.action, part) {
            (Action::Focus, Part::Message) => self.scm.read(cx).focus_ext_message(&root, window, cx),
            (Action::SetValue, Part::Message) if !src.busy => {
                if let Some(ActionData::Value(text)) = &req.data {
                    self.scm.update(cx, |p, cx| p.set_ext_message(&root, text, cx));
                }
            }
            (Action::Click, Part::Accept) if !src.busy && src.message == was.message => self.scm.update(cx, |p, cx| p.ext_accept(&root, cx)),
            (Action::Click, Part::Group(g)) => {
                if let Some(group) = crate::scm::ext_shown_groups(&state).nth(g) {
                    let id = group.id.clone();
                    self.scm.update(cx, |p, cx| p.toggle_ext_group(&root, &id, cx));
                }
            }
            (Action::Click, Part::File(k, j)) if !src.busy => {
                let shown = crate::scm::ext_shown_groups(&state).filter(|g| src.open.contains(&g.id));
                let Some((group, r)) = shown.flat_map(|g| g.resources.iter().map(move |r| (g, r))).nth(k) else { return true };
                if j == 0 {
                    let (root, uri) = (root.clone(), r.uri.clone());
                    self.on_ext_scm_event(crate::scm::ExtEvent::Open { root, uri }, window, cx);
                } else if let Some(a) = group.resource_actions.get(j - 1) {
                    let uris = vec![r.uri.clone()];
                    self.scm.update(cx, |p, cx| p.ext_run(&root, a, uris, cx));
                }
            }
            _ => {}
        }
        true
    }
}
