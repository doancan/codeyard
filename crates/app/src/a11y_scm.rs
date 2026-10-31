//! Source Control's AccessKit nodes and guarded actions. Git work stays in the existing ScmPanel operations.
//!
//! The change list is a list holding the groups of its exposed range (`scm::ACCESS_LINES` lines around the
//! cursor or the rows on screen). A group is a section with its header button; a file is a button that opens its diff,
//! followed by one button to stage it (to mark a conflict resolved) or unstage it and, for an unstaged change, one to
//! discard it behind the painted button's question, and a "More Actions…" button that opens the menu a right click opens
//! (as `ShowContextMenu` on the file does; the native adapters do not forward that action yet). Ids: group `g` at `SCM_GROUP + 2g` (its header one above), file `k`
//! of the range at `SCM_FILE + 4k` (its actions above it). An action applies only to the same repository and to the
//! same file in the same state as when the tree was read.

use super::*;
use crate::scm::{AccessFile, ChangeKey};

#[path = "a11y_scm_ext.rs"]
pub(super) mod ext;

const STAGE: u64 = 1;
const DISCARD: u64 = 2;
const MENU: u64 = 3;

fn group_id(g: usize) -> NodeId {
    NodeId(SCM_GROUP + g as u64 * 2)
}

fn file_id(k: usize) -> NodeId {
    NodeId(SCM_FILE + k as u64 * 4)
}

impl ScmSnap {
    /// The node of the cursor row while it is in the exposed range, else the list itself.
    pub(super) fn focus_node(&self) -> NodeId {
        let Some(cursor) = self.view.cursor else { return SCM_CHANGES };
        let mut k = 0;
        for (g, group) in self.view.groups.iter().enumerate() {
            if group.header_shown && group.line == cursor {
                return NodeId(group_id(g).0 + 1);
            }
            for file in &group.files {
                if file.line == cursor {
                    return file_id(k);
                }
                k += 1;
            }
        }
        SCM_CHANGES
    }

    /// The file of the exposed range a node id names, and which of its nodes it is.
    fn file(&self, id: NodeId) -> Option<(&AccessFile, u64)> {
        let off = id.0.checked_sub(SCM_FILE).filter(|o| *o < BLOCK)?;
        let file = self.view.groups.iter().flat_map(|g| &g.files).nth((off / 4) as usize)?;
        Some((file, off % 4))
    }

    pub(super) fn append_tree(&self, nodes: &mut Vec<(NodeId, Node)>) {
        let view = &self.view;
        let mut panel = Node::new(Role::Group);
        panel.set_label(t("Source Control"));
        if view.busy {
            panel.set_busy();
        }
        let mut children = Vec::new();
        if !view.repos.is_empty() {
            // Several repositories: a list to pick the one Source Control acts on, each with its state.
            let mut list = Node::new(Role::ListBox);
            list.set_label(t("Repositories"));
            let mut items = Vec::new();
            for (i, repo) in view.repos.iter().enumerate() {
                let id = NodeId(SCM_REPO + i as u64);
                let mut option = Node::new(Role::ListBoxOption);
                option.set_label(repo.label.clone());
                let mut description = format!("{} · {}", repo.summary, repo.path);
                if let Some((text, _)) = &repo.result {
                    description = format!("{description} · {text}");
                }
                option.set_description(description);
                option.set_selected(repo.selected);
                option.set_position_in_set(i + 1);
                option.set_size_of_set(view.repos.len());
                if repo.busy {
                    option.set_busy();
                }
                option.add_action(Action::Click);
                option.add_action(Action::Focus);
                nodes.push((id, option));
                items.push(id);
            }
            list.set_children(items);
            nodes.push((SCM_REPOS, list));
            children.push(SCM_REPOS);
        }
        if view.root.is_some() {
            let mut branch = Node::new(Role::Label);
            branch.set_value(view.branch.clone());
            nodes.push((SCM_BRANCH, branch));
            children.push(SCM_BRANCH);

            let mut message = Node::new(Role::MultilineTextInput);
            message.set_label(t("Message (↵ to commit)"));
            message.set_value(view.message.clone());
            message.add_action(Action::Focus);
            if view.busy {
                message.set_disabled();
            } else {
                message.add_action(Action::SetValue);
            }
            nodes.push((SCM_MESSAGE, message));
            children.push(SCM_MESSAGE);

            let mut commit = Node::new(Role::Button);
            commit.set_label(view.commit_label.clone());
            if view.can_commit {
                commit.add_action(Action::Click)
            } else {
                commit.set_disabled();
                if !view.busy {
                    commit.set_description(t("Nothing to commit"));
                }
            }
            nodes.push((SCM_COMMIT, commit));
            children.push(SCM_COMMIT);

            self.append_changes(nodes);
            children.push(SCM_CHANGES);
        } else if view.ext.is_empty() {
            let mut empty = Node::new(Role::Label);
            empty.set_value(t("This folder is not a git repository."));
            nodes.push((SCM_EMPTY, empty));
            children.push(SCM_EMPTY);
        }
        ext::append(&view.ext, nodes, &mut children);
        if let Some((text, error)) = &view.result {
            let mut result = Node::new(if *error { Role::Alert } else { Role::Status });
            result.set_label(text.clone());
            result.set_live(if *error { Live::Assertive } else { Live::Polite });
            result.set_live_atomic();
            nodes.push((SCM_RESULT, result));
            children.push(SCM_RESULT);
        }
        panel.set_children(children);
        nodes.push((SCM, panel));
    }

