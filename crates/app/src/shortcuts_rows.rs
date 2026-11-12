//! What the Keyboard Shortcuts page lists: one row per key a command has (one row without a key for a command that has
//! none), where each key comes from, and which keys several commands share. Built from the live keymap plus the entries
//! of `keybindings.jsonc`, so the list never differs from what a pressed key does.

use gpui::{Action, KeyBinding, Keymap, Keystroke};

use super::edit::{FileEntry, canonical_keys};

/// Which layer a key comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Default,
    /// A keymap preset or a key binding an extension ships.
    Extension,
    /// An entry of `keybindings.jsonc`.
    User,
}

/// A command the page can list. `id` is the name `keybindings.jsonc` uses; without one no key can be written for it.
pub struct Command {
    pub label: String,
    pub id: Option<String>,
    pub action: Box<dyn Action>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Binding {
    /// As `canonical_keys` spells it ("cmd-k cmd-i").
    pub keys: String,
    /// For the eye ("⌘K ⌘I", "Ctrl+K Ctrl+I").
    pub display: String,
    pub when: Option<String>,
    pub source: Source,
    /// The keys are turned off by an entry of the file; `entry` is that entry.
    pub disabled: bool,
    /// The entry of `keybindings.jsonc` that gives (or, when `disabled`, removes) this key.
    pub entry: Option<usize>,
    /// The other commands that react to the same keys (or to the start of them) in an overlapping context.
    pub conflicts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub label: String,
    pub id: Option<String>,
    pub binding: Option<Binding>,
    /// The file changes something for this command: Reset has an effect.
    pub changed: bool,
}

/// A keymap binding as the page reads it.
struct Live<'a> {
    keys: String,
    when: Option<String>,
    binding: &'a KeyBinding,
    no_action: bool,
}

fn live(keymap: &Keymap) -> Vec<Live<'_>> {
    keymap
        .bindings()
        .map(|b| Live {
            keys: b.keystrokes().iter().map(|k| k.inner().unparse()).collect::<Vec<_>>().join(" "),
            when: b.predicate().map(|p| p.to_string()),
            binding: b,
            no_action: gpui::is_no_action(b.action()),
        })
        .collect()
}

/// Two bindings can both be candidates for one key press: one has no context, or both have the same one.
fn overlaps(a: &Option<String>, b: &Option<String>) -> bool {
    a.is_none() || b.is_none() || a == b
}

/// The keys are the same or one is the start of the other (a chord whose first key is also a binding of its own).
fn clash(a: &str, b: &str) -> bool {
    a == b || a.strip_prefix(b).is_some_and(|rest| rest.starts_with(' ')) || b.strip_prefix(a).is_some_and(|rest| rest.starts_with(' '))
}

fn entry_matches(entry: &FileEntry, command: Option<&str>, keys: &str, when: &Option<String>) -> bool {
    entry.command.as_deref() == command && entry.keys == keys && (entry.command.is_none() && entry.when.is_none() || &entry.when == when)
}

