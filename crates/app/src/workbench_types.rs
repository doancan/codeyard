//! Workbench: the picker layer (`OverlayKind`), input purposes (`InputPurpose`) and context-menu commands (`MenuCmd`).
//! Kept in a separate file so `workbench.rs` doesn't grow; every new feature adds a variant.

use super::*;

pub(super) enum OverlayKind {
    /// Row -> theme id (including "auto"); the theme to revert to on Esc.
    Theme {
        ids: Vec<String>,
        original: String,
    },
    /// "Insert Snippet…": snippets for the active file's language.
    Snippets(Vec<crate::snippets::Snippet>),
    /// File icon theme picker; the theme to revert to on Esc.
    IconTheme {
        original: crate::icon_theme::IconTheme,
    },
    /// Sources: 0 files, 1 commands, 2 line, 3 symbols (`#`, with their locations).
    /// `compare`: selecting a file doesn't open it, it is compared with the active file.
    Palette {
        root: Option<String>,
        commands: Vec<Command>,
        symbols: Vec<(String, u32, u32)>,
        compare: bool,
    },
    /// Name entry (new file/folder, rename).
    Input(InputPurpose),
    /// Language server results: locations (references), symbols, code actions.
    Locations(Vec<(String, u32, u32)>),
    Symbols(Vec<(u32, u32)>),
    CodeActions {
        actions: Vec<crate::lsp::CodeAction>,
        client: Arc<crate::lsp::Client>,
    },
    /// Project switcher (⌃R), remove from recents, favorites.
    Projects(Vec<projects::SwitchChoice>),
    ForgetRecent(Vec<RecentWorkspace>),
    Favorites(Vec<String>),
    /// "Tasks: Run Task…"
    Tasks(Vec<tasks::Task>),
    /// Extra folder to remove from the workspace.
    RemoveFolder(Vec<String>),
    /// Debugger: exception filters (id, name); selecting toggles them.
    ExceptionFilters(Vec<(String, String)>),
    /// F5: multiple debug configurations.
    DebugConfigs(Vec<crate::debug_config::LaunchConfig>),
    Stashes {
        refs: Vec<String>,
        op: git_glue::StashOp,
    },
    FileHistory {
        root: String,
        log: Vec<crate::git::LogEntry>,
    },
    /// Open editors, most recently used first: row -> index into `editors`.
    Switcher(Vec<usize>),
    /// Files of the commit selected in the graph: (status, path, old path).
    CommitFiles {
        root: String,
        commit: crate::graph::GraphCommit,
        files: Vec<(char, String, Option<String>)>,
    },
    LspQuestion(lsp_glue::QuestionReply),
    Settings,
    /// Remote folder picker: the listed directory and the (filtered) names of its subfolders.
    RemoteFolder {
        dir: String,
        names: Vec<String>,
    },
    // --- compare ---
    /// "Compare: Folders…": the folders to choose from, and the left one once it is chosen.
    CompareFolder {
        first: Option<compare_glue::Spot>,
        choices: Vec<compare_glue::CompareChoice>,
    },
    SettingChoice {
        key: &'static str,
        options: &'static [&'static str],
    },
    Shortcuts,
    /// Active editor's setting from the status bar (line ending, indentation).
    EditorOption(Vec<EditorOpt>),
    /// Git branch picker (repo root, rows).
    Branches {
        root: String,
        choices: Vec<git_glue::BranchChoice>,
    },
    /// Merge / Delete / Rename Branch, Remove Remote: act on the selected name.
    BranchOp {
        root: String,
        op: git_glue::BranchOp,
        names: Vec<String>,
    },
    /// "Connect to Dev Container / WSL": alias of the selected target (`docker:name`, `wsl:name`).
    RemoteTargets(Vec<String>),
    /// "Remove SSH Host…" / "Test Connection…": host aliases.
    /// "GitHub: Pull Requests…": open requests and actions for the selected request.
    PullRequests {
        remote: String,
        pulls: Vec<crate::github::Pull>,
    },
    PullRequestActions {
        remote: String,
        pull: crate::github::Pull,
        choices: Vec<pulls_glue::PullAction>,
    },
    /// Spell check: list of corrections for a misspelled word.
    SpellFix {
        editor: gpui::WeakEntity<EditorView>,
        range: std::ops::Range<usize>,
        word: String,
        choices: Vec<spell_glue::SpellChoice>,
    },
    /// "Spelling: Install Dictionary…"
    Dictionaries,
    HostOp {
        op: hosts::HostOp,
        aliases: Vec<String>,
    },
    /// "Manage SSH hosts…": list of operations.
    HostMenu(Vec<hosts::HostMenu>),
    /// "Remote: Edit SSH Host…": the field to edit.
    HostField {
        alias: String,
        fields: Vec<host::ssh_config::Field>,
    },
    /// ssh is asking (password, host key); the answer goes to the askpass bridge.
    Ask {
        kind: AskKind,
        reply: AskReply,
    },
    // --- data ---
    /// "Data: Convert To…": the target of each row, and the text to convert.
    DataConvert(Box<data_glue::ConvertJob>),
    /// "Data: Decode Selection…": read-only lines.
    DataDecode,
}

