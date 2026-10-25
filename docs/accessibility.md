# Accessibility

GPUI 0.2.2 has no accessibility tree of its own. Codeyard adds one on macOS with AccessKit
(`crates/app/src/a11y.rs`, `accesskit 0.25`, `accesskit_macos 0.27`); no GPUI patch is needed. VoiceOver itself has
**not** been run against it (see "Not verified").

## What exists

- `Window` → `Tree` "Explorer" (one `TreeItem` per visible row: name, level, selected, expanded, set position;
  capped at 5,000 rows) → `MultilineTextInput` for the active editor (file name, `(modified)`, a text window around
  the caret, text runs of at most 200 characters, the primary selection) → `Status` "Status bar" (one label per item).
- While the update dialog (or the first-run question about update checks) is open: a modal `Dialog` with its title and text,
  a `ProgressIndicator` (0 to 100 while a download has a known size, without a value otherwise) and one `Button` per choice in
  tab order; the focused button is the tree's focus. The status bar names the update item. Everything in it is also reachable by
  keyboard (Tab, Shift+Tab, Enter, Space, Esc). On macOS the dialog is a window of its own ("Software Update"); the accessibility
  tree above belongs to the editor window and still lists the dialog, but the separate window itself has no adapter yet, so a screen
  reader reaches its title and nothing more. Until it has one, the status bar item and *Help: Check for Updates…* are the way in.
- Actions: `Focus` (editor, explorer, row, dialog button), `Click` on a row (select and open) or a dialog button,
  `SetTextSelection` on the editor.
- Accessibility of the interface that does not need a tree: two high-contrast themes, a contrast test for every
  theme, every command reachable from the palette and the keyboard.

## How it hooks in

1. `Window` implements `HasWindowHandle`; the `NSView` it returns gets `accesskit_macos::SubclassingAdapter`, which
   subclasses the view's class at run time. Call the trait method explicitly: the inherent `Window::window_handle()`
   is something else.
2. **Lazy activation.** Until an assistive technology (VoiceOver, Accessibility Inspector, any `AXUIElement` client)
   asks for the window's children there is no tree, task or timer. The `ActivationHandler` cannot build the tree
   itself (no GPUI context), so it returns `None` and starts an `AsyncApp` task that sends the first `TreeUpdate`.
3. **Updates.** While active, a 250 ms loop takes a snapshot of the workbench, compares it with the last one and calls
   `update_if_active` only when it changed. Polling is a simplification; observer-driven updates are the planned
   replacement.
4. **Actions** arrive on the main thread, are queued and run in a GPUI context through `AsyncApp::update_window`, so
   they never re-enter a GPUI borrow.

`CODEYARD_NO_A11Y=1` disables the adapter. `attach` is skipped under test because `TestWindow::window_handle()` is
`unimplemented!()`. `scripts/a11y-probe/` holds the Swift tools (`axdump`, `axact`, `axpress`) that read the real
tree with `AXUIElementCreateApplication(pid)`:

```
swiftc -O scripts/a11y-probe/axdump.swift -o /tmp/axdump
CODEYARD_STATE_DIR=/tmp/s CODEYARD_CONFIG_DIR=/tmp/c target/release/codeyard some/file.rs &
/tmp/axdump $!        # the terminal needs the Accessibility permission
```

Verified with those tools: the tree appears, character count and string-for-range read back correctly (including
non-ASCII), `AXSelectedTextRange` is writable and moves the real caret, `AXPress` on an explorer row opens the file,
the status bar follows. Unit tests check that the `TreeUpdate` is accepted by `accesskit_consumer::Tree` (valid ids,
parents and focus), the selection mapping round-trips and long lines split correctly.

Cost: with no assistive technology the adapter is within measurement noise (≤ 2 MB); with one active, about +5…8 MB.
The CPU cost of the 250 ms snapshot loop has not been measured.

## Not verified / missing

- **VoiceOver** (speech, rotor, VO cursor, "interact with text area") has not been run.
- **No bounds.** Nodes have no frames, so VoiceOver's cursor highlight, mouse exploration and hit testing have
  nothing to work with, and magnifier caret tracking is impossible. This needs painted element bounds from GPUI
  (a small patch or per-landmark `canvas` bounds) and then `set_bounds`, `character_positions`, `character_widths`.
- **Focus** was not observed live (`Focused="0"` in every dump: the test window was not key). The code path
  (`TreeUpdate.focus`, `update_view_focus_state`) exists but is unverified.
- The editor exposes a window of lines around the caret, not the whole document, so absolute offsets differ from
  file offsets in long files. Primary selection only; no multi-cursor, folding, wrapped lines, RTL or CRLF handling.
- Not exposed: tabs, sidebar panels other than the explorer, bottom panel, terminal, find bar, pickers and the command
  palette, completion popup, hover, settings, other dialogs, notices (these should be live regions; an update notice is
  only in the status bar's text). Only the first
  explorer root is exposed.
- No scroll actions, no `Blur`, no explicit expand/collapse action (Click toggles).
- Windows (UI Automation) and Linux (AT-SPI) are not implemented. The same AccessKit tree applies; Windows needs an
  `HWND` subclass adapter, Linux needs bounds, window events and an async D-Bus runtime.

## Order of work

1. Bounds, real focus tracking, tabs, panels, status items as buttons, pickers, live-region notices, observer-driven
   updates.
2. The editor proper: visible lines only with shaped character positions, correct absolute offsets, scroll actions,
   multi-cursor policy, diagnostics and completion announcements, find bar.
3. Terminal grid (one run per row) and the remaining panels.
4. Windows and Linux adapters.
5. Screen-reader passes (VoiceOver, NVDA, Orca) and regression tests.
