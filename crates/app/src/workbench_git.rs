//! The workbench's git network and branch operations: pick/create branch, pull, push, fetch, sync.
//! git runs where the files are (on the server over SSH); it can't prompt for a password,
//! so missing credentials produce an error that points at the terminal.

use gpui::{AppContext as _, Context, Focusable as _, PromptLevel, Window};

use super::{InputPurpose, Notice, OverlayKind, Workbench};
use crate::git;
use crate::picker::{PickItem, Picker, Source as PickSource};

#[derive(Clone, Copy, Debug)]
pub enum StashOp {
    Pop,
    Apply,
    Drop,
    /// Open what the stash holds as a diff.
    Show,
}

/// "Merge / Delete / Rename Branch", "Remove Remote": act on the name picked from the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchOp {
    Merge,
    Rebase,
    OpenWorktree,
    Delete,
    Rename,
    RemoveRemote,
    // --- git-refs --- (listed and run in `workbench_refs.rs`)
    DeleteTag,
    DeleteRemoteTag,
    PushTag,
    CheckoutDetached,
    CreateFrom,
    DeleteRemoteBranch,
    SetUpstream,
    RenameRemote,
    SetRemoteUrl,
    RemoveWorktree,
    CompareBranch,
    CompareCommit,
}

/// Row in the branch picker.
#[derive(Clone, Debug)]
pub enum BranchChoice {
    Create,
    CreateFrom,
    Detached,
    Local(String),
    Remote(String),
}

/// The explorer's TIMELINE section: git history of the active file.
#[derive(Default)]
pub struct Timeline {
    /// Loaded state: (file path, HEAD). Reloads when it changes.
    pub key: Option<(String, Option<String>, u64)>,
    pub entries: Vec<git::LogEntry>,
    /// Local history copies (newest first) and a save counter (incremented on every save).
    pub local: Vec<crate::history::Entry>,
    pub rev: u64,
    pub loading: bool,
    pub error: Option<gpui::SharedString>,
    pub root: Option<String>,
    pub task: Option<gpui::Task<()>>,
}

