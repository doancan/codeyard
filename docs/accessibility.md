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
  visible row: name, level, selected, expanded, set position; capped at 5,000 rows per tree. These trees are omitted
  while Explorer is hidden. Row actions resolve the current path rather than reusing an old row index.
- While Source Control is visible, a `Group` with the branch, a writable `MultilineTextInput` for the commit message,
  a correctly labeled/enabled commit button, change groups with expand/collapse buttons, file diff buttons and
  Stage/Unstage buttons. Operation errors are assertive live regions; successful results are polite. The message
  field's real focus is exposed. The graph, worktrees and sync controls are not exposed yet.
- While the Problems panel is open, a `List` labeled with the number of problems: a `Heading` per file and a `ListItem`
  per problem ("Error: …", with file, line and column as its description) or related location, each with `Click` to open
  it. At most 2,000 rows are exposed. With nothing to show, a polite `Status` says "No problems.". An activation from an
  outdated tree opens nothing unless the same row is still at the same place.
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
- `MultilineTextInput` for the active editor: file name, `(modified)`, a text window around the caret, text runs of at
  most 200 characters, the primary selection. While suggestions are open it holds a `ListBox` "Suggestions" (one
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
- Geometry: the window, primary Explorer viewport, focused text-editor pane and status bar expose their painted bounds.
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

## Not verified / missing

- **No screen reader has been run.** VoiceOver (speech, rotor, VO cursor, "interact with text area") and NVDA, JAWS or
  Narrator on Windows. The Windows adapter builds and passes CI, but no UI Automation client has read its tree yet.
- **Fine-grained bounds.** Landmark frames are available, but individual Explorer rows, status items, Source Control
  controls and text runs do not yet expose their painted frames. Text still needs `character_positions` and
  `character_widths`; precise character hit testing, caret highlighting and magnifier caret tracking remain unimplemented.
- **Focus** was not observed live on macOS (`Focused="0"` in every dump: the test window was not key). The code path
  (`TreeUpdate.focus`, `update_view_focus_state`) exists but is unverified.
- The editor exposes a window of lines around the caret, not the whole document, so absolute offsets differ from
  file offsets in long files. Primary selection only; no multi-cursor, folding, wrapped lines, RTL or CRLF handling.
- The terminal exposes the screen only, not the scrollback, and cannot be written to from the tree (typing goes through
  the keyboard as usual).
- Not exposed: the search sidebar (its results can be opened in a Search Editor), debug, testing and remote sidebars, an installed extension's page and the consent sheet, the Source Control graph, worktrees and sync controls, the bottom
  panel's other views (output, ports), hover and signature popups, the Settings and Keyboard Shortcuts pages,
  context menus, and dialogs other than the update dialog. Only the tabs of the focused editor group are listed, and
  diff, large-file and page tabs are not.
- No scroll actions, no `Blur`, no explicit expand/collapse action (Click toggles).
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
