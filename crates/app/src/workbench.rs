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
// --- data ---
#[path = "workbench_data.rs"]
mod data_glue;
#[path = "workbench_debug.rs"]
mod debug;
#[path = "workbench_editors.rs"]
mod editors_glue;
#[path = "workbench_extras.rs"]
mod extras_glue;
#[path = "workbench_folder.rs"]
mod folder_glue;
#[path = "workbench_fsops.rs"]
mod fsops_glue;
#[path = "workbench_git.rs"]
mod git_glue;
#[path = "workbench_groups.rs"]
mod groups_glue;
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
#[path = "workbench_ports.rs"]
mod ports_glue;
#[path = "workbench_projects.rs"]
pub mod projects;
#[path = "workbench_pulls.rs"]
mod pulls_glue;
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
#[path = "workbench_tasks.rs"]
pub mod tasks;
#[path = "workbench_terminals.rs"]
mod terminals_glue;
#[cfg(test)]
#[path = "workbench_testing.rs"]
mod testing_glue;
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
#[path = "workbench_welcome.rs"]
mod welcome_glue;

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
        ToggleMaximizedPanel,
        GitCheckout,
        GitCreateBranch,
        RemoteDocker,
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
        ShowSettings,
        ShowKeyboardShortcuts,
        GitFileHistory,
        GitBlameLine,
        FindTodos,
        SwitchEditor,
        ToggleZenMode,
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
    Settings(Entity<crate::settings_page::SettingsPage>),
    Markdown(Entity<crate::markdown::MarkdownPreview>),
    Shortcuts(Entity<crate::shortcuts_page::ShortcutsPage>),
    Merge(Entity<crate::merge::MergeView>),
    // --- compare ---
    Compare(Entity<crate::compare_folders::CompareView>),
}

/// Floating picker on top: theme picker or palette (files / commands / line).
struct Overlay {
    picker: Entity<Picker>,
    kind: OverlayKind,
    return_focus: Option<FocusHandle>,
    _sub: Subscription,
}

