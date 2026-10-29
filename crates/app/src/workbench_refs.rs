//! Tags, remote branches, upstreams, remotes, worktrees, named and staged stashes, comparing the open file with another
//! revision, and "Add to .gitignore". The pickers are the generic branch-operation picker (`BranchOp`); this file lists what each
//! one offers and runs the pick.

use gpui::{AppContext as _, Context, Window};

use super::git_glue::{BranchOp, first_line};
use super::{InputPurpose, Notice, OverlayKind, Workbench};
use crate::git::{self, BranchInfo};
use crate::picker::{PickItem, Picker, Source as PickSource};

impl BranchOp {
    /// The operations whose lists and actions live in this file.
    pub(super) fn is_ref_op(self) -> bool {
        !matches!(self, BranchOp::Merge | BranchOp::Rebase | BranchOp::OpenWorktree | BranchOp::Delete | BranchOp::Rename | BranchOp::RemoveRemote)
    }
}

/// What the branch picker shows next to a name: where it stands against its upstream and what its last commit was.
pub(super) fn branch_detail(b: &BranchInfo) -> String {
    let mut parts: Vec<String> = Vec::new();
    if b.current {
        parts.push(crate::i18n::t("current").into());
    }
    if b.remote {
        parts.push(crate::i18n::t("remote").into());
    }
    if b.gone {
        parts.push(crate::i18n::t("upstream gone").into());
    }
    if b.ahead > 0 || b.behind > 0 {
        parts.push(format!("↑{} ↓{}", b.ahead, b.behind));
    }
    if let Some(when) = b.when {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        parts.push(git::ago_phrase(&git::relative_time((now - when).max(0))));
    }
    if !b.subject.is_empty() {
        parts.push(b.subject.clone());
    }
    parts.join(" · ")
}

pub(super) fn title_for(op: BranchOp) -> String {
    match op {
        BranchOp::DeleteTag => crate::i18n::t("Delete which tag?"),
        BranchOp::DeleteRemoteTag => crate::i18n::t("Delete which tag on the remote?"),
        BranchOp::PushTag => crate::i18n::t("Push which tag?"),
        BranchOp::CheckoutDetached => crate::i18n::t("Check out which branch, tag or commit (detached)?"),
        BranchOp::CreateFrom => crate::i18n::t("Start the new branch from…"),
        BranchOp::DeleteRemoteBranch => crate::i18n::t("Delete which branch on the remote?"),
        BranchOp::SetUpstream => crate::i18n::t("Track which remote branch?"),
        BranchOp::RenameRemote => crate::i18n::t("Rename which remote?"),
        BranchOp::SetRemoteUrl => crate::i18n::t("Change the URL of which remote?"),
        BranchOp::RemoveWorktree => crate::i18n::t("Remove which worktree?"),
        BranchOp::CompareBranch => crate::i18n::t("Compare the file with which branch or tag?"),
        BranchOp::CompareCommit => crate::i18n::t("Compare the file with which commit?"),
        BranchOp::Merge | BranchOp::Rebase | BranchOp::OpenWorktree | BranchOp::Delete | BranchOp::Rename | BranchOp::RemoveRemote => "",
    }
    .into()
}

