# Accessibility (implementation)

The public statement (support, limitations, how to report a barrier) is [ACCESSIBILITY.md](../ACCESSIBILITY.md).
This page is the technical detail behind it.

GPUI 0.2.2 has no accessibility tree of its own. Codeyard adds one with AccessKit (`crates/app/src/a11y.rs`,
`accesskit 0.25`): `accesskit_macos 0.27` on macOS (NSAccessibility, for VoiceOver) and `accesskit_windows 0.35` on
Windows (UI Automation, for NVDA, JAWS and Narrator). No GPUI patch is needed. No screen reader has been run against
either yet (see "Not verified").

## What exists

The tree, in the order a screen reader walks it:

- `Window` (title "Codeyard — folder").
- The window's chrome (`a11y_chrome.rs`): on macOS the title bar's Back and Forward buttons (disabled when there is
  nowhere to go) and the command center (its label, Go to File… as description; `Click` opens it); on Windows the
  caption buttons the app draws (Minimize, Maximize or Restore, Close, which asks about unsaved changes as the system
  close does); while the activity bar is shown, a `TabList` "Views" (Explorer, Search, Source Control, Run and Debug,
  Testing, Remote Explorer, Extensions, Containers; the badge as description, the view shown selected; `Click` shows
  or hides it as a click does, only from the state the tree showed) and the Manage button (menu popup). While Remote
  Explorer is shown, a `Toolbar` with Add SSH Host…, Open SSH Config File and Refresh. Each has its painted frame.
- Painted controls (`a11y_controls.rs`): a view declares a control where it draws it (`a11y_controls::control`), with
  its role, name, state, focus and the function its click runs; the tree lists the controls laid out in the window's
  last frame, gathered in a `Group` per surface, each with its painted frame, `Click` (run after the request, so it
  may update any view) and `Focus` when it can take the keyboard. A press from an outdated tree acts only while the
  same control with the same name is still drawn. Surfaces declared this way: the status bar's other items (file
  operation progress, Sync, ports, Extensions off, transfer, language server progress, saving and unsaved marks, the
  terminal toggle, merge conflicts, a data path, the web preview's Open and Stop) joined to the status bar's node; the
  resource monitor (its "Resources" button, expanded while the figures show, each figure as a label, Pin and Close);
  the error card's Reconnect and Close; the containers' "Back to SSH workspace" bar; the breadcrumbs (`Link`s that
  show the folder or file in the Explorer); the menu bar the app draws on Windows and Linux (`MenuItem`s that open
  their menu under them); the panel's buttons (New Terminal, Launch Profile…, Split Terminal, Kill Terminal, Move
  Panel, Maximize or Restore, Hide Panel) and its list of terminals ("Terminals": a `Tab` each, selected while in
  front, with Kill Terminal); the editor tabs' buttons ("Tab actions": Close, Unpin, Read, Preview HTML, with the
  tab's name as description); the large-file, diff and page tabs (Settings, Welcome, Merge, Compare, Markdown,
  Containers, the Inspector, Language Services…) as tabs of the "Editor tabs" list with their Close buttons; and
  every other editor group's tabs in a group "Editor group 2", whose `Click` brings that group forward with the tab.
- The side views' drawn controls, declared the same way: the Explorer's empty-state buttons (Open Folder, Open
  recent), header buttons (New File, New Folder, Refresh, Expand and Collapse Folders), each section's header (a
  `Button` with its count, expanded while open; `Click` folds it), the open editors (a `ListItem` each, selected while
  in front, with Close), favorites, the outline (`TreeItem`s with their line, selected at the caret) and the timeline
  (saved copies and commits), and its notes as labels; Source Control's Refresh and More Actions…, the Stashes header
  and each stash's Apply, Pop and Drop (the stash as description), a repository's fold button; the Search view's
  Clear field, Previous and Next result, Expand or Collapse all, Open in a Search Editor tab, Run again, Clear and, in
  structural search, the pattern's language (`Click` takes the next one); Run and Debug's Remove All Breakpoints and
  "Create a launch configuration" (`Link`); the Testing view's filter field (`TextInput` with its text, `Focus` gives
  it the keyboard) and Collapse All; the Remote Explorer's per-group Refresh, a recent folder's Open in New Window and
  Remove from Recent, a port's Open in Browser, Forward Port and Stop Forwarding (the folder or port as description);
  the Extensions header (Install from File…, Install from Folder…, Refresh), the Market page's Show Installed; and an
  installed extension's page in a group named after it: its heading, source, state, description, contributions and
  permissions as labels, Enable or Disable, Uninstall…, Roll Back, Verify Files, Get or Stop Pre-releases, the update
  card's button, the permission switches (`Switch`, toggled while on), each recent denial with Copy fix, the crash
  card's buttons and the accounts' Sign Out, Remove and Add Instance…. A button drawn only on hover is in the tree
  while its row is laid out.
- The pages, declared the same way, each in a group named after it: the merge editor (its title as a heading, each
  conflict as a heading with its lines, each side as a `Document` whose value is the side's text, Accept Current,
  Accept Incoming, Accept Both and Both, Incoming First with the conflict as description, Accept All, Save File and
  Close); the folder comparison (the option chips as `CheckBox`es, Stop, Scan Again, Copy Report and Export, each row
  a `TreeItem` with both sides and its status as description, expanded for a folder, with Copy to the right or left
  and Delete from either side); the diff (a heading with the comparison and counts, Toggle inline view, every line
  drawn as a `ListItem` "Added, line 3: text", side by side or inline); the large-file view (its header, every line
  drawn as a `ListItem` "line 12: text", selected when it is the found line, Follow as a `CheckBox`, the new-lines
  button, the find field as a `SearchInput` that takes the keyboard and its count); the Markdown preview (each
  block as a `Heading` or `Paragraph` with its text, each link as a `Link` with its address that opens it as a click
  does, and Code); the Welcome page (its tiles with their shortcuts, each recent folder as a `ListItem` with Pin and
  Remove from Recent, More…); the data query bar (its field, status and Close); and the containers' pages (the
  navigation, filters, Clear filters, Needs attention, Columns and each column, the sort headers, each container as a
  `ListItem` with its image, status and usage, Logs and Actions, the search field, the detail tabs, Open files,
  Terminal, Refresh details, each value's Copy with the value as description, the log lines and the log buttons).
- The editor's own drawn controls, in a group named after the file: each breakpoint in the gutter ("Breakpoint on line
  2", its condition, log message, binding failure or "disabled" as description; `Click` removes it as a click on the
  dot does, F9 sets one), each fold button ("Fold line 1" or "Unfold line 1", expanded while open), each Git mark
  ("Line 3 changed", "Line 4 added", "Lines deleted above line 7"), an extension's lens at the end of a line, the
  sticky scope lines (`Link`s to their line), a merge conflict's Accept Current, Incoming and Both Changes and Open
  Merge Editor (`Link`s with "Conflict at line 2") and an `.http` request's Send Request. The color picker is a group
  "Color picker": Hue (0–359), Saturation, Brightness and Opacity (0–100 %) as `Slider`s over the planes a drag sets
  them in (Increment and Decrement step by 5, a value set from the tree is taken as typed; each change is written to
  the file as a release of the mouse writes it), the color's text and HEX, RGB and HSL as `RadioButton`s. The
  terminal's find bar is a group "Find in Terminal": the field (`SearchInput` with its text, `Focus` gives it the
  keyboard), the match count as a status, Previous Match, Next Match and Close.
