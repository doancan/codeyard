//! Main window (S01): activity bar, sidebar (Explorer / Remote), tabs,
//! editor area, status bar. Host switching (local <-> SSH) is handled here.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, MouseButton, PathPromptOptions, PromptLevel, SharedString, Subscription, Window, actions,
    deferred, div, prelude::*, px,
};
use host::Host;
use host::ssh::{AgentBundle, SshHost, Step};
use state::{RecentWorkspace, State};
use theme::themes::{self as themes, Source, Theme};

use crate::commands::{Command, Run};
use crate::editor::{self, EditorEvent, EditorView, button};
use crate::menu::{ContextMenu, MenuEvent, MenuItem};
use crate::picker::{Pick, PickItem, Picker, PickerEvent, Source as PickSource};
use crate::search::{SearchEvent, SearchPanel};
use crate::sections;
use crate::settings::{self, AutoSave, Settings};
use crate::terminal::{TerminalEvent, TerminalView};
use crate::tree::{FileTree, TreeEvent};
use crate::ui::{self, ActiveTheme, Palette, c, icon, layout, palette, text};

// The accessibility tree is macOS-only for now (AccessKit macOS adapter); unused elsewhere.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[path = "a11y.rs"]
pub mod a11y;
// --- compare ---
#[path = "workbench_compare.rs"]
mod compare_glue;
#[path = "workbench_connect.rs"]
mod connect_glue;
#[path = "workbench_container_browser.rs"]
mod container_browser_glue;
#[path = "workbench_container_detail.rs"]
mod container_detail_glue;
#[path = "workbench_container_logs.rs"]
mod container_logs_glue;
#[path = "workbench_container_monitor.rs"]
mod container_monitor_glue;
#[path = "workbench_container_overview.rs"]
mod container_overview_glue;
#[path = "workbench_docker.rs"]
mod docker_glue;
// --- data ---
#[path = "workbench_ext_accounts.rs"]
mod accounts_glue;
pub(crate) use accounts_glue::{deliver_callback as deliver_extension_callback, forward_callback_from_command_line};
#[path = "workbench_ext_code.rs"]
mod code_glue;
#[path = "workbench_data.rs"]
mod data_glue;
#[path = "workbench_debug.rs"]
mod debug;
#[cfg(test)]
#[path = "editor_grid_tests.rs"]
mod editor_grid_tests;
#[path = "workbench_editors.rs"]
mod editors_glue;
#[path = "workbench_ext.rs"]
mod ext_glue;
#[path = "ext_ui_render.rs"]
mod ext_ui_glue;
#[path = "workbench_ext_view.rs"]
mod ext_view_glue;
#[path = "workbench_ext_views.rs"]
mod ext_views_glue;
#[path = "workbench_extras.rs"]
mod extras_glue;
#[path = "workbench_file_batch.rs"]
mod file_batch_glue;
#[path = "workbench_folder.rs"]
mod folder_glue;
#[path = "workbench_fsops.rs"]
mod fsops_glue;
#[path = "workbench_git.rs"]
mod git_glue;
#[path = "workbench_groups.rs"]
mod groups_glue;
#[path = "workbench_ext_inspector.rs"]
mod inspector_glue;
#[path = "workbench_market.rs"]
mod market_glue;
#[path = "workbench_market_view.rs"]
mod market_view_glue;
#[path = "workbench_ext_providers.rs"]
mod providers_glue;
#[path = "workbench_repos.rs"]
pub(crate) mod repos_glue;
#[path = "workbench_ext_updates.rs"]
mod updates_glue;

use groups_glue::{Group, GroupKey, GroupResize};
#[path = "workbench_hosts.rs"]
mod hosts;
#[path = "workbench_types.rs"]
mod types;
use types::{InputPurpose, MenuCmd, OverlayKind};
#[path = "workbench_app_menu.rs"]
mod app_menu_glue;
#[path = "workbench_http.rs"]
mod http_glue;
#[path = "workbench_lsp.rs"]
mod lsp_glue;
#[path = "workbench_menus.rs"]
mod menus_glue;
#[path = "workbench_nav.rs"]
mod nav_glue;
#[path = "workbench_palette.rs"]
mod palette_glue;
#[path = "workbench_panels.rs"]
mod panels_glue;
#[cfg(test)]
pub use panels_glue::ProblemRow;
#[cfg(test)]
pub(crate) use panels_glue::interface_scale_notice;
#[path = "workbench_cyx.rs"]
mod cyx_glue;
#[path = "workbench_ports.rs"]
mod ports_glue;
#[path = "workbench_projects.rs"]
pub mod projects;
#[path = "workbench_pulls.rs"]
mod pulls_glue;
#[path = "workbench_remote.rs"]
mod remote_glue;
#[path = "workbench_row_nav.rs"]
mod row_nav_glue;
pub use row_nav_glue::{ROW_LIST, RowActivate, RowFirst, RowLast, RowNext, RowPrevious};
#[path = "workbench_rust.rs"]
mod rust_glue;
// --- git-refs ---
#[path = "workbench_recover.rs"]
mod recover_glue;
#[path = "workbench_refs.rs"]
mod refs_glue;
#[path = "workbench_render.rs"]
mod render_glue;
#[path = "workbench_replace.rs"]
mod replace_glue;
// --- structural ---
#[path = "workbench_schema.rs"]
mod schema_glue;
#[path = "workbench_search_editor.rs"]
mod search_editor_glue;
#[path = "workbench_session.rs"]
mod session_glue;
#[path = "workbench_settings.rs"]
pub mod settings_ui;
#[path = "workbench_spell.rs"]
mod spell_glue;
// --- git-sync ---
#[path = "workbench_state.rs"]
mod state_glue;
#[path = "workbench_structural.rs"]
mod structural_glue;
#[path = "workbench_sync.rs"]
mod sync_glue;
#[path = "workbench_task_run.rs"]
mod task_run_glue;
#[path = "workbench_tasks.rs"]
pub mod tasks;
#[path = "workbench_terminals.rs"]
mod terminals_glue;
#[cfg(test)]
#[path = "workbench_testing.rs"]
mod testing_glue;
#[path = "workbench_test_explorer.rs"]
mod tests_glue;
#[cfg(test)]
pub(crate) use testing_glue::DialogView;
#[path = "workbench_theme.rs"]
mod theme_glue;
#[path = "workbench_transfer.rs"]
mod transfer;
#[path = "workbench_update.rs"]
mod update_glue;
#[path = "workbench_update_window.rs"]
mod update_window_glue;
#[path = "workbench_vcs.rs"]
mod vcs_glue;
#[path = "workbench_view_editor.rs"]
mod view_editor_glue;
#[path = "workbench_view_explorer.rs"]
mod view_explorer_glue;
#[path = "workbench_view_status.rs"]
mod view_status_glue;
#[path = "workbench_view_update.rs"]
mod view_update_glue;
#[path = "workbench_watch.rs"]
mod watch_glue;
#[path = "workbench_web_preview.rs"]
mod web_preview_glue;
#[path = "workbench_welcome.rs"]
pub(crate) mod welcome_glue;
#[path = "workbench_workspace.rs"]
mod workspace_glue;

/// Project settings file (root-relative).
const PROJECT_SETTINGS: &str = ".codeyard/settings.jsonc";
/// Project settings left under the old name (codeapp): the new one wins if present.
const LEGACY_PROJECT_SETTINGS: &str = ".codeapp/settings.jsonc";
const PROJECT_SETTINGS_TEMPLATE: &str = "// Settings for this project only. They override your user settings.\n// Any key from Preferences: Open Settings works here.\n{\n  // \"editor.tabSize\": 2,\n}\n";

/// Length of the recent folders list (welcome screen, project switcher).
const RECENT_LIMIT: usize = 30;

actions!(
    workbench,
    [
        OpenFolder,
        CancelFileOperation,
        ToggleSidebar,
        ShowExplorer,
        ShowRemote,
        CloseEditor,
        Quit,
        SelectTheme,
        QuickOpen,
        CommandPalette,
        GoToLine,
        TogglePanel,
        ToggleTerminal,
        NewTerminal,
        KillTerminal,
        FindInFiles,
        OpenSearchInEditor,
        UndoSearchReplace,
        OpenAsHex,
        OpenSettings,
        OpenKeybindings,
        ImportVsCode,
        ShowScm,
        ShowProblems,
        ShowOutput,
        ClearOutput,
        ShowPorts,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        RenameSymbol,
        FindReferences,
        GoToSymbol,
        GoToWorkspaceSymbol,
        CodeActions,
        FormatDocument,
        FormatSelection,
        SaveAll,
        SplitEditor,
        SplitEditorLeft,
        SplitEditorUp,
        SplitEditorDown,
        CloseEmptyEditorGroup,
        MoveEditorToOtherGroup,
        ToggleWordWrap,
        CloseSavedEditors,
        ConvertIndentToSpaces,
        ConvertIndentToTabs,
        ToggleMinimap,
        ToggleStickyScroll,
        ToggleRenderWhitespace,
        GoBack,
        GoForward,
        ReopenClosedEditor,
        NewFile,
        SaveAs,
        SaveLocalCopy,
        RevealActiveFile,
        CollapseFolders,
        ChangeEol,
        ChangeLanguage,
        OpenWith,
        ChangeIndentation,
        NewWindow,
        CloseAllEditors,
        CloseOtherEditors,
        NextEditor,
        PreviousEditor,
        KeepEditor,
        CopyActivePath,
        CopyActiveRelativePath,
        CompareActiveWith,
        NewFolder,
        RefreshExplorer,
        CloseFolder,
        ClearTerminal,
        SplitTerminal,
        OpenWorkspaceSettings,
        SwitchProject,
        SaveProject,
        EditProjects,
        ForgetRecent,
        TrustWorkspace,
        ToggleFavorite,
        ShowFavorites,
        ListBookmarks,
        RunTask,
        RunBuildTask,
        RerunLastTask,
        TerminateTask,
        RestartRunningTask,
        RunTestAtCursor,
        RunFileTests,
        StartDebugging,
        StopDebugging,
        RestartDebugging,
        RemoveAllBreakpoints,
        EditBreakpoint,
        AddWatch,
        RemoveAllWatches,
        BreakOnExceptions,
        AddDataBreakpoint,
        RemoveDataBreakpoints,
        OpenWorkspaceFile,
        SaveWorkspace,
        SaveWorkspaceAs,
        ReloadWorkspace,
        OpenActiveWorkspaceFile,
        AddFolderInNewWindow,
        AddFolderToWorkspace,
        RemoveFolderFromWorkspace,
        InstallJsDebug,
        DebugContinue,
        DebugPause,
        DebugStepOver,
        DebugStepInto,
        DebugStepOut,
        ShowDebugPanel,
        ConfigureTasks,
        TogglePanelPosition,
        ClearSavedTerminalHistory,
        ToggleMaximizedPanel,
        GitCheckout,
        GitCreateBranch,
        RemoteDocker,
        DockerOpenProject,
        DockerRebuild,
        DockerServices,
        DockerCancel,
        ShowContainers,
        OpenWebPreview,
        CopyWebPreviewUrl,
        StopWebPreview,
        WebPreviewLog,
        RemoteWsl,
        GitMergeBranch,
        GitRebaseBranch,
        GitContinueRebase,
        GitAbortRebase,
        GitAddWorktree,
        GitOpenWorktree,
        GitDeleteBranch,
        GitRenameBranch,
        GitPublishBranch,
        GitAbortMerge,
        GitAddRemote,
        GitRemoveRemote,
        GitCreateTag,
        GitPull,
        GitPush,
        GitFetch,
        GitSync,
        // --- git-sync ---
        GitPullRebase,
        GitPullFrom,
        GitPushTo,
        GitFetchAll,
        GitCommitSignedOff,
        GitCommitNoVerify,
        GitCommitAndPush,
        GitCommitAndSync,
        FocusLeftGroup,
        FocusRightGroup,
        FocusUpGroup,
        FocusDownGroup,
        GitCommitAmend,
        GitUndoLastCommit,
        GitStash,
        GitStashIncludeUntracked,
        GitPopLatestStash,
        GitPopStash,
        GitApplyStash,
        GitDropStash,
        // --- git-refs ---
        GitDeleteTag,
        GitDeleteRemoteTag,
        GitPushTags,
        GitPushTag,
        GitCheckoutDetached,
        GitCreateBranchFrom,
        GitDeleteRemoteBranch,
        GitSetUpstream,
        GitStashStaged,
        GitStashNamed,
        GitShowStash,
        GitOpenChanges,
        GitOpenFileHead,
        GitCompareBranch,
        GitCompareCommit,
        GitAddToGitignore,
        GitRenameRemote,
        GitSetRemoteUrl,
        GitRemoveWorktree,
        GitPruneWorktrees,
        GitDiscardAll,
        RestartLanguageServers,
        ManageLanguageServices,
        CopyFullDiagnostic,
        RustExpandMacro,
        RustParentModule,
        RustOpenCargoToml,
        ShowSettings,
        ShowKeyboardShortcuts,
        GitFileHistory,
        GitBlameLine,
        FindTodos,
        SwitchEditor,
        ToggleZenMode,
        InstallCyx,
        NewExtension,
        ToggleFullScreen,
        SelectIconTheme,
        InsertSnippet,
        ConfigureSnippets,
        SaveThemeAs,
        OpenThemesFolder,
        MinifyJson,
        ShowShortcutsPage,
        ShowWelcome,
        OpenSettingsPage,
        CloneRepository,
        OpenMarkdownPreview,
        CompareWithClipboard,
        CompareWithSaved,
        // --- compare ---
        CompareFolders,
        RunSelectedText,
        GoToLastEdit,
        NewScratchFile,
        ShowAbout,
        CheckForUpdates,
        UpdateDismiss,
        UpdateConfirm,
        UpdateFocusNext,
        UpdateFocusPrev,
        ShowDiskAccess,
        FollowFile,
        AddSshHost,
        RemoveSshHost,
        TestSshHost,
        EditSshConfig,
        EditSshHost,
        ManageSshHosts,
        SetupKeyLogin,
        UploadHere,
        DownloadActive,
        ClearSchemaCache,
        FixSpelling,
        InstallDictionary,
        ToggleSpelling,
        OpenMergeEditor,
        ShowPullRequests,
        SendRequest,
        ShowResponseHeaders,
        FilterLog,
        // --- data ---
        DataConvert,
        DataQuery,
        DataCopyPath,
        DataDecode,
    ]
);

