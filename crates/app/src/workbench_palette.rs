//! Workbench: command list, palette and overlay events.

use super::*;

impl Workbench {
    pub(super) fn quick_open(&mut self, _: &QuickOpen, window: &mut Window, cx: &mut Context<Self>) {
        self.open_palette("", window, cx);
    }

    pub(super) fn command_palette(&mut self, _: &CommandPalette, window: &mut Window, cx: &mut Context<Self>) {
        self.open_palette(">", window, cx);
    }

    pub(super) fn go_to_workspace_symbol(&mut self, _: &GoToWorkspaceSymbol, window: &mut Window, cx: &mut Context<Self>) {
        self.open_palette("#", window, cx);
    }

    pub(super) fn go_to_line(&mut self, _: &GoToLine, window: &mut Window, cx: &mut Context<Self>) {
        self.open_palette(":", window, cx);
    }

    /// The command's name as shown in the palette (in the UI language); `label` stays as the English identifier.
    pub(super) fn command_title(c: &Command) -> String {
        match &c.run {
            Run::Connect(alias) => crate::i18n::tf("Remote: Connect to {}", &[alias]),
            _ => crate::i18n::t(&c.label).to_string(),
        }
    }

    pub(super) fn commands(&self) -> Vec<Command> {
        use crate::editor;
        let mut cmds = vec![
            Command::action("Go: File…", QuickOpen),
            Command::action("Go: Line…", GoToLine),
            Command::action("Search: Find in Files", FindInFiles),
            Command::action("Search: Open Results in Editor", OpenSearchInEditor),
            Command::action("File: Follow (Tail) Active File", FollowFile),
            Command::action("File: Filter Log Lines…", FilterLog),
            Command::action("File: Open Active File as Hex", OpenAsHex),
            Command::action("Search: Find TODO Comments", FindTodos),
            // --- structural ---
            Command::action("Search: Structural Search…", crate::search::StructuralSearch),
            Command::action("Search: Structural Replace…", crate::search::StructuralReplace),
            Command::action("View: Switch Editor…", SwitchEditor),
            Command::action("View: Toggle Zen Mode", ToggleZenMode),
            Command::action("Extensions: Install cyx Command in PATH", InstallCyx),
            Command::action("Extensions: New Extension…", NewExtension),
            Command::action("View: Toggle Full Screen", ToggleFullScreen),
            Command::action("Files: Open Folder…", OpenFolder),
            Command::action("Editor: Save", editor::Save),
            Command::action("Editor: Close", CloseEditor),
            Command::action("Editor: Undo", editor::Undo),
            Command::action("Editor: Redo", editor::Redo),
            Command::action("Editor: Select All", editor::SelectAll),
            Command::action("Editor: Find", editor::Find),
            Command::action("Editor: Replace", editor::FindReplace),
            Command::action("Editor: Toggle Line Comment", editor::ToggleComment),
            Command::action("Editor: Move Line Up", editor::MoveLineUp),
            Command::action("Editor: Move Line Down", editor::MoveLineDown),
            Command::action("Editor: Copy Line Down", editor::CopyLineDown),
            Command::action("Editor: Delete Line", editor::DeleteLine),
            Command::action("View: Zoom In", ZoomIn),
            Command::action("View: Zoom Out", ZoomOut),
            Command::action("View: Reset Zoom", ZoomReset),
            Command::action("View: Toggle Word Wrap", ToggleWordWrap),
            Command::action("Editor: Fold", editor::FoldRegion),
            Command::action("Editor: Fold Level 1", editor::FoldLevel1),
            Command::action("Editor: Fold Level 2", editor::FoldLevel2),
            Command::action("Editor: Fold Level 3", editor::FoldLevel3),
            Command::action("Editor: Fold Level 4", editor::FoldLevel4),
            Command::action("Editor: Fold Level 5", editor::FoldLevel5),
            Command::action("Editor: Fold Level 6", editor::FoldLevel6),
            Command::action("Editor: Fold Level 7", editor::FoldLevel7),
            Command::action("Editor: Fold Recursively", editor::FoldRecursively),
            Command::action("Editor: Unfold Recursively", editor::UnfoldRecursively),
            Command::action("Editor: Select All Occurrences of Find Match", editor::SelectAllOccurrences),
            Command::action("Editor: Expand Selection", editor::ExpandSelection),
            Command::action("Editor: Shrink Selection", editor::ShrinkSelection),
            Command::action("Editor: Join Lines", editor::JoinLines),
            Command::action("Editor: Sort Lines Ascending", editor::SortLinesAscending),
            Command::action("Editor: Sort Lines Descending", editor::SortLinesDescending),
            Command::action("Editor: Transform to Uppercase", editor::TransformToUppercase),
            Command::action("Editor: Transform to Title Case", editor::TransformToTitleCase),
            Command::action("Editor: Duplicate Selection", editor::DuplicateSelection),
            Command::action("Editor: Trim Trailing Whitespace", editor::TrimTrailingWhitespace),
            Command::action("Git: Stage Change at Cursor", editor::StageHunk),
            Command::action("Editor: Delete All Right", editor::DeleteAllRight),
            Command::action("Editor: Select to Bracket", editor::SelectToBracket),
            Command::action("Editor: Add Line Comment", editor::AddLineComment),
            Command::action("Editor: Remove Line Comment", editor::RemoveLineComment),
            Command::action("Editor: Add Selection to Previous Find Match", editor::AddSelectionToPreviousFindMatch),
            Command::action("Go: Next Change", editor::NextChange),
            Command::action("Editor: Peek Definition", editor::PeekDefinition),
            Command::action("Editor: Run Code Lens on This Line", editor::RunCodeLens),
            Command::action("Editor: Show Incoming Calls", editor::ShowIncomingCalls),
            Command::action("Editor: Show Outgoing Calls", editor::ShowOutgoingCalls),
            Command::action("Editor: Show Supertypes", editor::ShowSupertypes),
            Command::action("Editor: Show Subtypes", editor::ShowSubtypes),
            Command::action("Go: Previous Change", editor::PreviousChange),
            Command::action("View: Close Saved Editors", CloseSavedEditors),
            Command::action("Editor: Convert Indentation to Spaces", ConvertIndentToSpaces),
            Command::action("Editor: Convert Indentation to Tabs", ConvertIndentToTabs),
            Command::action("View: Toggle Minimap", ToggleMinimap),
            Command::action("View: Toggle Sticky Scroll", ToggleStickyScroll),
            Command::action("View: Toggle Render Whitespace", ToggleRenderWhitespace),
            Command::action("Git: Revert Change at Cursor", editor::RevertHunk),
            Command::action("Editor: Transpose Characters", editor::TransposeCharacters),
            Command::action("Editor: Transform to camelCase", editor::TransformToCamelCase),
            Command::action("Editor: Transform to PascalCase", editor::TransformToPascalCase),
            Command::action("Editor: Transform to snake_case", editor::TransformToSnakeCase),
            Command::action("Editor: Transform to kebab-case", editor::TransformToKebabCase),
            Command::action("Editor: Transform to CONSTANT_CASE", editor::TransformToConstantCase),
            Command::action("Editor: Toggle Block Comment", editor::ToggleBlockComment),
            Command::action("Editor: Base64 Encode", editor::TransformBase64Encode),
            Command::action("Editor: Base64 Decode", editor::TransformBase64Decode),
            Command::action("Editor: URL Encode", editor::TransformUrlEncode),
            Command::action("Editor: URL Decode", editor::TransformUrlDecode),
            Command::action("Editor: HTML Escape", editor::TransformHtmlEscape),
            Command::action("Editor: HTML Unescape", editor::TransformHtmlUnescape),
            Command::action("Editor: Pick Color…", editor::PickColor),
            Command::action("Editor: Sort Lines (Case-Insensitive)", editor::SortLinesInsensitive),
            Command::action("Editor: Sort Lines (Natural)", editor::SortLinesNatural),
            Command::action("Editor: Remove Blank Lines", editor::RemoveBlankLines),
            Command::action("Editor: Collapse Blank Lines", editor::CollapseBlankLines),
            Command::action("Editor: Insert Date", editor::InsertDate),
            Command::action("Editor: Insert Date and Time (UTC)", editor::InsertDateTime),
            Command::action("Editor: Insert UUID", editor::InsertUuid),
            Command::action("Editor: Reverse Lines", editor::ReverseLines),
            Command::action("Editor: Delete Duplicate Lines", editor::DeleteDuplicateLines),
            Command::action("Editor: Transform to Lowercase", editor::TransformToLowercase),
            Command::action("Go: Next Problem", editor::NextProblem),
            Command::action("Go: Previous Problem", editor::PreviousProblem),
            Command::action("Editor: Go to Type Definition", editor::GoToTypeDefinition),
            Command::action("Editor: Go to Implementations", editor::GoToImplementation),
            Command::action("File: Revert File", editor::RevertFile),
            Command::action("Editor: Unfold", editor::UnfoldRegion),
            Command::action("Editor: Fold All", editor::FoldAll),
            Command::action("Editor: Unfold All", editor::UnfoldAll),
            Command::action("Editor: Go to Definition", editor::GoToDefinition),
            Command::action("Editor: Rename Symbol", RenameSymbol),
            Command::action("Editor: Find All References", FindReferences),
            Command::action("Go: Symbol in File…", GoToSymbol),
            Command::action("Go: Symbol in Workspace…", GoToWorkspaceSymbol),
            Command::action("Editor: Quick Fix…", CodeActions),
            Command::action("Editor: Format Document", FormatDocument),
            Command::action("Editor: Format Selection", FormatSelection),
            Command::action("File: Save All", SaveAll),
            Command::action("File: New File…", NewFile),
            Command::action("File: New Window", NewWindow),
            Command::action("File: Open Workspace from File…", OpenWorkspaceFile),
            Command::action("File: Add Folder in New Window…", AddFolderInNewWindow),
            Command::action("File: Add Folder to Workspace…", AddFolderToWorkspace),
            Command::action("File: Remove Folder from Workspace…", RemoveFolderFromWorkspace),
            Command::action("File: New Folder…", NewFolder),
            Command::action("File: Close Folder", CloseFolder),
            Command::action("Projects: Switch Project… (Open Recent)", SwitchProject),
            Command::action("Projects: Add Folder to Projects…", SaveProject),
            Command::action("Projects: Edit Projects", EditProjects),
            Command::action("Projects: Remove from Recent…", ForgetRecent),
            Command::action("Favorites: Toggle Active File", ToggleFavorite),
            Command::action("Favorites: Show Favorites…", ShowFavorites),
            Command::action("Tasks: Run Task…", RunTask),
            Command::action("Tasks: Run Build Task", RunBuildTask),
            Command::action("Tasks: Rerun Last Task", RerunLastTask),
            Command::action("Debug: Start Debugging", StartDebugging),
            Command::action("Debug: Continue", DebugContinue),
            Command::action("Debug: Pause", DebugPause),
            Command::action("Debug: Step Over", DebugStepOver),
            Command::action("Debug: Step Into", DebugStepInto),
            Command::action("Debug: Step Out", DebugStepOut),
            Command::action("Debug: Restart", RestartDebugging),
            Command::action("Debug: Stop", StopDebugging),
            Command::action("Debug: Toggle Breakpoint", editor::ToggleBreakpoint),
            Command::action("Debug: Edit Breakpoint… (condition, log message, hit count)", EditBreakpoint),
            Command::action("Debug: Break on Exceptions…", BreakOnExceptions),
            Command::action("Debug: Break When Value Changes…", AddDataBreakpoint),
            Command::action("Debug: Remove Data Breakpoints", RemoveDataBreakpoints),
            Command::action("Debug: Add Watch Expression…", AddWatch),
            Command::action("Debug: Remove All Watch Expressions", RemoveAllWatches),
            Command::action("Debug: Install JavaScript Debugger", InstallJsDebug),
            Command::action("Debug: Remove All Breakpoints", RemoveAllBreakpoints),
            Command::action("Debug: Enable/Disable Breakpoint", ToggleBreakpointEnabled),
            Command::action("Debug: Enable All Breakpoints", EnableAllBreakpoints),
            Command::action("Debug: Disable All Breakpoints", DisableAllBreakpoints),
            Command::action("Debug: Deactivate/Activate Breakpoints", ToggleBreakpointsActive),
            Command::action("Debug: Add Function Breakpoint…", AddFunctionBreakpoint),
            Command::action("Debug: Show Debug Panel", ShowDebugPanel),
            Command::action("Test: Run Test at Cursor", RunTestAtCursor),
            Command::action("Test: Run Tests in File", RunFileTests),
            Command::action("Test: Run All Tests", RunAllTests),
            Command::action("Test: Run Failed Tests", RunFailedTests),
            Command::action("Test: Refresh Tests", RefreshTests),
            Command::action("Test: Stop Running Tests", StopTests),
            Command::action("Test: Debug Test at Cursor", DebugTestAtCursor),
            Command::action("Tasks: Configure Tasks", ConfigureTasks),
            Command::action("Merge Conflict: Open Merge Editor", OpenMergeEditor),
            Command::action("GitHub: Pull Requests…", ShowPullRequests),
            Command::action("HTTP: Send Request", SendRequest),
            Command::action("HTTP: Show Last Response Headers", ShowResponseHeaders),
            Command::action("Merge Conflict: Accept Current", editor::AcceptCurrentChange),
            Command::action("Merge Conflict: Accept Incoming", editor::AcceptIncomingChange),
            Command::action("Merge Conflict: Accept Both", editor::AcceptBothChanges),
            Command::action("Merge Conflict: Accept All Current", editor::AcceptAllCurrent),
            Command::action("Merge Conflict: Accept All Incoming", editor::AcceptAllIncoming),
            Command::action("Merge Conflict: Next Conflict", editor::NextConflict),
            Command::action("Merge Conflict: Previous Conflict", editor::PreviousConflict),
            Command::action("Bookmarks: Toggle", editor::ToggleBookmark),
            Command::action("Bookmarks: Jump to Next", editor::NextBookmark),
            Command::action("Bookmarks: Jump to Previous", editor::PreviousBookmark),
            Command::action("Bookmarks: List…", ListBookmarks),
            Command::action("Bookmarks: Clear in File", editor::ClearBookmarks),
            Command::action("Preferences: Open Workspace Settings", OpenWorkspaceSettings),
            Command::action("File: Refresh Explorer", RefreshExplorer),
            Command::action("File: Copy Path of Active File", CopyActivePath),
            Command::action("File: Copy Relative Path of Active File", CopyActiveRelativePath),
            Command::action("File: Compare Active File With…", CompareActiveWith),
            Command::action("View: Close All Editors", CloseAllEditors),
            Command::action("View: Close Other Editors", CloseOtherEditors),
            Command::action("View: Open Next Editor", NextEditor),
            Command::action("View: Open Previous Editor", PreviousEditor),
            Command::action("View: Keep Editor", KeepEditor),
            Command::action("Terminal: Clear", ClearTerminal),
            Command::action("File: Reveal Active File in Explorer", RevealActiveFile),
            Command::action("Editor: Change End of Line Sequence", ChangeEol),
            Command::action("Editor: Change Language Mode", ChangeLanguage),
            Command::action("File: Open Active File With…", OpenWith),
            Command::action("Editor: Change Indentation", ChangeIndentation),
            Command::action("Files: Collapse Folders in Explorer", CollapseFolders),
            Command::action("File: Save As…", SaveAs),
            Command::action("File: Save a Copy to This Computer…", SaveLocalCopy),
            Command::action("View: Reopen Closed Editor", ReopenClosedEditor),
            Command::action("Go: Back", GoBack),
            Command::action("Go: Forward", GoForward),
            Command::action("Editor: Trigger Suggest", editor::TriggerCompletion),
            Command::action("Editor: Show Hover", editor::ShowHover),
            Command::action("Editor: Add Cursor Above", editor::AddCursorAbove),
            Command::action("Editor: Add Cursor Below", editor::AddCursorBelow),
            Command::action("Editor: Add Next Occurrence to Selection", editor::SelectNextOccurrence),
            Command::action("Workspace: Trust This Folder", TrustWorkspace),
            Command::action("View: Toggle Sidebar", ToggleSidebar),
            Command::action("View: Files", ShowExplorer),
            Command::action("Remote: Connect to Host or Switch Workspace…", ShowRemote),
            Command::action("Theme: Pick Color Theme…", SelectTheme),
            Command::action("Theme: Pick File Icon Theme…", SelectIconTheme),
            Command::action("Remote: Add SSH Host…", AddSshHost),
            Command::action("Remote: Remove SSH Host…", RemoveSshHost),
            Command::action("Remote: Test SSH Connection…", TestSshHost),
            Command::action("Remote: Edit SSH Config", EditSshConfig),
            Command::action("Remote: Manage SSH Hosts…", ManageSshHosts),
            Command::action("Remote: Edit SSH Host…", EditSshHost),
            Command::action("Remote: Set Up Key Login…", SetupKeyLogin),
            Command::action("Schemas: Clear Download Cache", ClearSchemaCache),
            Command::action("Spelling: Fix Word at Cursor…", FixSpelling),
            Command::action("Spelling: Install Dictionary…", InstallDictionary),
            Command::action("Spelling: Toggle Spell Check", ToggleSpelling),
            Command::action("Remote: Upload Files to This Folder…", UploadHere),
            Command::action("Remote: Download Active File…", DownloadActive),
            Command::action("Help: About Codeyard", ShowAbout),
            Command::action("Help: Check for Updates…", CheckForUpdates),
            Command::action("Help: Grant Full Disk Access…", ShowDiskAccess),
            Command::action("Theme: Save Current Colors as User Theme…", SaveThemeAs),
            Command::action("Theme: Open User Themes Folder", OpenThemesFolder),
            Command::action("Preferences: Settings", ShowSettings),
            Command::action("Preferences: Keyboard Shortcuts", ShowKeyboardShortcuts),
            Command::action("Preferences: Open Settings", OpenSettingsPage),
            Command::action("Preferences: Open Settings (JSON)", OpenSettings),
            Command::action("Help: Welcome", ShowWelcome),
            Command::action("JSON: Minify Document", MinifyJson),
            Command::action("Help: Keyboard Shortcuts Reference", ShowShortcutsPage),
            Command::action("Snippets: Insert Snippet…", InsertSnippet),
            Command::action("Preferences: Configure User Snippets", ConfigureSnippets),
            Command::action("Markdown: Open Preview", OpenMarkdownPreview),
            Command::action("File: Compare Active File with Clipboard", CompareWithClipboard),
            Command::action("File: Compare Active File with Saved", CompareWithSaved),
            // --- compare ---
            Command::action("Compare: Folders…", CompareFolders),
            Command::action("File: New Scratch File…", NewScratchFile),
            // --- data ---
            Command::action("Data: Convert To…", DataConvert),
            Command::action("Data: Query…", DataQuery),
            Command::action("Data: Copy Path at Cursor", DataCopyPath),
            Command::action("Data: Decode Selection…", DataDecode),
            Command::action("Terminal: Run Selected Text", RunSelectedText),
            Command::action("Go: Last Edit Location", GoToLastEdit),
            Command::action("Git: Clone Repository…", CloneRepository),
            Command::action("Preferences: Open Keyboard Shortcuts", OpenKeybindings),
            Command::action("Preferences: Import VS Code Settings and Shortcuts", ImportVsCode),
            Command::action("View: Problems", ShowProblems),
            Command::action("Problems: Copy Full Diagnostic", CopyFullDiagnostic),
            Command::action("Rust: Expand Macro Recursively", RustExpandMacro),
            Command::action("Rust: Locate Parent Module", RustParentModule),
            Command::action("Rust: Open Cargo.toml", RustOpenCargoToml),
            Command::action("View: Output", ShowOutput),
            Command::action("Output: Clear", ClearOutput),
            Command::action("View: Ports (SSH port forwarding)", ShowPorts),
            Command::action("View: Split Editor", SplitEditor),
            Command::action("View: Move Editor into Other Group", MoveEditorToOtherGroup),
            Command::action("View: Focus Left Editor Group", FocusLeftGroup),
            Command::action("View: Focus Right Editor Group", FocusRightGroup),
            Command::action("View: Source Control", ShowScm),
            Command::action("View: Run and Debug", ShowRunAndDebug),
            Command::action("View: Testing", ShowTesting),
            Command::action("Extensions: Show Extensions", ShowExtensions),
            Command::action("Extensions: Install from File…", InstallExtensionFromFile),
            Command::action("Extensions: Install from Folder…", InstallExtensionFromFolder),
            Command::action("Terminal: New Terminal with Profile…", NewTerminalWithProfile),
            Command::action("Terminal: Select Default Profile…", SelectDefaultTerminalProfile),
            Command::action("Terminal: Rename…", RenameTerminal),
            Command::action("Terminal: Change Color…", ChangeTerminalColor),
            Command::action("Terminal: Kill All Terminals", KillAllTerminals),
            Command::action("Terminal: Go to Previous Command", crate::terminal::TermPrevCommand),
            Command::action("Terminal: Go to Next Command", crate::terminal::TermNextCommand),
            Command::action("Terminal: Copy Last Command Output", crate::terminal::TermCopyLastOutput),
            Command::action("Debug: Run Without Debugging", RunWithoutDebugging),
            Command::action("Debug: Run to Cursor", RunToCursor),
            Command::action("Debug: Add Configuration…", AddDebugConfiguration),
            Command::action("Debug: Open Launch Configuration", OpenLaunchConfiguration),
            Command::action("Debug: Stop All", StopAllDebugging),
            Command::action("View: Remote Explorer", ShowRemoteExplorer),
            Command::action("Git: Checkout to…", GitCheckout),
            Command::action("Git: Create Branch…", GitCreateBranch),
            Command::action("Remote: Connect to Dev Container…", RemoteDocker),
            Command::action("Docker: Open Project in Dev Container…", DockerOpenProject),
            Command::action("Docker: Rebuild Dev Container…", DockerRebuild),
            Command::action("Docker: Show Containers and Services…", DockerServices),
            Command::action("Docker: Cancel Operation", DockerCancel),
            Command::action("View: Containers", ShowContainers),
            Command::action("Web Preview: Open in Browser", OpenWebPreview),
            Command::action("Web Preview: Copy URL", CopyWebPreviewUrl),
            Command::action("Web Preview: Stop Server", StopWebPreview),
            Command::action("Web Preview: Show Server Log", WebPreviewLog),
            Command::action("Remote: Connect to WSL Distro…", RemoteWsl),
            Command::action("Git: Merge Branch…", GitMergeBranch),
            Command::action("Git: Rebase Branch…", GitRebaseBranch),
            Command::action("Git: Continue Rebase", GitContinueRebase),
            Command::action("Git: Abort Rebase", GitAbortRebase),
            Command::action("Git: Add Worktree…", GitAddWorktree),
            Command::action("Git: Open Worktree…", GitOpenWorktree),
            Command::action("Git: Delete Branch…", GitDeleteBranch),
            Command::action("Git: Rename Branch…", GitRenameBranch),
            Command::action("Git: Publish Branch", GitPublishBranch),
            Command::action("Git: Abort Merge", GitAbortMerge),
            Command::action("Git: Add Remote…", GitAddRemote),
            Command::action("Git: Remove Remote…", GitRemoveRemote),
            Command::action("Git: Create Tag…", GitCreateTag),
            Command::action("Git: Pull", GitPull),
            Command::action("Git: Push", GitPush),
            Command::action("Git: Fetch", GitFetch),
            Command::action("Git: Sync", GitSync),
            // --- git-sync ---
            Command::action("Git: Pull (Rebase)", GitPullRebase),
            Command::action("Git: Pull from…", GitPullFrom),
            Command::action("Git: Push to…", GitPushTo),
            Command::action("Git: Fetch (All Remotes)", GitFetchAll),
            Command::action("Git: Commit Signed Off", GitCommitSignedOff),
            Command::action("Git: Commit (No Verify)", GitCommitNoVerify),
            Command::action("Git: Commit & Push", GitCommitAndPush),
            Command::action("Git: Commit & Sync", GitCommitAndSync),
            Command::action("Developer: Restart Language Servers", RestartLanguageServers),
            Command::action("Git: View File History", GitFileHistory),
            Command::action("Git: Blame Line", GitBlameLine),
            Command::action("Git: Commit (Amend)", GitCommitAmend),
            Command::action("Git: Undo Last Commit", GitUndoLastCommit),
            Command::action("Git: Discard All Changes", GitDiscardAll),
            Command::action("Git: Stash", GitStash),
            Command::action("Git: Stash (Include Untracked)", GitStashIncludeUntracked),
            Command::action("Git: Pop Latest Stash", GitPopLatestStash),
            Command::action("Git: Pop Stash…", GitPopStash),
            Command::action("Git: Apply Stash…", GitApplyStash),
            Command::action("Git: Drop Stash…", GitDropStash),
            // --- git-refs ---
            Command::action("Git: Delete Tag…", GitDeleteTag),
            Command::action("Git: Delete Remote Tag…", GitDeleteRemoteTag),
            Command::action("Git: Push Tags", GitPushTags),
            Command::action("Git: Push Tag…", GitPushTag),
            Command::action("Git: Checkout (Detached)…", GitCheckoutDetached),
            Command::action("Git: Create Branch From…", GitCreateBranchFrom),
            Command::action("Git: Delete Remote Branch…", GitDeleteRemoteBranch),
            Command::action("Git: Set Upstream Branch…", GitSetUpstream),
            Command::action("Git: Stash Staged Changes", GitStashStaged),
            Command::action("Git: Stash with Name…", GitStashNamed),
            Command::action("Git: Show Stash…", GitShowStash),
            Command::action("Git: Open Changes", GitOpenChanges),
            Command::action("Git: Open File (HEAD)", GitOpenFileHead),
            Command::action("Git: Compare with Branch…", GitCompareBranch),
            Command::action("Git: Compare with Commit…", GitCompareCommit),
            Command::action("Git: Add to .gitignore", GitAddToGitignore),
            Command::action("Git: Rename Remote…", GitRenameRemote),
            Command::action("Git: Edit Remote URL…", GitSetRemoteUrl),
            Command::action("Git: Remove Worktree…", GitRemoveWorktree),
            Command::action("Git: Prune Worktrees", GitPruneWorktrees),
            Command::action("Terminal: New", NewTerminal),
            Command::action("Terminal: Toggle", ToggleTerminal),
            Command::action("Terminal: Close", KillTerminal),
            Command::action("Terminal: Split", SplitTerminal),
            Command::action("View: Toggle Panel", TogglePanel),
            Command::action("View: Resources", crate::resource_monitor::ToggleResources),
            Command::action("View: Move Panel (bottom ⇄ right)", TogglePanelPosition),
            Command::action("View: Toggle Maximized Panel", ToggleMaximizedPanel),
        ];
        for h in self.remote.ssh_hosts.iter().filter(|h| h.alias != "github.com" && h.alias != "gitlab.com") {
            cmds.push(Command { label: format!("Remote: Connect to {}", h.alias), run: Run::Connect(h.alias.clone()) });
        }
        if matches!(self.remote.conn, Conn::Connected { .. }) {
            cmds.push(Command { label: "Remote: Disconnect".into(), run: Run::Disconnect });
        }
        cmds.push(Command::action("App: Quit", Quit));
        cmds
    }

