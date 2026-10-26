//! Terminal profiles: named shells with arguments and environment variables, per operating system of the machine the
//! terminal runs on (`terminal.profiles.macos`, `.linux`, `.windows`), and the one new terminals use
//! (`terminal.defaultProfile`). A profile is a program to run, so it is checked like any setting from a file: the
//! path is one line, names of environment variables are plain, and a project file needs a trusted folder.

use serde_json::{Map, Value};

/// Colours a terminal can be marked with (theme roles, so they follow the theme).
pub const COLORS: [&str; 6] = ["accent", "success", "warning", "danger", "info", "muted"];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    /// Program: an absolute path or a name found on PATH.
    pub path: String,
    /// `None`: a login shell (`-l`).
    pub args: Option<Vec<String>>,
    pub env: Vec<(String, String)>,
    /// One of `COLORS`.
    pub color: Option<String>,
}

/// Settings key suffix for an `std::env::consts::OS` value.
pub fn os_key(os: &str) -> &'static str {
    match os {
        "macos" => "macos",
        "windows" => "windows",
        _ => "linux",
    }
}

fn plain_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Profiles of one `terminal.profiles.<os>` object: `{ "zsh": { "path": "/bin/zsh", "args": ["-l"], "env": {…}, "color": "info" } }`.
/// A bad entry is skipped with a warning (`key` names it).
pub fn parse_profiles(key: &str, map: &Map<String, Value>, warn: &mut Vec<String>) -> Vec<Profile> {
    let mut out = Vec::new();
    for (name, v) in map {
        let bad = |what: &str, warn: &mut Vec<String>| warn.push(format!("{key}.{name}: {what}"));
        let Some(obj) = v.as_object() else {
            bad(crate::i18n::t("expected an object with path, args, env or color"), warn);
            continue;
        };
        let Some(path) = obj.get("path").and_then(Value::as_str).map(str::trim).filter(|p| !p.is_empty() && !p.contains(['\n', '\r', '\0'])) else {
            bad(crate::i18n::t("needs a path to a program"), warn);
            continue;
        };
        let args = match obj.get("args") {
            None => None,
            Some(Value::Array(a)) if a.iter().all(Value::is_string) => Some(a.iter().filter_map(Value::as_str).map(str::to_string).collect()),
            Some(_) => {
                bad(crate::i18n::t("args must be a list of texts"), warn);
                continue;
            }
        };
        let mut env = Vec::new();
        if let Some(e) = obj.get("env") {
            let Some(e) = e.as_object() else {
                bad(crate::i18n::t("env must be an object of texts"), warn);
                continue;
            };
            for (k, v) in e {
                match v.as_str() {
                    Some(v) if plain_name(k) && !v.contains('\0') => env.push((k.clone(), v.to_string())),
                    _ => bad(&crate::i18n::tf("env.{} is not used: a name of letters, digits and _ with a text value", &[k]), warn),
                }
            }
        }
        let color = match obj.get("color").and_then(Value::as_str) {
            Some(c) if COLORS.contains(&c) => Some(c.to_string()),
            Some(c) => {
                bad(&crate::i18n::tf("color “{}” is not one of accent, success, warning, danger, info, muted", &[&c]), warn);
                None
            }
            None => None,
        };
        out.push(Profile { name: name.clone(), path: path.to_string(), args, env, color });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_are_read_and_bad_entries_reported() {
        let v: Value = serde_json::from_str(
            r#"{ "zsh": { "path": "/bin/zsh", "args": ["-l", "-i"], "env": { "EDITOR": "vim", "BAD-NAME": "x", "N": 3 }, "color": "info" },
                 "plain": { "path": "bash" }, "nopath": {}, "evil": { "path": "/bin/sh\n-c" }, "argsbad": { "path": "sh", "args": "-l" },
                 "odd": 3, "tinted": { "path": "fish", "color": "pink" } }"#,
        )
        .unwrap();
        let mut warn = Vec::new();
        let list = parse_profiles("terminal.profiles.macos", v.as_object().unwrap(), &mut warn);
        assert_eq!(list.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), vec!["zsh", "plain", "tinted"], "in the file's order");
        let zsh = &list[0];
        assert_eq!(
            (zsh.args.as_deref(), zsh.env.as_slice(), zsh.color.as_deref()),
            (Some(&["-l".to_string(), "-i".to_string()][..]), &[("EDITOR".to_string(), "vim".to_string())][..], Some("info"))
        );
        assert_eq!(list[1].args, None, "no args: a login shell");
        assert_eq!(list[2].color, None, "an unknown colour is dropped");
        assert_eq!(warn.len(), 7, "{warn:?}");
        assert_eq!((os_key("macos"), os_key("linux"), os_key("freebsd"), os_key("windows")), ("macos", "linux", "linux", "windows"));
    }
}
