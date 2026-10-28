//! "Preferences: Settings" and "Preferences: Keyboard Shortcuts" pickers: find a setting
//! in the list and change it; the value is written to `settings.jsonc` with comments preserved and
//! applied immediately. Choosing a command in the shortcuts list adds a line to `keybindings.jsonc`.

use gpui::{AppContext as _, Context, Window};
use serde_json::Value;

use super::{InputPurpose, Notice, OverlayKind, Workbench};
use crate::commands;
use crate::picker::{PickItem, Picker, Source as PickSource};
use crate::settings::{self, AutoSave, Settings};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Bool,
    Choice(&'static [&'static str]),
    Number { min: f64, max: f64 },
    Text,
}

pub struct Entry {
    pub key: &'static str,
    pub about: &'static str,
    pub kind: Kind,
}

const fn e(key: &'static str, about: &'static str, kind: Kind) -> Entry {
    Entry { key, about, kind }
}

/// Settings editable from the UI (theme has its own picker; arrays are in the file).
pub const CATALOG: &[Entry] = &[
    e("editor.fontSize", crate::i18n::tr("Font size of the editor"), Kind::Number { min: 6., max: 48. }),
    e("editor.lineHeight", crate::i18n::tr("Line height in pixels"), Kind::Number { min: 10., max: 80. }),
    e("editor.fontFamily", crate::i18n::tr("Editor font (empty: built-in monospace)"), Kind::Text),
    e("editor.tabSize", crate::i18n::tr("Spaces per indent level (empty: detect per file)"), Kind::Number { min: 1., max: 16. }),
    e("editor.insertSpaces", crate::i18n::tr("Indent with spaces instead of tabs"), Kind::Bool),
    e("editor.wordWrap", crate::i18n::tr("Wrap long lines at the window edge"), Kind::Choice(&["off", "on"])),
    e("editor.cursorBlinking", crate::i18n::tr("Cursor animation"), Kind::Choice(&["blink", "solid"])),
    e("editor.minimap.enabled", crate::i18n::tr("Show the minimap"), Kind::Bool),
    e("editor.wordHighlight", crate::i18n::tr("Highlight other uses of the word under the cursor"), Kind::Bool),
    e("editor.indentGuides", crate::i18n::tr("Show indent guides"), Kind::Bool),
    e("editor.formatOnSave", crate::i18n::tr("Format with the language server when saving"), Kind::Bool),
    e("editor.autoClosingBrackets", crate::i18n::tr("Insert the closing bracket or quote"), Kind::Bool),
    e("editor.wordBasedSuggestions", crate::i18n::tr("Suggest words from the file when there is no language server"), Kind::Bool),
    e("files.autoSave", crate::i18n::tr("Save files automatically"), Kind::Choice(&["off", "afterDelay", "onFocusChange"])),
    e("files.autoSaveDelay", crate::i18n::tr("Delay for afterDelay (ms)"), Kind::Number { min: 100., max: 600_000. }),
    e("files.trimTrailingWhitespace", crate::i18n::tr("Remove trailing spaces when saving"), Kind::Bool),
    e("files.insertFinalNewline", crate::i18n::tr("End files with a newline when saving"), Kind::Bool),
    e("files.hotExit", crate::i18n::tr("Keep unsaved changes across restarts"), Kind::Bool),
    e("files.enableTrash", crate::i18n::tr("Move deleted local files to the Trash"), Kind::Bool),
    e("files.fallbackEncoding", crate::i18n::tr("Encoding for files that are not UTF-8"), Kind::Choice(&["windows1254", "iso88599", "iso88591", "utf8"])),
    e("explorer.autoReveal", crate::i18n::tr("Select the active file in the explorer"), Kind::Bool),
    e("explorer.compactFolders", crate::i18n::tr("Show folders that only hold one folder as one row"), Kind::Bool),
    e("explorer.sortOrder", crate::i18n::tr("Order of files and folders in the explorer"), Kind::Choice(&["default", "filesFirst", "mixed", "type"])),
    e("explorer.openEditors", crate::i18n::tr("Show the Open Editors list in the explorer"), Kind::Bool),
    e("editor.renderWhitespace", crate::i18n::tr("Show spaces and tabs as marks"), Kind::Choice(&["none", "boundary", "all"])),
    e("editor.lineNumbers", crate::i18n::tr("Line numbers in the gutter"), Kind::Choice(&["on", "off", "relative"])),
    e("editor.cursorStyle", crate::i18n::tr("Shape of the cursor"), Kind::Choice(&["line", "block", "underline"])),
    e("editor.bracketPairColorization", crate::i18n::tr("Color matching brackets by nesting depth"), Kind::Bool),
    e("editor.stickyScroll.enabled", crate::i18n::tr("Pin the enclosing function or class line while scrolling"), Kind::Bool),
    e("editor.formatOnPaste", crate::i18n::tr("Format pasted text with the language server"), Kind::Bool),
    e("editor.formatOnType", crate::i18n::tr("Let the language server format while typing }, ; or Enter"), Kind::Bool),
    e(
        "editor.unicodeHighlight.enabled",
        crate::i18n::tr("Box invisible and look-alike Unicode characters (zero-width spaces, bidi controls, Cyrillic letters inside Latin words)"),
        Kind::Bool,
    ),
    e("editor.colorDecorators", crate::i18n::tr("Show a color swatch next to #rrggbb values"), Kind::Bool),
    e(
        "workbench.iconTheme",
        crate::i18n::tr("File and folder icons"),
        Kind::Choice(&["glyphs", "minimal", "codeyard", "badges", "colorful", "apple", "material", "none"]),
    ),
    e("workbench.fontFamily", crate::i18n::tr("UI font of the whole window (empty: system font)"), Kind::Text),
    e("workbench.editor.showTabs", crate::i18n::tr("Show the editor tab bar"), Kind::Bool),
    e("workbench.tabs.showIcons", crate::i18n::tr("Show file icons on tabs"), Kind::Bool),
    e("workbench.tabs.closeButton", crate::i18n::tr("When the close button of a tab shows"), Kind::Choice(&["hover", "always", "off"])),
    e(
        "workbench.reduceMotion",
        crate::i18n::tr("Leave out animations: the blinking cursor, spinners and sliding panels (auto: follow the system setting)"),
        Kind::Choice(&["auto", "on", "off"]),
    ),
    e("workbench.breadcrumbs.enabled", crate::i18n::tr("Show the file path above the editor"), Kind::Bool),
    e("editor.cursorWidth", crate::i18n::tr("Width of the line cursor (px)"), Kind::Number { min: 1., max: 8. }),
    e("editor.hover.enabled", crate::i18n::tr("Show a tooltip when the mouse rests on a word"), Kind::Bool),
    e("editor.quickSuggestions", crate::i18n::tr("Suggestions pop up while typing (off: only Ctrl+Space)"), Kind::Bool),
    e("workbench.statusBar.visible", crate::i18n::tr("Show the status bar"), Kind::Bool),
    e("workbench.activityBar.visible", crate::i18n::tr("Show the activity bar"), Kind::Bool),
    e("workbench.tree.indent", crate::i18n::tr("Indent of each folder level in the explorer (px)"), Kind::Number { min: 8., max: 32. }),
    e("explorer.confirmDelete", crate::i18n::tr("Ask before moving a file to the Trash"), Kind::Bool),
    e("search.smartCase", crate::i18n::tr("Ignore case unless the query has capital letters"), Kind::Bool),
    e("search.showLineNumbers", crate::i18n::tr("Show line numbers in search results"), Kind::Bool),
    e("search.collapseResults", crate::i18n::tr("Start search results folded or open"), Kind::Choice(&["auto", "alwaysCollapse", "alwaysExpand"])),
    e("git.graph.limit", crate::i18n::tr("Commits drawn in the Source Control graph"), Kind::Number { min: 20., max: 5000. }),
    e("git.timeline.limit", crate::i18n::tr("Commits listed in the explorer's Timeline"), Kind::Number { min: 5., max: 1000. }),
    e("git.inlineBlame", crate::i18n::tr("Show who last changed the cursor line (and when) at the end of the line"), Kind::Bool),
    e("git.graph.allBranches", crate::i18n::tr("Graph starts with every branch"), Kind::Bool),
    e("editor.renderLineHighlight", crate::i18n::tr("Highlight the line with the cursor"), Kind::Choice(&["line", "none"])),
    e("editor.matchBrackets", crate::i18n::tr("Highlight the matching bracket"), Kind::Bool),
    e("editor.folding", crate::i18n::tr("Show fold arrows in the gutter"), Kind::Bool),
    e("search.maxResults", crate::i18n::tr("Stop a search after this many matches"), Kind::Number { min: 1., max: 100_000. }),
    // --- git-sync ---
    e("git.autofetch", crate::i18n::tr("Fetch from the remote in the background while the window is in front"), Kind::Bool),
    e("git.autofetchPeriod", crate::i18n::tr("Seconds between background fetches"), Kind::Number { min: 30., max: 86_400. }),
    e("git.confirmSync", crate::i18n::tr("Ask before Sync pulls or pushes. Your own settings only; a folder cannot change it"), Kind::Bool),
    e("git.rebaseWhenSync", crate::i18n::tr("Pull with rebase instead of a merge commit"), Kind::Bool),
    e("git.pruneOnFetch", crate::i18n::tr("Fetch removes remote branches that were deleted on the server"), Kind::Bool),
    e("git.pullTags", crate::i18n::tr("Pull also fetches all tags"), Kind::Bool),
    e("git.fetchOnPull", crate::i18n::tr("Fetch every remote before pulling"), Kind::Bool),
    e(
        "git.postCommitCommand",
        crate::i18n::tr("What runs after a commit: nothing, push or sync. Your own settings only; a folder cannot change it"),
        Kind::Choice(&["none", "push", "sync"]),
    ),
    e("git.enableSmartCommit", crate::i18n::tr("Commit all tracked changes when nothing is staged"), Kind::Bool),
    // --- structural ---
    e("search.structural.maxFiles", crate::i18n::tr("Most files a structural search reads and parses"), Kind::Number { min: 1., max: 100_000. }),
    e("terminal.allowClipboardWrite", crate::i18n::tr("Let programs in the terminal write to the clipboard"), Kind::Bool),
    e("terminal.fontSize", crate::i18n::tr("Font size of the terminal"), Kind::Number { min: 6., max: 48. }),
    e("terminal.lineHeight", crate::i18n::tr("Terminal row height, as a multiple of the font size"), Kind::Number { min: 1., max: 2. }),
    e("terminal.scrollback", crate::i18n::tr("Lines kept in terminal history"), Kind::Number { min: 0., max: 50_000. }),
    e("terminal.scrollSensitivity", crate::i18n::tr("Mouse wheel and trackpad scroll speed multiplier"), Kind::Number { min: 0.1, max: 10. }),
    e("terminal.shellIntegration", crate::i18n::tr("Mark prompts, commands and the folder in zsh, bash and fish"), Kind::Bool),
    e("terminal.confirmOnClose", crate::i18n::tr("Ask before closing a terminal"), Kind::Choice(&["running", "always", "never"])),
    e("terminal.defaultProfile", crate::i18n::tr("Profile new terminals use"), Kind::Text),
    e("terminal.sanitizePaste", crate::i18n::tr("Remove control characters from pasted text"), Kind::Choice(&["auto", "always", "off"])),
    e("terminal.warnMultilinePaste", crate::i18n::tr("Ask before pasting several lines into the terminal"), Kind::Choice(&["unbracketed", "always", "never"])),
    e("terminal.shell", crate::i18n::tr("Shell for new terminals (empty: login shell)"), Kind::Text),
    e(
        "window.startup",
        crate::i18n::tr("What opens at start: welcome (the start page), continue (where you left off, SSH hosts included) or blank (an empty window)"),
        Kind::Choice(&["welcome", "continue", "blank"]),
    ),
    e("window.restoreSession", crate::i18n::tr("Bring a folder's tabs and unsaved changes back when you open it"), Kind::Bool),
    e("ports.autoForward", crate::i18n::tr("Forward ports opened on SSH hosts"), Kind::Bool),
    e("ports.localBind", crate::i18n::tr("Where forwarded ports listen: this computer only, or every network interface"), Kind::Choice(&["loopback", "all"])),
    e("lsp.enabled", crate::i18n::tr("Start language servers"), Kind::Bool),
    e(
        "http.runOn",
        crate::i18n::tr("Where .http requests are sent from: the workspace host (the server when connected over SSH) or this computer"),
        Kind::Choice(&["workspace", "local"]),
    ),
    e(
        "ui.language",
        crate::i18n::tr(
            "Language of the menus, command palette, Welcome screen, status bar, Source Control, Explorer, Settings page chrome, file operations and Git prompts (the user guide lists exactly what is translated; the rest stays English): auto (the system language) or a language code, en or tr. Your own settings only; a folder cannot change it",
        ),
        Kind::Choice(crate::i18n::SETTING_CHOICES),
    ),
    e(
        "update.check",
        crate::i18n::tr(
            "Look for new Codeyard versions: ask (not decided yet; nothing is checked), daily (once a day, downloads two small files from github.com, the checksum list of the latest release and its signature; GitHub sees your IP address and that the request comes from curl, nothing about your files) or manual (only when you choose Check for Updates). Your own settings only; a folder cannot change it",
        ),
        Kind::Choice(&["ask", "daily", "manual"]),
    ),
    e(
        "http.confirmRequests",
        crate::i18n::tr(
            "Ask before .http requests and schema downloads leave this computer: never, remote (the first request to each new host per session) or always. Your own settings only; a folder cannot change it",
        ),
        Kind::Choice(&["never", "remote", "always"]),
    ),
    e("spelling.enabled", crate::i18n::tr("Underline misspelled words in prose files and in code comments"), Kind::Bool),
    e("spelling.checkComments", crate::i18n::tr("Check comments in code files too (prose files are always checked)"), Kind::Bool),
    // --- data ---
    e("data.indent", crate::i18n::tr("Spaces per level in the JSON and YAML written by Data: Convert To… and Data: Query…"), Kind::Number { min: 1., max: 8. }),
    e(
        "data.sortKeys",
        crate::i18n::tr("Sort object keys alphabetically in the output of Data: Convert To… and Data: Query… (off: keep the file's order)"),
        Kind::Bool,
    ),
    e("json.validate.enable", crate::i18n::tr("Check JSON and YAML files against their schema (problems, hover, suggestions)"), Kind::Bool),
    e("json.schemaDownload.enable", crate::i18n::tr("Download schemas from SchemaStore and cache them (off: only local and cached schemas)"), Kind::Bool),
    e("explorer.confirmDragAndDrop", crate::i18n::tr("Ask before moving a file or folder by dragging it in the explorer"), Kind::Bool),
    e("workbench.tree.guides", crate::i18n::tr("Draw the folder hierarchy lines in the explorer"), Kind::Bool),
    e("git.graph.showAuthor", crate::i18n::tr("Show the author name on Source Control graph rows"), Kind::Bool),
    e(
        "git.graph.view",
        crate::i18n::tr("Source Control graph view: graph (branch lanes), compact (one line per commit) or dates (grouped by day)"),
        Kind::Choice(&["graph", "compact", "dates"]),
    ),
    e("git.graph.showHash", crate::i18n::tr("Show the short commit hash on Source Control graph rows"), Kind::Bool),
    e("editor.editorconfig", crate::i18n::tr("Apply .editorconfig files (indent, line endings, final newline…)"), Kind::Bool),
    e("editor.inlayHints.enabled", crate::i18n::tr("Show type and parameter name hints from the language server"), Kind::Bool),
    e("editor.linkedEditing", crate::i18n::tr("Rename the matching HTML/XML tag while you edit one"), Kind::Bool),
    e("editor.codeLens", crate::i18n::tr("Show code lenses (reference counts, run/debug) above symbols"), Kind::Bool),
    e("editor.semanticHighlighting.enabled", crate::i18n::tr("Color symbols by their meaning, from the language server"), Kind::Bool),
    e(
        "editor.caseLocale",
        crate::i18n::tr("Language of Uppercase / Lowercase / Title Case: auto (Turkish rules when the text has ı İ ş ğ), tr (i ↔ İ, ı ↔ I) or en"),
        Kind::Choice(&["auto", "tr", "en"]),
    ),
    e("editor.fontLigatures", crate::i18n::tr("Draw programming ligatures (=>, !=, …) in the editor font"), Kind::Bool),
    e("emmet.triggerExpansionOnTab", crate::i18n::tr("Expand Emmet abbreviations (div.box>ul>li*3) with Tab"), Kind::Bool),
    e("diffEditor.renderSideBySide", crate::i18n::tr("Diff view side by side (off: inline)"), Kind::Bool),
    e("security.workspaceTrust.enabled", crate::i18n::tr("Ask before running tasks, tests or the debugger of an untrusted folder"), Kind::Bool),
    e("modules.containers.enabled", crate::i18n::tr("Enable container environments (user settings only; disconnect containers before disabling)"), Kind::Bool),
];

