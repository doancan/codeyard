# Settings reference

<!-- Generated from crates/app/src/workbench_settings.rs and settings_docs.rs. Do not edit by hand:
run `UPDATE_DOCS=1 cargo test -p app docs_reference` after changing a setting. -->

Settings live in `~/.config/codeyard/settings.jsonc` (comments and trailing commas are fine); a folder can override them in
`.codeyard/settings.jsonc` or `.vscode/settings.json`. Every setting below can also be changed in the Settings page (⌘,)
unless it is listed under *File-only settings*. A wrong value never breaks the app: that key keeps its default and the
status bar reports it.

## `data`

| Setting | Values | Default | What it does |
| --- | --- | --- | --- |
| `data.indent` | number 1–8 | `2` | Spaces per level in the JSON and YAML written by Data: Convert To… and Data: Query… |
| `data.sortKeys` | `true` / `false` | `false` | Sort object keys alphabetically in the output of Data: Convert To… and Data: Query… (off: keep the file's order) |

## `diffEditor`

| Setting | Values | Default | What it does |
| --- | --- | --- | --- |
| `diffEditor.renderSideBySide` | `true` / `false` | `true` | Diff view side by side (off: inline) |

## `editor`

| Setting | Values | Default | What it does |
| --- | --- | --- | --- |
| `editor.autoClosingBrackets` | `true` / `false` | `true` | Insert the closing bracket or quote |
| `editor.bracketPairColorization` | `true` / `false` | `true` | Color matching brackets by nesting depth |
| `editor.caseLocale` | `auto`, `tr`, `en` | `auto` | Language of Uppercase / Lowercase / Title Case: auto (Turkish rules when the text has ı İ ş ğ), tr (i ↔ İ, ı ↔ I) or en |
| `editor.codeLens` | `true` / `false` | `true` | Show code lenses (reference counts, run/debug) above symbols |
| `editor.colorDecorators` | `true` / `false` | `true` | Show a color swatch next to #rrggbb values |
| `editor.cursorBlinking` | `blink`, `solid` | `blink` | Cursor animation |
| `editor.cursorStyle` | `line`, `block`, `underline` | `line` | Shape of the cursor |
| `editor.cursorWidth` | number 1–8 | `2.0` | Width of the line cursor (px) |
| `editor.editorconfig` | `true` / `false` | `true` | Apply .editorconfig files (indent, line endings, final newline…) |
| `editor.folding` | `true` / `false` | `true` | Show fold arrows in the gutter |
| `editor.fontFamily` | text | empty | Editor font (empty: built-in monospace) |
| `editor.fontLigatures` | `true` / `false` | `false` | Draw programming ligatures (=>, !=, …) in the editor font |
| `editor.fontSize` | number 6–48 | `12` on macOS, `14` on Windows and Linux | Font size of the editor |
| `editor.formatOnPaste` | `true` / `false` | `false` | Format pasted text with the language server |
| `editor.formatOnSave` | `true` / `false` | `false` | Format with the language server when saving |
| `editor.formatOnType` | `true` / `false` | `false` | Let the language server format while typing }, ; or Enter |
| `editor.hover.enabled` | `true` / `false` | `true` | Show a tooltip when the mouse rests on a word |
| `editor.indentGuides` | `true` / `false` | `true` | Show indent guides |
| `editor.inlayHints.enabled` | `true` / `false` | `true` | Show type and parameter name hints from the language server |
| `editor.insertSpaces` | `true` / `false` | `true` | Indent with spaces instead of tabs |
| `editor.lineHeight` | number 10–80 | `18` on macOS, `20` on Windows and Linux | Line height in pixels |
| `editor.lineNumbers` | `on`, `off`, `relative` | `on` | Line numbers in the gutter |
| `editor.linkedEditing` | `true` / `false` | `true` | Rename the matching HTML/XML tag while you edit one |
| `editor.matchBrackets` | `true` / `false` | `true` | Highlight the matching bracket |
| `editor.minimap.enabled` | `true` / `false` | `true` | Show the minimap |
| `editor.quickSuggestions` | `true` / `false` | `true` | Suggestions pop up while typing (off: only Ctrl+Space) |
| `editor.renderLineHighlight` | `line`, `none` | `line` | Highlight the line with the cursor |
| `editor.renderWhitespace` | `none`, `boundary`, `all` | `none` | Show spaces and tabs as marks |
| `editor.semanticHighlighting.enabled` | `true` / `false` | `true` | Color symbols by their meaning, from the language server |
| `editor.stickyScroll.enabled` | `true` / `false` | `true` | Pin the enclosing function or class line while scrolling |
| `editor.tabSize` | number 1–16 | detected | Spaces per indent level (empty: detect per file) |
| `editor.unicodeHighlight.enabled` | `true` / `false` | `true` | Box invisible and look-alike Unicode characters (zero-width spaces, bidi controls, Cyrillic letters inside Latin words) |
| `editor.wordBasedSuggestions` | `true` / `false` | `true` | Suggest words from the file when there is no language server |
| `editor.wordHighlight` | `true` / `false` | `true` | Highlight other uses of the word under the cursor |
| `editor.wordWrap` | `off`, `on` | `off` | Wrap long lines at the window edge |

