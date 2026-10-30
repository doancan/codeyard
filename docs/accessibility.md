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
- One `Tree` per folder of the workspace: "Explorer", or "Explorer: name" when there are several. One `TreeItem` per
  row in a range of 1,000 rows: name, level, selected, expanded, set position in the whole tree. The range is centred
  on the selection while it is on screen, otherwise on the visible rows; the tree reports its scroll position in rows
  and supports scroll up/down by a page, which moves the range. These trees are omitted while Explorer is hidden. Row actions resolve the current path rather than reusing an old row index.
- While Source Control is visible, a `Group` with the branch, a writable `MultilineTextInput` for the commit message,
  a correctly labeled/enabled commit button, change groups with expand/collapse buttons, file diff buttons and
  Stage/Unstage buttons. Operation errors are assertive live regions; successful results are polite. The message
  field's real focus is exposed. The graph, worktrees and sync controls are not exposed yet.
- While the Problems panel is open, a `List` labeled with the number of problems: a `Heading` per file and a `ListItem`
  per problem ("Error: …", with file, line and column as its description) or related location, each with `Click` to open
  it. At most 2,000 rows are exposed. With nothing to show, a polite `Status` says "No problems.". An activation from an
  outdated tree opens nothing unless the same row is still at the same place.
- While the bottom panel is open, a `TabList` with its tabs (Problems, Output, Debug Console while debugging, Ports,
  Terminal), the selected one marked and counts as descriptions; `Click` switches tabs. While Output is shown, a polite
  `Log` holds its last 400 lines.
- While the Search side bar is visible, a polite `Status` with its result line (the same words as on screen) and a `List`
  of up to 500 rows: a `Heading` per file and a `ListItem` per match (`line: text`). `Click` opens the match; it is
  found by file, line and column, so an activation after the results changed opens it only if it is still a result.
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
  page or a closed or reopened page changes nothing. With no rows, a polite `Status` says why.
- The Keyboard Shortcuts page in front is a `Group` with its `SearchInput` and a `List` ("{n} commands") of up to
  2,000 rows: command as label, its key (or "No shortcut") as description, set position. `Click` runs the command as a
  click on the row does, only while the page still lists it. The page is read-only otherwise: keys are assigned in the
  "Preferences: Open Keyboard Shortcuts" picker, which is exposed as a picker.
