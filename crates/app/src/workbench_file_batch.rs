//! Explorer batch confirmations, progress and reconciliation of completed side effects.

use std::sync::atomic::Ordering;

use super::*;
use crate::file_batch::{Clipboard, Operation, Progress};

impl Workbench {
    /// The host a file question names: the connection's alias, as the status bar shows it, else the host's label.
    pub(super) fn file_host_name(&self) -> String {
        match &self.remote.conn {
            Conn::Connected { alias } if self.remote.host.is_remote() => alias.clone(),
            _ => self.remote.host.label(),
        }
    }

    pub(super) fn set_file_clipboard(&mut self, paths: Vec<String>, cut: bool, cx: &App) {
        let paths = crate::file_batch::top_level(paths, &self.all_roots(cx));
        if !paths.is_empty() {
            self.roots.file_clip = Some(Arc::new(Clipboard { host: self.remote.host.clone(), paths, cut }));
        }
    }

    /// A context-menu operation uses the marked set only when its target belongs to it.
    pub(super) fn context_file_paths(&self, target: &str, cx: &App) -> Vec<String> {
        self.all_trees()
            .iter()
            .find_map(|tree| {
                let tree = tree.read(cx);
                tree.path_is_selected(target).then(|| tree.selected_paths())
            })
            .unwrap_or_else(|| vec![target.to_string()])
    }

    pub(super) fn cancel_file_operation(&mut self, _: &CancelFileOperation, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(progress) = &self.roots.file_operation {
            progress.cancel.store(true, Ordering::Release);
            cx.notify();
        }
    }

    pub(super) fn invalidate_file_operation(&mut self) {
        if let Some(progress) = self.roots.file_operation.take() {
            progress.cancel.store(true, Ordering::Release);
        }
    }

    pub(super) fn paste_file_clipboard(&mut self, dir: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(clip) = self.roots.file_clip.clone() else { return };
        if !Arc::ptr_eq(&clip.host, &self.remote.host) {
            self.notice = Some(Notice::Error(crate::i18n::t("Copy the files again on the current host before pasting.").into()));
            cx.notify();
            return;
        }
        let op = if clip.cut { Operation::Move(dir) } else { Operation::Copy(dir) };
        self.start_file_batch(clip.paths.clone(), op, Some(clip), false, window, cx);
    }

    pub(super) fn move_file_selection(&mut self, paths: Vec<String>, dir: String, window: &mut Window, cx: &mut Context<Self>) {
        let paths: Vec<_> = paths.into_iter().filter(|p| parent_of(p) != dir && *p != dir && !dir.starts_with(&format!("{p}/"))).collect();
        let confirm = cx.global::<Settings>().confirm_drag_and_drop;
        self.start_file_batch(paths, Operation::Move(dir), None, confirm, window, cx);
    }

    pub(super) fn delete_file_selection(&mut self, paths: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let trash = !self.remote.host.is_remote() && cx.global::<Settings>().enable_trash;
        // A batch always gets a summary even when recoverable single-file deletion is configured not to ask.
        self.start_file_batch(paths, Operation::Delete { trash }, None, true, window, cx);
    }