/// The rows of the picker for `op`: (name, detail).
pub(super) fn list_for(op: BranchOp, host: &dyn host::Host, root: &str) -> Result<Vec<(String, Option<String>)>, String> {
    let branches = |remote_only: bool| -> Result<Vec<(String, Option<String>)>, String> {
        Ok(git::branch_infos(host, root)?.into_iter().filter(|b| b.remote || !remote_only).map(|b| (b.name.clone(), Some(branch_detail(&b)))).collect())
    };
    match op {
        BranchOp::DeleteTag | BranchOp::PushTag => Ok(git::tags(host, root)?.into_iter().map(|t| (t, None)).collect()),
        BranchOp::DeleteRemoteTag => {
            let remote = git::default_remote(host, root)?;
            Ok(git::remote_tags(host, root, &remote)?.into_iter().map(|t| (t, Some(remote.clone()))).collect())
        }
        BranchOp::CheckoutDetached | BranchOp::CreateFrom | BranchOp::CompareBranch => {
            let mut all = branches(false)?;
            all.extend(git::tags(host, root)?.into_iter().map(|t| (t, Some(crate::i18n::t("tag").to_string()))));
            Ok(all)
        }
        BranchOp::DeleteRemoteBranch | BranchOp::SetUpstream => branches(true),
        BranchOp::RenameRemote | BranchOp::SetRemoteUrl => Ok(git::remotes(host, root)?.into_iter().map(|(n, u)| (n, Some(u))).collect()),
        // The first entry is the main worktree, which cannot be removed.
        BranchOp::RemoveWorktree => Ok(git::worktrees(host, root)?.into_iter().skip(1).collect()),
        BranchOp::CompareCommit
        | BranchOp::Merge
        | BranchOp::Rebase
        | BranchOp::OpenWorktree
        | BranchOp::Delete
        | BranchOp::Rename
        | BranchOp::RemoveRemote => Ok(Vec::new()),
    }
}

/// `head.rs` for `head`: "head (HEAD).rs", so the copy keeps its extension (and its syntax colors).
fn head_copy_name(file: &str) -> String {
    match file.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem} (HEAD).{ext}"),
        _ => format!("{file} (HEAD)"),
    }
}

impl Workbench {
    /// The repository root and `path` relative to it, or a notice saying why not.
    fn repo_rel(&mut self, path: &str, cx: &mut Context<Self>) -> Option<(String, String)> {
        let root = self.repo_root(cx)?;
        match path.strip_prefix(&format!("{}/", root.trim_end_matches('/'))) {
            Some(rel) => Some((root, rel.to_string())),
            None => {
                self.notice = Some(Notice::Info(crate::i18n::t("This file is not in the repository.").into()));
                cx.notify();
                None
            }
        }
    }

    fn active_file_path(&mut self, cx: &mut Context<Self>) -> Option<String> {
        let path = self.active_editor().map(|e| e.read(cx).path.clone());
        if path.is_none() {
            self.notice = Some(Notice::Info(crate::i18n::t("Open a file first.").into()));
            cx.notify();
        }
        path
    }

