//! Settings: `~/.config/codeyard/settings.jsonc` and `keybindings.jsonc`.
//! JSONC (comments and trailing commas allowed). A bad value never breaks the app:
//! that key falls back to its default and a warning is recorded.

use std::path::PathBuf;

use gpui::Global;
use serde_json::{Map, Value};

/// What happens when the app starts without arguments (`window.startup`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Startup {
    /// Welcome screen: recent folders are listed, nothing (SSH included) reconnects by itself.
    #[default]
    Welcome,
    /// Like a browser: the last folder (or SSH server) and tabs come back as they were.
    Continue,
    /// Blank window: no welcome content, just the faint logo.
    Blank,
}

impl Startup {
    pub fn id(self) -> &'static str {
        match self {
            Startup::Welcome => "welcome",
            Startup::Continue => "continue",
            Startup::Blank => "blank",
        }
    }

    pub fn from_id(id: &str) -> Option<Startup> {
        [Startup::Welcome, Startup::Continue, Startup::Blank].into_iter().find(|s| s.id() == id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AutoSave {
    Off,
    /// This many ms after the last edit.
    AfterDelay(u64),
    /// When the editor loses focus.
    OnFocusChange,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub theme: String,
    pub editor_font_family: Option<String>,
    pub editor_font_size: f32,
    pub editor_line_height: f32,
    /// `None`: guess from the file.
    pub tab_size: Option<usize>,
    pub insert_spaces: Option<bool>,
    pub word_highlight: bool,
    pub indent_guides: bool,
    pub trim_trailing_whitespace: bool,
    pub insert_final_newline: bool,
    pub auto_save: AutoSave,
    /// Names/globs hidden in the tree and skipped by search and quick open.
    pub files_exclude: Vec<String>,
    pub search_max_results: u32,
    // --- git-sync ---
    /// Fetch in the background while the window is in front (`git.autofetch`, every `git.autofetchPeriod` seconds).
    pub git_autofetch: bool,
    pub git_autofetch_period: u32,
    /// Ask before a sync that pulls or pushes.
    pub git_confirm_sync: bool,
    /// Pull with `--rebase` instead of merging.
    pub git_rebase_when_sync: bool,
    pub git_prune_on_fetch: bool,
    pub git_pull_tags: bool,
    /// Fetch every remote before pulling.
    pub git_fetch_on_pull: bool,
    /// "none", "push" or "sync": what runs after a commit.
    pub git_post_commit: String,
    /// Commit all tracked changes when nothing is staged.
    pub git_smart_commit: bool,
    // --- structural ---
    /// Most files a structural search reads and parses.
    pub search_structural_max_files: usize,
    // --- compare ---
    /// Globs the folder comparison skips on both sides.
    pub compare_ignore: Vec<String>,
    /// Entries per side before the folder comparison stops and says so.
    pub compare_max_files: u32,
    pub terminal_shell: Option<String>,
    pub terminal_font_size: f32,
    pub terminal_scrollback: usize,
    pub restore_session: bool,
    /// Whether to start with the welcome screen or continue where you left off.
    pub startup: Startup,
    pub hot_exit: bool,
    /// Small preview on the right (off by default).
    pub minimap: bool,
    /// Long lines wrap at the window width (⌥Z).
    pub word_wrap: bool,
    /// The active file is selected and visible in the tree.
    pub auto_reveal: bool,
    /// Files deleted locally go to the Trash (`files.enableTrash`).
    pub enable_trash: bool,
    /// In files without a language server, suggest words from the document.
    pub word_based_suggestions: bool,
    /// Whether the cursor blinks (`editor.cursorBlinking`; "solid" turns it off).
    pub cursor_blinking: bool,
    /// Format with the language server on save.
    pub format_on_save: bool,
    /// Code actions run on save (`editor.codeActionsOnSave`), e.g. `source.organizeImports`.
    pub code_actions_on_save: Vec<String>,
    /// Format pasted text with the language server (`editor.formatOnPaste`).
    pub format_on_paste: bool,
    /// Ask the server to format on triggers like `}` `;` Enter (`editor.formatOnType`).
    pub format_on_type: bool,
    /// Apply `.editorconfig` rules.
    pub editorconfig: bool,
    /// Language server type/parameter hints (at the end of the line).
    pub inlay_hints: bool,
    pub code_lens: bool,
    /// Mark invisible and confusable Unicode characters with a box.
    pub unicode_highlight: bool,
    pub semantic_highlighting: bool,
    /// Expand an Emmet abbreviation with Tab (`emmet.triggerExpansionOnTab`).
    pub emmet: bool,
    /// Language of the upper/lower-case commands (`editor.caseLocale`): "auto", "tr" or "en".
    pub case_locale: String,
    /// Font ligatures (`editor.fontLigatures`).
    pub font_ligatures: bool,
    /// HTML/XML/JSX: the tag name changes together with its matching tag.
    pub linked_editing: bool,
    /// Commands run from an untrusted folder (tasks, debugging, project-level
    /// `terminal.shell` / `lsp.servers`) ask for confirmation (`security.workspaceTrust.enabled`).
    pub workspace_trust: bool,
    /// Open the diff view as a single column (inline).
    pub diff_inline: bool,
    /// Auto-close brackets and quotes.
    pub auto_close_brackets: bool,
    /// Whether ports opened later over SSH are forwarded automatically.
    pub ports_auto_forward: bool,
    /// Whether language servers are enabled.
    pub lsp_enabled: bool,
    /// Language id → command and arguments (`None`: disabled for that language). Overrides the defaults.
    pub lsp_servers: Vec<ServerOverride>,
    /// Files that are not UTF-8 (and have no BOM) are read with this encoding.
    pub fallback_encoding: crate::encoding::Encoding,
    /// Chains of folders that each hold a single subfolder show on one row ("a/b/c").
    pub compact_folders: bool,
    /// "default" (folders first), "filesFirst", "mixed", "type".
    pub sort_order: String,
    /// "Open Editors" section above the explorer.
    pub open_editors: bool,
    /// Vertical rulers (column numbers).
    pub rulers: Vec<usize>,
    /// "none", "boundary" (all but between words), "all".
    pub render_whitespace: String,
    /// "on", "off", "relative".
    pub line_numbers: String,
    /// "line", "block", "underline".
    pub cursor_style: String,
    /// Matching bracket pairs are colored by depth.
    pub bracket_colors: bool,
    /// Scope headers (function/class line) stay pinned at the top.
    pub sticky_scroll: bool,
    /// Color swatch next to #rrggbb.
    pub color_decorators: bool,
    /// File and folder icon theme (`workbench.iconTheme`).
    pub icon_theme: String,
    pub status_bar: bool,
    pub activity_bar: bool,
    /// Explorer indent width (px).
    pub tree_indent: f32,
    /// Also ask for confirmation when moving to the Trash.
    pub confirm_delete: bool,
    /// Ask when moving by drag and drop in the explorer.
    pub confirm_drag_and_drop: bool,
    /// Validate JSON and YAML files against their schema (diagnostics, hover, completion).
    pub schema_validate: bool,
    /// Download schemas from SchemaStore (cached); when off, only local and cached schemas are used.
    pub schema_download: bool,
    /// `json.schemas` and `yaml.schemas`: (file patterns, schema URL or path).
    pub schema_associations: Vec<(Vec<String>, String)>,
    /// Spell checking: enabled or not, whether comments in code are checked too, which dictionaries (`en` ships with the app).
    pub spelling: bool,
    pub spelling_comments: bool,
    pub spell_languages: Vec<String>,
    // --- data ---
    /// Data lens: spaces per level in JSON and YAML output, and whether output keys are sorted.
    pub data_indent: usize,
    pub data_sort_keys: bool,
    /// Where `.http` requests run: "workspace" (on the server when you are on SSH) or "local".
    pub http_run_on: String,
    /// `never`, `remote` (first request to each non-local host per session) or `always`; see `schema::gate`.
    pub http_confirm: String,
    /// UI language (`ui.language`): "auto" (system language), "en" or "tr"; user settings only. "en" in tests.
    pub ui_language: String,
    /// Update checks (`update.check`); user settings only, because it decides whether the app contacts the network by itself.
    pub update_check: crate::update::Policy,
    /// Folder hierarchy lines in the explorer.
    pub tree_guides: bool,
    /// Whether the Git graph shows the author (avatar and name) and the short hash.
    pub graph_show_author: bool,
    pub graph_show_hash: bool,
    /// Default view of the Git graph: "graph", "compact" or "dates".
    pub graph_view: String,
    /// Search is case-insensitive unless an uppercase letter is typed.
    pub search_smart_case: bool,
    pub search_line_numbers: bool,
    /// "auto", "alwaysCollapse", "alwaysExpand".
    pub search_collapse: String,
    /// Maximum commits in the Git graph; maximum commits in TIMELINE.
    pub graph_limit: usize,
    pub timeline_limit: usize,
    /// The graph shows all branches by default.
    pub graph_all_branches: bool,
    /// Author/time of the cursor line at the end of the line (`git.inlineBlame`).
    pub inline_blame: bool,
    pub render_line_highlight: bool,
    pub match_brackets: bool,
    pub folding: bool,
    /// `workbench.colorCustomizations`: (theme id or name (every theme if empty), role, color).
    pub color_overrides: Vec<(Option<String>, String, u32)>,
    /// UI font (`workbench.fontFamily`).
    pub ui_font: Option<String>,
    pub show_tabs: bool,
    pub tab_icons: bool,
    /// "hover", "always", "off".
    pub tab_close: String,
    pub breadcrumbs: bool,
    pub cursor_width: f32,
    pub hover_enabled: bool,
    pub quick_suggestions: bool,
    /// Additionally skipped by search (`search.exclude`).
    pub search_exclude: Vec<String>,
    /// `files.associations`: (glob, language or extension).
    pub file_associations: Vec<(String, String)>,
}

/// Language id → (command, arguments); `None`: disabled for that language.
pub type ServerOverride = (String, Option<(String, Vec<String>)>);

impl Global for Settings {}

impl Settings {
    /// Whether to show welcome content in an empty editor (only `window.startup = "blank"` does not).
    pub fn shows_welcome(&self) -> bool {
        self.startup != Startup::Blank
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: "auto".into(),
            editor_font_family: None,
            editor_font_size: theme::text::CODE.0,
            editor_line_height: theme::text::CODE.1,
            tab_size: None,
            insert_spaces: None,
            word_highlight: true,
            indent_guides: true,
            trim_trailing_whitespace: false,
            insert_final_newline: false,
            auto_save: AutoSave::Off,
            files_exclude: vec![".git".into(), ".DS_Store".into()],
            search_max_results: 2000,
            // --- git-sync ---
            git_autofetch: false,
            git_autofetch_period: 180,
            git_confirm_sync: true,
            git_rebase_when_sync: false,
            git_prune_on_fetch: true,
            git_pull_tags: true,
            git_fetch_on_pull: false,
            git_post_commit: "none".into(),
            git_smart_commit: true,
            // --- structural ---
            search_structural_max_files: 5000,
            // --- compare ---
            compare_ignore: vec![".git".into(), "node_modules".into(), "target".into()],
            compare_max_files: 50_000,
            terminal_shell: None,
            terminal_font_size: theme::text::CODE.0,
            terminal_scrollback: 2000,
            restore_session: true,
            startup: Startup::Welcome,
            hot_exit: true,
            auto_close_brackets: true,
            format_on_save: false,
            code_actions_on_save: Vec::new(),
            format_on_paste: false,
            format_on_type: false,
            editorconfig: true,
            inlay_hints: true,
            code_lens: true,
            unicode_highlight: true,
            semantic_highlighting: true,
            emmet: true,
            case_locale: "auto".into(),
            font_ligatures: false,
            linked_editing: true,
            // Tests treat folders as trusted; those exercising the trust gate turn this on.
            workspace_trust: !cfg!(test),
            diff_inline: false,
            minimap: true,
            word_wrap: false,
            auto_reveal: true,
            cursor_blinking: true,
            word_based_suggestions: true,
            enable_trash: true,
            ports_auto_forward: true,
            lsp_enabled: true,
            lsp_servers: Vec::new(),
            fallback_encoding: crate::encoding::Encoding::Windows1254,
            compact_folders: true,
            sort_order: "default".into(),
            open_editors: true,
            rulers: Vec::new(),
            render_whitespace: "none".into(),
            line_numbers: "on".into(),
            cursor_style: "line".into(),
            bracket_colors: true,
            sticky_scroll: true,
            color_decorators: true,
            icon_theme: crate::icon_theme::ALL[0].id().into(),
            status_bar: true,
            activity_bar: true,
            tree_indent: theme::layout::TREE_INDENT,
            confirm_delete: true,
            confirm_drag_and_drop: true,
            schema_validate: true,
            schema_download: true,
            schema_associations: Vec::new(),
            spelling: true,
            spelling_comments: true,
            spell_languages: vec!["en".into()],
            data_indent: 2,
            data_sort_keys: false,
            http_run_on: "workspace".into(),
            http_confirm: "remote".into(),
            ui_language: if cfg!(test) { "en" } else { "auto" }.into(),
            update_check: crate::update::Policy::Ask,
            tree_guides: true,
            graph_show_author: true,
            graph_show_hash: true,
            graph_view: "graph".into(),
            search_smart_case: false,
            search_line_numbers: true,
            search_collapse: "auto".into(),
            graph_limit: 300,
            timeline_limit: 100,
            graph_all_branches: true,
            inline_blame: true,
            render_line_highlight: true,
            match_brackets: true,
            folding: true,
            color_overrides: Vec::new(),
            ui_font: None,
            show_tabs: true,
            tab_icons: true,
            tab_close: "hover".into(),
            breadcrumbs: true,
            cursor_width: 2.,
            hover_enabled: true,
            quick_suggestions: true,
            search_exclude: Vec::new(),
            file_associations: Vec::new(),
        }
    }
}

/// File created on first launch: every setting with its comment.
pub const TEMPLATE: &str = r##"// codeyard settings. Saved changes apply immediately.
// Comments and trailing commas are fine.
{
  // "auto" follows the system (Codeyard Dark / Codeyard Light), or a theme id:
  // codeyard-dark, codeyard-light, studio-dark, midnight, graphite, ember, fjord, daylight, paper,
  // quarry, dracula, one-dark, nord, solarized-dark, solarized-light,
  // github-light, high-contrast-dark, high-contrast-light, user:<file name>
  "theme": "auto",

  // "editor.fontFamily": "JetBrains Mono",
  "editor.fontSize": 13,
  "editor.lineHeight": 20,
  // Leave out to detect from each file.
  // "editor.tabSize": 4,
  // "editor.insertSpaces": true,
  "editor.wordHighlight": true,
  "editor.indentGuides": true,
  // "on" wraps long lines at the window edge (toggle with ⌥Z).
  "editor.wordWrap": "off",
  // "solid" stops the cursor from blinking (it also settles after ~5 s idle, which lets
  // the GPU release its frame buffers).
  "editor.cursorBlinking": "blink",
  // Files without a language server suggest words from the file (after two letters).
  "editor.wordBasedSuggestions": true,
  // Vertical rulers at these columns, e.g. [80, 120].
  "editor.rulers": [],
  // "none", "boundary" (all but single spaces between words), "all"
  "editor.renderWhitespace": "none",
  // "on", "off", "relative" (distance from the cursor line)
  "editor.lineNumbers": "on",
  // "line", "block", "underline"
  "editor.cursorStyle": "line",
  // Matching brackets are colored by nesting depth.
  "editor.bracketPairColorization": true,
  // The line of the enclosing function or class stays pinned at the top while you scroll.
  "editor.stickyScroll.enabled": true,
  // A color swatch next to #rrggbb values.
  "editor.colorDecorators": true,

  // The line with the cursor is highlighted: "line" or "none".
  "editor.renderLineHighlight": "line",
  // The bracket matching the one at the cursor is highlighted.
  "editor.matchBrackets": true,
  // Fold arrows in the gutter.
  "editor.folding": true,
  // Width of the line cursor (px).
  "editor.cursorWidth": 2,
  // Tooltips when the mouse rests on a word (language server).
  "editor.hover.enabled": true,
  // Suggestions pop up while you type (false: only on Ctrl+Space).
  "editor.quickSuggestions": true,
  // Treat files as another language: { "*.tpl": "html", "Jenkinsfile": "groovy" }.
  // "files.associations": {},
  // Extra globs skipped by search (on top of files.exclude).
  // "search.exclude": ["**/dist", "*.min.js"],

  // File and folder icons: "minimal", "codeyard", "badges", "colorful", "apple", "material", "none"
  "workbench.iconTheme": "minimal",
  "workbench.statusBar.visible": true,
  "workbench.activityBar.visible": true,
  // UI font for the whole window (the editor has its own "editor.fontFamily").
  // "workbench.fontFamily": "Inter",
  "workbench.editor.showTabs": true,
  "workbench.tabs.showIcons": true,
  // "hover" (the x shows on the active tab or on hover), "always", "off"
  "workbench.tabs.closeButton": "hover",
  "workbench.breadcrumbs.enabled": true,
  // Override any color of the theme (roles: bg, surface, accent, text, syntax-keyword …).
  // "[theme name]": { … } applies to that theme only.
  // "workbench.colorCustomizations": { "accent": "#ff8800", "[Quarry]": { "syntax-string": "#a6e22e" } },
  // Indent of each folder level in the explorer (px).
  "workbench.tree.indent": 16,

  // Ask before moving a file to the Trash (permanent deletes always ask).
  "explorer.confirmDelete": true,
  // Ask before moving a file or folder by dragging it in the explorer.
  "explorer.confirmDragAndDrop": true,
  // Vertical hierarchy lines under every folder arrow in the explorer.
  "workbench.tree.guides": true,
  // Source Control graph rows: show the author (avatar and name) and the short hash.
  "git.graph.showAuthor": true,
  "git.graph.showHash": true,
  // Source Control graph view: "graph" (lanes), "compact" (one line per commit) or "dates" (grouped by day).
  "git.graph.view": "graph",
  // A query without capital letters ignores case; one with capitals matches case.
  "search.smartCase": false,
  "search.showLineNumbers": true,
  // "auto", "alwaysCollapse", "alwaysExpand": results start folded or open.
  "search.collapseResults": "auto",

  // Data: Convert To… and Data: Query…: spaces per level in JSON and YAML, and sorted keys (false keeps file order).
  "data.indent": 2,
  "data.sortKeys": false,

  // Commits drawn in the Source Control graph, and listed in the explorer's Timeline.
  "git.graph.limit": 300,
  "git.timeline.limit": 100,
  // Who last changed the cursor line, and when, shown faintly at the end of the line.
  "git.inlineBlame": true,
  // The graph starts with every branch (false: only the current branch).
  "git.graph.allBranches": true,

  // Language-server actions run on save, before formatting (e.g. sort and drop unused imports).
  // "editor.codeActionsOnSave": { "source.organizeImports": "explicit", "source.fixAll": "explicit" },
  "files.trimTrailingWhitespace": false,
  "files.insertFinalNewline": false,
  // "off", "afterDelay", "onFocusChange"
  "files.autoSave": "off",
  "files.autoSaveDelay": 1000,
  // The active file is selected (and its folders opened) in the file tree.
  "explorer.autoReveal": true,
  // Folders that only hold one folder show as one row: "src/main/java".
  "explorer.compactFolders": true,
  // "default" (folders first), "filesFirst", "mixed", "type" (by extension)
  "explorer.sortOrder": "default",
  // The list of open files above the folder tree.
  "explorer.openEditors": true,
  // Hidden in the file tree, skipped by search and Go to File.
  "files.exclude": [".git", ".DS_Store"],
  // Unsaved changes survive quitting and crashes.
  "files.hotExit": true,
  // Deleted local files go to the Trash (SSH hosts have none: deletes are permanent).
  "files.enableTrash": true,
  // Files that are not UTF-8 (and have no BOM) are read as: "windows1254"
  // (Turkish), "iso88599" or "iso88591". They are saved back the same way.
  "files.fallbackEncoding": "windows1254",

  "search.maxResults": 2000,
  // --- git-sync ---
  // Fetch in the background while the window is in front, every this many seconds (30 or more). Not in folders you have not trusted.
  "git.autofetch": false,
  "git.autofetchPeriod": 180,
  // Ask before Sync pulls or pushes (your own settings only: a folder cannot turn the question off).
  "git.confirmSync": true,
  // Pull with rebase instead of a merge commit.
  "git.rebaseWhenSync": false,
  // Fetch drops remote branches that no longer exist, pull also fetches all tags, and fetchOnPull fetches every remote first.
  "git.pruneOnFetch": true,
  "git.pullTags": true,
  "git.fetchOnPull": false,
  // "none", "push" or "sync" after every commit (your own settings only).
  "git.postCommitCommand": "none",
  // Commit all tracked changes when nothing is staged (false: stage first).
  "git.enableSmartCommit": true,
  // --- structural ---
  // Most files a structural search reads and parses (the rest are left out and the results say so).
  "search.structural.maxFiles": 5000,

  // Compare Folders: globs skipped on both sides, and how many entries per side before it stops.
  // "compare.folders.ignore": [".git", "node_modules", "target"],
  // "compare.folders.maxFiles": 50000,

  // "terminal.shell": "/bin/zsh",
  "terminal.fontSize": 13,
  "terminal.scrollback": 2000,

  // What opens when you start the app without a folder: "welcome" (the start page; recent folders and SSH
  // hosts are listed, nothing reconnects by itself), "continue" (like a browser: the last folder or SSH
  // host and its tabs come back) or "blank" (an empty window).
  "window.startup": "welcome",
  // Bring a folder's tabs and unsaved changes back whenever you open that folder.
  "window.restoreSession": true,

  // Language servers run where the files are (locally or on the SSH host).
  // Defaults: rust-analyzer, clangd, gopls, pyright-langserver, typescript-language-server, jdtls, csharp-ls, intelephense,
  // ruby-lsp, kotlin-language-server, sourcekit-lsp, lua-language-server, bash-language-server, vscode-html/css-language-server.
  // Keys: rust c go python typescript java csharp php ruby kotlin swift lua bash html css (opt-in: json yaml toml).
  // "lsp.servers": { "python": { "command": "pylsp", "args": [] }, "go": null },
  "lsp.enabled": true,
}
"##;

pub const KEYBINDINGS_TEMPLATE: &str = r#"// codeyard keyboard shortcuts. These are added on top of the defaults.
// "when" is optional: Editor, FileTree, Terminal, Picker, Workbench...
// Use "command": null to turn a default shortcut off.
// Commands: run "Preferences: Open Keyboard Shortcuts" to see this file; the full
// list is in the command palette (⇧⌘P) and in README.md.
[
  // { "key": "cmd-shift-d", "command": "editor.selectAll", "when": "Editor" },
  // { "key": "ctrl-g", "command": null, "when": "Workbench" },
]
"#;

pub fn config_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("CODEYARD_CONFIG_DIR") {
        return Some(PathBuf::from(d));
    }
    if cfg!(windows) { dirs::config_dir().map(|d| d.join(host::ssh::APP_DIR)) } else { dirs::home_dir().map(|d| d.join(".config").join(host::ssh::APP_DIR)) }
}

