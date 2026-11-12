//! Changing `keybindings.jsonc` in place for the Keyboard Shortcuts page: entries are read with their position in the
//! file and added, re-keyed or removed as text, so comments, the other entries and the file's line endings stay as they
//! were.

use gpui::Keystroke;
use serde_json::Value;

use crate::settings::{Seg, array_items, members, value_range};

/// One entry of the file, in the order of the file.
#[derive(Debug, Clone, PartialEq)]
pub struct FileEntry {
    /// Position in the array (what `remove_entry` and `set_entry_keys` take).
    pub index: usize,
    /// The keys as `canonical_keys` writes them.
    pub keys: String,
    /// `None`: this entry turns the keys off.
    pub command: Option<String>,
    pub when: Option<String>,
    /// The entry has other members than key, command and when (`args`): removing it loses them.
    pub extras: bool,
}

/// "Cmd+K Cmd+I", "secondary-k" and "cmd-k cmd-i" all become the spelling the keymap reports (`Keystroke::unparse`), so
/// keys from the file and keys from the keymap compare equal. `None` when a chord does not parse.
pub fn canonical_keys(keys: &str) -> Option<String> {
    let converted = if keys.contains('+') { crate::vscode::map_key(keys)? } else { keys.to_string() };
    let chords: Option<Vec<String>> = converted.split_whitespace().map(|chord| Keystroke::parse(chord).ok().map(|k| k.unparse())).collect();
    chords.filter(|c| !c.is_empty()).map(|c| c.join(" "))
}

/// A context as the keymap spells it, so a `when` of the file equals the one of the binding it makes. A context that does
/// not parse is returned as written.
pub fn normalise_when(when: &str) -> String {
    gpui::KeyBindingContextPredicate::parse(when).map_or_else(|_| when.trim().to_string(), |p| p.to_string())
}

/// The entries of the file that name keys. `Err` when the file is not a JSON list (a half-written file): nothing may be
/// written over it then.
pub fn read_entries(text: &str) -> Result<Vec<FileEntry>, String> {
    let value: Value = serde_json::from_str(&theme::themes::strip_jsonc(text)).map_err(|e| e.to_string())?;
    let items = value.as_array().ok_or_else(|| crate::i18n::t("expected a list [ … ]").to_string())?;
    Ok(items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let keys = canonical_keys(item.get("key")?.as_str()?)?;
            let command = match item.get("command")? {
                Value::String(c) => Some(c.clone()),
                Value::Null => None,
                _ => return None,
            };
            let extras = item.as_object().is_some_and(|o| o.keys().any(|k| !matches!(k.as_str(), "key" | "command" | "when")));
            Some(FileEntry { index, keys, command, when: item.get("when").and_then(Value::as_str).map(normalise_when), extras })
        })
        .collect())
}