- Controls' groups come after the views and the editor and before the status bar and the overlays, so AccessKit's hit
  test finds a control drawn over the editor (a gutter mark, a link at the end of a line) rather than the editor.
- One `Tree` per folder of the workspace: "Explorer", or "Explorer: name" when there are several. One `TreeItem` per
  row in a range of 1,000 rows: name, level, selected, expanded, set position in the whole tree. The range is centred
  on the selection while it is on screen, otherwise on the visible rows; the tree reports its scroll position in rows
  and supports scroll up/down by a page, which moves the range. These trees are in the tree only while a side bar (or a window of its own) shows the Explorer: another view, a hidden side bar or zen mode takes them out, and focus, scroll and row actions from an older tree then do nothing. Row actions resolve the current path rather than reusing an old row index.
- While Source Control is visible, a `Group` with the branch, a writable `MultilineTextInput` for the commit message
  and the commit button, labeled with what it commits ("Commit (2)", "Commit All (3)") and disabled with "Nothing to
  commit" as its description when there is nothing. Then a `List` "Changed files" of the change groups: each group a
  `Group` with a header button ("Changes (12)", expanded or collapsed), each file a button that opens its diff (its
  change, such as "Modified", as the description; its place in the group as set position and size) followed by a
  "Stage …"/"Unstage …" button ("Mark … as resolved" for a conflict) and, for an unstaged change, "Discard changes in …",
  which asks the same question as the painted button. The list exposes 500 lines (headers and files) around the
  keyboard cursor while it is on screen, otherwise around the visible rows; it reports its scroll position in lines and
  supports scroll up/down by a page. Operation errors are assertive live regions; successful results are polite. The
  focus is exposed for the message field, the repository list and the change list, whose cursor row is the focused
  node; `Focus` on a row puts the keyboard cursor there.
