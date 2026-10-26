//! Workbench: rendering of the whole window (`Render`).

use super::*;

impl Workbench {
    fn finish_resize(&mut self, cx: &mut Context<Self>) {
        if self.sections.end_drag() {
            self.persist_sections(cx);
        }
        if std::mem::take(&mut self.chrome.resizing_sidebar) {
            if let Some(s) = &self.persist.state {
                let _ = s.set("sidebar_w", &format!("{:.0}", self.chrome.sidebar_w));
            }
            cx.notify();
        }
        if std::mem::take(&mut self.chrome.resizing_split) {
            cx.notify();
        }
        if std::mem::take(&mut self.chrome.resizing_panel) {
            if let Some(s) = &self.persist.state {
                let _ = s.set("panel_h", &format!("{:.0}", self.chrome.panel_h));
                let _ = s.set("panel_w", &format!("{:.0}", self.chrome.panel_w));
            }
            cx.notify();
        }
    }
}

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.update_title(window, cx);
        self.chrome.window_bounds = Some(window.window_bounds());
        self.track_notice(cx);
        self.sync_watches(cx);
        self.sync_slots(cx);
        self.sync_timeline(cx);
        self.sync_outline(cx);
        self.sync_update_window(window, cx);
        if let Some(ws) = self.persist.pending_restore.take() {
            self.restore_tabs(ws, window, cx);
        }
        let p = *palette(cx);
        let main: AnyElement = match (self.active_editor(), &self.tabs.open_error) {
            (_, Some((path, msg))) => center(&p)
                .child(icon("alert", 32., p.warning))
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(ui::file_name(path).to_string()))
                .child(div().max_w(px(480.)).text_color(c(p.text_muted)).child(msg.clone()))
                .when(msg.contains("binary file") || *msg == crate::i18n::t("This looks like a binary file, so it was not opened as text."), |d| {
                    let target = path.clone();
                    let entity = cx.entity();
                    d.child(button("open-hex", crate::i18n::t("Open as Hex"), true, &p, move |_, window, cx| {
                        let target = target.clone();
                        entity.update(cx, |this, cx| this.open_hex_of(target, window, cx));
                    }))
                })
                .into_any_element(),
            (_, None) if self.tabs.page_active && self.tabs.page.is_some() => match self.tabs.page.clone().unwrap() {
                Page::Welcome => self.welcome_view(&p, cx),
                Page::Settings(view) => view.into_any_element(),
                Page::Markdown(view) => view.into_any_element(),
                Page::Shortcuts(view) => view.into_any_element(),
                Page::Merge(view) => view.into_any_element(),
                Page::Compare(view) => view.into_any_element(),
            },
            (_, None) if self.tabs.diff_active && self.tabs.diff.is_some() => self.tabs.diff.clone().unwrap().into_any_element(),
            (_, None) if self.tabs.large_active && self.tabs.large.is_some() => self.tabs.large.clone().unwrap().into_any_element(),
            (Some(ed), None) => editor::slot_view(ed.clone(), self.focused_slot()).into_any_element(),
            (None, None) => self.empty_state(&p, cx),
        };
        let sidebar = self.chrome.panel.map(|panel| match panel {
            Panel::Explorer => self.explorer_panel(&p, cx),
            Panel::Search => div()
                .flex()
                .flex_col()
                .size_full()
                .child(self.view_header(crate::i18n::upper(crate::i18n::t("Search")), &p))
                .child(div().flex_1().min_h_0().child(self.search.clone()))
                .into_any_element(),
            Panel::Scm => self.scm.clone().into_any_element(),
            Panel::Remote => self.remote_view(&p, cx),
        });

        div()
            .id("workbench")
            .key_context("Workbench")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::open_folder))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::trust_this_folder))
            .on_action(cx.listener(Self::show_explorer))
            .on_action(cx.listener(Self::switch_editor))
            .on_action(cx.listener(Self::toggle_zen))
            .on_action(cx.listener(Self::toggle_full_screen))
            .on_action(cx.listener(Self::find_todos))
            .on_action(cx.listener(Self::show_remote))
            .on_action(cx.listener(Self::show_remote_explorer))
            .on_action(cx.listener(Self::toggle_breakpoint_enabled))
            .on_action(cx.listener(Self::enable_all_breakpoints))
            .on_action(cx.listener(Self::disable_all_breakpoints))
            .on_action(cx.listener(Self::toggle_breakpoints_active))
            .on_action(cx.listener(Self::add_function_breakpoint))
            .on_action(cx.listener(Self::show_scm))
            .on_action(cx.listener(Self::show_problems))
            .on_action(cx.listener(Self::show_ports))
            .on_action(cx.listener(Self::rename_symbol))
            .on_action(cx.listener(Self::find_references))
            .on_action(cx.listener(Self::go_to_symbol))
            .on_action(cx.listener(Self::go_to_workspace_symbol))
            .on_action(cx.listener(Self::code_actions))
            .on_action(cx.listener(Self::format_document))
            .on_action(cx.listener(Self::format_selection))
            .on_action(cx.listener(Self::save_all))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::split_editor))
            .on_action(cx.listener(Self::go_back))
            .on_action(cx.listener(|this, _: &CloseAllEditors, window, cx| this.close_all_editors(window, cx)))
            .on_action(cx.listener(|this, _: &CloseOtherEditors, window, cx| this.close_other_editors(window, cx)))
            .on_action(cx.listener(|this, _: &NextEditor, window, cx| this.cycle_editor(true, window, cx)))
            .on_action(cx.listener(|this, _: &PreviousEditor, window, cx| this.cycle_editor(false, window, cx)))
            .on_action(cx.listener(|this, _: &KeepEditor, _, cx| this.keep_editor(cx)))
            .on_action(cx.listener(|this, _: &CopyActivePath, _, cx| this.copy_active_path(false, cx)))
            .on_action(cx.listener(|this, _: &CopyActiveRelativePath, _, cx| this.copy_active_path(true, cx)))
            .on_action(cx.listener(|this, _: &CompareActiveWith, window, cx| {
                if this.active_editor().is_none() {
                    return;
                }
                this.open_palette("", window, cx);
                if let Some(Overlay { kind: OverlayKind::Palette { compare, .. }, .. }) = &mut this.overlay {
                    *compare = true;
                }
            }))
            .on_action(cx.listener(|this, _: &NewFolder, window, cx| this.new_folder(window, cx)))
            .on_action(cx.listener(|this, _: &RefreshExplorer, _, cx| this.refresh_explorer(cx)))
            .on_action(cx.listener(|this, _: &OpenWorkspaceSettings, window, cx| this.open_workspace_settings(window, cx)))
            .on_action(cx.listener(|this, _: &SwitchProject, window, cx| this.show_project_switcher(window, cx)))
            .on_action(cx.listener(|this, _: &SaveProject, window, cx| this.start_save_project(window, cx)))
            .on_action(cx.listener(|this, _: &EditProjects, window, cx| this.edit_projects(window, cx)))
            .on_action(cx.listener(|this, _: &ForgetRecent, window, cx| this.show_forget_recent(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleFavorite, _, cx| this.toggle_active_favorite(cx)))
            .on_action(cx.listener(|this, _: &ShowFavorites, window, cx| this.show_favorites(window, cx)))
            .on_action(cx.listener(|this, _: &ListBookmarks, window, cx| this.show_bookmarks(window, cx)))
            .on_action(cx.listener(|this, _: &RunTask, _, cx| this.show_tasks(cx)))
            .on_action(cx.listener(|this, _: &RunBuildTask, _, cx| this.run_build_task(cx)))
            .on_action(cx.listener(|this, _: &RerunLastTask, window, cx| this.rerun_last_task(window, cx)))
            .on_action(cx.listener(Self::start_debugging))
            .on_action(cx.listener(Self::stop_debugging))
            .on_action(cx.listener(Self::restart_debugging))
            .on_action(cx.listener(Self::remove_all_breakpoints))
            .on_action(cx.listener(Self::edit_breakpoint))
            .on_action(cx.listener(Self::break_on_exceptions))
            .on_action(cx.listener(Self::add_data_breakpoint))
            .on_action(cx.listener(Self::remove_data_breakpoints))
            .on_action(cx.listener(Self::add_watch))
            .on_action(cx.listener(Self::remove_all_watches))
            .on_action(cx.listener(Self::pick_workspace_file))
            .on_action(cx.listener(Self::add_folder_in_new_window))
            .on_action(cx.listener(Self::add_folder_to_workspace))
            .on_action(cx.listener(Self::show_remove_folder))
            .on_action(cx.listener(Self::install_js_debug))
            .on_action(cx.listener(|this, _: &DebugContinue, _, cx| this.debug_control("continue", cx)))
            .on_action(cx.listener(|this, _: &DebugPause, _, cx| this.debug_control("pause", cx)))
            .on_action(cx.listener(|this, _: &DebugStepOver, _, cx| this.debug_control("next", cx)))
            .on_action(cx.listener(|this, _: &DebugStepInto, _, cx| this.debug_control("stepIn", cx)))
            .on_action(cx.listener(|this, _: &DebugStepOut, _, cx| this.debug_control("stepOut", cx)))
            .on_action(cx.listener(|this, _: &ShowDebugPanel, _, cx| {
                this.chrome.panel_open = true;
                this.chrome.bottom = Bottom::Debug;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &RunTestAtCursor, _, cx| this.run_test(false, cx)))
            .on_action(cx.listener(|this, _: &RunFileTests, _, cx| this.run_test(true, cx)))
            .on_action(cx.listener(|this, _: &ConfigureTasks, window, cx| this.configure_tasks(window, cx)))
            .on_action(cx.listener(|this, _: &TogglePanelPosition, _, cx| {
                this.chrome.panel_right = !this.chrome.panel_right;
                this.chrome.panel_open = true;
                if let Some(s) = &this.persist.state {
                    let _ = s.set("panel_right", if this.chrome.panel_right { "1" } else { "0" });
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleMaximizedPanel, _, cx| {
                this.chrome.panel_max = !this.chrome.panel_max;
                this.chrome.panel_open = true;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &CloseFolder, _, cx| this.close_folder(cx)))
            .on_action(cx.listener(|this, _: &ClearTerminal, _, cx| {
                if let Some(t) = this.terminals.get(this.active_terminal).cloned() {
                    t.update(cx, |t, cx| t.clear_all(cx));
                }
            }))
            .on_action(cx.listener(|this, _: &ChangeEol, window, cx| this.show_eol_picker(window, cx)))
            .on_action(cx.listener(|this, _: &OpenWith, window, cx| this.show_open_with(window, cx)))
            .on_action(cx.listener(|this, _: &ChangeLanguage, window, cx| this.show_language_picker(window, cx)))
            .on_action(cx.listener(|this, _: &ChangeIndentation, window, cx| this.show_indent_picker(window, cx)))
            .on_action(cx.listener(|this, _: &GitCheckout, window, cx| this.show_branch_picker(window, cx)))
            .on_action(cx.listener(|this, _: &GitCreateBranch, window, cx| this.create_branch(window, cx)))
            .on_action(cx.listener(|this, _: &GitMergeBranch, window, cx| this.show_branch_op_picker(git_glue::BranchOp::Merge, window, cx)))
            .on_action(cx.listener(|this, _: &RemoteDocker, window, cx| this.show_remote_targets(false, window, cx)))
            .on_action(cx.listener(|this, _: &RemoteWsl, window, cx| this.show_remote_targets(true, window, cx)))
            .on_action(cx.listener(|this, _: &GitRebaseBranch, window, cx| this.show_branch_op_picker(git_glue::BranchOp::Rebase, window, cx)))
            .on_action(cx.listener(|this, _: &GitOpenWorktree, window, cx| this.show_branch_op_picker(git_glue::BranchOp::OpenWorktree, window, cx)))
            .on_action(cx.listener(|this, _: &GitContinueRebase, _, cx| this.git_continue_rebase(cx)))
            .on_action(cx.listener(|this, _: &GitAbortRebase, window, cx| this.git_abort_rebase(window, cx)))
            .on_action(cx.listener(|this, _: &GitAddWorktree, window, cx| this.git_add_worktree(window, cx)))
            .on_action(cx.listener(|this, _: &GitDeleteBranch, window, cx| this.show_branch_op_picker(git_glue::BranchOp::Delete, window, cx)))
            .on_action(cx.listener(|this, _: &GitRenameBranch, window, cx| this.show_branch_op_picker(git_glue::BranchOp::Rename, window, cx)))
            .on_action(cx.listener(|this, _: &GitRemoveRemote, window, cx| this.show_branch_op_picker(git_glue::BranchOp::RemoveRemote, window, cx)))
            .on_action(cx.listener(|this, _: &GitPublishBranch, _, cx| this.git_publish_branch(cx)))
            .on_action(cx.listener(|this, _: &GitAbortMerge, window, cx| this.git_abort_merge(window, cx)))
            .on_action(cx.listener(|this, _: &GitAddRemote, window, cx| this.git_add_remote(window, cx)))
            .on_action(cx.listener(|this, _: &GitCreateTag, window, cx| this.git_create_tag(window, cx)))
            .on_action(cx.listener(|this, _: &GitPull, _, cx| this.git_sync(vec![crate::git::SyncOp::Pull], cx)))
            .on_action(cx.listener(|this, _: &GitPush, _, cx| this.git_sync(vec![crate::git::SyncOp::Push], cx)))
            .on_action(cx.listener(|this, _: &GitFetch, _, cx| this.git_sync(vec![crate::git::SyncOp::Fetch], cx)))
            .on_action(cx.listener(|this, _: &GitSync, window, cx| this.git_sync_auto(window, cx)))
            // --- git-sync ---
            .on_action(cx.listener(|this, _: &GitPullRebase, _, cx| this.git_sync(vec![crate::git::SyncOp::PullRebase], cx)))
            .on_action(cx.listener(|this, _: &GitFetchAll, _, cx| this.git_sync(vec![crate::git::SyncOp::FetchAll], cx)))
            .on_action(cx.listener(|this, _: &GitPullFrom, window, cx| this.show_sync_target_picker(true, window, cx)))
            .on_action(cx.listener(|this, _: &GitPushTo, window, cx| this.show_sync_target_picker(false, window, cx)))
            .on_action(cx.listener(|this, _: &GitCommitSignedOff, _, cx| this.with_scm(cx, |s, cx| s.commit_signed_off(cx))))
            .on_action(cx.listener(|this, _: &GitCommitNoVerify, _, cx| this.with_scm(cx, |s, cx| s.commit_no_verify(cx))))
            .on_action(cx.listener(|this, _: &GitCommitAndPush, _, cx| this.with_scm(cx, |s, cx| s.commit_and(crate::git::PostCommit::Push, cx))))
            .on_action(cx.listener(|this, _: &GitCommitAndSync, _, cx| this.with_scm(cx, |s, cx| s.commit_and(crate::git::PostCommit::Sync, cx))))
            .on_action(cx.listener(|this, _: &GitFileHistory, window, cx| this.show_file_history(window, cx)))
            .on_action(cx.listener(|this, _: &GitBlameLine, _, cx| this.blame_line(cx)))
            .on_action(cx.listener(|this, _: &ShowSettings, window, cx| this.show_settings_picker(window, cx)))
            .on_action(cx.listener(|this, _: &ShowKeyboardShortcuts, window, cx| this.show_shortcuts_picker(window, cx)))
            .on_action(cx.listener(|this, _: &RestartLanguageServers, _, cx| {
                this.restart_lsp(cx);
                this.notice = Some(Notice::Info(crate::i18n::t("Restarting language servers…").into()));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &GitCommitAmend, _, cx| this.with_scm(cx, |s, cx| s.commit_amend(cx))))
            .on_action(cx.listener(|this, _: &GitUndoLastCommit, _, cx| this.with_scm(cx, |s, cx| s.undo_last_commit(cx))))
            .on_action(cx.listener(|this, _: &GitStash, _, cx| this.with_scm(cx, |s, cx| s.stash(false, cx))))
            .on_action(cx.listener(|this, _: &GitStashIncludeUntracked, _, cx| this.with_scm(cx, |s, cx| s.stash(true, cx))))
            .on_action(cx.listener(|this, _: &GitPopLatestStash, _, cx| this.with_scm(cx, |s, cx| s.stash_apply("stash@{0}".into(), true, cx))))
            .on_action(cx.listener(|this, _: &GitPopStash, window, cx| this.show_stash_picker(git_glue::StashOp::Pop, window, cx)))
            .on_action(cx.listener(|this, _: &GitApplyStash, window, cx| this.show_stash_picker(git_glue::StashOp::Apply, window, cx)))
            .on_action(cx.listener(|this, _: &GitDropStash, window, cx| this.show_stash_picker(git_glue::StashOp::Drop, window, cx)))
            // --- git-refs ---
            .on_action(cx.listener(|this, _: &GitDeleteTag, window, cx| this.show_branch_op_picker(git_glue::BranchOp::DeleteTag, window, cx)))
            .on_action(cx.listener(|this, _: &GitDeleteRemoteTag, window, cx| this.show_branch_op_picker(git_glue::BranchOp::DeleteRemoteTag, window, cx)))
            .on_action(cx.listener(|this, _: &GitPushTags, window, cx| this.git_push_tags(window, cx)))
            .on_action(cx.listener(|this, _: &GitPushTag, window, cx| this.show_branch_op_picker(git_glue::BranchOp::PushTag, window, cx)))
            .on_action(cx.listener(|this, _: &GitCheckoutDetached, window, cx| this.show_branch_op_picker(git_glue::BranchOp::CheckoutDetached, window, cx)))
            .on_action(cx.listener(|this, _: &GitCreateBranchFrom, window, cx| this.show_branch_op_picker(git_glue::BranchOp::CreateFrom, window, cx)))
            .on_action(
                cx.listener(|this, _: &GitDeleteRemoteBranch, window, cx| this.show_branch_op_picker(git_glue::BranchOp::DeleteRemoteBranch, window, cx)),
            )
            .on_action(cx.listener(|this, _: &GitSetUpstream, window, cx| this.show_branch_op_picker(git_glue::BranchOp::SetUpstream, window, cx)))
            .on_action(cx.listener(|this, _: &GitRenameRemote, window, cx| this.show_branch_op_picker(git_glue::BranchOp::RenameRemote, window, cx)))
            .on_action(cx.listener(|this, _: &GitSetRemoteUrl, window, cx| this.show_branch_op_picker(git_glue::BranchOp::SetRemoteUrl, window, cx)))
            .on_action(cx.listener(|this, _: &GitRemoveWorktree, window, cx| this.show_branch_op_picker(git_glue::BranchOp::RemoveWorktree, window, cx)))
            .on_action(cx.listener(|this, _: &GitPruneWorktrees, _, cx| this.git_prune_worktrees(cx)))
            .on_action(cx.listener(|this, _: &GitStashStaged, _, cx| this.git_stash_with(crate::git::StashOptions { staged: true, ..Default::default() }, cx)))
            .on_action(cx.listener(|this, _: &GitStashNamed, window, cx| this.git_stash_named(window, cx)))
            .on_action(cx.listener(|this, _: &GitShowStash, window, cx| this.show_stash_picker(git_glue::StashOp::Show, window, cx)))
            .on_action(cx.listener(|this, _: &GitOpenChanges, window, cx| this.git_open_active(false, window, cx)))
            .on_action(cx.listener(|this, _: &GitOpenFileHead, window, cx| this.git_open_active(true, window, cx)))
            .on_action(cx.listener(|this, _: &GitCompareBranch, window, cx| this.show_branch_op_picker(git_glue::BranchOp::CompareBranch, window, cx)))
            .on_action(cx.listener(|this, _: &GitCompareCommit, window, cx| this.show_compare_commit_picker(window, cx)))
            .on_action(cx.listener(|this, _: &GitAddToGitignore, _, cx| this.git_ignore_active(cx)))
            .on_action(cx.listener(|this, _: &GitDiscardAll, window, cx| {
                if this.repo_root(cx).is_some() {
                    this.chrome.panel = Some(Panel::Scm);
                    this.scm.update(cx, |s, cx| s.confirm_discard_all(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &RevealActiveFile, window, cx| {
                if let Some(path) = this.active_editor().map(|e| e.read(cx).path.clone()) {
                    this.chrome.panel = Some(Panel::Explorer);
                    this.roots.primary.update(cx, |t, cx| t.reveal(&path, cx));
                    window.focus(&this.roots.primary.focus_handle(cx));
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &CollapseFolders, _, cx| this.roots.primary.update(cx, |t, cx| t.collapse_all(cx))))
            .on_action(cx.listener(Self::show_output))
            .on_action(cx.listener(|this, _: &ClearOutput, _, cx| {
                this.lsp.output.clear();
                cx.notify();
            }))
            .on_action(cx.listener(Self::split_terminal))
            .on_action(cx.listener(Self::go_forward))
            .on_action(cx.listener(Self::reopen_closed_editor))
            .on_action(cx.listener(Self::new_file))
            .on_action(cx.listener(Self::save_as))
            .on_action(cx.listener(Self::save_local_copy))
            .on_action(cx.listener(|this, _: &CloseSavedEditors, window, cx| {
                let clean: Vec<usize> = (0..this.tabs.editors.len()).filter(|&i| !this.tabs.editors[i].read(cx).is_dirty()).collect();
                this.close_tabs(clean, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ConvertIndentToSpaces, _, cx| {
                if let Some(ed) = this.active_editor().cloned() {
                    ed.update(cx, |e, cx| e.convert_indentation(false, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &ConvertIndentToTabs, _, cx| {
                if let Some(ed) = this.active_editor().cloned() {
                    ed.update(cx, |e, cx| e.convert_indentation(true, cx));
                }
            }))
            .on_action(cx.listener(|_, _: &ToggleMinimap, _, cx| {
                let s = cx.global_mut::<Settings>();
                s.minimap = !s.minimap;
                cx.refresh_windows();
            }))
            .on_action(cx.listener(|_, _: &ToggleStickyScroll, _, cx| {
                let s = cx.global_mut::<Settings>();
                s.sticky_scroll = !s.sticky_scroll;
                cx.refresh_windows();
            }))
            .on_action(cx.listener(|_, _: &ToggleRenderWhitespace, _, cx| {
                let s = cx.global_mut::<Settings>();
                s.render_whitespace = if s.render_whitespace == "none" { "all".into() } else { "none".into() };
                cx.refresh_windows();
            }))
            .on_action(cx.listener(|_, _: &ToggleWordWrap, _, cx| {
                let s = cx.global_mut::<Settings>();
                s.word_wrap = !s.word_wrap;
                cx.refresh_windows();
            }))
            .on_action(cx.listener(Self::move_editor_to_other_group))
            .on_action(cx.listener(Self::focus_left_group))
            .on_action(cx.listener(Self::focus_right_group))
            .on_action(cx.listener(Self::close_editor))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::select_theme))
            .on_action(cx.listener(Self::select_icon_theme))
            .on_action(cx.listener(Self::show_welcome))
            .on_action(cx.listener(Self::show_about))
            .on_action(cx.listener(Self::check_for_updates))
            .on_action(cx.listener(Self::show_disk_access))
            .on_action(cx.listener(Self::follow_file))
            .on_action(cx.listener(Self::add_ssh_host))
            .on_action(cx.listener(Self::remove_ssh_host))
            .on_action(cx.listener(Self::test_ssh_host))
            .on_action(cx.listener(Self::edit_ssh_config))
            .on_action(cx.listener(Self::edit_ssh_host))
            .on_action(cx.listener(|this, _: &ManageSshHosts, window, cx| this.show_host_menu(window, cx)))
            .on_action(cx.listener(Self::setup_key_login))
            .on_action(cx.listener(Self::upload_here))
            .on_action(cx.listener(Self::download_active))
            .on_action(cx.listener(Self::clear_schema_cache))
            .on_action(cx.listener(Self::spell_fix_action))
            .on_action(cx.listener(Self::install_dictionary))
            .on_action(cx.listener(Self::toggle_spelling))
            .on_action(cx.listener(Self::open_merge_editor))
            .on_action(cx.listener(Self::show_pull_requests))
            .on_action(cx.listener(Self::send_request))
            .on_action(cx.listener(Self::show_response_headers))
            .on_action(cx.listener(Self::filter_log))
            .on_action(cx.listener(Self::start_save_theme))
            .on_action(cx.listener(Self::open_themes_folder))
            .on_action(cx.listener(Self::minify_json))
            .on_action(cx.listener(Self::show_shortcuts_page))
            .on_action(cx.listener(Self::insert_snippet))
            .on_action(cx.listener(Self::configure_snippets))
            .on_action(cx.listener(Self::open_markdown_preview))
            .on_action(cx.listener(Self::compare_with_clipboard))
            .on_action(cx.listener(Self::compare_with_saved))
            // --- compare ---
            .on_action(cx.listener(Self::compare_folders))
            .on_action(cx.listener(Self::run_selected_text))
            .on_action(cx.listener(Self::go_to_last_edit))
            .on_action(cx.listener(Self::new_scratch_file))
            // --- data ---
            .on_action(cx.listener(Self::data_convert))
            .on_action(cx.listener(Self::data_query))
            .on_action(cx.listener(Self::data_copy_path))
            .on_action(cx.listener(Self::data_decode))
            .on_action(cx.listener(Self::clone_repository))
            .on_action(cx.listener(Self::open_settings_page))
            .on_action(cx.listener(Self::quick_open))
            .on_action(cx.listener(Self::command_palette))
            .on_action(cx.listener(Self::go_to_line))
            .on_action(cx.listener(|this, _: &OpenAsHex, window, cx| this.open_as_hex(window, cx)))
            .on_action(cx.listener(Self::find_in_files))
            // --- structural ---
            .on_action(cx.listener(Self::structural_search))
            .on_action(cx.listener(Self::structural_replace))
            .on_action(cx.listener(|this, _: &OpenSearchInEditor, _, cx| {
                let search = this.search.clone();
                search.update(cx, |s, cx| s.open_in_editor(cx));
            }))
            .on_action(cx.listener(|this, _: &crate::search::NextResult, _, cx| this.search.update(cx, |s, cx| s.step_result(1, cx))))
            .on_action(cx.listener(|this, _: &crate::search::PrevResult, _, cx| this.search.update(cx, |s, cx| s.step_result(-1, cx))))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::open_keybindings))
            .on_action(cx.listener(Self::import_vscode))
            .on_action(cx.listener(Self::toggle_panel))
            .on_action(cx.listener(Self::toggle_terminal))
            .on_action(cx.listener(Self::new_terminal))
            .on_action(cx.listener(Self::kill_terminal))
            .size_full()
            .flex()
            .flex_col()
            .bg(c(p.surface))
            .font_family(cx.global::<Settings>().ui_font.clone().map_or(SharedString::from(ui::UI_FONT), SharedString::from))
            .text_size(px(text::SM.0))
            .line_height(px(text::SM.1))
            .text_color(c(p.text))
            // Sidebar resizing: the drag is tracked everywhere in the window.
            .on_mouse_move(cx.listener(|this, ev: &gpui::MouseMoveEvent, window, cx| {
                // A release outside the window may not have delivered a mouse-up event.
                if ev.pressed_button != Some(MouseButton::Left) {
                    this.finish_resize(cx);
                    return;
                }
                if this.sections.dragging() && this.sections.drag_to(ev.position.y / px(1.), window.viewport_size().height / px(1.) - 220.) {
                    cx.notify();
                }
                if this.chrome.resizing_sidebar {
                    let x = ev.position.x / px(1.) - layout::ACTIVITYBAR_W;
                    this.chrome.sidebar_w = x.clamp(layout::SIDEBAR_MIN, layout::SIDEBAR_MAX);
                    cx.notify();
                }
                if this.chrome.resizing_split {
                    // Editor area: everything after the activity bar + (if open) the sidebar.
                    let left = layout::ACTIVITYBAR_W + if this.chrome.panel.is_some() { this.chrome.sidebar_w } else { 0. };
                    let width = (window.viewport_size().width / px(1.) - left).max(1.);
                    let x = ev.position.x / px(1.) - left;
                    this.chrome.split_ratio = (x / width).clamp(0.15, 0.85);
                    cx.notify();
                }
                if this.chrome.resizing_panel && this.chrome.panel_right {
                    let total = window.viewport_size().width / px(1.);
                    let w = total - ev.position.x / px(1.);
                    this.chrome.panel_w = w.clamp(PANEL_MIN * 2., (total - 300.).max(PANEL_MIN * 2.));
                    cx.notify();
                } else if this.chrome.resizing_panel {
                    let total = window.viewport_size().height / px(1.);
                    let h = total - ev.position.y / px(1.) - layout::STATUSBAR_H;
                    this.chrome.panel_h = h.clamp(PANEL_MIN, (total - 200.).max(PANEL_MIN));
                    cx.notify();
                }
            }))
            // Finish before editor/terminal handlers can consume the release.
            .capture_any_mouse_up(cx.listener(|this, ev: &gpui::MouseUpEvent, _, cx| {
                if ev.button == MouseButton::Left {
                    this.finish_resize(cx);
                }
            }))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, cx| this.finish_resize(cx)))
            .on_action(cx.listener(|this, _: &crate::menus::FileMenu, w, cx| this.open_application_menu_by_index(0, w, cx)))
            .on_action(cx.listener(|this, _: &crate::menus::EditMenu, w, cx| this.open_application_menu_by_index(1, w, cx)))
            .on_action(cx.listener(|this, _: &crate::menus::SelectionMenu, w, cx| this.open_application_menu_by_index(2, w, cx)))
            .on_action(cx.listener(|this, _: &crate::menus::ViewMenu, w, cx| this.open_application_menu_by_index(3, w, cx)))
            .on_action(cx.listener(|this, _: &crate::menus::GoMenu, w, cx| this.open_application_menu_by_index(4, w, cx)))
            .on_action(cx.listener(|this, _: &crate::menus::TerminalMenu, w, cx| this.open_application_menu_by_index(5, w, cx)))
            .on_action(cx.listener(|this, _: &crate::menus::WindowMenu, w, cx| this.open_application_menu_by_index(6, w, cx)))
            .on_action(cx.listener(|this, _: &crate::menus::HelpMenu, w, cx| this.open_application_menu_by_index(7, w, cx)))
            .when(cfg!(target_os = "macos"), |d| d.child(self.title_bar(&p, cx)))
            .when(!cfg!(target_os = "macos") && self.chrome.zen.is_none(), |d| d.child(self.application_menu_bar(&p, cx)))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .when(self.chrome.zen.is_none() && cx.global::<Settings>().activity_bar, |d| d.child(self.activity_bar(&p, cx)))
                    .when_some(sidebar, |d, s| {
                        let resizing = self.chrome.resizing_sidebar;
                        d.child(
                            div().relative().w(px(self.chrome.sidebar_w)).flex_none().bg(c(p.surface)).border_r_1().border_color(c(p.border)).child(s).child(
                                div()
                                    .id("sidebar-sash")
                                    .absolute()
                                    // Keep the explorer row underneath from receiving the mouse press (drag and drop).
                                    .occlude()
                                    .top_0()
                                    .bottom_0()
                                    .right(px(-layout::SASH / 2.))
                                    .w(px(layout::SASH))
                                    .cursor(gpui::CursorStyle::ResizeLeftRight)
                                    .when(resizing, |d| d.bg(c(p.accent)))
                                    .hover(|d| d.bg(c(p.accent)))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.chrome.resizing_sidebar = true;
                                            cx.notify();
                                        }),
                                    ),
                            ),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .when(!self.chrome.panel_right, |d| d.flex_col())
                            // A maximized panel takes the editor's place.
                            .when(!(self.chrome.panel_open && self.chrome.panel_max), |d| {
                                d.child(div().flex_1().min_w_0().min_h_0().flex().flex_col().child(self.editor_area(main, &p, cx)))
                            })
                            .when(self.chrome.panel_open, |d| d.child(self.panel(&p, cx))),
                    ),
            )
            .when(self.chrome.zen.is_none() && cx.global::<Settings>().status_bar, |d| d.child(self.status_bar(&p, cx)))
            .children(self.error_toast(&p, cx))
            .children(self.update_dialog(&p, cx))
            .when_some(
                self.menu.as_ref().map(|m| (m.menu.clone(), m.position, m.app_path.is_some(), m.menu.read(cx).anchor())),
                |d, (menu, position, application, anchor)| {
                    d.child(deferred(
                        div()
                            .id("menu-backdrop")
                            .absolute()
                            .top(px(if application { app_menu_glue::MENU_BAR_H } else { 0. }))
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| {
                                    this.close_menu(window, cx);
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(|this, _, window, cx| {
                                    this.close_menu(window, cx);
                                }),
                            )
                            .child(gpui::anchored().position(position).anchor(anchor).snap_to_window().child(menu)),
                    ))
                },
            )
            .when_some(self.overlay.as_ref().map(|ov| ov.picker.clone()), |d, picker| {
                d.child(deferred(
                    div()
                        .id("picker-backdrop")
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .flex()
                        .justify_center()
                        // On macOS we draw the title bar ourselves: the palette sits below it.
                        .pt(px(if cfg!(target_os = "macos") { layout::TITLEBAR_H + 4. } else { theme::SPACE[4] }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                // Click outside = Esc.
                                this.on_overlay_event(&PickerEvent::Dismiss, window, cx);
                            }),
                        )
                        .child(div().occlude().child(picker)),
                ))
            })
    }
}

#[cfg(test)]
mod resize_tests {
    use super::*;
    use gpui::{Modifiers, TestAppContext, point};

    #[cfg(windows)]
    #[gpui::test]
    fn windows_workspace_paths_reuse_tabs_with_both_separators(cx: &mut TestAppContext) {
        use crate::qa_workbench_tests::{open, open_in, project, setup};
        let dir = project("windows-şablon boşluk");
        let path = dir.join("src/main.rs");
        let (wb, cx) = setup(cx);
        open(&wb, &dir, cx);
        open_in(&wb, &path, cx);
        let root = wb.read_with(cx, |wb, cx| wb.root(cx).unwrap());
        assert!(!root.contains('\\'), "{root}");
        assert!(!root.starts_with("//?/"), "{root}");
        let normalized = ui::local_path(&path);
        wb.update_in(cx, |wb, window, cx| wb.open_file(normalized.clone(), window, cx));
        cx.run_until_parked();
        let paths = wb.read_with(cx, |wb, cx| wb.open_paths(cx));
        assert_eq!(paths, vec![normalized]);
    }

    // Exercise the real event dispatch, including releases outside the root hitbox and
    // missed mouse-up events when a drag leaves the window.
    #[gpui::test]
    fn resize_stops_on_release_or_unpressed_move(cx: &mut TestAppContext) {
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        for release in [Some(point(px(500.), px(200.))), Some(point(px(-10.), px(-10.))), None] {
            wb.update(cx, |wb, cx| {
                wb.chrome.resizing_sidebar = true;
                wb.chrome.resizing_panel = true;
                wb.chrome.resizing_split = true;
                cx.notify();
            });
            cx.run_until_parked();
            let sizes = wb.read_with(cx, |wb, _| (wb.chrome.sidebar_w, wb.chrome.panel_h, wb.chrome.panel_w, wb.chrome.split_ratio));
            if let Some(position) = release {
                // An unrelated button must not end a left-button drag.
                cx.simulate_mouse_up(position, MouseButton::Right, Modifiers::none());
                assert!(wb.read_with(cx, |wb, _| wb.chrome.resizing_sidebar && wb.chrome.resizing_panel && wb.chrome.resizing_split));
                cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::none());
                assert!(wb.read_with(cx, |wb, _| !wb.chrome.resizing_sidebar && !wb.chrome.resizing_panel && !wb.chrome.resizing_split));
            }
            cx.simulate_mouse_move(point(px(600.), px(300.)), None, Modifiers::none());
            assert!(wb.read_with(cx, |wb, _| !wb.chrome.resizing_sidebar && !wb.chrome.resizing_panel && !wb.chrome.resizing_split));
            assert_eq!(wb.read_with(cx, |wb, _| (wb.chrome.sidebar_w, wb.chrome.panel_h, wb.chrome.panel_w, wb.chrome.split_ratio)), sizes);
        }
    }
}