/// The file's line ending, so a line added to a Windows file is a Windows line.
fn line_ending(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

/// Adds `{ "key": …, "command": …, "when": … }` before the list's closing bracket. The line before gets a comma when it
/// needs one. A missing or empty file becomes a list holding just the entry.
pub fn add_entry(text: &str, keys: &str, command: Option<&str>, when: Option<&str>) -> String {
    insert_entry(text, keys, command, when, false)
}

/// `add_entry`; with `first` the entry goes right after the opening bracket instead. An entry that turns keys off goes
/// first, so it does not also turn off a key the user bound later.
fn insert_entry(text: &str, keys: &str, command: Option<&str>, when: Option<&str>, first: bool) -> String {
    let eol = line_ending(text);
    let json = |v: &str| serde_json::to_string(v).unwrap_or_default();
    let mut entry = format!("{{ \"key\": {}, \"command\": {}", json(keys), command.map_or("null".to_string(), json));
    if let Some(when) = when {
        entry += &format!(", \"when\": {}", json(when));
    }
    entry += " }";
    let line = format!("  {entry},");
    let Some(list) = value_range(text, &[]).filter(|r| text[r.clone()].starts_with('[')) else {
        // Only comments so far: they stay above the new list.
        let comments_only = theme::themes::strip_jsonc(text).trim().is_empty();
        let keep = if comments_only { text } else { "" };
        let sep = if keep.is_empty() || keep.ends_with('\n') { "" } else { eol };
        return format!("{keep}{sep}[{eol}{line}{eol}]{eol}");
    };
    if first {
        let open = list.start + 1;
        return format!("{}{eol}{line}{}", &text[..open], &text[open..]);
    }
    let close = list.end - 1;
    let line_start = text[..close].rfind('\n').map_or(0, |n| n + 1);
    // `]` on its own line: the new line goes before that line; otherwise right before the bracket.
    let at = if text[line_start..close].trim().is_empty() && line_start > list.start { line_start } else { close };
    let build = |head: &str| {
        let sep = if head.ends_with('\n') { "" } else { eol };
        format!("{head}{sep}{line}{eol}{}", &text[at..])
    };
    let parses = |t: &str| serde_json::from_str::<Value>(&theme::themes::strip_jsonc(t)).is_ok();
    let out = build(&text[..at]);
    if parses(&out) {
        return out;
    }
    // The last item has no comma: put one right after it (a comment or blank lines after it stay where they are).
    let last_item = array_items(text, list).and_then(|items| items.last().map(|r| r.end));
    match last_item.filter(|end| *end <= at) {
        Some(end) => build(&format!("{},{}", &text[..end], &text[end..at])),
        None => out,
    }
}

/// Removes entry `index` (as `FileEntry::index` counts) with its own line and comma; `None` when there is no such item.
pub fn remove_entry(text: &str, index: usize) -> Option<String> {
    let item = value_range(text, &[Seg::Index(index)])?;
    let line_start = text[..item.start].rfind('\n').map_or(0, |i| i + 1);
    let start = if text[line_start..item.start].trim().is_empty() { line_start } else { item.start };
    let mut end = item.end + (text[item.end..].len() - text[item.end..].trim_start_matches([' ', '\t']).len());
    if text[end..].starts_with(',') {
        end += 1;
    }
    let line_end = text[end..].find('\n').map_or(text.len(), |i| end + i + 1);
    if start == line_start && text[end..line_end].trim().is_empty() {
        end = line_end;
    }
    Some(format!("{}{}", &text[..start], &text[end..]))
}

/// Gives entry `index` other keys, in place. `None` when the item has no `"key"` string.
pub fn set_entry_keys(text: &str, index: usize, keys: &str) -> Option<String> {
    let item = value_range(text, &[Seg::Index(index)])?;
    let (_, value) = members(text, item)?.into_iter().rev().find(|(k, _)| k == "key")?;
    let json = serde_json::to_string(keys).ok()?;
    Some(format!("{}{json}{}", &text[..value.start], &text[value.end..]))
}

/// What the page asks of the file.
#[derive(Debug, Clone, PartialEq)]
pub enum Change<'a> {
    /// Give `command` the keys `keys`. `replacing`: a key of the command that is dropped for it.
    Set { command: &'a str, keys: &'a str, when: Option<&'a str>, replacing: Option<Key<'a>> },
    /// Drop one key of the command.
    Remove { command: &'a str, key: Key<'a> },
    /// Undo everything the file says about the command. `turned_off`: the default keys it turned off.
    Reset { command: &'a str, turned_off: &'a [Key<'a>] },
}

/// A key a command has now. `user`: it comes from the file (otherwise from a default or an extension).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Key<'a> {
    pub keys: &'a str,
    pub when: Option<&'a str>,
    pub user: bool,
}

fn matches_key(e: &FileEntry, command: Option<&str>, key: &Key) -> bool {
    e.command.as_deref() == command && e.keys == key.keys && e.when.as_deref() == key.when
}

fn find<'e>(entries: &'e [FileEntry], command: Option<&str>, key: &Key) -> Option<&'e FileEntry> {
    entries.iter().find(|e| matches_key(e, command, key))
}