- Source Control's history (`a11y_scm_history.rs`): after the commit button, the "Sync Changes 1↑" / "Publish Branch"
  button (busy while a sync runs; `Click` only while it still says the same). After the change list, a `Group`
  "Worktrees": its header button ("Worktrees (3)", expanded or collapsed), Add worktree, Refresh, a polite `Status`
  (loading, none found) or an assertive `Alert` (the error), and a `List` of checkouts (branch as label; path and
  Current / Unavailable / locked as description; the current one selected; `Click` opens an available one in this
  window as a click does, the others are disabled). Then a `Group` "Graph": its header ("Graph (300)"), the Search
  commits toggle, Fetch from all remotes, Pull and Push (the number of commits to pull or push as description),
  Reload history (busy while history loads), More Actions… (the graph's menu, opened under the button), the active
  filters as buttons that remove them, the commit search field (`SearchInput`, `Focus`, `SetValue` filters as typing
  does) and a `List` "Commits" of up to 500 rows around the selected commit (or the rows on screen): day headings as
  `Heading`s and commits as `ListItem`s (subject; author, time, hash, branches and tags as description; the selected
  one selected; `Click` selects it as a click does, `ShowContextMenu` and a "More Actions…" button open the commit's
  menu under the row, "Open Changes" opens the commit as a double click does). The selected commit's details are a
  `Group` "Commit 1a2b3c4" with its header, Close details, the message lines, author and hash, and a `List` of its files
  (path; Added, Modified… as description; `Click` opens the file's diff in that commit). Every node has its painted
  frame. Actions name the commit hash, worktree path, file of that commit or label the tree showed: from an outdated
  tree they do nothing when it changed, and a header or the search toggle only acts from the state that was read.
- While the Problems panel is open, a `List` labeled with the number of problems: first a polite `Status` with the
  counts ("2 errors, 1 warnings", spoken when it changes), then a `Heading` per file and a `ListItem` per problem
  ("Error: …", with file, line and column as its description) or related location, each with `Click` to open it and its
  painted frame. A problem holds a "Quick Fix…" `Button` (a list popup): it opens the problem at its place and asks for
  its fixes there, as Quick Fix at the caret does; they come as the picker below, whose option applies the fix, and the
  keyboard is back in the editor afterwards. At most 2,000 rows are exposed. With nothing to show, a polite `Status`
  says "No problems.". An activation from an outdated tree opens or fixes nothing unless the same row is still at the
  same place.
- While the bottom panel is open, a `TabList` with its tabs (Problems, Output, Debug Console while debugging, Ports,
  Terminal), the selected one marked and counts as descriptions; `Click` switches tabs. While Output is shown, a polite
  `Log` holds its last 400 lines. While the Debug Console is shown (`a11y_console.rs`), a `Group` "Debug Console" (the
  session's name as description) with a polite `Log` "Output of app" (its last 300 lines, so new output is spoken),
  the expression field (`TextInput`, `Focus`, `SetValue`) and an "Evaluate" `Button` that evaluates the field's text
  as Enter does and empties it (disabled while the field is empty; it acts only on the text and session the tree
  showed). The button exists only in the tree: it has no control of its own on screen, so no frame. Without a session,
  a polite `Status` says "No debug session. F5 starts one."
- While the Search view is shown (in a side bar or a window of its own), a `Group` "Search": the query as a
  `SearchInput` ("Structural pattern" in structural mode; `Focus`, and `SetValue`, which searches as typing does), its
  options as `CheckBox`es (Match case, Whole word, Regular expression, Preserve case when replacing, Structural search;
  `Click` toggles from the state the tree showed), the replacement (`TextInput`), Replace All (disabled until there are
  results; it opens the review as the button does), the "Files to include / exclude" disclosure `Button` (expanded
  while the two fields show) and those fields, the status line as a polite `Status` and a search or pattern error as an
  assertive `Alert`, then a `Tree` "Search results" of up to 500 rows around the current match (or the rows on screen;
  scroll up/down by 100 rows moves the range): a file per level-1 `TreeItem` (folder as description, match count as
  value, `Expand`/`Collapse`/`Click`, with "Replace in this file" and "Dismiss this file from the results") and its
  matches at level 2 (line text, "line 2, column 9", the current one selected; `Click` opens it). Fields, options,
  buttons and rows have their painted frames. Rows are named by file, line and column, so an action from an outdated
  tree acts only on what is still a result. The focused field is the tree's focus.
- While the Ports tab of the bottom panel is shown, a `List` "Ports": the `TextInput` "Forward a port:" (`Focus`), and a
  `ListItem` per row ("Port 8000, label"; the forwarded address or the state as its value; process and origin as its
  description) with the row's buttons: Open in Browser, Copy Address, Stop Forwarding, or Forward Port for a port that is
  only listening. At most 500 rows. Without an SSH connection or with nothing listening, a polite `Status` says so. A
  button acts only while the same server port still shows the same forward (and, for Open and Copy, the same address).
- The Settings page in front is a `Group`: its `SearchInput` (`Focus`, `SetValue`), a `TabList` of categories with
  their counts (`Click` selects, as on screen), and a `Group` named like the list's heading with up to 256 settings:
  title as label, description (with where the value comes from and a rejected value's error, marked invalid) and the
  value as a `CheckBox` (toggled), a `RadioGroup` of `RadioButton`s, a `TextInput` (`Focus`; `SetValue` is checked
  and written as Enter in the field is) or the theme `Button`. A changed setting adds its Reset button; the buttons
  "Open settings.jsonc", "Keyboard Shortcuts" and "File icon theme…" follow. Actions send the page's own events, so
  writing is the pointer's path; a switch from an outdated tree that showed another state, a setting no longer on the
  page or a closed or reopened page changes nothing. With no rows, a polite `Status` says why. A rejected value is
  also an assertive `Alert` ("Editor: Font Size: …"), so it is said as the field turns invalid; the keyboard stays in
  the field.
- The Keyboard Shortcuts page in front is a `Group` with its `SearchInput` and a `List` ("{n} commands") of up to
  2,000 rows: command as label, its key (or "No shortcut") as description, set position. `Click` runs the command as a
  click on the row does, only while the page still lists it. The page is read-only otherwise: keys are assigned in the
  "Preferences: Open Keyboard Shortcuts" picker, which is exposed as a picker.
- While Run and Debug is visible, a `Group` "Run and Debug": the "Start Debugging (F5)" `Button` (the configuration it
  starts as description), a `RadioGroup` of the launch configurations (`Click` selects, as the selector's menu does),
  the selector itself (a `Button` with the configuration's name, "Launch configuration" as description, menu popup:
  `Click` opens its menu under it),
  "Open Launch Configuration", a polite `Status` with the current session and its state ("app: paused on breakpoint",
  or "No debug session."), a `Tree` "Variables" (name as label, value as value, type as description, level for the
  nesting, `Expand`/`Collapse`/`Click` on a value with children, `Click` on "… n more"), a `List` "Watch" (expression
  and value, a Remove Expression button each) with "Add Expression…", a `Tree` "Call Stack" (sessions with their state
  when there are several, threads, frames with their place; the current ones selected; `Click` makes current, as a
  click on the row does) and a `List` "Breakpoints" of `CheckBox`es ("main.rs, line 3" or "name()", checked while
  enabled, condition and binding failure as description, a Remove Breakpoint button each and, for a line breakpoint, Go
  to Breakpoint, which opens its place as a click on the row does), then "Add Function
  Breakpoint…" and "Activate/Deactivate Breakpoints". At most 2,000 variables, frames and breakpoints. While a session
  is live (whichever view is shown), a `Toolbar` "Debug toolbar" with the floating toolbar's buttons: Continue or
  Pause, Step Over, Step Into, Step Out (disabled while not paused), Restart and Stop. Every action calls what the
  pointer calls. A toolbar button acts only while the session it showed is still the current one; a variable, frame,
  thread or breakpoint acts only while it is still listed as the tree showed it (an outdated toggle is not repeated);
  Start starts only the configuration the tree showed.
- While Testing is visible, a `Group` "Testing": Run All Tests (Stop while a run is going), Run Failed Tests, Refresh
  Tests, the `Switch` "Show Only Failed Tests", a polite `Status` with the view's summary ("1/2 passed · 1 failed") or
  why there are no tests, and a `Tree` of up to 5,000 rows: files (name, outcome as value, folder and test count as
  description, `Expand`/`Collapse`/`Click`, a Run Tests in File button), tests (outcome and time as value, Run Test
  and, except JavaScript, Debug Test buttons; `Click` opens the test) and failure lines (`Click` opens the failure).
  Rows are found again by file and test id, so a test that is gone or renamed runs and opens nothing. Running and
  debugging go through the pointer's path, with the same Workspace Trust prompt.
- While Remote Explorer is visible, a `Tree` "Remote Explorer" of up to 2,000 rows: groups (SSH Hosts, Containers,
  WSL Targets; expandable), hosts (`user@host` as description, the connection state as value: Connected,
  Connecting…, the failure, Connected before, Not connected yet; Connect in Current Window or Open Folder…, Connect in
  New Window and, while connected, Disconnect buttons), recent folders (`Click` opens; the current one selected),
  the forwarded ports. A host's buttons act only while the same host is listed in the same connected state.
- While a context menu is open (row, tab, editor, application and Manage menus), a `Menu` "Context menu" of
  `MenuItem`s (a `MenuItemCheckBox` for a toggle or choice, toggled when checked): label, shortcut as description,
  disabled items marked disabled, submenus marked as having a popup, set position. The highlighted item, else the
  menu, has the focus. `Click` chooses the item through the menu itself, exactly as a click on it does (the menu
  closes, focus returns, the command runs); it acts only on the same menu with the same item at that place. Escape
  is unchanged.
- Row menus (for assistive technology): an Explorer row, a Run and Debug variable (not a scope), call stack frame and
  line breakpoint, a Remote Explorer host, a Ports row and a file of Source Control's change list have the `ShowContextMenu` action and a "More Actions…" `Button` (a menu popup, the row's name as
  description) among their children. Both open the menu a right click on the row opens (Set Value…, Copy Value, Add to
  Watch; Restart Frame, Copy Call Stack; Edit Condition…, Disable Breakpoint; Connect, Edit Host…, Remove Host…;
  Change Local Port…, Protocol, Retry, Stop Forwarding; Open Changes, Open File, Stage or Unstage, Discard changes,
  Copy Path; the Explorer's New File…, Rename…, Delete and the rest; an Explorer row's menu is opened for that row,
  selected first, as Shift+F10 on it opens it), at the bottom left of the row as last painted (the middle of
  the window when the row is scrolled out of its list). The menu then takes the focus as described above and returns
  it when it closes. The macOS and Windows adapters do not forward `ShowContextMenu` yet, so the button is the way in
  for VoiceOver and Narrator today (with the keyboard alone, Shift+F10 on the focused row does the same). The request is refused unless the row is still the same variable, frame, breakpoint
  (in the same enabled state), host (in the same state) or port forward at that place, in the same debug session, or
  the same file of the same repository in the same group (a Source Control menu also puts the list's cursor on the file,
  so closing the menu leaves the keyboard on it).
- While the extension consent sheet is open and in front: a modal `AlertDialog` named "Install {name}?", described by where the
  package comes from, the High-risk line and the warnings, with one `Label` per line of the sheet (id, version and
  licence, source, description, permissions, the publisher's reason, contributions, warnings) and the Install and
  Cancel `Button`s (`Click`, `Focus`). The keyboard lands on Install, or on Cancel for a High-risk set or a sheet with
  warnings, and the focused button is the tree's focus. Escape answers Cancel. Escape, Cancel and Install give the
  keyboard back to whatever had it when the sheet opened. A button acts only on the package the tree showed (a newer
  sheet for another package is left alone).
- Confirmations (discard changes, delete files, close an unsaved editor, apply a settings change, uninstall an
  extension, remove a host and the rest) are system dialogs on macOS (a sheet) and Windows (a task dialog): the system
  gives them to its screen reader, with their title, text and buttons, and they are not part of this tree. The tree
  stays as it was while one is open. Cancel changes nothing and leaves the keyboard, and the tree's focus, where they
  were (the row or field the question came from); the confirming button acts and leaves the keyboard in the same view
  (on the list when its row is gone). A failure afterwards is an assertive `Alert`. Every question (34 of them) is
  built as data by `crate::questions` (`question::Question`: title, text, buttons in order, the Cancel button) and
  asked with `Question::ask`, which marks the Cancel button ("Cancel", "Not Now", "Keep", "Keep Waiting", "Keep Going",
  "Deny") as the platform's cancel button whatever the language, so Escape answers it on macOS and Windows (GPUI
  recognised only the English word "Cancel"; a translated or differently worded one had no Escape and, on Windows, no
  `IDCANCEL`). Return answers the first button; Discard puts Cancel first, so Return never discards. If something took
  the keyboard while the question was open, Cancel gives it back to what had it. `questions_tests.rs` checks every
  question's level, title, buttons, Cancel and Return without a window, an SSH configuration, Docker, a registry or
  the network, and that Cancel gives the keyboard back. The questions a test can reach in a real workbench are
  answered with Cancel through `question::cancel_keeps_focus`, which checks that the keyboard and the tree's focus are
  where they were: deleting from an Explorer row (also from the tree), a batch delete, a move by drag, deleting for
  good, Discard (from the tree), closing an unsaved editor (and its own window), closing the window or quitting with
  unsaved changes, restarting for an update with unsaved changes, quitting while an update installs, the Rust setup,
  uninstalling an extension, an extension's first use of a permission (Deny), its GitHub account and its OAuth
  sign-in, adding an account instance, Workspace Trust (tasks and tests), a forgotten recent folder, opening a Market
  link, a structural replace, copying and deleting in a folder comparison, a hard reset, a sync or other Git
  confirmation, a multi-line terminal paste, closing a terminal with a running program, sending an `.http` request
  and downloading a schema. The questions about `~/.ssh/config` (adding and removing a host, setting up key login),
  Docker containers, transfers over SSH and macOS Full Disk Access are checked as data only, since a test cannot
  touch the user's SSH configuration, Docker or the system settings.
- While Extensions is visible, a `Group` with a `TabList` (Installed, Market), a `SearchInput` that controls the list
  and points at the highlighted row as its active descendant, a polite live `Status` that says how many extensions
  match (an `Alert` when the list cannot be read), and a `ListBox` of the rows (name; version, id and description).
  A Market extension's page in front adds a `Group` with its lines (publisher, downloads, permissions, versions) and
  its Install button. Actions check that a row is still the same extension.
- `TabList` "Editor tabs": one `Tab` per editor tab of the focused group (file name, `(modified)`, selected for the tab
  in front, set position).
- Search Editor tabs are listed in the same `TabList`. The Search Editor in front is a `Group` with its title: a
  `SearchInput` with the query, a polite `Status` (result counts, incomplete or restored results), a `ListBox` "Search
  results" or "Replacement preview" whose options are a window of at most 200 rows around the keyboard row (set size
  and position give the full count; Click opens a match or toggles a file, Focus moves the keyboard row), and the
  "Apply N changes in M files" and "Undo Replace" buttons. Apply is disabled while the results are incomplete; row and
  Apply actions are refused if the row's label or the Apply label changed since the tree was read.
- While the find bar is open, a `Group` "Find": a `SearchInput` with the query, a `TextInput` "Replace" while the replace
  row shows, a `Status` with "3 of 12" (a polite live region, so it is spoken while typing and moving between matches),
  then the options as `CheckBox` (with their state) and the buttons, in the order they are drawn. Labels are the tooltips,
  shortcut included.
- `MultilineTextInput` for the active editor: file name, `(modified)`, a range of 300 lines (centred on the caret
  while it is on screen, otherwise on the visible lines; scroll up/down by a page moves it), its scroll position in
  lines and "N lines, showing lines A–B" as description, text runs of at most 200 characters, the primary selection. While suggestions are open it holds a `ListBox` "Suggestions" (one
  `ListBoxOption` per suggestion with its detail, at most 500) and the selected suggestion is its active descendant, so a
  screen reader speaks it as the selection moves while the keyboard stays in the editor. While the hover is shown, a
  polite `Tooltip` "Hover" under the editor holds a line per diagnostic ("Error: …"), related location (a `Link`;
  `Click` opens it as a click on it does, only while the same line is there), "(+3 more)" line, the language server's
  text and each extension's text; while signature help is shown, a polite `Tooltip` "Signature help" has the
  signature as value and "Current parameter: b: i32" as description. Both have the popup's painted frame. The editor
  supports `ShowTooltip` (the hover at the caret, as Editor: Show Hover) and, while one is shown, `HideTooltip` (both
  popups close), so the text is reachable without the pointer.
- While the terminal panel is open, a read-only `Terminal`: one text run per row on screen and the cursor as a collapsed
  selection.
- `Status` "Status bar": one item per entry; every entry that opens something on a click is a `Button` with the same
  action (Remote, Branch, Problems, position, indentation, line ending, language, update), the encoding a label;
  then the painted controls of the status bar (above).
- The notice in the status bar is a live region of its own: an error (including a file that could not be opened) is an
  `Alert` with `Live::Assertive`, so it interrupts speech; an information notice is a `Status` with `Live::Polite`.
- While a picker is open (command palette, go to file, go to symbol, themes, branches and every other list that opens
  at the top of the window): a modal `Dialog` named by the picker's placeholder, an `EditableComboBox` with the query
  (masked for a password) and a `ListBox` with one `ListBoxOption` per row (label, detail, at most 500). The selected
  row is the combo box's active descendant. With no rows the list's description says why ("No matches", "Indexing
  files…").
- While the update dialog (or the first-run question about update checks) is open: a modal `Dialog` with its title and text,
  a `ProgressIndicator` (0 to 100 while a download has a known size, without a value otherwise) and one `Button` per choice in
  tab order; the focused button is the tree's focus. On macOS the dialog is a window of its own ("Software Update") with
  its own adapter: that window's tree is the dialog alone (titled "Software Update"), with the frames its card and
  buttons were drawn at there, the focused button as its focus, and buttons that act as in the main window (whose tree
  still lists the dialog). The status bar item and *Help: Check for Updates…* are the way in.
- Geometry: the window, primary Explorer viewport, focused text-editor pane and status bar expose their painted bounds,
  and so do the individual rows and controls drawn on screen: Explorer rows, the editor tabs of the focused group, the
  status bar's items, the find bar's fields, status and buttons, Source Control's message field, commit button, group
  headers, file rows and their Stage/Unstage and Discard buttons, the rows of the Run and Debug view (variables,
  watches, call stack, breakpoints), the Testing view, the Remote Explorer and the Ports panel with the buttons inside
  them, the Settings and Keyboard Shortcuts rows and the items of an open menu, the Settings page's search field,
  categories, a choice's options, a changed setting's Reset button and the page's three buttons, the Extensions view's
  Installed and Market rows, the picker (its dialog, query box, list and each row it draws), the suggestion list and each
  suggestion, the bottom panel's tabs, the Search Editor (the page, its query, status, list, each row it draws, Apply
  and Undo Replace) and the Search Editor and terminal tabs of the focused group, and the update dialog (or the
  first-run question) drawn in the window with each of its buttons. A row scrolled out of its list (a picker or the
  suggestion list draws only what fits), and an item the status bar leaves out at the current width, has none. The
  picker reports its frames only to the window it is drawn in, and suggestions only while their editor was drawn in
  that window's last frame. A row's buttons that appear while the pointer is over
  the row have the place they appear at (recorded at layout, before painting); a "More Actions…" button the tree adds
  to a row whose menu opens by right click has no control of its own on screen and therefore no frame.
  Bounds are clipped to the visible content mask and scaled from GPUI logical pixels to AccessKit physical pixels.
  Resizing, switching split-editor focus and hiding/restoring panels update or clear these coordinates.
- Text positions: every text run of the editor and the terminal that is on screen has its bounds (the painted line's
  top and bottom) and the position and advance width of each of its characters, taken from the line GPUI shaped and
  painted (wide characters and non-ASCII text included; a wrapped line is split into a run per painted line). A
  selected line end is as wide as the editor draws it. AccessKit computes the bounds of any range from these, so
  `AXBoundsForRange` (VoiceOver, Zoom) and UI Automation's `GetBoundingRectangles` (Narrator, NVDA, magnifiers) return
  the caret's and the selection's rectangles where they are drawn. A run that is not on screen (scrolled away or
  folded) has no bounds, so a range that reaches into it has none either.
- Right-to-left and mixed text: each character's place comes from the glyphs the platform shaped and painted
  (`painted::char_cells`), not from GPUI's `x_for_index`, which assumes left-to-right glyph order. A painted line is cut
  into stretches whose characters follow each other on screen in one direction (`TextLine::stretches`); each stretch
  becomes runs with that `TextDirection`, and a right-to-left run gives its character positions from its right edge, as
  AccessKit expects. "abc אבג def" is three runs; a range inside the Hebrew is the rectangle of those letters where they
  are drawn, and a range across the change of direction is a rectangle per stretch. A line break drawn at the line's
  right end that does not follow the last character read (the line ends in right-to-left text) is a run of its own.
  Characters sharing one glyph (a ligature, lam-alef) share its width in reading order. On macOS Core Text reorders
  right-to-left text for display and the frames follow it; GPUI's Windows text path (DirectWrite with the bidi level
  ignored) places a run's glyphs left to right in the order DirectWrite gives them, and the frames follow that too, so
  on both platforms they are where the characters are on screen.

Focus follows the keyboard: the selected explorer row, the editor (or its selected suggestion), a find field, the
picker's combo box (its selected row), the terminal or a dialog button.

Actions: `Focus` on the explorer, a row, a tab, the editor, a find field, the terminal, the picker's input or a row,
a suggestion or a dialog button; `Click` on a row (select and open), a tab (bring it to the front), a picker row (pick
it), a suggestion (insert it), a find option (toggle) or button (run) and a dialog button; `SetTextSelection` on the
editor, and `SetValue` on the Source Control message. Actions reject a changed workspace; Source Control also checks the
repository, branch and file state (a file action applies only to the same file in the same group and state), folding a
group only toggles it from the state that was read, and committing checks the reviewed status and message. A tree may be a quarter of a second old.

### Without a tree

- Two high-contrast themes; every built-in theme passes WCAG AA (`cargo test -p theme`). After a theme is chosen or the
  colour settings change, a notice names the first pair below AA when the colours in use (theme plus
  `workbench.colorCustomizations`) fail; a theme you wrote is kept as written, imported VS Code themes are repaired.
- *Auto* picks *High Contrast Dark* or *Light* while the system asks for more contrast (macOS *Increase contrast*,
  a Windows high-contrast theme). `crates/app/src/system_prefs.rs` reads both system settings at startup and whenever a
  window comes forward.
- `workbench.reduceMotion` (`auto` follows macOS *Reduce motion* or Windows *Show animations*): the cursor stops
  blinking, spinners stand still, the resource monitor card appears without fading and the update dialog's
  indeterminate bar stops sliding.
- Every command is reachable from the palette and the keyboard.

## How it hooks in

1. `Window` implements `HasWindowHandle`. On macOS the `NSView` it returns gets `accesskit_macos::SubclassingAdapter`;
   on Windows the `HWND` gets `accesskit_windows::SubclassingAdapter`. Both subclass at run time. Call the trait method
   explicitly: the inherent `Window::window_handle()` is something else. The Windows adapter must exist before the window
   is first shown, so the window opens with `show: false` there and is activated after `attach`; if it is somehow visible
   already, `attach` skips the adapter instead of panicking.
2. **Lazy activation.** Until an assistive technology asks for the window's children (or sends `WM_GETOBJECT`) there is
   no tree, task or timer. The `ActivationHandler` cannot build the tree itself (no GPUI context), so it returns `None`
   and starts an `AsyncApp` task that sends the first full `TreeUpdate`.
3. **Updates.** While active, a 250 ms loop takes a snapshot of the workbench, compares it with the last one and calls
   `update_if_active` only when it changed. On macOS it also reports window focus; the Windows adapter follows
   `WM_SETFOCUS` / `WM_KILLFOCUS` itself. Cost, measured in a release build with a 2,000-line file open, a folder of
   200 files and the editor's 300-line range (754 nodes): a snapshot takes about 0.17 ms and the comparison 0.01 ms per
   poll, and building the tree when something changed 0.06 ms, so about 0.1 % of one core at four polls a second. A
   change reaches the screen reader within 250 ms; observer-driven updates would shorten that, not the cost.
4. **Geometry.** Non-interactive canvas probes capture the primary Explorer viewport, focused editor body and status bar,
   and each row and control listed above, without drawing, taking focus or scheduling another frame. Each window keeps
   its own record (a side view in a window of its own is drawn by the workbench in that window's coordinates), cleared
   when that window draws again, so hidden surfaces lose their coordinates. Views that draw themselves (the Explorer
   tree, Source Control, the find bar, pages and menus) keep theirs in the view (`painted.rs`). Text positions come from
   the editor's painted lines (`EditorView::a11y_layout`) and the terminal's grid (`TerminalView::a11y_layout`). The window uses its content viewport. Geometry is part
   of the snapshot comparison, so a layout or display-scale change reaches AccessKit even when text is unchanged.
5. **Actions** may arrive on another thread (UI Automation does that). They go through an unbounded channel to a task on
   the main thread and run in a GPUI context through `AsyncApp::update_window`, so they never re-enter a GPUI borrow.

6. **Other windows.** An editor's or a terminal's own window gets the same adapter with its own snapshot
   (`a11y_windows.rs`): the window title, the editor as a text field with that window's caret and selection, its find
   bar, suggestions and the picker opened there, or the terminal's text and cursor. The editor fills the window, so its
   bounds are the window's; its text runs carry the character positions painted in that window. Actions go to the workbench with that window, after making it the window commands act on.
   A side view's own window holds that view's part of the workbench's tree (taken with that window, so focus and row
   frames are its own), named after the view; the main window's tree leaves it out. An extension view's window holds
   that view's nodes, and clicks and focus from the screen reader go to the extension as in the side bar.

7. **Layout.** The main window's description says which view each side bar shows, which views live in the secondary
   side bar and where the panel is docked, so moving a view by drag and drop is reflected without reading the screen.

`CODEYARD_NO_A11Y=1` disables the adapter. `attach` is skipped under test because `TestWindow::window_handle()` is
`unimplemented!()`. `scripts/a11y-probe/` holds the Swift tools (`axdump`, `axact`, `axpress`, `axframes`) that read
the real tree of one process with `AXUIElementCreateApplication(pid)`. `axframes <pid> --select 21 4 --shot out.png`
sets the text area's selection, prints every element's frame and the selection's `AXBoundsForRange`, and draws them on
a capture of that process's window:

```
swiftc -O scripts/a11y-probe/axdump.swift -o /tmp/axdump
CODEYARD_STATE_DIR=/tmp/s CODEYARD_CONFIG_DIR=/tmp/c target/release/codeyard some/file.rs &
/tmp/axdump $!        # the terminal needs the Accessibility permission
```

On Windows, *Accessibility Insights for Windows* or `inspect.exe` from the Windows SDK shows the UI Automation tree.

Verified with the macOS tools (before the picker, find bar, suggestion and terminal nodes existed): the tree appears,
character count and string-for-range read back correctly (including non-ASCII), `AXSelectedTextRange` is writable and
moves the real caret, `AXPress` on an explorer row opens the file, the status bar follows.

Tests: unit tests in `a11y.rs` check that every kind of `TreeUpdate` is accepted by `accesskit_consumer::Tree` (valid
ids, parents, focus and active descendants), the selection mapping round-trips and long lines split correctly.
`a11y_ui_tests.rs` drives the real workbench: the command palette, the find bar and the terminal reach the snapshot with
the right focus, find options toggle from the tree, a colour customization below AA is reported, Reduce motion stops the
cursor blinking and *Auto* follows Increase contrast.

Cost: with no assistive technology the adapter is within measurement noise (≤ 2 MB); with one active, about +5…8 MB on
macOS. The CPU cost of the 250 ms snapshot loop has not been measured.

Data breakpoints are rows of the Breakpoints list: a check box "Data breakpoint: <value>" that says it lasts for the
debug session, with Remove Breakpoint and More Actions… (Enable/Disable, Remove) like a line breakpoint; Enter on the
row turns it off or on. Pages and menus that draw themselves record their rows' painted bounds with the same kind of
probe as the workbench's lists (`painted.rs`).

## Not verified / missing

- **No screen reader has been run.** VoiceOver (speech, rotor, VO cursor, "interact with text area") and NVDA, JAWS or
  Narrator on Windows. The Windows adapter builds and passes CI, but no UI Automation client has read its tree yet. The
  steps to run are in "Manual screen-reader checklist" below.
- **Right-to-left caret.** The tree reports right-to-left characters where they are drawn (see Text positions), and so
  the caret and selection bounds AccessKit computes from them are on those characters. The editor itself still draws
  its caret and selection inside right-to-left text with GPUI's `x_for_index`, which assumes glyphs in reading order:
  there the drawn caret can be away from the character the tree (and a magnifier following it) points at. On Windows,
  GPUI's DirectWrite renderer (`vendor/gpui/src/platform/windows/direct_write.rs`, `DrawGlyphRun`) ignores each run's
  bidi level and places its glyphs left to right in the order DirectWrite gives them, so a Hebrew or Arabic run is drawn
  in reading order from the left; the tree reports that drawing, not the visual order the script needs. Both are GPUI's
  text layout, outside the tree; neither was checked with a right-to-left screen-reader user.
- **Frames not recorded.** An extension view's own nodes: their frames are the extension's business (only the
  Extensions list's rows are the workbench's), and extension-platform code is outside this work.
- **Focus** was observed live on macOS only for Source Control: setting `AXFocused` on a change row of a dev build moved
  the keyboard cursor there and the row reported `Focused="1"` as the application's `AXFocusedUIElement`. Other focus
  paths (`TreeUpdate.focus`, `update_view_focus_state`) are covered by tests but not observed in a live window.
- The editor, the Explorer and Source Control's change list expose a bounded range, not the whole document, tree or
  list. Run offsets, the selection and
  set positions are document- or tree-relative, but the text value of the editor node starts at the range, so a
  client that counts characters from the node's value sees offsets relative to the first exposed line; the scroll
  position and description give that line. Only the primary selection is reported: AccessKit 0.25's `TextSelection`
  holds one anchor and one focus, so the other cursors of a multi-cursor edit have no place in the tree. Line endings
  need nothing of their own: the editor's text is held with LF whatever the file uses (the status bar's line-ending
  item says which), so offsets in the tree count one character per line break. Right-to-left text: see above. A
  wrapped line is split into a run per painted line; a folded line is in the text but has no frame.
- The terminal exposes the rows on screen; its history is reached with `ScrollUp` and `ScrollDown` (a page each, as
  Shift+Page Up/Down; the scroll position counts lines of history), after which the rows are those of the history on
  screen. It is not written to from the tree: typing goes to the shell through the keyboard, as in any terminal, so
  the shell's own line editing and programs reading raw keys keep working.
- Keyboard: the Run and Debug lists (variables, watches, call stack, breakpoints), the Testing tree, the Remote
  Explorer, the Ports list and Source Control's change list are tab stops with a cursor row (Up/Down/Home/End, Enter as a click on the row, the focus
  colour on the list and the row). Shift+F10 and the Menu key (`list.showContextMenu`) open the row's menu under it; the
  menu returns the keyboard to the list. While such a list has the focus, the tree's focus is its cursor row. The same
  key opens the Explorer's menu for the selection and the editor's at the caret. The cursor always names a row of the
  list as it is now: when rows change under it, it stays inside the list and the key acts on the row it is on (a row
  without a menu opens none).