    /// Palette: "" files, ">" commands, ":" line, "#" symbols in the project.
    pub(super) fn open_palette(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Overlay { kind: OverlayKind::Palette { .. }, .. }) = &self.overlay {
            return;
        }
        if self.overlay.is_some() {
            self.close_overlay(window, cx);
        }
        // Recently used commands go first (like VS Code); the rest keep their order.
        let mut commands = self.commands();
        if !cx.global::<Settings>().docker_enabled {
            commands.retain(|c| !docker_glue::is_command(&c.label));
        }
        if !cx.global::<Settings>().web_preview_enabled {
            commands.retain(|c| !c.label.starts_with("Web Preview:"));
        }
        let rank = |label: &str| self.recent_commands.iter().position(|r| r == label).unwrap_or(usize::MAX);
        commands.sort_by_key(|c| rank(&c.label));
        // Labels are shown in the UI language; search also matches the English name, identifiers (recents, tests) stay English.
        let shown: Vec<String> = commands.iter().map(Self::command_title).collect();
        let aliases: Vec<SharedString> =
            commands.iter().zip(&shown).map(|(c, s)| if *s == c.label { SharedString::default() } else { c.label.clone().into() }).collect();
        let items = commands
            .iter()
            .zip(shown)
            .map(|(c, label)| PickItem {
                label: label.into(),
                detail: (rank(&c.label) != usize::MAX).then(|| crate::i18n::t("recently used").to_string().into()),
                keys: c.shortcut(cx).map(Into::into),
            })
            .collect();
        let max_line = match &self.tabs.large {
            Some(v) if self.tabs.large_active => v.read(cx).line_count() as usize,
            _ => self.active_editor().map_or(0, |e| e.read(cx).line_count()),
        };
        let root = self.roots.primary.read(cx).root().map(str::to_string);
        let placeholder = if root.is_some() {
            crate::i18n::t("Search files by name (> for commands, : for line, # for symbols)")
        } else {
            crate::i18n::t("Type > for commands")
        };
        let sources = vec![
            PickSource::paths(""),
            PickSource::List { prefix: ">", items },
            PickSource::Line { prefix: ":", max: max_line },
            PickSource::Remote { prefix: "#", items: Vec::new(), status: SYMBOL_HINT.into() },
        ];
        let picker = cx.new(|cx| Picker::new(sources, placeholder, query, None, cx).with_aliases(1, aliases));