/// The file after `change`. `Err`: the file is not a list or the keys are not keys; nothing is changed then.
pub fn apply(text: &str, change: &Change) -> Result<String, String> {
    let entries = read_entries(text)?;
    let mut out = text.to_string();
    // A key that goes away: the user's own entry is deleted; a default is turned off by an entry that comes first.
    let drop_key = |out: &mut String, command: &str, key: &Key| {
        if key.user {
            // Every identical entry goes (last first, so the places of the others stay true).
            let mut same: Vec<usize> = entries.iter().filter(|e| matches_key(e, Some(command), key)).map(|e| e.index).collect();
            same.sort_unstable_by(|a, b| b.cmp(a));
            for index in same {
                *out = remove_entry(out, index).unwrap_or_else(|| out.clone());
            }
        } else if find(&entries, None, key).is_none() {
            *out = insert_entry(out, key.keys, None, key.when, true);
        }
    };
    match change {
        Change::Set { command, keys, when, replacing } => {
            let new = canonical_keys(keys).ok_or_else(|| crate::i18n::tf("“{}” is not a key", &[keys]))?;
            if replacing.as_ref().is_some_and(|k| k.keys == new) || find(&entries, Some(command), &Key { keys: &new, when: *when, user: true }).is_some() {
                return Ok(out);
            }
            match replacing {
                // The user's own entry changes its keys in place: its place and comments stay.
                Some(old) if old.user && find(&entries, Some(command), old).is_some() => {
                    let entry = find(&entries, Some(command), old).map(|e| e.index).unwrap_or_default();
                    // A later entry for the new keys (a `null`, or another command) would win over the entry in its old place:
                    // then it moves to the end, where it has the last word.
                    if entries.iter().any(|e| e.index > entry && e.keys == new) {
                        drop_key(&mut out, command, old);
                        out = add_entry(&out, &new, Some(command), *when);
                    } else {
                        out = set_entry_keys(&out, entry, &new).ok_or_else(|| crate::i18n::t("An entry of keybindings.jsonc has no key").to_string())?;
                    }
                }
                Some(old) => {
                    drop_key(&mut out, command, old);
                    out = add_entry(&out, &new, Some(command), *when);
                }
                None => out = add_entry(&out, &new, Some(command), *when),
            }
        }
        Change::Remove { command, key } => drop_key(&mut out, command, key),
        Change::Reset { command, turned_off } => {
            let mut doomed: Vec<usize> = entries
                .iter()
                .filter(|e| {
                    e.command.as_deref() == Some(*command)
                        || (e.command.is_none() && turned_off.iter().any(|k| e.keys == k.keys && e.when.as_deref() == k.when))
                })
                .map(|e| e.index)
                .collect();
            doomed.sort_unstable_by(|a, b| b.cmp(a));
            for index in doomed {
                out = remove_entry(&out, index).unwrap_or(out);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key<'a>(keys: &'a str, when: Option<&'a str>, user: bool) -> Key<'a> {
        Key { keys, when, user }
    }

    fn state(text: &str) -> Vec<(String, Option<String>, Option<String>)> {
        parse_back(text).into_iter().map(|e| (e.keys, e.command, e.when)).collect()
    }

    #[test]
    fn recording_a_key_for_a_command_adds_an_entry() {
        let out = apply(WITH_COMMENTS, &Change::Set { command: "editor.undo", keys: "cmd-u", when: None, replacing: None }).unwrap();
        assert!(out.contains("// first") && out.contains("// go quickly"));
        assert_eq!(state(&out).last().unwrap(), &("cmd-u".to_string(), Some("editor.undo".into()), None));
        let again = apply(&out, &Change::Set { command: "editor.undo", keys: "cmd-u", when: None, replacing: None }).unwrap();
        assert_eq!(again, out, "the same key twice changes nothing");
    }

    #[test]
    fn changing_a_users_key_changes_it_in_place_and_changing_a_default_turns_it_off_first() {
        let user = apply(WITH_COMMENTS, &Change::Set { command: "workbench.quickOpen", keys: "cmd-9", when: None, replacing: Some(key("cmd-1", None, true)) })
            .unwrap();
        assert_eq!(user.replace("cmd-9", "cmd-1"), WITH_COMMENTS);
        let default = apply(
            WITH_COMMENTS,
            &Change::Set { command: "editor.undo", keys: "cmd-u", when: Some("Editor"), replacing: Some(key("cmd-z", Some("Editor"), false)) },
        )
        .unwrap();
        let s = state(&default);
        assert_eq!(s.first().unwrap(), &("cmd-z".to_string(), None, Some("Editor".to_string())), "the entry that turns the default off comes first: {default}");
        assert_eq!(s.last().unwrap().1.as_deref(), Some("editor.undo"));
        assert_eq!(s.len(), 4);
    }

    #[test]
    fn removing_a_default_key_turns_it_off_once_and_removing_a_users_key_deletes_it() {
        let off = apply("[]", &Change::Remove { command: "editor.undo", key: key("cmd-z", None, false) }).unwrap();
        assert_eq!(state(&off), [("cmd-z".to_string(), None, None)]);
        assert_eq!(apply(&off, &Change::Remove { command: "editor.undo", key: key("cmd-z", None, false) }).unwrap(), off);
        let gone = apply(WITH_COMMENTS, &Change::Remove { command: "editor.save", key: key("cmd-2", Some("Editor"), true) }).unwrap();
        assert_eq!(state(&gone).len(), 1);
        assert!(gone.contains("// { \"key\": \"cmd-3\""));
    }

    #[test]
    fn reset_removes_the_commands_entries_and_the_nulls_that_turned_its_defaults_off() {
        let text = "[\n  { \"key\": \"cmd-z\", \"command\": null, \"when\": \"Editor\" },\n  { \"key\": \"cmd-u\", \"command\": \"editor.undo\" },\n  { \"key\": \"cmd-9\", \"command\": \"editor.save\" },\n  { \"key\": \"cmd-q\", \"command\": null }\n]\n";
        let out = apply(text, &Change::Reset { command: "editor.undo", turned_off: &[key("cmd-z", Some("Editor"), false)] }).unwrap();
        assert_eq!(state(&out), [("cmd-9".into(), Some("editor.save".into()), None), ("cmd-q".into(), None, None)]);
    }

    #[test]
    fn a_key_moves_to_the_end_when_a_later_entry_would_kill_it_in_place() {
        let text = "[ {\"key\":\"cmd-1\",\"command\":\"editor.save\"}, {\"key\":\"cmd-2\",\"command\":null} ]";
        let out = apply(text, &Change::Set { command: "editor.save", keys: "cmd-2", when: None, replacing: Some(key("cmd-1", None, true)) }).unwrap();
        let s = state(&out);
        assert_eq!(s.last().unwrap(), &("cmd-2".to_string(), Some("editor.save".into()), None), "{out}");
        assert_eq!(s.len(), 2, "the old entry is gone: {out}");
        // With nothing later on the new key the entry stays where it is.
        let calm = apply(text, &Change::Set { command: "editor.save", keys: "cmd-3", when: None, replacing: Some(key("cmd-1", None, true)) }).unwrap();
        assert_eq!(calm.replace("cmd-3", "cmd-1"), text);
    }

    #[test]
    fn reset_keeps_a_global_null_of_the_user_when_the_default_has_a_context() {
        let text = "[ {\"key\":\"cmd-z\",\"command\":null}, {\"key\":\"cmd-z\",\"command\":null,\"when\":\"Editor\"}, {\"key\":\"cmd-u\",\"command\":\"editor.undo\"} ]";
        let out = apply(text, &Change::Reset { command: "editor.undo", turned_off: &[key("cmd-z", Some("Editor"), false)] }).unwrap();
        assert_eq!(state(&out), [("cmd-z".to_string(), None, None)], "{out}");
        let plain = "[ {\"key\":\"cmd-z\",\"command\":null}, {\"key\":\"cmd-q\",\"command\":null,\"when\":\"Editor\"} ]";
        let out = apply(plain, &Change::Reset { command: "editor.undo", turned_off: &[key("cmd-z", None, false)] }).unwrap();
        assert_eq!(state(&out), [("cmd-q".to_string(), None, Some("Editor".to_string()))]);
    }

    #[test]
    fn removing_deletes_every_identical_entry_and_reading_notes_extra_members() {
        let text = "[ {\"key\":\"cmd-1\",\"command\":\"a\"}, {\"key\":\"cmd-2\",\"command\":\"b\"}, {\"key\":\"cmd-1\",\"command\":\"a\",\"args\":{\"x\":1}} ]";
        let entries = read_entries(text).unwrap();
        assert_eq!(entries.iter().map(|e| e.extras).collect::<Vec<_>>(), [false, false, true]);
        let out = apply(text, &Change::Remove { command: "a", key: key("cmd-1", None, true) }).unwrap();
        assert_eq!(state(&out), [("cmd-2".to_string(), Some("b".into()), None)], "{out}");
    }

    #[test]
    fn a_broken_file_is_never_written_over() {
        assert!(apply("[ { ", &Change::Remove { command: "a", key: key("cmd-1", None, true) }).is_err());
        assert!(apply("[]", &Change::Set { command: "a", keys: "", when: None, replacing: None }).is_err());
    }

    const WITH_COMMENTS: &str = "// my keys\n[\n  // go quickly\n  { \"key\": \"cmd-1\", \"command\": \"workbench.quickOpen\" }, // first\n  { \"key\": \"cmd-2\", \"command\": \"editor.save\", \"when\": \"Editor\" }\n  // { \"key\": \"cmd-3\", \"command\": \"x\" },\n]\n";

    fn parse_back(text: &str) -> Vec<FileEntry> {
        read_entries(text).unwrap_or_else(|e| panic!("{e}: {text}"))
    }

    #[test]
    fn keys_are_spelled_one_way() {
        assert_eq!(canonical_keys("cmd-k cmd-i"), canonical_keys("Cmd+K Cmd+I"));
        assert_eq!(canonical_keys("secondary-k"), canonical_keys(if cfg!(target_os = "macos") { "cmd-k" } else { "ctrl-k" }));
        assert_eq!(canonical_keys("shift-cmd-k"), canonical_keys("cmd-shift-k"));
        assert_eq!(canonical_keys("ctrl-ö").as_deref(), Some("ctrl-ö"));
        assert_eq!(canonical_keys(""), None);
    }

    #[test]
    fn entries_are_read_with_their_place_in_the_list() {
        let entries = parse_back(WITH_COMMENTS);
        assert_eq!(entries.len(), 2);
        assert_eq!((entries[1].index, entries[1].command.as_deref(), entries[1].when.as_deref()), (1, Some("editor.save"), Some("Editor")));
        let off = parse_back(r#"[ { "key": "cmd-s", "command": null }, { "key": "oops" }, { "key": "cmd-d", "command": "a" } ]"#);
        assert_eq!(off.iter().map(|e| (e.index, e.command.clone())).collect::<Vec<_>>(), [(0, None), (2, Some("a".into()))]);
        assert!(read_entries("[ {").is_err());
        assert!(read_entries("{}").is_err());
    }

    #[test]
    fn adding_keeps_comments_and_the_other_entries() {
        let out = add_entry(WITH_COMMENTS, "cmd-k cmd-i", Some("editor.showHover"), Some("Editor"));
        assert!(out.starts_with("// my keys\n[\n  // go quickly\n") && out.contains("// first") && out.contains("// { \"key\": \"cmd-3\""), "{out}");
        let entries = parse_back(&out);
        assert_eq!(entries.len(), 3);
        assert_eq!(
            (entries[2].keys.as_str(), entries[2].command.as_deref(), entries[2].when.as_deref()),
            ("cmd-k cmd-i", Some("editor.showHover"), Some("Editor"))
        );
        assert_eq!(&out[..out.len() - 1].lines().last().unwrap()[..1], "]");
    }

    #[test]
    fn adding_after_an_item_without_a_comma_adds_the_comma() {
        let out = add_entry("[\n  { \"key\": \"cmd-1\", \"command\": \"a\" }\n]\n", "cmd-2", Some("b"), None);
        assert_eq!(parse_back(&out).len(), 2, "{out}");
        assert!(out.contains("\"a\" },\n"), "{out}");
        let one_line = add_entry(r#"[ { "key": "cmd-1", "command": "a" } ]"#, "cmd-2", None, None);
        assert_eq!(parse_back(&one_line).iter().map(|e| e.command.clone()).collect::<Vec<_>>(), [Some("a".into()), None], "{one_line}");
    }

    #[test]
    fn adding_to_a_missing_or_empty_file_makes_a_list() {
        for start in ["", "   \n", "[]", "[\n]\n", "// only a comment\n[\n]"] {
            let out = add_entry(start, "cmd-1", Some("a"), None);
            assert_eq!(parse_back(&out).len(), 1, "{start:?} -> {out}");
        }
        assert!(add_entry("// only a comment\n[\n]", "cmd-1", Some("a"), None).starts_with("// only a comment\n"));
    }

    #[test]
    fn windows_line_endings_stay_windows_line_endings() {
        let crlf = WITH_COMMENTS.replace('\n', "\r\n");
        let added = add_entry(&crlf, "cmd-4", Some("a"), None);
        assert_eq!(added.matches('\n').count(), added.matches("\r\n").count(), "a bare LF crept in: {added:?}");
        assert_eq!(parse_back(&added).len(), 3);
        let removed = remove_entry(&added, 0).unwrap();
        assert_eq!(removed.matches('\n').count(), removed.matches("\r\n").count(), "{removed:?}");
        assert_eq!(parse_back(&removed).len(), 2);
        let from_empty = add_entry("[\r\n]\r\n", "cmd-4", Some("a"), None);
        assert_eq!(from_empty.matches('\n').count(), from_empty.matches("\r\n").count(), "{from_empty:?}");
    }

    #[test]
    fn removing_takes_the_entry_and_nothing_else() {
        let out = remove_entry(WITH_COMMENTS, 0).unwrap();
        assert_eq!(parse_back(&out).len(), 1);
        assert!(out.contains("// go quickly") && out.contains("// { \"key\": \"cmd-3\"") && !out.contains("cmd-1"), "{out}");
        let last = remove_entry(WITH_COMMENTS, 1).unwrap();
        assert_eq!(parse_back(&last).iter().map(|e| e.keys.as_str()).collect::<Vec<_>>(), ["cmd-1"]);
        assert!(remove_entry(WITH_COMMENTS, 5).is_none());
        let single_line = remove_entry(r#"[ { "key": "cmd-1", "command": "a" }, { "key": "cmd-2", "command": "b" } ]"#, 0).unwrap();
        assert_eq!(parse_back(&single_line).len(), 1, "{single_line}");
    }

    #[test]
    fn rekeying_changes_only_the_key() {
        let out = set_entry_keys(WITH_COMMENTS, 1, "ctrl-ö").unwrap();
        assert_eq!(out.replace("ctrl-ö", "cmd-2"), WITH_COMMENTS);
        assert_eq!(parse_back(&out)[1].keys, "ctrl-ö");
        assert!(set_entry_keys(r#"[ { "command": "a" } ]"#, 0, "cmd-1").is_none());
    }

    #[test]
    fn special_characters_in_a_when_are_escaped() {
        let out = add_entry("[]", "cmd-1", Some("a"), Some(r#"Editor && !mode == "x\y""#));
        assert_eq!(parse_back(&out)[0].when.as_deref(), Some(r#"Editor && !mode == "x\y""#));
    }
}