pub fn build(commands: &[Command], keymap: &Keymap, user: &[FileEntry], extension: &[FileEntry]) -> Vec<Row> {
    let all = live(keymap);
    // Which binding belongs to which listed command (by position), whether it is in force, and who made it.
    struct Mine {
        command: usize,
        at: usize,
        enabled: bool,
    }
    let mut mine: Vec<Mine> = Vec::new();
    for (ci, command) in commands.iter().enumerate() {
        let enabled: Vec<&KeyBinding> = keymap.bindings_for_action(command.action.as_ref()).collect();
        for (at, l) in all.iter().enumerate().filter(|(_, l)| !l.no_action && l.binding.action().partial_eq(command.action.as_ref())) {
            mine.push(Mine { command: ci, at, enabled: enabled.iter().any(|b| std::ptr::eq(*b, l.binding)) });
        }
    }
    // A listed command's bindings, and the others' (an action no command lists), for finding who shares a key.
    let listed: std::collections::HashSet<usize> = mine.iter().map(|m| m.at).collect();
    let in_force = |at: usize| -> bool {
        let l = &all[at];
        !all.iter().skip(at + 1).any(|n| n.no_action && n.keys == l.keys && (n.when.is_none() || n.when == l.when))
    };
    let mut users: Vec<(usize, String, &Live)> = mine
        .iter()
        .filter(|m| m.enabled)
        .map(|m| (m.at, commands[m.command].label.clone(), &all[m.at]))
        .chain(
            all.iter()
                .enumerate()
                .filter(|(at, l)| !l.no_action && !listed.contains(at) && in_force(*at))
                .map(|(at, l)| (at, l.binding.action().name().to_string(), l)),
        )
        .collect();
    users.sort_by_key(|u| u.0);
    let conflicts_of = |at: usize, label: &str| -> Vec<String> {
        let me = &all[at];
        let mut out: Vec<String> = Vec::new();
        for (other, other_label, l) in &users {
            // The same command under another key is not a clash, and neither is a command that appears twice.
            if *other != at && other_label != label && clash(&me.keys, &l.keys) && overlaps(&me.when, &l.when) && !out.contains(other_label) {
                out.push(other_label.clone());
            }
        }
        out
    };

    let mut rows = Vec::new();
    for (ci, command) in commands.iter().enumerate() {
        let id = command.id.as_deref();
        let changed = user.iter().any(|e| id.is_some() && e.command.as_deref() == id)
            || mine.iter().any(|m| m.command == ci && !m.enabled && user.iter().any(|e| e.command.is_none() && e.keys == all[m.at].keys));
        let mut bindings = Vec::new();
        for m in mine.iter().filter(|m| m.command == ci) {
            let l = &all[m.at];
            let display = l.binding.keystrokes().iter().map(|k| crate::commands::format_keystroke(k.inner())).collect::<Vec<_>>().join(" ");
            let (source, entry, disabled) = if m.enabled {
                match (user.iter().find(|e| entry_matches(e, id, &l.keys, &l.when)), extension.iter().any(|e| entry_matches(e, id, &l.keys, &l.when))) {
                    (Some(e), _) => (Source::User, Some(e.index), false),
                    (None, true) => (Source::Extension, None, false),
                    (None, false) => (Source::Default, None, false),
                }
            } else {
                // Turned off: by the user's entry when there is one (otherwise by a layer below the file, which is not listed).
                match user.iter().find(|e| e.command.is_none() && e.keys == l.keys && (e.when.is_none() || e.when == l.when)) {
                    Some(e) => (Source::User, Some(e.index), true),
                    None => continue,
                }
            };
            let conflicts = if disabled { Vec::new() } else { conflicts_of(m.at, &command.label) };
            bindings.push(Binding { keys: l.keys.clone(), display, when: l.when.clone(), source, disabled, entry, conflicts });
        }
        if bindings.is_empty() {
            rows.push(Row { label: command.label.clone(), id: command.id.clone(), binding: None, changed });
        }
        rows.extend(bindings.into_iter().map(|b| Row { label: command.label.clone(), id: command.id.clone(), binding: Some(b), changed }));
    }
    rows.sort_by(|a, b| {
        b.binding.is_some().cmp(&a.binding.is_some()).then_with(|| a.label.cmp(&b.label)).then_with(|| {
            let key = |r: &Row| r.binding.as_ref().map(|b| (b.disabled, b.keys.clone()));
            key(a).cmp(&key(b))
        })
    });
    rows
}

/// The commands that have a key in more than one place: which row in `rows` clash with which.
pub fn conflict_count(rows: &[Row]) -> usize {
    rows.iter().filter(|r| r.binding.as_ref().is_some_and(|b| !b.conflicts.is_empty())).count()
}