- System confirmation dialogs on Linux are drawn by GPUI inside the window (there is no Linux adapter, see below).
- Scroll actions are on the lists the tree exposes a range of (the Explorer, the editor, Source Control's change list,
  the Search results, Language Services) and on the terminal's history; the other lists expose their rows up to the cap
  given for each view above. No `Blur`: neither adapter in use (`accesskit_macos` 0.27, `accesskit_windows` 0.35) sends
  it, so a node offering it could never receive it.
- `Expand` and `Collapse`: the Run and Debug, Testing and Remote Explorer trees and the Search results handle them, and
  so does every painted control with an expanded state (offered toward the other state only). UI Automation offers
  them on any node with an expanded state; where a node folds on `Click` only, the request is taken as that click
  when the node is in the other state, and does nothing when it already is in the state asked for.
- Linux (AT-SPI) is not implemented. Linux is tracked apart from the shipped macOS and Windows builds
  (`product-quality-plan.md`, Pass 5), and an adapter could not be verified here: `accesskit_unix` talks to the
  AT-SPI bus over D-Bus, which needs a Linux session with a client such as Orca to check against. On Wayland a client
  is never told where its window is on screen, so the frames it reports would be relative to the window, not the
  screen.
- Interface scale (`ui.scale`, zoom) is applied by GPUI at the window: the bounds reported to AccessKit are logical
  bounds × display scale × interface scale, so they match what is painted at every zoom level
  (`accessibility_bounds_follow_the_interface_scale`, `a11y_frames_tests.rs`). Checked natively at 100% and 200% with
  `axframes` (below); not yet with VoiceOver, NVDA or Narrator running, nor with UI Automation on Windows.