        match &root {
            None => picker.update(cx, |p, cx| p.set_paths(0, Vec::new(), false, cx)),
            Some(root) => {
                // Cached list first (instant), then a fresh one in the background.
                if let Some(ix) = self.roots.file_index.as_ref().filter(|ix| ix.root == *root) {
                    let (ordered, recent) = self.mru_first(root, &ix.paths, cx);
                    let truncated = ix.truncated;
                    picker.update(cx, |p, cx| {
                        p.set_recent(recent);
                        p.set_paths(0, ordered, truncated, cx)
                    });
                }
                let host = self.remote.host.clone();
                let r = root.clone();
                let extras = self.roots.extra_roots.clone();
                // Files of extra folders are listed as `folder › path`.
                let task = cx.background_executor().spawn(async move {
                    let mut list = host.list_files(&r, FILE_LIMIT)?;
                    for e in &extras {
                        if let Ok(more) = host.list_files(e, FILE_LIMIT) {
                            let label = ui::file_name(e).to_string();
                            list.paths.extend(more.paths.into_iter().map(|p| format!("{label}{ROOT_SEP}{p}")));
                            list.truncated |= more.truncated;
                        }
                    }
                    Ok::<_, host::HostError>(list)
                });
                let weak_picker = picker.downgrade();
                let root = root.clone();
                cx.spawn(async move |this, cx| {
                    let result = task.await;
                    this.update(cx, |this, cx| match result {
                        Ok(list) => {
                            let (ordered, recent) = this.mru_first(&root, &list.paths, cx);
                            this.roots.file_index = Some(FileIndex { root, paths: list.paths, truncated: list.truncated });
                            weak_picker
                                .update(cx, |p, cx| {
                                    p.set_recent(recent);
                                    p.set_paths(0, ordered, list.truncated, cx)
                                })
                                .ok();
                        }
                        Err(e) => {
                            this.notice = Some(Notice::Error(crate::i18n::tf("Could not list files: {}", &[&e]).into()));
                            weak_picker.update(cx, |p, cx| p.set_paths(0, Vec::new(), false, cx)).ok();
                            cx.notify();
                        }
                    })
                    .ok();
                })
                .detach();
            }
        }
        self.show_overlay(picker, OverlayKind::Palette { root, commands, symbols: Vec::new(), compare: false }, window, cx);
    }

    /// Recently opened files first, then the rest in folder order.
    pub(super) fn mru_first(&self, root: &str, paths: &[String], cx: &App) -> (Vec<String>, usize) {
        let excludes = cx.global::<Settings>().files_exclude.clone();
        let paths: Vec<String> = paths.iter().filter(|p| !crate::globs::is_excluded(&excludes, p)).cloned().collect();
        let paths = &paths;
        let prefix = format!("{}/", root.trim_end_matches('/'));
        let recent: Vec<String> = self
            .nav
            .recent_files
            .iter()
            .filter_map(|p| match p.strip_prefix(&prefix) {
                Some(rel) => Some(rel.to_string()),
                // Last file in an extra folder: `folder › path`.
                None => self
                    .roots
                    .extra_roots
                    .iter()
                    .find_map(|e| p.strip_prefix(&format!("{}/", e.trim_end_matches('/'))).map(|rel| format!("{}{ROOT_SEP}{rel}", ui::file_name(e)))),
            })
            .collect();
        let present: std::collections::HashSet<&str> = paths.iter().map(String::as_str).collect();
        let mut out: Vec<String> = recent.iter().filter(|r| present.contains(r.as_str())).cloned().collect();
        let recent_count = out.len();
        let seen: std::collections::HashSet<String> = out.iter().cloned().collect();
        out.extend(paths.iter().filter(|p| !seen.contains(*p)).cloned());
        (out, recent_count)
    }

    pub(super) fn on_overlay_event(&mut self, event: &PickerEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ov) = &self.overlay else { return };
        if matches!(ov.kind, OverlayKind::RemoteFolder { .. }) && self.on_folder_event(event, window, cx) {
            return;
        }
        // --- data ---
        if self.on_data_overlay_event(event, window, cx) {
            return;
        }
        let Some(ov) = &self.overlay else { return };
        match (&ov.kind, event) {
            (OverlayKind::Theme { ids, .. }, PickerEvent::Preview(Pick::Item { index, .. })) => {
                let id = ids[*index].clone();
                self.apply_theme(&id, window, cx);
            }
            (OverlayKind::Theme { ids, .. }, PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let id = ids[*index].clone();
                self.theme_choice = id.clone();
                self.persist_theme(&id);
                self.apply_theme(&id, window, cx);
                self.close_overlay(window, cx);
                self.warn_low_contrast(window, cx);
            }
            (OverlayKind::Theme { original, .. }, PickerEvent::Dismiss) => {
                let original = original.clone();
                self.apply_theme(&original, window, cx);
                self.close_overlay(window, cx);
            }
            (OverlayKind::Snippets(list), PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let body = list[*index].body.clone();
                self.close_overlay(window, cx);
                if let Some(ed) = self.active_editor().cloned() {
                    ed.update(cx, |e, cx| e.insert_snippet(&body, cx));
                    window.focus(&ed.focus_handle(cx));
                }
            }
            (OverlayKind::IconTheme { .. }, PickerEvent::Preview(Pick::Item { index, .. })) => {
                crate::icon_theme::set(crate::icon_theme::ALL[*index]);
                cx.notify();
            }
            (OverlayKind::NewExtensionKind, PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let kind = super::cyx_glue::KINDS[*index];
                self.close_overlay(window, cx);
                self.show_input(
                    InputPurpose::NewExtension { kind },
                    crate::i18n::t("Create"),
                    crate::i18n::t("Name of the new extension, like my-theme"),
                    "",
                    window,
                    cx,
                );
            }
            (OverlayKind::NewExtensionKind, PickerEvent::Dismiss) => {
                self.close_overlay(window, cx);
            }
            (OverlayKind::IconTheme { .. }, PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let t = crate::icon_theme::ALL[*index];
                self.close_overlay(window, cx);
                self.write_setting("workbench.iconTheme", serde_json::Value::String(t.id().into()), window, cx);
            }
            (OverlayKind::IconTheme { original }, PickerEvent::Dismiss) => {
                crate::icon_theme::set(*original);
                self.close_overlay(window, cx);
            }
            (OverlayKind::Ask { .. }, PickerEvent::Dismiss) => self.finish_ask(None, window, cx),
            (OverlayKind::Ask { .. }, PickerEvent::Confirm(Pick::Input)) => {
                let secret = ov.picker.read(cx).query().to_string();
                self.finish_ask(Some(secret), window, cx);
            }
            (OverlayKind::Ask { kind, .. }, PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let answer = match (kind, index) {
                    (AskKind::HostKey, 0) => Some("yes".to_string()),
                    (AskKind::HostKey, _) => Some("no".to_string()),
                    (_, 0) => Some(String::new()),
                    _ => None,
                };
                self.finish_ask(answer, window, cx);
            }
            (OverlayKind::Locations(locs), PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let (path, line, col) = locs[*index].clone();
                self.close_overlay(window, cx);
                self.open_location(path, line, col, window, cx);
            }
            (OverlayKind::Symbols(pos), PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let (line, col) = pos[*index];
                self.close_overlay(window, cx);
                self.record_nav(cx);
                if let Some(ed) = self.active_editor().cloned() {
                    ed.update(cx, |e, cx| e.reveal_lsp(line, col, window, cx));
                }
            }
            (OverlayKind::CodeActions { actions, client }, PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let (action, client) = (actions[*index].clone(), client.clone());
                self.close_overlay(window, cx);
                self.run_code_action(client, action, cx);
            }
            (OverlayKind::Projects(choices), PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let choice = choices[*index].clone();
                self.close_overlay(window, cx);
                self.pick_switch(choice, window, cx);
            }
            (OverlayKind::ForgetRecent(list), PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let r = list[*index].clone();
                self.close_overlay(window, cx);
                self.forget_recent(r, cx);
            }
            (OverlayKind::LspQuestion(_), PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let index = *index;
                if let Some(Overlay { kind: OverlayKind::LspQuestion(reply), .. }) = self.close_overlay(window, cx) {
                    reply.answer(index);
                }
            }
            (OverlayKind::FileHistory { root, log }, PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let (root, entry) = (root.clone(), log[*index].clone());
                self.close_overlay(window, cx);
                self.open_commit_diff(root, entry, window, cx);
            }
            (OverlayKind::Switcher(order), PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let ix = order[*index];
                self.close_overlay(window, cx);
                self.tabs.focused_left = true;
                self.activate(ix, window, cx);
            }
            (OverlayKind::CommitFiles { root, commit, files }, PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let (root, commit) = (root.clone(), commit.clone());
                let (_, path, old) = files[*index].clone();
                self.close_overlay(window, cx);
                let entry =
                    crate::git::LogEntry { hash: commit.hash, short: commit.short, author: commit.author, when: commit.when, subject: commit.subject, path };
                self.open_commit_diff_from(root, entry, old, window, cx);
            }
            (OverlayKind::Settings, PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let index = *index;
                self.close_overlay(window, cx);
                self.pick_setting(index, window, cx);
            }
            (OverlayKind::SettingChoice { key, options }, PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let (key, value) = (*key, options[*index]);
                self.close_overlay(window, cx);
                self.write_setting(key, serde_json::Value::String(value.into()), window, cx);
            }
            (OverlayKind::Shortcuts, PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let index = *index;
                self.close_overlay(window, cx);
                self.bind_shortcut(index, window, cx);
            }
            (OverlayKind::Stashes { refs, op }, PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let (stash, op) = (refs[*index].clone(), *op);
                self.close_overlay(window, cx);
                // --- git-refs ---
                if matches!(op, git_glue::StashOp::Show) {
                    self.show_stash_files(stash, window, cx);
                    return;
                }
                self.with_scm(cx, |s, cx| match op {
                    git_glue::StashOp::Pop => s.stash_apply(stash, true, cx),
                    git_glue::StashOp::Apply => s.stash_apply(stash, false, cx),
                    git_glue::StashOp::Drop => s.stash_drop(stash, cx),
                    git_glue::StashOp::Show => {}
                });
            }
            (OverlayKind::Tasks(list), PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let task = list[*index].clone();
                self.close_overlay(window, cx);
                self.run_task(task, window, cx);
            }
            (OverlayKind::ExceptionFilters(list), PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let id = list[*index].0.clone();
                self.close_overlay(window, cx);
                self.toggle_exception_filter(id, cx);
            }
            (OverlayKind::RemoveFolder(list), PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let path = list[*index].clone();
                self.close_overlay(window, cx);
                self.remove_workspace_folder(&path, cx);
            }
            (OverlayKind::DebugConfigs(list), PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let config = list[*index].clone();
                self.close_overlay(window, cx);
                self.launch_with_inputs(config, window, cx);
            }
            (OverlayKind::LaunchInput(options), PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let value = options[*index].clone();
                self.close_overlay(window, cx);
                self.answer_launch_input(value, window, cx);
            }
            (OverlayKind::TerminalLaunch(list), PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let launch = list[*index].clone();
                self.close_overlay(window, cx);
                let cwd = self.active_root(cx);
                self.new_terminal_launch(cwd, launch, window, cx);
            }
            (OverlayKind::TerminalDefaultProfile(list), PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let name = list[*index].clone();
                self.close_overlay(window, cx);
                self.write_setting("terminal.defaultProfile", serde_json::json!(name), window, cx);
            }
            (OverlayKind::TerminalColor, PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let color = crate::terminal_profiles::COLORS[*index];
                self.close_overlay(window, cx);
                self.set_terminal_color(color, cx);
            }
            (OverlayKind::LaunchTemplates(list), PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let snippet = list[*index].clone();
                self.close_overlay(window, cx);
                self.insert_debug_configuration(snippet, window, cx);
            }
            (OverlayKind::Favorites(list), PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let rel = list[*index].clone();
                self.close_overlay(window, cx);
                self.open_favorite(rel, window, cx);
            }
            (OverlayKind::EditorOption(opts), PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let opt = opts[*index];
                self.close_overlay(window, cx);
                if let EditorOpt::Associate(i) = opt {
                    self.associate_extension(i, window, cx);
                }
                if let EditorOpt::OpenWith(kind) = opt {
                    match kind {
                        OpenWithKind::Hex => self.open_as_hex(window, cx),
                        OpenWithKind::MarkdownPreview => self.open_markdown_preview(&OpenMarkdownPreview, window, cx),
                        OpenWithKind::CompareSaved => self.compare_with_saved(&CompareWithSaved, window, cx),
                    }
                    return;
                }
                if let Some(ed) = self.active_editor().cloned() {
                    ed.update(cx, |e, cx| match opt {
                        EditorOpt::OpenWith(_) => {}
                        EditorOpt::Language(ext) => e.set_language(ext, cx),
                        EditorOpt::Associate(_) => {}
                        EditorOpt::Eol { crlf } => e.set_line_ending(crlf, cx),
                        EditorOpt::Indent(n) => e.set_indent(n, cx),
                        EditorOpt::Convert { to_tabs } => e.convert_indentation(to_tabs, cx),
                    });
                }
            }
            (OverlayKind::Branches { root, choices }, PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let (root, choice) = (root.clone(), choices[*index].clone());
                self.close_overlay(window, cx);
                self.pick_branch(root, choice, window, cx);
            }
            (OverlayKind::RemoteTargets(aliases), PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let alias = aliases[*index].clone();
                self.close_overlay(window, cx);
                self.connect(alias, window, cx);
            }
            (OverlayKind::DockerContexts { contexts, action }, PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let (context, action) = (contexts[*index].clone(), action.clone());
                self.close_overlay(window, cx);
                self.docker_context_picked(context, action, window, cx);
            }
            (OverlayKind::ContainerEngines { choices, action }, PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let (choice, action) = (choices[*index].clone(), action.clone());
                self.close_overlay(window, cx);
                self.container_engine_picked(choice, action, window, cx);
            }
            (OverlayKind::DockerContainers { context, containers, services }, PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let (context, container, services) = (context.clone(), containers[*index].clone(), *services);
                self.close_overlay(window, cx);
                self.docker_container_picked(context, container, services, window, cx);
            }
            (OverlayKind::DockerPlan { project, context, rebuild }, PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let (project, context, rebuild, index) = (project.clone(), context.clone(), *rebuild, *index);
                self.close_overlay(window, cx);
                if index == 0 {
                    self.docker_build(project, context, rebuild, window, cx);
                }
            }
            (OverlayKind::SpellFix { editor, range, word, choices }, PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let (editor, range, word, choice) = (editor.clone(), range.clone(), word.clone(), choices[*index].clone());
                self.close_overlay(window, cx);
                self.apply_spell_choice(editor, range, word, choice, cx);
            }
            (OverlayKind::PullRequests { remote, pulls }, PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let (remote, pull) = (remote.clone(), pulls[*index].clone());
                self.close_overlay(window, cx);
                self.pick_pull_request(remote, pull, window, cx);
            }
            (OverlayKind::PullRequestActions { remote, pull, choices, .. }, PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let (remote, pull, action) = (remote.clone(), pull.clone(), choices[*index]);
                self.close_overlay(window, cx);
                self.run_pull_action(remote, pull, action, cx);
            }
            (OverlayKind::Dictionaries, PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let index = *index;
                self.close_overlay(window, cx);
                self.run_install_dictionary(index, window, cx);
            }
            (OverlayKind::HostMenu(choices), PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let choice = choices[*index];
                self.close_overlay(window, cx);
                self.run_host_menu(choice, window, cx);
            }
            (OverlayKind::HostField { alias, fields }, PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let (alias, field) = (alias.clone(), fields[*index]);
                self.close_overlay(window, cx);
                self.edit_host_field(alias, field, window, cx);
            }
            (OverlayKind::HostOp { op, aliases }, PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let (op, alias) = (*op, aliases[*index].clone());
                self.close_overlay(window, cx);
                self.run_host_op(op, alias, window, cx);
            }
            // --- git-sync ---
            (OverlayKind::SyncTarget(ops), PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let op = ops[*index].clone();
                self.close_overlay(window, cx);
                self.git_sync(vec![op], cx);
            }
            (OverlayKind::BranchOp { root, op, names }, PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let (root, op, name) = (root.clone(), *op, names[*index].clone());
                self.close_overlay(window, cx);
                self.run_branch_op(root, op, name, window, cx);
            }
            (
                OverlayKind::Locations(_)
                | OverlayKind::Symbols(_)
                | OverlayKind::CodeActions { .. }
                | OverlayKind::Branches { .. }
                | OverlayKind::BranchOp { .. }
                | OverlayKind::SyncTarget(_)
                | OverlayKind::RemoteTargets(_)
                | OverlayKind::DockerContexts { .. }
                | OverlayKind::ContainerEngines { .. }
                | OverlayKind::DockerContainers { .. }
                | OverlayKind::DockerPlan { .. }
                | OverlayKind::HostOp { .. }
                | OverlayKind::HostField { .. }
                | OverlayKind::HostMenu(_)
                | OverlayKind::SpellFix { .. }
                | OverlayKind::Dictionaries
                | OverlayKind::PullRequests { .. }
                | OverlayKind::PullRequestActions { .. }
                | OverlayKind::EditorOption(_)
                | OverlayKind::Projects(_)
                | OverlayKind::ForgetRecent(_)
                | OverlayKind::Favorites(_)
                | OverlayKind::Tasks(_)
                | OverlayKind::DebugConfigs(_)
                | OverlayKind::LaunchInput(_)
                | OverlayKind::LaunchTemplates(_)
                | OverlayKind::TerminalLaunch(_)
                | OverlayKind::TerminalDefaultProfile(_)
                | OverlayKind::TerminalColor
                | OverlayKind::RemoveFolder(_)
                | OverlayKind::CompareFolder { .. }
                | OverlayKind::ExceptionFilters(_)
                | OverlayKind::Stashes { .. }
                | OverlayKind::Settings
                | OverlayKind::FileHistory { .. }
                | OverlayKind::CommitFiles { .. }
                | OverlayKind::Switcher(_)
                | OverlayKind::LspQuestion(_)
                | OverlayKind::SettingChoice { .. }
                | OverlayKind::Shortcuts,
                PickerEvent::Dismiss,
            ) => {
                self.close_overlay(window, cx);
            }
            (OverlayKind::Palette { .. } | OverlayKind::Input(_), PickerEvent::Dismiss) => {
                self.close_overlay(window, cx);
            }
            // --- compare ---
            (OverlayKind::CompareFolder { .. }, PickerEvent::Confirm(Pick::Item { index, .. })) => {
                let index = *index;
                self.on_compare_pick(index, window, cx);
            }
            (OverlayKind::Input(_), PickerEvent::Confirm(Pick::Input)) => {
                let name = ov.picker.read(cx).query().trim().to_string();
                if let Some(Overlay { kind: OverlayKind::Input(purpose), .. }) = self.close_overlay(window, cx) {
                    self.finish_input(purpose, name, window, cx);
                }
            }
            (OverlayKind::Palette { root, compare, .. }, PickerEvent::Confirm(Pick::Item { source: 0, index })) => {
                let rel = ov.picker.read(cx).path_at(0, *index);
                let goto = ov.picker.read(cx).goto();
                let (root, compare) = (root.clone(), *compare);
                self.close_overlay(window, cx);
                if let (Some(root), Some(rel)) = (root, rel) {
                    let path = self.palette_path(&root, &rel);
                    if compare {
                        return self.compare_active_with(path, window, cx);
                    }
                    if let Some((line, col)) = goto {
                        self.tabs.pending_reveal = Some((path.clone(), line as u32, col.unwrap_or(1).saturating_sub(1) as u32, 0));
                    }
                    self.open_file(path, window, cx);
                }
            }
            (OverlayKind::Palette { commands, .. }, PickerEvent::Confirm(Pick::Item { source: 1, index })) => {
                let (run, label) = (commands[*index].run.clone(), commands[*index].label.clone());
                self.remember_command(label);
                self.close_overlay(window, cx);
                self.run_command(run, window, cx);
            }
            (OverlayKind::Palette { .. }, PickerEvent::Query(q)) => {
                let q = q.clone();
                self.search_workspace_symbols(q, cx);
            }
            (OverlayKind::Palette { symbols, .. }, PickerEvent::Confirm(Pick::Item { source: 3, index })) => {
                let (path, line, col) = symbols[*index].clone();
                self.close_overlay(window, cx);
                self.open_location(path, line, col, window, cx);
            }
            (OverlayKind::Palette { .. }, PickerEvent::Confirm(Pick::Line(n, col))) => {
                let (n, col) = (*n, *col);
                self.close_overlay(window, cx);
                self.record_nav(cx);
                if let Some(v) = self.tabs.large.clone().filter(|_| self.tabs.large_active) {
                    v.update(cx, |v, cx| v.go_to_line(n, window, cx));
                } else if let Some(ed) = self.active_editor().cloned() {
                    ed.update(cx, |e, cx| match col {
                        Some(c) => e.reveal(n, c - 1, 0, window, cx),
                        None => e.go_to_line(n, window, cx),
                    });
                }
            }
            _ => {}
        }
    }

    // --- Bottom panel and terminals ---
}
