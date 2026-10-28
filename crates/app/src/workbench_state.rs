//! Workbench's field groups: each holds the state of a single area. Related fields are gathered here
//! so `Workbench` stays slim (see docs/architecture.md).

use super::*;

/// Debug session and its settings (see workbench_debug.rs).
#[derive(Default)]
pub(super) struct DebugUi {
    /// Debug sessions (an ended one keeps its console until it is replaced), the one the controls act on, the next
    /// session id (events and answers of a session that is gone are dropped), breakpoints (including closed files; see
    /// debug_model.rs) and the last used configuration.
    pub(super) sessions: Vec<debug::DebugState>,
    pub(super) focused: Option<u64>,
    pub(super) next_id: u64,
    /// Watch expressions (kept across sessions and per project).
    pub(super) watch_exprs: Vec<String>,
    /// Selected exception filters (`None`: none selected yet, use the adapter's defaults).
    pub(super) exception_filters: Option<Vec<String>>,
    pub(super) bps: crate::debug_model::BreakpointStore,
    pub(super) last: Option<String>,
    /// Names of the launch file's configurations (the Run and Debug view's selector).
    pub(super) catalog: Vec<String>,
    /// Debug console inputs (per project) and the one ↑ / ↓ shows.
    pub(super) history: Vec<String>,
    pub(super) history_pos: Option<usize>,
    /// Group of the sessions a compound is starting (id, stop all together), set while they start.
    pub(super) starting_group: Option<(u64, bool)>,
    /// A configuration waiting for the answers to its `${input:…}` questions.
    pub(super) pending_launch: Option<debug::PendingLaunch>,
    /// A debug request waiting for workspace files to save; Stop cancels the pending start.
    pub(super) save_task: Option<gpui::Task<()>>,
    /// Invalidates late discovery/test-build replies when Stop or another preparation supersedes them.
    pub(super) prepare_epoch: u64,
}

/// Last task, plus the problems parsed from its terminal and output (see workbench_tasks.rs).
pub(super) struct TaskState {
    /// Last task that was run ("Tasks: Rerun Last Task").
    pub(super) last: Option<tasks::Task>,
    /// Problems parsed from the last task's terminal and output (they enter the Problems panel together with LSP diagnostics).
    pub(super) terminal: Option<gpui::EntityId>,
    /// Root the last task ran in (compiler paths are resolved against it).
    pub(super) root: Option<String>,
    pub(super) problems: Vec<(String, crate::lsp::Diagnostic)>,
    pub(super) scan_pending: bool,
    /// The Testing view: tests found, their results and the running test command.
    pub(super) tests: super::tests_glue::TestingState,
}

/// Language servers, diagnostics, progress and log (see workbench_lsp.rs).
pub(super) struct LspState {
    /// Language servers (language key -> state), diagnostics (path -> list) and progress.
    pub(super) servers: std::collections::HashMap<String, lsp_glue::ServerSlot>,
    /// Folders for which the user was told the language server is off because the folder is untrusted (notified once per folder).
    pub(super) restricted_notified: std::collections::HashSet<String>,
    pub(super) diagnostics: std::collections::BTreeMap<String, Vec<crate::lsp::Diagnostic>>,
    /// Output panel: language server logs (most recent 5,000 lines).
    pub(super) output: std::collections::VecDeque<String>,
    /// `willRenameFiles` edits to apply after a move (paths already mapped to the new location).
    pub(super) pending_rename_edits: Vec<serde_json::Value>,
    pub(super) progress: Option<SharedString>,
    pub(super) progress_next: Option<Option<SharedString>>,
    pub(super) progress_timer: Option<gpui::Task<()>>,
    pub(super) tasks: Vec<gpui::Task<()>>,
    pub(super) generation: u64,
    pub(super) restarts: std::collections::HashMap<String, u32>,
    /// Pending `workspace/symbol` search (dropped, i.e. cancelled, when a newer one arrives).
    pub(super) symbol_task: Option<gpui::Task<()>>,
    /// Position supplied by the language server: (path, line, UTF-16 column), 0-based.
    pub(super) pending_reveal: Option<(String, u32, u32)>,
    /// JSON/YAML schema service (set up on first use) and the pending check per file.
    pub(super) schema: Option<Arc<schema::ide::Service>>,
    pub(super) schema_tasks: std::collections::HashMap<String, gpui::Task<()>>,
    /// Spell checker, list of installed languages and the list being loaded.
    pub(super) spell: Option<Arc<spell::Speller>>,
    pub(super) spell_langs: Vec<String>,
    pub(super) spell_loading: Option<Vec<String>>,
    /// Dictionary cache folder (set only in tests; the system cache otherwise).
    pub(super) spell_root: Option<std::path::PathBuf>,
    /// `.http`: responses of named requests (for chaining), the last response and the file of the response tab.
    pub(super) http_responses: std::collections::HashMap<String, crate::http_client::Stored>,
    pub(super) http_last: Option<crate::http_client::Response>,
    pub(super) http_tab: Option<String>,
    /// A request is being sent (overlapping sends are prevented) and files whose sensitive send was approved (for the session).
    pub(super) http_busy: bool,
    pub(super) http_trusted: std::collections::HashSet<String>, // `trust_key` of the file: same path, other host, asks again
    /// Hosts for which a schema-download approval question is open right now (so one host is asked about once).
    pub(super) schema_asking: std::collections::HashSet<String>,
}

