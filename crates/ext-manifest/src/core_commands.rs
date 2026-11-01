//! The core commands extensions may run (docs/plugin-architecture.md §6.1): `commands.execute("core:…")` from code,
//! and `[[menus]]` and `[[keybindings]]` entries. Each one says which permission it implies; code that runs it must
//! list it in `permissions.commands` and hold that permission too. The author reference
//! (`docs/extensions/reference/core-commands.md`) is generated from this table, so it cannot drift.
//!
//! The list starts small on purpose: commands that change what the user sees, plus a few that read, write or run
//! something, each checked against the extension's own permissions. A command not listed here is refused.

/// The permission a core command implies, on top of listing it in `permissions.commands`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Implies {
    /// It only changes what the user sees.
    Nothing,
    /// `read` on the file named by its argument.
    ReadArgument,
    /// `write` on the file in the active editor.
    WriteActiveFile,
    /// `write` on every file it saves (the open files with unsaved changes).
    WriteUnsaved,
    /// `run` on the program of the task named by its argument (a task made of one program and its arguments).
    RunTask,
}

impl Implies {
    /// How the reference says it.
    pub fn describe(self) -> &'static str {
        match self {
            Implies::Nothing => "nothing",
            Implies::ReadArgument => "`read` on the file",
            Implies::WriteActiveFile => "`write` on the active file",
            Implies::WriteUnsaved => "`write` on every file it saves",
            Implies::RunTask => "`run` on the task's program",
        }
    }
}

/// What a core command takes as its JSON argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Argument {
    /// Nothing (`null`).
    None,
    /// A `file://` URI, required.
    FileUri,
    /// A `file://` URI, or `{"uri": …, "selection": range}` to select a range and scroll to it; required.
    FileLocation,
    /// A task's label; without one the task list opens for the user to choose.
    OptionalTaskLabel,
}

impl Argument {
    pub fn describe(self) -> &'static str {
        match self {
            Argument::None => "none",
            Argument::FileUri => "a `file://` URI (required)",
            Argument::FileLocation => "a `file://` URI, or `{\"uri\": …, \"selection\": range}` (required)",
            Argument::OptionalTaskLabel => "a task's label (optional)",
        }
    }

    /// Whether the command can run with no argument, as menus and key bindings run it.
    pub fn optional(self) -> bool {
        !matches!(self, Argument::FileUri | Argument::FileLocation)
    }
}

/// One core command extensions may run.
#[derive(Clone, Copy, Debug)]
pub struct CoreCommand {
    /// The id without `core:`, as in `keybindings.jsonc`.
    pub id: &'static str,
    /// Its title in the command palette (English), which menus show; empty for one the palette does not list.
    pub title: &'static str,
    /// What it does, for the reference.
    pub summary: &'static str,
    pub argument: Argument,
    pub implies: Implies,
}

const fn c(id: &'static str, title: &'static str, summary: &'static str, argument: Argument, implies: Implies) -> CoreCommand {
    CoreCommand { id, title, summary, argument, implies }
}

/// Every core command extensions may run, in the order the reference lists them.
pub const CORE_COMMANDS: [CoreCommand; 9] = [
    c("workbench.toggleZenMode", "View: Toggle Zen Mode", "Turns Zen mode on or off.", Argument::None, Implies::Nothing),
    c("workbench.openSettingsPage", "Preferences: Open Settings", "Opens the Settings page.", Argument::None, Implies::Nothing),
    c("workbench.showFiles", "View: Files", "Shows the file explorer and moves the keyboard to it.", Argument::None, Implies::Nothing),
    c("workbench.revealActiveFile", "File: Reveal Active File in Explorer", "Shows the active file in the explorer.", Argument::None, Implies::Nothing),
    c("workbench.showProblems", "View: Problems", "Opens the Problems panel.", Argument::None, Implies::Nothing),
    c("workbench.saveAll", "File: Save All", "Saves every open file with unsaved changes.", Argument::None, Implies::WriteUnsaved),
    c("editor.formatDocument", "Editor: Format Document", "Formats the active file.", Argument::None, Implies::WriteActiveFile),
    c(
        "editor.open",
        "",
        "Opens a workspace file in the editor; with a selection, selects that range and scrolls to it.",
        Argument::FileLocation,
        Implies::ReadArgument,
    ),
    c("tasks.run", "Tasks: Run Task…", "Runs the task with this label; with no label, opens the task list.", Argument::OptionalTaskLabel, Implies::RunTask),
];

