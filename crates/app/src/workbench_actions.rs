//! Workbench: the window's actions. The main window's root and an editor's own window (workbench_editor_windows.rs)
//! both register them, so a command or shortcut works the same in either; in an editor's own window the commands act
//! on that editor (see `Workbench::active_editor`).

use super::*;

impl Workbench {
    /// `d` with a listener for every workbench action.
    pub(super) fn with_actions(&self, d: gpui::Stateful<gpui::Div>, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        d.on_action(cx.listener(Self::open_folder))
            .on_action(cx.listener(Self::cancel_file_operation))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::trust_this_folder))
            .on_action(cx.listener(Self::show_explorer))
            .on_action(cx.listener(Self::switch_editor))
            .on_action(cx.listener(Self::toggle_zen))
            .on_action(cx.listener(Self::install_cyx))
            .on_action(cx.listener(Self::new_extension))
            .on_action(cx.listener(Self::toggle_full_screen))
            .on_action(cx.listener(Self::find_todos))
            .on_action(cx.listener(Self::show_remote))
            .on_action(cx.listener(Self::show_remote_explorer))
            .on_action(cx.listener(Self::show_run_debug))
            .on_action(cx.listener(Self::show_testing))
            .on_action(cx.listener(Self::show_extensions))
            .on_action(cx.listener(Self::show_extension_inspector))
            .on_action(cx.listener(Self::show_market))
            .on_action(cx.listener(Self::install_extension_from_file))
            .on_action(cx.listener(Self::install_extension_from_registry))
            .on_action(cx.listener(Self::install_extension_from_folder))
            .on_action(cx.listener(|this, _: &RunAllTests, w, cx| this.run_all_tests(w, cx)))
            .on_action(cx.listener(|this, _: &RunFailedTests, w, cx| this.run_failed_tests(w, cx)))
            .on_action(cx.listener(|this, _: &RefreshTests, _, cx| this.refresh_tests(cx)))
            .on_action(cx.listener(|this, _: &StopTests, _, cx| this.stop_tests(cx)))
            .on_action(cx.listener(|this, _: &DebugTestAtCursor, w, cx| this.debug_test_at_cursor(w, cx)))
            .on_action(cx.listener(|this, _: &MoveViewToSecondarySideBar, _, cx| this.move_view_to_secondary(cx)))
            .on_action(cx.listener(|this, _: &MoveViewToPrimarySideBar, _, cx| this.move_view_to_primary(cx)))
            .on_action(cx.listener(|this, _: &ToggleSecondarySideBar, _, cx| this.toggle_secondary(cx)))
            .on_action(cx.listener(|this, _: &MoveTerminalIntoNewWindow, window, cx| this.detach_terminal(window, cx)))
            .on_action(cx.listener(|this, _: &MoveEditorIntoNewWindow, window, cx| this.editor_into_new_window(false, window, cx)))
            .on_action(cx.listener(|this, _: &CopyEditorIntoNewWindow, window, cx| this.editor_into_new_window(true, window, cx)))
            .on_action(cx.listener(|this, _: &MoveEditorsBackToMainWindow, window, cx| this.return_editor_windows(window, cx)))
            .on_action(cx.listener(|this, _: &MoveTerminalsBackToPanel, _, cx| this.return_detached_terminals(cx)))
            .on_action(cx.listener(|this, _: &MoveTerminalIntoEditorArea, window, cx| this.move_terminal_into_editor(window, cx)))
            .on_action(cx.listener(|this, _: &MoveTerminalBackToPanelFromEditor, window, cx| this.move_terminal_back_to_panel(window, cx)))
            .on_action(cx.listener(|this, _: &ClearSavedTerminalHistory, _, cx| this.clear_saved_terminal_history(cx)))
            .on_action(cx.listener(Self::kill_all_terminals))
            .on_action(cx.listener(Self::kill_other_terminals))
            .on_action(cx.listener(Self::focus_next_terminal))
            .on_action(cx.listener(Self::focus_previous_terminal))
            .on_action(cx.listener(Self::configure_terminal_settings))
            .on_action(cx.listener(Self::rename_terminal))
            .on_action(cx.listener(Self::change_terminal_color))
            .on_action(cx.listener(Self::new_terminal_with_profile))
            .on_action(cx.listener(Self::select_default_terminal_profile))
            .on_action(cx.listener(Self::run_without_debugging))
            .on_action(cx.listener(Self::run_to_cursor))
            .on_action(cx.listener(Self::add_debug_configuration))
            .on_action(cx.listener(Self::stop_all_debugging))
            .on_action(cx.listener(|this, _: &OpenLaunchConfiguration, w, cx| this.open_launch_file(w, cx)))
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
            .on_action(cx.listener(Self::on_run_extension_command))
            .on_action(cx.listener(Self::format_document))
            .on_action(cx.listener(Self::format_selection))
            .on_action(cx.listener(Self::save_all))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::split_editor))
            .on_action(cx.listener(Self::split_editor_left))
            .on_action(cx.listener(Self::split_editor_up))
            .on_action(cx.listener(Self::split_editor_down))
            .on_action(cx.listener(Self::close_empty_editor_group))
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
            .on_action(cx.listener(|this, _: &TerminateTask, _, cx| this.terminate_tasks(cx)))
            .on_action(cx.listener(|this, _: &RestartRunningTask, window, cx| this.restart_task(window, cx)))
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
            .on_action(cx.listener(Self::pick_save_workspace_as))
            .on_action(cx.listener(|this, _: &SaveWorkspace, _, cx| this.save_workspace_now(cx)))
            .on_action(cx.listener(|this, _: &ReloadWorkspace, _, cx| this.reload_workspace(false, cx)))
            .on_action(cx.listener(|this, _: &OpenActiveWorkspaceFile, window, cx| this.open_active_workspace_file(window, cx)))
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
            .on_action(cx.listener(|this, _: &TogglePanelPosition, _, cx| this.toggle_panel_position(cx)))
            .on_action(cx.listener(|this, _: &MovePanelLeft, _, cx| this.dock_panel(docks_glue::PanelDock::Left, cx)))
            .on_action(cx.listener(|this, _: &MovePanelRight, _, cx| this.dock_panel(docks_glue::PanelDock::Right, cx)))
            .on_action(cx.listener(|this, _: &MovePanelToBottom, _, cx| this.dock_panel(docks_glue::PanelDock::Bottom, cx)))
            .on_action(cx.listener(|this, _: &ToggleMaximizedPanel, _, cx| this.toggle_maximized_panel(cx)))
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
            .on_action(cx.listener(|this, _: &DockerOpenProject, window, cx| this.docker_open_project(false, window, cx)))
            .on_action(cx.listener(|this, _: &DockerRebuild, window, cx| this.docker_open_project(true, window, cx)))
            .on_action(cx.listener(|this, _: &DockerServices, window, cx| this.docker_contexts(docker_glue::Action::Services, window, cx)))
            .on_action(cx.listener(|this, _: &DockerCancel, _, cx| this.cancel_docker(cx)))
            .on_action(cx.listener(|this, _: &ShowContainers, _, cx| this.show_containers(cx)))
            .on_action(cx.listener(|this, _: &OpenWebPreview, window, cx| this.open_web_preview(window, cx)))
            .on_action(cx.listener(|this, _: &CopyWebPreviewUrl, _, cx| this.copy_web_preview_url(cx)))
            .on_action(cx.listener(|this, _: &StopWebPreview, _, cx| this.stop_web_preview(cx)))
            .on_action(cx.listener(|this, _: &WebPreviewLog, _, cx| {
                this.chrome.panel_open = true;
                this.chrome.bottom = Bottom::Output;
                cx.notify();
            }))
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
            .on_action(cx.listener(|this, _: &CopyFullDiagnostic, _, cx| this.copy_full_diagnostic(cx)))
            .on_action(cx.listener(Self::rust_expand_macro))
            .on_action(cx.listener(Self::rust_parent_module))
            .on_action(cx.listener(Self::rust_open_cargo_toml))
            .on_action(cx.listener(|this, _: &RestartLanguageServers, _, cx| {
                this.restart_lsp(cx);
                this.notice = Some(Notice::Info(crate::i18n::t("Restarting language servers…").into()));
                cx.notify();
            }))
            .on_action(cx.listener(Self::manage_language_services))
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
            .on_action(cx.listener(Self::focus_up_group))
            .on_action(cx.listener(Self::focus_down_group))
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
            .on_action(cx.listener(|this, _: &OpenSearchInEditor, window, cx| this.open_search_editor_from_panel(window, cx)))
            .on_action(cx.listener(|this, _: &UndoSearchReplace, _, cx| this.undo_replace(cx)))
            .on_action(cx.listener(|this, _: &crate::search::NextResult, _, cx| this.search.update(cx, |s, cx| s.step_result(1, cx))))
            .on_action(cx.listener(|this, _: &crate::search::PrevResult, _, cx| this.search.update(cx, |s, cx| s.step_result(-1, cx))))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::open_keybindings))
            .on_action(cx.listener(Self::import_vscode))
            .on_action(cx.listener(Self::toggle_panel))
            .on_action(cx.listener(|this, _: &crate::resource_monitor::ToggleResources, window, cx| {
                this.chrome.resources.update(cx, |monitor, cx| monitor.toggle(window, cx));
            }))
            .on_action(cx.listener(Self::toggle_terminal))
            .on_action(cx.listener(Self::new_terminal))
            .on_action(cx.listener(Self::kill_terminal))
    }
}
