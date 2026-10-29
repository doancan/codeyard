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
    /// "Extensions: New Extension…": which kind to scaffold.
    NewExtensionKind,
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
    Input {
        purpose: InputPurpose,
        context: Option<InputContext>,
    },
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
    /// A `pickString` input of a starting configuration: its options.
    LaunchInput(Vec<String>),
    /// "Debug: Add Configuration…": the snippets offered.
    LaunchTemplates(Vec<serde_json::Value>),
    /// "Terminal: New Terminal with Profile…": what each row opens.
    TerminalLaunch {
        context: InputContext,
        launches: Vec<crate::terminal::Launch>,
    },
    /// "Terminal: Select Default Profile…": profile names.
    TerminalDefaultProfile {
        context: InputContext,
        names: Vec<String>,
    },
    /// "Terminal: Change Color…".
    TerminalColor {
        terminal: gpui::EntityId,
    },
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
        host: Arc<dyn Host>,
        root: String,
        choices: Vec<git_glue::BranchChoice>,
    },
    /// Merge / Delete / Rename Branch, Remove Remote: act on the selected name.
    BranchOp {
        host: Arc<dyn Host>,
        root: String,
        op: git_glue::BranchOp,
        names: Vec<String>,
    },
    // --- git-sync ---
    /// "Pull from…" / "Push to…": the step to run for each row.
    SyncTarget {
        host: Arc<dyn host::Host>,
        root: String,
        ops: Vec<crate::git::SyncOp>,
    },
    /// "Connect to Dev Container / WSL": alias of the selected target (`docker:name`, `wsl:name`).
    RemoteTargets(Vec<String>),
    ContainerEngines {
        choices: Vec<crate::container_engine::Choice>,
        action: docker_glue::Action,
    },
    DockerContexts {
        contexts: Vec<crate::docker::DockerContext>,
        action: docker_glue::Action,
    },
    DockerContainers {
        context: crate::docker::DockerContext,
        containers: Vec<crate::docker::Container>,
        services: bool,
    },
    DockerPlan {
        project: crate::docker::Project,
        context: crate::docker::DockerContext,
        rebuild: bool,
    },
    /// "Remove SSH Host…" / "Test Connection…": host aliases.
    /// "GitHub: Pull Requests…": open requests and actions for the selected request.
    PullRequests {
        host: Arc<dyn Host>,
        root: String,
        remote: pulls_glue::PullRemote,
        pulls: Vec<crate::github::Pull>,
    },
    PullRequestActions {
        host: Arc<dyn Host>,
        root: String,
        remote: pulls_glue::PullRemote,
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

#[derive(Clone, Copy)]
pub(super) struct TabTarget {
    pub group: GroupKey,
    pub editor: gpui::EntityId,
}

pub(super) enum InputContext {
    Git { host: Arc<dyn Host>, root: String },
    Workspace { host: Arc<dyn Host>, root: Option<String> },
}

impl InputContext {
    pub(super) fn matches(&self, workbench: &Workbench, cx: &gpui::App) -> bool {
        match self {
            Self::Git { host, root } => workbench.git_context_matches(host, root),
            Self::Workspace { host, root } => Arc::ptr_eq(host, &workbench.remote.host) && workbench.root(cx) == *root,
        }
    }
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
    /// Debugger: function name to break in.
    FunctionBreakpoint,
    /// Debugger: the answer to a `promptString` input of a starting configuration.
    LaunchInput,
    /// New name of the active terminal.
    RenameTerminal {
        terminal: gpui::EntityId,
    },
    /// Debugger: new value of a variable of the scope or object `parent`.
    SetVariable {
        parent: i64,
        name: String,
    },
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
    /// `publisher.name[@version]` to install from a registry.
    InstallFromRegistry,
    /// The name of a new extension of this kind (`cyx new <kind>`).
    NewExtension {
        kind: &'static str,
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
        original: String,
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
    /// Label of the forward of a server port.
    PortLabel {
        remote: u16,
    },
    /// Local port for the forward of a server port.
    PortLocal {
        remote: u16,
    },
}

#[derive(Clone, Copy)]
pub(super) enum MenuCmd {
    ContainerTool(usize),
    Container(super::container_overview_glue::ContainerAction),
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
    ScmFile(crate::scm::FileAction),
    /// A view or filter choice of the GRAPH section's "…" menu.
    Graph(crate::scm::GraphOption),
    /// Send the action in `MenuState::actions` to the focused item.
    Dispatch(usize),
    /// Start the terminal at this index of `Workbench::term_launches` (the New Terminal button's menu).
    TerminalLaunch(usize),
    AppSubmenu(usize),
    /// A row of the PORTS table (`MenuTarget::path` = the server port).
    Port(ports_glue::PortMenuCmd),
    /// A host row of the Remote Explorer (`MenuTarget::path` = the alias).
    Remote(remote_glue::RemoteMenuCmd),
    /// The Run and Debug view: configuration selector, breakpoint rows (`MenuTarget::path` = "path\tline").
    Debug(debug::DebugMenuCmd),
}