    pub(super) fn run_ref_op(&mut self, root: String, op: BranchOp, name: String, window: &mut Window, cx: &mut Context<Self>) {
        match op {
            BranchOp::DeleteTag => self.confirm(
                window,
                &crate::i18n::tf("Delete tag {}?", &[&name]),
                crate::i18n::t("Only the local tag is removed. A copy on a remote stays until you delete it there (Git: Delete Remote Tag…)."),
                crate::i18n::t("Delete"),
                cx,
                move |this, cx| {
                    this.git_task(&crate::i18n::tf("Deleting {}…", &[&name]), cx, move |h| {
                        git::delete_tag(h, &root, &name).map(|_| crate::i18n::tf("Deleted tag {}", &[&name]))
                    });
                },
            ),
            BranchOp::DeleteRemoteTag => self.confirm(
                window,
                &crate::i18n::tf("Delete tag {} on the remote?", &[&name]),
                crate::i18n::t("The tag disappears from the remote repository for everyone. Your local copy is kept."),
                crate::i18n::t("Delete"),
                cx,
                move |this, cx| {
                    this.git_task(&crate::i18n::tf("Deleting {}…", &[&name]), cx, move |h| {
                        let remote = git::default_remote(h, &root)?;
                        git::delete_remote_tag(h, &root, &remote, &name).map(|_| crate::i18n::tf("Deleted tag {0} on {1}", &[&name, &remote]))
                    });
                },
            ),
            BranchOp::PushTag => {
                self.git_task(&crate::i18n::tf("Pushing {}…", &[&name]), cx, move |h| {
                    git::push_tags(h, &root, Some(&name)).map(|remote| crate::i18n::tf("Pushed tag {0} to {1}", &[&name, &remote]))
                });
            }
            BranchOp::CheckoutDetached => {
                self.git_task(&crate::i18n::tf("Checking out {}…", &[&name]), cx, move |h| {
                    git::checkout_detached(h, &root, &name).map(|_| crate::i18n::tf("Checked out {} (detached HEAD)", &[&name]))
                });
            }
            BranchOp::CreateFrom => {
                self.show_input(
                    InputPurpose::CreateBranchFrom { root, start: name.clone() },
                    crate::i18n::t("Create branch"),
                    &crate::i18n::tf("Name of the new branch (starts at {})", &[&name]),
                    "",
                    window,
                    cx,
                );
            }
            BranchOp::DeleteRemoteBranch => self.confirm(
                window,
                &crate::i18n::tf("Delete branch {} on the remote?", &[&name]),
                crate::i18n::t("The branch is removed from the remote repository for everyone. Local branches are not touched."),
                crate::i18n::t("Delete"),
                cx,
                move |this, cx| {
                    this.git_task(&crate::i18n::tf("Deleting {}…", &[&name]), cx, move |h| {
                        git::delete_remote_branch(h, &root, &name).map(|_| crate::i18n::tf("Deleted {}", &[&name]))
                    });
                },
            ),
            BranchOp::SetUpstream => {
                let branch = self.git_status().and_then(|s| s.branch.clone()).unwrap_or_else(|| "HEAD".into());
                self.git_task(&crate::i18n::tf("Setting upstream of {}…", &[&branch]), cx, move |h| {
                    git::set_upstream(h, &root, &name).map(|_| crate::i18n::tf("{0} now tracks {1}", &[&branch, &name]))
                });
            }
            BranchOp::RenameRemote => {
                let initial = name.clone();
                self.show_input(
                    InputPurpose::RenameRemote { root, old: name },
                    crate::i18n::t("Rename remote"),
                    crate::i18n::t("New name of the remote"),
                    &initial,
                    window,
                    cx,
                );
            }
            BranchOp::SetRemoteUrl => {
                // The current URL is the starting text; it is read first so the field is not empty.
                let host = self.remote.host.clone();
                let (r, n) = (root.clone(), name.clone());
                let task = cx.background_executor().spawn(async move { git::remotes(host.as_ref(), &r) });
                cx.spawn_in(window, async move |this, cx| {
                    let list = task.await.unwrap_or_default();
                    this.update_in(cx, |this, window, cx| {
                        let url = list.into_iter().find(|(x, _)| *x == n).map(|(_, u)| u).unwrap_or_default();
                        this.show_input(
                            InputPurpose::SetRemoteUrl { root, name: n.clone() },
                            crate::i18n::t("Set URL"),
                            &crate::i18n::tf("New URL of {}", &[&n]),
                            &url,
                            window,
                            cx,
                        );
                    })
                    .ok();
                })
                .detach();
            }
            BranchOp::RemoveWorktree => self.confirm(
                window,
                &crate::i18n::tf("Remove worktree {}?", &[&name]),
                crate::i18n::t("Its folder is deleted. Git refuses when the worktree has uncommitted changes; the branch stays."),
                crate::i18n::t("Remove"),
                cx,
                move |this, cx| {
                    this.git_task(&crate::i18n::tf("Removing {}…", &[&name]), cx, move |h| {
                        git::remove_worktree(h, &root, &name, false).map(|_| crate::i18n::tf("Removed worktree {}", &[&name]))
                    });
                },
            ),
            BranchOp::CompareBranch => {
                if let Some((root, rel)) = self.active_in_repo(cx) {
                    self.open_compare_with(root, name.clone(), rel, name, window, cx);
                }
            }
            BranchOp::CompareCommit => {
                // The pick is "hash<TAB>path of the file at that commit" (the path differs after a rename).
                let Some((hash, path)) = name.split_once('\t') else { return };
                let label = hash[..hash.len().min(7)].to_string();
                self.open_compare_with(root, hash.to_string(), path.to_string(), label, window, cx);
            }
            BranchOp::Merge | BranchOp::Rebase | BranchOp::OpenWorktree | BranchOp::Delete | BranchOp::Rename | BranchOp::RemoveRemote => {}
        }
    }

