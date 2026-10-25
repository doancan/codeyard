# User guide

Everything Codeyard does, by area. Key tables show macOS first; on Linux and Windows `⌘` is Ctrl and `⌥` is Alt unless a row says
otherwise (Windows is released, Linux is not yet; see [platforms.md](platforms.md)). The in-app *Help: Keyboard Shortcuts Reference*
is the complete list and always matches your key bindings; every setting is in [settings.md](settings.md).

<!-- toc -->
- [Keys](#keys)
- [Welcome, Settings page and customization](#welcome-settings-page-and-customization)
- [Source control, Timeline and more](#source-control-timeline-and-more)
- [Settings and keyboard shortcuts](#settings-and-keyboard-shortcuts)
- [Interface language](#interface-language)
- [Updates](#updates)
- [Themes](#themes)
- [Git](#git)
- [Projects, favorites, bookmarks, tasks](#projects-favorites-bookmarks-tasks)
- [Highlighting, diffs and crash reports](#highlighting-diffs-and-crash-reports)
- [Debugging](#debugging)
- [Workspaces with several folders](#workspaces-with-several-folders)
- [Language servers](#language-servers)
- [JSON and YAML schemas](#json-and-yaml-schemas)
- [Colours](#colours)
- [Spell check](#spell-check)
- [Merge editor](#merge-editor)
- [GitHub pull requests](#github-pull-requests)
- [HTTP files](#http-files)
- [Port forwarding](#port-forwarding)
- [Remote (SSH)](#remote-ssh)
<!-- /toc -->

## Keys

| | macOS | Linux / Windows |
| --- | --- | --- |
| Go to file | ⌘P | Ctrl+P |
| Command palette | ⇧⌘P | Ctrl+Shift+P |
| Go to line | ⌃G | Ctrl+G |
| Color theme | ⌘K ⌘T | Ctrl+K Ctrl+T |
| Toggle sidebar | ⌘B | Ctrl+B |
| Find / replace in file | ⌘F / ⌥⌘F | Ctrl+F / Ctrl+H |
| Next / previous match | ↵ / ⇧↵, ⌘G / ⇧⌘G | Enter / Shift+Enter, F3 / Shift+F3 |
| Match case / whole word / regex / preserve case (replace) | ⌥C / ⌥W / ⌥R / ⌥P | Alt+C / Alt+W / Alt+R / Alt+P |
| Replace all | ⌥⌘↵ | Ctrl+Alt+Enter |
| Find in selection / select all matches (find bar) | ⌥⌘L / ⌥↵ | Ctrl+Alt+L / Alt+Enter |
| Find in files | ⇧⌘F | Ctrl+Shift+F |
| Structural search (replace: from the palette) | ⌥⇧⌘F | Ctrl+Alt+Shift+F |
| Split editor (same file in the other group) | ⌘\ | Ctrl+\ |
| Move tab to the other group | ⌃⌘→ / ⌃⌘← | Ctrl+Ctrl+→ (rebind) |
| Focus left / right group | ⌘1 / ⌘2 | Ctrl+1 / Ctrl+2 |
| Source control | ⌃⇧G | Ctrl+Shift+G |
| Problems | ⇧⌘M | Ctrl+Shift+M |
| Go to definition | F12 or ⌘-click | F12 or Ctrl+click |
| Suggestions / hover | ⌃Space / ⌘K ⌘I | Ctrl+Space / Ctrl+K Ctrl+I |
| Next / previous change (diff) | F7 / ⇧F7 | F7 / Shift+F7 |
| Word left / right (+Shift selects) | ⌥← / ⌥→ | Ctrl+← / Ctrl+→ |
| Delete word left / right | ⌥⌫ / ⌥⌦ | Ctrl+Backspace / Ctrl+Delete |
| Select word / line | double / triple click | double / triple click |
| Add cursor (click) | ⌥-click | Alt+click |
| Add cursor above / below | ⌥⌘↑ / ⌥⌘↓ | Ctrl+Alt+↑ / Ctrl+Alt+↓ |
| Select next occurrence | ⌘D | Ctrl+D |
| Back to one cursor | Esc | Esc |
| Terminal / new terminal | ⌃\` / ⌃⇧\` | Ctrl+\` / Ctrl+Shift+\` |
| Terminal copy / paste | ⌘C / ⌘V | Ctrl+Shift+C / Ctrl+Shift+V (Ctrl+C stays SIGINT) |
| Toggle bottom panel (opens a terminal if none) | ⌘J | Ctrl+J |
| Rename / delete (file tree) | F2 / ⌘⌫ | F2 / Ctrl+Backspace |
| Toggle line comment | ⌘/ | Ctrl+/ |
| Indent / outdent lines | Tab / ⇧Tab (with a selection) | Tab / Shift+Tab |
| Move lines up / down | ⌥↑ / ⌥↓ | Alt+↑ / Alt+↓ |
| Copy lines up / down | ⇧⌥↑ / ⇧⌥↓ | Shift+Alt+↑ / Shift+Alt+↓ |
| Delete line / select line | ⇧⌘K / ⌘L | Ctrl+Shift+K / Ctrl+L |
| Rename symbol | F2 | F2 |
| Find references | ⇧F12 | Shift+F12 |
| Go to symbol in file | ⇧⌘O | Ctrl+Shift+O |
| Go to symbol in workspace (or `#` in ⌘P) | ⌘T | Ctrl+T |
| Quick fix | ⌘. | Ctrl+. |
| Format document | ⇧⌥F | Shift+Alt+F |
| Save all | ⌥⌘S | Ctrl+Alt+S |
| Zoom in / out / reset | ⌘= / ⌘- / ⌘0 | Ctrl+= / Ctrl+- / Ctrl+0 |
| Back / forward | ⌃- / ⌃⇧- | Ctrl+Alt+- / Ctrl+Shift+- |
| Reopen closed tab | ⇧⌘T | Ctrl+Shift+T |
| New file / save as / new window | ⌘N / ⇧⌘S / ⇧⌘N | Ctrl+N / Ctrl+Shift+S / Ctrl+Shift+N |
| Word wrap | ⌥Z | Alt+Z |
| Fold / unfold / fold all / unfold all | ⌥⌘[ / ⌥⌘] / ⌘K ⌘0 / ⌘K ⌘J | Ctrl+Alt+[ / ] / Ctrl+K Ctrl+0 / J |
| Select all occurrences | ⇧⌘L | Ctrl+Shift+L |
| Expand / shrink selection | ⌃⇧⌘→ / ⌃⇧⌘← | (rebind) |
| Join lines | ⌃J | Ctrl+J |
| Next / previous problem | F8 / ⇧F8 | F8 / Shift+F8 |
| Go to implementations | ⌘F12 | Ctrl+F12 |
| Switch project | ⌃R | Ctrl+R |
| Run build task | ⇧⌘B | Ctrl+Shift+B |
| Start / continue debugging, stop, restart | F5, ⇧F5, ⇧⌘F5 | F5, Shift+F5, Ctrl+Shift+F5 |
| Toggle breakpoint (or click the gutter) | F9 | F9 |
| Step over / into / out | F10, F11, ⇧F11 | F10, Ctrl+F11, Shift+F11 |
| Toggle bookmark / next / previous | ⌥⌘K / ⌥⌘L / ⌥⌘J | Ctrl+Alt+K / L / J |
| Cut / copy / paste in the file tree | ⌘X / ⌘C / ⌘V | Ctrl+X / Ctrl+C / Ctrl+V |
| Switch editor (open files, last used first) | ⌘E | Ctrl+E |
| Zen mode / full screen | ⌘K Z / ⌃⌘F | Ctrl+K Z / F11 |
| Duplicate selection (or line) | ⇧⌘D | Ctrl+Shift+D |
| Transpose characters | ⌃T | (palette) |
| Type in the file tree to jump to a name | letters | letters |
| Settings (UI page) | ⌘, | Ctrl+, |
| Markdown preview | ⇧⌘V | Ctrl+Shift+V |
| Next / previous search result | F4 / ⇧F4 | F4 / Shift+F4 |
| Toggle block comment | ⇧⌥A | Shift+Alt+A |
| Go to last edit location | ⌘K ⌘Q | Ctrl+K Ctrl+Q |
| Open / close everything inside a folder (file tree) | ⇧→ / ⇧← | Shift+→ / Shift+← |
| Expand snippet / next tab stop / previous | Tab / Tab / ⇧Tab | Tab / Tab / Shift+Tab |

Right-click in the file tree for new file/folder, cut/copy/paste, rename, delete, copy path,
favorites. Drag files and folders onto a folder to move them (an existing file is never
overwritten). Deleting a local file moves it to the Trash (`"files.enableTrash": false` deletes
permanently; on SSH hosts there is no Trash, so it asks). Right-click a tab for close others /
to the right / saved, pin, copy path; drag tabs to reorder them or onto the other group.
Right-click in the editor for go to definition / references, rename, cut/copy/paste,
file history and blame. A single click in the tree opens a preview tab (italic) that the next
preview replaces; editing or double-clicking keeps it. Tabs of files with the same name show
the folder that tells them apart; the breadcrumbs above the editor reveal a folder in the tree.

**Explorer.** Vertical guides under every folder arrow show the hierarchy; the guide of the
selected folder is highlighted. Folders that only hold one folder show as one row
(`src/main/java`, `"explorer.compactFolders"`). `"explorer.sortOrder"` is `default` (folders
first), `filesFirst`, `mixed` or `type` (by extension). Type letters with the tree focused to
jump to the next name that starts with them. OPEN EDITORS lists the open files above the tree
(click to switch, × to close, a dot when unsaved; `"explorer.openEditors": false` hides it).
Right-click also has Duplicate, Open to the Side, Select for Compare / Compare with Selected
(two files side by side) and Open in Integrated Terminal (in that folder).

**Editor.** `"editor.rulers": [80, 120]` draws vertical rulers; `"editor.renderWhitespace"`
(`none`, `boundary`, `all`) marks spaces and tabs; `"editor.lineNumbers"` is `on`, `off` or
`relative`; `"editor.cursorStyle"` is `line`, `block` or `underline`. Matching brackets are
colored by nesting depth (`"editor.bracketPairColorization"`), a bar under `#rrggbb` shows the
color (`"editor.colorDecorators"`), and while you scroll inside a function or class its header
lines stay pinned at the top (`"editor.stickyScroll.enabled"`; click one to jump there).
Palette commands: Transform to Title Case, Duplicate Selection, Trim Trailing Whitespace,
Transpose Characters, Reverse Lines, Delete Duplicate Lines, "Search: Find TODO Comments"
(TODO, FIXME, HACK, XXX, BUG), "View: Switch Editor…" and "View: Toggle Zen Mode".

The cursor blinks for a few seconds after you stop typing, then stays solid: an idle window
draws nothing, so the GPU gives its frame buffers back (about 110 MB on a Retina Mac).
`"editor.cursorBlinking": "solid"` turns blinking off.

In the terminal, ⌘-click (Ctrl+click) a URL or a location such as `src/main.rs:12:5`,
`app.ts(3,7)` or a Node stack line to open it (relative paths are taken from the project
folder); holding ⌘ underlines what can be opened. Drag to select, double-click for a word, triple-click for a line, Shift+click
to extend; ⌘C copies. On keyboard layouts without a reachable \` key (Turkish Q, German),
use ⌘J or bind `terminal.toggle` yourself (below). Find with several cursors: copy gives one
line per cursor, and pasting the same number of lines puts one on each cursor.
On a Turkish Q layout ⌘/ is ⇧⌘7; if that is awkward, bind `editor.toggleComment` to
another key.

Brackets and quotes close themselves (typing over the closing one skips it; with a
selection they wrap it); turn off with `"editor.autoClosingBrackets": false`. Long lines
scroll sideways (trackpad or ⇧+wheel). The minimap on the right is on by default
(`"editor.minimap.enabled": false` hides it); click or drag it to jump.

**Folding** is by indentation (works in every language): hover the gutter for the arrows.
Folds belong to the view, so the two groups of a split can fold differently. **Word wrap**
breaks at spaces at the window edge; ↑↓ move by screen row. Sort lines, upper/lower case,
change line endings and indentation (click them in the status bar) are in the palette.
With the same file open in both groups, each group has its own cursors, scroll and find bar.

**Images** (png, jpg, gif, webp, svg, bmp) open in a preview tab.

**Find in files** (⇧⌘F) has a replace field: "Replace All" asks first, then changes open
files in their editors (undoable) and closed files on disk.

### Structural search and replace

"Search: Structural Search…" (⌥⇧⌘F) and "Search: Structural Replace…" open the Search panel in structural mode; the `$X` button in
the search box switches it on and off. The pattern is code in a language you pick (the chip next to the field; a click goes to
the next one; it starts as the language of the active file, and a selection becomes the pattern). It is matched against the
syntax tree, so spacing, line breaks and comments inside the code do not matter, and a match can span several lines.

- `$X` matches one node: an identifier, a call, a whole block. A name used twice (`$X == $X`) must match the same text both times.
- `$$$X` matches zero or more neighbouring nodes, for example all the arguments or all the statements of a block.
- Names are upper case (`$X`, `$ARGS_2`). A name that starts with `_` (and `$_`, `$$$`) matches without remembering. `$x`, a lone
  `$` and `$$` are ordinary text, so shell and PHP variables can be written as they are.
- Everything else must be the same code. A `;` at the end may be left out. Comments are ignored unless the pattern has one.
- A pattern is one expression or statement (or item, or type), not a sequence of them. In Python and Go a block is easiest written
  on one line, or with ⇧↵ in the field. Rust macro arguments are plain tokens: use `$$$` for them.
- If the pattern does not parse, the parser's position is shown under the field and nothing runs.

Results list like any search (file, line, preview), with "replace in this file" and "dismiss" on each file. The replacement field
takes `$X` and `$$$X` of the pattern; any other `$name` stays as typed. "Replace All" asks first, like Replace in Files: open files
change in their editors (one undo step), closed files are written to disk and keep their line endings. Matches inside other matches
(`f(f(1))`) are changed outermost first; search again for the inner ones. If a replacement would leave a file that no longer parses, the
prompt names it and offers to skip it.

How it works and what it leaves out:

- The folder's text search first finds files that contain the longest word of the pattern; only those are read and parsed (`.gitignore`,
  `files.exclude` and `search.exclude` apply, and so do the include and exclude fields). Unsaved editor text is used instead of the
  file on disk. It works over SSH the same way: the server searches, this computer parses.
- Files over 2 MB, binary files, and files with syntax errors are skipped; the second line of the status says how many.
  `"search.structural.maxFiles"` (default 5000) caps how many files are parsed, and the status says when it did.
- Nothing from the folder is run, so it also works in untrusted workspaces, and nothing is written before you confirm.
- Languages: Rust, TypeScript, JavaScript, Python, Go, Java, C, C++, C#, Ruby, PHP, Kotlin, Swift, Lua and Shell. A TypeScript pattern
  also searches `.tsx`, a JavaScript one `.jsx`.

Six recipes (pattern → replacement):

1. Rust, avoid building the default when it is not needed: `$R.unwrap_or($X.clone())` → `$R.unwrap_or_else(|| $X.clone())`.
2. Python, compare with `None` the right way: `$X == None` → `$X is None`.
3. TypeScript, move console logging to a logger: `console.log($$$ARGS)` → `logger.debug($$$ARGS)`; `$$$ARGS` carries any number of arguments.
4. Go, switch the logging package: `fmt.Println($$$A)` → `log.Println($$$A)`.
5. Rust, change a type everywhere it is written: `Box<Vec<$T>>` → `Box<[$T]>`.
6. Rust, only look: `$X == $X` finds a value compared with itself, and `if $C { $$$B }` lists every `if`. Leave the replacement empty to search.

**Encodings:** UTF-8 (with or without BOM), UTF-16 with a BOM, and single-byte files.
A file that isn't valid UTF-8 is read as Windows-1254 (Turkish) by default; change it with
`"files.fallbackEncoding": "iso88599"` or `"iso88591"`. Files are saved in the encoding
they were opened with (shown in the status bar). If you type a character that encoding
can't hold, saving stops with a message instead of writing a broken file.

**Large files:** files over 32 MB open read-only, a few hundred lines at a time (the line
index lives in the agent), so a 500 MB log opens in about the memory of a small one.
⌃G works there too, and ⌘F searches the file on the machine where it is (it is never copied
over SSH); ↵ / ⇧↵ step through the matches.

**Suggestions without a language server:** after two letters, words from the file are
suggested (nearby words first); `"editor.wordBasedSuggestions": false` turns this off.

## Welcome, Settings page and customization

**What opens at start** is a setting, `"window.startup"` (Settings ▸ Window):

- `"welcome"` (default): the **Welcome** screen. Start actions (new file, open folder, clone a
  repository, connect over SSH), your recent projects, customize links, the most useful shortcuts
  and a tip of the day. Nothing reconnects by itself, SSH hosts included.
- `"continue"`: like a browser, the last folder (or SSH host) and its tabs come back. If the folder is
  gone you land on the Welcome screen with a note.
- `"blank"`: an empty window with just the logo.

A folder or file given on the command line always opens, whatever the setting. An empty editor
shows the Welcome content unless the setting is `"blank"`; "Help: Welcome" opens it as a tab.
`"window.restoreSession"` is separate: it brings a folder's tabs and unsaved changes back whenever
you open that folder. Right-click the Dock icon (macOS) for New Window, Open Folder and your
recent projects; they are also under File ▸ Open Recent.

The gear at the bottom of the activity bar is one menu: **About Codeyard** (version), Settings,
Color Theme, File Icon Theme, Keyboard Shortcuts and the Command Palette.

**Settings** (⌘, or "Preferences: Open Settings") is a page with categories, search, switches,
option buttons and number fields; every change is written to `settings.jsonc` (comments stay)
and applies at once. A dot marks values that differ from the default, "Reset" removes them, and the **Modified** view lists them all.
"Preferences: Open Settings (JSON)" opens the file itself. The Uppercase / Lowercase / Title Case commands follow `editor.caseLocale`
(`auto`: Turkish `i`/`İ`, `ı`/`I` rules when the text contains ı İ ş ğ).
"Help: Keyboard Shortcuts Reference" lists every command with its keys (searchable, click to run).

Make it yours:

- **Colors:** pick one of 18 themes (the plain defaults Codeyard Dark and Codeyard Light, Studio Dark, Midnight, Graphite, Ember, Fjord, Quarry,
  Dracula, One Dark, Nord, Solarized Dark/Light, GitHub Light, Daylight, Paper, high contrast…),
  change single colors with `"workbench.colorCustomizations": { "accent": "#ff8800",
  "[Quarry]": { "syntax-string": "#a6e22e" } }` (roles are the names in the theme files; an
  entry in `[brackets]` applies to that theme only), or "Theme: Save Current Colors as User
  Theme…" to keep your tweaks as a theme file. VS Code color themes dropped into
  `~/.config/codeyard/themes` are imported.
- **Icons:** "Theme: Pick File Icon Theme…" previews Minimal (default), Codeyard, Language Badges,
  Colorful Folders, Apple / GitHub (solid blue folders, outline files with a language dot),
  Material (folders colored and marked by purpose, solid colored files) and None (`"workbench.iconTheme"`).
- **Layout:** `workbench.fontFamily` (UI font), `workbench.activityBar.visible`,
  `workbench.statusBar.visible`, `workbench.editor.showTabs`, `workbench.tabs.closeButton`,
  `workbench.tabs.showIcons`, `workbench.breadcrumbs.enabled`, `workbench.tree.indent`.
  Sidebar sections (Open Editors, Outline, Timeline, Graph…) fold when you click their title and
  resize when you drag their edge; the layout is remembered.
- **Editor:** `editor.cursorWidth`, `editor.renderLineHighlight`, `editor.matchBrackets`,
  `editor.folding`, `editor.hover.enabled`, `editor.quickSuggestions`, and
  `files.associations` (`{ "*.tpl": "html" }`) next to the options listed below.
- **Snippets:** type a prefix and press Tab (`fn`, `for`, `def`, `html5`…); "Snippets: Insert
  Snippet…" lists them. "Preferences: Configure User Snippets" opens
  `~/.config/codeyard/snippets/<language>.json` in the VS Code snippet format (`global.json`
  applies to every file).

## Source control, Timeline and more

**Graph.** Source Control shows the history of all branches as colored lanes (at most seven
columns, so the left edge stays tight), two lines per commit: message with branch/tag chips,
author · time and the short hash. Click a commit for its message, files and `+/−` counts below
the graph (click a file for its diff; double-click for the file list). Right-click for copy hash,
create branch or tag here, checkout (detached), cherry-pick, revert and reset (hard reset asks;
a cherry-pick or revert that conflicts is undone, never left half done; for a merge commit the first parent is the mainline).
There is no interactive rebase (squash, reorder, edit): use a terminal for that. The header switches
between all branches and the current branch, searches commits (message, author, hash, branch)
and reloads. Stashes are listed with apply, pop and drop. `"git.graph.limit"`,
`"git.graph.allBranches"` and `"git.timeline.limit"` tune it; the Changes groups fold too.

**Explorer sections.** OPEN EDITORS, the folder tree, **OUTLINE** (functions, types and
headings of the open file, found without a language server) and **TIMELINE** (the file's git
commits plus a saved copy for every save, "local history"; click an entry for a diff).
Clicking the empty space under the tree clears the selection so new files go to the root; open
folders are remembered per project; ⇧→ / ⇧← (or the context menu) open or close everything
inside a folder. Right-click ▸ Find in Folder… limits the search to it.

**Search.** F4 / ⇧F4 step through the results; ↑ / ↓ in the field recall earlier queries;
the toolbar folds or opens all results and re-runs; each file has "replace in this file" and
"dismiss"; `"search.smartCase"`, `"search.exclude"`, `"search.collapseResults"`.

**Also new:** inline blame (who changed the cursor line, at its end; `"git.inlineBlame"`), Markdown
preview (⇧⌘V, live), compare with clipboard / with the saved file, run the selected text (or
line) in the terminal, last edit location, scratch files that need no folder, JSON format and
minify without a language server, camelCase / PascalCase / snake_case / kebab-case /
CONSTANT_CASE transforms, Base64 / URL / HTML encode and decode, more line sorts, insert date or
UUID, block comment toggle, recently used commands first in the palette,
"Git: Clone Repository…".

## Settings and keyboard shortcuts

`⇧⌘P` → "Preferences: Settings" lists the settings with their current values: pick one to
toggle it, choose from its options or type a value; it is written to `settings.jsonc` (your
comments stay) and applies at once. "Preferences: Keyboard Shortcuts" lists every command with
its keys; picking one adds a line for it to `keybindings.jsonc` and opens the file there.
"Preferences: Open Settings" / "Open Keyboard Shortcuts" open the files themselves:
`~/.config/codeyard/settings.jsonc` and `keybindings.jsonc` (comments and trailing commas are
fine). Changes apply on save; mistakes show in the status bar instead of being ignored.
A project can override settings in `.codeyard/settings.jsonc` (or `.vscode/settings.json`).

**Workspace Trust.** A folder you open can carry settings, tasks and debug configurations that run
programs. Until you trust a folder, its project settings cannot set `terminal.shell`, `lsp.servers` or schema
associations (the status bar says which keys were ignored), **language servers do not start** (a notice says so once;
they can run the project's build scripts), `$schema` addresses are only downloaded from SchemaStore, and running a task,
a test or the debugger first asks "Do you trust the authors of this folder?". Folders you had already opened before this
rule existed are trusted once. Trusting is remembered per folder (per host over SSH) and starts the language servers at once;
`Workspace: Trust This Folder` does it without a question. `security.workspaceTrust.enabled: false`
in your own `settings.jsonc` turns the check off; a project file can never change that key.

```jsonc
{
  "theme": "studio-dark",              // or "auto"; see Themes for the ids
  "editor.fontSize": 14,
  "editor.tabSize": 4, "editor.insertSpaces": true,
  "files.autoSave": "afterDelay",      // "off", "afterDelay", "onFocusChange"
  "files.trimTrailingWhitespace": true,
  "files.exclude": ["**/node_modules"],
  "terminal.shell": "/bin/zsh",
  "window.startup": "welcome",         // or "continue", "blank"
  "window.restoreSession": true
}
```

```jsonc
[
  { "key": "ctrl-ö", "command": "terminal.toggle" },
  { "key": "cmd-d", "command": null }   // unbind
]
```

Unknown command ids get a "did you mean" hint. Coming from VS Code? ⇧⌘P → "Preferences:
Import VS Code Settings and Shortcuts" copies the settings that have an equivalent here and
your shortcuts (into a marked block of `keybindings.jsonc`, so importing again replaces it);
it reports what had no equivalent. Your VS Code files are only read. Open tabs, cursors and unsaved changes are
restored on the next start (hot exit).

## Interface language

The interface is English; **Turkish (`tr`)** is the first translation. The setting `"ui.language"` (your own `settings.jsonc` only,
a project file cannot change it; also on the Settings page) takes `"auto"` (default), `"en"` or `"tr"`:

- `auto` follows the system language: on macOS the first supported language of System Settings ▸ Language & Region, on Windows
  the user's locale, elsewhere `LC_ALL`, `LC_MESSAGES`, `LANG` and `LANGUAGE` in that order. A system language without a translation
  (or no information at all) gives English. Note: `editor.caseLocale` is a separate setting (it only affects the case commands).
- The language is applied when the settings load and **at once when you change the setting**, no restart: the macOS menu bar and Dock
  menu are rebuilt, the other surfaces read the language every time they are drawn. A palette or dialog that is already open keeps
  the language it was opened with.

**What is translated.** Everything the program itself writes: the application menus (macOS menu bar and Dock menu, the in-window menu bar
on Windows and Linux), every command in the palette, the Welcome screen, the status bar, the editor and its context menus, the find
bar and popups, Source Control and its menus, the Explorer, Search, Problems and the bottom panels, the pickers, the Settings and
Keyboard Shortcuts pages (including the description of every setting), prompts, notices and error messages, confirmation dialogs
(unsaved changes, delete, replace in files, folder trust, transfers, Git), SSH hosts and port forwarding, the debugger, `.http`
requests, GitHub pull requests and the About dialog. The palette finds a command by its translated **or** its English label
(`file: new file` finds "Dosya: Yeni Dosya…"; Turkish letters match their plain forms, `gorunum` finds "Görünüm").

**What stays English**, on purpose:

- command ids, key bindings, setting names and the values in `settings.jsonc`, and everything that is data: file, folder, branch and
  author names, commit messages, language names, shell commands in install hints (`npm i -g …`) and task labels (`test: name`), which
  double as identifiers;
- text written by other programs and shown inside a Codeyard message unchanged: `git`, `ssh`, `curl`, language servers, debug adapters,
  the operating system; HTTP reason phrases ("Not Found"); the lines in the Output panel; the crash report file;
- the Alt+letter menu shortcuts on Windows and Linux (Alt+F opens "Dosya") and the items the system adds to its own menus
  (macOS Services, Window and Edit extras), which follow the system language;
- the documentation.

Warnings about the settings file are produced while it loads, so right after a language change the first reload may still show them in
the previous language. A string that bypasses the translation system is a bug: a test fails when user-visible text is not wrapped, so new
text arrives translated (see [translating.md](translating.md)).

**Adding or improving a language.** Translations are plain text files compiled into the binary (`crates/app/locales/<code>.tsv`, no new
dependency). The format, plurals, quality bar, the template file and the commands that show coverage are in
[translating.md](translating.md). A missing entry is never an error at run time: that string simply stays English.

## Updates

Codeyard can tell you when a new version is out and install it for you. It never looks on its own unless you said so.

![The update dialog, with made-up release notes for a version that does not exist](screenshots/update-available.png)

**What is checked.** A check downloads two small files from the project's GitHub releases: the checksum list `SHA256SUMS.txt` and its
signature. github.com answers each request with a redirect to GitHub's release download host (`release-assets.githubusercontent.com`).
When the list names a newer version a third file follows, the release notes, and the installer when you choose to install. GitHub
sees your IP address and that the request comes from `curl`. No cookies are kept, no account or identifier is in the address, and
nothing about you, your files or your machine is sent beyond what any web request carries.

**What it does not do.** Codeyard never installs anything without your confirmation, and an update that Codeyard installs does not go
through macOS's first-launch check (Gatekeeper), because the files come from `curl` and Codeyard builds are not notarized yet. What
protects you instead is the signed checksum list (see *How it is verified*).

**When.** The setting `"update.check"` (your own `settings.jsonc` or the Settings page; a folder's `.codeyard/settings.jsonc` cannot
set it) takes:

| Value | What happens |
| --- | --- |
| `ask` (default) | Nothing is checked. A few seconds after the first window opens, once, Codeyard asks whether to check daily. *Check Daily* and *Only When I Ask* write `daily` or `manual` for you; closing the question with Esc writes nothing and asks again after 30 days. |
| `daily` | Looks when Codeyard starts and then once in 24 hours while it runs (one successful check a day). If a check fails, for instance offline, it is tried again every three hours while Codeyard runs, but after three failed attempts in a day it waits for the next day. A newer version shows one notice and an item at the right end of the status bar; a failed check is only logged to stderr. |
| `manual` | Only **Help ▸ Check for Updates…** (palette: *Help: Check for Updates…*; on macOS also under the *codeyard* menu) looks. |

That command works whatever the setting says, and shows its errors. Debug builds and windows started with `--do` or `--panel` never
ask and never check by themselves.

**The dialog.** On macOS it is a window of its own, titled *Software Update*, with the app's icon, like the update windows of other Mac
apps (other systems draw it as a card over the editor). Closing it with the red button, ⌘W or Esc does what *Later* does. *Checking*, then either "Codeyard is up to date" or the new version with its release notes. *Install and Restart*
downloads the installer (progress in megabytes), checks it, replaces the app and offers *Restart Now* (*What comes back* below). *Later* closes the dialog and keeps the item
in the status bar. *Skip This Version* hides that version from the notice and the item for good; a manual check still offers it and a
newer version is not skipped. *Release Page* opens the release on GitHub. Esc or *Hide* during a download closes the dialog and the
download goes on ("Updating… 40%" in the status bar; click it to come back); *Cancel* stops it and removes the partial file. Quitting
during a download cancels it; during the few seconds the app is being replaced (in any window) you are asked before quitting, and that
window does not close. The notes are reached with Tab and scroll with the arrow keys, Page Up/Down, Home and End (a screen reader is
given their text, the first 8 KB of it; Codeyard draws at most the first 64 KB of very long notes and points to GitHub for the rest).
Closing the dialog of a failed update dismisses the failure and its red status bar item.

**What comes back after the restart.** Every window that has a folder open is reopened, on the same computer or SSH host, and the one that
was in front is in front again; it does not matter what `window.startup` says, because a restart is not a launch. Windows that showed the
start page stay away. What a window shows again is the same as after any quit: its tabs, cursors and layout when `window.restoreSession`
is on, and the edits you had not saved when `files.hotExit` is on as well (with hot exit off you are asked before a restart that would
lose edits). The dialog lists only what will happen with your settings. A window on an SSH host reconnects like *continue* does, which
can ask for a password again; terminals and running programs do not survive a restart, and a folder that has been deleted is reported
instead of reopened. Codeyard leaves the list of windows in a small file in its state folder just before it quits; the new version reads it
once, only for ten minutes and only when it is the version that was just installed, and deletes it, so nothing is left to repeat the
reopening later (details: [architecture.md](architecture.md#updates)). On Windows the installer starts the new version with the
same flag; that has not been tried there yet.

**How it is verified.** The checksum list must carry a valid Ed25519 signature made with the key built into Codeyard before anything in
it is read; the release notes and the installer must match their signed SHA-256 (the installer is hashed again right before it is
mounted), and on macOS the new app must have the same identifier and a valid code signature before it replaces the old one (an atomic
exchange; where the file system cannot do that, two renames that are undone on failure). A release without a signature, like v0.1.0, is
refused with "this release isn't signed". The signature has no expiry: whoever controls a connection can show it an older signed
release, and Codeyard cannot tell that apart from the truth the first time. It does remember the highest version it has been offered
and ignores an older list afterwards (see [SECURITY.md](../SECURITY.md#updates-and-signatures)).
If a check fails, nothing was installed: the dialog says so and offers the release page (details: [architecture.md](architecture.md#updates)).
Leftovers of an update that was interrupted (`.Codeyard.update-*`, `.Codeyard.old-*` next to the app, old downloads) are cleaned up at the next start.

**Copies that cannot update themselves.** A debug build, a bare binary, Linux, the Windows portable ZIP and a macOS app in a folder you
cannot write to (an administrator's `/Applications`) show *Download Page* instead of *Install and Restart*, and say which of these it is.
A release that has no installer for your system (a Windows-only release on a Mac, say) is offered the same way.
Windows installations made by the setup program run the installer silently and restart.

## Themes

Readability is built in: every text, status and syntax color is checked against every surface it can sit on (WCAG AA,
103 pairs); a built-in or imported VS Code theme that falls short is repaired when it loads by moving only the failing
foreground colors the smallest step toward black or white. Theme files you wrote yourself and your own
`workbench.colorCustomizations` are applied as written.

`⌘K ⌘T` (Ctrl+K Ctrl+T on Linux/Windows) opens the theme picker; arrow keys preview live,
Enter keeps the theme, Esc goes back. "Auto" follows the system light/dark setting.

Built in: Codeyard Dark and Codeyard Light (the defaults, following the system appearance), Studio Dark, Midnight,
Graphite, Ember, Fjord, Quarry, Dracula, One Dark, Nord, Solarized Dark, Solarized Light, GitHub Light, Daylight, Paper,
High Contrast Dark and High Contrast Light. Every built-in theme passes WCAG AA for all text and UI pairs
(`cargo test -p theme`).

Your own themes go in `~/.config/codeyard/themes/*.json`:

```json
{ "name": "My Theme", "appearance": "dark", "color": { "bg": "#101418", "accent": "#e0a050" } }
```

Missing roles fall back to the default dark or light theme. The full role list is in
`crates/theme/design/tokens.json`.

**VS Code themes:** drop the theme's `.json` file (from its extension's `themes/` folder)
into the same directory. Workbench colors and syntax colors (TextMate scopes) are mapped
to the editor's roles; comments and trailing commas are fine.

## Git

Uses the `git` on the machine where the files are (locally or on the SSH host), so it
behaves exactly like your terminal. Changed files are colored in the tree (M, U, A, D,
! for conflicts), changed lines get a bar in the gutter, and the branch is in the status bar.
`⌃⇧G` opens Source Control: stage (+), unstage (−), discard (↺, asks first; it deletes
untracked files), type a message and press ↵ to commit. The button says what it commits:
"Commit (2)" for the staged files, or "Commit All (3)" (tracked files only) when nothing is
staged. The panel also shows the last commit and who the commits will be made as
(`user.name <user.email>` of that host's git); if the identity is missing it says so, and a
refused commit explains how to set it. Clicking a file opens a side-by-side diff; F7 jumps
between changes. Commits made in the terminal show up on their own.

**Graph:** the GRAPH section under the changes draws the history of every branch as lanes,
with branch (blue), remote (green), tag (yellow) and HEAD labels; merges are hollow dots.
Click a commit to list the files it changed and open one as a side-by-side diff.

The "…" menu in Source Control (also in the palette) has commit (amend), undo last commit
(the changes stay staged and the message comes back), stash / stash with untracked files,
pop / apply / drop a stash, and discard all changes. "Git: View File History" lists the
commits of the open file (following renames); pick one to see what it changed. "Git: Blame
Line" tells who last changed the line under the cursor, also when the file has unsaved edits.

**Merge conflicts:** conflicted files are listed under "Merge Changes". In the editor the two
sides are tinted and the `<<<<<<<` line has "Accept Current | Accept Incoming | Accept Both";
the palette has accept all and next/previous conflict, and the status bar counts them.
Staging a file (marking it resolved) is refused while it still has conflict markers.

Click the branch in the status bar to switch or create branches; the arrows next to it show
how far you are ahead/behind and sync (pull, then push; fetch when nothing is pending). Git
never asks for passwords in the app: if the server needs credentials, run the command once in
the terminal or use an SSH key.

## Projects, favorites, bookmarks, tasks

`⌃R` opens the **workspace switcher**, the one place for opening and connecting: saved projects first
("Projects: Add Folder to Projects…", stored in `~/.config/codeyard/projects.jsonc`), then recent folders
(local or SSH, with how long ago), your SSH hosts with their live state (● connected, ✖ failed and why),
and the actions Open Folder, Connect to SSH host (type a name or `user@host`), Clone Repository and
containers. While connected it starts with **Disconnect**, after a lost connection with **Reconnect**. The status-bar
connection indicator, "Connect to SSH Host…" in the menus, the Welcome screen and the command palette ("Remote:
Connect to Host or Switch Workspace…") all open this same switcher; there is no separate hosts panel.
A folder that is already open in another window brings that window to the front. Windows reopen at their last
size and place.

Star files and folders ("Add to Favorites" in the tree's menu) to keep them in the Favorites
section of the explorer and at the top of ⌘P. Bookmarks (⌥⌘K) mark lines; "Bookmarks: List…"
shows them across the project.

"Tasks: Run Task" finds `.codeyard/tasks.jsonc`, `.vscode/tasks.json`, package.json scripts
(npm, pnpm, yarn or bun, by lock file), Makefile targets, Cargo and Go; each runs in its own
terminal. ⇧⌘B runs the first task with "build" in its name.

Compiler messages in a task's output (gcc / clang, Go, javac, rustc / cargo, tsc / MSBuild, eslint
stylish) go to the Problems panel and underline the line in the editor, like a VS Code problem
matcher without any setup; running another task clears them. Click a problem to jump to it.

## Highlighting, diffs and crash reports

Syntax colors come from tree-sitter for Rust, JavaScript / TypeScript (and JSX / TSX), Python, Go, C, C++,
Java, C#, Ruby, PHP, Swift, Kotlin, SQL, Lua, Shell, Dockerfile, Makefile, CSS, HTML, JSON, TOML, YAML and
Markdown; "Editor: Change Language Mode" sets it by hand for one file. The diff view marks the characters that
changed inside a changed line.

If Codeyard panics it writes a report to `crashes/` next to its state database
(`~/.local/state/codeyard` or `~/Library/Application Support/codeyard`, or `CODEYARD_STATE_DIR`). Nothing is
sent anywhere: attach the file to a bug report yourself.

Over SSH, "File: Save a Copy to This Computer…" saves the open remote file to a path on your own machine.

## Debugging

F5 debugs the project: a Cargo crate (`cargo build`, then `target/debug/<name>`), a Go module
(`dlv dap`) or the open Python file (`python3 -m debugpy.adapter`) with no setup; C, C++, Rust and
Swift use `lldb-dap` (from PATH or Xcode via `xcrun`). Add `.vscode/launch.json` (or
`.codeyard/launch.jsonc`) for anything else: `type` `lldb` / `go` / `python`, `program`, `args`,
`cwd`, `env`, `${workspaceFolder}` / `${file}`, `preLaunchTask` (a task from tasks.json, run first), or
`"debugAdapter": { "command": "…", "args": [] }` to use your own adapter. Click the gutter or press F9
for a breakpoint; the file opens on the paused line, the DEBUG panel lists the call stack, the
variables (click to expand) and a console that evaluates expressions; `Debug: Add Watch Expression…` keeps expressions evaluated at
every stop, hovering an expression shows its value while paused, `Debug: Break When Value Changes…` stops the
program whenever a variable is written, `Debug: Break on Exceptions…` toggles
the adapter's exception filters, and several threads get a list; local variables also appear at
the end of the lines that use them. `Debug: Edit Breakpoint…` makes a conditional breakpoint (`i == 3`),
a logpoint (`log: i is {i}`) or a hit-count one (`hit: >= 5`). Node needs Microsoft's js-debug:
`Debug: Install JavaScript Debugger` fetches it once (VS Code's copy is used when installed).
Over SSH the adapter runs on the server; Go and Node adapters talk TCP and are reached through a
port forward on the shared SSH connection. `CODEYARD_DAP_TRACE=1` prints the protocol when a
configuration misbehaves.

To debug a program that is already running, start it with its debug server and attach:
`python -m debugpy --listen 5678 --wait-for-client app.py`, then a configuration
`{ "name": "Attach", "type": "python", "request": "attach", "connect": { "host": "127.0.0.1", "port": 5678 } }`
(for other servers: `"debugAdapter": { "host": "…", "port": … }`). Stopping the session only detaches.

## Workspaces with several folders

`File: Add Folder to Workspace…` adds a folder to the Explorer as its own section (up to 32 extra);
`Remove Folder from Workspace…` takes it out; a `.code-workspace` file (opened from the CLI, the
palette or `File: Open Workspace from File…`) adds all its folders. The folder list is remembered per
project. Go to file lists `folder › path` for the extra folders, Find in Files searches all of them,
every folder gets its own language server, tasks / tests / debugging / new terminals use the folder
of the active file, and Source Control shows the repository of the active file. Each extra folder's own settings
(`.vscode/settings.json`, `.codeyard/settings.jsonc`) apply to its files: indentation (`editor.tabSize`,
`editor.insertSpaces`), saving (`editor.formatOnSave`, `editor.codeActionsOnSave`,
`files.trimTrailingWhitespace`, `files.insertFinalNewline`), `editor.formatOnPaste` / `formatOnType`
and `editor.rulers`; the first folder's settings already merge into the global ones. Editing the file
re-reads it.

## Language servers

"Developer: Restart Language Servers" restarts them (after installing one, for example).
When a server asks something ("reload the project?"), its options appear in a picker; Esc
answers "none".
Errors, suggestions, hover and go to definition come from language servers that run next
to the files (on the SSH host for remote folders; only messages cross the network).
They start the first time you open a file of that language:

| Language | Default server | Install (example) |
| --- | --- | --- |
| Rust | `rust-analyzer` | `rustup component add rust-analyzer` |
| C / C++ | `clangd` | Xcode tools, `apt install clangd` |
| Go | `gopls` | `go install golang.org/x/tools/gopls@latest` |
| Python | `pyright-langserver` | `npm i -g pyright` |
| TypeScript / JavaScript | `typescript-language-server` | `npm i -g typescript-language-server typescript` |
| Java | `jdtls` | `brew install jdtls`, or unpack Eclipse JDT LS and put `jdtls` on PATH |
| C# | `csharp-ls` | `dotnet tool install -g csharp-ls` |
| PHP | `intelephense` | `npm i -g intelephense` |
| Ruby | `ruby-lsp` | `gem install ruby-lsp` |
| Kotlin | `kotlin-language-server` | `brew install kotlin-language-server` |
| Swift | `sourcekit-lsp` | comes with Xcode or a Swift toolchain |
| Lua | `lua-language-server` | `brew install lua-language-server` |
| Shell (`.sh`, `.bash`) | `bash-language-server` | `npm i -g bash-language-server` |
| HTML | `vscode-html-language-server` | `npm i -g vscode-langservers-extracted` |
| CSS / SCSS / Less | `vscode-css-language-server` | `npm i -g vscode-langservers-extracted` |

Commands are looked up on the `PATH` of the machine that holds the files (the SSH host for remote folders). If yours lives
elsewhere, set the full command, for example Swift through Xcode: `"swift": { "command": "xcrun", "args": ["sourcekit-lsp"] }`.
Folders you have not trusted start no server at all (a server can run the project's code); trusting the folder starts them.

JSON, YAML and TOML have **no** default server on purpose: [JSON and YAML schemas](#json-and-yaml-schemas) already check JSON and
YAML (a second server would show every problem twice), and the usual servers download schemas on their own, outside
`schema.download`. You can still switch one on with the keys `"json"`, `"yaml"` or `"toml"`, for example
`"toml": { "command": "taplo", "args": ["lsp", "stdio"] }`.

If a server isn't installed the status bar says so once; nothing else breaks. Change or
turn off servers in settings:

```jsonc
"lsp.servers": { "python": { "command": "pylsp" }, "go": null },
"lsp.enabled": true,
```

Besides errors, suggestions, hover and definition: rename (F2), references (⇧F12), symbols
in the file (⇧⌘O), quick fixes (⌘.), format (⇧⌥F, or `"editor.formatOnSave": true`) and
parameter hints while typing a call. `"editor.codeActionsOnSave": { "source.organizeImports":
"explicit" }` runs those actions on save, before formatting (each step gives up after 2 s and the
file is saved anyway). Invisible characters (zero-width spaces, bidi controls) and Cyrillic / Greek
letters inside Latin words are boxed (`"editor.unicodeHighlight.enabled"`), which exposes
"Trojan Source" tricks and look-alike identifiers.

## JSON and YAML schemas

JSON, JSONC and YAML files are checked against their JSON Schema without a language server (nothing to install, also over SSH):
problems appear in the Problems panel and as underlines, hovering a key shows its description, and suggestions offer the
property names and allowed values of the object you are in. The schema is found from, in order: `"$schema"` in a JSON document or a
`# yaml-language-server: $schema=<url or path>` line in YAML (relative paths work), your `json.schemas` / `yaml.schemas` settings,
then the [SchemaStore](https://www.schemastore.org) catalogue (`package.json`, `tsconfig.json`, GitHub workflows, docker-compose,
Kubernetes…).

```jsonc
"json.schemas": [{ "fileMatch": ["config/*.json"], "url": "./schemas/app.schema.json" }],
"yaml.schemas": { "https://json.schemastore.org/github-workflow.json": ".github/workflows/*.yml" },
"json.validate.enable": true,
"json.schemaDownload.enable": true   // false: only local files and what is already cached
```

Schemas from the internet are downloaded with `curl` and cached for a week (`~/Library/Caches/codeyard/schemas` on macOS);
**Schemas: Clear Download Cache** forgets them. The only request made without you naming a schema is the SchemaStore
catalogue; turn `json.schemaDownload.enable` off to stop all of them. A `$schema` that points to any host other than SchemaStore
asks first, once per host and session (`"http.confirmRequests"`, see [HTTP files](#http-files); declining is remembered until you
restart, and the file keeps a note that the schema was not loaded). Custom YAML tags (`!Ref`, `!reference`) are skipped.

## Colours

`#rgb`, `#rrggbb(aa)`, `rgb()`, `hsl()` (comma and space syntax, alpha) and, in CSS-like files, colour names get a swatch under
the literal. Click the swatch, or run **Editor: Pick Color…** at the cursor (with no colour there it inserts one), to open a picker:
saturation/value square, hue and alpha sliders, and HEX / RGB / HSL buttons that rewrite the literal in the other notation.
A change is written when you release the mouse and is one undo step.

## Spell check

Prose files (Markdown, text, reStructuredText, AsciiDoc, LaTeX) are checked everywhere except code, links and addresses; in code
files only comments are (`"spelling.checkComments": false` turns that off). Identifiers (`camelCase`, `snake_case`, acronyms,
anything with digits) and a built-in list of developer words are skipped. **Quick Fix (⌘.)** on an underlined word lists suggestions
(a capital letter is kept), *Add to the dictionary* (saved in `spelling.txt` next to `settings.jsonc`) and *Ignore in this session*.

English is built in. **Spelling: Install Dictionary…** downloads others (Turkish, German, French, Spanish, …; the picker shows each
dictionary's licence and size) from the Hunspell collection at github.com/wooorm/dictionaries, caches them and adds them to
`"spelling.languages": ["en", "tr"]`; a word is fine if any listed dictionary accepts it. **Spelling: Toggle Spell Check** and
`"spelling.enabled"` switch it off.

## Merge editor

**Merge Conflict: Open Merge Editor** (or the link on a conflict's `<<<<<<<` line) lists every conflict of the open file as a card:
Current and Incoming side by side (plus the common ancestor when the file has `diff3` markers) with two lines of context. Pick
*Accept Current*, *Accept Incoming*, *Accept Both* or *Both, Incoming First*; hovering an option previews the **Result**.
Each resolution is an undoable edit of the file itself (⌘Z works from the page); when none are left, *Save File* writes it.
Closing the file's tab closes the page; with very many conflicts the first 40 are shown and the header still counts all of them.

## GitHub pull requests

**GitHub: Pull Requests…** lists the open pull requests of the `origin` (or first GitHub) remote with author, branches, age, check
results and review state. Pick one to *check it out* (`git fetch origin pull/N/head` into the local branch `pr-N`, again to update it),
open it on GitHub, or copy its link or branch name. It uses the `gh` CLI when it is installed and signed in (private repositories,
checks, reviews); otherwise the public GitHub API, with `GITHUB_TOKEN` / `GH_TOKEN` if set. Read-only: reviewing and commenting stay on GitHub.
Lists show the latest 50 open pull requests. Errors say what to do (sign in with `gh auth login`, commit or stash local changes, check the remote); calls time out after 30 seconds and never wait for a prompt.

## HTTP files

In `.http` / `.rest` files, **HTTP: Send Request** (⌥⌘R, or the *Send Request* link at the end of each request line) sends the request
under the cursor with `curl` and opens the response body in a tab (JSON pretty-printed, highlighted by content type); the status line,
time and size appear in the status bar and **HTTP: Show Last Response Headers** opens the headers.
Requests that read environment variables (`{{$processEnv}}`) or upload a file from outside the `.http` file's folder ask once per file
before sending. **Where it goes is asked too:** `"http.confirmRequests"` is `"remote"` by default, which asks before the first
request to each host that is not on this computer (`localhost`, `127.x.x.x`, `::1`) in a session; the host is read from the address
after variables are filled in, so `{{base}}` cannot hide it, and an address whose host cannot be read reliably (a backslash in front of
`@`) always asks. Choose `"never"` for the old behaviour (no question) or `"always"` to confirm every request; only your own settings file can
set it (a folder's `.codeyard/settings.jsonc` is ignored for this key). Redirects are
not followed: a 3xx answer is shown with its `Location` in the status bar, because the next address was never approved and would
receive the method, body and headers again; send a new request to it if you want to go there. Connection problems are explained with the machine the request ran on (`http.runOn`: `workspace` or `local`);
responses over 20 MB are refused, and binary bodies open as a hex dump.

```http
@host = http://localhost:3000

### log in
# @name login
POST {{host}}/login
Content-Type: application/json

{ "user": "ada", "password": "{{$processEnv ADA_PASSWORD}}" }

### who am I
GET {{host}}/me
Authorization: Bearer {{login.response.body.$.token}}
X-Request-Id: {{$guid}}
```

Variables: `@name = value`, `{{$guid}}`, `{{$timestamp}}`, `{{$randomInt min max}}`, `{{$datetime iso8601}}`, `{{$processEnv NAME}}`, and
`{{name.response.body.$.path}}` / `{{name.response.headers.Name}}` from a request marked `# @name name`. A body can come from a file
(`< ./payload.json`), and `Authorization: Basic user:password` is encoded for you. While connected over SSH the request is sent **from the
server** (so `localhost` services work); `"http.runOn": "local"` sends it from your computer instead.

## Port forwarding

While connected over SSH, the Ports tab in the bottom panel (⇧⌘P → "View: Ports") lists
what is listening on the server. Ports that start listening after you connect (a dev server
on :5173, say) are forwarded automatically and announced in the status bar; ports that were
already open (databases, ssh) are only listed, with a Forward button. Type a number to
forward any port. If the same port is busy on your machine, another free one is used.
Forwarding rides on the existing ssh connection (no extra processes) and is removed when
you quit. Turn off the automatic part with `"ports.autoForward": false`.

## Remote (SSH)

Uses your system `ssh` and `~/.ssh/config`. Pick a host in the workspace switcher (⌃R) or type a name.
The last remote folder only opens again at start if `"window.startup"` is `"continue"`. After connecting, a folder prompt opens (like VS Code):
type a path (`~` is your home folder), pick sub-folders from the list to step into them, and press
Enter on the first row to open the typed path. Esc keeps the home folder. `⌘O` opens the same prompt
on a remote host.

On first connect a static agent (about 1.5 MB) is
copied to `~/.cache/codeyard/` on the server (SHA-256 checked). Passwords, key passphrases,
one-time codes and unknown host keys are asked in the window (the app acts as `SSH_ASKPASS`
over a private socket; nothing is written to disk or the environment).

If the connection drops, the agent keeps running on the server for 5 minutes: the app
reconnects on its own (status bar shows "Reconnecting") and terminals and language servers continue where they were.
If it cannot reconnect, a message gives the reason and a **Reconnect** button; your open files and unsaved changes
stay in their tabs (saving waits for the connection; *File: Save a Copy to This Computer…* keeps a local copy).
A file that is deleted or changed on the server while you edit it shows a banner instead of being overwritten.

Search and terminals run on the server through the agent: only results and terminal
output cross the network.

How the connection, the agent and reconnecting work is described in [remote.md](remote.md).

### Manage SSH hosts and move files

- **Remote: Add SSH Host…** takes `user@host`, `user@host:2222` or `user@host as name` and appends a `Host` block to
  `~/.ssh/config` (nothing else in the file is touched; the previous file is kept as `config.codeyard-bak`). It then offers to connect.
  **Remote: Remove SSH Host…**, **Remote: Test SSH Connection…** (reachable? which architecture? is a matching agent bundled?) and
  **Remote: Edit SSH Config** complete the set. The workspace switcher has an "Add SSH host…" row too.
- **Download / upload** over the open ssh connection: right-click a file or folder in the explorer and choose *Download…*;
  choose *Upload Files Here…* on a folder, run **Remote: Upload Files to This Folder…**, or just drag files from Finder onto the explorer
  (in a local workspace the same drop copies them in). Transfers stream (no size limit), folders travel as `tar`, progress shows in the
  status bar (click it to cancel; a cancelled or failed upload leaves no partial file). Existing names ask before being replaced.
- **Follow (Tail)**: *File: Follow (Tail) Active File*, or *Follow (Tail)* in the explorer and tab menus, opens the file in a read-only
  view that sticks to the end while the file grows (`tail -f`). It works for any size, survives log rotation, colours `ERROR`/`WARN`/`DEBUG`
  lines, and starts on its own for `.log` files (and `/var/log`) that are too large to edit. Scrolling up pauses it and a
  "↓ N new lines" badge counts what arrived; click it, press End or ⌘↓ to catch up. **File: Filter Log Lines…** runs `grep` on the host
  (the last 256 MB, at most 5000 lines, case-insensitive regular expression) and shows the matches in a tab.
- **Edit and secure hosts**: **Remote: Edit SSH Host…** changes `HostName`, `User`, `Port`, `IdentityFile` or `ProxyJump` of one host block
  (values are validated; other lines, comments and blocks stay as they are). **Remote: Set Up Key Login…** finds or creates
  `~/.ssh/id_ed25519`, copies the public key to the server's `authorized_keys` (once, over the connection, asking for the password in the
  window if needed) so later connections need no password. **Remote: Manage SSH Hosts…** (also a row in the workspace switcher) lists all of these.
