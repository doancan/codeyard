//! Consistency tests between docs and code: the key table in the user guide must match the real bindings,
//! and the built-in theme list and counts must match the themes.

use gpui::TestAppContext;

use crate::qa_workbench_tests::setup;

/// Modifier order, letter case and the ⌦/Tab spelling may differ: normalize to a single form for comparison.
#[cfg(target_os = "macos")]
fn canon(chord: &str) -> String {
    chord
        .split(' ')
        .map(|c| {
            let mut mods: Vec<char> = c.chars().filter(|m| "⌃⌥⇧⌘".contains(*m)).collect();
            mods.sort_unstable();
            let key: String = c.chars().filter(|m| !"⌃⌥⇧⌘".contains(*m)).collect::<String>().to_uppercase();
            let key = if key == "⌦" { "DELETE".to_string() } else { key };
            format!("{}{key}", mods.into_iter().collect::<String>())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn read_doc(name: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs").join(name)).unwrap()
}

/// Every shortcut in the macOS column (⌘P, ⇧⌘P, ⌘K ⌘T…) must really be bound. Mouse gestures and plain keys excluded.
#[cfg(target_os = "macos")]
#[gpui::test]
fn the_keys_table_in_the_user_guide_matches_the_real_key_bindings(cx: &mut TestAppContext) {
    let (_wb, cx) = setup(cx);
    let bound: std::collections::HashSet<String> = cx.update(|_, cx| {
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        keymap.bindings().map(|b| canon(&b.keystrokes().iter().map(|k| crate::commands::format_keystroke(k.inner())).collect::<Vec<_>>().join(" "))).collect()
    });
    let guide = read_doc("user-guide.md");
    let table = guide.split("## Keys").nth(1).and_then(|t| t.split("\nRight-click").next()).expect("Keys table");
    let mut checked = 0;
    let mut missing = Vec::new();
    for row in table.lines().filter(|l| l.starts_with("| ") && !l.starts_with("| ---") && !l.starts_with("| |")) {
        let cells: Vec<&str> = row.trim_matches('|').split(" | ").map(str::trim).collect();
        let Some(mac) = cells.get(1) else { continue };
        // "⌘P", "⌥⌘↑ / ⌥⌘↓", "↵ / ⇧↵, ⌘G / ⇧⌘G", "⌃\\` / ⌃⇧\\`", "F5, ⇧F5, ⇧⌘F5"
        for token in mac.split(" / ").flat_map(|t| t.split(", ")).map(|t| t.trim().replace("\\`", "`")) {
            let token = token.split(" (").next().unwrap_or(&token).trim().to_string();
            let has_modifier = token.chars().any(|c| "⌘⌥⌃⇧".contains(c));
            let is_fkey = token.starts_with('F') && token[1..].chars().all(|c| c.is_ascii_digit()) && token.len() > 1;
            if token.is_empty() || !(has_modifier || is_fkey) || token.contains("click") || token.contains("wheel") || token.contains("rebind") {
                continue;
            }
            checked += 1;
            if !bound.contains(&canon(&token)) {
                missing.push(format!("{token} (row: {})", cells[0]));
            }
        }
    }
    assert!(checked > 80, "only {checked} shortcuts were checked; did the table change shape?");
    assert!(missing.is_empty(), "shortcuts in docs/user-guide.md that are not bound: {missing:#?}");
}

/// Counts and names come from the themes, not from prose: the theme list in the README/guide must equal `builtin()`.
#[test]
fn the_documented_themes_are_the_built_in_themes() {
    let guide = read_doc("user-guide.md");
    let built = theme::themes::builtin();
    // The "Built in:" paragraph, not the whole guide: a name used in a colour-customization example elsewhere must not count as documented.
    let list = guide.split("\nBuilt in:").nth(1).and_then(|t| t.split("Every built-in").next()).expect("the guide's “Built in:” theme paragraph");
    for t in &built {
        assert!(list.contains(&t.name), "theme “{}” is built in but not listed in the “Built in:” paragraph of docs/user-guide.md", t.name);
    }
    let count = built.len();
    assert!(guide.contains(&format!("pick one of {count} themes")), "the guide must say “pick one of {count} themes” (the built-in count)");
    let pairs = theme::PAIRS.len();
    assert!(guide.contains(&format!("{pairs} pairs")), "the guide must say “{pairs} pairs” (the contrast pair count)");
}

/// The language-server table of the user guide lists exactly the default servers of `lsp`, with the same commands.
#[test]
fn the_language_server_table_lists_the_default_servers() {
    let guide = read_doc("user-guide.md");
    let section = guide.split("\n## Language servers").nth(1).and_then(|t| t.split("\n## ").next()).expect("Language servers section");
    let rows: Vec<Vec<String>> = section
        .lines()
        .filter(|l| l.starts_with("| ") && !l.starts_with("| ---") && !l.starts_with("| Language"))
        .map(|l| l.trim_matches('|').split(" | ").map(|c| c.trim().to_string()).collect())
        .collect();
    let mut commands: Vec<String> = rows.iter().map(|r| r[1].replace('`', "")).collect();
    commands.sort();
    let mut expected: Vec<String> = crate::lsp::DEFAULT_SERVERS.iter().map(|d| d.1.to_string()).collect();
    expected.sort();
    expected.dedup();
    commands.dedup();
    assert_eq!(commands, expected, "docs/user-guide.md \"Language servers\" table and lsp::DEFAULT_SERVERS disagree");
    for opt_in in ["json", "yaml", "toml"] {
        assert!(section.contains(&format!("\"{opt_in}\"")), "opt-in key {opt_in} is not explained in the section");
    }
}

/// The `docs/translating.md` table matches `LANGUAGES`: each language code is listed as a row with its own name and status;
/// every plural rule is documented and the commands needed to add a language really exist.
#[test]
fn docs_translating_matches_the_language_registry() {
    use crate::i18n::{LANGUAGES, Plural};
    let doc = read_doc("translating.md");
    for l in LANGUAGES {
        let row = format!("| `{}` | {} | {:?} |", l.code, l.name, l.status);
        assert!(doc.contains(&row), "docs/translating.md: the languages table has no row `{row}` (code, native name, status)");
    }
    let listed = doc
        .lines()
        .filter(|l| {
            l.starts_with("| `") && l.matches('|').count() == 4 && ["Source", "Complete", "Partial"].iter().any(|s| l.trim_end().ends_with(&format!("| {s} |")))
        })
        .count();
    assert_eq!(listed, LANGUAGES.len(), "docs/translating.md lists a language that is not in LANGUAGES (or the other way round)");
    for rule in [Plural::None, Plural::OneOther, Plural::French, Plural::EastSlavic, Plural::Polish] {
        // A row looks like "| `OneOther` | 2: one, other | …": the rule name and form count come first.
        let row = format!("| `{rule:?}` | {}", rule.forms());
        assert!(doc.contains(&row), "docs/translating.md: the plural table has no row starting `{row}` ({rule:?}, {} forms)", rule.forms());
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for script in ["scripts/i18n-status.sh", "scripts/i18n-new-language.sh", "crates/app/locales/template.tsv"] {
        assert!(doc.contains(script.rsplit('/').next().unwrap()), "docs/translating.md does not mention {script}");
        assert!(root.join(script).exists(), "{script} is documented but missing");
    }
}