/// Git state of the open folder and refresh flags (see workbench_git.rs).
pub(super) struct GitState {
    pub(super) branch: Option<SharedString>,
    /// Git state of the open folder (`None` if it isn't a repository).
    pub(super) status: Option<Arc<crate::git::Status>>,
    pub(super) refresh: Option<gpui::Task<()>>,
    /// A `git status` is running (so polling doesn't overlap); if a new request arrived meanwhile,
    /// it runs once more when done (so the running refresh isn't cancelled and the update isn't lost).
    pub(super) refreshing: bool,
    pub(super) refresh_again: bool,
    /// Network operations: which one runs, and the background fetch's last error (see workbench_sync.rs).
    pub(super) sync: sync_glue::SyncState,
    pub(super) error: Option<SharedString>,
    /// Root source control looks at: the active file's root (`None` for the first root).
    pub(super) scm_root: Option<String>,
    /// Explorer tree root -> git marks of that root (trees show the marks of their own roots).
    /// Whether the marks of inactive roots are being refreshed right now.
    pub(super) marks_busy: std::collections::HashSet<String>,
    pub(super) tree_marks: std::collections::HashMap<String, Arc<std::collections::HashMap<String, crate::git::Mark>>>,
}

/// Layout: side panel, bottom panel, split editor, zoom, zen mode, window bounds.
pub(super) struct ChromeLayout {
    pub(super) web_preview: super::web_preview_glue::State,
    pub(super) resources: Entity<crate::resource_monitor::ResourceMonitor>,
    pub(super) panel: Option<Panel>,
    /// The bottom panel shows the Problems list instead of the terminal.
    pub(super) bottom: Bottom,
    pub(super) sidebar_w: f32,
    pub(super) resizing_sidebar: bool,
    pub(super) panel_open: bool,
    pub(super) panel_h: f32,
    /// Panel on the right (next to the editor) and its width; maximized (covers the editor).
    pub(super) panel_right: bool,
    pub(super) panel_w: f32,
    pub(super) panel_max: bool,
    pub(super) resizing_panel: bool,
    /// Width ratio of the left group while split.
    pub(super) split_ratio: f32,
    pub(super) resizing_split: bool,
    /// Zoom delta (pt) and the base sizes from settings (editor, line, terminal).
    pub(super) zoom: f32,
    pub(super) zoom_base: (f32, f32, f32),
    /// Zen mode: the previous layout is kept here while it is on.
    pub(super) zen: Option<ZenSaved>,
    /// Window bounds at the last draw (saved on exit, used at the next launch).
    pub(super) window_bounds: Option<gpui::WindowBounds>,
}

/// Navigation history, closed tabs and the last edit location.
pub(super) struct NavState {
    /// Back/forward navigation (⌃- / ⌃⇧-) and closed tabs (⇧⌘T).
    pub(super) back: Vec<NavPos>,
    pub(super) forward: Vec<NavPos>,
    pub(super) navigating: bool,
    pub(super) closed: Vec<NavPos>,
    /// Last edit location (⌘K ⌘Q) and the last seen text version of each file.
    pub(super) last_edit: Option<NavPos>,
    pub(super) edit_versions: std::collections::HashMap<String, u64>,
    /// Recently opened files (absolute path, newest first): at the top in quick open.
    pub(super) recent_files: Vec<String>,
}