## `emmet`

| Setting | Values | Default | What it does |
| --- | --- | --- | --- |
| `emmet.triggerExpansionOnTab` | `true` / `false` | `true` | Expand Emmet abbreviations (div.box>ul>li*3) with Tab |

## `explorer`

| Setting | Values | Default | What it does |
| --- | --- | --- | --- |
| `explorer.autoReveal` | `true` / `false` | `true` | Select the active file in the explorer |
| `explorer.compactFolders` | `true` / `false` | `true` | Show folders that only hold one folder as one row |
| `explorer.confirmDelete` | `true` / `false` | `true` | Ask before moving a file to the Trash |
| `explorer.confirmDragAndDrop` | `true` / `false` | `true` | Ask before moving a file or folder by dragging it in the explorer |
| `explorer.openEditors` | `true` / `false` | `true` | Show the Open Editors list in the explorer |
| `explorer.sortOrder` | `default`, `filesFirst`, `mixed`, `type` | `default` | Order of files and folders in the explorer |

## `files`

| Setting | Values | Default | What it does |
| --- | --- | --- | --- |
| `files.autoSave` | `off`, `afterDelay`, `onFocusChange` | `off` | Save files automatically |
| `files.autoSaveDelay` | number 100–600000 | `1000` | Delay for afterDelay (ms) |
| `files.enableTrash` | `true` / `false` | `true` | Move deleted local files to the Trash |
| `files.fallbackEncoding` | `windows1254`, `iso88599`, `iso88591`, `utf8` | `windows1254` | Encoding for files that are not UTF-8 |
| `files.hotExit` | `true` / `false` | `true` | Keep unsaved changes across restarts |
| `files.insertFinalNewline` | `true` / `false` | `false` | End files with a newline when saving |
| `files.trimTrailingWhitespace` | `true` / `false` | `false` | Remove trailing spaces when saving |

## `git`

| Setting | Values | Default | What it does |
| --- | --- | --- | --- |
| `git.autofetch` | `true` / `false` | `false` | Fetch from the remote in the background while the window is in front |
| `git.autofetchPeriod` | number 30–86400 | `180` | Seconds between background fetches |
| `git.confirmSync` | `true` / `false` | `false` | Ask before Sync pulls or pushes. Your own settings only; a folder cannot change it |
| `git.enableSmartCommit` | `true` / `false` | `true` | Commit all tracked changes when nothing is staged |
| `git.fetchOnPull` | `true` / `false` | `false` | Fetch every remote before pulling |
| `git.graph.allBranches` | `true` / `false` | `true` | Graph starts with every branch |
| `git.graph.limit` | number 20–5000 | `300` | Commits drawn in the Source Control graph |
| `git.graph.showAuthor` | `true` / `false` | `true` | Show the author name on Source Control graph rows |
| `git.graph.showHash` | `true` / `false` | `false` | Show the short commit hash on Source Control graph rows |
| `git.graph.view` | `graph`, `compact`, `dates` | `graph` | Source Control graph view: graph (branch lanes), compact (one line per commit) or dates (grouped by day) |
| `git.inlineBlame` | `true` / `false` | `true` | Show who last changed the cursor line (and when) at the end of the line |
| `git.postCommitCommand` | `none`, `push`, `sync` | `none` | What runs after a commit: nothing, push or sync. Your own settings only; a folder cannot change it |
| `git.pruneOnFetch` | `true` / `false` | `true` | Fetch removes remote branches that were deleted on the server |
| `git.pullTags` | `true` / `false` | `true` | Pull also fetches all tags |
| `git.rebaseWhenSync` | `true` / `false` | `false` | Pull with rebase instead of a merge commit |
| `git.timeline.limit` | number 5–1000 | `100` | Commits listed in the explorer's Timeline |