impl CoreCommand {
    /// The `invalid` message for an argument the command cannot use.
    pub fn bad_argument(&self) -> String {
        format!("core:{} takes {}", self.id, self.argument.describe())
    }
}

/// A line and a column, zero-based, UTF-16 columns.
pub type Position = (u32, u32);

/// `core:editor.open`'s argument: a `file://` URI, or `{"uri": …, "selection": {"start": {"line", "character"}, "end": …}}`.
/// `None` when it is neither (the app, the mock hosts and `cyx test` refuse it with [`CoreCommand::bad_argument`]).
pub fn file_location(arg: &serde_json::Value) -> Option<(String, Option<(Position, Position)>)> {
    if let Some(uri) = arg.as_str() {
        return Some((uri.to_string(), None));
    }
    let uri = arg.get("uri")?.as_str()?.to_string();
    let position = |p: &serde_json::Value| -> Option<Position> {
        Some((u32::try_from(p.get("line")?.as_u64()?).ok()?, u32::try_from(p.get("character")?.as_u64()?).ok()?))
    };
    let selection = match arg.get("selection") {
        None | Some(serde_json::Value::Null) => None,
        Some(s) => Some((position(s.get("start")?)?, position(s.get("end")?)?)),
    };
    Some((uri, selection))
}

/// The listed core command `id` names, with or without `core:`.
pub fn core_command(id: &str) -> Option<&'static CoreCommand> {
    let id = id.strip_prefix("core:").unwrap_or(id);
    CORE_COMMANDS.iter().find(|c| c.id == id)
}

/// `docs/extensions/reference/core-commands.md`.
pub fn reference() -> String {
    let mut out = String::from(
        "# Core commands\n\n<!-- Generated from crates/ext-manifest/src/core_commands.rs: run \
         `UPDATE_DOCS=1 cargo test -p ext-manifest core_commands` after changing it. -->\n\n\
         The Codeyard commands an extension may run. Code runs one with `commands.execute(\"core:<id>\", args)`; it must \
         list the command in `[permissions] commands` and hold the permission the command implies, both in this \
         window (what the user turned off and Workspace Trust count). A command that is not listed here is refused \
         with the fix. A `[[menus]]` entry may name one of these that needs no argument, and `[[keybindings]]` any \
         command of `keybindings.jsonc`; the user's click or key press runs them, so they need no permission.\n\n\
         ```toml\n[permissions]\ncommands = [\"core:workbench.saveAll\"]\nwrite = [\"src/**\"]\n```\n\n\
         | Command | In the palette | Argument | Implies | What it does |\n| --- | --- | --- | --- | --- |\n",
    );
    for c in &CORE_COMMANDS {
        let title = if c.title.is_empty() { "—".to_string() } else { format!("*{}*", c.title) };
        out.push_str(&format!("| `core:{}` | {title} | {} | {} | {} |\n", c.id, c.argument.describe(), c.implies.describe(), c.summary));
    }
    out.push_str(
        "\nA `range` is `{\"start\": {\"line\": 9, \"character\": 0}, \"end\": {\"line\": 9, \"character\": 4}}`: zero-based lines \
         and UTF-16 columns, as everywhere in the API. The selection form of `editor.open` needs Codeyard 0.7.0 (API 1.1); \
         the SDKs' `open_in_editor` and `openInEditor` send it.\n",
    );
    out.push_str(
        "\n`tasks.run` with a label runs only a task whose command is one program with its arguments (no `&&`, pipes \
         or redirections), and that program must be in `run`. A task an extension provides runs in that extension, \
         under its own permissions.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_well_formed() {
        for (i, c) in CORE_COMMANDS.iter().enumerate() {
            assert!(CORE_COMMANDS[..i].iter().all(|o| o.id != c.id), "{} is listed twice", c.id);
            assert!(crate::validate::check_core_command(&format!("core:{}", c.id)).is_ok(), "{}", c.id);
        }
        assert_eq!(core_command("core:workbench.saveAll").map(|c| c.implies), Some(Implies::WriteUnsaved));
        assert!(core_command("workbench.quit").is_none());
    }

    /// `docs/extensions/reference/core-commands.md` is generated from the table.
    #[test]
    fn core_commands_reference() {
        let want = reference();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/extensions/reference/core-commands.md");
        if std::env::var_os("UPDATE_DOCS").is_some() {
            std::fs::write(&path, &want).unwrap();
        }
        // Windows checkouts may turn line ends into CRLF.
        let have = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
        assert!(have == want, "docs/extensions/reference/core-commands.md is out of date: run `UPDATE_DOCS=1 cargo test -p ext-manifest core_commands`");
    }
}
