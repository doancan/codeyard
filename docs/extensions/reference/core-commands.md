# Core commands

<!-- Generated from crates/ext-manifest/src/core_commands.rs: run `UPDATE_DOCS=1 cargo test -p ext-manifest core_commands` after changing it. -->

The Codeyard commands an extension may run. Code runs one with `commands.execute("core:<id>", args)`; it must list the command in `[permissions] commands` and hold the permission the command implies, both in this window (what the user turned off and Workspace Trust count). A command that is not listed here is refused with the fix. A `[[menus]]` entry may name one of these that needs no argument, and `[[keybindings]]` any command of `keybindings.jsonc`; the user's click or key press runs them, so they need no permission.

```toml
[permissions]
commands = ["core:workbench.saveAll"]
write = ["src/**"]
```

| Command | In the palette | Argument | Implies | What it does |
| --- | --- | --- | --- | --- |
| `core:workbench.toggleZenMode` | *View: Toggle Zen Mode* | none | nothing | Turns Zen mode on or off. |
| `core:workbench.openSettingsPage` | *Preferences: Open Settings* | none | nothing | Opens the Settings page. |
| `core:workbench.showFiles` | *View: Files* | none | nothing | Shows the file explorer and moves the keyboard to it. |
| `core:workbench.revealActiveFile` | *File: Reveal Active File in Explorer* | none | nothing | Shows the active file in the explorer. |
| `core:workbench.showProblems` | *View: Problems* | none | nothing | Opens the Problems panel. |
| `core:workbench.saveAll` | *File: Save All* | none | `write` on every file it saves | Saves every open file with unsaved changes. |
| `core:editor.formatDocument` | *Editor: Format Document* | none | `write` on the active file | Formats the active file. |
| `core:editor.open` | — | a `file://` URI (required) | `read` on the file | Opens a workspace file in the editor. |
| `core:tasks.run` | *Tasks: Run Task…* | a task's label (optional) | `run` on the task's program | Runs the task with this label; with no label, opens the task list. |

`tasks.run` with a label runs only a task whose command is one program with its arguments (no `&&`, pipes or redirections), and that program must be in `run`. A task an extension provides runs in that extension, under its own permissions.