/// Create from the template if the file does not exist.
pub fn ensure_file(path: &PathBuf, template: &str) -> std::io::Result<()> {
    if !path.exists() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, template)?;
    }
    Ok(())
}

#[cfg(test)]
pub fn parse(text: &str) -> (Settings, Vec<String>) {
    parse_checked(text).unwrap_or_else(|e| (Settings::default(), vec![e]))
}

/// `Err`: the file could not be read as JSON (half-written). The caller keeps the last valid
/// settings; individual bad values only produce warnings and the rest is applied.
#[cfg(test)]
pub fn parse_checked(text: &str) -> Result<(Settings, Vec<String>), String> {
    parse_layers(text, &[])
}

/// Project settings layer: (file name, text, whether to warn about unknown keys).
/// `.vscode/settings.json` is full of extension settings: no warnings there.
pub type Layer<'a> = (&'a str, &'a str, bool);

/// User settings + project layers (later over earlier, key by key).
/// `Err` if the user file cannot be read (the last valid settings stay); a broken project file
/// only produces a warning.
pub fn parse_layers(user: &str, project: &[Layer]) -> Result<(Settings, Vec<String>), String> {
    parse_layers_trusted(user, project, false)
}

/// A project's settings file could choose a program by itself: these keys come only from user
/// settings or a trusted folder.
pub const RESTRICTED_KEYS: &[&str] = &["terminal.shell", "lsp.servers", "json.schemas", "yaml.schemas"];