pub use host::askpass::{AskKind, AskRequest};
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
#[derive(Clone, Copy, PartialEq, Eq)]
enum Bottom {
    Terminal,
    Problems,
    Output,
    Ports,
    /// Debug: call stack, variables, console.
    Debug,
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

/// The editor group that isn't focused.
struct Group {
    editors: Vec<Entity<EditorView>>,
    active: Option<usize>,
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
    /// "Terminal: Split": the peer shown next to the active terminal.
    term_split: Option<Entity<TerminalView>>,
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
    from_focused: bool,
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
                SearchEvent::OpenInEditor(text) => {
                    this.create_scratch_with("search-results.txt".into(), text, window, cx);
                }
                SearchEvent::ReplaceAll { files, query, replacement, preserve_case, matches, truncated } => {
                    this.confirm_replace_all(files.clone(), query.clone(), replacement.clone(), *preserve_case, *matches, *truncated, window, cx)
                }
                // --- structural ---
                SearchEvent::StructuralBuffers { generation, family } => this.structural_buffers(*generation, family, cx),
                SearchEvent::StructuralReplace { files, family, pattern, replacement, matches } => {
                    this.confirm_structural_replace(files.clone(), family, pattern.clone(), replacement.clone(), *matches, window, cx)
                }
            }),
            cx.subscribe_in(&port_field, window, |this, field, ev: &crate::input::FieldEvent, _, cx| {
                if matches!(ev, crate::input::FieldEvent::Submit) {
                    let text = field.read(cx).text().trim().trim_start_matches("localhost:").to_string();
                    match text.parse::<u16>() {
                        Ok(port) if port > 0 => {
                            field.update(cx, |f, cx| f.set_text("", cx));
                            this.forward_port(port, cx);
                        }
                        _ => {
                            this.notice = Some(Notice::Error(crate::i18n::tf("“{}” is not a port number", &[&text]).into()));
                            cx.notify();
                        }
                    }
                }
            }),
            cx.subscribe_in(&scm, window, |this, _, event: &crate::scm::ScmEvent, window, cx| match event {
                crate::scm::ScmEvent::Refresh => this.refresh_git(0, cx),
                crate::scm::ScmEvent::OpenDiff { path, staged } => this.open_diff(path.clone(), *staged, window, cx),
                crate::scm::ScmEvent::MoreActions(position) => this.open_scm_menu(*position, window, cx),
                // --- git-sync ---
                crate::scm::ScmEvent::Sync => this.git_sync_auto(window, cx),
                crate::scm::ScmEvent::Publish => this.git_publish_branch(cx),
                crate::scm::ScmEvent::PostCommit(then) => this.git_post_commit(*then, cx),
                crate::scm::ScmEvent::OpenCommit(commit) => this.show_commit_files(commit.clone(), window, cx),
                crate::scm::ScmEvent::OpenCommitFile(commit, file) => this.open_commit_file(commit.clone(), file.clone(), window, cx),
                crate::scm::ScmEvent::CommitMenu(commit, position) => this.open_commit_menu(commit.clone(), *position, window, cx),
                crate::scm::ScmEvent::FileMenu(path, position) => this.open_scm_file_menu(path, *position, window, cx),
                crate::scm::ScmEvent::SectionsChanged(text) => {
                    if let Some(s) = &this.persist.state {
                        let _ = s.set("scm_sections", text);
                    }
                }
            }),
            // A download in progress is cancelled and its partial file removed before the process ends.
            cx.on_app_quit(|this, cx| {
                this.discard_response_on_quit();
                let wait = this.cancel_update_for_quit().map(|exited| Workbench::wait_for_update_thread(exited, cx));
                async move {
                    if let Some(wait) = wait {
                        wait.await;
                    }
                }
            }),
            // The first-run update question waits for a quiet keyboard (a letter typed a moment ago must not answer it).
            cx.observe_keystrokes(|this, _, _, _| this.update.last_key = Some(this.update.backend.now())),
            cx.observe_window_appearance(window, |this, window, cx| {
                if this.theme_choice == THEME_AUTO {
                    let choice = this.theme_choice.clone();
                    this.apply_theme(&choice, window, cx);
                }
            }),
        ];
        let (recent, pinned) = state.as_ref().map(projects::load_recent).unwrap_or_default();
        let dev = std::env::var_os("CODEYARD_DEV").is_some() || cfg!(debug_assertions);
        let mut themes = themes::builtin();
        let mut notice = None;
        if let Some(dir) = user_theme_dir() {
            let (user, errors) = themes::load_dir(&dir);
            themes.extend(user);
            if let Some((path, error)) = errors.first() {
                let reason = theme_glue::theme_load_error(path, error);
                notice = Some(Notice::Error(crate::i18n::tf("Theme skipped: {}", &[&reason]).into()));
            }
        }
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
            term_split: None,
            active_terminal: 0,
            shell: None,
            dbg: state_glue::DebugUi {
                session: None,
                generation: 0,
                watch_exprs: Vec::new(),
                exception_filters: None,
                bp_cache: Default::default(),
                last: None,
            },
            jobs: state_glue::TaskState { last: None, terminal: None, root: None, problems: Vec::new(), scan_pending: false },
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
            },
            chrome: state_glue::ChromeLayout {
                panel: Some(Panel::Explorer),
                bottom: Bottom::Terminal,
                sidebar_w,
                resizing_sidebar: false,
                panel_open: false,
                panel_h,
                panel_right,
                panel_w,
                panel_max: false,
                resizing_panel: false,
                split_ratio: 0.5,
                resizing_split: false,
                zoom: 0.,
                zoom_base: (theme::text::CODE.0, theme::text::CODE.1, theme::text::CODE.0),
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
                stamp: (None, None),
                theme_override: false,
                project: Vec::new(),
                trusted: Default::default(),
                dirty: false,
                folders: Default::default(),
            },
            persist: state_glue::PersistState {
                workspace_id: None,
                data_dir: None,
                pending_restore: None,
                backup_task: None,
                backed_up: Default::default(),
                state,
                recent,
                pinned,
                opening_recent: None,
            },
            remote: state_glue::RemoteState {
                local_host: host.clone(),
                host,
                conn: Conn::Local,
                pending_path: None,
                ssh_hosts: host::ssh::list_hosts(),
                bundle: Arc::new(crate::bundled_agents::discover()),
                asks: Default::default(),
                transfer: None,
            },
            roots: state_glue::RootsState {
                primary: tree,
                extra_roots: Vec::new(),
                extra_trees: Vec::new(),
                pending_extra_roots: Vec::new(),
                file_clip: None,
                compare_a: None,
                // --- compare ---
                compare_folder_a: None,
                favorites: Vec::new(),
                file_index: None,
            },
            tabs: state_glue::TabsState {
                editors: Vec::new(),
                other: None,
                focused_left: true,
                active: None,
                opening: None,
                open_error: None,
                diff: None,
                diff_active: false,
                large: None,
                large_active: false,
                page: None,
                page_active: false,
                _page_sub: None,
                pending_reveal: None,
                tab_scroll: gpui::ScrollHandle::new(),
            },
            update: state_glue::UpdateUi::new(dev, cx),
            data: Default::default(),
            focus_handle: cx.focus_handle(),
            _subs: subs,
        };
        wb.apply_theme(&theme_choice, window, cx);
        wb.reload_config(window, cx);
        wb.watch_config(window, cx);
        wb.poll_git(window, cx);
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
            .relative()
            .flex()
            .flex_col()
            .bg(c(p.bg))
            .border_color(c(p.border))
            .map(|d| match (max, right) {
                (true, _) => d.flex_1().min_h_0().min_w_0(),
                (false, true) => d.w(px(self.chrome.panel_w)).flex_none().h_full().border_l_1(),
                (false, false) => d.h(px(self.chrome.panel_h)).flex_none().border_t_1(),
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
                    .when(resizing, |d| d.bg(c(p.accent)))
                    .hover(|d| d.bg(c(p.accent)))
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
                    .text_size(px(text::XXS.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child({
                        let (errors, warnings) = self.problem_counts();
                        panel_tab("problems-tab", crate::i18n::upper(crate::i18n::t("Problems")), self.chrome.bottom == Bottom::Problems, errors + warnings, p)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.chrome.bottom = Bottom::Problems;
                                cx.notify();
                            }))
                    })
                    .child(panel_tab("output-tab", crate::i18n::upper(crate::i18n::t("Output")), self.chrome.bottom == Bottom::Output, 0, p).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.chrome.bottom = Bottom::Output;
                            cx.notify();
                        }),
                    ))
                    .when(self.dbg.session.is_some(), |d| {
                        d.child(panel_tab("debug-tab", crate::i18n::upper(crate::i18n::t("Debug")), self.chrome.bottom == Bottom::Debug, 0, p).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.chrome.bottom = Bottom::Debug;
                                cx.notify();
                            }),
                        ))
                    })
                    .child(
                        panel_tab("ports-tab", crate::i18n::upper(crate::i18n::t("Ports")), self.chrome.bottom == Bottom::Ports, self.ports.forwards.len(), p)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.chrome.bottom = Bottom::Ports;
                                cx.notify();
                            })),
                    )
                    .child(panel_tab("terminal-tab", crate::i18n::upper(crate::i18n::t("Terminal")), self.chrome.bottom == Bottom::Terminal, 0, p).on_click(
                        cx.listener(|this, _, window, cx| {
                            if this.terminals.is_empty() {
                                this.new_terminal(&NewTerminal, window, cx);
                            } else {
                                this.focus_terminal(window, cx);
                            }
                        }),
                    ))
                    .child(div().flex_1())
                    .child(
                        ui::icon_button("panel-new", "plus", crate::i18n::t("New Terminal (⌃⇧`)"), p)
                            .on_click(cx.listener(|this, _, w, cx| this.new_terminal(&NewTerminal, w, cx))),
                    )
                    .child(
                        ui::icon_button("panel-kill", "trash", crate::i18n::t("Kill Terminal"), p)
                            .on_click(cx.listener(|this, _, w, cx| this.kill_terminal(&KillTerminal, w, cx))),
                    )
                    .child(
                        ui::icon_button(
                            "panel-move",
                            if self.chrome.panel_right { "panel-bottom" } else { "panel-right" },
                            if self.chrome.panel_right { crate::i18n::t("Move Panel to Bottom") } else { crate::i18n::t("Move Panel Right") },
                            p,
                        )
                        .on_click(cx.listener(|this, _, w, cx| this.dispatch_now(&TogglePanelPosition, w, cx))),
                    )
                    .child(
                        ui::icon_button(
                            "panel-max",
                            if self.chrome.panel_max { "minimize-2" } else { "maximize-2" },
                            if self.chrome.panel_max { crate::i18n::t("Restore Panel Size") } else { crate::i18n::t("Maximize Panel") },
                            p,
                        )
                        .on_click(cx.listener(|this, _, w, cx| this.dispatch_now(&ToggleMaximizedPanel, w, cx))),
                    )
                    .child(
                        ui::icon_button("panel-close", "x", crate::i18n::t("Hide Panel (⌘J)"), p)
                            .on_click(cx.listener(|this, _, w, cx| this.toggle_panel(&TogglePanel, w, cx))),
                    ),
            )
            .child(div().flex_1().min_h_0().flex().map(|d| {
                match self.chrome.bottom {
                    Bottom::Problems => d.child(self.problems_view(p, cx)),
                    Bottom::Output => d.child(self.output_view(p)),
                    Bottom::Debug => d.child(self.debug_view(p, cx)),
                    Bottom::Ports => d.child(self.ports_view(p, cx)),
                    Bottom::Terminal => {
                        // Multiple terminals: list on the right (like VS Code), name and close.
                        let list = (self.terminals.len() > 1).then(|| {
                            div().id("terminal-list").w(px(180.)).flex_none().h_full().border_l_1().border_color(c(p.border)).overflow_y_scroll().children(
                                self.terminals.iter().enumerate().map(|(i, t)| {
                                    let active = i == self.active_terminal;
                                    let t = t.read(cx);
                                    let label = if t.has_exited() { crate::i18n::tf("{} (exited)", &[&t.title]) } else { t.title.to_string() };
                                    let group: SharedString = format!("term-row-{i}").into();
                                    div()
                                        .id(("term-tab", i))
                                        .group(group.clone())
                                        .w_full()
                                        .h(px(layout::ROW_H))
                                        .flex()
                                        .items_center()
                                        .gap(px(theme::SPACE[2]))
                                        .px(px(theme::SPACE[4]))
                                        .cursor_pointer()
                                        .text_size(px(text::XS.0))
                                        .when(active, |d| d.bg(c(p.surface_active)))
                                        .hover(|d| d.bg(c(p.surface_hover)))
                                        .text_color(c(if active { p.text } else { p.text_muted }))
                                        .child(icon("terminal", 14., if active { p.text } else { p.text_muted }))
                                        .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(label))
                                        .child(
                                            div()
                                                .id(("term-close", i))
                                                .opacity(0.)
                                                .group_hover(group, |s| s.opacity(1.))
                                                .child(icon("x", 12., p.text_muted))
                                                .on_click(cx.listener(move |this, _, window, cx| {
                                                    cx.stop_propagation();
                                                    this.close_terminal(i, window, cx);
                                                })),
                                        )
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.active_terminal = i;
                                            this.chrome.bottom = Bottom::Terminal;
                                            this.focus_terminal(window, cx);
                                        }))
                                }),
                            )
                        });
                        let active = self.terminals.get(self.active_terminal).cloned();
                        let other = self.term_split.clone().filter(|o| Some(o) != active.as_ref() && self.terminals.contains(o));
                        d.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .min_h_0()
                                .flex()
                                .when_some(other, |d, o| {
                                    d.child(div().flex_1().min_w_0().min_h_0().child(o)).child(div().w(px(1.)).h_full().flex_none().bg(c(p.border)))
                                })
                                .child(div().flex_1().min_w_0().min_h_0().children(active)),
                        )
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
        }
    }

    /// All editors in both groups (focused group first; one open in both counts once).
    fn all_editors(&self) -> Vec<Entity<EditorView>> {
        let mut v = self.tabs.editors.clone();
        if let Some(g) = &self.tabs.other {
            v.extend(g.editors.iter().filter(|e| !self.tabs.editors.contains(e)).cloned());
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
        // Same folder: keep tabs in place.
        if self.root(cx).as_deref() == Some(path.as_str()) {
            return;
        }
        // With hot exit on, dirty files are backed up and come back when returning to the folder.
        self.flush_backups(cx);
        if (!cx.global::<Settings>().hot_exit || self.has_unbacked_dirty(cx)) && self.refuse_if_dirty(cx) {
            return;
        }
        self.persist_session(cx);
        self.shutdown_lsp(cx);
        self.roots.extra_roots.clear();
        self.roots.extra_trees.clear();
        self.roots.pending_extra_roots.clear();
        self.cfg.folders.clear();
        self.persist.backed_up.clear();
        self.tabs.editors.clear();
        self.tabs.other = None;
        self.tabs.focused_left = true;
        self.tabs.active = None;
        self.tabs.open_error = None;
        self.notice = None;
        self.roots.file_index = None;
        self.load_git_branch(&path, cx);
        self.vcs.status = None;
        self.vcs.scm_root = None;
        self.vcs.tree_marks.clear();
        self.roots.primary.update(cx, |t, cx| t.set_git(Arc::default(), cx));
        self.refresh_git_at(&path, 0, cx);
        let (h, r) = (self.remote.host.clone(), Some(path.clone()));
        self.search.update(cx, |s, cx| s.set_context(h, r, cx));
        let host = self.remote.host.clone();
        self.roots.primary.update(cx, |t, cx| t.set_root(host, Some(path.clone()), cx));
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
        self.load_watches();
        self.load_exception_filters();
        self.load_favorites();
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
                    let changed = loaded != this.cfg.project;
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

    fn open_file(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.remote.host.clone();
        self.open_file_on(host, path, window, cx);
    }

    fn open_file_on(&mut self, host: Arc<dyn Host>, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.open_file_as(host, path, false, window, cx);
    }

    /// `preview`: preview tab. Opens in place of a non-dirty preview tab if there is one
    /// (so tabs don't pile up while browsing).
    fn open_file_as(&mut self, host: Arc<dyn Host>, path: String, preview: bool, window: &mut Window, cx: &mut Context<Self>) {
        let path = if host.is_remote() { path } else { ui::local_path(std::path::Path::new(&path)) };
        if let Some(format) = crate::large::image_format(&path) {
            return self.open_image(path, format, window, cx);
        }
        if self.active_editor().is_some_and(|e| e.read(cx).path != path) {
            self.record_nav(cx);
        }
        // If open in the other group, the same document shows in this group too (no second copy is loaded).
        if !self.tabs.editors.iter().any(|e| e.read(cx).path == path)
            && let Some(ed) = self.tabs.other.as_ref().and_then(|g| g.editors.iter().find(|e| e.read(cx).path == path)).cloned()
        {
            ed.update(cx, |e, _| e.preview = e.preview && preview);
            self.tabs.editors.push(ed);
            let ix = self.tabs.editors.len() - 1;
            self.activate(ix, window, cx);
            return;
        }
        if let Some(ix) = self.tabs.editors.iter().position(|e| e.read(cx).path == path) {
            if !preview {
                self.tabs.editors[ix].update(cx, |e, _| e.preview = false);
            }
            self.activate(ix, window, cx);
            return;
        }
        self.tabs.opening = Some(path.clone());
        self.tabs.open_error = None;
        let editor_host = host.clone();
        let p = path.clone();
        let fallback = cx.global::<Settings>().fallback_encoding;
        let task = cx.background_executor().spawn(async move { editor::load(host.as_ref(), &p, fallback) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                if this.tabs.opening.as_deref() != Some(path.as_str()) {
                    return; // something else was opened meanwhile
                }
                this.tabs.opening = None;
                match result {
                    Ok(loaded) => {
                        let reuse = preview
                            .then(|| {
                                this.tabs.editors.iter().position(|e| {
                                    let e = e.read(cx);
                                    e.preview && !e.is_dirty()
                                })
                            })
                            .flatten();
                        let ed = this.add_editor(editor_host, path, loaded, window, cx);
                        ed.update(cx, |e, _| e.preview = preview);
                        // Mixed line endings collapse to one style on save (can cause a big git diff): warn beforehand.
                        let (mixed, name) = ed.read_with(cx, |e, _| (e.has_mixed_line_endings(), e.line_ending_name()));
                        if mixed {
                            this.notice = Some(Notice::Info(
                                crate::i18n::tf(
                                    "This file mixes line endings; saving writes all of them as {}, like its first line. Change it in the status bar.",
                                    &[&name],
                                )
                                .into(),
                            ));
                        }
                        let ix = match reuse {
                            Some(old) => {
                                // Take the preview tab's place (same position).
                                let new = this.tabs.editors.pop().expect("just pushed");
                                this.tabs.editors[old] = new;
                                old
                            }
                            None => this.tabs.editors.len() - 1,
                        };
                        this.activate(ix, window, cx);
                    }
                    // Too big to read in one go: read-only view loaded in pieces.
                    Err(editor::OpenError::Host(host::HostError::Op(proto::Error::TooLarge { .. }))) => {
                        this.open_large(path, window, cx);
                    }
                    Err(e) => {
                        this.tabs.open_error = Some((path, e.to_string().into()));
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    // --- Git ---

    /// Refresh git status after `delay` ms (back-to-back events coalesce into one run).
    pub fn refresh_git(&mut self, delay: u64, cx: &mut Context<Self>) {
        if let Some(root) = self.vcs.scm_root.clone().or_else(|| self.root(cx)) {
            self.refresh_git_at(&root, delay, cx);
            // Tree marks of inactive roots stay current too (Source Control only shows the active root).
            for other in self.all_roots(cx).into_iter().filter(|r| *r != root) {
                self.refresh_marks_only(other, cx);
            }
        }
    }

    /// Explorer marks only: one root's `git status`, without touching Source Control.
    fn refresh_marks_only(&mut self, root: String, cx: &mut Context<Self>) {
        if !self.vcs.marks_busy.insert(root.clone()) {
            return;
        }
        let host = self.remote.host.clone();
        let r = root.clone();
        let trusted = self.is_trusted_root(&root, cx);
        let task = cx.background_executor().spawn(async move { crate::git::status_for(host.as_ref(), &r, trusted) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.vcs.marks_busy.remove(&root);
                if !this.all_roots(cx).contains(&root) {
                    return;
                }
                if let Ok(st) = result {
                    let marks = Arc::new(st.map(|s| s.marks()).unwrap_or_default());
                    if this.vcs.tree_marks.get(&root).map(|m| **m != *marks).unwrap_or(!marks.is_empty()) {
                        this.vcs.tree_marks.insert(root.clone(), marks.clone());
                        for t in this.all_trees() {
                            if t.read(cx).root() == Some(root.as_str()) {
                                t.update(cx, |t, cx| t.set_git(marks.clone(), cx));
                            }
                        }
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Source control shows the active file's repo: if the file is in another root, branch and status are refreshed.
    pub(super) fn sync_scm_root(&mut self, cx: &mut Context<Self>) {
        if self.roots.extra_roots.is_empty() {
            return;
        }
        let Some(root) = self.active_root(cx) else { return };
        if self.vcs.scm_root.as_deref().or(self.root(cx).as_deref()) == Some(root.as_str()) {
            return;
        }
        self.vcs.scm_root = Some(root.clone());
        self.load_git_branch(&root, cx);
        self.vcs.error = None;
        self.refresh_git(0, cx);
    }

    fn refresh_git_at(&mut self, dir: &str, delay: u64, cx: &mut Context<Self>) {
        if self.vcs.refreshing {
            self.vcs.refresh_again = true;
            return;
        }
        let (host, dir) = (self.remote.host.clone(), dir.to_string());
        let trusted = self.is_trusted_root(&dir, cx);
        #[cfg(test)]
        let hangs = self.git_status_hangs;
        // A task in its waiting phase is replaced by the newer one (cancelled); a running `git status` isn't.
        self.vcs.refresh = Some(cx.spawn(async move |this, cx| {
            if delay > 0 {
                cx.background_executor().timer(Duration::from_millis(delay)).await;
            }
            if this.update(cx, |this, _| this.vcs.refreshing = true).is_err() {
                return;
            }
            let d = dir.clone();
            let status = cx.background_executor().spawn(async move { crate::git::status_for(host.as_ref(), &d, trusted) });
            #[cfg(test)]
            let status: std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<crate::git::Status>, String>>>> =
                if hangs { Box::pin(futures::future::pending()) } else { Box::pin(status) };
            // Watchdog: if no status arrives (hung git, slow network disk) the lock is released; later
            // refreshes run and the late result isn't applied.
            let timer = cx.background_executor().timer(GIT_STATUS_TIMEOUT);
            let result = match futures::future::select(status, timer).await {
                futures::future::Either::Left((result, _)) => result,
                futures::future::Either::Right(_) => Err(format!("git status did not finish in {} s", GIT_STATUS_TIMEOUT.as_secs())),
            };
            this.update(cx, |this, cx| {
                this.vcs.refreshing = false;
                if std::mem::take(&mut this.vcs.refresh_again) {
                    this.refresh_git(0, cx);
                }
                if !this.all_roots(cx).contains(&dir) {
                    return; // another folder was opened meanwhile
                }
                if this.vcs.scm_root.clone().or_else(|| this.root(cx)).as_deref() != Some(dir.as_str()) {
                    this.refresh_git(0, cx); // the active file's root changed meanwhile
                    return;
                }
                match result {
                    // No change (most polls): no redraw. Every frame makes Metal allocate ~110 MB of
                    // temporary buffers; polling every 4 s kept inflating memory.
                    Ok(st) if this.vcs.error.is_none() && st.as_ref() == this.vcs.status.as_deref() => return,
                    Ok(st) => {
                        this.vcs.error = None;
                        this.apply_git(st.map(Arc::new), cx);
                    }
                    Err(e) => {
                        // Say it only the first time (polling retries every 4 s).
                        if this.vcs.error.is_none() {
                            this.notice = Some(Notice::Error(format!("Git: {e}").into())); // i18n-keep: "Git: {}" would read the same in every language; {e} is git's own text
                        }
                        this.vcs.error = Some(e.into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn apply_git(&mut self, st: Option<Arc<crate::git::Status>>, cx: &mut Context<Self>) {
        let head_changed = st.as_ref().map(|s| (&s.root, &s.head)) != self.vcs.status.as_ref().map(|s| (&s.root, &s.head));
        let marks = Arc::new(st.as_ref().map(|s| s.marks()).unwrap_or_default());
        // Each tree shows the marks of its own root: the new status updates the cache of roots inside the repo
        // (clears the active root's if there is no repo); other roots' marks are kept.
        let scm_dir = self.vcs.scm_root.clone().or_else(|| self.root(cx));
        let repo_root = st.as_ref().map(|s| format!("{}/", s.root.trim_end_matches('/')));
        for r in self.all_roots(cx) {
            let inside = repo_root.as_ref().is_some_and(|p| format!("{}/", r.trim_end_matches('/')).starts_with(p.as_str()));
            if inside {
                self.vcs.tree_marks.insert(r, marks.clone());
            } else if repo_root.is_none() && scm_dir.as_deref() == Some(r.as_str()) {
                self.vcs.tree_marks.remove(&r);
            }
        }
        for t in self.all_trees() {
            let m = t.read(cx).root().and_then(|r| self.vcs.tree_marks.get(r).cloned()).unwrap_or_default();
            t.update(cx, |t, cx| t.set_git(m, cx));
        }
        if let Some(s) = &st {
            self.vcs.branch = s.branch.clone().map(Into::into).or(self.vcs.branch.take());
        }
        // Gutter base is re-read only when HEAD (or the repo) changes.
        if head_changed {
            for ed in self.all_editors() {
                if ed.read(cx).uses_host(&self.remote.host) {
                    let (root, changed) = git_context(st.as_deref(), &ed.read(cx).path);
                    ed.update(cx, |e, cx| e.set_git_root(root, changed, cx));
                }
            }
        }
        let host = self.remote.host.clone();
        self.scm.update(cx, |s, cx| s.set_status(host, st.clone(), cx));
        self.vcs.status = st;
    }

    pub fn git_status(&self) -> Option<Arc<crate::git::Status>> {
        self.vcs.status.clone()
    }

    /// Read `.git/HEAD` through the host (works remotely too); if the root is a
    /// subfolder, looks up at most 6 parent levels.
    fn load_git_branch(&mut self, root: &str, cx: &mut Context<Self>) {
        self.vcs.branch = None;
        let host = self.remote.host.clone();
        let root = root.trim_end_matches('/').to_string();
        let task = cx.background_executor().spawn(async move {
            let mut dir = root;
            for _ in 0..6 {
                if let Ok((data, _)) = host.read(&format!("{dir}/.git/HEAD")) {
                    let head = String::from_utf8_lossy(&data).trim().to_string();
                    return Some(match head.strip_prefix("ref: refs/heads/") {
                        Some(branch) => branch.to_string(),
                        None => head.chars().take(7).collect(), // detached: short sha
                    });
                }
                match dir.rsplit_once('/') {
                    Some((parent, _)) if !parent.is_empty() => dir = parent.to_string(),
                    _ => break,
                }
            }
            None
        });
        cx.spawn(async move |this, cx| {
            let branch = task.await;
            this.update(cx, |this, cx| {
                this.vcs.branch = branch.map(Into::into);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Place restored editors into the saved layout: left/right group, order, active ones.
    fn restore_layout(&mut self, layout: &[(String, u8, bool)], focus_left: bool, window: &mut Window, cx: &mut Context<Self>) {
        let all = self.all_editors();
        let find = |p: &str| all.iter().find(|e| e.read(cx).path == p).cloned();
        let mut groups: [(Vec<Entity<EditorView>>, Option<usize>); 2] = [(Vec::new(), None), (Vec::new(), None)];
        for (path, g, active) in layout {
            let Some(ed) = find(path) else { continue };
            let group = &mut groups[(*g as usize).min(1)];
            if !group.0.contains(&ed) {
                group.0.push(ed);
                if *active {
                    group.1 = Some(group.0.len() - 1);
                }
            }
        }
        // Anything not in the layout (opened from the command line) goes to the left group.
        for ed in all {
            if !groups[0].0.contains(&ed) && !groups[1].0.contains(&ed) {
                groups[0].0.push(ed);
            }
        }
        let [(left, left_active), (right, right_active)] = groups;
        if left.is_empty() && right.is_empty() {
            return;
        }
        let pick = |v: &Vec<Entity<EditorView>>, a: Option<usize>| a.filter(|&a| a < v.len()).or((!v.is_empty()).then(|| v.len() - 1));
        let (la, ra) = (pick(&left, left_active), pick(&right, right_active));
        let split = !left.is_empty() && !right.is_empty();
        let focus_left = !split || focus_left || right.is_empty();
        let (focused, focused_active, other, other_active) = if focus_left || left.is_empty() { (left, la, right, ra) } else { (right, ra, left, la) };
        let (focused, focused_active) = if focused.is_empty() { (other.clone(), other_active) } else { (focused, focused_active) };
        self.tabs.editors = focused;
        self.tabs.other = split.then(|| Group { editors: other, active: other_active });
        self.tabs.focused_left = !split || focus_left;
        if let Some(ix) = focused_active {
            self.activate(ix, window, cx);
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
        if self.tabs.editors.is_empty() && self.tabs.other.is_some() {
            self.swap_groups();
            self.tabs.other = None;
            self.tabs.focused_left = true;
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

    /// Replace in files: can't be undone (closed files are written to disk), so ask first.
    fn confirm_replace_all(
        &mut self,
        files: Vec<String>,
        query: proto::SearchQuery,
        replacement: String,
        preserve: bool,
        matches: u32,
        truncated: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let n = files.len();
        let (m, r) = (matches as usize, &replacement);
        let title = if n == 1 {
            crate::i18n::tn("Replace {0} match in 1 file with “{1}”?", "Replace {0} matches in 1 file with “{1}”?", m, &[r])
        } else {
            crate::i18n::tn("Replace {0} match in {1} files with “{2}”?", "Replace {0} matches in {1} files with “{2}”?", m, &[&n, r])
        };
        let detail = if truncated {
            crate::i18n::t(
                "The search stopped at the result limit: only the files listed are changed. Open files can be undone; closed files are written to disk.",
            )
        } else {
            crate::i18n::t("Open files can be undone with ⌘Z; closed files are written to disk.")
        };
        let answer = window.prompt(PromptLevel::Warning, &title, Some(detail), &[crate::i18n::t("Replace"), crate::i18n::t("Cancel")], cx);
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                this.update(cx, |this, cx| this.replace_in_files_with(files, query, replacement, preserve, cx)).ok();
            }
        })
        .detach();
    }

    pub fn replace_in_files(&mut self, files: Vec<String>, query: proto::SearchQuery, replacement: String, cx: &mut Context<Self>) {
        self.replace_in_files_with(files, query, replacement, false, cx)
    }

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
        // If already open in another window, switch to it instead of making a second copy (like VS Code).
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
                // If the folder was deleted or moved, don't open it; ask whether to remove it from the list.
                if !std::path::Path::new(&r.path).is_dir() {
                    return self.ask_forget_unreachable(r, "The folder no longer exists on this computer.".into(), window, cx);
                }
                if self.remote.host.is_remote() {
                    self.disconnect(cx);
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

    /// VS Code layout: `● file — folder — ssh-host` (● unsaved).
    fn update_title(&mut self, window: &mut Window, cx: &App) {
        let remote = self.remote.host.is_remote().then(|| self.remote.host.label());
        let parts: Vec<String> = [self.title_text(cx), remote].into_iter().flatten().collect();
        let title = if parts.is_empty() { "codeyard".to_string() } else { parts.join(" — ") };
        if title != self.title {
            window.set_window_title(&title);
            self.title = title;
        }
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
                // Breakpoints shift with edits: keep the cache (used on close) current.
                if !this.dbg.bp_cache.is_empty() || !ed.read(cx).breakpoint_lines().is_empty() {
                    this.collect_breakpoints(cx);
                }
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

/// Tab for a view in the panel header (PROBLEMS, PORTS, TERMINAL): underlined when active.
fn panel_tab(id: &'static str, label: String, active: bool, badge: usize, p: &Palette) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h_full()
        .flex()
        .items_center()
        .gap(px(theme::SPACE[2]))
        .px(px(theme::SPACE[3]))
        .cursor_pointer()
        .border_b_1()
        .border_color(c(if active { p.accent } else { p.bg }))
        .text_color(c(if active { p.text } else { p.text_muted }))
        .hover(|d| d.text_color(c(p.text)))
        .child(label)
        .when(badge > 0, |d| d.child(div().px(px(theme::SPACE[2])).rounded(px(theme::radius::LG)).bg(c(p.surface_active)).child(badge.to_string())))
}