/// Typing "#" in the palette before any query exists.
const SYMBOL_HINT: &str = "Type to search symbols in the workspace";

/// Minimum height of the bottom panel.
const PANEL_MIN: f32 = 120.;

/// Terminal list: default, narrowest and widest width, and the room always left to the terminals beside it.
const TERM_LIST_W: f32 = 220.;
const TERM_LIST_MIN: f32 = 120.;
const TERM_LIST_MAX: f32 = 480.;
const TERM_LIST_ROOM: f32 = 200.;

/// Width of the terminal list for a drag that asks for `want` in a panel `panel` wide.
fn term_list_width(want: f32, panel: f32) -> f32 {
    want.clamp(TERM_LIST_MIN, TERM_LIST_MAX.min(panel - TERM_LIST_ROOM).max(TERM_LIST_MIN))
}

/// Quick open lists at most this many files.
const FILE_LIMIT: u32 = 100_000;

/// Midnight / Daylight following the system light/dark mode.
pub const THEME_AUTO: &str = "auto";

/// Explorer OUTLINE section: symbols of the active file (refreshed at most every 400 ms while typing).
#[derive(Default)]
struct OutlineState {
    key: Option<(String, u64)>,
    symbols: Vec<crate::outline::Symbol>,
    supported: bool,
    too_large: bool,
    computed_at: Option<std::time::Instant>,
    refresh_pending: bool,
}

/// Pages opened as tabs.
#[derive(Clone)]
enum Page {
    Welcome,
    About(Entity<crate::about_page::AboutPage>),
    Containers,
    /// The Extensions view's detail page and consent sheet (state in `ConfigState::exts`).
    Extension,
    /// **Developer: Extension Inspector** (workbench_ext_inspector.rs).
    ExtensionInspector,
    Settings(Entity<crate::settings_page::SettingsPage>),
    Markdown(Entity<crate::markdown::MarkdownPreview>),
    Shortcuts(Entity<crate::shortcuts_page::ShortcutsPage>),
    LanguageServices(Entity<crate::language_services_page::LanguageServicesPage>),
    Merge(Entity<crate::merge::MergeView>),
    // --- compare ---
    Compare(Entity<crate::compare_folders::CompareView>),
    /// The Search Editor in front; every open one is also listed in its group's `searches`.
    Search(Entity<crate::search_editor::SearchEditor>),
    /// An extension view in the editor area, by its full id (workbench_ext_views.rs).
    ExtView(String),
}

/// Floating picker on top: theme picker or palette (files / commands / line).
struct Overlay {
    picker: Entity<Picker>,
    kind: OverlayKind,
    return_focus: Option<FocusHandle>,
    _sub: Subscription,
}

pub use debug::{
    AddDebugConfiguration, AddFunctionBreakpoint, ConsoleComplete, ConsoleHistoryNext, ConsoleHistoryPrev, DisableAllBreakpoints, EnableAllBreakpoints,
    OpenLaunchConfiguration, RunToCursor, RunWithoutDebugging, ShowRunAndDebug, StopAllDebugging, ToggleBreakpointEnabled, ToggleBreakpointsActive,
};
pub use ext_glue::{CancelExtensionConsent, InstallExtensionFromFile, InstallExtensionFromFolder, InstallExtensionFromRegistry, ShowExtensions};
pub use host::askpass::{AskKind, AskRequest};
pub use inspector_glue::ShowExtensionInspector;
pub use market_glue::{MarketNext, MarketPrevious, ShowMarket};
pub use providers_glue::RunExtensionCommand;
pub use remote_glue::ShowRemoteExplorer;
pub use terminals_glue::{
    ChangeTerminalColor, ConfigureTerminalSettings, FocusNextTerminal, FocusPreviousTerminal, KillAllTerminals, KillOtherTerminals, NewTerminalWithProfile,
    RenameTerminal, SelectDefaultTerminalProfile,
};
pub use tests_glue::{DebugTestAtCursor, RefreshTests, RunAllTests, RunFailedTests, ShowTesting, StopTests};
pub type AskReply = std::sync::mpsc::Sender<Option<String>>;

/// Options in the status bar pickers.
#[derive(Clone, Copy)]
enum EditorOpt {
    /// "Open With…": hex dump, Markdown preview, compare with the saved version.
    OpenWith(OpenWithKind),
    /// Language of this file (`None`: plain text).
    Language(Option<&'static str>),
    /// Write the `editor::LANGUAGES[i]` mapping for the active file's extension to the settings.
    Associate(usize),
    Eol {
        crlf: bool,
    },
    Indent(Option<usize>),
    Convert {
        to_tabs: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpenWithKind {
    Hex,
    MarkdownPreview,
    CompareSaved,
}

/// A position in the navigation history: (path, 1-based line, column).
type NavPos = (String, u32, u32);

/// Upper limit on simultaneously open terminals (PTYs).
const MAX_TERMINALS: usize = 8;

/// What the bottom panel shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bottom {
    Terminal,
    Problems,
    Output,
    Ports,
    /// Debug: call stack, variables, console.
    Debug,
    /// The extension view in `cfg.exts.views.bottom`.
    ExtView,
}

/// Settings files of a folder (lowest to highest priority).
fn folder_settings_paths(root: &str) -> Vec<String> {
    vec![ui::join(root, ".vscode/settings.json"), ui::join(root, LEGACY_PROJECT_SETTINGS), ui::join(root, PROJECT_SETTINGS)]
}

/// Separator between extra folder name and path in the Go to file list (never occurs in file names).
pub(super) const ROOT_SEP: &str = " › ";

/// ID of the extra folder section (header drawing needs `&'static str`; IDs are created once).
fn extra_folder_id(i: usize) -> &'static str {
    static IDS: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
    IDS.get_or_init(|| (1..=folder_glue::MAX_EXTRA_ROOTS).map(|n| &*Box::leak(format!("folder-{n}").into_boxed_str())).collect())[i]
}

/// Context menu of the file tree.
struct MenuState {
    menu: Entity<ContextMenu>,
    position: gpui::Point<gpui::Pixels>,
    target: MenuTarget,
    commands: Vec<MenuCmd>,
    actions: Vec<Box<dyn gpui::Action>>,
    return_focus: Option<FocusHandle>,
    app_path: Option<Vec<usize>>,
    /// The Manage menu: re-anchored to the gear button whenever the viewport changes (zoom, resize).
    follows_manage: bool,
    _sub: Subscription,
}

#[derive(Clone)]
struct MenuTarget {
    path: String,
    is_dir: bool,
    /// Tab index if this is a tab menu.
    tab: Option<usize>,
}

/// Layout to restore when leaving zen mode.
struct ZenSaved {
    panel: Option<Panel>,
    panel_open: bool,
    was_fullscreen: bool,
}

/// Name of the pasted copy: "a.txt" -> "a copy.txt", "a copy 2.txt"… (`n` = 0 is the first copy).
pub(crate) fn copy_name(name: &str, n: usize) -> String {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    if n == 0 { format!("{stem} copy{ext}") } else { format!("{stem} copy {}{ext}", n + 1) }
}

fn parent_of(path: &str) -> String {
    match path.trim_end_matches('/').rsplit_once('/') {
        Some(("", _)) | None => "/".to_string(),
        Some((parent, _)) => parent.to_string(),
    }
}

/// Seconds since the Unix epoch (0 if the clock is before it); shared by the `.http` and pull request views.
fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// File list of a folder (the palette shows this first on every open, then refreshes).
struct FileIndex {
    root: String,
    paths: Vec<String>,
    truncated: bool,
}

/// ~/.config/codeyard/themes (Windows: %APPDATA%\codeyard\themes).
/// Under the config dir (moves with `CODEYARD_CONFIG_DIR`).
pub fn user_theme_dir() -> Option<std::path::PathBuf> {
    settings::config_dir().map(|d| d.join("themes"))
}

/// Dev only: fake host name that exercises the remote code path without ssh.
pub const LOCAL_AGENT_ALIAS: &str = "agent-stdio";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Panel {
    Explorer,
    Search,
    Scm,
    Debug,
    Testing,
    Remote,
    Containers,
    Extensions,
    /// The extension view in `cfg.exts.views.sidebar`.
    ExtView,
}

enum Conn {
    Local,
    Connecting {
        alias: String,
        step: Arc<AtomicU8>,
    },
    Connected {
        alias: String,
    },
    /// Connection dropped, retrying (terminals live on the server).
    Reconnecting {
        alias: String,
        attempt: u32,
    },
    Failed {
        alias: String,
        message: SharedString,
    },
}

enum Notice {
    Info(SharedString),
    Error(SharedString),
}

pub struct Workbench {
    /// Watch set last sent to the agent, and the task listening for events.
    watched: Vec<String>,
    fs_task: Option<gpui::Task<()>>,
    status_task: Option<gpui::Task<()>>,
    search: Entity<SearchPanel>,
    scm: Entity<crate::scm::ScmPanel>,
    /// Explorer sections (OPEN EDITORS, FAVORITES, TIMELINE…) and the active file's history.
    sections: crate::sections::Sections,
    timeline: git_glue::Timeline,
    /// Commands last run from the palette (newest first, at most 8; persisted).
    recent_commands: Vec<String>,
    outline: OutlineState,
    /// Port forwarding state (see `workbench_ports.rs`).
    ports: ports_glue::PortsState,
    /// Remote folder picker: subfolder listing / path validation.
    folder_task: Option<gpui::Task<()>>,
    /// This workbench's window (for calling window-requiring work from the background).
    window_handle: gpui::AnyWindowHandle,
    terminals: Vec<Entity<TerminalView>>,
    /// "Terminal: Split": two terminals side by side (left, right): shown together whenever one of them is the active terminal.
    term_split: Option<(Entity<TerminalView>, Entity<TerminalView>)>,
    /// What the entries of the open "launch profile" menu start.
    term_launches: Vec<crate::terminal::Launch>,
    active_terminal: usize,
    /// A specific shell in tests; the user's shell otherwise.
    shell: Option<String>,
    notice: Option<Notice>,
    title: String,
    /// Last seen notice: on change, a clear timer is set up for info messages.
    /// Message identity (string address and length): the same text arriving again is still a new message.
    notice_seen: Option<(usize, usize)>,
    notice_task: Option<gpui::Task<()>>,
    dev: bool,
    themes: Vec<Theme>,
    theme_choice: String,
    overlay: Option<Overlay>,
    menu: Option<MenuState>,
    /// Test: `git status` never returns (hung git).
    #[cfg(test)]
    pub git_status_hangs: bool,
    /// Debug session and its settings (see workbench_debug.rs).
    dbg: state_glue::DebugUi,
    /// Last task, plus problems parsed from its terminal and output (see workbench_tasks.rs).
    jobs: state_glue::TaskState,
    /// Language servers, diagnostics, progress and log (see workbench_lsp.rs).
    lsp: state_glue::LspState,
    /// Git state of the open folder and refresh flags (see workbench_git.rs).
    vcs: state_glue::GitState,
    /// Layout: side panel, bottom panel, split editor, zoom, zen mode, window bounds.
    chrome: state_glue::ChromeLayout,
    /// Navigation history, closed tabs and the last edit location.
    nav: state_glue::NavState,
    /// Settings files, project settings and workspace trust (see workbench_settings.rs).
    cfg: state_glue::ConfigState,
    /// Workspace record, backups, session state and recents.
    persist: state_glue::PersistState,
    /// Connection: host machine, SSH details and pending questions (see workbench_folder.rs).
    remote: state_glue::RemoteState,
    /// Explorer roots, clipboard and favorites.
    roots: state_glue::RootsState,
    /// In-app updates: state, dialog and worker thread (see workbench_update.rs).
    update: state_glue::UpdateUi,
    /// Editor groups: tabs, separate tab views (diff, large file, page) and startup state.
    tabs: state_glue::TabsState,
    /// Search Editors: subscriptions, the last replacement's undo record and saved state (see workbench_search_editor.rs).
    searches: search_editor_glue::SearchEditors,
    // --- data ---
    /// Data lens: the query bar and the status bar path (see workbench_data.rs).
    data: data_glue::DataState,
    focus_handle: FocusHandle,
    _subs: Vec<Subscription>,
}

impl Focusable for Workbench {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// A refresh is abandoned if `git status` doesn't finish in this time (the next one retries).
const GIT_STATUS_TIMEOUT: Duration = Duration::from_secs(20);

const WINDOW_BOUNDS_KEY: &str = "window.bounds";

/// `"windowed x y w h"`, `"maximized …"`, `"fullscreen …"` (the bounds are the size to restore to).
pub(crate) fn format_bounds(b: gpui::WindowBounds) -> String {
    let (kind, r) = match b {
        gpui::WindowBounds::Windowed(r) => ("windowed", r),
        gpui::WindowBounds::Maximized(r) => ("maximized", r),
        gpui::WindowBounds::Fullscreen(r) => ("fullscreen", r),
    };
    let f = |p: gpui::Pixels| f32::from(p).round();
    format!("{kind} {} {} {} {}", f(r.origin.x), f(r.origin.y), f(r.size.width), f(r.size.height))
}

pub(crate) fn parse_bounds(s: &str) -> Option<gpui::WindowBounds> {
    let mut it = s.split_whitespace();
    let kind = it.next()?;
    let n: Vec<f32> = it.map(|x| x.parse().ok()).collect::<Option<_>>()?;
    let [x, y, w, h] = n[..] else { return None };
    if w < 200. || h < 150. {
        return None;
    }
    let r = gpui::Bounds::new(gpui::point(px(x), px(y)), gpui::size(px(w), px(h)));
    Some(match kind {
        "windowed" => gpui::WindowBounds::Windowed(r),
        "maximized" => gpui::WindowBounds::Maximized(r),
        "fullscreen" => gpui::WindowBounds::Fullscreen(r),
        _ => return None,
    })
}

/// Whether the saved window is usable on this screen: if the display was unplugged or the resolution
/// changed, the window must not open as a sliver at the screen edge. A grabbable part
/// of the title bar (200×100 px) has to be visible.
pub(crate) fn visible_enough(display: gpui::Bounds<gpui::Pixels>, window: gpui::Bounds<gpui::Pixels>) -> bool {
    let top = gpui::Bounds::new(window.origin, gpui::size(window.size.width, px(100.).min(window.size.height)));
    let seen = display.intersect(&top);
    seen.size.width >= px(200.) && seen.size.height >= px(30.) && display.intersect(&window).size.height >= px(100.)
}

/// Last saved window bounds (the first window opens with these).
pub fn saved_window_bounds() -> Option<gpui::WindowBounds> {
    State::open(&state_path()?).ok()?.get(WINDOW_BOUNDS_KEY).as_deref().and_then(parse_bounds)
}

/// Can be redirected with `CODEYARD_STATE_DIR` (so measurement scripts and
/// experiments don't pollute the user's recents list).
/// The app's old name: config and state folders are moved to the new name on first launch.
const LEGACY_APP_DIR: &str = "codeapp";

/// Move config and state folders left under the old name (`codeapp`) to the new name. Only if the new
/// folder doesn't exist and the path wasn't overridden by an env var; `rename` within the same parent.
pub fn migrate_legacy_dirs() {
    let dirs = [settings::config_dir(), state_path().and_then(|p| p.parent().map(|d| d.to_path_buf()))];
    for new in dirs.into_iter().flatten() {
        if new.file_name().and_then(|n| n.to_str()) != Some(host::ssh::APP_DIR) {
            continue; // CODEYARD_CONFIG_DIR / CODEYARD_STATE_DIR
        }
        if let Err(e) = move_legacy_dir(&new) {
            eprintln!("could not move the old {LEGACY_APP_DIR} folder to {}: {e}", new.display());
        }
    }
}

/// Move the old-named folder next to `new` into `new` (if `new` is absent). Returns `true` if moved.
pub(crate) fn move_legacy_dir(new: &std::path::Path) -> std::io::Result<bool> {
    let old = new.with_file_name(LEGACY_APP_DIR);
    if !old.is_dir() || new.exists() {
        return Ok(false);
    }
    std::fs::rename(&old, new).map(|_| true)
}

/// Path typed for "Save a Copy to This Computer" -> absolute local path. `~` is the home dir, relative paths are relative to it.
pub(crate) fn local_copy_target(name: &str, home: Option<&std::path::Path>) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Type a path on this computer.".into());
    }
    let home_str = home.map(|h| h.to_string_lossy().into_owned());
    let expanded = match (name, &home_str) {
        ("~", Some(h)) => h.clone(),
        (n, Some(h)) if n.starts_with("~/") => ui::join(h, &n[2..]),
        (n, _) if std::path::Path::new(n).is_absolute() => n.to_string(),
        (n, Some(h)) => ui::join(h, n),
        _ => return Err("Cannot find your home folder; type an absolute path.".into()),
    };
    if expanded.ends_with('/') || expanded.ends_with('\\') {
        return Err("Type a file name, not a folder.".into());
    }
    Ok(expanded)
}