/// Not read from the project file even in a trusted folder: the gate is global, so one
/// repo touching it would open it for every folder.
pub const USER_ONLY_KEYS: &[&str] = &[
    "security.workspaceTrust.enabled",
    "http.confirmRequests",
    "ui.language",
    "update.check",
    // --- git-sync ---
    // A repository must not turn the question before a sync off, or make every commit push by itself.
    "git.confirmSync",
    "git.postCommitCommand",
];

/// `trusted`: the folder is trusted. If not, `RESTRICTED_KEYS` in the project layer are ignored and a warning is recorded.
pub fn parse_layers_trusted(user: &str, project: &[Layer], trusted: bool) -> Result<(Settings, Vec<String>), String> {
    let mut warnings = Vec::new();
    let value: Value = serde_json::from_str(&theme::themes::strip_jsonc(user))
        .map_err(|e| format!("settings.jsonc: {}", crate::i18n::tf("{} (previous settings kept)", &[&e])))?;
    let Some(user_obj) = value.as_object() else {
        return Err(format!("settings.jsonc: {}", crate::i18n::t("expected an object { … } (previous settings kept)")));
    };
    let mut merged = user_obj.clone();
    let mut quiet: Vec<String> = Vec::new();
    for (name, text, warn) in project {
        match serde_json::from_str::<Value>(&theme::themes::strip_jsonc(text)) {
            Ok(Value::Object(o)) => {
                for (k, v) in o {
                    if USER_ONLY_KEYS.contains(&k.as_str()) {
                        warnings.push(crate::i18n::tf("{}: “{}” can only be set in your own settings.jsonc", &[&name, &k]));
                        continue;
                    }
                    if !trusted && RESTRICTED_KEYS.contains(&k.as_str()) {
                        warnings.push(crate::i18n::tf("{}: “{}” is ignored until you trust this folder (Workspace: Trust This Folder)", &[&name, &k]));
                        continue;
                    }
                    if !warn && !user_obj.contains_key(&k) {
                        quiet.push(k.clone());
                    }
                    merged.insert(k, v);
                }
            }
            Ok(_) => warnings.push(crate::i18n::tf("{}: expected an object { … }", &[&name])),
            Err(e) => warnings.push(format!("{name}: {e}")),
        }
    }
    let (s, more) = parse_object(&merged, &quiet);
    warnings.extend(more);
    Ok((s, warnings))
}

