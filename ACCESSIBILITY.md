# Accessibility

Codeyard should be usable by people who rely on a screen reader, the keyboard alone, zoom or high-contrast colours. Today it is
only partly there. This page states what works, what does not, and how to tell us about a barrier. The technical detail
behind it is in [docs/accessibility.md](docs/accessibility.md).

## Commitment

- An accessibility barrier is a bug, not a feature request. Reports are triaged like other bugs and labelled `accessibility`.
- Every built-in theme must pass WCAG 2.x AA contrast for all text and UI colour pairs. A test (`cargo test -p theme`) enforces
  this, so a theme that falls below it cannot be released.
- Every command can be run from the keyboard, through the command palette and through rebindable shortcuts.
- New interface text is translatable (English and Turkish today). The accessible names the screen reader speaks use the same
  translations.
- This page is updated whenever support changes, including when something gets worse.

## Supported environments

| Platform | Keyboard, zoom, themes | Screen reader |
| --- | --- | --- |
| macOS 11 or newer, Apple silicon | Yes | Partial, through VoiceOver's accessibility API (AccessKit). Not yet tested with VoiceOver itself |
| Windows 10 1809 or newer, x64 | Yes | **No.** UI Automation is not implemented, so NVDA, JAWS and Narrator see an empty window |
| Linux | Not released | No (AT-SPI is not implemented) |

## What works today

**Keyboard.** Every command is in the command palette (⇧⌘P on macOS, Ctrl+Shift+P on Windows), and *Help: Keyboard Shortcuts
Reference* lists each command with its keys. Shortcuts can be changed in `keybindings.json`, and VS Code key bindings can be
imported. The update dialog can be used entirely with Tab, Shift+Tab, Enter, Space and Esc.

**Seeing the screen.**
- *High Contrast Dark* and *High Contrast Light* themes. The *Auto* theme follows the system's light or dark setting.
- ⌘= / ⌘- / ⌘0 (Ctrl on Windows) make the editor and terminal text larger, smaller or the default size for the session. The
  `editor.fontSize` and `terminal.fontSize` settings (6–48) keep a size permanently.
- `editor.cursorBlinking: "solid"` stops the cursor from blinking.

**Screen readers (macOS only).** An assistive technology can read and use:
- the file explorer as a tree (name, level, expanded or collapsed, selected). Activating a row opens the file;
- the editor tabs as a tab list (name, modified, the tab in front). Activating a tab brings it to the front;
- the active editor as a text field: the text around the caret, the caret and the selection. Moving the selection from the
  assistive technology moves the real caret;
- the status bar items as text;
- notices as live regions. An error interrupts speech, other notices wait their turn;
- the update dialog: title, text, progress and buttons.

The tree is only built when an assistive technology asks for it, so it costs nothing when no assistive technology is running.
`CODEYARD_NO_A11Y=1` turns it off.

## Known limitations

These are the important gaps, in rough order of how much they get in the way. The full list is in
[docs/accessibility.md](docs/accessibility.md#not-verified--missing).

- **No screen reader at all on Windows or Linux.**
- **VoiceOver itself has not been tested.** The tree has been checked with the macOS accessibility API and with unit tests. No
  one has run a real VoiceOver session against it yet.
- **No on-screen positions.** Elements have no on-screen bounds, so the VoiceOver cursor highlight, exploring with the mouse or
  touch, and Zoom following the caret do not work.
- **Many views are silent:** the command palette and other pickers, the find bar, completion and hover popups, the terminal,
  the bottom panel, sidebar panels other than the explorer, the Settings page and most dialogs.
- The editor exposes about 300 lines around the caret, not the whole file. Only the primary selection is exposed, not extra
  cursors. Folding and wrapped lines are not reflected.
- Only editor and terminal text can be enlarged. The rest of the interface (sidebar, tabs, status bar, menus, dialogs) keeps a
  fixed size; the system's display scaling (*Larger Text* on macOS, *Scale* on Windows) enlarges everything.
- The interface does not follow the system's *Reduce motion* or *Increase contrast* settings. Use the settings above instead.
- Contrast is enforced for built-in themes only. Themes you import or write yourself are not checked.
- Only the first folder of a multi-folder workspace appears in the explorer tree.
- Updates reach the assistive technology up to a quarter of a second after the screen changes.

## Reporting a barrier

Open an issue with the **[Accessibility barrier](https://github.com/doancan/codeyard/issues/new?template=6-accessibility.yml)**
form. Describe what you tried to do, what got in the way, and which assistive technology or setting you used. You do not need to
know the cause. The exact words a screen reader spoke are the most useful detail.

If you cannot use GitHub's issue forms, start a thread in
[Discussions](https://github.com/doancan/codeyard/discussions). A security problem goes through [SECURITY.md](SECURITY.md)
instead, even if it is also an accessibility problem.

You can expect a reply within a few days. A barrier with no workaround is fixed before new features in the same area.
