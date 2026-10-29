//! Commands in the command palette and shortcut display. Shortcuts are read from the keymap:
//! what the list shows never drifts from what actually runs.

use gpui::{Action, App, Keystroke};

pub enum Run {
    Action(Box<dyn Action>),
    Connect(String),
    Disconnect,
}

impl Clone for Run {
    fn clone(&self) -> Self {
        match self {
            Run::Action(a) => Run::Action(a.boxed_clone()),
            Run::Connect(h) => Run::Connect(h.clone()),
            Run::Disconnect => Run::Disconnect,
        }
    }
}

pub struct Command {
    pub label: String,
    pub run: Run,
}

impl Command {
    pub fn action(label: &str, action: impl Action) -> Self {
        Command { label: label.to_string(), run: Run::Action(Box::new(action)) }
    }

    /// Shortcut text ("⌘S", "⌘K ⌘T"); `None` if unbound.
    pub fn shortcut(&self, cx: &App) -> Option<String> {
        let Run::Action(action) = &self.run else { return None };
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        let binding = keymap.bindings_for_action(action.as_ref()).last()?;
        Some(binding.keystrokes().iter().map(|k| format_keystroke(k.inner())).collect::<Vec<_>>().join(" "))
    }
}

/// With macOS symbols (⌃⌥⇧⌘); on other platforms the "Ctrl+Shift+P" form.
pub fn format_keystroke(k: &Keystroke) -> String {
    let key = match k.key.as_str() {
        "enter" => "↵".to_string(),
        "escape" => "Esc".to_string(),
        "backspace" => "⌫".to_string(),
        "up" => "↑".to_string(),
        "down" => "↓".to_string(),
        "left" => "←".to_string(),
        "right" => "→".to_string(),
        k => k.to_uppercase(),
    };
    let m = &k.modifiers;
    if cfg!(target_os = "macos") {
        let mut s = String::new();
        if m.control {
            s.push('⌃');
        }
        if m.alt {
            s.push('⌥');
        }
        if m.shift {
            s.push('⇧');
        }
        if m.platform {
            s.push('⌘');
        }
        s + &key
    } else {
        let mut parts = Vec::new();
        if m.control {
            parts.push("Ctrl");
        }
        if m.alt {
            parts.push("Alt");
        }
        if m.shift {
            parts.push("Shift");
        }
        if m.platform {
            parts.push("Super");
        }
        parts.push(&key);
        parts.join("+")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_keystrokes() {
        let k = Keystroke::parse("cmd-shift-p").unwrap();
        if cfg!(target_os = "macos") {
            assert_eq!(format_keystroke(&k), "⇧⌘P");
            assert_eq!(format_keystroke(&Keystroke::parse("ctrl-g").unwrap()), "⌃G");
        } else {
            assert_eq!(format_keystroke(&k), "Shift+Super+P");
        }
    }
}

/// Command ids used in the shortcut file → action.
macro_rules! registry {
    ($($id:literal => $action:expr),* $(,)?) => {
        pub const COMMAND_IDS: &[&str] = &[$($id),*];

        pub fn action_by_id(id: &str) -> Option<Box<dyn Action>> {
            match id {
                $($id => Some(Box::new($action)),)*
                _ => None,
            }
        }
    };
}

use crate::{editor as ed, input as field, menu, picker, terminal as term, tree, workbench as wb};

registry! {
    "workbench.openFolder" => wb::OpenFolder,
    "workbench.switchEditor" => wb::SwitchEditor,
    "workbench.toggleZenMode" => wb::ToggleZenMode,
    "extensions.installCyx" => wb::InstallCyx,
    "extensions.newExtension" => wb::NewExtension,
    "workbench.toggleFullScreen" => wb::ToggleFullScreen,
    "search.findTodos" => wb::FindTodos,
    // --- structural ---
    "search.structuralSearch" => crate::search::StructuralSearch,
    "search.structuralReplace" => crate::search::StructuralReplace,
    "workbench.quickOpen" => wb::QuickOpen,
    "workbench.commandPalette" => wb::CommandPalette,
    "workbench.goToLine" => wb::GoToLine,
    "workbench.action.openAsHex" => wb::OpenAsHex,
    "explorer.openWith" => wb::OpenWith,
    "search.action.openInEditor" => wb::OpenSearchInEditor,
    "workbench.findInFiles" => wb::FindInFiles,
    "workbench.toggleSidebar" => wb::ToggleSidebar,
    "workspace.trust" => wb::TrustWorkspace,
    "workbench.showFiles" => wb::ShowExplorer,
    "remote.connectDevContainer" => wb::RemoteDocker,
    "docker.openProject" => wb::DockerOpenProject,
    "docker.rebuild" => wb::DockerRebuild,
    "docker.services" => wb::DockerServices,
    "docker.cancel" => wb::DockerCancel,
    "workbench.showContainers" => wb::ShowContainers,
    "remote.connectWsl" => wb::RemoteWsl,
    "workbench.showRemote" => wb::ShowRemote,
    "workbench.showSourceControl" => wb::ShowScm,
    "workbench.showRemoteExplorer" => wb::ShowRemoteExplorer,
    "workbench.showRunAndDebug" => wb::ShowRunAndDebug,
    "workbench.showTesting" => wb::ShowTesting,
    "workbench.showExtensions" => wb::ShowExtensions,
    "extensions.installFromFile" => wb::InstallExtensionFromFile,
    "extensions.installFromFolder" => wb::InstallExtensionFromFolder,
    "test.runAll" => wb::RunAllTests,
    "test.runFailed" => wb::RunFailedTests,
    "test.refresh" => wb::RefreshTests,
    "test.stop" => wb::StopTests,
    "test.debugAtCursor" => wb::DebugTestAtCursor,
    "terminal.newWithProfile" => wb::NewTerminalWithProfile,
    "terminal.selectDefaultProfile" => wb::SelectDefaultTerminalProfile,
    "terminal.rename" => wb::RenameTerminal,
    "terminal.changeColor" => wb::ChangeTerminalColor,
    "terminal.killAll" => wb::KillAllTerminals,
    "terminal.killOthers" => wb::KillOtherTerminals,
    "terminal.focusNext" => wb::FocusNextTerminal,
    "terminal.focusPrevious" => wb::FocusPreviousTerminal,
    "terminal.configureSettings" => wb::ConfigureTerminalSettings,
    "debug.runWithoutDebugging" => wb::RunWithoutDebugging,
    "debug.runToCursor" => wb::RunToCursor,
    "debug.addConfiguration" => wb::AddDebugConfiguration,
    "debug.openLaunchConfiguration" => wb::OpenLaunchConfiguration,
    "debug.stopAll" => wb::StopAllDebugging,
    "workbench.showProblems" => wb::ShowProblems,
    "problems.copyFullDiagnostic" => wb::CopyFullDiagnostic,
    "rust.expandMacro" => wb::RustExpandMacro,
    "rust.parentModule" => wb::RustParentModule,
    "rust.openCargoToml" => wb::RustOpenCargoToml,
    "workbench.action.output.toggleOutput" => wb::ShowOutput,
    "workbench.output.action.clearOutput" => wb::ClearOutput,
    "workbench.showPorts" => wb::ShowPorts,
    "workbench.splitEditor" => wb::SplitEditor,
    "workbench.splitEditorLeft" => wb::SplitEditorLeft,
    "workbench.splitEditorUp" => wb::SplitEditorUp,
    "workbench.splitEditorDown" => wb::SplitEditorDown,
    "workbench.closeEmptyEditorGroup" => wb::CloseEmptyEditorGroup,
    "workbench.moveEditorToOtherGroup" => wb::MoveEditorToOtherGroup,
    "editor.toggleWordWrap" => wb::ToggleWordWrap,
    "workbench.goBack" => wb::GoBack,
    "workbench.goForward" => wb::GoForward,
    "workbench.reopenClosedEditor" => wb::ReopenClosedEditor,
    "workbench.newFile" => wb::NewFile,
    "workbench.newWindow" => wb::NewWindow,
    "workbench.closeAllEditors" => wb::CloseAllEditors,
    "workbench.closeOtherEditors" => wb::CloseOtherEditors,
    "workbench.nextEditor" => wb::NextEditor,
    "workbench.previousEditor" => wb::PreviousEditor,
    "workbench.keepEditor" => wb::KeepEditor,
    "workbench.copyPath" => wb::CopyActivePath,
    "workbench.copyRelativePath" => wb::CopyActiveRelativePath,
    "workbench.compareWith" => wb::CompareActiveWith,
    "workbench.newFolder" => wb::NewFolder,
    "workbench.refreshExplorer" => wb::RefreshExplorer,
    "workbench.closeFolder" => wb::CloseFolder,
    "workbench.openWorkspaceSettings" => wb::OpenWorkspaceSettings,
    "projects.switch" => wb::SwitchProject,
    "projects.save" => wb::SaveProject,
    "projects.edit" => wb::EditProjects,
    "projects.forgetRecent" => wb::ForgetRecent,
    "favorites.toggle" => wb::ToggleFavorite,
    "favorites.show" => wb::ShowFavorites,
    "bookmarks.toggle" => ed::ToggleBookmark,
    "bookmarks.jumpToNext" => ed::NextBookmark,
    "bookmarks.jumpToPrevious" => ed::PreviousBookmark,
    "bookmarks.list" => wb::ListBookmarks,
    "tasks.run" => wb::RunTask,
    "workbench.togglePanelPosition" => wb::TogglePanelPosition,
    "workbench.toggleMaximizedPanel" => wb::ToggleMaximizedPanel,
    "tasks.build" => wb::RunBuildTask,
    "tasks.rerun" => wb::RerunLastTask,
    "test.runAtCursor" => wb::RunTestAtCursor,
    "debug.start" => wb::StartDebugging,
    "debug.stop" => wb::StopDebugging,
    "debug.restart" => wb::RestartDebugging,
    "debug.continue" => wb::DebugContinue,
    "debug.pause" => wb::DebugPause,
    "debug.stepOver" => wb::DebugStepOver,
    "debug.stepInto" => wb::DebugStepInto,
    "debug.stepOut" => wb::DebugStepOut,
    "debug.toggleBreakpoint" => ed::ToggleBreakpoint,
    "debug.removeAllBreakpoints" => wb::RemoveAllBreakpoints,
    "debug.editBreakpoint" => wb::EditBreakpoint,
    "debug.breakOnExceptions" => wb::BreakOnExceptions,
    "debug.addDataBreakpoint" => wb::AddDataBreakpoint,
    "debug.toggleBreakpointEnabled" => wb::ToggleBreakpointEnabled,
    "debug.enableAllBreakpoints" => wb::EnableAllBreakpoints,
    "debug.disableAllBreakpoints" => wb::DisableAllBreakpoints,
    "debug.toggleBreakpointsActive" => wb::ToggleBreakpointsActive,
    "debug.addFunctionBreakpoint" => wb::AddFunctionBreakpoint,
    "debug.removeDataBreakpoints" => wb::RemoveDataBreakpoints,
    "debug.addWatch" => wb::AddWatch,
    "debug.removeAllWatches" => wb::RemoveAllWatches,
    "workspace.openFile" => wb::OpenWorkspaceFile,
    "workspace.addFolder" => wb::AddFolderInNewWindow,
    "workspace.addToWorkspace" => wb::AddFolderToWorkspace,
    "workspace.removeFromWorkspace" => wb::RemoveFolderFromWorkspace,
    "debug.installJsDebug" => wb::InstallJsDebug,
    "test.runFile" => wb::RunFileTests,
    "tasks.configure" => wb::ConfigureTasks,
    "bookmarks.clear" => ed::ClearBookmarks,
    "terminal.clear.all" => wb::ClearTerminal,
    "workbench.saveAs" => wb::SaveAs,
    "workbench.saveLocalCopy" => wb::SaveLocalCopy,
    "workbench.revealActiveFile" => wb::RevealActiveFile,
    "workbench.collapseFolders" => wb::CollapseFolders,
    "editor.changeEol" => wb::ChangeEol,
    "editor.changeIndentation" => wb::ChangeIndentation,
    "git.checkout" => wb::GitCheckout,
    "git.branch" => wb::GitCreateBranch,
    "git.merge" => wb::GitMergeBranch,
    "git.rebase" => wb::GitRebaseBranch,
    "git.rebaseContinue" => wb::GitContinueRebase,
    "git.rebaseAbort" => wb::GitAbortRebase,
    "git.addWorktree" => wb::GitAddWorktree,
    "git.openWorktree" => wb::GitOpenWorktree,
    "git.deleteBranch" => wb::GitDeleteBranch,
    "git.renameBranch" => wb::GitRenameBranch,
    "git.publish" => wb::GitPublishBranch,
    "git.abortMerge" => wb::GitAbortMerge,
    "git.addRemote" => wb::GitAddRemote,
    "git.removeRemote" => wb::GitRemoveRemote,
    "git.createTag" => wb::GitCreateTag,
    "git.pull" => wb::GitPull,
    "git.push" => wb::GitPush,
    "git.fetch" => wb::GitFetch,
    "git.sync" => wb::GitSync,
    // --- git-sync ---
    "git.pullRebase" => wb::GitPullRebase,
    "git.pullFrom" => wb::GitPullFrom,
    "git.pushTo" => wb::GitPushTo,
    "git.fetchAll" => wb::GitFetchAll,
    "git.commitSignedOff" => wb::GitCommitSignedOff,
    "git.commitNoVerify" => wb::GitCommitNoVerify,
    "git.commitAndPush" => wb::GitCommitAndPush,
    "git.commitAndSync" => wb::GitCommitAndSync,
    "git.commitAmend" => wb::GitCommitAmend,
    "git.viewFileHistory" => wb::GitFileHistory,
    "git.blameLine" => wb::GitBlameLine,
    "git.undoLastCommit" => wb::GitUndoLastCommit,
    "git.stash" => wb::GitStash,
    "git.stashIncludeUntracked" => wb::GitStashIncludeUntracked,
    "git.stashPopLatest" => wb::GitPopLatestStash,
    "git.stashPop" => wb::GitPopStash,
    "git.stashApply" => wb::GitApplyStash,
    "git.stashDrop" => wb::GitDropStash,
    // --- git-refs ---
    "git.deleteTag" => wb::GitDeleteTag,
    "git.deleteRemoteTag" => wb::GitDeleteRemoteTag,
    "git.pushTags" => wb::GitPushTags,
    "git.pushTag" => wb::GitPushTag,
    "git.checkoutDetached" => wb::GitCheckoutDetached,
    "git.branchFrom" => wb::GitCreateBranchFrom,
    "git.deleteRemoteBranch" => wb::GitDeleteRemoteBranch,
    "git.setUpstream" => wb::GitSetUpstream,
    "git.stashStaged" => wb::GitStashStaged,
    "git.stashNamed" => wb::GitStashNamed,
    "git.stashShow" => wb::GitShowStash,
    "git.openChanges" => wb::GitOpenChanges,
    "git.openFileHead" => wb::GitOpenFileHead,
    "git.compareBranch" => wb::GitCompareBranch,
    "git.compareCommit" => wb::GitCompareCommit,
    "git.addToGitignore" => wb::GitAddToGitignore,
    "git.renameRemote" => wb::GitRenameRemote,
    "git.setRemoteUrl" => wb::GitSetRemoteUrl,
    "git.removeWorktree" => wb::GitRemoveWorktree,
    "git.pruneWorktrees" => wb::GitPruneWorktrees,
    "git.discardAll" => wb::GitDiscardAll,
    "workbench.restartLanguageServers" => wb::RestartLanguageServers,
    "editor.fold" => ed::FoldRegion,
    "editor.selectAllOccurrences" => ed::SelectAllOccurrences,
    "editor.deleteToLineStart" => ed::DeleteToLineStart,
    "editor.insertLineBelow" => ed::InsertLineBelow,
    "editor.insertLineAbove" => ed::InsertLineAbove,
    "editor.indentLine" => ed::IndentLine,
    "editor.outdentLine" => ed::OutdentLine,
    "editor.selectPageUp" => ed::SelectPageUp,
    "editor.selectPageDown" => ed::SelectPageDown,
    "editor.jumpToBracket" => ed::JumpToBracket,
    "editor.cursorUndo" => ed::CursorUndo,
    "editor.cursorsAtLineEnds" => ed::CursorsAtLineEnds,
    "editor.expandSelection" => ed::ExpandSelection,
    "editor.shrinkSelection" => ed::ShrinkSelection,
    "editor.joinLines" => ed::JoinLines,
    "editor.sortLinesAscending" => ed::SortLinesAscending,
    "editor.sortLinesDescending" => ed::SortLinesDescending,
    "editor.transformToUppercase" => ed::TransformToUppercase,
    "editor.transformToLowercase" => ed::TransformToLowercase,
    "editor.transformToTitlecase" => ed::TransformToTitleCase,
    "editor.duplicateSelection" => ed::DuplicateSelection,
    "editor.trimTrailingWhitespace" => ed::TrimTrailingWhitespace,
    "editor.foldLevel1" => ed::FoldLevel1,
    "editor.foldLevel2" => ed::FoldLevel2,
    "editor.foldLevel3" => ed::FoldLevel3,
    "editor.foldLevel4" => ed::FoldLevel4,
    "editor.foldLevel5" => ed::FoldLevel5,
    "editor.foldLevel6" => ed::FoldLevel6,
    "editor.foldLevel7" => ed::FoldLevel7,
    "editor.foldRecursively" => ed::FoldRecursively,
    "editor.unfoldRecursively" => ed::UnfoldRecursively,
    "git.stageSelectedRanges" => ed::StageHunk,
    "deleteAllRight" => ed::DeleteAllRight,
    "editor.action.peekDefinition" => ed::PeekDefinition,
    "editor.runCodeLens" => ed::RunCodeLens,
    "references-view.showCallHierarchy" => ed::ShowIncomingCalls,
    "editor.showOutgoingCalls" => ed::ShowOutgoingCalls,
    "editor.showSupertypes" => ed::ShowSupertypes,
    "editor.showSubtypes" => ed::ShowSubtypes,
    "workbench.action.editor.changeLanguageMode" => wb::ChangeLanguage,
    "editor.selectToBracket" => ed::SelectToBracket,
    "editor.addCommentLine" => ed::AddLineComment,
    "editor.removeCommentLine" => ed::RemoveLineComment,
    "editor.addSelectionToPreviousFindMatch" => ed::AddSelectionToPreviousFindMatch,
    "workbench.action.editor.nextChange" => ed::NextChange,
    "workbench.action.editor.previousChange" => ed::PreviousChange,
    "workbench.closeSavedEditors" => wb::CloseSavedEditors,
    "editor.convertIndentationToSpaces" => wb::ConvertIndentToSpaces,
    "editor.convertIndentationToTabs" => wb::ConvertIndentToTabs,
    "editor.toggleMinimap" => wb::ToggleMinimap,
    "editor.toggleStickyScroll" => wb::ToggleStickyScroll,
    "editor.toggleRenderWhitespace" => wb::ToggleRenderWhitespace,
    "cursorColumnSelectUp" => ed::ColumnSelectUp,
    "cursorColumnSelectDown" => ed::ColumnSelectDown,
    "cursorColumnSelectLeft" => ed::ColumnSelectLeft,
    "cursorColumnSelectRight" => ed::ColumnSelectRight,
    "git.revertSelectedRanges" => ed::RevertHunk,
    "editor.transposeCharacters" => ed::TransposeCharacters,
    "editor.transformToCamelCase" => ed::TransformToCamelCase,
    "editor.transformToPascalCase" => ed::TransformToPascalCase,
    "editor.transformToSnakeCase" => ed::TransformToSnakeCase,
    "editor.transformToKebabCase" => ed::TransformToKebabCase,
    "editor.transformToConstantCase" => ed::TransformToConstantCase,
    "editor.toggleBlockComment" => ed::ToggleBlockComment,
    "editor.transformBase64Encode" => ed::TransformBase64Encode,
    "editor.transformBase64Decode" => ed::TransformBase64Decode,
    "editor.transformUrlEncode" => ed::TransformUrlEncode,
    "editor.transformUrlDecode" => ed::TransformUrlDecode,
    "editor.transformHtmlEscape" => ed::TransformHtmlEscape,
    "editor.transformHtmlUnescape" => ed::TransformHtmlUnescape,
    "editor.pickColor" => ed::PickColor,
    "editor.sortLinesInsensitive" => ed::SortLinesInsensitive,
    "editor.sortLinesNatural" => ed::SortLinesNatural,
    "editor.removeBlankLines" => ed::RemoveBlankLines,
    "editor.collapseBlankLines" => ed::CollapseBlankLines,
    "editor.insertDate" => ed::InsertDate,
    "editor.insertDateTime" => ed::InsertDateTime,
    "editor.insertUuid" => ed::InsertUuid,
    "editor.reverseLines" => ed::ReverseLines,
    "editor.deleteDuplicateLines" => ed::DeleteDuplicateLines,
    "editor.nextProblem" => ed::NextProblem,
    "merge-conflict.accept.current" => ed::AcceptCurrentChange,
    "merge-conflict.accept.incoming" => ed::AcceptIncomingChange,
    "merge-conflict.accept.both" => ed::AcceptBothChanges,
    "merge-conflict.accept.all-current" => ed::AcceptAllCurrent,
    "merge-conflict.accept.all-incoming" => ed::AcceptAllIncoming,
    "merge-conflict.next" => ed::NextConflict,
    "merge-conflict.previous" => ed::PreviousConflict,
    "editor.previousProblem" => ed::PreviousProblem,
    "editor.goToTypeDefinition" => ed::GoToTypeDefinition,
    "editor.goToImplementation" => ed::GoToImplementation,
    "workbench.revertFile" => ed::RevertFile,
    "editor.unfold" => ed::UnfoldRegion,
    "editor.foldAll" => ed::FoldAll,
    "editor.unfoldAll" => ed::UnfoldAll,
    "workbench.focusLeftGroup" => wb::FocusLeftGroup,
    "workbench.focusRightGroup" => wb::FocusRightGroup,
    "workbench.focusUpGroup" => wb::FocusUpGroup,
    "workbench.focusDownGroup" => wb::FocusDownGroup,
    "diff.nextChange" => crate::diff::NextChange,
    "diff.previousChange" => crate::diff::PrevChange,
    "workbench.closeEditor" => wb::CloseEditor,
    "workbench.selectTheme" => wb::SelectTheme,
    "workbench.selectIconTheme" => wb::SelectIconTheme,
    "workbench.showWelcome" => wb::ShowWelcome,
    "workbench.saveThemeAs" => wb::SaveThemeAs,
    "workbench.openThemesFolder" => wb::OpenThemesFolder,
    "json.minify" => wb::MinifyJson,
    "workbench.showShortcuts" => wb::ShowShortcutsPage,
    "editor.insertSnippet" => wb::InsertSnippet,
    "workbench.configureSnippets" => wb::ConfigureSnippets,
    "markdown.openPreview" => wb::OpenMarkdownPreview,
    "workbench.compareWithClipboard" => wb::CompareWithClipboard,
    "workbench.compareWithSaved" => wb::CompareWithSaved,
    // --- compare ---
    "workbench.compareFolders" => wb::CompareFolders,
    "workbench.runSelectedText" => wb::RunSelectedText,
    "workbench.goToLastEdit" => wb::GoToLastEdit,
    "workbench.newScratchFile" => wb::NewScratchFile,
    // --- data ---
    "data.convert" => wb::DataConvert,
    "data.query" => wb::DataQuery,
    "data.copyPath" => wb::DataCopyPath,
    "data.decodeSelection" => wb::DataDecode,
    "workbench.openSettingsPage" => wb::OpenSettingsPage,
    "git.cloneRepository" => wb::CloneRepository,
    "workbench.togglePanel" => wb::TogglePanel,
    "workbench.openSettings" => wb::OpenSettings,
    "workbench.openKeybindings" => wb::OpenKeybindings,
    "workbench.settings" => wb::ShowSettings,
    "workbench.keyboardShortcuts" => wb::ShowKeyboardShortcuts,
    "workbench.importVsCode" => wb::ImportVsCode,
    "workbench.quit" => wb::Quit,
    "workbench.checkForUpdates" => wb::CheckForUpdates,
    "terminal.toggle" => wb::ToggleTerminal,
    "terminal.new" => wb::NewTerminal,
    "terminal.split" => wb::SplitTerminal,
    "terminal.kill" => wb::KillTerminal,
    "terminal.paste" => term::TermPaste,
    "terminal.copy" => term::TermCopy,
    "terminal.clear" => term::TermClear,
    "editor.save" => ed::Save,
    "editor.undo" => ed::Undo,
    "editor.redo" => ed::Redo,
    "editor.selectAll" => ed::SelectAll,
    "editor.copy" => ed::Copy,
    "editor.cut" => ed::Cut,
    "editor.paste" => ed::Paste,
    "editor.newline" => ed::Newline,
    "editor.indent" => ed::Tab,
    "editor.deleteLeft" => ed::Backspace,
    "editor.deleteRight" => ed::Delete,
    "editor.cursorLeft" => ed::MoveLeft,
    "editor.cursorRight" => ed::MoveRight,
    "editor.cursorUp" => ed::MoveUp,
    "editor.cursorDown" => ed::MoveDown,
    "editor.cursorWordLeft" => ed::MoveWordLeft,
    "editor.cursorWordRight" => ed::MoveWordRight,
    "editor.selectWordLeft" => ed::SelectWordLeft,
    "editor.selectWordRight" => ed::SelectWordRight,
    "editor.deleteWordLeft" => ed::DeleteWordLeft,
    "editor.deleteWordRight" => ed::DeleteWordRight,
    "editor.cursorHome" => ed::MoveHome,
    "editor.cursorEnd" => ed::MoveEnd,
    "editor.cursorTop" => ed::DocStart,
    "editor.cursorBottom" => ed::DocEnd,
    "editor.selectLeft" => ed::SelectLeft,
    "editor.selectRight" => ed::SelectRight,
    "editor.selectUp" => ed::SelectUp,
    "editor.selectDown" => ed::SelectDown,
    "editor.selectHome" => ed::SelectHome,
    "editor.selectEnd" => ed::SelectEnd,
    "editor.pageUp" => ed::PageUp,
    "editor.pageDown" => ed::PageDown,
    "editor.cancel" => ed::Escape,
    "editor.addCursorAbove" => ed::AddCursorAbove,
    "editor.addCursorBelow" => ed::AddCursorBelow,
    "editor.addNextOccurrence" => ed::SelectNextOccurrence,
    "editor.triggerSuggest" => ed::TriggerCompletion,
    "editor.acceptSuggestion" => ed::CompletionAccept,
    "editor.goToDefinition" => ed::GoToDefinition,
    "editor.showHover" => ed::ShowHover,
    "editor.toggleComment" => ed::ToggleComment,
    "editor.outdent" => ed::Outdent,
    "editor.moveLinesUp" => ed::MoveLineUp,
    "editor.moveLinesDown" => ed::MoveLineDown,
    "editor.copyLinesUp" => ed::CopyLineUp,
    "editor.copyLinesDown" => ed::CopyLineDown,
    "editor.deleteLines" => ed::DeleteLine,
    "editor.selectLine" => ed::SelectLine,
    "editor.rename" => wb::RenameSymbol,
    "editor.findReferences" => wb::FindReferences,
    "editor.goToSymbol" => wb::GoToSymbol,
    "workbench.goToWorkspaceSymbol" => wb::GoToWorkspaceSymbol,
    "editor.quickFix" => wb::CodeActions,
    "editor.formatDocument" => wb::FormatDocument,
    "editor.formatSelection" => wb::FormatSelection,
    "workbench.saveAll" => wb::SaveAll,
    "workbench.zoomIn" => wb::ZoomIn,
    "workbench.zoomOut" => wb::ZoomOut,
    "workbench.zoomReset" => wb::ZoomReset,
    "editor.find" => ed::Find,
    "editor.findReplace" => ed::FindReplace,
    "editor.findNext" => ed::FindNext,
    "editor.findPrevious" => crate::find::FindPrev,
    "find.replaceAll" => crate::find::ReplaceAll,
    "find.toggleCase" => crate::find::ToggleCase,
    "find.toggleWholeWord" => crate::find::ToggleWord,
    "find.toggleRegex" => crate::find::ToggleRegex,
    "find.togglePreserveCase" => crate::find::TogglePreserveCase,
    "tree.rename" => tree::RenameEntry,
    "tree.delete" => tree::DeleteEntry,
    "tree.copy" => tree::CopyEntry,
    "tree.cut" => tree::CutEntry,
    "tree.paste" => tree::PasteEntry,
    "tree.selectAll" => tree::SelectAllEntries,
    "tree.cancelOperation" => wb::CancelFileOperation,
    "list.up" => picker::PickPrev,
    "list.down" => picker::PickNext,
    "list.accept" => picker::PickConfirm,
    "list.close" => picker::PickDismiss,
    "menu.up" => menu::MenuPrev,
    "menu.down" => menu::MenuNext,
    "field.submit" => field::FieldSubmit,
}