pub fn entry(key: &str) -> Option<&'static Entry> {
    CATALOG.iter().find(|e| e.key == key)
}

/// The current (effective) value; with the user file and project layers applied.
pub fn current(s: &Settings, key: &str) -> Value {
    use serde_json::json;
    match key {
        "editor.fontSize" => json!(s.editor_font_size),
        "editor.lineHeight" => json!(s.editor_line_height),
        "editor.fontFamily" => json!(s.editor_font_family.clone().unwrap_or_default()),
        "editor.tabSize" => s.tab_size.map_or(Value::Null, |n| json!(n)),
        "editor.insertSpaces" => json!(s.insert_spaces.unwrap_or(true)),
        "editor.wordWrap" => json!(if s.word_wrap { "on" } else { "off" }),
        "editor.cursorBlinking" => json!(if s.cursor_blinking { "blink" } else { "solid" }),
        "editor.minimap.enabled" => json!(s.minimap),
        "editor.wordHighlight" => json!(s.word_highlight),
        "editor.indentGuides" => json!(s.indent_guides),
        "editor.formatOnSave" => json!(s.format_on_save),
        "editor.formatOnPaste" => json!(s.format_on_paste),
        "editor.formatOnType" => json!(s.format_on_type),
        "editor.editorconfig" => json!(s.editorconfig),
        "editor.inlayHints.enabled" => json!(s.inlay_hints),
        "editor.linkedEditing" => json!(s.linked_editing),
        "security.workspaceTrust.enabled" => json!(s.workspace_trust),
        "modules.containers.enabled" => json!(s.docker_enabled),
        "editor.codeLens" => json!(s.code_lens),
        "editor.unicodeHighlight.enabled" => json!(s.unicode_highlight),
        "editor.semanticHighlighting.enabled" => json!(s.semantic_highlighting),
        "emmet.triggerExpansionOnTab" => json!(s.emmet),
        "editor.caseLocale" => json!(s.case_locale),
        "editor.fontLigatures" => json!(s.font_ligatures),
        "diffEditor.renderSideBySide" => json!(!s.diff_inline),
        "editor.autoClosingBrackets" => json!(s.auto_close_brackets),
        "editor.wordBasedSuggestions" => json!(s.word_based_suggestions),
        "files.autoSave" => json!(match s.auto_save {
            AutoSave::Off => "off",
            AutoSave::AfterDelay(_) => "afterDelay",
            AutoSave::OnFocusChange => "onFocusChange",
        }),
        "files.autoSaveDelay" => json!(match s.auto_save {
            AutoSave::AfterDelay(ms) => ms,
            _ => 1000,
        }),
        "files.trimTrailingWhitespace" => json!(s.trim_trailing_whitespace),
        "files.insertFinalNewline" => json!(s.insert_final_newline),
        "files.hotExit" => json!(s.hot_exit),
        "files.enableTrash" => json!(s.enable_trash),
        "files.fallbackEncoding" => json!(s.fallback_encoding.setting_name()),
        "explorer.autoReveal" => json!(s.auto_reveal),
        "explorer.compactFolders" => json!(s.compact_folders),
        "explorer.sortOrder" => json!(s.sort_order),
        "explorer.openEditors" => json!(s.open_editors),
        "editor.renderWhitespace" => json!(s.render_whitespace),
        "editor.lineNumbers" => json!(s.line_numbers),
        "editor.cursorStyle" => json!(s.cursor_style),
        "editor.bracketPairColorization" => json!(s.bracket_colors),
        "editor.stickyScroll.enabled" => json!(s.sticky_scroll),
        "editor.colorDecorators" => json!(s.color_decorators),
        "workbench.iconTheme" => json!(s.icon_theme),
        "workbench.fontFamily" => json!(s.ui_font.clone().unwrap_or_default()),
        "workbench.editor.showTabs" => json!(s.show_tabs),
        "workbench.tabs.showIcons" => json!(s.tab_icons),
        "workbench.tabs.closeButton" => json!(s.tab_close),
        "workbench.reduceMotion" => json!(match s.reduce_motion {
            Some(true) => "on",
            Some(false) => "off",
            None => "auto",
        }),
        "workbench.breadcrumbs.enabled" => json!(s.breadcrumbs),
        "editor.cursorWidth" => json!(s.cursor_width),
        "editor.hover.enabled" => json!(s.hover_enabled),
        "editor.quickSuggestions" => json!(s.quick_suggestions),
        "workbench.statusBar.visible" => json!(s.status_bar),
        "workbench.activityBar.visible" => json!(s.activity_bar),
        "workbench.tree.indent" => json!(s.tree_indent),
        "explorer.confirmDelete" => json!(s.confirm_delete),
        "explorer.confirmDragAndDrop" => json!(s.confirm_drag_and_drop),
        "http.runOn" => json!(s.http_run_on),
        "http.confirmRequests" => json!(s.http_confirm),
        "ui.language" => json!(s.ui_language),
        "update.check" => json!(s.update_check.as_str()),
        "spelling.enabled" => json!(s.spelling),
        "spelling.checkComments" => json!(s.spelling_comments),
        // --- data ---
        "data.indent" => json!(s.data_indent),
        "data.sortKeys" => json!(s.data_sort_keys),
        "json.validate.enable" => json!(s.schema_validate),
        "json.schemaDownload.enable" => json!(s.schema_download),
        "workbench.tree.guides" => json!(s.tree_guides),
        "git.graph.showAuthor" => json!(s.graph_show_author),
        "git.graph.showHash" => json!(s.graph_show_hash),
        "git.graph.view" => json!(s.graph_view),
        "search.smartCase" => json!(s.search_smart_case),
        "search.showLineNumbers" => json!(s.search_line_numbers),
        "search.collapseResults" => json!(s.search_collapse),
        "git.graph.limit" => json!(s.graph_limit),
        "git.timeline.limit" => json!(s.timeline_limit),
        "git.inlineBlame" => json!(s.inline_blame),
        "git.graph.allBranches" => json!(s.graph_all_branches),
        "editor.renderLineHighlight" => json!(if s.render_line_highlight { "line" } else { "none" }),
        "editor.matchBrackets" => json!(s.match_brackets),
        "editor.folding" => json!(s.folding),
        "search.maxResults" => json!(s.search_max_results),
        // --- git-sync ---
        "git.autofetch" => json!(s.git_autofetch),
        "git.autofetchPeriod" => json!(s.git_autofetch_period),
        "git.confirmSync" => json!(s.git_confirm_sync),
        "git.rebaseWhenSync" => json!(s.git_rebase_when_sync),
        "git.pruneOnFetch" => json!(s.git_prune_on_fetch),
        "git.pullTags" => json!(s.git_pull_tags),
        "git.fetchOnPull" => json!(s.git_fetch_on_pull),
        "git.postCommitCommand" => json!(s.git_post_commit),
        "git.enableSmartCommit" => json!(s.git_smart_commit),
        // --- structural ---
        "search.structural.maxFiles" => json!(s.search_structural_max_files),
        "terminal.allowClipboardWrite" => json!(s.terminal_allow_clipboard_write),
        "terminal.fontSize" => json!(s.terminal_font_size),
        // Through f64 an f32 1.2 would read 1.2000000476837158: two decimals are all the setting means.
        "terminal.lineHeight" => json!((f64::from(s.terminal_line_height) * 100.).round() / 100.),
        "terminal.scrollback" => json!(s.terminal_scrollback),
        "terminal.scrollSensitivity" => json!(s.terminal_scroll_sensitivity),
        "terminal.sanitizePaste" => json!(s.terminal_sanitize_paste.as_str()),
        "terminal.confirmOnClose" => json!(s.terminal_confirm_close),
        "terminal.shellIntegration" => json!(s.terminal_shell_integration),
        "terminal.defaultProfile" => json!(s.terminal_default_profile.clone().unwrap_or_default()),
        "terminal.warnMultilinePaste" => json!(s.terminal_warn_multiline.as_str()),
        "terminal.shell" => json!(s.terminal_shell.clone().unwrap_or_default()),
        "window.restoreSession" => json!(s.restore_session),
        "window.startup" => json!(s.startup.id()),
        "ports.autoForward" => json!(s.ports_auto_forward),
        "ports.localBind" => json!(if s.ports_local_bind == host::forward::LocalBind::All { "all" } else { "loopback" }),
        "lsp.enabled" => json!(s.lsp_enabled),
        _ => Value::Null,
    }
}

