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
    /// Optional Docker / Dev Container module; user settings only.
    pub docker_enabled: bool,
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
    /// Whether a program in the terminal may put text on the clipboard (OSC 52).
    pub terminal_allow_clipboard_write: bool,
    /// zsh, bash and fish mark prompts, commands and their folder (`terminal.shellIntegration`).
    pub terminal_shell_integration: bool,
    /// Closing a terminal asks first: "running" (a program is in the foreground), "always" or "never".
    pub terminal_confirm_close: String,
    /// Terminal profiles per operating system of the machine (`terminal.profiles.macos` / `.linux` / `.windows`).
    pub terminal_profiles: std::collections::BTreeMap<String, Vec<crate::terminal_profiles::Profile>>,
    /// Profile new terminals use (`terminal.defaultProfile`).
    pub terminal_default_profile: Option<String>,
    /// What pasting into the terminal removes (`terminal.sanitizePaste`).
    pub terminal_sanitize_paste: crate::terminal_paste::Sanitize,
    /// When a paste of several lines asks first (`terminal.warnMultilinePaste`).
    pub terminal_warn_multiline: crate::terminal_paste::WarnMultiline,
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
    /// Local addresses forwarded ports listen on (`ports.localBind`: this computer only, or every interface).
    pub ports_local_bind: host::forward::LocalBind,
    /// Per-port label, protocol and detection behaviour (`ports.rules`).
    pub ports_rules: Vec<crate::port_forwards::PortRule>,
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
            docker_enabled: false,
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
            terminal_allow_clipboard_write: false,
            terminal_shell_integration: true,
            terminal_confirm_close: "running".into(),
            terminal_profiles: Default::default(),
            terminal_default_profile: None,
            terminal_sanitize_paste: Default::default(),
            terminal_warn_multiline: Default::default(),
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
            ports_local_bind: host::forward::LocalBind::Loopback,
            ports_rules: Vec::new(),
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
            graph_show_hash: false,
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

#[path = "settings_templates.rs"]
mod templates;
pub use templates::{KEYBINDINGS_TEMPLATE, TEMPLATE};
#[path = "settings_text.rs"]
mod text;
pub use text::{set_string_in, set_value_in};

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
pub const RESTRICTED_KEYS: &[&str] = &[
    "terminal.shell",
    "terminal.defaultProfile",
    "terminal.profiles.macos",
    "terminal.profiles.linux",
    "terminal.profiles.windows",
    "lsp.servers",
    "json.schemas",
    "yaml.schemas",
];

/// Not read from the project file even in a trusted folder: the gate is global, so one
/// repo touching it would open it for every folder.
pub const USER_ONLY_KEYS: &[&str] = &[
    "modules.containers.enabled",
    "security.workspaceTrust.enabled",
    "http.confirmRequests",
    "ui.language",
    "update.check",
    // --- git-sync ---
    // A repository must not turn the question before a sync off, or make every commit push by itself.
    "git.confirmSync",
    "git.postCommitCommand",
    // A repository must not make forwarded ports reachable from the network.
    "ports.localBind",
    // A repository must not let programs fill the clipboard, or turn off the checks on what is pasted back: together
    // they would put a command on the clipboard that a later paste runs.
    "terminal.allowClipboardWrite",
    "terminal.sanitizePaste",
    "terminal.warnMultilinePaste",
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
    if let Some(v) = r.number("terminal.scrollback", 0., crate::terminal::SCROLLBACK_MAX as f64) {
        s.terminal_scrollback = v as usize;
    }
    if let Some(v) = r.bool("terminal.allowClipboardWrite") {
        s.terminal_allow_clipboard_write = v;
    }
    if let Some(v) = r.bool("terminal.shellIntegration") {
        s.terminal_shell_integration = v;
    }
    if let Some(v) = r.choice("terminal.confirmOnClose", &["running", "always", "never"]) {
        s.terminal_confirm_close = v;
    }
    s.terminal_default_profile = r.string("terminal.defaultProfile").filter(|p| !p.trim().is_empty());
    for os in ["macos", "linux", "windows"] {
        let key = format!("terminal.profiles.{os}");
        match obj.get(&key) {
            None | Some(Value::Null) => {}
            Some(Value::Object(m)) => {
                let mut w = Vec::new();
                s.terminal_profiles.insert(os.to_string(), crate::terminal_profiles::parse_profiles(&key, m, &mut w));
                for w in w {
                    r.warn(w);
                }
            }
            Some(_) => r.warn(crate::i18n::tf("{0}: expected an object {1}", &[&key, &"{ \"zsh\": { \"path\": \"/bin/zsh\" } }"])),
        }
    }
    if let Some(v) = r.choice("terminal.sanitizePaste", &["auto", "always", "off"]) {
        s.terminal_sanitize_paste = crate::terminal_paste::Sanitize::parse(&v).unwrap_or_default();
    }
    if let Some(v) = r.choice("terminal.warnMultilinePaste", &["unbracketed", "always", "never"]) {
        s.terminal_warn_multiline = crate::terminal_paste::WarnMultiline::parse(&v).unwrap_or_default();
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
    if let Some(v) = r.choice("ports.localBind", &["loopback", "all"]) {
        s.ports_local_bind = if v == "all" { host::forward::LocalBind::All } else { host::forward::LocalBind::Loopback };
    }
    match obj.get("ports.rules") {
        None | Some(Value::Null) => {}
        Some(Value::Object(m)) => {
            let mut w = Vec::new();
            s.ports_rules = crate::port_forwards::parse_rules(m, &mut w);
            for w in w {
                r.warn(w);
            }
        }
        Some(_) => r.warn(crate::i18n::tf("{0}: expected an object {1}", &[&"ports.rules", &"{ \"3000\": { \"label\": \"web\" } }"])),
    }
    if let Some(v) = r.bool("modules.containers.enabled") {
        s.docker_enabled = v;
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
        "terminal.sanitizePaste",
        "terminal.warnMultilinePaste",
        "terminal.confirmOnClose",
        "terminal.shellIntegration",
        "terminal.defaultProfile",
        "terminal.profiles.macos",
        "terminal.profiles.linux",
        "terminal.profiles.windows",
        "terminal.allowClipboardWrite",
        "window.restoreSession",
        "window.startup",
        "lsp.enabled",
        "modules.containers.enabled",
        "lsp.servers",
        "ports.autoForward",
        "ports.localBind",
        "ports.rules",
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

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