## Manual screen-reader checklist

The limits above that only a person with a screen reader can close. Each pass is written down with the date, the
build (`codeyard --version`), the system and screen-reader versions, and for every step **pass**, **fail** (what was
heard or seen instead) or **not run**. A failure is fixed or recorded under "Not verified / missing"; a pass replaces
the matching "not run" sentence there.

Set up once per pass:

1. Use a release build or a dev build with its own folders (`CODEYARD_DEV=1`), so the pass never touches a real
   profile.
2. Make a throwaway folder with a Git repository: `main.rs` (about 200 lines, one line with "café", one with
   "abc אבג def", one misspelled word), `notes.md`, one uncommitted change and one new file.
3. Open that folder (`codeyard <folder>`) at 100% (`View: Reset Zoom`).

### VoiceOver (macOS)

Turn VoiceOver on (Cmd+F5) after Codeyard has opened; "VO" below is Control+Option.

1. **Window and its parts.** Cmd+Tab to Codeyard. VoiceOver says the window title. VO+Right walks the window's top
   level: the activity bar, the side bar, the editor area, the panel and the status bar are each reached; write down
   the name and role VoiceOver says for each (the tree has no landmark roles, so the rotor's Landmarks list is expected
   to be empty; write down what it shows).
2. **Explorer.** Focus the Explorer (click it, or `File: Reveal Active File in Explorer`). Up and Down move between
   rows; each is read with its name, its level and "collapsed" or "expanded" for a folder. Right and Left expand and
   collapse a folder, and VoiceOver says the new state. Enter on `main.rs` opens it, and the keyboard is in the editor.