fn parse_object(obj: &Map<String, Value>, quiet: &[String]) -> (Settings, Vec<String>) {
    let mut s = Settings::default();
    let mut warnings = Vec::new();
    let mut r = Reader { obj, warnings: &mut warnings };
    if let Some(v) = r.string("theme") {
        s.theme = v;
    }
    s.editor_font_family = r.string("editor.fontFamily");
    if let Some(v) = r.number("editor.fontSize", 6., 48.) {
        s.editor_font_size = v as f32;
    }
    if let Some(v) = r.number("editor.lineHeight", 10., 80.) {
        s.editor_line_height = v as f32;
    }
    s.tab_size = r.number("editor.tabSize", 1., 16.).map(|v| v as usize);
    s.insert_spaces = r.bool("editor.insertSpaces");
    if let Some(v) = r.bool("editor.wordHighlight") {
        s.word_highlight = v;
    }
    if let Some(v) = r.bool("editor.indentGuides") {
        s.indent_guides = v;
    }
    if let Some(v) = r.bool("files.trimTrailingWhitespace") {
        s.trim_trailing_whitespace = v;
    }
    if let Some(v) = r.bool("files.insertFinalNewline") {
        s.insert_final_newline = v;
    }
    let delay = r.number("files.autoSaveDelay", 100., 600_000.).unwrap_or(1000.) as u64;
    if let Some(v) = r.string("files.autoSave") {
        s.auto_save = match v.as_str() {
            "off" => AutoSave::Off,
            "afterDelay" => AutoSave::AfterDelay(delay),
            "onFocusChange" => AutoSave::OnFocusChange,
            other => {
                r.warn(crate::i18n::tf("files.autoSave: unknown value {} (off, afterDelay, onFocusChange)", &[&format!("\"{other}\"")]));
                AutoSave::Off
            }
        };
    }
    if let Some(v) = r.strings("files.exclude") {
        s.files_exclude = v;
    }
    if let Some(v) = r.bool("files.hotExit") {
        s.hot_exit = v;
    }
    if let Some(v) = r.number("search.maxResults", 1., 100_000.) {
        s.search_max_results = v as u32;
    }
    // --- git-sync ---
    if let Some(v) = r.bool("git.autofetch") {
        s.git_autofetch = v;
    }
    if let Some(v) = r.number("git.autofetchPeriod", 30., 86_400.) {
        s.git_autofetch_period = v as u32;
    }
    if let Some(v) = r.bool("git.confirmSync") {
        s.git_confirm_sync = v;
    }
    if let Some(v) = r.bool("git.rebaseWhenSync") {
        s.git_rebase_when_sync = v;
    }
    if let Some(v) = r.bool("git.pruneOnFetch") {
        s.git_prune_on_fetch = v;
    }
    if let Some(v) = r.bool("git.pullTags") {
        s.git_pull_tags = v;
    }
    if let Some(v) = r.bool("git.fetchOnPull") {
        s.git_fetch_on_pull = v;
    }
    if let Some(v) = r.choice("git.postCommitCommand", &["none", "push", "sync"]) {
        s.git_post_commit = v;
    }
    if let Some(v) = r.bool("git.enableSmartCommit") {
        s.git_smart_commit = v;
    }
    // --- structural ---
    if let Some(v) = r.number("search.structural.maxFiles", 1., 100_000.) {
        s.search_structural_max_files = v as usize;
    }
    // --- compare ---
    if let Some(v) = r.strings("compare.folders.ignore") {
        s.compare_ignore = v;
    }
    if let Some(v) = r.number("compare.folders.maxFiles", 100., 5_000_000.) {
        s.compare_max_files = v as u32;
    }
    s.terminal_shell = r.string("terminal.shell");
    if let Some(v) = r.number("terminal.fontSize", 6., 48.) {
        s.terminal_font_size = v as f32;
    }
    if let Some(v) = r.number("terminal.scrollback", 0., 100_000.) {
        s.terminal_scrollback = v as usize;
    }
    if let Some(v) = r.bool("window.restoreSession") {
        s.restore_session = v;
    }
    // Old key: `workbench.startupEditor: "none"` meant a blank window; follow it if the new one is not written.
    if r.choice("workbench.startupEditor", &["welcome", "none"]).as_deref() == Some("none") {
        s.startup = Startup::Blank;
    }
    if let Some(v) = r.choice("window.startup", &["welcome", "continue", "blank"]) {
        s.startup = Startup::from_id(&v).unwrap_or_default();
    }
    if let Some(v) = r.bool("explorer.autoReveal") {
        s.auto_reveal = v;
    }
    if let Some(v) = r.bool("editor.wordBasedSuggestions") {
        s.word_based_suggestions = v;
    }
    if let Some(v) = r.bool("files.enableTrash") {
        s.enable_trash = v;
    }
    if let Some(v) = r.string("editor.cursorBlinking") {
        s.cursor_blinking = v != "solid";
    }
    if let Some(v) = r.string("editor.wordWrap") {
        s.word_wrap = v != "off";
    }
    if let Some(v) = r.bool("editor.minimap.enabled") {
        s.minimap = v;
    }
    if let Some(v) = r.bool("diffEditor.renderSideBySide") {
        s.diff_inline = !v;
    }
    if let Some(v) = r.choice("editor.caseLocale", &["auto", "tr", "en"]) {
        s.case_locale = v;
    }
    if let Some(v) = r.bool("editor.fontLigatures") {
        s.font_ligatures = v;
    }
    if let Some(v) = r.bool("emmet.triggerExpansionOnTab") {
        s.emmet = v;
    }
    if let Some(v) = r.bool("editor.codeLens") {
        s.code_lens = v;
    }
    if let Some(v) = r.bool("editor.unicodeHighlight.enabled") {
        s.unicode_highlight = v;
    }
    if let Some(v) = r.bool("editor.semanticHighlighting.enabled") {
        s.semantic_highlighting = v;
    }
    if let Some(v) = r.bool("editor.linkedEditing") {
        s.linked_editing = v;
    }
    if let Some(v) = r.bool("security.workspaceTrust.enabled") {
        s.workspace_trust = v;
    }
    if let Some(v) = r.bool("editor.inlayHints.enabled") {
        s.inlay_hints = v;
    }
    if let Some(v) = r.bool("editor.editorconfig") {
        s.editorconfig = v;
    }
    if let Some(v) = r.bool("editor.formatOnSave") {
        s.format_on_save = v;
    }
    if let Some(v) = r.bool("editor.formatOnPaste") {
        s.format_on_paste = v;
    }
    if let Some(v) = r.bool("editor.formatOnType") {
        s.format_on_type = v;
    }
    if let Some(v) = r.code_actions("editor.codeActionsOnSave") {
        s.code_actions_on_save = v;
    }
    if let Some(v) = r.bool("editor.autoClosingBrackets") {
        s.auto_close_brackets = v;
    }
    if let Some(v) = r.string("files.fallbackEncoding").and_then(|n| crate::encoding::Encoding::from_setting(&n)) {
        s.fallback_encoding = v;
    }
    if let Some(v) = r.bool("explorer.compactFolders") {
        s.compact_folders = v;
    }
    if let Some(v) = r.choice("http.runOn", &["workspace", "local"]) {
        s.http_run_on = v;
    }
    if let Some(v) = r.choice("http.confirmRequests", &["never", "remote", "always"]) {
        s.http_confirm = v;
    }
    if let Some(v) = r.choice("ui.language", crate::i18n::SETTING_CHOICES) {
        s.ui_language = v;
    }
    if let Some(v) = r.choice("update.check", &["ask", "daily", "manual"]).and_then(|v| v.parse().ok()) {
        s.update_check = v;
    }
    if let Some(v) = r.choice("explorer.sortOrder", &["default", "filesFirst", "mixed", "type"]) {
        s.sort_order = v;
    }
    if let Some(v) = r.bool("explorer.openEditors") {
        s.open_editors = v;
    }
    if let Some(v) = r.numbers("editor.rulers", 1., 500.) {
        s.rulers = v;
    }
    if let Some(v) = r.choice("editor.renderWhitespace", &["none", "boundary", "all"]) {
        s.render_whitespace = v;
    }
    if let Some(v) = r.choice("editor.lineNumbers", &["on", "off", "relative"]) {
        s.line_numbers = v;
    }
    if let Some(v) = r.choice("editor.cursorStyle", &["line", "block", "underline"]) {
        s.cursor_style = v;
    }
    if let Some(v) = r.bool("editor.bracketPairColorization") {
        s.bracket_colors = v;
    }
    if let Some(v) = r.bool("editor.stickyScroll.enabled") {
        s.sticky_scroll = v;
    }
    if let Some(v) = r.bool("editor.colorDecorators") {
        s.color_decorators = v;
    }
    if let Some(v) = r.choice("workbench.iconTheme", &["codeyard", "badges", "colorful", "minimal", "apple", "material", "none"]) {
        s.icon_theme = v;
    }
    if let Some(v) = r.bool("workbench.statusBar.visible") {
        s.status_bar = v;
    }
    if let Some(v) = r.bool("workbench.activityBar.visible") {
        s.activity_bar = v;
    }
    if let Some(v) = r.number("workbench.tree.indent", 8., 32.) {
        s.tree_indent = v as f32;
    }
    if let Some(v) = r.bool("explorer.confirmDelete") {
        s.confirm_delete = v;
    }
    if let Some(v) = r.bool("explorer.confirmDragAndDrop") {
        s.confirm_drag_and_drop = v;
    }
    if let Some(v) = r.bool("spelling.enabled") {
        s.spelling = v;
    }
    if let Some(v) = r.bool("spelling.checkComments") {
        s.spelling_comments = v;
    }
    // --- data ---
    if let Some(v) = r.number("data.indent", 1., 8.) {
        s.data_indent = v as usize;
    }
    if let Some(v) = r.bool("data.sortKeys") {
        s.data_sort_keys = v;
    }
    if let Some(v) = r.bool("json.validate.enable") {
        s.schema_validate = v;
    }
    if let Some(v) = r.bool("json.schemaDownload.enable") {
        s.schema_download = v;
    }
    if let Some(v) = r.bool("workbench.tree.guides") {
        s.tree_guides = v;
    }
    if let Some(v) = r.bool("git.graph.showAuthor") {
        s.graph_show_author = v;
    }
    if let Some(v) = r.bool("git.graph.showHash") {
        s.graph_show_hash = v;
    }
    if let Some(v) = r.choice("git.graph.view", &["graph", "compact", "dates"]) {
        s.graph_view = v;
    }
    if let Some(v) = r.bool("search.smartCase") {
        s.search_smart_case = v;
    }
    if let Some(v) = r.bool("search.showLineNumbers") {
        s.search_line_numbers = v;
    }
    if let Some(v) = r.choice("search.collapseResults", &["auto", "alwaysCollapse", "alwaysExpand"]) {
        s.search_collapse = v;
    }
    if let Some(v) = r.number("git.graph.limit", 20., 5000.) {
        s.graph_limit = v as usize;
    }
    if let Some(v) = r.number("git.timeline.limit", 5., 1000.) {
        s.timeline_limit = v as usize;
    }
    if let Some(v) = r.bool("git.inlineBlame") {
        s.inline_blame = v;
    }
    if let Some(v) = r.bool("git.graph.allBranches") {
        s.graph_all_branches = v;
    }
    if let Some(v) = r.choice("editor.renderLineHighlight", &["line", "none"]) {
        s.render_line_highlight = v == "line";
    }
    if let Some(v) = r.bool("editor.matchBrackets") {
        s.match_brackets = v;
    }
    if let Some(v) = r.bool("editor.folding") {
        s.folding = v;
    }
    let mut cw: Vec<String> = Vec::new();
    s.ui_font = r.string("workbench.fontFamily");
    if let Some(v) = r.bool("workbench.editor.showTabs") {
        s.show_tabs = v;
    }
    if let Some(v) = r.bool("workbench.tabs.showIcons") {
        s.tab_icons = v;
    }
    if let Some(v) = r.choice("workbench.tabs.closeButton", &["hover", "always", "off"]) {
        s.tab_close = v;
    }
    if let Some(v) = r.bool("workbench.breadcrumbs.enabled") {
        s.breadcrumbs = v;
    }
    if let Some(v) = r.number("editor.cursorWidth", 1., 8.) {
        s.cursor_width = v as f32;
    }
    if let Some(v) = r.bool("editor.hover.enabled") {
        s.hover_enabled = v;
    }
    if let Some(v) = r.bool("editor.quickSuggestions") {
        s.quick_suggestions = v;
    }
    if let Some(v) = r.strings("search.exclude") {
        s.search_exclude = v;
    }
    match obj.get("files.associations") {
        None | Some(Value::Null) => {}
        Some(Value::Object(m)) => {
            for (glob, lang) in m {
                match lang.as_str() {
                    Some(l) if !l.trim().is_empty() => s.file_associations.push((glob.clone(), l.trim().to_string())),
                    _ => cw.push(crate::i18n::tf("files.associations.{}: expected a language name or extension", &[&glob])),
                }
            }
        }
        Some(_) => cw.push(crate::i18n::tf("{0}: expected an object {1}", &[&"files.associations", &"{ \"*.tpl\": \"html\" }"])),
    }
    match obj.get("workbench.colorCustomizations") {
        None | Some(Value::Null) => {}
        Some(Value::Object(m)) => {
            let mut add = |scope: Option<String>, colors: &Map<String, Value>, warnings: &mut Vec<String>| {
                for (role, v) in colors {
                    if !theme::ROLES.contains(&role.as_str()) {
                        warnings.push(crate::i18n::tf(
                            "workbench.colorCustomizations: unknown color role {0} (see crates/theme/design/tokens.json: {1})",
                            &[&format!("\"{role}\""), &theme::ROLES.join(", ")],
                        ));
                        continue;
                    }
                    match v.as_str().and_then(|h| theme::themes::parse_color(h, 0)) {
                        Some(c) => s.color_overrides.push((scope.clone(), role.clone(), c)),
                        None => warnings
                            .push(crate::i18n::tf("{0}: expected a color like {1}", &[&format!("workbench.colorCustomizations.{role}"), &"\"#ff8800\""])),
                    }
                }
            };
            for (k, v) in m {
                match (k.strip_prefix('[').and_then(|k| k.strip_suffix(']')), v) {
                    (Some(theme_name), Value::Object(inner)) => add(Some(theme_name.to_string()), inner, &mut cw),
                    (Some(_), _) => cw.push(crate::i18n::tf("{}: expected an object", &[&format!("workbench.colorCustomizations.{k}")])),
                    (None, _) => add(None, &Map::from_iter([(k.clone(), v.clone())]), &mut cw),
                }
            }
        }
        Some(_) => cw.push(crate::i18n::tf("{0}: expected an object {1}", &[&"workbench.colorCustomizations", &"{ \"accent\": \"#ff8800\" }"])),
    }
    for w in cw {
        r.warn(w);
    }
    if let Some(v) = r.bool("ports.autoForward") {
        s.ports_auto_forward = v;
    }
    if let Some(v) = r.bool("lsp.enabled") {
        s.lsp_enabled = v;
    }
    match obj.get("lsp.servers") {
        None | Some(Value::Null) => {}
        Some(Value::Object(m)) => {
            for (lang, cfg) in m {
                let entry = match cfg {
                    Value::Null => Some(None),
                    Value::String(cmd) if !cmd.trim().is_empty() => Some(Some((cmd.clone(), Vec::new()))),
                    Value::Object(o) => match o.get("command").and_then(Value::as_str) {
                        Some(cmd) => {
                            let args = o
                                .get("args")
                                .and_then(Value::as_array)
                                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                                .unwrap_or_default();
                            Some(Some((cmd.to_string(), args)))
                        }
                        None => None,
                    },
                    _ => None,
                };
                match entry {
                    Some(e) => s.lsp_servers.push((lang.clone(), e)),
                    None => warnings.push(format!(
                        "settings.jsonc: {}",
                        crate::i18n::tf("{0}: expected {1} or null", &[&format!("lsp.servers.{lang}"), &"{ \"command\": \"…\", \"args\": [] }"])
                    )),
                }
            }
        }
        Some(_) => warnings.push(format!("settings.jsonc: {}", crate::i18n::tf("{}: expected an object", &[&"lsp.servers"]))),
    }
    match obj.get("spelling.languages") {
        None | Some(Value::Null) => {}
        Some(Value::Array(a)) => {
            let ids: Vec<String> = a.iter().filter_map(|x| x.as_str()).filter(|id| spell::valid_id(id)).map(str::to_string).collect();
            if ids.len() != a.len() {
                warnings.push(format!(
                    "settings.jsonc: {}",
                    crate::i18n::tf("{0}: expected dictionary names like {1}", &[&"spelling.languages", &"\"en\", \"tr\", \"de\""])
                ));
            }
            if !ids.is_empty() {
                s.spell_languages = ids;
            }
        }
        Some(_) => {
            warnings.push(format!("settings.jsonc: {}", crate::i18n::tf("{0}: expected an array like {1}", &[&"spelling.languages", &"[\"en\", \"tr\"]"])))
        }
    }
    // `json.schemas`: [{ "fileMatch": ["*.cfg.json"], "url": "./schema.json" }]; `yaml.schemas`: { "url": "glob" | ["glob"] }.
    match obj.get("json.schemas") {
        None | Some(Value::Null) => {}
        Some(Value::Array(a)) => {
            for item in a {
                let globs: Vec<String> = match item.get("fileMatch") {
                    Some(Value::Array(g)) => g.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
                    Some(Value::String(g)) => vec![g.clone()],
                    _ => Vec::new(),
                };
                match item.get("url").and_then(Value::as_str) {
                    Some(url) if !globs.is_empty() && !url.trim().is_empty() => s.schema_associations.push((globs, url.trim().to_string())),
                    _ => warnings.push(format!(
                        "settings.jsonc: {}",
                        crate::i18n::tf("{0}: each entry needs {1} and {2}", &[&"json.schemas", &"\"fileMatch\"", &"\"url\""])
                    )),
                }
            }
        }
        Some(_) => warnings.push(format!(
            "settings.jsonc: {}",
            crate::i18n::tf("{0}: expected an array of {1}", &[&"json.schemas", &"{ \"fileMatch\": [...], \"url\": \"...\" }"])
        )),
    }
    match obj.get("yaml.schemas") {
        None | Some(Value::Null) => {}
        Some(Value::Object(m)) => {
            for (url, globs) in m {
                let globs: Vec<String> = match globs {
                    Value::Array(g) => g.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
                    Value::String(g) => vec![g.clone()],
                    _ => Vec::new(),
                };
                if globs.is_empty() {
                    warnings.push(format!(
                        "settings.jsonc: {}",
                        crate::i18n::tf("{}: expected a file pattern or a list of patterns", &[&format!("yaml.schemas.{url}")])
                    ));
                } else {
                    s.schema_associations.push((globs, url.clone()));
                }
            }
        }
        Some(_) => {
            warnings.push(format!("settings.jsonc: {}", crate::i18n::tf("{0}: expected an object {1}", &[&"yaml.schemas", &"{ \"schema-url\": \"pattern\" }"])))
        }
    }
    const KNOWN: &[&str] = &[
        "ui.language",
        "update.check",
        "http.runOn",
        "http.confirmRequests",
        "spelling.enabled",
        "spelling.checkComments",
        "spelling.languages",
        // --- data ---
        "data.indent",
        "data.sortKeys",
        "json.validate.enable",
        "json.schemaDownload.enable",
        "json.schemas",
        "yaml.schemas",
        "theme",
        "editor.fontFamily",
        "editor.fontSize",
        "editor.lineHeight",
        "editor.tabSize",
        "editor.insertSpaces",
        "editor.wordHighlight",
        "editor.indentGuides",
        "files.trimTrailingWhitespace",
        "files.insertFinalNewline",
        "files.autoSave",
        "files.autoSaveDelay",
        "files.exclude",
        "files.hotExit",
        "search.maxResults",
        // --- git-sync ---
        "git.autofetch",
        "git.autofetchPeriod",
        "git.confirmSync",
        "git.rebaseWhenSync",
        "git.pruneOnFetch",
        "git.pullTags",
        "git.fetchOnPull",
        "git.postCommitCommand",
        "git.enableSmartCommit",
        // --- structural ---
        "search.structural.maxFiles",
        "compare.folders.ignore",
        "compare.folders.maxFiles",
        "terminal.shell",
        "terminal.fontSize",
        "terminal.scrollback",
        "window.restoreSession",
        "window.startup",
        "lsp.enabled",
        "lsp.servers",
        "ports.autoForward",
        "editor.autoClosingBrackets",
        "editor.formatOnSave",
        "editor.formatOnPaste",
        "editor.formatOnType",
        "editor.codeActionsOnSave",
        "editor.editorconfig",
        "editor.inlayHints.enabled",
        "editor.linkedEditing",
        "editor.codeLens",
        "editor.unicodeHighlight.enabled",
        "editor.semanticHighlighting.enabled",
        "emmet.triggerExpansionOnTab",
        "editor.fontLigatures",
        "editor.caseLocale",
        "diffEditor.renderSideBySide",
        "editor.minimap.enabled",
        "editor.wordWrap",
        "explorer.autoReveal",
        "files.fallbackEncoding",
        "editor.cursorBlinking",
        "files.enableTrash",
        "editor.wordBasedSuggestions",
        "explorer.compactFolders",
        "explorer.sortOrder",
        "explorer.openEditors",
        "editor.rulers",
        "editor.renderWhitespace",
        "editor.lineNumbers",
        "editor.cursorStyle",
        "editor.bracketPairColorization",
        "editor.stickyScroll.enabled",
        "editor.colorDecorators",
        "workbench.iconTheme",
        "workbench.startupEditor",
        "workbench.statusBar.visible",
        "workbench.activityBar.visible",
        "workbench.tree.indent",
        "workbench.fontFamily",
        "workbench.editor.showTabs",
        "workbench.tabs.showIcons",
        "workbench.tabs.closeButton",
        "workbench.breadcrumbs.enabled",
        "editor.cursorWidth",
        "editor.hover.enabled",
        "editor.quickSuggestions",
        "search.exclude",
        "files.associations",
        "workbench.colorCustomizations",
        "security.workspaceTrust.enabled",
        "explorer.confirmDelete",
        "explorer.confirmDragAndDrop",
        "workbench.tree.guides",
        "git.graph.showAuthor",
        "git.graph.showHash",
        "git.graph.view",
        "search.smartCase",
        "search.showLineNumbers",
        "search.collapseResults",
        "git.graph.limit",
        "git.timeline.limit",
        "git.graph.allBranches",
        "git.inlineBlame",
        "editor.renderLineHighlight",
        "editor.matchBrackets",
        "editor.folding",
    ];
    for k in obj.keys().filter(|k| !KNOWN.contains(&k.as_str()) && !quiet.contains(k)) {
        warnings.push(format!("settings.jsonc: {}", crate::i18n::tf("unknown setting {}", &[&format!("\"{k}\"")])));
    }
    (s, warnings)
}