/// Crash report folder: `crashes/` under the state folder.
pub(crate) fn crash_dir() -> Option<std::path::PathBuf> {
    state_path()?.parent().map(|d| d.join("crashes"))
}

pub(crate) fn state_path() -> Option<std::path::PathBuf> {
    if let Some(dir) = std::env::var_os("CODEYARD_STATE_DIR") {
        return Some(std::path::PathBuf::from(dir).join("state.db"));
    }
    dirs::state_dir().or_else(dirs::data_local_dir).map(|d| d.join(host::ssh::APP_DIR).join("state.db"))
}

/// Shorten the home dir to `~`. The macOS filesystem is case-insensitive:
/// with $HOME "/Users/Ad" the path may arrive as "/Users/ad/...".
pub(super) fn tilde(path: &str) -> String {
    let Some(home) = dirs::home_dir().map(|h| h.to_string_lossy().into_owned()) else { return path.to_string() };
    let matches = if cfg!(target_os = "macos") { path.len() >= home.len() && path[..home.len()].eq_ignore_ascii_case(&home) } else { path.starts_with(&home) };
    if matches && path.is_char_boundary(home.len()) { format!("~{}", &path[home.len()..]) } else { path.to_string() }
}

/// Tab being dragged (to reorder within a group or move to the other group).
#[derive(Clone)]
struct DraggedTab {
    editor: Entity<EditorView>,
    /// Whether it was in the focused group when the drag started.
    from: GroupKey,
    name: SharedString,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        div()
            .px(px(theme::SPACE[4]))
            .py(px(theme::SPACE[1]))
            .rounded(px(theme::radius::SM))
            .bg(c(p.bg))
            .border_1()
            .border_color(c(p.accent))
            .text_size(px(text::SM.0))
            .text_color(c(p.text))
            .child(self.name.clone())
    }
}

/// Folder hint for same-named tabs: the shortest folder suffix that tells them apart
/// (`server/src/main.rs` and `client/src/main.rs` -> "server/src", "client/src"). `None` for a unique name.
pub(crate) fn twin_hints(paths: &[String]) -> Vec<Option<String>> {
    let split = |p: &str| -> (String, Vec<String>) {
        let mut parts: Vec<String> = p.split('/').filter(|s| !s.is_empty()).map(str::to_string).collect();
        let name = parts.pop().unwrap_or_default();
        (name, parts)
    };
    let all: Vec<(String, Vec<String>)> = paths.iter().map(|p| split(p)).collect();
    all.iter()
        .enumerate()
        .map(|(i, (name, dirs))| {
            let twins: Vec<&Vec<String>> = all.iter().enumerate().filter(|(j, (n, _))| *j != i && n == name).map(|(_, (_, d))| d).collect();
            if twins.is_empty() {
                return None;
            }
            let suffix = |d: &[String], n: usize| d[d.len().saturating_sub(n)..].join("/");
            // Shortest distinguishing suffix; the whole path if none separates them (e.g. one is the tail of the other).
            let n = (1..=dirs.len()).find(|&n| twins.iter().all(|t| suffix(t, n) != suffix(dirs, n))).unwrap_or(dirs.len());
            let hint = suffix(dirs, n);
            Some(if hint.is_empty() {
                "/".to_string()
            } else if n < dirs.len() && n > 1 {
                format!("…/{hint}")
            } else {
                hint
            })
        })
        .collect()
}

/// Breadcrumb items: (name, absolute path). The root is only cut at a folder boundary:
/// with root `/a/b`, `/a/bc/x` is not under the root and the full path is shown.
pub(crate) fn crumb_paths(root: Option<&str>, path: &str) -> Vec<(String, String)> {
    let root = root.map(|r| r.trim_end_matches('/')).filter(|r| path.strip_prefix(r).is_some_and(|rest| rest.starts_with('/')));
    let mut abs = root.unwrap_or("").to_string();
    let rel = root.map_or(path, |r| &path[r.len()..]);
    rel.split('/')
        .filter(|s| !s.is_empty())
        .map(|part| {
            abs = format!("{abs}/{part}");
            (part.to_string(), abs.clone())
        })
        .collect()
}