fn show(v: &Value) -> String {
    match v {
        Value::Null => crate::i18n::t("auto").into(),
        Value::String(s) if s.is_empty() => crate::i18n::t("default").into(),
        Value::String(s) => s.clone(),
        Value::Number(n) => n.as_f64().map_or(n.to_string(), |f| if f.fract() == 0. { format!("{}", f as i64) } else { format!("{f}") }),
        other => other.to_string(),
    }
}

/// Convert the entered text to the setting's type. Empty text: a value meaning "default"
/// instead of removing the key (empty string is ignored for strings, `null` for numbers).
pub fn parse_input(kind: Kind, text: &str) -> Result<Value, String> {
    let text = text.trim();
    match kind {
        Kind::Number { min, max } => {
            if text.is_empty() {
                return Ok(Value::Null);
            }
            let n: f64 = text.parse().map_err(|_| format!("“{text}” is not a number"))?;
            if !(min..=max).contains(&n) {
                return Err(format!("Must be between {min} and {max}"));
            }
            Ok(if n.fract() == 0. { Value::from(n as i64) } else { Value::from(n) })
        }
        Kind::Text => Ok(if text.is_empty() { Value::Null } else { Value::String(text.to_string()) }),
        Kind::Bool => match text {
            "true" | "on" => Ok(Value::Bool(true)),
            "false" | "off" => Ok(Value::Bool(false)),
            _ => Err("true or false".into()),
        },
        Kind::Choice(opts) => opts.iter().find(|o| **o == text).map(|o| Value::String(o.to_string())).ok_or_else(|| format!("One of: {}", opts.join(", "))),
    }
}

