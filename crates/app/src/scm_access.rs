//! The accessible Source Control model uses the same state and operations as the painted panel.

use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AccessFile {
    pub change: Change,
    pub staged: bool,
}

impl AccessFile {
    pub fn mark(&self) -> Mark {
        if self.staged { self.change.staged } else { self.change.unstaged }.unwrap_or(Mark::Modified)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AccessGroup {
    pub id: &'static str,
    pub label: String,
    pub expanded: bool,
    pub total: usize,
    pub files: Vec<AccessFile>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AccessView {
    pub revision: Option<Arc<Status>>,
    pub root: Option<String>,
    pub branch: String,
    pub message: String,
    pub message_focused: bool,
    pub busy: bool,
    pub commit_label: String,
    pub can_commit: bool,
    pub result: Option<(String, bool)>,
    pub groups: Vec<AccessGroup>,
}

impl ScmPanel {
    pub(super) fn commit_state(&self, status: &Status, cx: &App) -> (String, bool) {
        let staged = status.staged().count();
        let changed = status.unstaged().filter(|c| c.unstaged != Some(Mark::Untracked)).count();
        let smart = cx.global::<crate::settings::Settings>().git_smart_commit;
        if smart || staged > 0 { commit_button(staged, changed) } else { commit_button(0, 0) }
    }

    pub(crate) fn a11y_view(&self, window: &Window, cx: &App) -> AccessView {
        let mut groups = Vec::new();
        if let Some(status) = &self.status {
            for (id, label, staged, conflict) in [
                ("scm-merge", crate::i18n::t("Merge Changes"), false, true),
                ("scm-staged", crate::i18n::t("Staged Changes"), true, false),
                ("scm-changes", crate::i18n::t("Changes"), false, false),
            ] {
                let matches = |c: &&Change| if staged { c.staged.is_some() } else { c.unstaged.is_some() && (c.unstaged == Some(Mark::Conflict)) == conflict };
                let total = status.changes.iter().filter(matches).count();
                if total == 0 && id != "scm-changes" {
                    continue;
                }
                let expanded = self.sections.is_open(id, true);
                let files = if expanded {
                    status.changes.iter().filter(matches).take(5000).cloned().map(|change| AccessFile { change, staged }).collect()
                } else {
                    Vec::new()
                };
                groups.push(AccessGroup { id, label: label.into(), expanded, total, files });
            }
        }
        let (commit_label, can_commit) = self.status.as_ref().map_or_else(|| commit_button(0, 0), |s| self.commit_state(s, cx));
        AccessView {
            revision: self.status.clone(),
            root: self.status.as_ref().map(|s| s.root.clone()),
            branch: self.status.as_ref().and_then(|s| s.branch.clone()).unwrap_or_else(|| crate::i18n::t("detached HEAD").into()),
            message: self.message.read(cx).text().to_string(),
            message_focused: self.message.focus_handle(cx).is_focused(window),
            busy: self.busy,
            commit_label,
            can_commit: can_commit && !self.busy,
            result: self.result.as_ref().map(|(s, error)| (s.to_string(), *error)),
            groups,
        }
    }

    pub(crate) fn a11y_set_message(&mut self, text: &str, cx: &mut Context<Self>) {
        if !self.busy {
            self.message.update(cx, |m, cx| m.set_text(text, cx));
        }
    }

    pub(crate) fn a11y_toggle_group(&mut self, id: &str, cx: &mut Context<Self>) {
        self.sections.toggle(id, true);
        cx.emit(ScmEvent::SectionsChanged(self.sections.encode()));
        cx.notify();
    }

    /// Reject a stale action if the same path is no longer in the same index/worktree state.
    pub(crate) fn a11y_has_file(&self, file: &AccessFile) -> bool {
        self.status.as_ref().is_some_and(|s| s.changes.contains(&file.change))
            && if file.staged { file.change.staged.is_some() } else { file.change.unstaged.is_some() }
    }
}