/// Settings files, project settings and workspace trust (see workbench_settings.rs).
pub(super) struct ConfigState {
    /// Settings folder (`None` in tests: the user's files are left alone).
    pub(super) dir: Option<std::path::PathBuf>,
    /// Last modification times of (settings, keybindings): live reload.
    pub(super) stamp: (Option<std::time::SystemTime>, Option<std::time::SystemTime>),
    /// `--theme` was given: the theme in the settings file isn't applied this session.
    pub(super) theme_override: bool,
    /// Project settings: (file name, text, unknown-key warning). Applied on top of the
    /// user settings; when `config_dirty` is set the settings are re-read.
    pub(super) project: Vec<(String, String, bool)>,
    /// Folders trusted this session, as `trust_key`s so the same path on another host is not covered (persistent record: state
    /// `trust:` keys; only here if there is no state).
    pub(super) trusted: std::collections::HashSet<String>,
    pub(super) dirty: bool,
    /// Own settings of extra folders (root -> settings): applied to that folder's files.
    pub(super) folders: std::collections::HashMap<String, Arc<crate::folder_settings::FolderSettings>>,
}

/// Workspace record, backups, session state and recents.
pub(super) struct PersistState {
    /// Record of the open folder (session and backups are tied to it).
    pub(super) workspace_id: Option<i64>,
    /// Folder the backups are written to (`None` in tests: no backups).
    pub(super) data_dir: Option<std::path::PathBuf>,
    /// Workspace to restore on the next render (the window lives there).
    pub(super) pending_restore: Option<i64>,
    pub(super) backup_task: Option<gpui::Task<()>>,
    /// Path -> backed-up version.
    pub(super) backed_up: std::collections::HashMap<String, u64>,
    pub(super) state: Option<State>,
    pub(super) recent: Vec<RecentWorkspace>,
    /// Pinned folders (host alias or `None`, path).
    pub(super) pinned: Vec<(Option<String>, String)>,
    /// Recent entry opened over SSH: if it can't connect or the folder is missing, asks "remove from the list?".
    pub(super) opening_recent: Option<RecentWorkspace>,
}

/// Connection: host machine, SSH details and pending questions (see workbench_folder.rs).
pub(super) struct RemoteState {
    pub(super) docker: crate::docker::State,
    pub(super) host: Arc<dyn Host>,
    /// For things that always open on this machine, such as settings files.
    pub(super) local_host: Arc<dyn Host>,
    pub(super) conn: Conn,
    /// State of the still-open workspace while a different connection is pending.
    pub(super) connection_origin: Option<Conn>,
    /// Folder to open once connected (if picked from recents).
    pub(super) pending_path: Option<String>,
    pub(super) ssh_hosts: Vec<SshHost>,
    pub(super) bundle: Arc<AgentBundle>,
    /// ssh questions waiting in the queue (several connections can be active at once).
    pub(super) asks: std::collections::VecDeque<(AskRequest, AskReply)>,
    /// Ongoing file transfer (upload, download, copy); one at a time.
    pub(super) transfer: Option<Arc<super::transfer::TransferRun>>,
    /// The Remote Explorer view.
    pub(super) view: super::remote_glue::RemoteView,
}

/// Explorer roots, clipboard and favorites.
pub(super) struct RootsState {
    pub(super) primary: Entity<FileTree>,
    /// Extra folders added to the workspace (the first root is `tree`); at most `MAX_EXTRA_ROOTS`.
    pub(super) extra_roots: Vec<String>,
    pub(super) extra_trees: Vec<Entity<FileTree>>,
    /// Extra folders returning from the previous session: their paths are known until the trees are built (so tabs, bookmarks and
    /// the language server root resolve correctly).
    pub(super) pending_extra_roots: Vec<String>,
    /// Path taken in the tree with ⌘C / ⌘X (`true` if cut).
    pub(super) file_clip: Option<(String, bool)>,
    /// File marked with "Select for Compare".
    pub(super) compare_a: Option<String>,
    // --- compare ---
    /// Folder marked with "Select Folder for Compare".
    pub(super) compare_folder_a: Option<compare_glue::Spot>,
    /// Favorite files/folders of the open project (relative to the root).
    pub(super) favorites: Vec<String>,
    pub(super) file_index: Option<FileIndex>,
}