struct Reader<'a> {
    obj: &'a Map<String, Value>,
    warnings: &'a mut Vec<String>,
}

impl Reader<'_> {
    fn warn(&mut self, w: String) {
        self.warnings.push(format!("settings.jsonc: {w}"));
    }

    fn get(&self, key: &str) -> Option<&Value> {
        self.obj.get(key).filter(|v| !v.is_null())
    }

    fn string(&mut self, key: &str) -> Option<String> {
        match self.get(key)? {
            Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
            Value::String(_) => None,
            _ => {
                self.warn(crate::i18n::tf("{}: expected text", &[&key]));
                None
            }
        }
    }

    fn bool(&mut self, key: &str) -> Option<bool> {
        match self.get(key)? {
            Value::Bool(b) => Some(*b),
            _ => {
                self.warn(crate::i18n::tf("{}: expected true or false", &[&key]));
                None
            }
        }
    }

    fn number(&mut self, key: &str, min: f64, max: f64) -> Option<f64> {
        match self.get(key)?.as_f64() {
            Some(n) if (min..=max).contains(&n) => Some(n),
            Some(n) => {
                self.warn(crate::i18n::tf("{}: {} is out of range ({}–{})", &[&key, &n, &min, &max]));
                None
            }
            None => {
                self.warn(crate::i18n::tf("{}: expected a number", &[&key]));
                None
            }
        }
    }

    /// One of the fixed options; any other value produces a warning.
    fn choice(&mut self, key: &str, options: &[&str]) -> Option<String> {
        let v = self.string(key)?;
        if options.contains(&v.as_str()) {
            Some(v)
        } else {
            self.warn(crate::i18n::tf("{}: unknown value {} ({})", &[&key, &format!("\"{v}\""), &options.join(", ")]));
            None
        }
    }

    fn numbers(&mut self, key: &str, min: f64, max: f64) -> Option<Vec<usize>> {
        match self.get(key)? {
            Value::Array(a) if a.iter().all(|v| v.as_f64().is_some_and(|n| (min..=max).contains(&n))) => {
                let mut v: Vec<usize> = a.iter().filter_map(Value::as_f64).map(|n| n as usize).collect();
                v.sort_unstable();
                v.dedup();
                Some(v)
            }
            _ => {
                self.warn(crate::i18n::tf("{}: expected a list of numbers ({}–{})", &[&key, &min, &max]));
                None
            }
        }
    }

    /// `{ "source.organizeImports": "explicit" }` (VS Code) or `["source.organizeImports"]`.
    /// Entries that are `false` / `"never"` are skipped.
    fn code_actions(&mut self, key: &str) -> Option<Vec<String>> {
        match self.get(key)? {
            Value::Object(o) => Some(
                o.iter()
                    .filter(|(k, v)| k.starts_with("source") && !matches!(v, Value::Bool(false)) && v.as_str() != Some("never"))
                    .map(|(k, _)| k.clone())
                    .collect(),
            ),
            Value::Array(a) if a.iter().all(Value::is_string) => Some(a.iter().filter_map(Value::as_str).map(str::to_string).collect()),
            _ => {
                self.warn(crate::i18n::tf("{0}: expected {1} or a list of kinds", &[&key, &"{ \"source.organizeImports\": \"explicit\" }"]));
                None
            }
        }
    }

    fn strings(&mut self, key: &str) -> Option<Vec<String>> {
        match self.get(key)? {
            Value::Array(a) if a.iter().all(Value::is_string) => Some(a.iter().filter_map(Value::as_str).map(str::to_string).collect()),
            _ => {
                self.warn(crate::i18n::tf("{}: expected a list of text", &[&key]));
                None
            }
        }
    }
}