impl Workbench {
    /// "Preferences: Settings": the settings list (key, value, description).
    pub(super) fn show_settings_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.config_paths().is_none() {
            self.notice = Some(Notice::Info(crate::i18n::t("No settings folder on this machine.").to_string().into()));
            cx.notify();
            return;
        }
        if self.overlay.is_some() {
            self.close_overlay(window, cx);
        }
        let s = cx.global::<Settings>().clone();
        let mut items = vec![PickItem {
            label: "theme".into(),
            detail: Some(crate::i18n::tf("{} · Color theme (opens the theme picker)", &[&s.theme]).into()),
            keys: None,
        }];
        items.extend(CATALOG.iter().map(|e| {
            let v = current(&s, e.key);
            PickItem { label: e.key.into(), detail: Some(format!("{} · {}", show(&v), crate::i18n::t(e.about)).into()), keys: None }
        }));
        items.push(PickItem {
            label: crate::i18n::t("Open settings.jsonc").into(),
            detail: Some(crate::i18n::t("Every setting, with comments").into()),
            keys: None,
        });
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], crate::i18n::t("Search settings"), "", None, cx));
        self.show_overlay(picker, OverlayKind::Settings, window, cx);
    }

    /// The row chosen from the list (`index` 0: theme, then the catalog, the file last).
    pub(super) fn pick_setting(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index == 0 {
            return self.select_theme(&super::SelectTheme, window, cx);
        }
        let Some(entry) = CATALOG.get(index - 1) else {
            return self.open_settings(&super::OpenSettings, window, cx);
        };
        let now = current(cx.global::<Settings>(), entry.key);
        match entry.kind {
            Kind::Bool => self.write_setting(entry.key, Value::Bool(!now.as_bool().unwrap_or(false)), window, cx),
            Kind::Choice(opts) => {
                let items = opts
                    .iter()
                    .map(|o| PickItem { label: (*o).into(), detail: (now.as_str() == Some(*o)).then(|| crate::i18n::t("current").into()), keys: None })
                    .collect();
                let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], entry.key, "", None, cx));
                self.show_overlay(picker, OverlayKind::SettingChoice { key: entry.key, options: opts }, window, cx);
            }
            Kind::Number { .. } | Kind::Text => {
                let initial = match &now {
                    Value::Null => String::new(),
                    Value::String(s) => s.clone(),
                    v => show(v),
                };
                self.show_input(InputPurpose::Setting { key: entry.key }, crate::i18n::t("Set"), crate::i18n::t(entry.about), &initial, window, cx);
            }
        }
    }

    /// Write to the settings file (comments preserved) and apply immediately. `null`: back to default.
    pub(super) fn write_setting(&mut self, key: &str, value: Value, window: &mut Window, cx: &mut Context<Self>) {
        if key == "modules.containers.enabled" && value != Value::Bool(true) && self.any_container_connected(cx) {
            self.notice =
                Some(Notice::Info(crate::i18n::t("Disconnect the container before disabling the container module. Your open files are kept.").into()));
            cx.notify();
            return;
        }
        let Some((spath, _)) = self.config_paths() else { return };
        let result = settings::ensure_file(&spath, settings::TEMPLATE)
            .and_then(|_| std::fs::read_to_string(&spath))
            .and_then(|text| crate::fsutil::write_atomic(&spath, settings::set_value_in(&text, key, &value)));
        match result {
            Ok(()) => {
                // A stale message from the previous setting must not hide the new one; a reload warning wins.
                self.notice = None;
                self.reload_config(window, cx);
                if self.notice.is_none() {
                    let shown = if value.is_null() { crate::i18n::t("default").into() } else { show(&value) };
                    self.notice = Some(Notice::Info(format!("{key}: {shown}").into()));
                }
            }
            Err(e) => self.notice = Some(Notice::Error(crate::i18n::tf("Could not write {0}: {1}", &[&spath.display(), &e]).into())),
        }
        cx.notify();
    }

    /// The value entered after the picker closed.
    pub(super) fn finish_setting_input(&mut self, key: &'static str, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = entry(key) else { return };
        match parse_input(entry.kind, &text) {
            Ok(v) => self.write_setting(key, v, window, cx),
            Err(e) => {
                self.notice = Some(Notice::Error(format!("{key}: {e}").into()));
                cx.notify();
            }
        }
    }

    /// "Preferences: Keyboard Shortcuts": command ids and their keys; choosing one
    /// adds an empty line for that command to `keybindings.jsonc` and opens the file.
    pub(super) fn show_shortcuts_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.config_paths().is_none() {
            return;
        }
        if self.overlay.is_some() {
            self.close_overlay(window, cx);
        }
        let items = commands::COMMAND_IDS
            .iter()
            .map(|id| {
                let keys = commands::action_by_id(id).and_then(|a| commands::Command { label: String::new(), run: commands::Run::Action(a) }.shortcut(cx));
                PickItem { label: (*id).into(), detail: None, keys: keys.map(Into::into) }
            })
            .collect();
        let picker = cx.new(|cx| Picker::new(vec![PickSource::List { prefix: "", items }], crate::i18n::t("Search commands to bind a key"), "", None, cx));
        self.show_overlay(picker, OverlayKind::Shortcuts, window, cx);
    }

    pub(super) fn bind_shortcut(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(id), Some((_, kpath))) = (commands::COMMAND_IDS.get(index), self.config_paths()) else { return };
        let result = settings::ensure_file(&kpath, settings::KEYBINDINGS_TEMPLATE).and_then(|_| std::fs::read_to_string(&kpath)).and_then(|text| {
            let (text, line) = add_binding_line(&text, id);
            crate::fsutil::write_atomic(&kpath, text).map(|_| line)
        });
        match result {
            Ok(line) => {
                let path = kpath.to_string_lossy().into_owned();
                let host = self.remote.local_host.clone();
                self.open_file_on(host, path.clone(), window, cx);
                // Cursor goes inside the quotes of the "key" value.
                self.open_location(path, line as u32, 12, window, cx);
                self.notice = Some(Notice::Info(crate::i18n::tf("Type the shortcut for {} (e.g. cmd-shift-d), then save", &[&id]).into()));
            }
            Err(e) => self.notice = Some(Notice::Error(crate::i18n::tf("Could not write {0}: {1}", &[&kpath.display(), &e]).into())),
        }
        cx.notify();
    }
}

