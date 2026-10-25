//! User-visible text must not bypass the translation system: if a bare English literal is passed to one of the known "text sinks" in the source (`.child("…")`,
//! `Notice::Info("…")`, tooltip, placeholder, confirmation and input prompts), the test fails.
//! Fix: use `t()` / `tf()` / `tn()` / `tr()` and add an entry to `locales/tr.tsv` (docs/translating.md). If the text genuinely
//! must not be translated (product name, protocol name, someone else's output), write `// i18n-keep: reason` on the line: the exception stays visible in review.
//! This is a heuristic: it only sees calls that fit on a single line and only looks at the known sinks; it may miss things,
//! but everything it catches is real.

use std::path::Path;

/// Text-taking calls. The first six carry the literal in their own delimiter; the last ones (`confirm`, `show_input`) search for the first literal.
const SINKS: &[&str] = &[
    ".child(\"",
    ".child(format!(\"",
    "Notice::Info(\"",
    "Notice::Info(format!(\"",
    "Notice::Error(\"",
    "Notice::Error(format!(\"",
    "Tooltip(\"",
    "Tooltip(format!(\"",
    ".placeholder(\"",
    ".confirm(",
    "show_input(",
];

/// Is there a translation call (`t(`, `tf(`, `tn(`, `tr(` and their `i18n::`-prefixed forms)?
fn is_wrapped(line: &str) -> bool {
    ["t(", "tf(", "tn(", "tr("]
        .iter()
        .any(|needle| line.match_indices(needle).any(|(at, _)| !line[..at].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_')))
}

/// After dropping `{…}` placeholders, is a word of at least three letters left? Data texts such as numbers, `+{n}`, `{a}: {b}` do not qualify.
fn has_words(literal: &str) -> bool {
    let mut depth = 0;
    let mut run = 0;
    for ch in literal.chars() {
        match ch {
            '{' => depth += 1,
            '}' if depth > 0 => depth -= 1,
            c if depth == 0 && c.is_ascii_alphabetic() => {
                run += 1;
                if run >= 3 {
                    return true;
                }
            }
            _ => run = 0,
        }
    }
    false
}

/// The first string literal on the line (skipping escapes), after position `from`.
fn first_literal(line: &str, from: usize) -> Option<&str> {
    let start = line[from..].find('"')? + from + 1;
    let mut escaped = false;
    for (i, ch) in line[start..].char_indices() {
        match ch {
            '\\' if !escaped => escaped = true,
            '"' if !escaped => return Some(&line[start..start + i]),
            _ => escaped = false,
        }
    }
    None
}

/// Lines in the source that break the rule: `(line number, line)`.
fn violations(source: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut in_tests = false;
    let mut after_cfg_test = false;
    for (n, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        // If `#[cfg(test)]` precedes a `mod`, what follows is test code; if it marks a single helper function, the rest of the file is not.
        if trimmed.starts_with("#[cfg(test)]") {
            after_cfg_test = true;
            continue;
        }
        if after_cfg_test && !trimmed.starts_with("#[") {
            // Inline blocks only (`mod tests {`); `mod workbench_testing;` declares a separate file, so the rest of this file is not test code.
            in_tests |= (trimmed.starts_with("mod ") || trimmed.starts_with("pub mod ") || trimmed.starts_with("pub(crate) mod ")) && trimmed.ends_with('{');
            after_cfg_test = false;
        }
        if in_tests || trimmed.starts_with("//") || line.contains("i18n-keep") {
            continue;
        }
        let wrapped = is_wrapped(line);
        for sink in SINKS {
            // A `t(` elsewhere on the line must not hide a bare literal that sits directly in the sink; only `confirm(`/`show_input(` (first literal may be inside `t(…)`) take the line-wide check.
            if wrapped && !sink.ends_with('"') {
                continue;
            }
            let Some(at) = line.find(sink) else { continue };
            let after = if sink.ends_with('"') { at + sink.len() - 1 } else { at + sink.len() };
            if first_literal(line, after).is_some_and(has_words) {
                out.push((n + 1, line.trim().to_string()));
                break;
            }
        }
    }
    out
}

#[test]
fn user_visible_text_goes_through_the_translation_system() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    for path in crate::i18n_tests::rs_files(&dir) {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        // Test files and the translation infrastructure are out of scope. The command palette is scanned too: its labels go through `t()` at display time, but its notices do not.
        if !name.ends_with(".rs") || name == "tests.rs" || name.ends_with("_tests.rs") || name.starts_with("i18n") || name == "main.rs" {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        found.extend(violations(&text).into_iter().map(|(n, l)| format!("{name}:{n}: {l}")));
    }
    assert!(
        found.is_empty(),
        "untranslated UI text (wrap it with t()/tf()/tn()/tr() and add it to locales/tr.tsv, or mark data and product names with `// i18n-keep: reason`):\n{}",
        found.join("\n")
    );
}

#[test]
fn the_scanner_flags_bare_text_and_accepts_the_documented_exceptions() {
    let flagged = |s: &str| !violations(s).is_empty();
    // bare text
    assert!(flagged(r#"d.child("Reconnect")"#));
    assert!(flagged(r#"self.notice = Some(Notice::Info("Saved.".into()));"#));
    assert!(flagged(r#"self.notice = Some(Notice::Error(format!("Could not save {name}").into()));"#));
    assert!(flagged(r#".tooltip(|_, cx| cx.new(|_| Tooltip("Toggle it".into())).into())"#));
    assert!(flagged(r#".placeholder("Search")"#));
    assert!(flagged(r#"self.confirm(window, "Delete it?", "Cannot be undone.", "Delete", cx, f);"#));
    // translated
    assert!(!flagged(r#"d.child(crate::i18n::t("Reconnect"))"#));
    assert!(!flagged(r#"Notice::Info(tf("Saved {}", &[&n]).into())"#));
    assert!(!flagged(r#"d.child(tn("{} file", "{} files", n, &[]))"#));
    assert!(!flagged(r#"d.child(tr("Search"))"#));
    // data: number, placeholder, short glyph
    assert!(!flagged(r#"d.child(format!("+{added}"))"#));
    assert!(!flagged(r#"d.child(format!("{key}: {value}"))"#));
    assert!(!flagged(r##"d.child(format!("#{id}"))"##));
    assert!(!flagged(r#"d.child("→")"#));
    // documented exception and comment line
    assert!(!flagged(r#"d.child("Codeyard") // i18n-keep: product name"#));
    assert!(!flagged(r#"// d.child("Not a call")"#));
    // `tr(` counts as a translation call only on its own: names like `str(`/`attr(` do not
    assert!(flagged(r#"d.child("Hello").attr("x")"#));
    // a translated call elsewhere on the line does not excuse a bare literal in a direct sink
    assert!(flagged(r#"d.child("Hello there").child(t("Other"))"#));
    assert!(!flagged(r#"self.confirm(window, t("Delete it?"), t("Cannot be undone."), t("Delete"), cx, f);"#));
    // the test module is not scanned...
    assert!(!flagged("#[cfg(test)]\nmod tests {\n    fn f() { d.child(\"Bare\"); }\n}\n"));
    // ...but a `#[cfg(test)]` marking a single test helper does not hide the rest of the file
    assert!(flagged("#[cfg(test)]\npub fn helper() {}\nfn real() { d.child(\"Bare\"); }\n"));
    // a `#[cfg(test)] mod x;` declaration does not hide it either
    assert!(flagged("#[cfg(test)]\nmod workbench_testing;\nfn real() { d.child(\"Bare\"); }\n"));
    assert_eq!(violations("#[cfg(test)]\n#[allow(dead_code)]\npub fn helper() {}\nfn real() { d.child(\"Bare\"); }\n")[0].0, 4);
    // line number is reported correctly
    assert_eq!(violations("fn a() {}\nd.child(\"Bare\")\n")[0].0, 2);
}
