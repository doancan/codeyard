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
| Windows 10 1809 or newer, x64 | Yes | Partial, through UI Automation (AccessKit), the same tree as on macOS. Not yet tested with NVDA, JAWS or Narrator |
| Linux | Not released | No (AT-SPI is not implemented) |

## What works today

**Keyboard.** Every command is in the command palette (⇧⌘P on macOS, Ctrl+Shift+P on Windows), and *Help: Keyboard Shortcuts
Reference* lists each command with its keys. Shortcuts can be changed in `keybindings.json`, and VS Code key bindings can be
imported. The update dialog can be used entirely with Tab, Shift+Tab, Enter, Space and Esc.

**Seeing the screen.**
- *High Contrast Dark* and *High Contrast Light* themes. The *Auto* theme follows the system's light or dark setting, and
  switches to the high-contrast pair while the system asks for more contrast (*Increase contrast* on macOS, a contrast theme on
  Windows).
- If the colours in use fall below WCAG AA (a theme you wrote, or `workbench.colorCustomizations`), a notice says which pair
  and by how much when you choose the theme or change the colours. Imported VS Code themes are repaired automatically.
- ⌘= / ⌘- / ⌘0 (Ctrl on Windows) scale the whole interface from 75% to 200% (text, controls, menus, dialogs and
  accessibility bounds together) or reset it to 100%. The value is saved as `ui.scale` and applies to every window. The
  `editor.fontSize` and `terminal.fontSize` settings (6–48) set the editor and terminal text size within that scale.

**Motion.** `workbench.reduceMotion` leaves out animations: the cursor stops blinking, spinners and the update progress bar
stand still, and panels appear without fading. Its default, `auto`, follows the system (*Reduce motion* on macOS, *Show
animations* on Windows). `editor.cursorBlinking: "solid"` stops only the cursor.

**Screen readers (macOS and Windows).** An assistive technology can read and use:
- the file explorer, one tree per folder of the workspace (name, level, expanded or collapsed, selected). Activating a row
  opens the file;
- Source Control: the branch, commit message and commit state, expandable change groups, file diffs, Stage/Unstage and
  operation results. Hidden Explorer trees are omitted while another sidebar is visible;
- the editor tabs as a tab list (name, modified, the tab in front). Activating a tab brings it to the front;
- the active editor as a text field: the text around the caret, the caret and the selection. Moving the selection from the
  assistive technology moves the real caret;
- suggestions while typing: the selected suggestion is spoken as it changes, and can be inserted from the list;
- the find bar: the search and replace fields, the match count ("3 of 12", spoken as it changes), the options with their
  state, and the buttons;
- the command palette and every other picker (go to file, symbols, themes, branches…): the query, the list and the selected
  row, spoken as it changes. Activating a row picks it;
- the terminal: the text on screen and the cursor position;
- the status bar items as text;
- notices as live regions. An error interrupts speech, other notices wait their turn;
- the update dialog: title, text, progress and buttons.

The tree is only built when an assistive technology asks for it, so it costs nothing when no assistive technology is running.
`CODEYARD_NO_A11Y=1` turns it off.

## Known limitations

These are the important gaps, in rough order of how much they get in the way. The full list is in
[docs/accessibility.md](docs/accessibility.md#not-verified--missing).

- **No screen reader has been tested yet.** The tree is checked with the macOS accessibility API and with automated tests on
  every change, but no one has run a real VoiceOver, NVDA, JAWS or Narrator session against it. Expect rough edges and please
  report them.
- **No screen reader on Linux.**
- **Incomplete on-screen positions.** The window, primary Explorer viewport, active editor pane and status bar have
  bounds. Individual controls, rows and characters still need geometry for precise cursor highlights and caret tracking.
- **Some views are still silent:** search results, the Source Control graph, worktrees and sync controls, debug, testing and remote
  views, the problems, output and ports panels, hover popups, the Settings and Keyboard Shortcuts pages, context
  menus and dialogs other than the update dialog.
- The editor exposes about 300 lines around the caret, not the whole file. Only the primary selection is exposed, not extra
  cursors. Folding and wrapped lines are not reflected. The terminal exposes the screen, not the scrollback.
- Only editor and terminal text can be enlarged. The rest of the interface (sidebar, tabs, status bar, menus, dialogs) keeps a
  fixed size; the system's display scaling (*Larger Text* on macOS, *Scale* on Windows) enlarges everything.
- On macOS the update window is separate from the editor window, and a screen reader reaches only its title there. The
  status bar item and *Help: Check for Updates…* lead to the same choices.
- Updates reach the assistive technology up to a quarter of a second after the screen changes.

## Reporting a barrier

Open an issue with the **[Accessibility barrier](https://github.com/doancan/codeyard/issues/new?template=6-accessibility.yml)**
form. Describe what you tried to do, what got in the way, and which assistive technology or setting you used. You do not need to
know the cause. The exact words a screen reader spoke are the most useful detail.

If you cannot use GitHub's issue forms, start a thread in
[Discussions](https://github.com/doancan/codeyard/discussions). A security problem goes through [SECURITY.md](SECURITY.md)
instead, even if it is also an accessibility problem.

You can expect a reply within a few days. A barrier with no workaround is fixed before new features in the same area.