3. **VO cursor follows focus.** With "VoiceOver cursor follows keyboard focus" on (VoiceOver Utility › Navigation), Tab
   and Shift+Tab move the VO cursor with the keyboard; the cursor outline is drawn on the focused part. This is the
   live focus check that "Not verified" lacks for everything but Source Control.
4. **Editor text.** In `main.rs` VoiceOver says "edit text" (text area). Down reads the next line; Option+Right reads
   word by word; Shift+Right says what was selected. VO+Shift+Down ("interact") and VO+Right inside the text read the
   same characters. Select "café": VoiceOver says it, and the VO cursor outline sits on the drawn selection.
5. **A long file.** Cmd+Down to the end of the file and back: the line read is the one the caret is on, never a line of
   the previously exposed range. Then VO+Shift+Up ("stop interacting") and the scroll position is read from the text
   area's description.
6. **Right-to-left.** Put the caret before "א" in "abc אבג def" and press Right three times: VoiceOver reads א, ב, ג.
   Write down where the VO cursor outline is drawn for each (on the letter, or elsewhere, see "Right-to-left caret").
7. **One cursor.** Place a second cursor (`Editor: Add Cursor Below`) and type a letter. VoiceOver reads the primary
   cursor's line and position only; note what it says about the other cursor (nothing is expected: AccessKit 0.25's
   `TextSelection` holds one anchor and one focus).