    fn append_changes(&self, nodes: &mut Vec<(NodeId, Node)>) {
        let view = &self.view;
        let mut list = Node::new(Role::List);
        list.set_label(t("Changed files"));
        set_scroll(&mut list, view.lines_first, view.lines_shown, view.lines_total);
        list.add_action(Action::Focus);
        let mut sections = Vec::new();
        let mut k = 0;
        for (g, group) in view.groups.iter().enumerate() {
            let id = group_id(g);
            let mut section = Node::new(Role::Group);
            section.set_label(group.label.clone());
            let mut rows = Vec::new();
            if group.header_shown {
                let header_id = NodeId(id.0 + 1);
                let mut header = Node::new(Role::Button);
                header.set_label(format!("{} ({})", group.label, group.total));
                header.set_expanded(group.expanded);
                header.add_action(Action::Click);
                header.add_action(Action::Focus);
                nodes.push((header_id, header));
                rows.push(header_id);
            }
            for file in &group.files {
                let path = &file.change.path;
                let open_id = file_id(k);
                k += 1;
                let mut open = Node::new(Role::Button);
                open.set_label(path.clone());
                open.set_description(file.mark().describe());
                open.set_position_in_set(file.position);
                open.set_size_of_set(group.total);
                open.add_action(Action::Click);
                open.add_action(Action::Focus);
                open.add_action(Action::ShowContextMenu);
                nodes.push((open_id, open));
                rows.push(open_id);
                let mut actions = vec![(
                    STAGE,
                    if file.conflict() {
                        tf("Mark {} as resolved", &[path])
                    } else if file.staged {
                        tf("Unstage {}", &[path])
                    } else {
                        tf("Stage {}", &[path])
                    },
                )];
                if file.discardable() {
                    actions.push((DISCARD, tf("Discard changes in {}", &[path])));
                }
                for (part, label) in actions {
                    let mut button = Node::new(Role::Button);
                    button.set_label(label);
                    if view.busy {
                        button.set_disabled()
                    } else {
                        button.add_action(Action::Click)
                    }
                    nodes.push((NodeId(open_id.0 + part), button));
                    rows.push(NodeId(open_id.0 + part));
                }
                let mut more = Node::new(Role::Button);
                more.set_label(t("More Actions…"));
                more.set_description(path.clone());
                more.set_has_popup(accesskit::HasPopup::Menu);
                more.add_action(Action::Click);
                nodes.push((NodeId(open_id.0 + MENU), more));
                rows.push(NodeId(open_id.0 + MENU));
            }
            section.set_children(rows);
            nodes.push((id, section));
            sections.push(id);
        }
        list.set_children(sections);
        nodes.push((SCM_CHANGES, list));
    }
}

