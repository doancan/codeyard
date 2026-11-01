//! What `cyx check` warns about in key bindings and language ids: problems the manifest check cannot call errors,
//! because the app decides them at run time. Codeyard does not bind an extension's key that a built-in shortcut uses,
//! and a language id may come from another extension.
//!
//! `builtin_keys.txt` lists the keys of the app's built-in shortcuts. It is generated from `crates/app/src/main.rs`
//! by `UPDATE_DOCS=1 cargo test -p cyx reference`, and the test fails while it is out of date.

/// One key sequence per line, as the app's `KeyBinding::new` calls write them.
const BUILTIN_KEYS: &str = include_str!("builtin_keys.txt");

/// Whether a built-in shortcut uses `key` (on some platform: `secondary` is `cmd` on macOS and `ctrl` elsewhere).
pub fn is_builtin_key(key: &str) -> bool {
    let wanted = forms(key);
    BUILTIN_KEYS.lines().filter(|l| !l.is_empty()).any(|b| forms(b).iter().any(|f| wanted.contains(f)))
}

/// The ways `key` is typed: each keystroke with its modifiers in one order and `super`/`win` as `cmd`; one form per
/// meaning of `secondary`.
fn forms(key: &str) -> Vec<String> {
    ["cmd", "ctrl"].iter().map(|secondary| key.split(' ').map(|s| stroke(s, secondary)).collect::<Vec<_>>().join(" ")).collect()
}

fn stroke(s: &str, secondary: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let (mods, key) = match lower.strip_suffix("--") {
        Some(m) => (m.to_string(), "-".to_string()),
        None => match lower.rsplit_once('-') {
            Some((m, k)) => (m.to_string(), k.to_string()),
            None => (String::new(), lower.clone()),
        },
    };
    let mut mods: Vec<&str> = mods
        .split('-')
        .filter(|m| !m.is_empty())
        .map(|m| match m {
            "super" | "win" => "cmd",
            "secondary" => secondary,
            m => m,
        })
        .collect();
    mods.sort_unstable();
    mods.dedup();
    mods.push(&key);
    mods.join("-")
}

/// The closest built-in language id to an unknown one, when it is a likely typo (at most two edits away).
pub fn closest_language(id: &str) -> Option<&'static str> {
    ext_manifest::LANGUAGE_IDS.iter().map(|l| (distance(id, l), *l)).filter(|(d, _)| *d <= 2).min().map(|(_, l)| l)
}

/// Levenshtein distance.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != *cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

#[cfg(test)]
#[path = "keys_tests.rs"]
mod tests;