/// Keys as the file spells them ("cmd-k cmd-i") for the eye ("⌘K ⌘I").
pub fn display_keys(keys: &str) -> String {
    keys.split_whitespace()
        .map(|chord| Keystroke::parse(chord).map_or_else(|_| chord.to_string(), |k| crate::commands::format_keystroke(&k)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The text of a source, as the page's column says it.
pub fn source_name(source: Source) -> std::borrow::Cow<'static, str> {
    match source {
        Source::Default => crate::i18n::t("Default").into(),
        Source::Extension => crate::i18n::t("Extension").into(),
        Source::User => crate::i18n::t("User").into(),
    }
}

/// Whether a row matches the search: the command's name or id, its keys (as shown or as typed in the file), its context
/// or where it comes from. Case does not matter; "cmd+shift+p" finds ⇧⌘P.
pub fn matches(row: &Row, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    if row.label.to_lowercase().contains(&q) || row.id.as_ref().is_some_and(|id| id.to_lowercase().contains(&q)) {
        return true;
    }
    let Some(b) = &row.binding else { return false };
    let typed = canonical_keys(&q).filter(|_| Keystroke::parse(q.split_whitespace().next().unwrap_or_default()).is_ok());
    b.display.to_lowercase().contains(&q)
        || b.keys.contains(&q)
        || typed.is_some_and(|t| b.keys.contains(&t))
        || b.when.as_ref().is_some_and(|w| w.to_lowercase().contains(&q))
        || source_name(b.source).to_lowercase().contains(&q)
        || (!b.conflicts.is_empty() && "conflict".contains(&q))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workbench as wb;

    fn command(label: &str, id: &str, action: impl Action) -> Command {
        Command { label: label.into(), id: Some(id.into()), action: Box::new(action) }
    }

    fn entry(index: usize, keys: &str, command: Option<&str>, when: Option<&str>) -> FileEntry {
        FileEntry { index, keys: canonical_keys(keys).unwrap(), command: command.map(Into::into), when: when.map(Into::into), extras: false }
    }

    fn keymap(bindings: Vec<KeyBinding>) -> Keymap {
        Keymap::new(bindings)
    }

    fn only(rows: &[Row], label: &str) -> Binding {
        let mut found = rows.iter().filter(|r| r.label == label);
        let row = found.next().unwrap_or_else(|| panic!("no row {label}"));
        row.binding.clone().unwrap_or_else(|| panic!("{label} has no key"))
    }

    #[test]
    fn a_command_without_a_key_has_one_row_without_one() {
        let cmds = [command("Quick Open", "workbench.quickOpen", wb::QuickOpen), command("Go to Line", "workbench.goToLine", wb::GoToLine)];
        let rows = build(&cmds, &keymap(vec![KeyBinding::new("cmd-p", wb::QuickOpen, None)]), &[], &[]);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].label.as_str(), rows[0].binding.as_ref().map(|b| b.source)), ("Quick Open", Some(Source::Default)));
        assert_eq!((rows[1].label.as_str(), rows[1].binding.is_none()), ("Go to Line", true), "bound commands come first");
    }

    #[test]
    fn a_command_with_two_keys_has_two_rows_and_the_file_marks_its_own() {
        let cmds = [command("Quick Open", "workbench.quickOpen", wb::QuickOpen)];
        let km = keymap(vec![KeyBinding::new("cmd-p", wb::QuickOpen, None), KeyBinding::new("cmd-alt-o", wb::QuickOpen, None)]);
        let rows = build(&cmds, &km, &[entry(3, "alt-cmd-o", Some("workbench.quickOpen"), None)], &[]);
        assert_eq!(rows.len(), 2);
        let sources: Vec<_> =
            rows.iter().map(|r| (r.binding.as_ref().unwrap().keys.clone(), r.binding.as_ref().unwrap().source, r.binding.as_ref().unwrap().entry)).collect();
        assert!(sources.contains(&("cmd-p".into(), Source::Default, None)) && sources.contains(&("alt-cmd-o".into(), Source::User, Some(3))), "{sources:?}");
        assert!(rows.iter().all(|r| r.changed));
    }

    #[test]
    fn the_same_key_in_two_commands_is_named_on_both_sides() {
        let cmds = [command("Quick Open", "workbench.quickOpen", wb::QuickOpen), command("Go to Line", "workbench.goToLine", wb::GoToLine)];
        let km = keymap(vec![KeyBinding::new("cmd-p", wb::QuickOpen, None), KeyBinding::new("cmd-p", wb::GoToLine, None)]);
        let rows = build(&cmds, &km, &[], &[]);
        assert_eq!(only(&rows, "Quick Open").conflicts, ["Go to Line"]);
        assert_eq!(only(&rows, "Go to Line").conflicts, ["Quick Open"]);
        assert_eq!(conflict_count(&rows), 2);
    }

    #[test]
    fn different_contexts_do_not_clash_but_no_context_clashes_with_every_context() {
        let cmds = [command("Quick Open", "workbench.quickOpen", wb::QuickOpen), command("Go to Line", "workbench.goToLine", wb::GoToLine)];
        let apart = keymap(vec![KeyBinding::new("cmd-p", wb::QuickOpen, Some("Editor")), KeyBinding::new("cmd-p", wb::GoToLine, Some("Terminal"))]);
        assert_eq!(conflict_count(&build(&cmds, &apart, &[], &[])), 0);
        let shadowed = keymap(vec![KeyBinding::new("cmd-p", wb::QuickOpen, Some("Editor")), KeyBinding::new("cmd-p", wb::GoToLine, None)]);
        assert_eq!(conflict_count(&build(&cmds, &shadowed, &[], &[])), 2);
    }

    #[test]
    fn a_key_that_starts_a_chord_clashes_with_the_chord() {
        let cmds = [command("Quick Open", "workbench.quickOpen", wb::QuickOpen), command("Go to Line", "workbench.goToLine", wb::GoToLine)];
        let km = keymap(vec![KeyBinding::new("cmd-k", wb::QuickOpen, None), KeyBinding::new("cmd-k cmd-i", wb::GoToLine, None)]);
        let rows = build(&cmds, &km, &[], &[]);
        assert_eq!(only(&rows, "Go to Line").conflicts, ["Quick Open"]);
        let apart = keymap(vec![KeyBinding::new("cmd-k", wb::QuickOpen, None), KeyBinding::new("cmd-j cmd-i", wb::GoToLine, None)]);
        assert_eq!(conflict_count(&build(&cmds, &apart, &[], &[])), 0);
    }

    #[test]
    fn a_command_does_not_clash_with_itself() {
        let cmds = [command("Quick Open", "workbench.quickOpen", wb::QuickOpen)];
        let km = keymap(vec![KeyBinding::new("cmd-p", wb::QuickOpen, None), KeyBinding::new("cmd-p", wb::QuickOpen, Some("Editor"))]);
        assert_eq!(conflict_count(&build(&cmds, &km, &[], &[])), 0);
    }

    #[test]
    fn a_key_used_by_an_action_no_command_lists_is_named_by_the_action() {
        let cmds = [command("Quick Open", "workbench.quickOpen", wb::QuickOpen)];
        let km = keymap(vec![KeyBinding::new("cmd-p", wb::QuickOpen, None), KeyBinding::new("cmd-p", wb::GoToLine, None)]);
        let rows = build(&cmds, &km, &[], &[]);
        assert_eq!(only(&rows, "Quick Open").conflicts.len(), 1);
        assert!(only(&rows, "Quick Open").conflicts[0].contains("GoToLine"), "{:?}", only(&rows, "Quick Open").conflicts);
    }

    #[test]
    fn a_key_the_user_turned_off_is_listed_as_off_and_does_not_clash() {
        let cmds = [command("Quick Open", "workbench.quickOpen", wb::QuickOpen), command("Go to Line", "workbench.goToLine", wb::GoToLine)];
        let km = keymap(vec![
            KeyBinding::new("cmd-p", wb::QuickOpen, None),
            KeyBinding::new("cmd-p", wb::GoToLine, None),
            KeyBinding::load("cmd-p", Box::new(gpui::NoAction), None, false, None, &gpui::DummyKeyboardMapper).unwrap(),
            KeyBinding::new("cmd-j", wb::GoToLine, None),
        ]);
        let rows = build(&cmds, &km, &[entry(0, "cmd-p", None, None)], &[]);
        let off: Vec<&Row> = rows.iter().filter(|r| r.binding.as_ref().is_some_and(|b| b.disabled)).collect();
        assert_eq!(off.len(), 2, "{rows:?}");
        assert!(off.iter().all(|r| r.binding.as_ref().unwrap().entry == Some(0) && r.changed));
        assert_eq!(conflict_count(&rows), 0, "a key that is off cannot clash");
        let go: Vec<_> = rows.iter().filter(|r| r.label == "Go to Line").map(|r| r.binding.as_ref().unwrap()).collect();
        assert!(go.iter().any(|b| b.keys == "cmd-j" && !b.disabled), "the other key of the command is still on");
    }

    #[test]
    fn extension_keys_are_told_from_default_and_user_keys() {
        let cmds = [command("Quick Open", "workbench.quickOpen", wb::QuickOpen)];
        let km = keymap(vec![KeyBinding::new("cmd-p", wb::QuickOpen, None)]);
        let rows = build(&cmds, &km, &[], &[entry(0, "cmd-p", Some("workbench.quickOpen"), None)]);
        assert_eq!(only(&rows, "Quick Open").source, Source::Extension);
    }

    #[test]
    fn search_finds_by_name_id_keys_context_and_source() {
        let cmds = [command("Go to Line…", "workbench.goToLine", wb::GoToLine)];
        let km = keymap(vec![KeyBinding::new("cmd-shift-g", wb::GoToLine, Some("Editor"))]);
        let rows = build(&cmds, &km, &[], &[]);
        let row = &rows[0];
        for q in ["line", "GOTOLINE", "editor", "default", "cmd-shift-g", "cmd+shift+g", ""] {
            assert!(matches(row, q), "{q}");
        }
        assert!(!matches(row, "terminal") && !matches(row, "cmd-p") && !matches(row, "user"));
        assert!(matches(&Row { label: "x".into(), id: None, binding: None, changed: false }, "x"));
        assert!(!matches(&Row { label: "x".into(), id: None, binding: None, changed: false }, "cmd-p"));
    }
}
