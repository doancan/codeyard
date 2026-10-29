//! Source Control's AccessKit nodes and guarded actions. Git work stays in the existing ScmPanel operations.

use super::*;

impl ScmSnap {
    pub(super) fn append_tree(&self, nodes: &mut Vec<(NodeId, Node)>) {
        let view = &self.view;
        let mut panel = Node::new(Role::Group);
        panel.set_label(t("Source Control"));
        if view.busy {
            panel.set_busy();
        }
        let mut children = Vec::new();
        if !view.repos.is_empty() {
            let list_id = NodeId(SCM_REPO);
            let mut list = Node::new(Role::List);
            list.set_label(t("Repositories"));
            let mut rows = Vec::new();
            for (i, repo) in view.repos.iter().enumerate() {
                let id = NodeId(SCM_REPO + 1 + i as u64);
                let mut row = Node::new(Role::ListItem);
                row.set_label(repo.label.clone());
                row.set_description(repo.description());
                row.set_selected(repo.selected);
                row.set_position_in_set(i + 1);
                row.set_size_of_set(view.repos.len());
                row.add_action(Action::Click);
                nodes.push((id, row));
                rows.push(id);
            }
            list.set_children(rows);
            nodes.push((list_id, list));
            children.push(list_id);
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
                commit.set_disabled()
            }
            nodes.push((SCM_COMMIT, commit));
            children.push(SCM_COMMIT);

            let mut file_index = 0;
            for (i, group) in view.groups.iter().enumerate() {
                let id = NodeId(SCM_GROUP + i as u64 * 2);
                let mut section = Node::new(Role::Group);
                section.set_label(group.label.clone());
                let header_id = NodeId(id.0 + 1);
                let mut header = Node::new(Role::Button);
                header.set_label(group.label.clone());
                header.set_expanded(group.expanded);
                header.add_action(Action::Click);
                nodes.push((header_id, header));
                let mut rows = vec![header_id];
                for (index, file) in group.files.iter().enumerate() {
                    let open_id = NodeId(SCM_FILE + file_index * 2);
                    let stage_id = NodeId(open_id.0 + 1);
                    file_index += 1;
                    let mut open = Node::new(Role::Button);
                    open.set_label(file.change.path.clone());
                    open.set_description(file.mark().describe());
                    open.set_position_in_set(index + 1);
                    open.set_size_of_set(group.total);
                    open.add_action(Action::Click);
                    nodes.push((open_id, open));
                    rows.push(open_id);
                    let mut stage = Node::new(Role::Button);
                    stage.set_label(if file.staged { tf("Unstage {}", &[&file.change.path]) } else { tf("Stage {}", &[&file.change.path]) });
                    if view.busy {
                        stage.set_disabled()
                    } else {
                        stage.add_action(Action::Click)
                    }
                    nodes.push((stage_id, stage));
                    rows.push(stage_id);
                }
                section.set_children(rows);
                nodes.push((id, section));
                children.push(id);
            }
        } else {
            let mut empty = Node::new(Role::Label);
            empty.set_value(t("This folder is not a git repository."));
            nodes.push((SCM_EMPTY, empty));
            children.push(SCM_EMPTY);
        }
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
}

impl Workbench {
    pub(super) fn a11y_scm_action(&mut self, req: &ActionRequest, last: &Snapshot, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let id = req.target_node;
        let is_scm = [SCM_MESSAGE, SCM_COMMIT].contains(&id) || (SCM_GROUP..SCM_FILE + BLOCK).contains(&id.0);
        if !is_scm {
            return false;
        }
        let Some(snap) = &last.scm else { return true };
        if self.chrome.panel != Some(Panel::Scm) || snap.generation != self.lsp.generation {
            return true;
        }
        let current = self.scm.read(cx).a11y_view(window, cx);
        // A repository row: picked only if that repository is still listed (the list may have changed since it was read).
        if (SCM_REPO + 1..SCM_REPO + 1 + crate::git::repos::MAX_REPOS as u64).contains(&id.0) {
            let index = (id.0 - SCM_REPO - 1) as usize;
            if req.action == Action::Click
                && let Some(repo) = snap.view.repos.get(index)
                && current.repos.iter().any(|r| r.root == repo.root)
            {
                self.select_repo(repo.root.clone(), cx);
            }
            return true;
        }
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
            (Action::Click, id) if (SCM_GROUP..SCM_GROUP + BLOCK).contains(&id.0) && !(id.0 - SCM_GROUP).is_multiple_of(2) => {
                let i = ((id.0 - SCM_GROUP) / 2) as usize;
                if let Some(group) = snap.view.groups.get(i)
                    && current.groups.iter().any(|g| g.id == group.id && g.expanded == group.expanded)
                {
                    self.scm.update(cx, |s, cx| s.a11y_toggle_group(group.id, cx));
                }
            }
            (Action::Click, id) if (SCM_FILE..SCM_FILE + BLOCK).contains(&id.0) && !current.busy => {
                let index = ((id.0 - SCM_FILE) / 2) as usize;
                let Some(file) = snap.view.groups.iter().flat_map(|g| &g.files).nth(index) else { return true };
                if !self.scm.read(cx).a11y_has_file(file) || !current.groups.iter().any(|g| g.files.contains(file)) {
                    return true;
                }
                self.scm.update(cx, |s, cx| {
                    if (id.0 - SCM_FILE).is_multiple_of(2) {
                        cx.emit(crate::scm::ScmEvent::OpenDiff { path: file.change.path.clone(), staged: file.staged });
                    } else if file.staged {
                        s.unstage(vec![file.change.path.clone()], cx);
                    } else {
                        s.stage(vec![file.change.path.clone()], cx);
                    }
                });
            }
            _ => {}
        }
        true
    }
}