- While Run and Debug is visible, a `Group` "Run and Debug": the "Start Debugging (F5)" `Button` (the configuration it
  starts as description), a `RadioGroup` of the launch configurations (`Click` selects, as the selector's menu does),
  "Open Launch Configuration", a polite `Status` with the current session and its state ("app: paused on breakpoint",
  or "No debug session."), a `Tree` "Variables" (name as label, value as value, type as description, level for the
  nesting, `Expand`/`Collapse`/`Click` on a value with children, `Click` on "… n more"), a `List` "Watch" (expression
  and value, a Remove Expression button each) with "Add Expression…", a `Tree` "Call Stack" (sessions with their state
  when there are several, threads, frames with their place; the current ones selected; `Click` makes current, as a
  click on the row does) and a `List` "Breakpoints" of `CheckBox`es ("main.rs, line 3" or "name()", checked while
  enabled, condition and binding failure as description, a Remove Breakpoint button each), then "Add Function
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
- Row menus (for assistive technology): a Run and Debug variable (not a scope), call stack frame and line breakpoint, a Remote Explorer host and a
  Ports row have the `ShowContextMenu` action and a "More Actions…" `Button` (a menu popup, the row's name as
  description) among their children. Both open the menu a right click on the row opens (Set Value…, Copy Value, Add to
  Watch; Restart Frame, Copy Call Stack; Edit Condition…, Disable Breakpoint; Connect, Edit Host…, Remove Host…;
  Change Local Port…, Protocol, Retry, Stop Forwarding), at the bottom left of the row as last painted (the middle of
  the window when the row is scrolled out of its list). The menu then takes the focus as described above and returns
  it when it closes. The macOS and Windows adapters do not forward `ShowContextMenu` yet, so the button is the way in
  for VoiceOver and Narrator today (with the keyboard alone, Shift+F10 on the focused row does the same). The request is refused unless the row is still the same variable, frame, breakpoint
  (in the same enabled state), host (in the same state) or port forward at that place, in the same debug session.
- While the extension consent sheet is open and in front: a modal `AlertDialog` named "Install {name}?", described by where the
  package comes from, the High-risk line and the warnings, with one `Label` per line of the sheet (id, version and
  licence, source, description, permissions, the publisher's reason, contributions, warnings) and the Install and
  Cancel `Button`s (`Click`, `Focus`). The keyboard lands on Install, or on Cancel for a High-risk set or a sheet with
  warnings, and the focused button is the tree's focus. Escape answers Cancel. Escape, Cancel and Install give the
  keyboard back to whatever had it when the sheet opened. A button acts only on the package the tree showed (a newer
  sheet for another package is left alone).
- Confirmations asked with a system dialog (discard changes, uninstall an extension, remove a host, delete files) are
  native dialogs on macOS (a sheet) and Windows (a task dialog); the system exposes them to its screen reader and they
  are not part of this tree.
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
  screen reader speaks it as the selection moves while the keyboard stays in the editor.
- While the terminal panel is open, a read-only `Terminal`: one text run per row on screen and the cursor as a collapsed
  selection.
- `Status` "Status bar" (one label per item).
- The notice in the status bar is a live region of its own: an error (including a file that could not be opened) is an
  `Alert` with `Live::Assertive`, so it interrupts speech; an information notice is a `Status` with `Live::Polite`.
- While a picker is open (command palette, go to file, go to symbol, themes, branches and every other list that opens
  at the top of the window): a modal `Dialog` named by the picker's placeholder, an `EditableComboBox` with the query
  (masked for a password) and a `ListBox` with one `ListBoxOption` per row (label, detail, at most 500). The selected
  row is the combo box's active descendant. With no rows the list's description says why ("No matches", "Indexing
  files…").
- While the update dialog (or the first-run question about update checks) is open: a modal `Dialog` with its title and text,
  a `ProgressIndicator` (0 to 100 while a download has a known size, without a value otherwise) and one `Button` per choice in
  tab order; the focused button is the tree's focus. On macOS the dialog is a window of its own ("Software Update"); the
  tree belongs to the editor window and still lists the dialog, but the separate window has no adapter yet, so a screen
  reader reaches its title and nothing more there. The status bar item and *Help: Check for Updates…* are the way in.
- Geometry: the window, primary Explorer viewport, focused text-editor pane and status bar expose their painted bounds,
  and so do the rows of the Run and Debug view (variables, watches, call stack, breakpoints), the Testing view, the
  Remote Explorer and the Ports panel while they are drawn (a row scrolled out of its list has none).
  Bounds are clipped to the visible content mask and scaled from GPUI logical pixels to AccessKit physical pixels.
  Resizing, switching split-editor focus and hiding/restoring panels update or clear these coordinates.

Focus follows the keyboard: the selected explorer row, the editor (or its selected suggestion), a find field, the
picker's combo box (its selected row), the terminal or a dialog button.

Actions: `Focus` on the explorer, a row, a tab, the editor, a find field, the terminal, the picker's input or a row,
a suggestion or a dialog button; `Click` on a row (select and open), a tab (bring it to the front), a picker row (pick
it), a suggestion (insert it), a find option (toggle) or button (run) and a dialog button; `SetTextSelection` on the
editor, and `SetValue` on the Source Control message. Actions reject a changed workspace; Source Control also checks the
repository, branch and file state, and committing checks the reviewed status and message. A tree may be a quarter of a second old.

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
   `WM_SETFOCUS` / `WM_KILLFOCUS` itself. Polling is a simplification; observer-driven updates are the planned
   replacement.
4. **Geometry.** Three non-interactive canvas probes capture the primary Explorer viewport, focused editor body and status bar
   during painting, without drawing, taking focus or scheduling another frame. The workbench clears these cached bounds
   before each render, so hidden surfaces lose their coordinates. The window uses its content viewport. Geometry is part
   of the snapshot comparison, so a layout or display-scale change reaches AccessKit even when text is unchanged.
5. **Actions** may arrive on another thread (UI Automation does that). They go through an unbounded channel to a task on
   the main thread and run in a GPUI context through `AsyncApp::update_window`, so they never re-enter a GPUI borrow.

6. **Other windows.** An editor's or a terminal's own window gets the same adapter with its own snapshot
   (`a11y_windows.rs`): the window title, the editor as a text field with that window's caret and selection, its find
   bar, suggestions and the picker opened there, or the terminal's text and cursor. The editor fills the window, so its
   bounds are the window's. Actions go to the workbench with that window, after making it the window commands act on.
   A side view's own window holds that view's part of the workbench's tree (taken with that window, so focus and row
   frames are its own), named after the view; the main window's tree leaves it out. An extension view's window holds
   that view's nodes, and clicks and focus from the screen reader go to the extension as in the side bar.

7. **Layout.** The main window's description says which view each side bar shows, which views live in the secondary
   side bar and where the panel is docked, so moving a view by drag and drop is reflected without reading the screen.

`CODEYARD_NO_A11Y=1` disables the adapter. `attach` is skipped under test because `TestWindow::window_handle()` is
`unimplemented!()`. `scripts/a11y-probe/` holds the Swift tools (`axdump`, `axact`, `axpress`) that read the real
tree with `AXUIElementCreateApplication(pid)`:

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
  Narrator on Windows. The Windows adapter builds and passes CI, but no UI Automation client has read its tree yet.
- **Fine-grained bounds.** Landmark frames, the rows of the Run and Debug, Testing, Remote Explorer and Ports lists, the
  rows of the Settings and Keyboard Shortcuts pages and the items of an open menu are available (painted, clipped to
  what is visible; a row scrolled out of view has none), but individual Explorer rows, status items, Source Control
  controls, the buttons inside rows and text runs do not yet expose their painted frames. Text still needs `character_positions` and
  `character_widths`; precise character hit testing, caret highlighting and magnifier caret tracking remain unimplemented.
- **Focus** was not observed live on macOS (`Focused="0"` in every dump: the test window was not key). The code path
  (`TreeUpdate.focus`, `update_view_focus_state`) exists but is unverified.
- The editor and the Explorer expose a bounded range, not the whole document or tree. Run offsets, the selection and
  set positions are document- or tree-relative, but the text value of the editor node starts at the range, so a
  client that counts characters from the node's value sees offsets relative to the first exposed line; the scroll
  position and description give that line. Primary selection only; no multi-cursor, folding, wrapped lines, RTL or CRLF handling.
- The terminal exposes the screen only, not the scrollback, and cannot be written to from the tree (typing goes through
  the keyboard as usual).
- Not exposed: an installed extension's page, the Source Control graph, worktrees and sync controls, hover and
  signature popups, the Run and Debug view's configuration selector's menu, opening a breakpoint's place, the Testing filter field, the Remote
  Explorer's Add, SSH config and Refresh buttons, and the Debug Console. Only the tabs of the focused editor group are
  listed, and diff, large-file and page tabs are not.
- Keyboard: the Run and Debug lists (variables, watches, call stack, breakpoints), the Testing tree, the Remote
  Explorer and the Ports list are tab stops with a cursor row (Up/Down/Home/End, Enter as a click on the row, the focus
  colour on the list and the row). Shift+F10 and the Menu key (`list.showContextMenu`) open the row's menu under it; the
  menu returns the keyboard to the list. While such a list has the focus, the tree's focus is its cursor row. The same
  key opens the Explorer's menu for the selection and the editor's at the caret. The cursor always names a row of the
  list as it is now: when rows change under it, it stays inside the list and the key acts on the row it is on (a row
  without a menu opens none).
- System confirmation dialogs on Linux are drawn by GPUI inside the window (there is no Linux adapter, see below).
- No scroll actions, no `Blur`. Explicit `Expand`/`Collapse` only in the Run and Debug, Testing and Remote Explorer
  trees (elsewhere Click toggles).
- The update window on macOS has no adapter of its own.
- Linux (AT-SPI) is not implemented: it needs bounds, window events and an async D-Bus runtime.
- Interface scale (`ui.scale`, zoom) is applied by GPUI at the window: the bounds reported to AccessKit are logical
  bounds × display scale × interface scale, so they match what is painted at every zoom level
  (`accessibility_bounds_follow_the_interface_scale`). Not yet checked with VoiceOver, NVDA or Narrator at 150/200%.

## Order of work

1. Screen-reader passes (VoiceOver, NVDA, Narrator) on real machines; fix what they find.
2. Per-control/text-run bounds and real focus tracking, observer-driven updates.
3. The remaining panels (search results, Source Control graph/worktrees/sync, problems), the Settings page, context menus and dialogs.
4. The editor proper: visible lines only with shaped character positions, correct absolute offsets, scroll actions,
   multi-cursor policy, diagnostics announcements.
5. The Linux adapter and regression tests against a real accessibility client.

## Landmark geometry verification

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
