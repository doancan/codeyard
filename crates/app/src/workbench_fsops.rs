//! Workbench: Explorer file operations: create, rename, move, paste, delete; ssh prompts.

use super::*;

impl Workbench {
    /// A prompt from ssh: queue it, show it when its turn comes.
    pub fn ask(&mut self, request: AskRequest, reply: AskReply, window: &mut Window, cx: &mut Context<Self>) {
        self.remote.asks.push_back((request, reply));
        self.next_ask(window, cx);
    }

    #[cfg(test)]
    pub fn pending_asks(&self) -> (bool, usize) {
        (self.asking(), self.remote.asks.len())
    }

    pub(super) fn asking(&self) -> bool {
        matches!(self.overlay, Some(Overlay { kind: OverlayKind::Ask { .. }, .. }))
    }

    pub(super) fn next_ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.asking() {
            return;
        }
        let Some((request, reply)) = self.remote.asks.pop_front() else { return };
        // If another picker is open, close it as if Esc was pressed (so the theme preview is reverted).
        if self.overlay.is_some() {
            self.on_overlay_event(&PickerEvent::Dismiss, window, cx);
        }
        let prompt = request.prompt.trim().to_string();
        let lower = prompt.to_lowercase();
        let picker = cx.new(|cx| {
            let picker = match request.kind {
                AskKind::Secret => {
                    let what = if lower.contains("passphrase") {
                        crate::i18n::t("Passphrase")
                    } else if lower.contains("password") {
                        crate::i18n::t("Password")
                    } else {
                        crate::i18n::t("Response")
                    };
                    Picker::new(vec![PickSource::Secret { verb: crate::i18n::t("Send") }], what, "", None, cx)
                }
                AskKind::HostKey | AskKind::Confirm => {
                    let (yes, no) = if request.kind == AskKind::HostKey {
                        (crate::i18n::t("Trust this host and connect"), crate::i18n::t("Cancel"))
                    } else {
                        (crate::i18n::t("Allow"), crate::i18n::t("Deny"))
                    };
                    let items = vec![PickItem { label: yes.into(), detail: None, keys: None }, PickItem { label: no.into(), detail: None, keys: None }];
                    Picker::new(vec![PickSource::List { prefix: "", items }], "", "", None, cx)
                }
            };
            picker.with_message(prompt)
        });
        self.show_overlay(picker, OverlayKind::Ask { kind: request.kind, reply }, window, cx);
    }

    pub(super) fn finish_ask(&mut self, answer: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Overlay { kind: OverlayKind::Ask { reply, .. }, .. }) = self.close_overlay(window, cx) {
            let _ = reply.send(answer);
        }
        self.next_ask(window, cx);
    }

    pub(super) fn show_input(
        &mut self,
        purpose: InputPurpose,
        verb: &'static str,
        placeholder: &str,
        initial: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.asking() {
            return;
        }
        if self.overlay.is_some() {
            self.close_overlay(window, cx);
        }
        let path = matches!(
            purpose,
            InputPurpose::Create { .. }
                | InputPurpose::NewFile { .. }
                | InputPurpose::SaveAs { .. }
                | InputPurpose::CreateBranch { .. }
                | InputPurpose::CommitBranch { .. }
                | InputPurpose::CommitTag { .. }
                | InputPurpose::CloneTarget { .. }
        );
        let picker = cx.new(|cx| Picker::new(vec![PickSource::Input { verb, path }], placeholder, initial, None, cx));
        self.show_overlay(picker, OverlayKind::Input(purpose), window, cx);
    }

    pub fn start_create(&mut self, dir: String, is_dir: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (verb, placeholder) = if is_dir {
            (crate::i18n::t("Create folder"), crate::i18n::t("Name of the new folder"))
        } else {
            (crate::i18n::t("Create file"), crate::i18n::t("Name of the new file"))
        };
        self.show_input(InputPurpose::Create { dir, is_dir }, verb, placeholder, "", window, cx);
    }

    pub(super) fn start_rename(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.all_roots(cx).contains(&path) {
            return;
        }
        let name = ui::file_name(&path).to_string();
        self.show_input(InputPurpose::Rename { path }, crate::i18n::t("Rename to"), crate::i18n::t("New name"), &name, window, cx);
    }

    /// A file operation in the background; `then` on completion, a status bar message on error.
    pub(super) fn fs_op(
        &mut self,
        op: impl FnOnce(&dyn Host) -> host::Result<()> + Send + 'static,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let host = self.remote.host.clone();
        let task = cx.background_executor().spawn(async move { op(host.as_ref()) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(()) => then(this, window, cx),
                Err(e) => {
                    this.notice = Some(Notice::Error(e.to_string().into()));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn finish_input(&mut self, purpose: InputPurpose, name: String, window: &mut Window, cx: &mut Context<Self>) {
        match purpose {
            InputPurpose::RenameSymbol { client, path, pos } => self.rename_symbol_to(client, path, pos, name, cx),
            InputPurpose::EditBreakpoint { path, line } => self.finish_edit_breakpoint(path, line, name, cx),
            InputPurpose::AddWatch => self.finish_add_watch(name, cx),
            InputPurpose::DataBreakpoint => self.finish_data_breakpoint(name, cx),
            InputPurpose::NewFile { base } => {
                let target = if name.starts_with('/') { name.clone() } else { ui::join(&base, &name) };
                let (t, parent) = (target.clone(), parent_of(&target));
                self.fs_op(
                    move |h| h.create_file(&t),
                    move |this, window, cx| {
                        this.roots.primary.update(cx, |t, cx| t.reload(&parent, cx));
                        this.roots.file_index = None;
                        this.open_file(target, window, cx);
                    },
                    window,
                    cx,
                );
            }
            InputPurpose::SaveAs { from } => self.save_as_to(from, name, window, cx),
            InputPurpose::SaveLocalCopy { from } => self.save_local_copy_to(from, name, cx),
            InputPurpose::CreateBranch { root } => self.git_checkout(root, name, false, true, cx),
            InputPurpose::RenameBranch { root, old } => {
                let n = name.trim().to_string();
                self.git_task(&crate::i18n::tf("Renaming {}…", &[&old]), cx, move |h| {
                    crate::git::rename_branch(h, &root, &old, &n).map(|_| crate::i18n::tf("Renamed {0} to {1}", &[&old, &n]))
                });
            }
            InputPurpose::AddWorktree { root } => match name.trim().rsplit_once(char::is_whitespace) {
                Some((path, branch)) if !path.trim().is_empty() => {
                    let (path, branch) = (path.trim().to_string(), branch.trim().to_string());
                    self.git_task(crate::i18n::t("Adding worktree…"), cx, move |h| {
                        crate::git::add_worktree(h, &root, &path, &branch)
                            .map(|_| crate::i18n::tf("Added worktree {}: open it with Git: Open Worktree…", &[&path]))
                    });
                }
                _ => {
                    self.notice = Some(Notice::Error(crate::i18n::t("Enter: path branch").into()));
                    cx.notify();
                }
            },
            InputPurpose::AddRemote { root } => match name.trim().split_once(char::is_whitespace) {
                Some((n, url)) if !url.trim().is_empty() => {
                    let (n, url) = (n.to_string(), url.trim().to_string());
                    self.git_task(crate::i18n::t("Adding remote…"), cx, move |h| {
                        crate::git::add_remote(h, &root, &n, &url).map(|_| crate::i18n::tf("Added remote {}", &[&n]))
                    });
                }
                _ => {
                    self.notice = Some(Notice::Error(crate::i18n::t("Enter the remote as: name url").into()));
                    cx.notify();
                }
            },
            InputPurpose::Scratch => self.create_scratch(name, window, cx),
            // --- compare ---
            InputPurpose::CompareFolder { first } => self.finish_compare_input(first, name, window, cx),
            InputPurpose::SaveTheme => self.save_theme_as(name, window, cx),
            InputPurpose::CloneUrl => self.ask_clone_target(name, window, cx),
            InputPurpose::ConnectHost => {
                let host = name.trim().to_string();
                if !host.is_empty() {
                    self.connect(host, window, cx);
                }
            }
            InputPurpose::AddSshHost => self.finish_add_ssh_host(name, window, cx),
            InputPurpose::EditHostField { alias, field } => self.finish_edit_host_field(alias, field, name, cx),
            InputPurpose::FilterLog { path } => self.run_filter_log(path, name, window, cx),
            InputPurpose::CloneTarget { url } => self.run_clone(url, name, cx),
            InputPurpose::CommitBranch { hash } => self.scm.update(cx, |s, cx| s.commit_op(hash, crate::graph::CommitOp::Branch(name), window, cx)),
            InputPurpose::CommitTag { hash } => {
                // Tag names cannot hold a space, so whatever follows the first word is the message of an annotated tag.
                let op = match name.trim().split_once(char::is_whitespace) {
                    Some((tag, message)) if !message.trim().is_empty() => {
                        crate::graph::CommitOp::AnnotatedTag { name: tag.to_string(), message: message.trim().to_string() }
                    }
                    _ => crate::graph::CommitOp::Tag(name.trim().to_string()),
                };
                self.scm.update(cx, |s, cx| s.commit_op(hash, op, window, cx))
            }
            // --- git-refs ---
            InputPurpose::CreateBranchFrom { .. } | InputPurpose::RenameRemote { .. } | InputPurpose::SetRemoteUrl { .. } | InputPurpose::StashNamed => {
                self.finish_ref_input(purpose, name, cx)
            }
            InputPurpose::SaveProject { path } => self.save_project(path, name, cx),
            InputPurpose::Create { dir, is_dir } => {
                let target = ui::join(&dir, &name);
                let t = target.clone();
                self.fs_op(
                    move |h| if is_dir { h.create_dir(&t) } else { h.create_file(&t) },
                    move |this, window, cx| {
                        this.roots.primary.update(cx, |t, cx| t.reload(&dir, cx));
                        this.roots.file_index = None;
                        if !is_dir {
                            this.open_file(target, window, cx);
                        }
                    },
                    window,
                    cx,
                );
            }
            InputPurpose::Setting { key } => self.finish_setting_input(key, name, window, cx),
            InputPurpose::Rename { path } => {
                let target = ui::join(&parent_of(&path), &name);
                if target != path {
                    self.move_path(path, target, false, window, cx);
                }
            }
        }
    }

    /// Move or rename a path; open tabs, backups and favorites follow the new path.
    /// `check`: stop if the target exists (rename silently overwrites it; for drag-and-drop and paste).
    pub(super) fn move_path(&mut self, path: String, target: String, check: bool, window: &mut Window, cx: &mut Context<Self>) {
        // Language servers can update import paths before the move (`willRenameFiles`).
        let clients: Vec<Arc<crate::lsp::Client>> = self
            .lsp
            .servers
            .values()
            .filter_map(|s| match s {
                lsp_glue::ServerSlot::Running(c) if c.capabilities["workspace"]["fileOperations"]["willRename"].is_object() => Some(c.clone()),
                _ => None,
            })
            .collect();
        if clients.is_empty() {
            return self.move_path_now(path, target, check, window, cx);
        }
        let params = serde_json::json!({ "files": [{ "oldUri": crate::lsp::path_to_uri(&path), "newUri": crate::lsp::path_to_uri(&target) }] });
        let (host, to) = (self.remote.host.clone(), target.clone());
        let task = cx.background_executor().spawn(async move {
            // Don't ask the server if the target is taken: the move gets rejected anyway, and imports shouldn't break.
            if check && host.stat(&to).is_ok() {
                return (Vec::new(), Vec::new());
            }
            let mut edits = Vec::new();
            let mut failures = Vec::new();
            for c in &clients {
                match c.request("workspace/willRenameFiles", params.clone(), Duration::from_secs(5)) {
                    Ok(v) if !v.is_null() => edits.push(v),
                    Ok(_) => {}
                    Err(e) => failures.push(format!("{}: {e}", c.config.language)),
                }
            }
            (edits, failures)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (edits, failures) = task.await;
            this.update_in(cx, |this, window, cx| {
                for f in failures {
                    this.log_output(format!("willRenameFiles failed ({f}); imports were not updated"));
                }
                // Edits apply *after* the move (paths mapped to the new location), so a background task
                // writing to the moved file can't race the rename.
                this.lsp.pending_rename_edits = edits.into_iter().map(|e| crate::lsp::remap_edit_uris(&e, &path, &target)).collect();
                this.move_path_now(path, target, check, window, cx);
            })
            .ok();
        })
        .detach();
    }

    #[cfg(test)]
    pub fn move_path_for_test(&mut self, from: String, to: String, window: &mut Window, cx: &mut Context<Self>) {
        self.move_path(from, to, false, window, cx);
    }

    pub(super) fn move_path_now(&mut self, path: String, target: String, check: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (from, to) = (path.clone(), target.clone());
        let edits = std::mem::take(&mut self.lsp.pending_rename_edits);
        self.fs_op(
            move |h| {
                if check && h.stat(&to).is_ok() {
                    return Err(host::HostError::Op(proto::Error::AlreadyExists(to.clone())));
                }
                h.rename(&from, &to)
            },
            move |this, _, cx| {
                for edit in &edits {
                    this.apply_workspace_edit(edit, cx);
                }
                let (old_parent, new_parent) = (parent_of(&path), parent_of(&target));
                this.roots.primary.update(cx, |t, cx| {
                    t.reload(&old_parent, cx);
                    if new_parent != old_parent {
                        t.reload(&new_parent, cx);
                    }
                });
                this.roots.file_index = None;
                // Open tabs follow the new path (including those under it for a folder).
                let under = format!("{path}/");
                for ed in this.all_editors() {
                    let p = ed.read(cx).path.clone();
                    let new = if p == path { Some(target.clone()) } else { p.strip_prefix(&under).map(|rest| format!("{target}/{rest}")) };
                    if let Some(new) = new {
                        ed.update(cx, |e, cx| e.set_path(new, cx));
                        // The hot-exit backup must not stay at the old path (ghost tab on reopen).
                        if this.persist.backed_up.contains_key(&p) {
                            this.drop_backup(&p, cx);
                        }
                    }
                }
                this.favorites_moved(&path, Some(&target), cx);
                this.schedule_backup(cx);
                this.persist_session(cx);
                cx.notify();
            },
            window,
            cx,
        );
    }

    /// Drag-and-drop: move into the folder (dropping into itself or onto its current place does nothing).
    /// Explorer drag-and-drop: asks before moving if `explorer.confirmDragAndDrop` is on.
    pub(super) fn confirm_drop(&mut self, from: String, to_dir: String, window: &mut Window, cx: &mut Context<Self>) {
        let noop = to_dir.is_empty() || to_dir == from || to_dir.starts_with(&format!("{from}/")) || parent_of(&from) == to_dir;
        if noop || !cx.global::<Settings>().confirm_drag_and_drop {
            return self.drop_entry(from, to_dir, window, cx);
        }
        let title = crate::i18n::tf("Move “{0}” into “{1}”?", &[&ui::file_name(&from), &ui::file_name(&to_dir)]);
        let answer = window.prompt(
            PromptLevel::Info,
            &title,
            Some(crate::i18n::t("An existing file with the same name is never overwritten.")),
            &[crate::i18n::t("Move"), crate::i18n::t("Cancel")],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                this.update_in(cx, |this, window, cx| this.drop_entry(from, to_dir, window, cx)).ok();
            }
        })
        .detach();
    }

    pub(super) fn drop_entry(&mut self, from: String, to_dir: String, window: &mut Window, cx: &mut Context<Self>) {
        if to_dir.is_empty() || to_dir == from || to_dir.starts_with(&format!("{from}/")) || parent_of(&from) == to_dir {
            return;
        }
        let target = ui::join(&to_dir, ui::file_name(&from));
        self.move_path(from, target, true, window, cx);
    }

    /// Tree ⌘V: move if cut, copy as "name copy.ext" if copied.
    pub(super) fn paste_entry(&mut self, dir: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some((from, cut)) = self.roots.file_clip.clone() else { return };
        if cut {
            self.roots.file_clip = None;
            self.drop_entry(from, dir, window, cx);
            return;
        }
        if dir == from || dir.starts_with(&format!("{from}/")) {
            self.notice = Some(Notice::Error(crate::i18n::t("Can't copy a folder into itself.").into()));
            cx.notify();
            return;
        }
        let host = self.remote.host.clone();
        let reload = dir.clone();
        let task = cx.background_executor().spawn(async move {
            let name = ui::file_name(&from).to_string();
            // Keep the name unless taken; else "name copy.ext", "name copy 2.ext"… (VS Code).
            let target = std::iter::once(ui::join(&dir, &name))
                .chain((0..1000).map(|n| ui::join(&dir, &copy_name(&name, n))))
                .find(|t| host.stat(t).is_err())
                .ok_or_else(|| crate::i18n::t("too many copies").to_string())?;
            // The protocol has no copy: run `cp` on the machine where the files are (on the server over SSH).
            let out = host.exec("cp", &["-R", "--", &from, &target], None, None).map_err(|e| e.to_string())?;
            if !out.ok() {
                return Err(out.stderr_str());
            }
            let is_file = host.stat(&target).is_ok_and(|m| m.kind == proto::Kind::File);
            Ok((target, is_file))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result: Result<(String, bool), String> = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok((target, is_file)) => {
                    this.roots.primary.update(cx, |t, cx| {
                        t.reload(&reload, cx);
                        t.reveal(&target, cx);
                    });
                    this.roots.file_index = None;
                    if is_file {
                        this.open_file(target, window, cx);
                    }
                }
                Err(e) => {
                    this.notice = Some(Notice::Error(crate::i18n::tf("Could not paste: {}", &[&e.trim()]).into()));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn confirm_delete(&mut self, path: String, is_dir: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.all_roots(cx).contains(&path) {
            return;
        }
        let name = ui::file_name(&path).to_string();
        let under = format!("{path}/");
        let dirty = self.all_editors().iter().any(|e| {
            let e = e.read(cx);
            e.is_dirty() && (e.path == path || e.path.starts_with(&under))
        });
        // Local: trash (recoverable). SSH: no server trash, so delete permanently.
        let trash = !self.remote.host.is_remote() && cx.global::<Settings>().enable_trash;
        let unsaved = if dirty { crate::i18n::t("Unsaved changes stay open in their tabs; saving recreates the file.") } else { "" };
        let sep = if unsaved.is_empty() { "" } else { " " };
        let (title, detail, verb) = if trash {
            (
                crate::i18n::tf("Move “{}” to the Trash?", &[&name]),
                format!("{}{sep}{unsaved}", crate::i18n::t("You can restore it from the Trash.")),
                crate::i18n::t("Move to Trash"),
            )
        } else {
            let what = if is_dir { crate::i18n::t("The folder and everything in it will be deleted.") } else { "" };
            let sep_what = if what.is_empty() { "" } else { " " };
            (
                crate::i18n::tf("Delete “{}” permanently?", &[&name]),
                format!("{what}{sep_what}{}{sep}{unsaved}", crate::i18n::t("This can't be undone.")),
                crate::i18n::t("Delete"),
            )
        };
        // The trash is recoverable: if the setting is off, move without asking.
        let ask = !trash || cx.global::<Settings>().confirm_delete;
        let answer = ask.then(|| window.prompt(PromptLevel::Warning, &title, Some(&detail), &[verb, crate::i18n::t("Cancel")], cx));
        cx.spawn_in(window, async move |this, cx| {
            if let Some(answer) = answer
                && answer.await != Ok(0)
            {
                return;
            }
            this.update_in(cx, |this, window, cx| {
                let p = path.clone();
                if !trash {
                    this.fs_op(move |h| h.delete(&p), move |this, window, cx| this.after_delete(path, window, cx), window, cx);
                    return;
                }
                let task = cx.background_executor().spawn(async move { crate::trash::move_to_trash(std::path::Path::new(&p)) });
                cx.spawn_in(window, async move |this, cx| {
                    let result = task.await;
                    this.update_in(cx, |this, window, cx| match result {
                        Ok(_) => this.after_delete(path, window, cx),
                        // E.g. a file on another disk can't go to the trash; ask explicitly about permanent deletion.
                        Err(e) => this.confirm_permanent_delete(path, e.to_string(), window, cx),
                    })
                    .ok();
                })
                .detach();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn confirm_permanent_delete(&mut self, path: String, why: String, window: &mut Window, cx: &mut Context<Self>) {
        let name = ui::file_name(&path).to_string();
        let detail = crate::i18n::tf("It can't be moved to the Trash ({}). Deleting it permanently can't be undone.", &[&why]);
        let answer = window.prompt(
            PromptLevel::Warning,
            &crate::i18n::tf("Delete “{}” permanently?", &[&name]),
            Some(&detail),
            &[crate::i18n::t("Delete Permanently"), crate::i18n::t("Cancel")],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            this.update_in(cx, |this, window, cx| {
                let p = path.clone();
                this.fs_op(move |h| h.delete(&p), move |this, window, cx| this.after_delete(path, window, cx), window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Terminal ⌘-click: URL opens in this machine's browser; file location in the editor. A relative path
    /// is relative to the project root (the terminal opens there); files are looked up on the host (on the server over SSH).
    pub(crate) fn open_link(&mut self, link: crate::links::Link, window: &mut Window, cx: &mut Context<Self>) {
        use crate::links::Link;
        match link {
            Link::Url(url) => {
                cx.open_url(&url);
            }
            Link::File { path, line, col } => {
                let candidates: Vec<String> =
                    if ui::is_absolute_path(&path) { vec![path.clone()] } else { self.root(cx).map(|r| ui::join(&r, &path)).into_iter().collect() };
                let host = self.remote.host.clone();
                let task =
                    cx.background_executor().spawn(async move { candidates.into_iter().find(|c| host.stat(c).is_ok_and(|m| m.kind == proto::Kind::File)) });
                cx.spawn_in(window, async move |this, cx| {
                    let found = task.await;
                    this.update_in(cx, |this, window, cx| match found {
                        Some(abs) => {
                            let (l, c) = (line.unwrap_or(1).saturating_sub(1), col.unwrap_or(1).saturating_sub(1));
                            this.open_location(abs, l, c, window, cx);
                        }
                        None => {
                            this.notice = Some(Notice::Info(crate::i18n::tf("No file “{}” in this project", &[&path]).into()));
                            cx.notify();
                        }
                    })
                    .ok();
                })
                .detach();
            }
        }
    }

    /// Drop the deleted path from the tree, tabs and favorites.
    pub(super) fn after_delete(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let parent = parent_of(&path);
        self.reload_in_trees(&parent, cx);
        self.roots.file_index = None;
        // Clean tabs on the deleted path are closed (also those in the other group); one with
        // unsaved changes stays open and is marked "deleted from disk".
        let under = format!("{path}/");
        let gone = |e: &Entity<EditorView>, cx: &App| {
            let ep = &e.read(cx).path;
            *ep == path || ep.starts_with(&under)
        };
        for e in self.all_editors() {
            if gone(&e, cx) && e.read(cx).is_dirty() {
                e.update(cx, |e, cx| {
                    e.mark_deleted_on_disk();
                    cx.notify();
                });
            }
        }
        if let Some(g) = &mut self.tabs.other {
            let active = g.active.and_then(|a| g.editors.get(a)).cloned();
            g.editors.retain(|e| !gone(e, cx) || e.read(cx).is_dirty());
            g.active = active.and_then(|a| g.editors.iter().position(|e| *e == a)).or((!g.editors.is_empty()).then_some(0));
            if g.editors.is_empty() {
                self.tabs.other = None;
                self.tabs.focused_left = true; // a single group is always "left"
            }
        }
        while let Some(ix) = self.tabs.editors.iter().position(|e| gone(e, cx) && !e.read(cx).is_dirty()) {
            self.remove_editor(ix, window, cx);
        }
        self.favorites_moved(&path, None, cx);
    }
}