8. **Leaving a control (Blur).** Focus the editor, then move the VO cursor to the status bar without moving the
   keyboard (VO+arrows, with "VoiceOver cursor follows keyboard focus" off). VoiceOver may set `AXFocused` to false on
   the text area; `accesskit_macos` 0.27 turns that into a focus request for the window's root, and only when the root
   can take focus, which it cannot (it offers no `Focus` action): nothing reaches the app, never a `Blur`. Expected:
   the keyboard stays in the editor and typing still edits `main.rs`.
9. **Terminal.** `Terminal: Toggle`. VoiceOver reads the shell prompt; type `echo hello` and Return: "hello" is read.
   Shift+Page Up scrolls the history a page and VoiceOver reads the rows now on screen.
10. **Lists with a cursor row.** Source Control: Down and Up move the cursor row, each change is read with its status;
    Shift+F10 opens the row's menu, Escape returns to the same row. Repeat in Run and Debug's breakpoints, the
    Testing tree and Ports.
11. **Popups.** Cmd+Shift+P: the palette is read as a combo box, typing narrows it and the selected row is read.
    "Editor: Show Hover" over the misspelled word reads the hover text. Word suggestions while typing are read as a
    list with the selected row.
12. **Dialogs.** Close an unsaved file: the confirmation is read with its buttons and Escape cancels it.
13. **Zoom.** Cmd+= twice, then repeat steps 2, 4 and 6: the VO cursor outline stays on the drawn row and selection.

### NVDA, Narrator and JAWS (Windows)

Run the Windows build with NVDA (the free reference) first, then the same steps with Narrator (Ctrl+Win+Enter);
JAWS when a licence is at hand. NVDA's Speech Viewer (NVDA menu › Tools) keeps a written record of what was said.

1. **Window and focus.** Alt+Tab to Codeyard: the window title is read. NVDA+Tab reads the focused element, its role
   and name.
2. **Explorer.** As VoiceOver step 2; NVDA reads "tree view item", the level, and "collapsed"/"expanded". With
   NVDA's object navigation (NVDA+Numpad 8/2/4/6, or NVDA+Shift+arrows on a laptop), the navigator object moves from
   the tree to its parent group and back.