impl Workbench {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_at(state_path(), settings::config_dir(), window, cx)
    }

    fn root(&self, cx: &App) -> Option<String> {
        self.roots.primary.read(cx).root().map(str::to_string)
    }

    /// Key for persisted records (tabs, bookmarks, favorites): relative path under the first root,
    /// absolute path for extra folders (`ui::join` leaves an absolute name as is).
    fn rel(&self, path: &str, cx: &App) -> Option<String> {
        let root = self.root(cx)?;
        match path.strip_prefix(&format!("{}/", root.trim_end_matches('/'))) {
            Some(rel) => Some(rel.to_string()),
            None => self
                .roots
                .extra_roots
                .iter()
                .chain(&self.roots.pending_extra_roots)
                .any(|e| path.starts_with(&format!("{}/", e.trim_end_matches('/'))))
                .then(|| path.to_string()),
        }
    }

    /// Tests supply an in-memory database and no config dir; the user's
    /// files are left alone.
    pub fn with_state(state: Option<State>, config_dir: Option<std::path::PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let host: Arc<dyn Host> = Arc::new(host::local());
        if let Some(s) = state.as_ref() {
            Self::migrate_trust(s);
        }
        let tree = cx.new(|cx| FileTree::new(host.clone(), cx));
        let search = cx.new(|cx| SearchPanel::new(host.clone(), cx));
        let scm = cx.new(|cx| crate::scm::ScmPanel::new(host.clone(), cx));
        if let Some(text) = state.as_ref().and_then(|s| s.get("scm_sections")) {
            scm.update(cx, |s, cx| s.restore_sections(&text, cx));
        }
        let port_field = cx.new(|cx| crate::input::TextField::new(crate::i18n::tr("e.g. 3000"), cx));
        let subs = vec![
            cx.subscribe_in(&tree, window, |this, _, event: &TreeEvent, window, cx| this.on_tree_event(event, window, cx)),
            cx.subscribe_in(&search, window, |this, _, event: &SearchEvent, window, cx| match event {
                SearchEvent::Open { path, line, column, len } => {
                    this.tabs.pending_reveal = Some((path.clone(), *line, *column, *len));
                    this.open_file(path.clone(), window, cx);
                }
                SearchEvent::OpenInEditor(spec) => {
                    this.open_search_editor(spec.clone(), None, window, cx);
                }
                SearchEvent::ReviewReplace { spec, only } => {
                    let review = match only {
                        Some(path) => crate::search_editor::Review::Only(path.clone()),
                        None => crate::search_editor::Review::All,
                    };
                    this.open_search_editor(spec.clone(), Some(review), window, cx);
                }
                // --- structural ---
                SearchEvent::StructuralBuffers { generation, family } => this.structural_buffers(*generation, family, cx),
                SearchEvent::StructuralReplace { files, family, pattern, replacement, matches } => {
                    this.confirm_structural_replace(files.clone(), family, pattern.clone(), replacement.clone(), *matches, window, cx)
                }
            }),
            cx.subscribe_in(&port_field, window, |this, field, ev: &crate::input::FieldEvent, _, cx| {
                let text = field.read(cx).text().to_string();
                if matches!(ev, crate::input::FieldEvent::Submit) && this.submit_port_field(&text, cx) {
                    field.update(cx, |f, cx| f.set_text("", cx));
                }
            }),
            cx.subscribe_in(&scm, window, |this, _, event: &crate::scm::ScmEvent, window, cx| match event {
                crate::scm::ScmEvent::Refresh => this.refresh_git(0, cx),
                crate::scm::ScmEvent::OpenDiff { path, staged } => this.open_diff(path.clone(), *staged, window, cx),
                crate::scm::ScmEvent::OpenWorktree { root, path } => {
                    if this.vcs.status.as_ref().is_some_and(|s| &s.root == root) {
                        this.set_root(path.clone(), cx);
                    }
                }
                crate::scm::ScmEvent::AddWorktree => this.git_add_worktree(window, cx),
                crate::scm::ScmEvent::MoreActions(position) => this.open_scm_menu(*position, window, cx),
                crate::scm::ScmEvent::GraphMenu(position) => this.open_graph_menu(*position, window, cx),
                // --- git-sync ---
                crate::scm::ScmEvent::Sync => this.git_sync_auto(window, cx),
                crate::scm::ScmEvent::Publish => this.git_publish_branch(cx),
                crate::scm::ScmEvent::PostCommit(then) => this.git_post_commit(*then, cx),
                crate::scm::ScmEvent::OpenCommit(commit) => this.show_commit_files(commit.clone(), window, cx),
                crate::scm::ScmEvent::OpenCommitFile(commit, file) => this.open_commit_file(commit.clone(), file.clone(), window, cx),
                crate::scm::ScmEvent::CommitMenu(commit, position) => this.open_commit_menu(commit.clone(), *position, window, cx),
                crate::scm::ScmEvent::FileMenu(path, staged, position) => this.open_scm_file_menu(path, *staged, *position, window, cx),
                crate::scm::ScmEvent::SectionsChanged(text) => {
                    if let Some(s) = &this.persist.state {
                        let _ = s.set("scm_sections", text);
                    }
                }
                crate::scm::ScmEvent::SelectRepository(key) => {
                    this.select_repository(*key, cx);
                }
                crate::scm::ScmEvent::OpenRepoDiff { repo, path, staged } => {
                    if this.select_repository(*repo, cx) {
                        this.open_diff(path.clone(), *staged, window, cx);
                    }
                }
                crate::scm::ScmEvent::RepoFileMenu(repo, path, staged, position) => {
                    if this.select_repository(*repo, cx) {
                        this.open_scm_file_menu(path, *staged, *position, window, cx);
                    }
                }
                crate::scm::ScmEvent::RefreshRepo(key) => this.refresh_repo(*key, cx),
                crate::scm::ScmEvent::Rediscover => this.rediscover(cx),
                crate::scm::ScmEvent::RefreshInventory => this.refresh_inventory(cx),
            }),
            // A download in progress is cancelled and its partial file removed before the process ends.
            cx.on_app_quit(|this, cx| {
                this.discard_response_on_quit();
                // Tasks lead their own process groups: they would outlive the app if they were not stopped here.
                this.stop_task_runs_for_quit();
                let wait = this.cancel_update_for_quit().map(|exited| Workbench::wait_for_update_thread(exited, cx));
                async move {
                    if let Some(wait) = wait {
                        wait.await;
                    }
                }
            }),
            // The first-run update question waits for a quiet keyboard (a letter typed a moment ago must not answer it).
            cx.observe_keystrokes(|this, _, _, _| this.update.last_key = Some(this.update.backend.now())),
            // The Dock menu lists the windows front one first: it changes whenever one comes forward.
            cx.observe_window_activation(window, |this, window, cx| {
                #[cfg(target_os = "macos")]
                crate::menus::refresh_dock_windows(cx);
                // Reduce motion / Increase contrast may have changed in System Settings meanwhile.
                if window.is_window_active() && crate::system_prefs::refresh(cx) {
                    let choice = this.theme_choice.clone();
                    this.apply_theme(&choice, window, cx);
                    window.refresh();
                }
            }),
            cx.observe_window_appearance(window, |this, window, cx| {
                if this.theme_choice == THEME_AUTO {
                    let choice = this.theme_choice.clone();
                    this.apply_theme(&choice, window, cx);
                }
            }),
        ];
        let (recent, pinned) = state.as_ref().map(projects::load_recent).unwrap_or_default();
        let dev = std::env::var_os("CODEYARD_DEV").is_some() || cfg!(debug_assertions);
        // The settings folder given (a temporary one in tests), as `theme_dir` does later, else the default.
        let loading = std::time::Instant::now();
        crate::ext::reload(config_dir.as_deref());
        crate::perf::sample("startup.extensions_ms", loading.elapsed().as_secs_f64() * 1000.0);
        let (themes, errors) = theme_glue::all_themes(config_dir.as_ref().map(|d| d.join("themes")).or_else(user_theme_dir).as_deref());
        let notice = errors.first().map(|reason| Notice::Error(crate::i18n::tf("Theme skipped: {}", &[reason]).into()));
        let theme_choice = state.as_ref().and_then(|s| s.get("theme")).unwrap_or_else(|| THEME_AUTO.into());
        let sidebar_w = state
            .as_ref()
            .and_then(|s| s.get("sidebar_w"))
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(layout::SIDEBAR_W)
            .clamp(layout::SIDEBAR_MIN, layout::SIDEBAR_MAX);
        let panel_h = state.as_ref().and_then(|s| s.get("panel_h")).and_then(|v| v.parse::<f32>().ok()).unwrap_or(layout::PANEL_H).max(PANEL_MIN);
        let panel_right = state.as_ref().and_then(|s| s.get("panel_right")).is_some_and(|v| v == "1");
        let panel_w = state.as_ref().and_then(|s| s.get("panel_w")).and_then(|v| v.parse::<f32>().ok()).unwrap_or(480.).max(PANEL_MIN * 2.);
        let term_list_w = state
            .as_ref()
            .and_then(|s| s.get("terminal_list_w"))
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(TERM_LIST_W)
            .clamp(TERM_LIST_MIN, TERM_LIST_MAX);
        let mut wb = Workbench {
            watched: Vec::new(),
            fs_task: None,
            status_task: None,
            search,
            scm,
            sections: state.as_ref().and_then(|s| s.get("explorer_sections")).map(|t| crate::sections::Sections::decode(&t)).unwrap_or_default(),
            timeline: Default::default(),
            recent_commands: state.as_ref().and_then(|s| s.get("recent_commands")).map(|t| t.lines().map(str::to_string).collect()).unwrap_or_default(),
            outline: Default::default(),
            ports: ports_glue::PortsState::new(port_field),
            folder_task: None,
            window_handle: window.window_handle(),
            notice,
            title: String::new(),
            notice_seen: None,
            notice_task: None,
            dev,
            themes,
            theme_choice: theme_choice.clone(),
            overlay: None,
            menu: None,
            #[cfg(test)]
            git_status_hangs: false,
            terminals: Vec::new(),
            term_launches: Vec::new(),
            term_split: None,
            active_terminal: 0,
            shell: None,
            dbg: Default::default(),
            jobs: state_glue::TaskState {
                pending: None,
                asking: 0,
                runs: Vec::new(),
                next_token: 0,
                last: None,
                terminal: None,
                root: None,
                problems: Vec::new(),
                scan_pending: false,
                tests: Default::default(),
            },
            lsp: state_glue::LspState {
                restricted_notified: Default::default(),
                servers: Default::default(),
                diagnostics: Default::default(),
                output: Default::default(),
                pending_rename_edits: Vec::new(),
                progress: None,
                progress_next: None,
                progress_timer: None,
                tasks: Vec::new(),
                generation: 0,
                restarts: Default::default(),
                symbol_task: None,
                pending_reveal: None,
                schema: None,
                schema_tasks: Default::default(),
                spell: None,
                spell_langs: Vec::new(),
                spell_loading: None,
                spell_root: None,
                http_responses: Default::default(),
                http_last: None,
                http_tab: None,
                http_busy: false,
                http_trusted: Default::default(),
                schema_asking: Default::default(),
            },
            vcs: state_glue::GitState {
                branch: None,
                status: None,
                refresh: None,
                refreshing: false,
                refresh_again: false,
                sync: Default::default(),
                error: None,
                scm_root: None,
                marks_busy: Default::default(),
                tree_marks: Default::default(),
                repos: Default::default(),
                menu_repo: None,
            },
            chrome: state_glue::ChromeLayout {
                a11y_bounds: Default::default(),
                row_nav: row_nav_glue::RowNav::new(cx),
                resources: cx.new(crate::resource_monitor::ResourceMonitor::new),
                web_preview: Default::default(),
                panel: Some(Panel::Explorer),
                bottom: Bottom::Terminal,
                sidebar_w,
                resizing_sidebar: false,
                panel_open: false,
                panel_h,
                panel_right,
                panel_w,
                panel_max: false,
                panel_anim: None,
                panel_area: None,
                resizing_panel: false,
                term_list_w,
                resizing_term_list: false,
                resizing_split: false,
                activity_scroll: gpui::ScrollHandle::new(),
                activity_hints: Default::default(),
                zen: None,
                window_bounds: None,
            },
            nav: state_glue::NavState {
                back: Vec::new(),
                forward: Vec::new(),
                navigating: false,
                closed: Vec::new(),
                last_edit: None,
                edit_versions: Default::default(),
                recent_files: Vec::new(),
            },
            cfg: state_glue::ConfigState {
                dir: config_dir,
                stamp: (None, None, None, 0),
                theme_override: false,
                project: Vec::new(),
                trusted: Default::default(),
                dirty: false,
                folders: Default::default(),
                folder_texts: Default::default(),
                user_text: None,
                exts: Default::default(),
            },
            persist: state_glue::PersistState {
                workspace_id: None,
                data_dir: None,
                pending_restore: None,
                pending_restore_workspace: None,
                backup_task: None,
                backed_up: Default::default(),
                state,
                recent,
                pinned,
                opening_recent: None,
            },
            remote: state_glue::RemoteState {
                docker: Default::default(),
                local_host: host.clone(),
                host,
                conn: Conn::Local,
                pending_path: None,
                connection_origin: None,
                ssh_hosts: host::ssh::list_hosts(),
                bundle: Arc::new(crate::bundled_agents::discover()),
                asks: Default::default(),
                transfer: None,
                view: Default::default(),
            },
            roots: state_glue::RootsState {
                primary: tree,
                extra_roots: Vec::new(),
                extra_trees: Vec::new(),
                pending_extra_roots: Vec::new(),
                file_clip: None,
                file_operation: None,
                compare_a: None,
                // --- compare ---
                compare_folder_a: None,
                favorites: Vec::new(),
                file_index: None,
                workspace: None,
                workspace_request: 0,
                workspace_generation: 0,
            },
            tabs: Default::default(),
            searches: Default::default(),
            update: state_glue::UpdateUi::new(dev, cx),
            data: Default::default(),
            focus_handle: cx.focus_handle(),
            _subs: subs,
        };
        wb.apply_theme(&theme_choice, window, cx);
        wb.reload_config(window, cx);
        wb.watch_config(window, cx);
        wb.poll_git(window, cx);
        wb.poll_extension_updates(window, cx);
        wb.poll_autofetch(window, cx);
        wb.listen_fs(cx);
        wb
    }

    // --- File watching ---

    pub(super) fn open_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        self.open_config_file(true, window, cx);
    }

    pub(super) fn open_keybindings(&mut self, _: &OpenKeybindings, window: &mut Window, cx: &mut Context<Self>) {
        self.open_config_file(false, window, cx);
    }

    /// The settings file always opens on this machine (also while connected over SSH).
    fn open_config_file(&mut self, settings_file: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((spath, kpath)) = self.config_paths() else { return };
        let (path, template) = if settings_file { (spath, settings::TEMPLATE) } else { (kpath, settings::KEYBINDINGS_TEMPLATE) };
        if let Err(e) = settings::ensure_file(&path, template) {
            self.notice = Some(Notice::Error(crate::i18n::tf("Could not create {0}: {1}", &[&path.display(), &e]).into()));
            cx.notify();
            return;
        }
        let host = self.remote.local_host.clone();
        self.open_file_on(host, path.to_string_lossy().into_owned(), window, cx);
    }

    /// Import the equivalents of VS Code's settings and shortcuts (VS Code's files
    /// are left alone). If the theme isn't here, copy it from the installed VS Code theme.
    fn import_vscode(&mut self, _: &ImportVsCode, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = crate::vscode::user_dir() else {
            self.notice = Some(Notice::Info(crate::i18n::t("No VS Code settings found on this machine").into()));
            cx.notify();
            return;
        };
        match self.import_vscode_from(&dir) {
            Ok(msg) => {
                self.reload_user_themes();
                self.notice = Some(Notice::Info(msg.into()));
                self.reload_config(window, cx);
            }
            Err(e) => self.notice = Some(Notice::Error(e.into())),
        }
        cx.notify();
    }

    pub fn import_vscode_from(&mut self, dir: &std::path::Path) -> Result<String, String> {
        let (spath, kpath) = self.config_paths().ok_or(crate::i18n::t("no configuration directory"))?;
        let read = |p: &std::path::Path| std::fs::read_to_string(p).ok();
        let mut notes = Vec::new();
        let mut n_settings = 0;
        let mut skipped_settings = 0;
        if let Some(vs) = read(&dir.join("settings.json")) {
            let mut themes: Vec<(String, String)> = self.themes.iter().map(|t| (t.id.clone(), t.name.clone())).collect();
            // Fetch the VS Code theme if it isn't here yet.
            let wanted =
                serde_json::from_str::<serde_json::Value>(&themes::strip_jsonc(&vs)).ok().and_then(|v| v["workbench.colorTheme"].as_str().map(str::to_string));
            if let Some(name) = wanted.filter(|n| !themes.iter().any(|(_, t)| t.eq_ignore_ascii_case(n)))
                && let (Some(theme), Some(tdir)) = (crate::vscode::find_theme(&name), user_theme_dir())
            {
                let slug: String = name.chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
                let file = tdir.join(format!("vscode-{slug}.json"));
                if std::fs::create_dir_all(&tdir).and_then(|_| crate::fsutil::write_atomic(&file, theme.to_string())).is_ok() {
                    self.reload_user_themes();
                    themes = self.themes.iter().map(|t| (t.id.clone(), t.name.clone())).collect();
                }
            }
            let converted = crate::vscode::convert_settings(&vs, &themes);
            settings::ensure_file(&spath, settings::TEMPLATE).map_err(|e| e.to_string())?;
            let mut text = read(&spath).unwrap_or_default();
            for (k, v) in &converted.settings {
                text = settings::set_value_in(&text, k, v);
            }
            crate::fsutil::write_atomic(&spath, text).map_err(|e| format!("{}: {e}", spath.display()))?;
            n_settings = converted.settings.len();
            skipped_settings = converted.skipped;
            notes.extend(converted.notes);
        }
        let mut n_keys = 0;
        let mut skipped_keys = 0;
        if let Some(vs) = read(&dir.join("keybindings.json")) {
            let (entries, skipped) = crate::vscode::convert_keybindings(&vs);
            settings::ensure_file(&kpath, settings::KEYBINDINGS_TEMPLATE).map_err(|e| e.to_string())?;
            let ours = read(&kpath).unwrap_or_default();
            crate::fsutil::write_atomic(&kpath, crate::vscode::merge_keybindings(&ours, &entries)).map_err(|e| format!("{}: {e}", kpath.display()))?;
            n_keys = entries.len();
            skipped_keys = skipped;
        }
        let mut msg = crate::i18n::tf(
            "Imported {0} settings and {1} shortcuts from VS Code ({2} settings and {3} shortcuts have no equivalent here)",
            &[&n_settings, &n_keys, &skipped_settings, &skipped_keys],
        );
        if let Some(n) = notes.first() {
            msg = format!("{msg}; {n}");
        }
        Ok(msg)
    }

    pub fn focus_active_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.tabs.active {
            self.activate(ix, window, cx);
        }
    }

    fn show_overlay(&mut self, picker: Entity<Picker>, kind: OverlayKind, window: &mut Window, cx: &mut Context<Self>) {
        let sub = cx.subscribe_in(&picker, window, |this, _, event: &PickerEvent, window, cx| {
            this.on_overlay_event(event, window, cx);
        });
        let return_focus = window.focused(cx);
        window.focus(&picker.focus_handle(cx));
        self.overlay = Some(Overlay { picker, kind, return_focus, _sub: sub });
        cx.notify();
    }

    /// Close the overlay, give focus back to where it was before it opened.
    fn close_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Overlay> {
        let ov = self.overlay.take()?;
        if matches!(ov.kind, OverlayKind::TaskInput(_) | OverlayKind::Input(InputPurpose::TaskInput)) {
            self.task_input_closed(window, cx);
        }
        match &ov.return_focus {
            Some(f) => window.focus(f),
            None => window.focus(&self.focus_handle),
        }
        cx.notify();
        Some(ov)
    }

    /// `--terminal`
    pub fn open_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.new_terminal(&NewTerminal, window, cx);
    }

    /// `--search TEXT`
    pub fn search_for(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.chrome.panel = Some(Panel::Search);
        self.search.update(cx, |s, cx| s.focus_query(Some(text.to_string()), window, cx));
        cx.notify();
    }

    fn find_in_files(&mut self, _: &FindInFiles, window: &mut Window, cx: &mut Context<Self>) {
        self.chrome.panel = Some(Panel::Search);
        let prefill = self.active_editor().and_then(|e| e.read(cx).selected_text());
        self.search.update(cx, |s, cx| s.focus_query(prefill, window, cx));
        cx.notify();
    }

    pub fn notice_text(&self) -> Option<String> {
        self.notice.as_ref().map(|n| match n {
            Notice::Info(m) | Notice::Error(m) => m.to_string(),
        })
    }

    fn panel(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let resizing = self.chrome.resizing_panel;
        let (right, max) = (self.chrome.panel_right, self.chrome.panel_max);
        div()
            .debug_selector(|| "panel".into())
            .relative()
            .flex()
            .flex_col()
            .bg(c(p.bg))
            .border_color(c(p.border))
            .map(|d| match (self.animated_panel_size(), max, right) {
                (Some(size), _, true) => d.w(px(size)).flex_none().h_full().border_l_1(),
                (Some(size), _, false) => d.h(px(size)).flex_none().border_t_1(),
                (None, true, _) => d.flex_1().min_h_0().min_w_0(),
                (None, false, true) => d.w(px(self.chrome.panel_w)).flex_none().h_full().border_l_1(),
                (None, false, false) => d.h(px(self.chrome.panel_h)).flex_none().border_t_1(),
            })
            .child(
                div()
                    .id("panel-sash")
                    .absolute()
                    // Keep content underneath (terminal, editor) from receiving the mouse press.
                    .occlude()
                    .when(max, |d| d.invisible())
                    .map(|d| {
                        if right {
                            d.left(px(-layout::SASH / 2.)).top_0().bottom_0().w(px(layout::SASH)).cursor(gpui::CursorStyle::ResizeLeftRight)
                        } else {
                            d.top(px(-layout::SASH / 2.)).left_0().right_0().h(px(layout::SASH)).cursor(gpui::CursorStyle::ResizeUpDown)
                        }
                    })
                    .child(crate::ui_sash::highlight(p.accent, resizing))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.chrome.resizing_panel = true;
                            cx.notify();
                        }),
                    ),
            )
            .child(
                div()
                    .h(px(layout::VIEW_HEADER_H))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[1]))
                    .px(px(theme::SPACE[4]))
                    .text_size(px(text::SM.0))
                    .line_height(px(text::SM.1))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child({
                        let (errors, warnings) = self.problem_counts();
                        panel_tab("problems-tab", crate::i18n::t("Problems").into(), self.chrome.bottom == Bottom::Problems, errors + warnings, p).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.chrome.bottom = Bottom::Problems;
                                cx.notify();
                            }),
                        )
                    })
                    .child(panel_tab("output-tab", crate::i18n::t("Output").into(), self.chrome.bottom == Bottom::Output, 0, p).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.chrome.bottom = Bottom::Output;
                            cx.notify();
                        },
                    )))
                    .when(self.dbg.has_sessions(), |d| {
                        d.child(panel_tab("debug-tab", crate::i18n::t("Debug Console").into(), self.chrome.bottom == Bottom::Debug, 0, p).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.chrome.bottom = Bottom::Debug;
                                cx.notify();
                            }),
                        ))
                    })
                    .child(panel_tab("ports-tab", crate::i18n::t("Ports").into(), self.chrome.bottom == Bottom::Ports, self.active_forwards(), p).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.chrome.bottom = Bottom::Ports;
                            cx.notify();
                        }),
                    ))
                    .child(panel_tab("terminal-tab", crate::i18n::t("Terminal").into(), self.chrome.bottom == Bottom::Terminal, 0, p).on_click(cx.listener(
                        |this, _, window, cx| {
                            if this.terminals.is_empty() {
                                this.new_terminal(&NewTerminal, window, cx);
                            } else {
                                this.focus_terminal(window, cx);
                            }
                        },
                    )))
                    .children(self.ext_view_panel_tabs(p, cx))
                    .child(div().flex_1())
                    // The terminal's own actions only while the terminal is the panel's view.
                    .when(self.chrome.bottom == Bottom::Terminal, |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .child(
                                    ui::icon_button("panel-new", "tb-plus", crate::i18n::t("New Terminal (⌃⇧`)"), p)
                                        .on_click(cx.listener(|this, _, w, cx| this.new_terminal(&NewTerminal, w, cx))),
                                )
                                .child(ui::icon_button("panel-launch", "tb-dropdown", crate::i18n::t("Launch Profile…"), p).w(px(16.)).on_click(cx.listener(
                                    |this, ev: &gpui::ClickEvent, w, cx| {
                                        // Below the button, right-aligned with it (a keyboard press has no position: under the pointer then).
                                        let at = ev.position();
                                        let at = gpui::point(at.x + px(8.), at.y + px(14.));
                                        this.open_terminal_launch_menu(at, w, cx);
                                    },
                                ))),
                        )
                        .child(
                            ui::icon_button("panel-split", "tb-split", crate::i18n::t("Split Terminal"), p)
                                .on_click(cx.listener(|this, _, w, cx| this.split_terminal(&SplitTerminal, w, cx))),
                        )
                        .child(
                            ui::icon_button("panel-kill", "tb-trash", crate::i18n::t("Kill Terminal"), p)
                                .on_click(cx.listener(|this, _, w, cx| this.kill_terminal(&KillTerminal, w, cx))),
                        )
                        .child(div().w(px(1.)).h(px(16.)).mx(px(theme::SPACE[2])).bg(c(p.border)))
                    })
                    .child(
                        ui::icon_button(
                            "panel-move",
                            if self.chrome.panel_right { "tb-panel-bottom" } else { "tb-panel-right" },
                            if self.chrome.panel_right { crate::i18n::t("Move Panel to Bottom") } else { crate::i18n::t("Move Panel Right") },
                            p,
                        )
                        .on_click(cx.listener(|this, _, w, cx| this.dispatch_now(&TogglePanelPosition, w, cx))),
                    )
                    .child(
                        ui::toggle_button(
                            "panel-max",
                            if self.chrome.panel_max { "tb-restore" } else { "tb-maximize" },
                            if self.chrome.panel_max { crate::i18n::t("Restore Panel Size") } else { crate::i18n::t("Maximize Panel") },
                            self.chrome.panel_max,
                            p,
                        )
                        .on_click(cx.listener(|this, _, w, cx| this.dispatch_now(&ToggleMaximizedPanel, w, cx))),
                    )
                    .child(
                        ui::icon_button("panel-close", "tb-close", crate::i18n::t("Hide Panel (⌘J)"), p)
                            .on_click(cx.listener(|this, _, w, cx| this.toggle_panel(&TogglePanel, w, cx))),
                    ),
            )
            .child(div().flex_1().min_h_0().flex().map(|d| {
                match self.chrome.bottom {
                    Bottom::Problems => d.child(self.problems_view(p, cx)),
                    Bottom::Output => d.child(self.output_view(p)),
                    Bottom::Debug => d.child(self.debug_view(p, cx)),
                    Bottom::Ports => d.child(self.ports_view(p, cx)),
                    Bottom::ExtView => d.child(self.ext_view_in(ext_manifest::ViewLocation::Bottom, p, cx)),
                    Bottom::Terminal => {
                        // Several terminals: their list on the right.
                        let list = self.terminal_list(p, cx);
                        let panes = match (self.shown_split(), self.terminals.get(self.active_terminal).cloned()) {
                            (Some((left, right)), _) => vec![left, right],
                            (None, active) => active.into_iter().collect(),
                        };
                        d.child(div().flex_1().min_w_0().min_h_0().flex().children(panes.into_iter().enumerate().map(|(i, t)| {
                            div()
                                .flex_1()
                                .min_w_0()
                                .min_h_0()
                                .flex()
                                .when(i > 0, |d| d.child(div().w(px(1.)).h_full().flex_none().bg(c(p.border))))
                                .child(div().flex_1().min_w_0().min_h_0().child(t))
                        })))
                        .children(list)
                    }
                }
            }))
    }

    /// `--do`: run the command by name. `shortcut:<name>` first opens the shortcuts page and runs
    /// the command from there as if clicking the row (to verify in a real window).
    pub fn run_command_label(&mut self, label: &str, window: &mut Window, cx: &mut Context<Self>) {
        // `dump:` writes the active editor's path, cursor and text to stderr (to read the result in a real window).
        if label == "dump:" {
            if let Some(e) = self.active_editor() {
                let e = e.read(cx);
                eprintln!("DUMP path={} cursor={:?} text={:?}", e.path, e.cursor_position(), e.text_lines().join("\n"));
            } else {
                eprintln!("DUMP no active editor");
            }
            return;
        }
        if let Some(name) = label.strip_prefix("shortcut:") {
            self.show_shortcuts_page(&ShowShortcutsPage, window, cx);
            if let Some(Page::Shortcuts(v)) = self.tabs.page.clone() {
                let name = name.to_string();
                v.update(cx, |_, cx| cx.emit(crate::shortcuts_page::ShortcutsEvent::Run(name)));
            }
        } else if let Some(c) = self.commands().into_iter().find(|c| c.label == label) {
            self.run_command(c.run, window, cx);
        } else {
            eprintln!("--do: no command named {label:?}");
        }
    }

    fn run_command(&mut self, run: Run, window: &mut Window, cx: &mut Context<Self>) {
        match run {
            // Run after focus is restored: editor-specific commands (Save) must reach the editor.
            Run::Action(action) => window.defer(cx, move |window, cx| window.dispatch_action(action, cx)),
            Run::Connect(alias) => self.connect(alias, window, cx),
            Run::Disconnect => self.disconnect(cx),
            Run::Extension { extension, command } => self.run_extension_command(&extension, &command, window, cx),
            Run::ExtensionView(view) => self.open_ext_view(&view, window, cx),
        }
    }

    /// All editors in both groups (focused group first; one open in both counts once).
    fn all_editors(&self) -> Vec<Entity<EditorView>> {
        let mut v = self.tabs.editors.clone();
        for g in self.tabs.groups.values() {
            for ed in &g.editors {
                if !v.contains(ed) {
                    v.push(ed.clone());
                }
            }
        }
        v
    }

    // --- Navigation history, closed tabs, new file, save as ---

    pub fn any_dirty(&self, cx: &App) -> bool {
        self.all_editors().iter().any(|e| e.read(cx).is_dirty())
    }

    fn active_editor(&self) -> Option<&Entity<EditorView>> {
        self.tabs.active.and_then(|i| self.tabs.editors.get(i))
    }

    /// Git mark of a file (whichever root's tree it is in); `None` if unmarked.
    pub(super) fn git_mark_of(&self, path: &str) -> Option<crate::git::Mark> {
        self.vcs.tree_marks.values().find_map(|m| m.get(path).copied())
    }

    fn refuse_if_dirty(&mut self, cx: &mut Context<Self>) -> bool {
        if self.any_dirty(cx) {
            // Can't save while disconnected: saying "save first" would be a dead end, so point the way.
            let down = matches!(self.remote.conn, Conn::Failed { .. } | Conn::Reconnecting { .. });
            self.notice = Some(Notice::Error(
                if down {
                    crate::i18n::t("The connection is down, so files with unsaved changes cannot be saved. Use File: Save a Copy to This Computer… to keep them, or close them, then reconnect.")
                } else {
                    crate::i18n::t("Save or close the files with unsaved changes first.")
                }
                .into(),
            ));
            cx.notify();
            return true;
        }
        false
    }

    /// Change the root folder (same host).
    pub fn set_root(&mut self, path: String, cx: &mut Context<Self>) {
        let path = if self.remote.host.is_remote() { path } else { ui::local_path(std::path::Path::new(&path)) };
        // Same folder: keep tabs in place. Opened through a workspace file, the folder opens on its own instead.
        if self.root(cx).as_deref() == Some(path.as_str()) && self.roots.workspace.is_none() {
            self.cancel_pending_connection();
            cx.notify();
            return;
        }
        self.replace_root(path, cx);
    }

    /// Replace the workspace even if another host uses the same absolute folder path.
    pub(super) fn replace_root(&mut self, path: String, cx: &mut Context<Self>) {
        // With hot exit on, dirty files are backed up and come back when returning to the folder.
        self.flush_backups(cx);
        if (!cx.global::<Settings>().hot_exit || self.has_unbacked_dirty(cx)) && self.refuse_if_dirty(cx) {
            return;
        }
        self.cancel_pending_connection();
        self.invalidate_file_operation();
        self.stop_web_preview(cx);
        self.persist_session(cx);
        self.shutdown_lsp(cx);
        self.roots.extra_roots.clear();
        self.roots.extra_trees.clear();
        self.roots.pending_extra_roots.clear();
        // The workspace file's settings layer goes with it: the settings are read again.
        self.cfg.dirty |= self.roots.workspace.take().is_some_and(|w| w.doc.settings_json().is_some());
        self.roots.workspace_request += 1;
        self.cfg.folders.clear();
        self.cfg.folder_texts.clear();
        self.persist.pending_restore_workspace = None;
        self.persist.backed_up.clear();
        self.tabs = Default::default();
        self.tabs.active = None;
        self.tabs.open_error = None;
        self.tabs.opening = None;
        self.notice = None;
        self.roots.file_index = None;
        self.load_git_branch(&path, cx);
        self.vcs.status = None;
        self.scm.update(cx, |s, cx| s.set_status(self.remote.host.clone(), None, cx));
        self.vcs.scm_root = None;
        self.reset_repos(cx);
        self.vcs.tree_marks.clear();
        self.roots.primary.update(cx, |t, cx| t.set_git(Arc::default(), cx));
        self.refresh_git_at(&path, 0, cx);
        let (h, r) = (self.remote.host.clone(), Some(path.clone()));
        self.search.update(cx, |s, cx| s.set_context(h, r, cx));
        let host = self.remote.host.clone();
        self.roots.primary.update(cx, |t, cx| t.set_root(host, Some(path.clone()), cx));
        self.discover_repos(cx);
        let alias = match &self.remote.conn {
            Conn::Connected { alias } => Some(alias.as_str()),
            _ => None,
        };
        // Dev connections are not written to history.
        self.persist.workspace_id = None;
        if let Some(s) = self.persist.state.as_ref().filter(|_| alias != Some(LOCAL_AGENT_ALIAS)) {
            self.persist.workspace_id = s.record_workspace(alias, &path).ok();
        }
        self.reload_recent(cx);
        self.persist.pending_restore = self.persist.workspace_id;
        self.restore_tree(cx);
        self.restore_extra_roots(cx);
        self.load_debug_state(cx);
        self.load_favorites();
        self.restore_forwards(cx);
        self.load_project_settings(cx);
        cx.notify();
    }

    /// Project settings files (under the root, possibly on the server): `.vscode/settings.json`
    /// (keys we know) and `.codeyard/settings.jsonc` (the latter wins if both exist).
    fn project_settings_paths(&self, cx: &App) -> Vec<(String, &'static str, bool)> {
        let Some(root) = self.root(cx) else { return Vec::new() };
        vec![
            (ui::join(&root, ".vscode/settings.json"), ".vscode/settings.json", false),
            (ui::join(&root, LEGACY_PROJECT_SETTINGS), LEGACY_PROJECT_SETTINGS, true),
            (ui::join(&root, PROJECT_SETTINGS), PROJECT_SETTINGS, true),
        ]
    }

    fn load_project_settings(&mut self, cx: &mut Context<Self>) {
        let files = self.project_settings_paths(cx);
        let had = !self.cfg.project.is_empty();
        if files.is_empty() {
            self.cfg.project.clear();
            self.cfg.dirty = had;
            return;
        }
        let host = self.remote.host.clone();
        let source = host.clone();
        let generation = self.lsp.generation;
        let task = cx.background_executor().spawn(async move {
            files
                .into_iter()
                .filter_map(|(path, name, warn)| host.read(&path).ok().map(|(d, _)| (name.to_string(), String::from_utf8_lossy(&d).into_owned(), warn)))
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let loaded = task.await;
            let changed = this
                .update(cx, |this, _| {
                    if this.lsp.generation != generation || !Arc::ptr_eq(&this.remote.host, &source) {
                        return None;
                    }
                    let changed = loaded != this.cfg.project || this.cfg.dirty;
                    this.cfg.project = loaded;
                    changed.then_some(this.window_handle)
                })
                .ok()
                .flatten();
            // Apply right away (including indentation of open editors).
            if let Some(handle) = changed {
                cx.update_window(handle, |_, window, cx| {
                    this.update(cx, |this, cx| this.reload_config(window, cx)).ok();
                })
                .ok();
            }
        })
        .detach();
    }

    /// "Preferences: Open Workspace Settings": `.codeyard/settings.jsonc` (created if missing).
    fn open_workspace_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root(cx) else {
            self.notice = Some(Notice::Info(crate::i18n::t("Open a folder first: project settings live in it.").into()));
            cx.notify();
            return;
        };
        let path = ui::join(&root, PROJECT_SETTINGS);
        let host = self.remote.host.clone();
        let p = path.clone();
        let task = cx.background_executor().spawn(async move {
            if host.stat(&p).is_err() {
                host.create_file(&p)?;
                host.write(&p, PROJECT_SETTINGS_TEMPLATE.as_bytes().to_vec(), None)?;
            }
            Ok::<_, host::HostError>(())
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(()) => this.open_file(path, window, cx),
                Err(e) => {
                    this.notice = Some(Notice::Error(crate::i18n::tf("Could not create {0}: {1}", &[&PROJECT_SETTINGS, &e]).into()));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn open_path(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        if crate::workspace_file::is_workspace_file(path) {
            return self.open_workspace_file(path, window, cx);
        }
        let p = std::path::Path::new(path);
        let abs = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        if abs.is_file() {
            if let Some(dir) = abs.parent() {
                self.set_root(dir.to_string_lossy().into_owned(), cx);
            }
            self.open_file(abs.to_string_lossy().into_owned(), window, cx);
        } else {
            self.set_root(abs.to_string_lossy().into_owned(), cx);
        }
    }

    fn add_editor(&mut self, host: Arc<dyn Host>, path: String, loaded: editor::Loaded, window: &mut Window, cx: &mut Context<Self>) -> Entity<EditorView> {
        let path_for_versions = path.clone();
        let ed = cx.new(|cx| EditorView::new(host, path, loaded, cx));
        if self.vcs.status.is_some() && ed.read(cx).uses_host(&self.remote.host) {
            let (root, changed) = git_context(self.vcs.status.as_deref(), &ed.read(cx).path);
            ed.update(cx, |e, cx| e.set_git_root(root, changed, cx));
        }
        this_subscribe(self, &ed, window, cx);
        self.nav.edit_versions.insert(path_for_versions, ed.read(cx).version());
        self.load_bookmarks(&ed, cx);
        self.tabs.editors.push(ed.clone());
        self.attach_lsp(&ed, cx);
        self.apply_task_problems(&ed, cx);
        self.attach_extensions(&ed, window, cx);
        self.schedule_schema(&ed, Duration::ZERO, cx);
        self.attach_speller(&ed, cx);
        self.apply_breakpoints_to(&ed, cx);
        self.apply_folder_settings_to(&ed, cx);
        ed
    }

    fn remember_file(&mut self, path: &str) {
        self.nav.recent_files.retain(|p| p != path);
        self.nav.recent_files.insert(0, path.to_string());
        self.nav.recent_files.truncate(50);
    }

    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.chrome.panel_max = false;
        self.tabs.diff_active = false;
        self.tabs.large_active = false;
        self.tabs.page_active = false;
        self.tabs.active = Some(ix);
        self.sync_slots(cx);
        let path = self.tabs.editors[ix].read(cx).path.clone();
        self.remember_file(&path);
        self.sync_scm_root(cx);
        if cx.global::<Settings>().auto_reveal {
            self.reveal_in_trees(&path, cx);
        }
        if let Some((p, line, col)) = self.lsp.pending_reveal.take()
            && p == path
        {
            let ed = self.tabs.editors[ix].clone();
            self.refresh_active_flags(cx);
            self.tabs.open_error = None;
            ed.update(cx, |e, cx| e.reveal_lsp(line, col, window, cx));
            self.persist_session(cx);
            cx.notify();
            return;
        }
        if let Some((p, line, col, len)) = self.tabs.pending_reveal.take()
            && p == path
        {
            let ed = self.tabs.editors[ix].clone();
            self.refresh_active_flags(cx);
            self.tabs.open_error = None;
            ed.update(cx, |e, cx| e.reveal(line as usize, col as usize, len as usize, window, cx));
            self.persist_session(cx);
            cx.notify();
            return;
        }
        // Only front tabs hold a syntax tree.
        self.refresh_active_flags(cx);
        self.tabs.tab_scroll.scroll_to_item(ix);
        self.tabs.open_error = None;
        let handle = self.tabs.editors[ix].focus_handle(cx);
        window.focus(&handle);
        self.persist_session(cx);
        cx.notify();
    }

    fn close_editor_at(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.tabs.editors.get(ix) else { return };
        if !ed.read(cx).is_dirty() || self.also_in_other(ed) {
            self.remove_editor(ix, window, cx);
            return;
        }
        let name = ed.read(cx).name.clone();
        let answer = window.prompt(
            PromptLevel::Warning,
            &crate::i18n::tf("Do you want to save the changes you made to {}?", &[&name]),
            Some(crate::i18n::t("Your changes will be lost if you don't save them.")),
            &[crate::i18n::t("Save"), crate::i18n::t("Don't Save"), crate::i18n::t("Cancel")],
            cx,
        );
        let ed = ed.downgrade();
        cx.spawn_in(window, async move |this, cx| {
            let choice = answer.await;
            this.update_in(cx, |this, window, cx| {
                let Some(entity) = ed.upgrade() else { return };
                match choice {
                    // Save: closes when the write finishes (and succeeds); stays open on conflict/error.
                    Ok(0) => {
                        let done = std::rc::Rc::new(std::cell::Cell::new(false));
                        cx.subscribe_in(&entity, window, move |this, e, ev: &EditorEvent, window, cx| {
                            if done.get() || !matches!(ev, EditorEvent::Saved) {
                                return;
                            }
                            done.set(true);
                            if let Some(ix) = this.tabs.editors.iter().position(|x| x == e) {
                                this.remove_editor(ix, window, cx);
                            }
                        })
                        .detach();
                        entity.update(cx, |e, cx| e.save_now(cx));
                    }
                    Ok(1) => {
                        if let Some(ix) = this.tabs.editors.iter().position(|e| *e == entity) {
                            this.remove_editor(ix, window, cx);
                        }
                    }
                    _ => {}
                }
            })
            .ok();
        })
        .detach();
    }

    fn remove_editor(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let ed = self.tabs.editors.remove(ix);
        // If also open in the other group, only this view closes; the backup stays.
        if !self.also_in_other(&ed) {
            let path = ed.read(cx).path.clone();
            self.discard_response_file(&path);
            self.drop_backup(&path, cx);
            let (line, col) = ed.read(cx).cursor_position();
            self.nav.closed.retain(|c| c.0 != path);
            self.nav.closed.push((path, line as u32, col.saturating_sub(1) as u32));
            if self.nav.closed.len() > 20 {
                self.nav.closed.remove(0);
            }
        }
        self.sync_slots(cx);
        // When a split group's last tab closes, the group closes too and the other becomes the only one.
        let group_key = self.tabs.current_key();
        if self.tabs.remove_empty(group_key) {
            if let Some(a) = self.tabs.active {
                self.activate(a, window, cx);
            }
            cx.notify();
            return;
        }
        self.tabs.active = match self.tabs.active {
            _ if self.tabs.editors.is_empty() => None,
            Some(a) if a > ix => Some(a - 1),
            Some(a) => Some(a.min(self.tabs.editors.len() - 1)),
            None => None,
        };
        match self.tabs.active {
            Some(a) => self.activate(a, window, cx),
            None => {
                window.focus(&self.roots.primary.focus_handle(cx));
                self.persist_session(cx);
            }
        }
        cx.notify();
    }

    fn close_editor(&mut self, _: &CloseEditor, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Page::Search(ed)) = self.tabs.page.clone().filter(|_| self.tabs.page_active) {
            return self.close_search_editor(&ed, cx);
        }
        if self.tabs.diff_active {
            return self.close_diff(window, cx);
        }
        if self.tabs.large_active {
            return self.close_large(window, cx);
        }
        if let Some(ix) = self.tabs.active {
            self.close_editor_at(ix, window, cx);
        }
    }

    fn open_folder(&mut self, _: &OpenFolder, window: &mut Window, cx: &mut Context<Self>) {
        if self.remote.host.is_remote() {
            return self.open_remote_folder(window, cx);
        }
        let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Open".into()) });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                this.update(cx, |this, cx| this.set_root(path.to_string_lossy().into_owned(), cx)).ok();
            }
        })
        .detach();
    }

    /// Switch between open editors (⌃Tab): most recently used first, first row is the current editor;
    /// ↵ jumps straight to the previous one (to toggle between two files).
    fn switch_editor(&mut self, _: &SwitchEditor, window: &mut Window, cx: &mut Context<Self>) {
        if self.asking() || self.tabs.editors.is_empty() {
            return;
        }
        let rank = |ed: &Entity<EditorView>| self.nav.recent_files.iter().position(|p| *p == ed.read(cx).path).unwrap_or(usize::MAX);
        let mut order: Vec<usize> = (0..self.tabs.editors.len()).collect();
        order.sort_by_key(|&i| rank(&self.tabs.editors[i]));
        let items: Vec<PickItem> = order
            .iter()
            .map(|&i| {
                let e = self.tabs.editors[i].read(cx);
                let detail = self.rel_in_roots(&e.path, cx).map(|r| parent_of(&r)).filter(|d| !d.is_empty());
                let label = if e.is_dirty() { format!("{} ●", e.name) } else { e.name.to_string() };
                PickItem { label: label.into(), detail: detail.map(Into::into), keys: None }
            })
            .collect();
        let initial = Pick::Item { source: 0, index: 1.min(items.len() - 1) };
        if self.overlay.is_some() {
            self.close_overlay(window, cx);
        }
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], "Switch to an open editor", "", Some(initial), cx));
        self.show_overlay(picker, OverlayKind::Switcher(order), window, cx);
    }

    /// Zen mode: sidebar, panel, activity bar and status bar are hidden, full screen.
    fn toggle_zen(&mut self, _: &ToggleZenMode, window: &mut Window, cx: &mut Context<Self>) {
        match self.chrome.zen.take() {
            Some(saved) => {
                self.chrome.panel = saved.panel;
                self.chrome.panel_open = saved.panel_open;
                if !saved.was_fullscreen && window.is_fullscreen() {
                    window.toggle_fullscreen();
                }
            }
            None => {
                let was_fullscreen = window.is_fullscreen();
                self.chrome.zen = Some(ZenSaved { panel: self.chrome.panel, panel_open: self.chrome.panel_open, was_fullscreen });
                self.chrome.panel = None;
                self.chrome.panel_open = false;
                if !was_fullscreen {
                    window.toggle_fullscreen();
                }
            }
        }
        if let Some(ed) = self.active_editor() {
            window.focus(&ed.focus_handle(cx));
        }
        cx.notify();
    }

    fn toggle_full_screen(&mut self, _: &ToggleFullScreen, window: &mut Window, _: &mut Context<Self>) {
        window.toggle_fullscreen();
    }

    /// "Search: Find TODO Comments": lines containing TODO, FIXME, HACK, XXX, BUG.
    fn find_todos(&mut self, _: &FindTodos, window: &mut Window, cx: &mut Context<Self>) {
        self.chrome.panel = Some(Panel::Search);
        self.search.update(cx, |s, cx| s.search_todos(window, cx));
        cx.notify();
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.chrome.panel = match self.chrome.panel {
            Some(_) => None,
            None => Some(Panel::Explorer),
        };
        cx.notify();
    }

    fn show_explorer(&mut self, _: &ShowExplorer, window: &mut Window, cx: &mut Context<Self>) {
        self.chrome.panel = Some(Panel::Explorer);
        window.focus(&self.roots.primary.focus_handle(cx));
        cx.notify();
    }

    /// "Connect to SSH Host…", the status bar indicator, the welcome screen: all open the workspace switcher.
    fn show_remote(&mut self, _: &ShowRemote, window: &mut Window, cx: &mut Context<Self>) {
        self.show_project_switcher(window, cx);
    }

    fn quit(&mut self, _: &Quit, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_quit(window, cx);
    }

    /// Replace in files without review (tests drive the shared `apply_file_edits` path with it; the UI goes through a
    /// Search Editor preview).
    #[cfg(test)]
    pub fn replace_in_files(&mut self, files: Vec<String>, query: proto::SearchQuery, replacement: String, cx: &mut Context<Self>) {
        self.replace_in_files_with(files, query, replacement, false, cx)
    }

    #[cfg(test)]
    pub fn replace_in_files_with(&mut self, files: Vec<String>, query: proto::SearchQuery, replacement: String, preserve: bool, cx: &mut Context<Self>) {
        let re = match agent::search::compile(&query) {
            Ok(re) => re,
            Err(e) => {
                self.notice = Some(Notice::Error(e.to_string().into()));
                cx.notify();
                return;
            }
        };
        let regex = query.regex;
        let edits_for: replace_glue::EditsFor = Arc::new(move |_path, b| {
            crate::find::find_all(b, &re)
                .into_iter()
                .map(|m| {
                    let mut t = crate::find::replacement_for(b, &re, &m, &replacement, regex);
                    if preserve {
                        t = crate::find::preserve_case(&b.text_range(m.clone()), &t);
                    }
                    (m, t)
                })
                .collect()
        });
        self.apply_file_edits(files, edits_for, cx)
    }

    /// Hot exit: back up and quit. Asks if it's off and a dirty file exists.
    /// ⌘Q: all windows are backed up; asks if hot exit is off and any file is dirty.
    pub fn confirm_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_quit_with(update_glue::QuitMode::Quit, window, cx);
    }

    /// Quit, or restart into the version that was just installed: both go through hot exit, so unsaved work survives either.
    fn confirm_quit_with(&mut self, mode: update_glue::QuitMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.hold_quit_for_install(mode.clone(), window, cx) {
            return;
        }
        let me = window.window_handle();
        let mut ok = self.prepare_quit(cx);
        for w in cx.windows().into_iter().filter(|w| *w != me).filter_map(|w| w.downcast::<Workbench>()) {
            ok &= w.update(cx, |wb, _, cx| wb.prepare_quit(cx)).unwrap_or(true);
        }
        if ok {
            self.finish_quit(mode, cx);
            return;
        }
        let (detail, leave) = match mode {
            update_glue::QuitMode::Quit => (crate::i18n::t("Quitting loses everything since the last save."), crate::i18n::t("Quit without saving")),
            update_glue::QuitMode::Restart(_) => (crate::i18n::t("Restarting loses everything since the last save."), crate::i18n::t("Restart without saving")),
        };
        let answer = window.prompt(PromptLevel::Warning, crate::i18n::t("You have unsaved changes"), Some(detail), &[leave, crate::i18n::t("Cancel")], cx);
        cx.spawn(async move |this, cx| {
            if answer.await == Ok(0) {
                this.update(cx, |this, cx| this.finish_quit(mode, cx)).ok();
            }
        })
        .detach();
    }

    /// Close the window (others stay open); asks if there are unsaved changes.
    pub fn confirm_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let answer = window.prompt(
            PromptLevel::Warning,
            crate::i18n::t("You have unsaved changes"),
            Some(crate::i18n::t("Closing the window loses everything since the last save.")),
            &[crate::i18n::t("Close without saving"), crate::i18n::t("Cancel")],
            cx,
        );
        cx.spawn_in(window, async move |_, cx| {
            if answer.await == Ok(0) {
                cx.update(|window, _| window.remove_window()).ok();
            }
        })
        .detach();
    }

    // --- Remote connection ---

    /// Update recents in the Dock menu (no effect outside macOS).
    pub(crate) fn sync_dock_menu(&self, cx: &mut App) {
        crate::menus::remember_recent(&self.persist.recent);
        #[cfg(target_os = "macos")]
        crate::menus::set_dock_menu(cx, &self.persist.recent);
        #[cfg(not(target_os = "macos"))]
        let _ = cx;
    }

    pub(crate) fn open_recent(&mut self, r: RecentWorkspace, window: &mut Window, cx: &mut Context<Self>) {
        // If already open in another window, switch to it instead of making a second copy.
        let me = window.window_handle();
        let key = Some((r.host.clone(), r.path.clone()));
        let open_elsewhere = cx
            .windows()
            .into_iter()
            .filter(|w| *w != me)
            .filter_map(|w| w.downcast::<Workbench>())
            .find(|w| w.read(cx).is_ok_and(|wb| wb.workspace_key(cx) == key));
        if let Some(w) = open_elsewhere {
            w.update(cx, |_, window, _| window.activate_window()).ok();
            return;
        }
        match r.host.clone() {
            None => {
                let workspace_file = crate::workspace_file::is_workspace_file(&r.path);
                // If the folder (or workspace file) was deleted or moved, don't open it; ask whether to remove it from the list.
                if workspace_file && !std::path::Path::new(&r.path).is_file() {
                    return self.ask_forget_unreachable(r, crate::i18n::t("The workspace file no longer exists on this computer.").into(), window, cx);
                }
                if !workspace_file && !std::path::Path::new(&r.path).is_dir() {
                    return self.ask_forget_unreachable(r, "The folder no longer exists on this computer.".into(), window, cx);
                }
                if self.remote.host.is_remote() {
                    self.disconnect(cx);
                    if self.remote.host.is_remote() {
                        return;
                    }
                }
                if workspace_file {
                    return self.open_path(&r.path, window, cx);
                }
                self.set_root(r.path, cx);
            }
            Some(alias) => {
                self.remote.pending_path = Some(r.path.clone());
                // If it can't connect or the folder is missing on the server, ask about removing (`connect` / `attach_host`).
                self.persist.opening_recent = Some(r);
                self.connect(alias, window, cx);
            }
        }
    }

    // --- View parts ---

    /// "● file — folder" (window title and drawn title bar).
    fn title_text(&self, cx: &App) -> Option<String> {
        let root = self.roots.primary.read(cx).root().map(|r| ui::file_name(r).to_string());
        let file = self.active_editor().map(|e| {
            let e = e.read(cx);
            format!("{}{}", if e.is_dirty() { "● " } else { "" }, e.name)
        });
        let parts: Vec<String> = [file, root].into_iter().flatten().collect();
        (!parts.is_empty()).then(|| parts.join(" — "))
    }

    /// Info messages stay in the status bar a few seconds; errors stay as a card until dismissed.
    fn track_notice(&mut self, cx: &mut Context<Self>) {
        const INFO_FOR: Duration = Duration::from_secs(8);
        let current = self.notice.as_ref().map(|n| match n {
            Notice::Info(m) | Notice::Error(m) => m.clone(),
        });
        let id = current.as_ref().map(|m| (m.as_ptr() as usize, m.len()));
        if id == self.notice_seen {
            return;
        }
        self.notice_seen = id;
        self.notice_task = match (&self.notice, current) {
            (Some(Notice::Info(_)), Some(text)) => Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(INFO_FOR).await;
                this.update(cx, |this, cx| {
                    // If another message arrived meanwhile, leave it alone.
                    if matches!(&this.notice, Some(Notice::Info(m)) if *m == text) {
                        this.notice = None;
                        cx.notify();
                    }
                })
                .ok();
            })),
            _ => None,
        };
    }

    /// Error card: bottom right, above the status bar; closed with ×.
    fn error_toast(&self, p: &Palette, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let Some(Notice::Error(message)) = &self.notice else { return None };
        Some(
            div()
                .id("error-toast")
                .absolute()
                .right(px(theme::SPACE[6]))
                .bottom(px(layout::STATUSBAR_H + theme::SPACE[4]))
                .w(px(420.))
                .flex()
                .items_start()
                .gap(px(theme::SPACE[3]))
                .p(px(theme::SPACE[4]))
                .bg(c(p.surface_raised))
                .border_1()
                .border_color(c(p.border))
                .rounded(px(theme::radius::MD))
                .shadow_lg()
                .text_size(px(text::SM.0))
                .text_color(c(p.text))
                .occlude()
                .child(div().flex_none().pt(px(1.)).child(icon("circle-x", 16., p.danger)))
                .child(div().flex_1().min_w_0().child(message.clone()))
                .when(matches!(self.remote.conn, Conn::Failed { .. }), |d| {
                    d.child(
                        div()
                            .id("error-toast-reconnect")
                            .flex_none()
                            .h(px(22.))
                            .px(px(theme::SPACE[4]))
                            .flex()
                            .items_center()
                            .rounded(px(theme::radius::SM))
                            .bg(c(p.accent))
                            .text_color(c(p.on_accent))
                            .text_size(px(text::XS.0))
                            .cursor_pointer()
                            .hover(|d| d.opacity(0.9))
                            .child(crate::i18n::t("Reconnect"))
                            .on_click(cx.listener(|this, _, window, cx| this.reconnect_lost(window, cx))),
                    )
                })
                .child(
                    div()
                        .id("error-toast-close")
                        .flex_none()
                        .size(px(20.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(theme::radius::SM))
                        .cursor_pointer()
                        .hover(|d| d.bg(c(p.surface_hover)))
                        .child(icon("x", 14., p.text_muted))
                        .tooltip(|_, cx| cx.new(|_| Tooltip(crate::i18n::t("Close").into())).into())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.notice = None;
                            cx.notify();
                        })),
                ),
        )
    }

    /// Title layout: `● file — folder — ssh-host` (● unsaved).
    fn update_title(&mut self, window: &mut Window, cx: &mut App) {
        let remote = self.remote.host.is_remote().then(|| self.remote.host.label());
        let parts: Vec<String> = [self.title_text(cx), remote].into_iter().flatten().collect();
        let title = if parts.is_empty() { "codeyard".to_string() } else { parts.join(" — ") };
        if title != self.title {
            window.set_window_title(&title);
            self.title = title;
            // The Dock menu lists the open windows by title.
            #[cfg(target_os = "macos")]
            crate::menus::refresh_dock_windows(cx);
        }
    }

    /// The window's title as last set (the Dock menu's window rows).
    #[cfg(target_os = "macos")]
    pub(crate) fn window_title(&self) -> &str {
        &self.title
    }
}