/// Change a text setting in the file; comments and other lines are preserved.
/// If the key is missing it is inserted after the `{`.
pub fn set_string_in(text: &str, key: &str, value: &str) -> String {
    set_value_in(text, key, &Value::String(value.to_string()))
}

/// Any JSON value (number, array, object). Even if the existing value spans several
/// lines (an array), the whole of it is replaced by tracking brackets and strings.
pub fn set_value_in(text: &str, key: &str, value: &Value) -> String {
    let quoted_key = format!("\"{key}\"");
    let new_value = serde_json::to_string(value).unwrap_or_default();
    // Find the real line, not a key inside a comment line.
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("//")
            && trimmed.starts_with(&quoted_key)
            && let Some(colon) = line[line.len() - trimmed.len() + quoted_key.len()..].find(':').map(|c| c + line.len() - trimmed.len() + quoted_key.len())
        {
            let after = &line[colon + 1..];
            let v_start = offset + colon + 1 + (after.len() - after.trim_start().len());
            let v_end = v_start + value_len(&text[v_start..]);
            return format!("{}{}{}", &text[..v_start], new_value, &text[v_end..]);
        }
        offset += line.len();
    }
    match text.find('{') {
        Some(i) => format!("{}\n  {quoted_key}: {new_value},{}", &text[..=i], &text[i + 1..]),
        None => format!("{{\n  {quoted_key}: {new_value}\n}}\n"),
    }
}

/// Length of a JSON value (bytes): string, nested array/object or bare value.
fn value_len(rest: &str) -> usize {
    let bytes = rest.as_bytes();
    let (mut depth, mut in_str, mut esc) = (0i32, false, false);
    for (i, &b) in bytes.iter().enumerate() {
        if in_str {
            match (esc, b) {
                (true, _) => esc = false,
                (false, b'\\') => esc = true,
                (false, b'"') => {
                    in_str = false;
                    if depth == 0 {
                        return i + 1;
                    }
                }
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'[' | b'{' => depth += 1,
            b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
                if depth < 0 {
                    return i; // end of the object: the bare value is over
                }
            }
            b',' | b'\n' | b'/' if depth == 0 => return i,
            _ => {}
        }
    }
    rest.len()
}

/// User shortcut: (keys, command id or disable, context).
#[derive(Debug, Clone, PartialEq)]
pub struct UserBinding {
    pub key: String,
    /// `None`: disable this key.
    pub command: Option<String>,
    pub when: Option<String>,
}

pub fn parse_keybindings(text: &str) -> (Vec<UserBinding>, Vec<String>) {
    let mut warnings = Vec::new();
    let value: Value = match serde_json::from_str(&theme::themes::strip_jsonc(text)) {
        Ok(v) => v,
        Err(e) => return (Vec::new(), vec![format!("keybindings.jsonc: {e}")]),
    };
    let Some(items) = value.as_array() else {
        return (Vec::new(), vec![format!("keybindings.jsonc: {}", crate::i18n::t("expected a list [ … ]"))]);
    };
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        // VS Code syntax ("cmd+shift+p", "cmd+k cmd+t") is accepted too: converted to ours.
        let converted = item.get("key").and_then(Value::as_str).filter(|k| k.contains('+')).and_then(crate::vscode::map_key);
        let key = converted.as_deref().or_else(|| item.get("key").and_then(Value::as_str));
        let command = item.get("command");
        let when = item.get("when").and_then(Value::as_str).map(str::to_string);
        match (key, command) {
            (Some(k), Some(Value::String(c))) => out.push(UserBinding { key: k.into(), command: Some(c.clone()), when }),
            (Some(k), Some(Value::Null)) => out.push(UserBinding { key: k.into(), command: None, when }),
            _ => warnings.push(format!("keybindings.jsonc: {}", crate::i18n::tf("entry {0} needs {1} and {2}", &[&(i + 1), &"\"key\"", &"\"command\""]))),
        }
    }
    (out, warnings)
}

