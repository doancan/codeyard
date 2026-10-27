//! Quoting for commands typed into the user's shell (tasks, tests). The result reads the same in POSIX shells (sh, bash,
//! zsh, dash) and in fish. Inside single quotes fish still treats `\\` and `\'` as escapes, so the POSIX `'\''` idiom breaks
//! there: a test named `\'; touch p; #` would run `touch p`. A single quote or a backslash is written as its own
//! double-quoted piece instead (`"'"`, `"\\"`), which every one of these shells reads as that one character.

/// `s` as one shell word, always quoted.
pub fn quote(s: &str) -> String {
    format!("'{}'", in_single(s))
}

/// `s` as one shell word: left as it is when nothing in it means anything to a shell, quoted otherwise.
pub fn word(s: &str) -> String {
    let plain = !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | ':' | '-' | '+' | ',' | '@' | '=' | '%'));
    if plain && !s.starts_with('-') { s.to_string() } else { quote(s) }
}

/// `s` for the inside of a single-quoted string that is already open (and stays open after it).
pub fn in_single(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '\'' => out.push_str(r#"'"'"'"#),
            '\\' => out.push_str(r#"'"\\"'"#),
            c => out.push(c),
        }
    }
    out
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    const HOSTILE: &[&str] =
        &["plain", "two words", "doesn't crash", r"\'; touch p; #", r"a\b", r#"say "hi" $HOME `id` $(id) !! ; | & > <"#, "", "-n", "tab\there"];

    /// Every hostile name comes back from the shell exactly as it went in, and nothing else runs.
    fn round_trip(shell: &str) {
        let dir = std::env::temp_dir().join(format!("codeyard-quote-{}-{}", std::process::id(), shell.replace('/', "_")));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for s in HOSTILE {
            for script in [format!("printf '%s' {}", quote(s)), format!("printf '%s' {}", word(s)), format!("printf '%s' 'x{}y'", in_single(s))] {
                let out = std::process::Command::new(shell).arg("-c").arg(&script).current_dir(&dir).output().unwrap();
                let got = String::from_utf8_lossy(&out.stdout);
                let want = if script.ends_with("y'") { format!("x{s}y") } else { s.to_string() };
                assert_eq!(got, want, "{shell}: {script}");
            }
        }
        assert!(!dir.join("p").exists(), "{shell} ran an injected command");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn words_survive_posix_shells_and_fish() {
        for shell in ["/bin/sh", "/bin/bash", "/bin/zsh", "/bin/dash"] {
            if std::path::Path::new(shell).exists() {
                round_trip(shell);
            }
        }
        // fish is checked where it is installed (CODEYARD_FISH points at one that is not on PATH).
        let fish = std::env::var("CODEYARD_FISH").ok().or_else(|| {
            std::process::Command::new("sh").args(["-c", "command -v fish"]).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        });
        if let Some(fish) = fish.filter(|f| !f.is_empty()) {
            round_trip(&fish);
        }
        assert_eq!(word("src/a.rs"), "src/a.rs");
        assert_eq!(word("-rf"), "'-rf'", "a leading dash is never bare");
        assert_eq!(word(""), "''");
    }
}