/// For a file: (repo root, whether git sees it as changed).
fn git_context(st: Option<&crate::git::Status>, path: &str) -> (Option<String>, bool) {
    let Some(st) = st else { return (None, false) };
    let prefix = format!("{}/", st.root.trim_end_matches('/'));
    let changed = path.strip_prefix(&prefix).is_some_and(|rel| st.changes.iter().any(|c| c.path == rel));
    (Some(st.root.clone()), changed)
}

/// Editor events: refresh the UI and schedule a backup on change; save on onFocusChange.
fn this_subscribe(wb: &mut Workbench, ed: &Entity<EditorView>, window: &mut Window, cx: &mut Context<Workbench>) {
    wb._subs.push(cx.subscribe_in(ed, window, |this, ed, ev: &EditorEvent, window, cx| {
        match ev {
            EditorEvent::Saved => {
                if let Some(server) = &this.chrome.web_preview.server
                    && ed.read(cx).uses_host(&this.remote.host)
                {
                    server.changed(&ed.read(cx).path);
                }
                this.reload_snippets_if_edited(ed, cx);
                this.refresh_git(200, cx);
                this.record_history(ed, cx);
            }
            EditorEvent::Overwrote(text) => return this.keep_overwritten(ed, text, cx),
            EditorEvent::OpenLocation { path, line, col } => {
                return this.open_location(path.clone(), *line, *col, window, cx);
            }
            EditorEvent::Log(line) => {
                this.log_output(line.chars().take(2000).collect());
                cx.notify();
                return;
            }
            EditorEvent::Message(m) => {
                this.notice = Some(Notice::Info(m.clone().into()));
                cx.notify();
                return;
            }
            EditorEvent::ShowLocations { locs, what } => {
                return this.show_locations(locs.clone(), what, window, cx);
            }
            EditorEvent::BreakpointsChanged => {
                this.on_breakpoints_changed(ed, cx);
                return;
            }
            EditorEvent::BookmarksChanged => {
                this.save_bookmarks(ed, cx);
                cx.notify();
                return;
            }
            EditorEvent::ContextMenu(position) => return this.open_editor_menu(*position, window, cx),
            EditorEvent::OpenMergeEditor => return this.open_merge_editor(&OpenMergeEditor, window, cx),
            EditorEvent::SendRequest(line) => return this.send_request_at(Some(*line), window, cx),
            EditorEvent::Changed => {
                // The user moved on: what extensions were asked about the old text is cancelled.
                this.cancel_providers_about(&ed.read(cx).path.clone());
                // Breakpoints shift with edits: keep the store (used on close and saved) current.
                this.sync_breakpoints_from(ed, cx);
                this.note_edit(ed, cx);
                this.schedule_schema(ed, Duration::from_millis(600), cx);
            }
        }
        if ed.read(cx).is_dirty() && ed.read(cx).preview {
            ed.update(cx, |e, _| e.preview = false);
        }
        if ed.read(cx).is_dirty() || this.persist.backed_up.contains_key(&ed.read(cx).path) {
            this.schedule_backup(cx);
        }
        cx.notify();
    }));
    let weak = ed.downgrade();
    let focus = ed.focus_handle(cx);
    wb._subs.push(cx.on_blur(&focus, window, move |_, _, cx| {
        if cx.global::<Settings>().auto_save == AutoSave::OnFocusChange {
            weak.update(cx, |e, cx| e.autosave_now(cx)).ok();
        }
    }));
}