/// Wildcard matching, non-recursive: `star` swallows anything (empty too), `one` matches a single item. Two pointers
/// backtracking to the last star: O(pattern × input) worst case. The recursive version went exponential on patterns like `*a*a*a*b`, and the pattern
/// can come from a project file (`.vscode/settings.json`).
fn glob_match<P, N>(p: &[P], n: &[N], star: impl Fn(&P) -> bool, one: impl Fn(&P, &N) -> bool) -> bool {
    let (mut pi, mut ni) = (0, 0);
    let mut last_star: Option<(usize, usize)> = None;
    while ni < n.len() {
        if pi < p.len() && star(&p[pi]) {
            last_star = Some((pi, ni));
            pi += 1;
        } else if pi < p.len() && one(&p[pi], &n[ni]) {
            pi += 1;
            ni += 1;
        } else if let Some((sp, sn)) = last_star {
            pi = sp + 1;
            ni = sn + 1;
            last_star = Some((sp, sn + 1));
        } else {
            return false;
        }
    }
    while pi < p.len() && star(&p[pi]) {
        pi += 1;
    }
    pi == p.len()
}

/// Simple glob: `*` and `?`; a pattern without `/` applies to the file/folder name.
pub fn name_matches(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.trim_end_matches('/').chars().collect();
    let n: Vec<char> = name.chars().collect();
    glob_match(&p, &n, |c| *c == '*', |c, x| *c == '?' || c == x)
}

/// Does the root-relative path match the pattern: `**` is zero or more folders, `*`/`?` within one segment.
pub fn path_matches(pattern: &str, rel_path: &str) -> bool {
    let p: Vec<&str> = pattern.trim_start_matches("./").trim_matches('/').split('/').collect();
    let n: Vec<&str> = rel_path.trim_matches('/').split('/').collect();
    glob_match(&p, &n, |s| *s == "**", |a, b| name_matches(a, b))
}