impl Workbench {
    /// Refresh TIMELINE in the background when the active file or HEAD changes (while visible).
    pub(super) fn sync_timeline(&mut self, cx: &mut Context<Self>) {
        use super::Panel;
        if self.chrome.panel != Some(Panel::Explorer) || !self.sections.is_open("timeline", true) {
            return;
        }
        let active_path = self
            .tabs
            .active
            .filter(|_| !self.tabs.diff_active && !self.tabs.large_active && !self.tabs.page_active)
            .and_then(|i| self.tabs.editors.get(i))
            .map(|e| e.read(cx).path.clone());
        let target = self.vcs.status.as_ref().and_then(|st| {
            let path = active_path.clone()?;
            let rel = path.strip_prefix(&format!("{}/", st.root.trim_end_matches('/')))?.to_string();
            Some((st.root.clone(), rel, st.head.clone()))
        });
        let key = active_path.clone().map(|p| (p, target.as_ref().and_then(|t| t.2.clone()), self.timeline.rev));
        if key == self.timeline.key {
            return;
        }
        self.timeline.key = key;
        self.timeline.error = None;
        // Local history: copies in the data folder (a small directory read).
        self.timeline.local = match (&self.persist.data_dir, &active_path) {
            (Some(data), Some(path)) => crate::history::list(&crate::history::dir_for(data, &self.history_host(), path)),
            _ => Vec::new(),
        };
        let Some((root, rel, _)) = target else {
            self.timeline.entries.clear();
            self.timeline.root = None;
            self.timeline.loading = false;
            self.timeline.task = None;
            return;
        };
        self.timeline.loading = true;
        self.timeline.root = Some(root.clone());
        let host = self.remote.host.clone();
        let limit = cx.global::<crate::settings::Settings>().timeline_limit;
        let task = cx.background_executor().spawn(async move { git::file_log(host.as_ref(), &root, &rel, limit) });
        // A new request drops the old one (the task is cancelled when dropped).
        self.timeline.task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.timeline.loading = false;
                match result {
                    Ok(log) => this.timeline.entries = log,
                    Err(e) => {
                        this.timeline.entries.clear();
                        this.timeline.error = Some(first_line(&e).to_string().into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Server name in the history copies' key (empty locally).
    pub(super) fn history_host(&self) -> String {
        match &self.remote.conn {
            super::Conn::Connected { alias } | super::Conn::Reconnecting { alias, .. } => alias.clone(),
            _ => String::new(),
        }
    }

    /// After a save: keep a copy of the file (in the background), refresh TIMELINE.
    pub(super) fn record_history(&mut self, ed: &gpui::Entity<crate::editor::EditorView>, cx: &mut Context<Self>) {
        let Some(data) = self.persist.data_dir.clone() else { return };
        let e = ed.read(cx);
        if e.line_count() > 60_000 {
            return;
        }
        let (path, text) = (e.path.clone(), e.text_lines().join("\n"));
        let dir = crate::history::dir_for(&data, &self.history_host(), &path);
        let task = cx.background_executor().spawn(async move { crate::history::record(&dir, &text) });
        cx.spawn(async move |this, cx| {
            if let Ok(true) = task.await {
                this.update(cx, |this, cx| {
                    this.timeline.rev += 1;
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Show a local history copy next to the editor's current version.
    pub(super) fn open_local_history(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.timeline.local.get(ix).cloned() else { return };
        let Some(ed) = self.active_editor().cloned() else { return };
        let (name, current) = {
            let e = ed.read(cx);
            (e.name.to_string(), e.text_lines().join("\n"))
        };
        match crate::history::read(&entry) {
            Ok(old) => {
                let ago =
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64).saturating_sub(entry.at_ms) / 1000;
                self.open_compare(
                    crate::i18n::tf("{} (saved {}) ↔ editor", &[&name, &git::ago_phrase(&git::relative_time(ago as i64))]),
                    crate::i18n::t("Local history"),
                    &old,
                    &current,
                    window,
                    cx,
                );
            }
            Err(e) => {
                self.notice = Some(Notice::Error(crate::i18n::tf("Could not read the saved copy: {}", &[&e]).into()));
                cx.notify();
            }
        }
    }

    /// A commit in Timeline: open this file's change as a diff.
    pub(super) fn open_timeline_entry(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(root), Some(entry)) = (self.timeline.root.clone(), self.timeline.entries.get(ix).cloned()) else { return };
        self.open_commit_diff(root, entry, window, cx);
    }

    #[cfg(test)]
    pub fn timeline_local_for_test(&self) -> usize {
        self.timeline.local.len()
    }

    #[cfg(test)]
    pub fn open_local_history_for_test(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.open_local_history(ix, window, cx);
    }

    #[cfg(test)]
    pub fn open_timeline_entry_for_test(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.open_timeline_entry(ix, window, cx);
    }

    #[cfg(test)]
    pub fn timeline_subjects(&self) -> Vec<String> {
        self.timeline.entries.iter().map(|e| e.subject.clone()).collect()
    }

    /// A git operation in the SCM panel (result and busy state show there).
    pub(super) fn with_scm(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut crate::scm::ScmPanel, &mut Context<crate::scm::ScmPanel>)) {
        if self.repo_root(cx).is_none() {
            return;
        }
        self.chrome.panel = Some(super::Panel::Scm);
        self.scm.update(cx, f);
        cx.notify();
    }

    /// Stash picker (pop / apply / drop).
    pub(super) fn show_stash_picker(&mut self, op: StashOp, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.repo_root(cx) else { return };
        let host = self.remote.host.clone();
        let task = cx.background_executor().spawn(async move { git::stash_list(host.as_ref(), &root) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(list) if list.is_empty() => {
                    this.notice = Some(Notice::Info(crate::i18n::t("There are no stashes.").into()));
                    cx.notify();
                }
                Ok(list) => {
                    let items = list.iter().map(|(r, m)| PickItem { label: m.clone().into(), detail: Some(r.clone().into()), keys: None }).collect();
                    let refs = list.into_iter().map(|(r, _)| r).collect();
                    let title = match op {
                        StashOp::Pop => crate::i18n::t("Pick a stash to pop"),
                        StashOp::Apply => crate::i18n::t("Pick a stash to apply"),
                        StashOp::Drop => crate::i18n::t("Pick a stash to drop"),
                        StashOp::Show => crate::i18n::t("Pick a stash to show"),
                    };
                    let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], title, "", None, cx));
                    this.show_overlay(picker, OverlayKind::Stashes { refs, op }, window, cx);
                }
                Err(e) => {
                    this.notice = Some(Notice::Error(crate::i18n::tf("Could not list stashes: {}", &[&first_line(&e)]).into()));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The active file's repository and its path relative to the repository.
    pub(super) fn active_in_repo(&mut self, cx: &mut Context<Self>) -> Option<(String, String)> {
        let root = self.repo_root(cx)?;
        let Some(path) = self.active_editor().map(|e| e.read(cx).path.clone()) else {
            self.notice = Some(Notice::Info(crate::i18n::t("Open a file first.").into()));
            cx.notify();
            return None;
        };
        match path.strip_prefix(&format!("{}/", root.trim_end_matches('/'))) {
            Some(rel) => Some((root, rel.to_string())),
            None => {
                self.notice = Some(Notice::Info(crate::i18n::t("This file is not in the repository.").into()));
                cx.notify();
                None
            }
        }
    }

    /// "Git: View File History": commits (including renames); the picked commit's change
    /// to this file opens side by side.
    pub(super) fn show_file_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((root, rel)) = self.active_in_repo(cx) else { return };
        let host = self.remote.host.clone();
        let (r, f) = (root.clone(), rel.clone());
        let task = cx.background_executor().spawn(async move { git::file_log(host.as_ref(), &r, &f, 300) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(log) if log.is_empty() => {
                    this.notice = Some(Notice::Info(crate::i18n::tf("{} has no commits yet.", &[&crate::ui::file_name(&rel)]).into()));
                    cx.notify();
                }
                Ok(log) => {
                    let items = log
                        .iter()
                        .map(|e| PickItem {
                            label: e.subject.clone().into(),
                            detail: Some(format!("{} · {} · {}", e.short, e.author, e.when).into()),
                            keys: None,
                        })
                        .collect();
                    let title = crate::i18n::tf("History of {}", &[&crate::ui::file_name(&rel)]);
                    let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], &title, "", None, cx));
                    this.show_overlay(picker, OverlayKind::FileHistory { root, log }, window, cx);
                }
                Err(e) => {
                    this.notice = Some(Notice::Error(crate::i18n::tf("Could not read the history: {}", &[&first_line(&e)]).into()));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// What the commit changed in the file: the parent's version ↔ the commit's.
    pub(super) fn open_commit_diff(&mut self, root: String, entry: git::LogEntry, window: &mut Window, cx: &mut Context<Self>) {
        self.open_commit_diff_from(root, entry, None, window, cx);
    }

    /// `old_path`: the renamed file's name in the parent.
    pub(super) fn open_commit_diff_from(&mut self, root: String, entry: git::LogEntry, old_path: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.remote.host.clone();
        let e = entry.clone();
        let task = cx.background_executor().spawn(async move {
            let h = host.as_ref();
            // The first commit has no parent: left side is empty.
            let old = git::show(h, &root, &format!("{}^", e.hash), old_path.as_deref().unwrap_or(&e.path)).unwrap_or_default();
            let new = git::show(h, &root, &e.hash, &e.path).unwrap_or_default();
            (old, new)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (old, new) = task.await;
            this.update_in(cx, |this, window, cx| {
                let label = format!("{} {} · {}", entry.short, entry.subject, entry.author);
                let view = cx.new(|cx| crate::diff::DiffView::new(entry.path.clone(), false, &old, &new, cx).with_label(label));
                window.focus(&view.focus_handle(cx));
                this.tabs.diff = Some(view);
                this.tabs.diff_active = true;
                this.tabs.page_active = false;
                this.tabs.large_active = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Commit picked in the graph: list the files it changed, open the picked one side by side.
    pub(super) fn show_commit_files(&mut self, commit: crate::graph::GraphCommit, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.repo_root(cx) else { return };
        let host = self.remote.host.clone();
        let (r, h) = (root.clone(), commit.hash.clone());
        let task = cx.background_executor().spawn(async move { crate::graph::changed_files(host.as_ref(), &r, &h) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(files) if files.is_empty() => {
                    this.notice = Some(Notice::Info(crate::i18n::tf("{} changed no files (merge or empty commit).", &[&commit.short]).into()));
                    cx.notify();
                }
                Ok(files) => this.show_changed_files(root, commit, files, window, cx),
                Err(e) => {
                    this.notice = Some(Notice::Error(crate::i18n::tf("Could not read the commit: {}", &[&first_line(&e)]).into()));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The picker of a commit's files (a stash is a commit too); the picked file opens as a diff.
    pub(super) fn show_changed_files(
        &mut self,
        root: String,
        commit: crate::graph::GraphCommit,
        files: Vec<(char, String, Option<String>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = files
            .iter()
            .map(|(status, path, old)| PickItem {
                label: crate::ui::file_name(path).to_string().into(),
                detail: Some(
                    match old {
                        Some(o) => format!("{status} · {o} → {path}"),
                        None => format!("{status} · {path}"),
                    }
                    .into(),
                ),
                keys: None,
            })
            .collect();
        let title = format!("{} {} — {}", commit.short, commit.subject, commit.author);
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], &title, "", None, cx));
        self.show_overlay(picker, OverlayKind::CommitFiles { root, commit, files }, window, cx);
    }

    /// "Git: Blame Line": who last changed the line under the cursor.
    pub(super) fn blame_line(&mut self, cx: &mut Context<Self>) {
        let Some((root, rel)) = self.active_in_repo(cx) else { return };
        let Some(ed) = self.active_editor().cloned() else { return };
        let (line, contents) = {
            let e = ed.read(cx);
            (e.cursor_position().0.saturating_sub(1), e.is_dirty().then(|| e.text_lines().join("\n")))
        };
        let host = self.remote.host.clone();
        let task = cx.background_executor().spawn(async move { git::blame_line(host.as_ref(), &root, &rel, line, contents) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.notice = Some(match result {
                    Ok(b) if b.hash.chars().all(|c| c == '0') => Notice::Info(crate::i18n::tf("Line {}: not committed yet", &[&(line + 1)]).into()),
                    Ok(b) => Notice::Info(
                        crate::i18n::tf("Line {}: {} · {}, {} · {}", &[&(line + 1), &b.short, &b.author, &git::ago_phrase(&b.when), &b.subject]).into(),
                    ),
                    Err(e) => Notice::Error(crate::i18n::tf("Could not blame: {}", &[&first_line(&e)]).into()),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The "…" in the SCM header: the commonly used items from VS Code's menu.
    pub(super) fn open_scm_menu(&mut self, position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        use super::*;
        let entries: Vec<(&str, Box<dyn gpui::Action>, bool)> = vec![
            // Pull, Push, Fetch and Stash are git terms and stay as they are in every language.
            // --- git-sync ---
            (crate::i18n::t("Sync"), Box::new(GitSync), false),
            ("Pull", Box::new(GitPull), false),
            (crate::i18n::t("Pull (Rebase)"), Box::new(GitPullRebase), false),
            (crate::i18n::t("Pull from…"), Box::new(GitPullFrom), false),
            ("Push", Box::new(GitPush), false),
            (crate::i18n::t("Push to…"), Box::new(GitPushTo), false),
            ("Fetch", Box::new(GitFetch), false),
            (crate::i18n::t("Fetch (All Remotes)"), Box::new(GitFetchAll), false),
            (crate::i18n::t("Publish Branch"), Box::new(GitPublishBranch), false),
            (crate::i18n::t("Checkout to…"), Box::new(GitCheckout), true),
            (crate::i18n::t("Create Branch…"), Box::new(GitCreateBranch), false),
            (crate::i18n::t("Commit (Amend)"), Box::new(GitCommitAmend), true),
            // --- git-sync ---
            (crate::i18n::t("Commit Signed Off"), Box::new(GitCommitSignedOff), false),
            (crate::i18n::t("Commit (No Verify)"), Box::new(GitCommitNoVerify), false),
            (crate::i18n::t("Commit & Push"), Box::new(GitCommitAndPush), false),
            (crate::i18n::t("Commit & Sync"), Box::new(GitCommitAndSync), false),
            (crate::i18n::t("Undo Last Commit"), Box::new(GitUndoLastCommit), false),
            ("Stash", Box::new(GitStash), true),
            (crate::i18n::t("Stash (Include Untracked)"), Box::new(GitStashIncludeUntracked), false),
            // --- git-refs ---
            (crate::i18n::t("Stash Staged Changes"), Box::new(GitStashStaged), false),
            (crate::i18n::t("Stash with Name…"), Box::new(GitStashNamed), false),
            (crate::i18n::t("Show Stash…"), Box::new(GitShowStash), false),
            (crate::i18n::t("Pop Latest Stash"), Box::new(GitPopLatestStash), false),
            (crate::i18n::t("Pop Stash…"), Box::new(GitPopStash), false),
            (crate::i18n::t("Apply Stash…"), Box::new(GitApplyStash), false),
            (crate::i18n::t("Drop Stash…"), Box::new(GitDropStash), false),
            // --- git-refs ---
            (crate::i18n::t("Create Tag…"), Box::new(GitCreateTag), true),
            (crate::i18n::t("Delete Tag…"), Box::new(GitDeleteTag), false),
            (crate::i18n::t("Push Tags"), Box::new(GitPushTags), false),
            (crate::i18n::t("Discard All Changes"), Box::new(GitDiscardAll), true),
        ];
        self.show_action_menu(entries, position, window, cx);
    }

    /// Right-click on a commit in the graph: copy, files, open branch/tag, cherry-pick, revert, reset.
    pub(super) fn open_commit_menu(
        &mut self,
        commit: crate::graph::GraphCommit,
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use super::{MenuCmd, MenuTarget};
        use crate::graph::ResetMode;
        use crate::menu::MenuItem;
        let rows: Vec<(MenuItem, MenuCmd)> = vec![
            (MenuItem::new(crate::i18n::t("Copy Commit Hash")), MenuCmd::CopyHash),
            (MenuItem::new(crate::i18n::t("Copy Message")), MenuCmd::CopySubject),
            (MenuItem::new(crate::i18n::t("Show Changed Files…")), MenuCmd::CommitFiles),
            (MenuItem::new(crate::i18n::t("Create Branch Here…")).separator(), MenuCmd::CommitBranch),
            (MenuItem::new(crate::i18n::t("Create Tag Here…")), MenuCmd::CommitTag),
            (MenuItem::new(crate::i18n::t("Checkout (Detached)")), MenuCmd::CommitCheckout),
            (MenuItem::new(crate::i18n::t("Cherry-pick")).separator(), MenuCmd::CommitCherryPick),
            (MenuItem::new(crate::i18n::t("Revert")), MenuCmd::CommitRevert),
            (MenuItem::new(crate::i18n::t("Reset Soft to Here")).separator(), MenuCmd::CommitReset(ResetMode::Soft)),
            (MenuItem::new(crate::i18n::t("Reset Mixed to Here")), MenuCmd::CommitReset(ResetMode::Mixed)),
            (MenuItem::new(crate::i18n::t("Reset Hard to Here…")), MenuCmd::CommitReset(ResetMode::Hard)),
        ];
        let (items, commands): (Vec<_>, Vec<_>) = rows.into_iter().unzip();
        let target = MenuTarget { path: commit.hash, is_dir: false, tab: None };
        self.show_menu(items, commands, Vec::new(), target, position, window, cx);
    }

    /// Open a file of the commit in a diff tab (compared with its parent).
    pub(super) fn open_commit_file(&mut self, commit: crate::graph::GraphCommit, file: crate::graph::FileChange, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.repo_root(cx) else { return };
        let entry =
            git::LogEntry { hash: commit.hash, short: commit.short, author: commit.author, when: commit.when, subject: commit.subject, path: file.path };
        self.open_commit_diff_from(root, entry, file.old, window, cx);
    }

    pub(super) fn repo_root(&mut self, cx: &mut Context<Self>) -> Option<String> {
        match self.git_status() {
            Some(st) => Some(st.root.clone()),
            None => {
                self.notice = Some(Notice::Info(crate::i18n::t("This folder is not a git repository.").into()));
                cx.notify();
                None
            }
        }
    }

    /// Branch in the status bar or "Git: Checkout to…": list the branches.
    pub(super) fn show_branch_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.repo_root(cx) else { return };
        let host = self.remote.host.clone();
        let r = root.clone();
        let task = cx.background_executor().spawn(async move { git::branch_infos(host.as_ref(), &r) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                let infos = match result {
                    Ok(b) => b,
                    Err(e) => {
                        this.notice = Some(Notice::Error(crate::i18n::tf("Could not list branches: {}", &[&e]).into()));
                        cx.notify();
                        return;
                    }
                };
                let (local, remote): (Vec<_>, Vec<_>) = infos.into_iter().partition(|b| !b.remote);
                // Remote branches that have a local tracking branch are not listed again.
                let remote: Vec<_> = remote.into_iter().filter(|r| !local.iter().any(|l| r.name.split_once('/').is_some_and(|(_, b)| b == l.name))).collect();
                let mut choices = vec![BranchChoice::Create, BranchChoice::CreateFrom, BranchChoice::Detached];
                let mut items = vec![
                    PickItem { label: crate::i18n::t("+ Create new branch…").into(), detail: None, keys: None },
                    PickItem { label: crate::i18n::t("+ Create new branch from…").into(), detail: None, keys: None },
                    PickItem { label: crate::i18n::t("Check out detached…").into(), detail: None, keys: None },
                ];
                for b in local {
                    items.push(PickItem { label: b.name.clone().into(), detail: Some(super::refs_glue::branch_detail(&b).into()), keys: None });
                    choices.push(BranchChoice::Local(b.name));
                }
                for b in remote {
                    items.push(PickItem { label: b.name.clone().into(), detail: Some(super::refs_glue::branch_detail(&b).into()), keys: None });
                    choices.push(BranchChoice::Remote(b.name));
                }
                let picker =
                    cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], crate::i18n::t("Select a branch to check out"), "", None, cx));
                this.show_overlay(picker, OverlayKind::Branches { root, choices }, window, cx);
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn pick_branch(&mut self, root: String, choice: BranchChoice, window: &mut Window, cx: &mut Context<Self>) {
        match choice {
            BranchChoice::Create => self.start_create_branch(root, window, cx),
            BranchChoice::CreateFrom => self.show_branch_op_picker(BranchOp::CreateFrom, window, cx),
            BranchChoice::Detached => self.show_branch_op_picker(BranchOp::CheckoutDetached, window, cx),
            BranchChoice::Local(b) => self.git_checkout(root, b, false, false, cx),
            BranchChoice::Remote(b) => self.git_checkout(root, b, true, false, cx),
        }
    }

    pub(super) fn start_create_branch(&mut self, root: String, window: &mut Window, cx: &mut Context<Self>) {
        self.show_input(
            InputPurpose::CreateBranch { root },
            crate::i18n::t("Create branch"),
            crate::i18n::t("Name of the new branch (from the current one)"),
            "",
            window,
            cx,
        );
    }

    pub(super) fn create_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(root) = self.repo_root(cx) {
            self.start_create_branch(root, window, cx);
        }
    }

    pub(super) fn git_checkout(&mut self, root: String, branch: String, remote: bool, create: bool, cx: &mut Context<Self>) {
        let host = self.remote.host.clone();
        let b = branch.clone();
        let task = cx.background_executor().spawn(async move { git::checkout(host.as_ref(), &root, &b, remote, create) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.notice = Some(match result {
                    Ok(()) => Notice::Info(crate::i18n::tf("Switched to {}", &[&branch]).into()),
                    Err(e) => Notice::Error(crate::i18n::tf("Could not switch to {0}: {1}", &[&branch, &first_line(&e)]).into()),
                });
                this.refresh_git(0, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Ask for a branch or remote name (list depends on `op`), then run the operation.
    pub(super) fn show_branch_op_picker(&mut self, op: BranchOp, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.repo_root(cx) else { return };
        let current = self.git_status().and_then(|s| s.branch.clone());
        let host = self.remote.host.clone();
        let r = root.clone();
        let task = cx.background_executor().spawn(async move {
            match op {
                BranchOp::RemoveRemote => git::remotes(host.as_ref(), &r).map(|v| v.into_iter().map(|(n, u)| (n, Some(u))).collect::<Vec<_>>()),
                BranchOp::OpenWorktree => git::worktrees(host.as_ref(), &r),
                // --- git-refs ---
                op if op.is_ref_op() => super::refs_glue::list_for(op, host.as_ref(), &r),
                _ => git::branches(host.as_ref(), &r).map(|(l, _)| l.into_iter().map(|n| (n, None)).collect()),
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                let entries = match result {
                    Ok(v) => v,
                    Err(e) => {
                        this.notice = Some(Notice::Error(crate::i18n::tf("Could not list: {}", &[&first_line(&e)]).into()));
                        cx.notify();
                        return;
                    }
                };
                // Merge and delete don't apply to the current branch.
                let entries: Vec<(String, Option<String>)> = entries
                    .into_iter()
                    .filter(|(n, _)| !(matches!(op, BranchOp::Merge | BranchOp::Rebase | BranchOp::Delete) && current.as_deref() == Some(n.as_str())))
                    .collect();
                if entries.is_empty() {
                    this.notice = Some(Notice::Info(crate::i18n::t("Nothing to choose from.").into()));
                    cx.notify();
                    return;
                }
                let title = match op {
                    BranchOp::Merge => crate::i18n::tf("Merge into {}", &[&current.as_deref().unwrap_or("HEAD")]),
                    BranchOp::Rebase => crate::i18n::tf("Rebase {} onto…", &[&current.as_deref().unwrap_or("HEAD")]),
                    BranchOp::OpenWorktree => crate::i18n::t("Open which worktree?").into(),
                    BranchOp::Delete => crate::i18n::t("Delete which branch?").into(),
                    BranchOp::Rename => crate::i18n::t("Rename which branch?").into(),
                    BranchOp::RemoveRemote => crate::i18n::t("Remove which remote?").into(),
                    // --- git-refs ---
                    op => super::refs_glue::title_for(op),
                };
                let items = entries.iter().map(|(n, d)| PickItem { label: n.clone().into(), detail: d.clone().map(Into::into), keys: None }).collect();
                let names = entries.into_iter().map(|(n, _)| n).collect();
                let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], &title, "", None, cx));
                this.show_overlay(picker, OverlayKind::BranchOp { root, op, names }, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Operate on the picked name. Irreversible ones ask a local confirm sheet first.
    pub(super) fn run_branch_op(&mut self, root: String, op: BranchOp, name: String, window: &mut Window, cx: &mut Context<Self>) {
        match op {
            BranchOp::Rename => {
                let initial = name.clone();
                self.show_input(
                    InputPurpose::RenameBranch { root, old: name },
                    crate::i18n::t("Rename branch"),
                    crate::i18n::t("New branch name"),
                    &initial,
                    window,
                    cx,
                );
            }
            BranchOp::Merge => {
                let into = self.git_status().and_then(|s| s.branch.clone()).unwrap_or_else(|| "HEAD".into());
                self.confirm(
                    window,
                    &crate::i18n::tf("Merge {} into {}?", &[&name, &into]),
                    crate::i18n::t("A merge commit is created unless it can fast-forward. Conflicts are resolved in Source Control."),
                    crate::i18n::t("Merge"),
                    cx,
                    move |this, cx| {
                        this.git_task(&crate::i18n::tf("Merging {}…", &[&name]), cx, move |h| match git::merge(h, &root, &name) {
                            Ok(false) => Ok(crate::i18n::tf("Merged {}", &[&name])),
                            Ok(true) => Ok(crate::i18n::tf("Merge of {} has conflicts: resolve them in Source Control (or Git: Abort Merge)", &[&name])),
                            Err(e) => Err(e),
                        });
                    },
                );
            }
            BranchOp::Rebase => {
                let cur = self.git_status().and_then(|s| s.branch.clone()).unwrap_or_else(|| "HEAD".into());
                self.confirm(window, &crate::i18n::tf("Rebase {} onto {}?", &[&cur, &name]), crate::i18n::t("Your commits on this branch are rewritten on top of it (their hashes change). Conflicts stop the rebase; resolve them and use Git: Continue Rebase, or Git: Abort Rebase."), crate::i18n::t("Rebase"), cx, move |this, cx| {
                    this.git_task(&crate::i18n::tf("Rebasing onto {}…", &[&name]), cx, move |h| match git::rebase(h, &root, &name) {
                        Ok(false) => Ok(crate::i18n::tf("Rebased onto {}", &[&name])),
                        Ok(true) => Ok(crate::i18n::t("Rebase stopped on conflicts: resolve them, then Git: Continue Rebase (or Abort Rebase)").to_string()),
                        Err(e) => Err(e),
                    });
                });
            }
            BranchOp::OpenWorktree => self.open_path(&name, window, cx),
            BranchOp::Delete => {
                // First count how many commits remain only on this branch, so the confirm text can say it.
                let host = self.remote.host.clone();
                let (r, n) = (root.clone(), name.clone());
                let task = cx.background_executor().spawn(async move { git::unmerged_commits(host.as_ref(), &r, &n) });
                cx.spawn_in(window, async move |this, cx| {
                    let unmerged = task.await;
                    this.update_in(cx, |this, window, cx| {
                        // If counting fails, never force delete: let git decide with its own check (-d).
                        let (detail, force) = match unmerged {
                            Ok(0) => (crate::i18n::t("The branch is fully merged into the current branch.").to_string(), true),
                            Ok(n) => (
                                crate::i18n::tf(
                                    "{0} commit{1} only exist on this branch and will be lost (the reflog keeps them for a while).",
                                    &[&n, &if n == 1 { "" } else { "s" }],
                                ),
                                true,
                            ),
                            Err(e) => (
                                crate::i18n::tf("Could not tell whether it is merged ({}): git will refuse if commits would be lost.", &[&first_line(&e)]),
                                false,
                            ),
                        };
                        this.confirm(window, &crate::i18n::tf("Delete branch {}?", &[&name]), &detail, crate::i18n::t("Delete"), cx, move |this, cx| {
                            this.git_task(&crate::i18n::tf("Deleting {}…", &[&name]), cx, move |h| {
                                git::delete_branch(h, &root, &name, force).map(|_| crate::i18n::tf("Deleted {}", &[&name]))
                            });
                        });
                    })
                    .ok();
                })
                .detach();
            }
            BranchOp::RemoveRemote => {
                self.confirm(
                    window,
                    &crate::i18n::tf("Remove remote {}?", &[&name]),
                    crate::i18n::t("Only the connection is removed; the remote repository itself is untouched."),
                    crate::i18n::t("Remove"),
                    cx,
                    move |this, cx| {
                        this.git_task(&crate::i18n::tf("Removing {}…", &[&name]), cx, move |h| {
                            git::remove_remote(h, &root, &name).map(|_| crate::i18n::tf("Removed remote {}", &[&name]))
                        });
                    },
                );
            }
            // --- git-refs ---
            op => self.run_ref_op(root, op, name, window, cx),
        }
    }

    /// Local confirm sheet; `then` runs if anything but "Cancel" is picked.
    pub(super) fn confirm(
        &mut self,
        window: &mut Window,
        title: &str,
        detail: &str,
        verb: &str,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
    ) {
        // Callers already pass title, detail and button in the UI language; only "Cancel" is translated here.
        let answer = window.prompt(PromptLevel::Warning, title, Some(detail), &[verb, crate::i18n::t("Cancel")], cx);
        cx.spawn(async move |this, cx| {
            if answer.await == Ok(0) {
                this.update(cx, |this, cx| then(this, cx)).ok();
            }
        })
        .detach();
    }

    /// Git operation in the background; the result goes to a notification, state is refreshed.
    pub(super) fn git_task(&mut self, working: &str, cx: &mut Context<Self>, f: impl FnOnce(&dyn host::Host) -> Result<String, String> + Send + 'static) {
        self.git_task_then(working, cx, f, |_, _| {});
    }

    /// `git_task`, then `after` runs with the outcome already shown (e.g. to reload the stash list).
    pub(super) fn git_task_then(
        &mut self,
        working: &str,
        cx: &mut Context<Self>,
        f: impl FnOnce(&dyn host::Host) -> Result<String, String> + Send + 'static,
        after: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
    ) {
        self.notice = Some(Notice::Info(working.to_string().into()));
        cx.notify();
        let host = self.remote.host.clone();
        let task = cx.background_executor().spawn(async move { f(host.as_ref()) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(m) => this.notice = Some(Notice::Info(m.into())),
                    Err(e) => this.notice = Some(Notice::Error(first_line(&e).to_string().into())),
                }
                this.refresh_git(0, cx);
                after(this, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn git_abort_merge(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.repo_root(cx) else { return };
        self.confirm(
            window,
            crate::i18n::t("Abort the merge?"),
            crate::i18n::t("Everything resolved so far in this merge is discarded."),
            crate::i18n::t("Abort Merge"),
            cx,
            move |this, cx| {
                this.git_task(crate::i18n::t("Aborting merge…"), cx, move |h| {
                    git::merge_abort(h, &root).map(|_| crate::i18n::t("Merge aborted").to_string())
                });
            },
        );
    }

    pub(super) fn git_continue_rebase(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.repo_root(cx) else { return };
        self.git_task(crate::i18n::t("Continuing rebase…"), cx, move |h| match git::rebase_continue(h, &root) {
            Ok(false) => Ok(crate::i18n::t("Rebase finished").to_string()),
            Ok(true) => Ok(crate::i18n::t("Rebase stopped again on conflicts: resolve and stage them, then Git: Continue Rebase").to_string()),
            Err(e) => Err(e),
        });
    }

    pub(super) fn git_abort_rebase(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.repo_root(cx) else { return };
        self.confirm(
            window,
            crate::i18n::t("Abort the rebase?"),
            crate::i18n::t("The branch goes back to how it was before the rebase started."),
            crate::i18n::t("Abort Rebase"),
            cx,
            move |this, cx| {
                this.git_task(crate::i18n::t("Aborting rebase…"), cx, move |h| {
                    git::rebase_abort(h, &root).map(|_| crate::i18n::t("Rebase aborted").to_string())
                });
            },
        );
    }

    pub(super) fn git_add_worktree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(root) = self.repo_root(cx) {
            self.show_input(
                InputPurpose::AddWorktree { root },
                crate::i18n::t("Add worktree"),
                crate::i18n::t("path branch (new branch is created if it does not exist)"),
                "",
                window,
                cx,
            );
        }
    }

    pub(super) fn git_add_remote(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(root) = self.repo_root(cx) {
            self.show_input(
                InputPurpose::AddRemote { root },
                crate::i18n::t("Add remote"),
                crate::i18n::t("name url (e.g. origin https://host/repo.git)"),
                "",
                window,
                cx,
            );
        }
    }

    pub(super) fn git_create_tag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.repo_root(cx).is_some() {
            self.show_input(
                InputPurpose::CommitTag { hash: "HEAD".into() },
                crate::i18n::t("Create tag"),
                crate::i18n::t("Tag name, then an optional message for an annotated tag (at HEAD)"),
                "",
                window,
                cx,
            );
        }
    }
}

pub(super) fn first_line(e: &str) -> &str {
    e.lines().find(|l| !l.trim().is_empty()).unwrap_or(e).trim()
}