impl Workbench {
    pub(super) fn a11y_scm_action(&mut self, req: &ActionRequest, last: &Snapshot, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.a11y_scm_ext_action(req, last, window, cx) {
            return true;
        }
        let id = req.target_node;
        let is_repo = (SCM_REPO..SCM_REPO + BLOCK).contains(&id.0);
        let is_scm = [SCM_MESSAGE, SCM_COMMIT, SCM_CHANGES].contains(&id) || (SCM_GROUP..SCM_FILE + BLOCK).contains(&id.0) || is_repo;
        if !is_scm {
            return false;
        }
        let Some(snap) = &last.scm else { return true };
        if !self.view_shown(Panel::Scm) || snap.generation != self.lsp.generation {
            return true;
        }
        let current = self.scm.read(cx).a11y_view(window, cx);
        if is_repo {
            // The option names a repository by its identity: if the inventory changed since the tree was read, the
            // action applies to that same repository or to nothing.
            let Some(repo) = snap.view.repos.get((id.0 - SCM_REPO) as usize) else { return true };
            if matches!(req.action, Action::Click | Action::Focus) && current.repos.iter().any(|r| r.key == repo.key && r.path == repo.path) {
                self.select_repository(repo.key, cx);
                self.scm.update(cx, |s, cx| s.focus_repositories(window, cx));
            }
            return true;
        }

        // Another repository (or none) since the tree was read: nothing of the old one is acted on.
        if snap.view.root != current.root || snap.view.branch != current.branch || current.root.is_none() {
            return true;
        }
        match (req.action, id) {
            (Action::Focus, SCM_MESSAGE) => self.scm.read(cx).focus_message(window, cx),
            (Action::SetValue, SCM_MESSAGE) if !current.busy => {
                if let Some(ActionData::Value(text)) = &req.data {
                    self.scm.update(cx, |s, cx| s.a11y_set_message(text, cx));
                }
            }
            (Action::Click, SCM_COMMIT) if current.can_commit && snap.view.revision == current.revision && snap.view.message == current.message => {
                self.scm.update(cx, |s, cx| s.commit(cx));
            }
            (Action::Focus, SCM_CHANGES) => {
                self.scm.update(cx, |s, cx| s.focus_changes(None, window, cx));
            }
            (Action::ScrollUp | Action::ScrollDown, SCM_CHANGES) => {
                let pages = if req.action == Action::ScrollUp { -1 } else { 1 };
                self.scm.update(cx, |s, cx| s.a11y_scroll_changes(pages, cx));
            }
            (Action::Click | Action::Focus, id) if (SCM_GROUP..SCM_GROUP + BLOCK).contains(&id.0) && !(id.0 - SCM_GROUP).is_multiple_of(2) => {
                let Some(group) = snap.view.groups.get(((id.0 - SCM_GROUP) / 2) as usize) else { return true };
                let scm = self.scm.read(cx);
                // The group must still be listed; a press folds it only if it is still as it was read (a replayed
                // press would undo the first one).
                if !scm.a11y_has_group(group.group) {
                    return true;
                }
                if req.action == Action::Focus {
                    self.scm.update(cx, |s, cx| s.focus_changes(Some(&ChangeKey::Group(group.group)), window, cx));
                } else if scm.a11y_group_open(group.group) == group.expanded {
                    self.scm.update(cx, |s, cx| s.a11y_toggle_group(group.id, cx));
                }
            }
            (action, id) if (SCM_FILE..SCM_FILE + BLOCK).contains(&id.0) => {
                let Some((file, part)) = snap.file(id) else { return true };
                if !self.scm.read(cx).a11y_has_file(file) {
                    return true;
                }
                let file = file.clone();
                match (action, part) {
                    (Action::Focus, 0) => {
                        self.scm.update(cx, |s, cx| s.focus_changes(Some(&file.key()), window, cx));
                    }
                    (Action::Click, 0) => {
                        self.scm.update(cx, |_, cx| cx.emit(crate::scm::ScmEvent::OpenDiff { path: file.change.path.clone(), staged: file.staged }));
                    }
                    (Action::Click, STAGE) if !current.busy => self.scm.update(cx, |s, cx| s.a11y_stage_toggle(&file, cx)),
                    (Action::Click, DISCARD) if !current.busy => self.scm.update(cx, |s, cx| s.a11y_discard(&file, window, cx)),
                    // The menu opens under the row with the list focused on it, so closing it gives the list the keys.
                    (Action::ShowContextMenu, 0) | (Action::Click, MENU) => self.scm.update(cx, |s, cx| {
                        if s.focus_changes(Some(&file.key()), window, cx) {
                            s.open_row_menu(Some(&file.key()), window, cx);
                        }
                    }),
                    _ => {}
                }
            }
            _ => {}
        }
        true
    }
}