fn center(p: &Palette) -> gpui::Div {
    div().size_full().flex().flex_col().items_center().justify_center().gap(px(theme::SPACE[4])).bg(c(p.bg)).text_color(c(p.text))
}

/// Keyboard shortcut badge.
pub(super) fn kbd(k: String, p: &Palette) -> impl IntoElement {
    div()
        .px(px(theme::SPACE[2]))
        .border_1()
        .border_color(c(p.border))
        .rounded(px(theme::radius::SM))
        .text_size(px(text::XS.0))
        .text_color(c(p.text_muted))
        .child(k)
}

pub struct Tooltip(pub SharedString);

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        div()
            .font_family(ui::UI_FONT)
            .text_size(px(text::XS.0))
            .px(px(theme::SPACE[4]))
            .py(px(theme::SPACE[2]))
            .bg(c(p.surface_raised))
            .border_1()
            .border_color(c(p.border))
            .rounded(px(theme::radius::MD))
            .text_color(c(p.text))
            .child(self.0.clone())
    }
}

/// Panel views use quiet button backgrounds for selection and hover.
fn panel_tab(id: &'static str, label: String, active: bool, badge: usize, p: &Palette) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(layout::VIEW_HEADER_H - 8.))
        .flex()
        .items_center()
        .gap(px(theme::SPACE[2]))
        .px(px(theme::SPACE[3]))
        .cursor_pointer()
        .rounded(px(theme::radius::SM))
        .when(active, |d| d.bg(c(p.surface_active)))
        .text_color(c(if active { p.text } else { p.text_muted }))
        .when(!active, |d| d.hover(|d| d.bg(c(p.surface_hover)).text_color(c(p.text))))
        .child(label)
        .when(badge > 0, |d| d.child(div().px(px(theme::SPACE[2])).rounded(px(theme::radius::LG)).bg(c(p.surface_active)).child(badge.to_string())))
}