    /// Answers of the text fields opened by `run_ref_op` and the stash command.
    pub(super) fn finish_ref_input(&mut self, purpose: InputPurpose, name: String, cx: &mut Context<Self>) {
        let name = name.trim().to_string();
        match purpose {
            InputPurpose::CreateBranchFrom { root, start } => {
                self.git_task(&crate::i18n::tf("Creating {}…", &[&name]), cx, move |h| {
                    git::create_branch_from(h, &root, &name, &start).map(|_| crate::i18n::tf("Created branch {0} at {1}", &[&name, &start]))
                });
            }
            InputPurpose::RenameRemote { root, old } => {
                self.git_task(&crate::i18n::tf("Renaming {}…", &[&old]), cx, move |h| {
                    git::rename_remote(h, &root, &old, &name).map(|_| crate::i18n::tf("Renamed remote {0} to {1}", &[&old, &name]))
                });
            }
            InputPurpose::SetRemoteUrl { root, name: remote } => {
                self.git_task(&crate::i18n::tf("Updating {}…", &[&remote]), cx, move |h| {
                    git::set_remote_url(h, &root, &remote, &name).map(|_| crate::i18n::tf("Updated the URL of {}", &[&remote]))
                });
            }
            InputPurpose::StashNamed { repo } if self.repo_unchanged(&repo, cx) => {
                self.git_stash_with(git::StashOptions { message: Some(name), ..Default::default() }, cx);
            }
            _ => {}
        }
    }

    pub(super) fn git_push_tags(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.repo_root(cx) else { return };
        self.git_task(crate::i18n::t("Pushing tags…"), cx, move |h| {
            git::push_tags(h, &root, None).map(|remote| crate::i18n::tf("Pushed tags to {}", &[&remote]))
        });
    }

    pub(super) fn git_prune_worktrees(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.repo_root(cx) else { return };
        self.git_task(crate::i18n::t("Pruning worktrees…"), cx, move |h| {
            git::prune_worktrees(h, &root).map(|n| crate::i18n::tn("Pruned {} stale worktree entry", "Pruned {} stale worktree entries", n, &[]))
        });
    }

    // --- Stash ---

    /// Stash in the background; the stash list in Source Control is reloaded afterwards (HEAD does not change, so nothing else would).
    pub(super) fn git_stash_with(&mut self, opts: git::StashOptions, cx: &mut Context<Self>) {
        let Some(root) = self.repo_root(cx) else { return };
        self.git_task_then(
            crate::i18n::t("Stashing…"),
            cx,
            move |h| git::stash_push(h, &root, &opts).map(|_| crate::i18n::t("Stashed changes").to_string()),
            |this, cx| this.scm.update(cx, |s, cx| s.load_graph(cx)),
        );
    }

    pub(super) fn git_stash_named(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.repo_root(cx).is_some() {
            self.show_input(
                InputPurpose::StashNamed { repo: self.scm.read(cx).target(None).ok() },
                // A git term, left as it is in every language (like the Stash entries of the "…" menu).
                "Stash",
                crate::i18n::t("Name of the stash (shown in the stash list)"),
                "",
                window,
                cx,
            );
        }
    }