/// Is the path excluded: a pattern without "/" matches the name at every level (".git", "*.log"), one with it
/// (like VS Code's "**/node_modules", "build/out") matches the root-relative path; everything
/// under a matching folder is excluded too.
pub fn is_excluded(excludes: &[String], rel_path: &str) -> bool {
    let parts: Vec<&str> = rel_path.trim_matches('/').split('/').collect();
    excludes.iter().any(|e| {
        if e.trim_end_matches('/').contains('/') {
            (1..=parts.len()).any(|k| path_matches(e, &parts[..k].join("/")))
        } else {
            parts.iter().any(|part| name_matches(e, part))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_are_linear_even_for_hostile_patterns() {
        let t = std::time::Instant::now();
        assert!(!name_matches("*a*a*a*a*a*a*a*a*a*a*a*a*a*a*a*b", &"a".repeat(60)));
        assert!(name_matches("*a*a*a*a*a*a*a*a*a*a*a*a*a*a*a*", &"a".repeat(60)));
        assert!(!path_matches("**/**/**/**/**/**/**/**/**/**/**/**/x", &vec!["d"; 60].join("/")));
        assert!(path_matches("**/**/**/**/**/**/**/d", &vec!["d"; 60].join("/")));
        assert!(!name_matches(&format!("{}b", "?*".repeat(200)), &"a".repeat(2000)));
        assert!(t.elapsed().as_millis() < 1000 * u128::from(crate::test_sshd::timeout_scale()), "took {:?}", t.elapsed());
    }

    #[test]
    fn globs_keep_their_meaning() {
        for (p, n, want) in [
            ("*.log", "a.log", true),
            ("*.log", "a.logx", false),
            ("a?c", "abc", true),
            ("a?c", "ac", false),
            ("*", "", true),
            ("a*", "a", true),
            ("*a", "ba", true),
            ("*a", "bab", false),
            ("", "", true),
            ("", "x", false),
            ("build/", "build", true),
            ("ç*ş", "çaşış", true),
        ] {
            assert_eq!(name_matches(p, n), want, "{p} vs {n}");
        }
        for (p, n, want) in [
            ("**/node_modules", "node_modules", true),
            ("**/node_modules", "a/b/node_modules", true),
            ("**/node_modules", "a/node_modules/x", false),
            ("src/**/*.rs", "src/a/b/c.rs", true),
            ("src/**/*.rs", "src/c.rs", true),
            ("src/**/*.rs", "lib/c.rs", false),
            ("a/*/c", "a/b/c", true),
            ("a/*/c", "a/b/x/c", false),
            ("./build/out", "build/out", true),
        ] {
            assert_eq!(path_matches(p, n), want, "{p} vs {n}");
        }
    }

    #[test]
    fn color_customizations_parse_globally_and_per_theme_and_report_mistakes() {
        let text =
            r##"{ "workbench.colorCustomizations": { "accent": "#ff8800", "[Quarry]": { "syntax-string": "#a6e22e" }, "nope": "#fff", "text": "red" } }"##;
        let (s, w) = parse_layers(text, &[]).unwrap();
        assert_eq!(s.color_overrides.len(), 2);
        assert!(s.color_overrides.contains(&(None, "accent".into(), 0xff8800)));
        assert!(s.color_overrides.contains(&(Some("Quarry".into()), "syntax-string".into(), 0xa6e22e)));
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(w.iter().any(|x| x.contains("unknown color role")) && w.iter().any(|x| x.contains("expected a color")));
    }

    #[test]
    fn customization_settings_are_read() {
        let (s, w) = parse_layers(r#"{ "workbench.tabs.closeButton": "off", "editor.cursorWidth": 4, "workbench.fontFamily": "Inter", "files.associations": { "*.tpl": "html" }, "search.exclude": ["dist"], "editor.hover.enabled": false }"#, &[]).unwrap();
        assert!(w.is_empty(), "{w:?}");
        assert_eq!((s.tab_close.as_str(), s.cursor_width, s.ui_font.as_deref()), ("off", 4., Some("Inter")));
        assert_eq!(s.file_associations, [("*.tpl".to_string(), "html".to_string())]);
        assert_eq!((s.search_exclude.as_slice(), s.hover_enabled), (&["dist".to_string()][..], false));
        let (_, w) = parse_layers(r#"{ "workbench.tabs.closeButton": "sometimes", "editor.cursorWidth": 99 }"#, &[]).unwrap();
        assert_eq!(w.len(), 2);
    }

    /// A repository must not be able to switch the request confirmation off (or pick its own value): user settings only.
    #[test]
    fn http_confirm_requests_parses_and_cannot_come_from_a_project() {
        assert_eq!(Settings::default().http_confirm, "remote", "the default asks for the first request to a new remote host");
        for v in ["never", "remote", "always"] {
            let (s, w) = parse_layers(&format!(r#"{{ "http.confirmRequests": "{v}" }}"#), &[]).unwrap();
            assert_eq!((s.http_confirm.as_str(), w.len()), (v, 0), "{w:?}");
        }
        let (s, w) = parse_layers(r#"{ "http.confirmRequests": "sometimes" }"#, &[]).unwrap();
        assert_eq!(s.http_confirm, "remote", "an invalid value keeps the default");
        assert_eq!(w.len(), 1, "{w:?}");
        let project = [(".codeyard/settings.jsonc", r#"{ "http.confirmRequests": "never" }"#, true)];
        for trusted in [false, true] {
            let (s, w) = parse_layers_trusted("{}", &project, trusted).unwrap();
            assert_eq!(s.http_confirm, "remote", "trusted={trusted}: a folder cannot turn the question off");
            assert!(w.iter().any(|m| m.contains("only be set in your own")), "{w:?}");
        }
        let (s, _) = parse_layers_trusted(r#"{ "http.confirmRequests": "always" }"#, &project, true).unwrap();
        assert_eq!(s.http_confirm, "always", "the user's own value stands");
    }

    /// `update.check` decides whether the app contacts github.com by itself: a repository must never choose it, trusted or not.
    #[test]
    fn update_check_parses_and_cannot_come_from_a_project() {
        use crate::update::Policy;
        assert_eq!(Settings::default().update_check, Policy::Ask, "undecided until the user answers");
        for (v, want) in [("ask", Policy::Ask), ("daily", Policy::Daily), ("manual", Policy::Manual)] {
            let (s, w) = parse_layers(&format!(r#"{{ "update.check": "{v}" }}"#), &[]).unwrap();
            assert_eq!((s.update_check, w.len()), (want, 0), "{w:?}");
        }
        let (s, w) = parse_layers(r#"{ "update.check": "weekly" }"#, &[]).unwrap();
        assert_eq!(s.update_check, Policy::Ask, "an invalid value keeps the default");
        assert_eq!(w.len(), 1, "{w:?}");
        let project =
            [(".codeyard/settings.jsonc", r#"{ "update.check": "daily" }"#, true), (".vscode/settings.json", r#"{ "update.check": "daily" }"#, false)];
        for trusted in [false, true] {
            let (s, w) = parse_layers_trusted(r#"{ "update.check": "manual" }"#, &project, trusted).unwrap();
            assert_eq!(s.update_check, Policy::Manual, "trusted={trusted}: the user's own value wins over every folder file");
            assert!(w.iter().any(|m| m.contains("only be set in your own")), "{w:?}");
            let (s, _) = parse_layers_trusted("{}", &project, trusted).unwrap();
            assert_eq!(s.update_check, Policy::Ask, "trusted={trusted}: a folder cannot turn network checks on");
        }
    }

    #[test]
    fn untrusted_project_layer_drops_restricted_keys_and_says_so() {
        let project = [(
            ".codeyard/settings.jsonc",
            r#"{ "terminal.shell": "/x", "lsp.servers": { "c": { "command": "/y" } }, "security.workspaceTrust.enabled": false, "editor.tabSize": 2 }"#,
            true,
        )];
        let (s, w) = parse_layers_trusted("{}", &project, false).unwrap();
        assert_eq!((s.terminal_shell, s.lsp_servers.len(), s.tab_size), (None, 0, Some(2)));
        assert_eq!(w.iter().filter(|m| m.contains("ignored until you trust")).count(), 2, "{w:?}");
        assert_eq!(w.iter().filter(|m| m.contains("only be set in your own")).count(), 1, "{w:?}");
        // `parse_layers` (no trust given) is the safe default.
        assert_eq!(parse_layers("{}", &project).unwrap().0.terminal_shell, None);
        let (s, w) = parse_layers_trusted("{}", &project, true).unwrap();
        assert_eq!((s.terminal_shell.as_deref(), s.lsp_servers.len()), (Some("/x"), 1));
        assert!(s.workspace_trust == Settings::default().workspace_trust, "a trusted project still cannot flip the gate");
        assert_eq!(w.len(), 1, "{w:?}");
        // The user's own file is never restricted.
        let (s, _) = parse_layers_trusted(r#"{ "terminal.shell": "/mine" }"#, &[], false).unwrap();
        assert_eq!(s.terminal_shell.as_deref(), Some("/mine"));
    }

    #[test]
    fn project_layers_override_user_settings() {
        let user = r#"{ "editor.tabSize": 4, "theme": "ember" }"#;
        let (s, w) = parse_layers(
            user,
            &[
                (".vscode/settings.json", r#"{ "editor.tabSize": 2, "python.linting": true }"#, false),
                (".codeyard/settings.jsonc", r#"{ "editor.fontSize": 15 }"#, true),
            ],
        )
        .unwrap();
        assert_eq!((s.tab_size, s.theme.as_str(), s.editor_font_size), (Some(2), "ember", 15.));
        assert!(w.is_empty(), "extension settings do not warn: {w:?}");
        let (_, w) = parse_layers(user, &[(".codeyard/settings.jsonc", "{ bozuk", true)]).unwrap();
        assert_eq!(w.len(), 1, "a broken project file only warns");
    }

    #[test]
    fn exclude_globs_like_vscode() {
        let ex = |p: &[&str], path: &str| is_excluded(&p.iter().map(|s| s.to_string()).collect::<Vec<_>>(), path);
        assert!(ex(&["**/node_modules"], "node_modules"));
        assert!(ex(&["**/node_modules"], "web/node_modules/react/index.js"));
        assert!(ex(&["node_modules"], "a/node_modules"), "adla her seviyede");
        assert!(ex(&["build/out"], "build/out/x.o"));
        assert!(!ex(&["build/out"], "src/build/out"), "relative to the root");
        assert!(ex(&["**/*.log"], "logs/a.log"));
        assert!(!ex(&["**/node_modules"], "src/node_modules_x/a"));
    }

    #[test]
    fn template_parses_cleanly() {
        let (s, w) = parse(TEMPLATE);
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(s, Settings::default());
        assert!(parse_keybindings(KEYBINDINGS_TEMPLATE).1.is_empty());
    }

    #[test]
    fn bad_values_fall_back_and_warn() {
        let (s, w) = parse(r#"{ "editor.fontSize": "büyük", "editor.lineHeight": 500, "files.autoSave": "sometimes", "bilinmeyen": 1, "editor.tabSize": 2 }"#);
        assert_eq!(s.editor_font_size, Settings::default().editor_font_size);
        assert_eq!(s.editor_line_height, Settings::default().editor_line_height);
        assert_eq!(s.auto_save, AutoSave::Off);
        assert_eq!(s.tab_size, Some(2), "valid ones are applied");
        assert_eq!(w.len(), 4, "{w:?}");
    }

    #[test]
    fn auto_save_with_delay() {
        let (s, _) = parse(r#"{ "files.autoSave": "afterDelay", "files.autoSaveDelay": 500 }"#);
        assert_eq!(s.auto_save, AutoSave::AfterDelay(500));
    }

    #[test]
    fn set_string_keeps_comments_and_other_lines() {
        let text = "{\n  // \"theme\": \"eski yorum\"\n  \"theme\": \"auto\", // seçim\n  \"x\": 1,\n}\n";
        let out = set_string_in(text, "theme", "fjord");
        assert_eq!(out, "{\n  // \"theme\": \"eski yorum\"\n  \"theme\": \"fjord\", // seçim\n  \"x\": 1,\n}\n");
        let added = set_string_in("{\n  \"x\": 1\n}\n", "theme", "paper");
        assert_eq!(parse(&added).0.theme, "paper");
    }

    #[test]
    fn keybindings_and_unbinding() {
        let (b, w) = parse_keybindings(r#"[{"key":"cmd-1","command":"editor.save","when":"Editor"},{"key":"ctrl-g","command":null},{"key":"x"}]"#);
        assert_eq!(b.len(), 2);
        assert_eq!(b[1].command, None);
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn excludes() {
        let ex = vec!["node_modules".to_string(), "*.log".to_string(), ".git".to_string()];
        assert!(is_excluded(&ex, "web/node_modules/x/i.js"));
        assert!(is_excluded(&ex, "logs/build.log"));
        assert!(!is_excluded(&ex, "src/main.rs"));
        assert!(name_matches("*.min.js", "app.min.js"));
    }

    #[test]
    fn set_value_replaces_multiline_values() {
        let text = "{\n  // yorum: \"files.exclude\": []\n  \"files.exclude\": [\n    \".git\",\n    \"target\"\n  ],\n  \"editor.fontSize\": 13,\n}\n";
        let out = set_value_in(text, "files.exclude", &serde_json::json!(["**/.git"]));
        assert!(out.contains("\"files.exclude\": [\"**/.git\"],\n  \"editor.fontSize\": 13"), "{out}");
        assert!(out.contains("// yorum: \"files.exclude\": []"), "comment is preserved");
        let out = set_value_in(&out, "editor.fontSize", &serde_json::json!(15));
        let (s, w) = parse(&out);
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(s.editor_font_size, 15.);
        assert_eq!(s.files_exclude, vec!["**/.git".to_string()]);
        let out = set_value_in(&out, "editor.formatOnSave", &serde_json::json!(true));
        assert!(parse(&out).0.format_on_save, "added when missing");
    }
}

#[cfg(test)]
mod code_actions_on_save {
    use super::*;

    #[test]
    fn reads_vscode_object_and_plain_list() {
        let (s, w) = parse(
            r#"{ "editor.codeActionsOnSave": { "source.organizeImports": "explicit", "source.fixAll": true, "source.sortMembers": false, "source.x": "never", "quickfix": true } }"#,
        );
        assert!(w.is_empty(), "{w:?}");
        let mut got = s.code_actions_on_save.clone();
        got.sort();
        assert_eq!(got, ["source.fixAll", "source.organizeImports"], "disabled and non-source entries are skipped");
        let (s, _) = parse(r#"{ "editor.codeActionsOnSave": ["source.organizeImports"] }"#);
        assert_eq!(s.code_actions_on_save, ["source.organizeImports"]);
        let (s, w) = parse(r#"{ "editor.codeActionsOnSave": 3 }"#);
        assert!(s.code_actions_on_save.is_empty() && w.len() == 1, "{w:?}");
    }
}

#[cfg(test)]
mod hardening {
    use super::*;

    /// Every known key given a value of every type must neither crash parsing nor go unreported: it warns.
    #[test]
    fn wrong_types_for_every_key_only_warn() {
        let keys: Vec<&str> = {
            let mut v: Vec<&str> = crate::workbench::settings_ui::CATALOG.iter().map(|e| e.key).collect();
            v.extend([
                "theme",
                "workbench.colorCustomizations",
                "files.associations",
                "lsp.servers",
                "json.schemas",
                "yaml.schemas",
                "spelling.languages",
                "files.exclude",
                "editor.rulers",
                "search.exclude",
                "editor.codeActionsOnSave",
            ]);
            v
        };
        let values = ["null", "true", "-5", "1e308", "\"\"", "\"zzz\"", "[]", "[1,\"a\",null]", "{}", "{\"a\":1}", "{\"[x]\":3,\"accent\":4}", "99999999999"];
        for key in keys {
            for v in values {
                let text = format!("{{ \"{key}\": {v} }}");
                let parsed = parse_layers(&text, &[]);
                assert!(parsed.is_ok(), "{text}");
            }
        }
        assert!(parse_layers("{ \"a\": ", &[]).is_err());
        assert!(parse_layers("[1]", &[]).is_err());
    }

    #[test]
    fn default_settings_match_the_template() {
        // Values in the template (without comments) produce no warnings and agree with the defaults.
        let (s, w) = parse_layers(TEMPLATE, &[]).unwrap();
        assert!(w.is_empty(), "{w:?}");
        let d = Settings::default();
        assert_eq!((s.graph_limit, s.timeline_limit, s.tree_indent, s.cursor_width), (d.graph_limit, d.timeline_limit, d.tree_indent, d.cursor_width));
        assert_eq!((s.startup, s.inline_blame, s.tab_close.as_str()), (d.startup, d.inline_blame, d.tab_close.as_str()));
    }
}