pub(super) enum InputPurpose {
    Create {
        dir: String,
        is_dir: bool,
    },
    Rename {
        path: String,
    },
    /// Rename a symbol via the language server.
    RenameSymbol {
        client: Arc<crate::lsp::Client>,
        path: String,
        pos: (u32, u32),
    },
    /// ⌘N: path relative to the folder (may be a subfolder).
    NewFile {
        base: String,
    },
    /// ⇧⌘S: write the content to the new path, switch the tab to it.
    SaveAs {
        from: String,
    },
    /// Over SSH, write a copy of the active file to this computer; the tab stays on the remote file.
    SaveLocalCopy {
        from: String,
    },
    /// Git: new branch from the current branch.
    CreateBranch {
        root: String,
    },
    RenameBranch {
        root: String,
        old: String,
    },
    /// "name url"
    AddRemote {
        root: String,
    },
    /// "path branch"
    AddWorktree {
        root: String,
    },
    /// Scratch file name.
    Scratch,
    // --- compare ---
    /// A typed folder path for "Compare: Folders…"; `first` is the left folder once chosen.
    CompareFolder {
        first: Option<compare_glue::Spot>,
    },
    /// Debugger: condition / log message / hit count of the breakpoint on the line.
    EditBreakpoint {
        path: String,
        line: usize,
    },
    /// Debugger: expression to watch.
    AddWatch,
    /// Debugger: variable to break on when its value changes.
    DataBreakpoint,
    /// Save the active colors (including customizations) as a user theme.
    SaveTheme,
    /// "Clone Git Repository…": URL first, then the target folder.
    CloneUrl,
    /// Workspace switcher: connect to the typed host name (or `user@host`).
    ConnectHost,
    /// `user@host[:port] [as name]` -> ~/.ssh/config.
    AddSshHost,
    /// New value of the host setting.
    EditHostField {
        alias: String,
        field: host::ssh_config::Field,
    },
    /// "File: Filter Log Lines…": pattern (regular expression).
    FilterLog {
        path: String,
    },
    CloneTarget {
        url: String,
    },
    /// New branch / tag from a commit in the graph.
    CommitBranch {
        hash: String,
    },
    CommitTag {
        hash: String,
    },
    // --- git-refs ---
    /// Name of a new branch that starts at `start`.
    CreateBranchFrom {
        root: String,
        start: String,
    },
    RenameRemote {
        root: String,
        old: String,
    },
    SetRemoteUrl {
        root: String,
        name: String,
    },
    /// Description of a new stash.
    StashNamed,
    /// Add the open folder to the saved projects under its name.
    SaveProject {
        path: String,
    },
    /// Number or text setting from the settings picker.
    Setting {
        key: &'static str,
    },
}

#[derive(Clone, Copy)]
pub(super) enum MenuCmd {
    NewFile,
    NewFolder,
    Rename,
    Delete,
    CopyPath,
    CopyRelativePath,
    Reveal,
    CloseTab,
    CloseOthers,
    CloseRight,
    CloseSaved,
    TogglePin,
    Favorite,
    CutEntry,
    CopyEntry,
    PasteEntry,
    Duplicate,
    OpenTerminal,
    SelectForCompare,
    CompareWithSelected,
    // --- compare ---
    SelectFolderForCompare,
    CompareFolderWithSelected,
    OpenSide,
    ExpandAllInside,
    CollapseAllInside,
    FindInFolder,
    /// File transfer with the server (remote workspace only).
    Download,
    Upload,
    /// Open the file in the tail view (`tail -f`).
    Follow,
    /// Graph commit menu (`MenuTarget::path` = commit summary).
    CopyHash,
    CopySubject,
    CommitFiles,
    CommitCheckout,
    CommitBranch,
    CommitTag,
    CommitCherryPick,
    CommitRevert,
    CommitReset(crate::graph::ResetMode),
    // --- git-refs ---
    GitIgnore,
    GitOpenChanges,
    GitOpenHead,
    /// Send the action in `MenuState::actions` to the focused item.
    Dispatch(usize),
    AppSubmenu(usize),
}