    fn start_file_batch(
        &mut self,
        paths: Vec<String>,
        operation: Operation,
        clipboard: Option<Arc<Clipboard>>,
        confirm: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.roots.file_operation.is_some() {
            self.notice = Some(Notice::Info(crate::i18n::t("Wait for the file operation to finish, or cancel it from the status bar.").into()));
            cx.notify();
            return;
        }
        let paths = crate::file_batch::top_level(paths, &self.all_roots(cx));
        if paths.is_empty() {
            return;
        }
        let progress = Arc::new(Progress { total: paths.len(), ..Default::default() });
        self.roots.file_operation = Some(progress.clone());
        let host = self.remote.host.clone();
        let root = self.root(cx);
        let question = crate::file_batch::question(&operation, paths.len(), &self.file_host_name(), host.is_remote());
        let answer = if confirm || paths.len() > 1 {
            let mut detail = question.detail.clone();
            if matches!(operation, Operation::Delete { .. })
                && self.all_editors().iter().any(|e| {
                    let e = e.read(cx);
                    e.is_dirty() && paths.iter().any(|p| e.path == *p || e.path.starts_with(&format!("{p}/")))
                })
            {
                detail.push_str(&format!("\n{}", crate::i18n::t("Unsaved changes stay open in their tabs; saving recreates the file.")));
            }
            for path in paths.iter().take(12) {
                detail.push_str(&format!("\n{path}"));
            }
            if paths.len() > 12 {
                detail.push_str(&format!("\n{}", crate::i18n::t("The complete file list is available in Output.")));
            }
            self.log_output(format!("{}:\n{}", question.verb, paths.join("\n")));
            let level = if question.destructive { PromptLevel::Critical } else { PromptLevel::Warning };
            let buttons = [gpui::PromptButton::new(question.verb.clone()), gpui::PromptButton::cancel(crate::i18n::t("Cancel"))];
            Some(window.prompt(level, &question.title, Some(&detail), &buttons, cx))
        } else {
            None
        };
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let accepted = match answer {
                Some(answer) => answer.await == Ok(0),
                None => true,
            };
            this.update_in(cx, |this, window, cx| {
                let current = Arc::ptr_eq(&host, &this.remote.host)
                    && this.root(cx) == root
                    && this.roots.file_operation.as_ref().is_some_and(|p| Arc::ptr_eq(p, &progress));
                if !current {
                    return;
                }
                if !accepted || progress.cancel.load(Ordering::Acquire) {
                    this.roots.file_operation = None;
                    cx.notify();
                    return;
                }
                let clients: Vec<_> = if matches!(operation, Operation::Move(_)) {
                    this.lsp
                        .servers
                        .values()
                        .filter_map(|slot| match slot {
                            lsp_glue::ServerSlot::Running(c) if c.capabilities["workspace"]["fileOperations"]["willRename"].is_object() => Some(c.clone()),
                            _ => None,
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                let (h, ps, op, run) = (host.clone(), paths.clone(), operation.clone(), progress.clone());
                let task = cx.background_executor().spawn(async move { crate::file_batch::run(h.as_ref(), &ps, &op, &run, &clients) });
                cx.spawn_in(window, async move |this, cx| {
                    let result = task.await;
                    this.update_in(cx, |this, window, cx| {
                        if !Arc::ptr_eq(&host, &this.remote.host)
                            || this.root(cx) != root
                            || !this.roots.file_operation.as_ref().is_some_and(|p| Arc::ptr_eq(p, &progress))
                        {
                            return;
                        }
                        this.roots.file_operation = None;
                        if let Some(clip) = clipboard.filter(|clip| clip.cut)
                            && this.roots.file_clip.as_ref().is_some_and(|now| Arc::ptr_eq(now, &clip))
                        {
                            let completed: std::collections::HashSet<_> = result.completed.iter().map(|c| &c.source).collect();
                            let remaining: Vec<_> = clip.paths.iter().filter(|p| !completed.contains(p)).cloned().collect();
                            this.roots.file_clip = (!remaining.is_empty()).then(|| Arc::new(Clipboard { paths: remaining, ..(*clip).clone() }));
                        }
                        for item in &result.completed {
                            match (&operation, &item.target) {
                                (Operation::Move(_), Some(target)) => {
                                    this.after_move(item.source.clone(), target.clone(), cx);
                                    for edit in &item.edits {
                                        this.apply_workspace_edit(edit, cx);
                                    }
                                }
                                (Operation::Copy(_), Some(target)) => {
                                    this.reload_in_trees(&parent_of(target), cx);
                                }
                                (Operation::Delete { .. }, _) => this.after_delete(item.source.clone(), window, cx),
                                _ => {}
                            }
                        }
                        for warning in &result.warnings {
                            this.log_output(warning.clone());
                        }
                        for (path, error) in &result.failed {
                            this.log_output(format!("{path}: {error}"));
                        }
                        this.roots.file_index = None;
                        // Keep the established single-copy behavior; a batch does not steal the selection or open many tabs.
                        if matches!(operation, Operation::Copy(_))
                            && paths.len() == 1
                            && let Some(target) = result.completed.first().filter(|r| r.is_file).and_then(|r| r.target.as_ref())
                        {
                            this.open_file(target.clone(), window, cx);
                        }
                        let summary = crate::i18n::tf(
                            "File operation: {0} completed, {1} failed, {2} not processed.",
                            &[&result.completed.len(), &result.failed.len(), &result.remaining.len()],
                        );
                        this.notice = Some(if let Some((path, error)) = result.failed.first() {
                            Notice::Error(format!("{summary}\n{path}: {error}").into())
                        } else {
                            Notice::Info(summary.into())
                        });
                        this.schedule_backup(cx);
                        this.persist_session(cx);
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn after_move(&mut self, path: String, target: String, cx: &mut Context<Self>) {
        self.reload_in_trees(&parent_of(&path), cx);
        self.reload_in_trees(&parent_of(&target), cx);
        self.roots.file_index = None;
        let under = format!("{path}/");
        for ed in self.all_editors() {
            let old = ed.read(cx).path.clone();
            let new = if old == path { Some(target.clone()) } else { old.strip_prefix(&under).map(|rest| format!("{target}/{rest}")) };
            if let Some(new) = new {
                ed.update(cx, |e, cx| e.set_path(new, cx));
                if self.persist.backed_up.contains_key(&old) {
                    self.drop_backup(&old, cx);
                }
            }
        }
        self.favorites_moved(&path, Some(&target), cx);
    }
}