/// Whether the byte is real JSON (`false` inside comments and strings), so a `]` or `}`
/// in a comment isn't taken as the insertion point.
fn code_mask(text: &str) -> Vec<bool> {
    let b = text.as_bytes();
    let mut mask = vec![true; b.len()];
    let mut i = 0;
    while i < b.len() {
        match (b[i], b.get(i + 1)) {
            (b'/', Some(b'/')) => {
                let end = text[i..].find('\n').map_or(b.len(), |n| i + n);
                mask[i..end].iter_mut().for_each(|m| *m = false);
                i = end;
            }
            (b'/', Some(b'*')) => {
                let end = text[i + 2..].find("*/").map_or(b.len(), |n| i + 2 + n + 2);
                mask[i..end].iter_mut().for_each(|m| *m = false);
                i = end;
            }
            (b'"', _) => {
                let mut j = i + 1;
                while j < b.len() && b[j] != b'"' {
                    j += if b[j] == b'\\' { 2 } else { 1 };
                }
                let end = (j + 1).min(b.len());
                mask[i..end].iter_mut().for_each(|m| *m = false);
                i = end;
            }
            _ => i += 1,
        }
    }
    mask
}

/// Before the array's closing, add `{ "key": "", "command": "<id>" },`; (new text, line).
/// If the previous item has no trailing comma, one is added (comments and other lines preserved).
pub fn add_binding_line(text: &str, id: &str) -> (String, usize) {
    let entry = format!("  {{ \"key\": \"\", \"command\": \"{id}\" }},");
    let mask = code_mask(text);
    let last_code = |ch: u8, before: usize| (0..before).rev().find(|&i| mask[i] && text.as_bytes()[i] == ch);
    let Some(close) = last_code(b']', text.len()) else { return (format!("[\n{entry}\n]\n"), 1) };
    let line_start = text[..close].rfind('\n').map_or(0, |n| n + 1);
    // If `]` is on its own line, a new line before it; otherwise in between.
    let pos = if text[line_start..close].trim().is_empty() { line_start } else { close };
    let build = |head: &str| {
        let sep = if head.ends_with('\n') { "" } else { "\n" };
        format!("{head}{sep}{entry}\n{}", &text[pos..])
    };
    let parses = |t: &str| serde_json::from_str::<Value>(&theme::themes::strip_jsonc(t)).is_ok();
    let mut out = build(&text[..pos]);
    if !parses(&out)
        && let Some(last) = last_code(b'}', pos)
    {
        out = build(&format!("{},{}", &text[..=last], &text[last + 1..pos]));
    }
    let line = out[..out.rfind(&entry).unwrap_or(0)].matches('\n').count();
    (out, line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalog_entry_has_a_current_value_and_parses_back() {
        let s = Settings::default();
        for e in CATALOG {
            let v = current(&s, e.key);
            assert!(!v.is_null() || matches!(e.kind, Kind::Number { .. }), "{} yields no value", e.key);
            // The settings file must read this value and produce the same setting.
            if !v.is_null() {
                let text = settings::set_value_in("{}", e.key, &v);
                let (parsed, warnings) = settings::parse_layers(&text, &[]).unwrap();
                assert!(warnings.is_empty(), "{}: {warnings:?}", e.key);
                assert_eq!(current(&parsed, e.key), v, "{}", e.key);
            }
        }
    }

    #[test]
    fn input_is_checked_against_the_kind() {
        let n = Kind::Number { min: 6., max: 48. };
        assert_eq!(parse_input(n, "14"), Ok(Value::from(14)));
        assert_eq!(parse_input(n, "13.5"), Ok(Value::from(13.5)));
        assert!(parse_input(n, "99").is_err());
        assert!(parse_input(n, "büyük").is_err());
        assert_eq!(parse_input(n, ""), Ok(Value::Null));
        assert_eq!(parse_input(Kind::Text, " /bin/zsh "), Ok(Value::from("/bin/zsh")));
        assert!(parse_input(Kind::Choice(&["a", "b"]), "c").is_err());
    }

    #[test]
    fn binding_line_goes_before_the_closing_bracket() {
        let (t, line) = add_binding_line(settings::KEYBINDINGS_TEMPLATE, "git.stash");
        let lines: Vec<&str> = t.lines().collect();
        assert_eq!(lines[line].trim(), r#"{ "key": "", "command": "git.stash" },"#);
        assert_eq!(lines[line + 1].trim(), "]");
        // No comma on the previous item: it is added, the result is valid JSONC.
        let (t, line) = add_binding_line("[ { \"key\": \"a\", \"command\": \"x\" } ]", "y");
        assert_eq!(t.lines().nth(line).map(str::trim), Some(r#"{ "key": "", "command": "y" },"#));
        assert!(serde_json::from_str::<Value>(&theme::themes::strip_jsonc(&t)).is_ok(), "{t}");
        assert_eq!(add_binding_line("", "z").1, 1);
        // Brackets/braces in a comment aren't taken for the insertion point.
        let text = "[\n  { \"key\": \"a\", \"command\": \"x\" } // bkz. {belge}\n]\n// son not: [docs]\n";
        let (t, line) = add_binding_line(text, "y");
        assert_eq!(t.lines().nth(line).map(str::trim), Some(r#"{ "key": "", "command": "y" },"#), "{t}");
        let v = serde_json::from_str::<Value>(&theme::themes::strip_jsonc(&t)).expect(&t);
        assert_eq!(v.as_array().map(Vec::len), Some(2));
    }
}