/// Editor groups: tabs, separate tab views (diff, large file, page) and startup state.
pub(super) struct TabsState {
    /// Tabs of the focused editor group (when split, the other lives in `other`;
    /// they swap when focus changes, so the tab code only knows a single group).
    pub(super) editors: Vec<Entity<EditorView>>,
    pub(super) other: Option<Group>,
    /// Whether the focused group is the left one.
    pub(super) focused_left: bool,
    pub(super) active: Option<usize>,
    pub(super) opening: Option<String>,
    pub(super) open_error: Option<(String, SharedString)>,
    /// Single diff tab (a new diff replaces the previous one); `diff_active`: that tab is in front.
    pub(super) diff: Option<Entity<crate::diff::DiffView>>,
    pub(super) diff_active: bool,
    /// Single large-file view (read-only; a separate tab like diff).
    pub(super) large: Option<Entity<crate::large::LargeFileView>>,
    pub(super) large_active: bool,
    /// Welcome or Settings page (single tab, like diff); `page_active`: that tab is in front.
    pub(super) page: Option<Page>,
    pub(super) page_active: bool,
    pub(super) _page_sub: Option<Subscription>,
    /// Match to show in the file opened from a search result: (path, line, column, length).
    pub(super) pending_reveal: Option<(String, u32, u32, u32)>,
    /// Tab bar scroll (so the active tab stays visible).
    pub(super) tab_scroll: gpui::ScrollHandle,
}

/// In-app updates: what the updater is doing, the dialog, and the worker thread (see workbench_update.rs).
pub(super) struct UpdateUi {
    /// The network, the installer, the clock and quitting; replaced by a fake in tests.
    pub(super) backend: Arc<dyn crate::update_backend::UpdateBackend>,
    pub(super) state: update_glue::UpdateState,
    pub(super) dialog: update_glue::Dialog,
    /// The check in progress was asked for by the user: its dialog is open and its errors are shown.
    pub(super) manual: bool,
    /// Release notes of the offered release (parsed once) and how this copy can be updated (looked up when a release is
    /// offered, because it probes the disk).
    pub(super) notes: Vec<crate::markdown::Block>,
    /// The same notes as plain text for the accessibility tree (capped, with a pointer to the release page).
    pub(super) notes_text: String,
    /// The notes region takes the keyboard (Up, Down, Page Up/Down, Home, End scroll it) and is reached by Tab before the buttons.
    pub(super) notes_focus: FocusHandle,
    pub(super) notes_scroll: gpui::ScrollHandle,
    pub(super) kind: crate::update_install::InstallKind,
    pub(super) run: Option<update_glue::Run>,
    pub(super) next_run: u64,
    /// Time of the last check that got an answer (also kept in the state database).
    pub(super) last_check: Option<std::time::SystemTime>,
    /// Checks that failed within the last day (not stored: a restart is a fresh start), see `update::retry_allowed`.
    pub(super) failures: Vec<std::time::SystemTime>,
    /// When a key was last pressed in any window: the first-run question waits for a quiet keyboard.
    pub(super) last_key: Option<std::time::SystemTime>,
    /// One handle per dialog button, in tab order, and where focus returns when the dialog closes.
    pub(super) focus: Vec<FocusHandle>,
    pub(super) return_focus: Option<FocusHandle>,
    /// Enter and Space answer the first-run question only after a short delay (see `ARM_AFTER`).
    pub(super) armed: bool,
    pub(super) arm: Option<gpui::Task<()>>,
    /// The startup timers (the question, then the daily check).
    pub(super) schedule: Option<gpui::Task<()>>,
    /// macOS: the dialog lives in a window of its own (like the update windows of other Mac apps) instead of a card over the editor.
    /// Off in tests, which drive the card, and on other systems.
    pub(super) native: bool,
    /// That window while it is open, and whether it is being opened (the open is deferred out of `render`).
    pub(super) window: Option<gpui::WindowHandle<update_window_glue::UpdateWindow>>,
    pub(super) opening: bool,
}

impl UpdateUi {
    pub(super) fn new(dev: bool, cx: &mut App) -> Self {
        UpdateUi {
            backend: crate::update_backend::default_backend(dev),
            state: update_glue::UpdateState::Idle,
            dialog: update_glue::Dialog::Closed,
            manual: false,
            notes: Vec::new(),
            notes_text: String::new(),
            notes_focus: cx.focus_handle().tab_stop(true),
            notes_scroll: gpui::ScrollHandle::new(),
            kind: crate::update_install::InstallKind::Manual(crate::update_install::ManualReason::NotInstalled),
            run: None,
            next_run: 0,
            last_check: None,
            failures: Vec::new(),
            last_key: None,
            focus: (0..update_glue::MAX_BUTTONS).map(|_| cx.focus_handle().tab_stop(true)).collect(),
            return_focus: None,
            armed: false,
            arm: None,
            schedule: None,
            native: cfg!(all(target_os = "macos", not(test))),
            window: None,
            opening: false,
        }
    }
}