    /// "Git: Show Stash…": the files the picked stash changed; one opens as a diff against the commit the stash was made on.
    pub(super) fn show_stash_files(&mut self, stash: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.repo_root(cx) else { return };
        let host = self.remote.host.clone();
        let (r, s) = (root.clone(), stash.clone());
        let task = cx.background_executor().spawn(async move {
            let files = git::stash_files(host.as_ref(), &r, &s)?;
            let subject = git::stash_list(host.as_ref(), &r).unwrap_or_default().into_iter().find(|(x, _)| *x == s).map(|(_, m)| m).unwrap_or_default();
            Ok::<_, String>((files, subject))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(((_, files), _)) if files.is_empty() => {
                    this.notice = Some(Notice::Info(crate::i18n::tf("{} holds no changes to tracked files.", &[&stash]).into()));
                    cx.notify();
                }
                Ok(((hash, files), subject)) => {
                    let commit = crate::graph::GraphCommit { hash, short: stash, subject, author: "stash".to_string(), ..Default::default() };
                    this.show_changed_files(root, commit, files, window, cx);
                }
                Err(e) => {
                    this.notice = Some(Notice::Error(crate::i18n::tf("Could not read the stash: {}", &[&first_line(&e)]).into()));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    // --- The open file against another revision ---

    pub(super) fn git_open_active(&mut self, head: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.active_file_path(cx) else { return };
        if head {
            self.git_open_head_file(path, window, cx);
        } else {
            self.git_open_changes(path, window, cx);
        }
    }

    /// "Open Changes": the file's diff in the same view Source Control uses (the staged diff when only the index differs).
    pub(super) fn git_open_changes(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, rel)) = self.repo_rel(&path, cx) else { return };
        let change = self.git_status().and_then(|s| s.changes.iter().find(|c| c.path == rel).cloned());
        match change {
            Some(c) => self.open_diff(rel, c.unstaged.is_none() && c.staged.is_some(), window, cx),
            None => {
                self.notice = Some(Notice::Info(crate::i18n::tf("{} has no changes.", &[&crate::ui::file_name(&rel)]).into()));
                cx.notify();
            }
        }
    }

    /// "Open File (HEAD)": the committed version in a scratch tab named "name (HEAD).ext".
    pub(super) fn git_open_head_file(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some((root, rel)) = self.repo_rel(&path, cx) else { return };
        let host = self.remote.host.clone();
        let r = rel.clone();
        let task = cx.background_executor().spawn(async move { git::show(host.as_ref(), &root, "HEAD", &r) });
        cx.spawn_in(window, async move |this, cx| {
            let text = task.await;
            this.update_in(cx, |this, window, cx| match text {
                Some(text) => {
                    this.create_scratch_with(head_copy_name(crate::ui::file_name(&rel)), &text, window, cx);
                }
                None => {
                    this.notice = Some(Notice::Info(crate::i18n::tf("{} is not in HEAD yet.", &[&crate::ui::file_name(&rel)]).into()));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Side by side: the file as it is at `rev` (under `rev_path`) and the editor's text, unsaved changes included.
    fn open_compare_with(&mut self, root: String, rev: String, rev_path: String, rev_label: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.active_editor().cloned() else { return };
        let (name, current) = {
            let e = ed.read(cx);
            (e.name.to_string(), e.text_lines().join("\n"))
        };
        let host = self.remote.host.clone();
        // A file that did not exist there compares as empty, like the first commit of a history.
        let task = cx.background_executor().spawn(async move { git::show(host.as_ref(), &root, &rev, &rev_path).unwrap_or_default() });
        cx.spawn_in(window, async move |this, cx| {
            let old = task.await;
            this.update_in(cx, |this, window, cx| {
                this.open_compare(format!("{name} ({rev_label}) ↔ {name}"), crate::i18n::t("Compare"), &old, &current, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// "Git: Compare with Commit…": the commits that changed the open file; the pick is compared with the editor's text.
    pub(super) fn show_compare_commit_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                    let names = log.into_iter().map(|e| format!("{}\t{}", e.hash, e.path)).collect();
                    let title = crate::i18n::tf("Compare {} with a commit", &[&crate::ui::file_name(&rel)]);
                    let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], &title, "", None, cx));
                    this.show_overlay(picker, OverlayKind::BranchOp { root, op: BranchOp::CompareCommit, names }, window, cx);
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

    // --- .gitignore ---

    pub(super) fn git_ignore_active(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.active_file_path(cx) {
            self.git_ignore_path(path, false, cx);
        }
    }

    /// Append the path to the repository's `.gitignore` and refresh the marks.
    pub(super) fn git_ignore_path(&mut self, path: String, is_dir: bool, cx: &mut Context<Self>) {
        let Some((root, rel)) = self.repo_rel(&path, cx) else { return };
        self.git_task(crate::i18n::t("Updating .gitignore…"), cx, move |h| {
            git::add_to_gitignore(h, &root, &rel, is_dir)
                .map(|added| if added { crate::i18n::tf("Added {} to .gitignore", &[&rel]) } else { crate::i18n::tf("{} is already in .gitignore", &[&rel]) })
        });
    }
}
