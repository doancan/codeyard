//! The files created on first launch: every setting with its comment, and the keyboard shortcuts file.

/// File created on first launch: every setting with its comment.
pub const TEMPLATE: &str = r##"// codeyard settings. Saved changes apply immediately.
// Comments and trailing commas are fine.
{
  // "auto" follows the system (Codeyard Dark / Codeyard Light), or a theme id:
  // codeyard-dark, codeyard-light, codeyard-one, studio-dark, midnight, graphite, ember, fjord, daylight, paper,
  // quarry, dracula, one-dark, nord, solarized-dark, solarized-light,
  // github-light, high-contrast-dark, high-contrast-light, user:<file name>
  "theme": "auto",
  // Optional HTML/CSS/JS preview. No server runs until explicitly started.
  "modules.webPreview.enabled": false,
  // Web Preview options (user settings only). A running preview restarts when changed.
  // 0 selects a free port. A fixed port must be available.
  "webPreview.port": 0,
  // 127.0.0.1 or ::1: this computer. 0.0.0.0 or :: also allow network connections.
  "webPreview.host": "127.0.0.1",
  "webPreview.liveReload": true,
  // Milliseconds between browser checks (250–60000).
  "webPreview.reloadInterval": 1000,
  // Empty: automatic. Relative to the workspace, or an absolute path on the active host.
  "webPreview.root": "",

  // "editor.fontFamily": "JetBrains Mono",
  // Default: 12 (line 18) on macOS, 14 (line 20) on Windows and Linux.
  // "editor.fontSize": 12,
  // "editor.lineHeight": 18,
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

  // File and folder icons: "glyphs", "minimal", "codeyard", "badges", "colorful", "apple", "material", "none"
  "workbench.iconTheme": "glyphs",
  "workbench.statusBar.visible": true,
  "workbench.activityBar.visible": true,
  // UI font for the whole window (the editor has its own "editor.fontFamily").
  // "workbench.fontFamily": "Inter",
  "workbench.editor.showTabs": true,
  "workbench.tabs.showIcons": true,
  // "hover" (the x shows on the active tab or on hover), "always", "off"
  "workbench.tabs.closeButton": "hover",
  // "on" leaves out animations (blinking cursor, spinners, sliding panels); "auto" follows the system's
  // Reduce motion setting (macOS, Windows).
  "workbench.reduceMotion": "auto",
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
  // Source Control graph rows: show the author name and the short hash.
  "git.graph.showAuthor": true,
  "git.graph.showHash": false,
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
  // Deleted local files go to the Trash instead of being deleted for good (SSH hosts have none: deletes there are
  // always permanent). Off by default: a delete is permanent and asks first.
  "files.enableTrash": false,
  // Files that are not UTF-8 (and have no BOM) are read as: "windows1254"
  // (Turkish), "iso88599" or "iso88591". They are saved back the same way.
  "files.fallbackEncoding": "windows1254",

  "search.maxResults": 2000,
  // --- git-sync ---
  // Fetch in the background while the window is in front, every this many seconds (30 or more). Not in folders you have not trusted.
  "git.autofetch": true,
  "git.autofetchPeriod": 180,
  // Ask before Sync pulls or pushes (your own settings only: a folder cannot change it).
  "git.confirmSync": false,
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
  // Default: 12 on macOS, 14 on Windows and Linux.
  // "terminal.fontSize": 12,
  // Row height as a multiple of the font size (1 to 2).
  "terminal.lineHeight": 1.2,
  "terminal.scrollback": 2000,
  // Lines of each terminal's history kept for the next start, as plain text (0: none). Terminals always come back as
  // new shells: nothing is run again.
  // "terminal.persistentScrollback": 0,
  // Minutes the terminals of a window keep running after Codeyard quits, crashes or is killed, so the next start takes
  // them back (0: they end with the app; at most 1440).
  // "terminal.persistentSessionTimeout": 5,
  "terminal.scrollSensitivity": 2,
  // Let programs in the terminal write to the clipboard (OSC 52). Off by default: a program you run, or a file you cat, could replace
  // what you copied.
  // "terminal.allowClipboardWrite": false,
  // Pasting: "auto" removes control characters unless the shell reads the paste as text (bracketed paste), "always" or
  // "off"; bracketed-paste markers inside pasted text are always removed. A paste of several lines asks first
  // ("unbracketed": when the shell would run each line at once, "always" or "never").
  // Closing a terminal never asks by default; "running" asks when a program other than the shell is in the foreground, "always" asks every time.
  // "terminal.confirmOnClose": "never",
  // Profiles for new terminals, per operating system of the machine they run on; "args" replace the login flag.
  // "terminal.profiles.macos": { "zsh": { "path": "/bin/zsh" }, "bash": { "path": "/bin/bash", "args": ["-l"], "env": { "LANG": "en_US.UTF-8" }, "color": "info" } },
  // "terminal.defaultProfile": "zsh",
  // "terminal.sanitizePaste": "auto",
  // "terminal.warnMultilinePaste": "unbracketed",

  // Ports forwarded from SSH hosts listen on this computer only ("loopback") or on every network interface ("all",
  // reachable from other computers).
  // "ports.localBind": "loopback",
  // Per port or range: a label, the protocol ("http", "https" or "tcp") and what happens when the port is detected
  // ("notify", "silent", "openBrowser" or "ignore").
  // "ports.rules": { "3000": { "label": "web", "onForward": "openBrowser" }, "9000-9100": { "onForward": "ignore" } },

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
  // Server settings, without replacing the server: "lsp.servers": { "rust": { "settings": { "rust-analyzer.check.command": "clippy" } } },
  // "Preferences: Apply Rust Setup…" writes a common Rust setup here after showing the exact changes; "Preferences:
  // Reset Rust Setup…" removes it again.
  "lsp.enabled": true,
}
"##;

/// The optional Rust setup: (path into settings.jsonc, value). Clippy for checks, checks with the Cargo features Run and
/// Debug build with (`rust.features`), rustfmt (through rust-analyzer) on save, and an output folder of its own for
/// checks (inside the target directory, `rust.targetDir` or Cargo's) so they do not wait for builds. Reset removes these.
pub fn rust_setup(s: &crate::settings::Settings) -> Vec<(Vec<&'static str>, serde_json::Value)> {
    use serde_json::json;
    let ra = |key| vec!["lsp.servers", "rust", "settings", key];
    vec![
        (ra("rust-analyzer.check.command"), json!("clippy")),
        (ra("rust-analyzer.cargo.features"), json!(s.rust_build.features)),
        (ra("rust-analyzer.cargo.noDefaultFeatures"), json!(s.rust_build.no_default_features)),
        (ra("rust-analyzer.cargo.targetDir"), json!(true)),
        (vec!["editor.formatters", "rust"], json!("lsp")),
        (vec!["editor.formatOnSave"], json!(true)),
    ]
}

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
