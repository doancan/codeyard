//! `keys.rs`: the built-in key list is current, keys match in any spelling, language typos are found.

use super::*;

#[test]
fn reference_builtin_keys() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = std::fs::read_to_string(root.join("../app/src/main.rs")).unwrap();
    let keys = keys_in_source(&source);
    assert!(keys.len() > 100, "the app's bindings were not found: {}", keys.len());
    let want: String = keys.iter().map(|k| format!("{k}\n")).collect();
    let path = root.join("src/builtin_keys.txt");
    if std::env::var_os("UPDATE_DOCS").is_some() {
        std::fs::write(&path, &want).unwrap();
    }
    let have = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
    assert!(have == want, "crates/cyx/src/builtin_keys.txt is out of date: run `UPDATE_DOCS=1 cargo test -p cyx reference`");
}

#[test]
fn keys_are_read_from_the_source() {
    let src = r#"cx.bind_keys([KeyBinding::new("cmd-p", A, None), KeyBinding::new("cmd-\\", C, None), KeyBinding::new(if cfg!(target_os = "macos") { "ctrl-shift-d" } else { "alt-shift-d" }, B, Some("Editor"))]);"#;
    assert_eq!(keys_in_source(src), ["alt-shift-d", "cmd-\\", "cmd-p", "ctrl-shift-d"]);
}

#[test]
fn built_in_keys_are_found_in_any_spelling() {
    // A key the app binds with `secondary-p` on every platform (Go to File).
    assert!(BUILTIN_KEYS.lines().any(|l| l == "secondary-p"), "the fixture key moved: pick another");
    assert!(is_builtin_key("cmd-p") && is_builtin_key("ctrl-p") && is_builtin_key("super-p"));
    assert!(!is_builtin_key("cmd-alt-shift-z"), "the template's key is free");
    assert_eq!(stroke("shift-cmd-k", "cmd"), stroke("cmd-shift-k", "cmd"));
    assert_eq!(stroke("ctrl--", "cmd"), "ctrl--");
}

#[test]
fn typos_of_language_ids_are_suggested() {
    assert_eq!(closest_language("rustt"), Some("rust"));
    assert_eq!(closest_language("pyhton"), Some("python"));
    assert_eq!(closest_language("brainfuck"), None);
    assert_eq!(distance("kitten", "sitting"), 3);
}

/// The key sequences of the `KeyBinding::new(…)` calls in the app's source: the string literals of the first argument
/// (both of a `if cfg!(…) { "a" } else { "b" }`) that are keys, sorted and deduplicated.
fn keys_in_source(source: &str) -> Vec<String> {
    const CALL: &str = "KeyBinding::new(";
    let mut out = Vec::new();
    for (at, _) in source.match_indices(CALL) {
        let (mut depth, mut literal, mut in_string) = (0usize, String::new(), false);
        let mut chars = source[at + CALL.len()..].chars();
        while let Some(c) = chars.next() {
            match (in_string, c) {
                // `"cmd-\\"` is the backslash key.
                (true, '\\') => literal.extend(chars.next()),
                (true, '"') => {
                    in_string = false;
                    out.push(std::mem::take(&mut literal));
                }
                (true, c) => literal.push(c),
                (false, '"') => in_string = true,
                (false, '(' | '{') => depth += 1,
                (false, ')' | '}') if depth > 0 => depth -= 1,
                (false, ',' | ')') if depth == 0 => break,
                _ => {}
            }
        }
    }
    // Other literals in the argument (`cfg!(target_os = "macos")`) are not keys.
    out.retain(|k| ext_manifest::check_key(k).is_ok());
    out.sort();
    out.dedup();
    out
}