3. **Editor text.** In `main.rs` NVDA reads "edit, multi line". Up/Down read lines, Ctrl+Right words, Shift+Right the
   selected character; NVDA+Up re-reads the current line. Narrator: Caps Lock+Right reads the next character. Select
   "café": the selection is read, and NVDA's focus highlight (Focus Highlight add-on, or Narrator's blue box) is on the
   drawn selection.
4. **Browse mode.** NVDA+Space switches to browse mode in the editor: the text is readable line by line and
   NVDA+Space returns to focus mode with the caret where it was.
5. **Right-to-left, one cursor, leaving a control.** VoiceOver steps 6, 7 and 8. On Windows a UI Automation client has
   no blur either: moving NVDA's navigator object away (object navigation) must not move the keyboard.
6. **Terminal, lists, popups, dialogs.** VoiceOver steps 9 to 12, with Shift+F10 or the Applications key for row
   menus.
7. **Expand and Collapse through UI Automation.** In the Search results, Run and Debug and the Testing tree, Narrator's
   Caps Lock+Space ("do default action") or Inspect's ExpandCollapse pattern expands and collapses a node that folds
   on a click; asking for the state it already has changes nothing.
8. **Scale.** Settings › Display at 150%, then `ui.scale` 2: repeat steps 2 and 3; the focus highlight stays on what
   is drawn.

### Linux (AT-SPI)

There is no Linux adapter (see "Not verified / missing"); this pass checks that the gap is what the documents say,
and is the list to run once an adapter exists.

1. Run Codeyard on a GNOME session with Orca (Super+Alt+S). Expected today: Orca reads the window title and nothing
   inside the window; Accerciser shows the application with no children. Anything more means an adapter appeared and
   this section and "Not verified" must be updated.
2. The confirmation dialogs are drawn inside the window (GPUI); check that Escape and Enter work on them with Orca on,
   since Orca will not read them.
3. With an adapter: VoiceOver steps 1 to 13 with Orca (Orca+Space for its preferences, Orca+Up to read the current
   line), on X11 and on Wayland. On Wayland, write down whether Orca's flat review and the magnifier land on the drawn
   parts: frames there are window-relative (see "Not verified / missing").

## Order of work

1. Screen-reader passes (VoiceOver, NVDA, Narrator) on real machines, with focus tracking observed live ("Manual
   screen-reader checklist"); fix what they find.
2. The Linux adapter, once GPUI's Linux backends give the app its window's bounds and focus.

## Geometry verification

Automated regressions exercise the real GPUI layout at narrow and wide window sizes, then pass the
resulting tree to AccessKit's consumer and hit-test the Explorer, editor and status bar. Separate cases
check split-editor focus and hiding/restoring panels in Zen mode. Coordinate conversion covers 1×,
1.5× and 2× displays and rejects empty or non-finite geometry. This establishes landmark geometry,
not per-character navigation or VoiceOver speech/rotor behavior.

A native macOS AXUIElement probe on 2026-10-28 opened a disposable project in an isolated
profile, activated `main.rs` through its accessible Explorer row, and read non-zero frames for
`AXOutline`, `AXTextArea` and the status bar (`AXGroup`). Native point hit testing returned each
landmark at its own center. Resizing the window from 1280 to 920 points changed the editor width
from 972 to 612 points and the status-bar width from 1280 to 920, and all three hit tests still
passed. The probe did not run VoiceOver or validate per-character bounds.

Rows, controls and text (Q05): `a11y_frames_tests.rs`, `a11y_row_frames_tests.rs` and the Source Control, find bar and
terminal cases compare each node's bounds with the frame its probe recorded, hit-test the centre of each Explorer row,
tab, status item, Source Control control and row button, find bar part and host row button, and ask AccessKit's
consumer for the caret's and a selection's bounding boxes, which must match the editor's own IME caret rectangle at
100% and 200%, in the main window and in an editor's own window (where the main window then reports no stale
positions). A Remote Explorer moved into its own window reports its rows in that window's coordinates.

A native macOS check on 2026-10-31 ran a dev build (`CODEYARD_DEV=1`, its own state and configuration folders, a
throwaway project) and read only that process with `axframes`. At 100% the Explorer rows were 24 points tall and
stacked under the outline's top edge, the tab and the status items sat where they are drawn, and after setting
`AXSelectedTextRange` to "café" on line 2 its `AXBoundsForRange` (636.3, 270.0, 28.9 × 18.0 points) outlined exactly
the drawn selection; the caret character's bounds were 7.2 points wide. At `ui.scale` 2 the same rows were 48 points
tall, the selection's bounds 57.8 × 36.0 points, again on the drawn highlight; status items the bar leaves out at that
width had no frame. With the editor moved into a window of its own the selection's bounds were on the highlight in
that window at 100% and 200%, and the main window's leftover text area reported none. The captures with the frames
drawn on them were checked by eye. VoiceOver itself was not run.

Search (Q06), natively on the same kind of dev build: `axframes --set-value Search café` set the query through
`AXValue`, the view searched, and the tree listed the status ("2 results in 1 file") and an `AXOutline` with the file
row (its Replace in this file and Dismiss buttons at the place they appear on hover) and both matches, each frame on
its drawn row; `AXPress` on the second match opened `text.rs` with the match selected and the text area focused.

More frames and right-to-left text, natively on a dev build (`CODEYARD_DEV=1`, its own state and config folders, a
throwaway project, `axframes` against that process only; keys sent with `--do key:…`/`type:…` inside that window): the
Extensions view with two installed extensions listed its tabs (59 × 24 and 48 × 24 points), search field, list and
both rows (257 × 44) on what is drawn; the command palette its window, combo box and twelve drawn rows (586 × 24, one
under the other); word suggestions after typing "me" their list and both rows under the caret's line; the Settings
page its search field, categories, a choice's options ("off", "on") inside their row and the page's buttons; the
bottom panel's tabs side by side; a Search Editor its tab, page, query, list and match rows. In "abc אבג def",
`AXBoundsForRange` for א (character 4) was 621.8 and for ג (character 6) 607.4 points from the left, each 7.2 wide:
leftwards in reading order, and the capture showed the rectangle on ג. A range over "c אבג d" was one 50.5-point
rectangle over those letters. The editor's own drawn selection in that line was elsewhere (see "Right-to-left caret").

History, popups and the Debug Console, natively on a dev build (same rules): with this repository open, Source
Control's tree listed the Worktrees section (header 259 × 21 points, Add worktree, Refresh, each checkout on screen
with its frame), the Graph's header, Search commits (a check box), Fetch, Pull, Push, Reload history and More Actions…
(22 × 22 each), and the commits on screen (259 × 22, one under the other) with Open Changes and More Actions….
"Editor: Show Hover" over a misspelled word gave a "Hover" group (368 × 32) with the line "Information: “sentense” is
not in the dictionary…" on its drawn text. With a C program paused by `lldb-dap`, `AXPress` on the Debug Console tab
showed the console; `axframes --set-value` put "1+2" in the expression field and `AXPress` on Evaluate ran it: the log
read "> 1+2" and "(int) $0 = 3".