## `http`

| Setting | Values | Default | What it does |
| --- | --- | --- | --- |
| `http.confirmRequests` | `never`, `remote`, `always` | `remote` | Ask before .http requests and schema downloads leave this computer: never, remote (the first request to each new host per session) or always. Your own settings only; a folder cannot change it |
| `http.runOn` | `workspace`, `local` | `workspace` | Where .http requests are sent from: the workspace host (the server when connected over SSH) or this computer |

## `json`

| Setting | Values | Default | What it does |
| --- | --- | --- | --- |
| `json.schemaDownload.enable` | `true` / `false` | `true` | Download schemas from SchemaStore and cache them (off: only local and cached schemas) |
| `json.validate.enable` | `true` / `false` | `true` | Check JSON and YAML files against their schema (problems, hover, suggestions) |

## `keymap`

| Setting | Values | Default | What it does |
| --- | --- | --- | --- |
| `keymap.preset` | text | empty | Keymap preset from an installed extension, as publisher.name/label (empty: Codeyard's own keys) |

## `lsp`

| Setting | Values | Default | What it does |
| --- | --- | --- | --- |
| `lsp.enabled` | `true` / `false` | `true` | Start language servers |

## `modules`

| Setting | Values | Default | What it does |
| --- | --- | --- | --- |
| `modules.containers.enabled` | `true` / `false` | `false` | Enable container environments (user settings only; disconnect containers before disabling) |
| `modules.webPreview.enabled` | `true` / `false` | `false` | Enable Web Preview for HTML, CSS and JavaScript (user settings only; stops servers when disabled) |

## `ports`

| Setting | Values | Default | What it does |
| --- | --- | --- | --- |
| `ports.autoForward` | `true` / `false` | `true` | Forward ports opened on SSH hosts |
| `ports.localBind` | `loopback`, `all` | `loopback` | Where forwarded ports listen: this computer only, or every network interface |

## `search`

| Setting | Values | Default | What it does |
| --- | --- | --- | --- |
| `search.collapseResults` | `auto`, `alwaysCollapse`, `alwaysExpand` | `auto` | Start search results folded or open |
| `search.maxResults` | number 1–100000 | `2000` | Stop a search after this many matches |
| `search.showLineNumbers` | `true` / `false` | `true` | Show line numbers in search results |
| `search.smartCase` | `true` / `false` | `false` | Ignore case unless the query has capital letters |
| `search.structural.maxFiles` | number 1–100000 | `5000` | Most files a structural search reads and parses |

## `security`

| Setting | Values | Default | What it does |
| --- | --- | --- | --- |
| `security.workspaceTrust.enabled` | `true` / `false` | `true` | Ask before running tasks, tests or the debugger of an untrusted folder |

## `spelling`

| Setting | Values | Default | What it does |
| --- | --- | --- | --- |
| `spelling.checkComments` | `true` / `false` | `true` | Check comments in code files too (prose files are always checked) |
| `spelling.enabled` | `true` / `false` | `true` | Underline misspelled words in prose files and in code comments |

## `terminal`

| Setting | Values | Default | What it does |
| --- | --- | --- | --- |
| `terminal.allowClipboardWrite` | `true` / `false` | `false` | Let programs in the terminal write to the clipboard |
| `terminal.confirmOnClose` | `running`, `always`, `never` | `running` | Ask before closing a terminal |
| `terminal.defaultProfile` | text | empty | Profile new terminals use |
| `terminal.fontSize` | number 6–48 | `12` on macOS, `14` on Windows and Linux | Font size of the terminal |
| `terminal.lineHeight` | number 1–2 | `1.2` | Terminal row height, as a multiple of the font size |
| `terminal.sanitizePaste` | `auto`, `always`, `off` | `auto` | Remove control characters from pasted text |
| `terminal.scrollSensitivity` | number 0.1–10 | `2.0` | Mouse wheel and trackpad scroll speed multiplier |
| `terminal.scrollback` | number 0–50000 | `2000` | Lines kept in terminal history |
| `terminal.shell` | text | empty | Shell for new terminals (empty: login shell) |
| `terminal.shellIntegration` | `true` / `false` | `true` | Mark prompts, commands and the folder in zsh, bash and fish |
| `terminal.warnMultilinePaste` | `unbracketed`, `always`, `never` | `unbracketed` | Ask before pasting several lines into the terminal |

## `ui`

| Setting | Values | Default | What it does |
| --- | --- | --- | --- |
| `ui.language` | `auto`, `en`, `tr` | `auto` | Language of the menus, command palette, Welcome screen, status bar, Source Control, Explorer, Settings page chrome, file operations and Git prompts (the user guide lists exactly what is translated; the rest stays English): auto (the system language) or a language code, en or tr. Your own settings only; a folder cannot change it |

## `update`

| Setting | Values | Default | What it does |
| --- | --- | --- | --- |
| `update.check` | `ask`, `daily`, `manual` | `ask` | Look for new Codeyard versions: ask (not decided yet; nothing is checked), daily (once a day, downloads two small files from github.com, the checksum list of the latest release and its signature; GitHub sees your IP address and that the request comes from curl, nothing about your files) or manual (only when you choose Check for Updates). Your own settings only; a folder cannot change it |

## `webPreview`

| Setting | Values | Default | What it does |
| --- | --- | --- | --- |
| `webPreview.host` | `127.0.0.1`, `::1`, `0.0.0.0`, `::` | `127.0.0.1` | Listen address (local: 127.0.0.1 or ::1; network: 0.0.0.0 or ::) |
| `webPreview.liveReload` | `true` / `false` | `true` | Refresh the browser when saved preview files change |
| `webPreview.port` | number 0–65535 | `0` | Preview port (0: choose a free port automatically) |
| `webPreview.reloadInterval` | number 250–60000 | `1000` | Milliseconds between live preview checks |
| `webPreview.root` | text | empty | Served folder (empty: automatic; relative to the workspace, or an absolute path on the active host) |

## `window`

| Setting | Values | Default | What it does |
| --- | --- | --- | --- |
| `window.restoreSession` | `true` / `false` | `true` | Bring a folder's tabs and unsaved changes back when you open it |
| `window.startup` | `welcome`, `continue`, `blank` | `welcome` | What opens at start: welcome (the start page), continue (where you left off, SSH hosts included) or blank (an empty window) |

## `workbench`

| Setting | Values | Default | What it does |
| --- | --- | --- | --- |
| `workbench.activityBar.visible` | `true` / `false` | `true` | Show the activity bar |
| `workbench.breadcrumbs.enabled` | `true` / `false` | `true` | Show the file path above the editor |
| `workbench.editor.showTabs` | `true` / `false` | `true` | Show the editor tab bar |
| `workbench.fontFamily` | text | empty | UI font of the whole window (empty: system font) |
| `workbench.iconTheme` | `glyphs`, `minimal`, `codeyard`, `badges`, `colorful`, `apple`, `material`, `none` | `glyphs` | File and folder icons |
| `workbench.reduceMotion` | `auto`, `on`, `off` | `auto` | Leave out animations: the blinking cursor, spinners and sliding panels (auto: follow the system setting) |
| `workbench.statusBar.visible` | `true` / `false` | `true` | Show the status bar |
| `workbench.tabs.closeButton` | `hover`, `always`, `off` | `hover` | When the close button of a tab shows |
| `workbench.tabs.showIcons` | `true` / `false` | `true` | Show file icons on tabs |
| `workbench.tree.guides` | `true` / `false` | `true` | Draw the folder hierarchy lines in the explorer |
| `workbench.tree.indent` | number 8–32 | `16.0` | Indent of each folder level in the explorer (px) |

## File-only settings

| Setting | Values | Default | What it does |
| --- | --- | --- | --- |
| `theme` | theme id or `auto` | `auto` | Colour theme; `auto` follows the system light/dark setting. Picked with the theme picker (⌘K ⌘T). |
| `editor.rulers` | list of columns | `[]` | Draw vertical rulers at these columns, e.g. `[80, 120]`. |
| `files.exclude` | list of globs | `[]` | Hide matching paths from the explorer and from file lists, e.g. `["**/node_modules"]`. |
| `search.exclude` | list of globs | `[]` | Skip matching paths in Find in Files. |
| `compare.folders.ignore` | list of globs | `[".git", "node_modules", "target"]` | Names and paths *Compare: Folders…* leaves out on both sides (gitignore syntax). |
| `compare.folders.maxFiles` | number 100–5000000 | `50000` | Entries per side *Compare: Folders…* reads before it stops and reports that the result is partial. |
| `workbench.startupEditor` | string | `welcome` | VS Code compatible alias of `window.startup`. |
| `editor.codeActionsOnSave` | object | `{}` | Code actions to run on save before formatting, e.g. `{ "source.organizeImports": "explicit" }`; each gives up after 2 s. |
| `files.associations` | object of glob → language | `{}` | Force a language mode for matching files, e.g. `{ "*.tpl": "html" }`. |
| `terminal.profiles.macos` | object of name → profile | `{}` | Profiles for terminals on macOS: `{ "zsh": { "path": "/bin/zsh", "args": ["-l"], "env": { … }, "color": "info" } }`; `args` replace the login flag, `color` is accent, success, warning, danger, info or muted. Needs a trusted folder in a project file. |
| `terminal.profiles.linux` | object of name → profile | `{}` | Profiles for terminals on Linux (also SSH hosts), like `terminal.profiles.macos`. |
| `terminal.profiles.windows` | object of name → profile | `{}` | Profiles for terminals on Windows, like `terminal.profiles.macos`. |
| `ports.rules` | object of port or range → rule | `{}` | Label, protocol (`http`, `https`, `tcp`) and detection behaviour (`notify`, `silent`, `openBrowser`, `ignore`) per forwarded port, e.g. `{ "3000": { "label": "web", "onForward": "openBrowser" } }`; a single port beats a range. |
| `json.schemas` | list of `{ fileMatch, url }` | `[]` | Associate JSON files with a schema (URL or path relative to the workspace). Ignored in untrusted folders. |
| `yaml.schemas` | object of schema → globs | `{}` | Associate YAML files with a schema. Ignored in untrusted folders. |
| `lsp.servers` | object of language → `{ command, args, settings, initializationOptions }` or `null` | built-in servers | Replace, configure or turn off (`null`) the language server of a language. `settings` is what the server reads as its configuration (dotted keys are nested objects; a change reaches a running server without a restart), `initializationOptions` is sent when it starts; without `command` the default server is used. Keys: `rust`, `c`, `go`, `python`, `typescript`, `java`, `csharp`, `php`, `ruby`, `kotlin`, `swift`, `lua`, `bash`, `html`, `css`; `json`, `yaml` and `toml` have no default and are opt-in. Ignored in untrusted folders. |
| `spelling.languages` | list of dictionary ids | `["en"]` | Dictionaries used together; install more with *Spelling: Install Dictionary…*. |
| `workbench.colorCustomizations` | object | `{}` | Override single colour roles; an entry in `[Theme Name]` applies to that theme only. |
