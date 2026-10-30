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
        let is_repo = (SCM_REPO..SCM_REPO + BLOCK).contains(&id.0);
        let is_scm = [SCM_MESSAGE, SCM_COMMIT].contains(&id) || (SCM_GROUP..SCM_FILE + BLOCK).contains(&id.0) || is_repo;
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
