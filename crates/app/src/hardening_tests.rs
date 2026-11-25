//! Hardening tests: rules, not individual examples. Contracts that would break silently: not crashing on
//! random input, the settings catalog agreeing with the parser, every icon named in the source being embedded.
//! Randomness is deterministic (fixed seed): a failure reproduces identically on every run.

use std::path::{Path, PathBuf};

use crate::graph::{GraphCommit, cap_lanes, layout};
use crate::settings::{self, Settings};
use crate::workbench::settings_ui::{CATALOG, Kind, current};

/// xorshift64*: dependency-free, reproducible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn commit(i: usize, parents: Vec<usize>) -> GraphCommit {
    GraphCommit { hash: format!("h{i}"), short: format!("h{i}"), parents: parents.into_iter().map(|p| format!("h{p}")).collect(), ..Default::default() }
}

/// Random history in child-first order: each commit's parent comes after it in the list.
fn random_history(rng: &mut Rng, n: usize) -> Vec<GraphCommit> {
    (0..n)
        .map(|i| {
            let later = n - i - 1;
            let count = if later == 0 { 0 } else { [1, 1, 1, 2, 2, 0][rng.below(6)].min(later) };
            let mut parents = Vec::new();
            for _ in 0..count {
                let p = i + 1 + rng.below(later.min(6));
                if !parents.contains(&p) {
                    parents.push(p);
                }
            }
            commit(i, parents)
        })
        .collect()
}

#[test]
fn graph_layout_invariants_hold_for_random_histories() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for round in 0..200 {
        let n = 1 + rng.below(60);
        let commits = random_history(&mut rng, n);
        let rows = layout(&commits);
        assert_eq!(rows.len(), commits.len(), "round {round}: one row per commit");
        let mut expected: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for (c, r) in commits.iter().zip(&rows) {
            let mut cols: Vec<usize> = r.pass.iter().map(|l| l.0).collect();
            cols.extend(r.merge_in.iter().map(|l| l.0));
            cols.extend(r.fork_out.iter().map(|l| l.0));
            cols.push(r.col);
            assert!(cols.iter().all(|&x| x < r.width.max(1)), "round {round}: a lane is outside the row width ({} cols, width {})", cols.len(), r.width);
            assert!(r.pass.iter().all(|l| l.0 != r.col), "round {round}: a passing lane runs through the node");
            let mut seen = std::collections::HashSet::new();
            assert!(r.pass.iter().all(|l| seen.insert(l.0)), "round {round}: two passing lanes share a column");
            assert_eq!(r.above.is_some(), expected.contains(c.hash.as_str()), "round {round}: `above` must mean an earlier commit points here ({})", c.hash);
            assert_eq!(r.below.is_some(), !c.parents.is_empty(), "round {round}: `below` must mean there is a parent ({})", c.hash);
            for p in &c.parents {
                expected.insert(p.as_str());
            }
        }
    }
}

#[test]
fn capping_lanes_never_leaves_a_column_beyond_the_limit() {
    let mut rng = Rng(42);
    for max in [1usize, 2, 3, 7] {
        for _ in 0..50 {
            let n = 5 + rng.below(80);
            let commits = random_history(&mut rng, n);
            let mut rows = layout(&commits);
            cap_lanes(&mut rows, max);
            for r in &rows {
                assert!(r.col < max && r.width <= max, "max {max}: col {} width {}", r.col, r.width);
                assert!(r.pass.iter().chain(&r.merge_in).chain(&r.fork_out).all(|l| l.0 < max), "max {max}");
            }
        }
    }
}

fn junk(rng: &mut Rng, max_len: usize) -> String {
    const PIECES: &[&str] = &[
        "error",
        "warning",
        ":",
        ":",
        "1",
        "27",
        "0",
        "99999999999",
        "-",
        " ",
        "\n",
        "\r\n",
        "\t",
        "src/a.c",
        "/abs/x.rs",
        "C:\\w\\x.cs",
        "ğüşiöç",
        "İı",
        "💥",
        "\u{0}",
        "\u{1b}[31m",
        "(",
        ")",
        "[",
        "]",
        "{",
        "}",
        "\"",
        "'",
        "note: ",
        "^~~~",
        "  --> ",
        "= help",
        "rustc",
        "gcc",
        "tsc",
        "TS2304",
        "x undeclared",
        ",",
        ";",
    ];
    let mut s = String::new();
    for _ in 0..rng.below(max_len) {
        s.push_str(PIECES[rng.below(PIECES.len())]);
    }
    s
}

#[test]
fn task_output_parser_survives_random_text() {
    let mut rng = Rng(7);
    for _ in 0..2000 {
        let text = junk(&mut rng, 60);
        for (path, d) in crate::problems::parse_output(&text, "/work/root") {
            assert!(!path.is_empty(), "empty path from {text:?}");
            assert!((1..=4).contains(&d.severity), "severity {} from {text:?}", d.severity);
            assert!(d.start <= d.end || d.end == (0, 0) || d.end.0 >= d.start.0, "inverted range {:?}..{:?} from {text:?}", d.start, d.end);
        }
    }
}

#[test]
fn settings_parser_survives_junk_and_wrong_types() {
    let mut rng = Rng(11);
    for _ in 0..500 {
        // Plain garbage.
        let _ = settings::parse(&junk(&mut rng, 40));
        // Right keys, wrong types.
        let mut obj = serde_json::Map::new();
        for e in CATALOG {
            let v = match rng.below(6) {
                0 => serde_json::Value::Null,
                1 => serde_json::json!(true),
                2 => serde_json::json!(-1e300),
                3 => serde_json::json!(junk(&mut rng, 6)),
                4 => serde_json::json!([1, "a", null]),
                _ => serde_json::json!({ "x": 1 }),
            };
            obj.insert(e.key.to_string(), v);
        }
        let (s, _) = settings::parse(&serde_json::Value::Object(obj).to_string());
        // No value falls outside its bounds.
        assert!((6.0..=48.0).contains(&s.editor_font_size), "{}", s.editor_font_size);
    }
}

#[test]
fn every_catalog_setting_round_trips_through_the_parser() {
    let default = Settings::default();
    let mut obj = serde_json::Map::new();
    for e in CATALOG {
        let v = current(&default, e.key);
        if !v.is_null() {
            obj.insert(e.key.to_string(), v);
        }
    }
    let (parsed, warnings) = settings::parse(&serde_json::Value::Object(obj).to_string());
    assert!(warnings.is_empty(), "the catalog writes values the parser rejects or does not know: {warnings:?}");
    for e in CATALOG {
        assert_eq!(current(&parsed, e.key), current(&default, e.key), "{} does not survive a round trip", e.key);
    }
}

#[test]
fn every_choice_in_the_catalog_is_accepted_and_read_back() {
    for e in CATALOG {
        let Kind::Choice(options) = e.kind else { continue };
        for option in options {
            let (s, warnings) = settings::parse(&serde_json::json!({ e.key: option }).to_string());
            assert!(warnings.is_empty(), "{} = {option}: {warnings:?}", e.key);
            assert_eq!(current(&s, e.key), serde_json::json!(option), "{} = {option} is not read back", e.key);
        }
    }
}

#[test]
fn catalog_keys_are_unique_and_described() {
    let mut seen = std::collections::HashSet::new();
    for e in CATALOG {
        assert!(seen.insert(e.key), "{} is listed twice", e.key);
        assert!(e.about.len() >= 10, "{} has no real description", e.key);
        assert!(e.key.chars().all(|c| c.is_ascii_alphanumeric() || c == '.'), "{}", e.key);
    }
}

fn source_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = entry.path();
        if p.is_dir() {
            source_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

/// Every icon named in the source with `icon("name", …)` must be embedded; otherwise it silently renders empty.
#[test]
fn every_icon_named_in_the_source_is_embedded() {
    let embedded: std::collections::HashSet<&str> = crate::ui::embedded_icon_names().into_iter().collect();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    source_files(&src, &mut files);
    let mut missing = Vec::new();
    for f in files.iter().filter(|f| !f.ends_with("hardening_tests.rs")) {
        let text = std::fs::read_to_string(f).unwrap();
        for part in text.split("icon(\"").skip(1) {
            let Some(name) = part.split('"').next() else { continue };
            if name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') && !name.is_empty() && !embedded.contains(name) {
                missing.push(format!("{}: {name}", f.file_name().unwrap().to_string_lossy()));
            }
        }
    }
    assert!(missing.is_empty(), "icons used but not embedded: {missing:?}");
}

/// `assets/icons` and the embedded list must match: no icon added but not wired up, or deleted but left behind.
#[test]
fn embedded_icons_match_the_assets_folder() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/icons");
    let mut on_disk: Vec<String> =
        std::fs::read_dir(dir).unwrap().flatten().filter_map(|e| e.file_name().to_str()?.strip_suffix(".svg").map(str::to_string)).collect();
    let mut embedded: Vec<String> = crate::ui::embedded_icon_names().into_iter().map(str::to_string).collect();
    on_disk.sort();
    embedded.sort();
    embedded.dedup();
    assert_eq!(embedded, on_disk, "assets/icons and the embedded list differ");
}

#[test]
fn builtin_themes_have_unique_names_and_a_sane_default_pair() {
    let themes = theme::themes::builtin();
    let mut names = std::collections::HashSet::new();
    assert!(themes.iter().all(|t| names.insert(t.name.clone())), "two themes share a name");
    assert_eq!(themes[0].id, theme::themes::DEFAULT_DARK, "the dark default leads the list");
    assert_eq!(themes[1].id, theme::themes::DEFAULT_LIGHT, "the light default is second");
    assert!(themes[0].dark && !themes[1].dark);
}

#[test]
fn the_settings_template_is_clean_and_equals_the_defaults() {
    let (s, warnings) = settings::parse(settings::TEMPLATE);
    assert!(warnings.is_empty(), "{warnings:?}");
    let d = Settings::default();
    assert_eq!((s.startup, s.minimap, s.icon_theme.as_str(), s.confirm_drag_and_drop), (d.startup, d.minimap, d.icon_theme.as_str(), d.confirm_drag_and_drop));
}

/// Git status colors (M orange, N/A green, D red, R blue) must stay readable on every embedded theme, on all
/// file list backgrounds (normal, hover, selected): WCAG AA text ratio 4.5.
#[test]
fn git_mark_colors_are_readable_in_every_theme() {
    use crate::git::Mark;
    let marks = [Mark::Untracked, Mark::Renamed, Mark::Added, Mark::Modified, Mark::Deleted, Mark::Conflict];
    let mut failures = Vec::new();
    for t in theme::themes::builtin() {
        let p = &t.palette;
        for m in marks {
            for (bg_name, bg) in [("bg", p.bg), ("surface", p.surface), ("surface-hover", p.surface_hover), ("surface-active", p.surface_active)] {
                let r = theme::themes::contrast(m.color(p), bg);
                if r < 4.5 {
                    failures.push(format!("{} / {:?} on {bg_name}: {r:.2}", t.name, m));
                }
            }
        }
    }
    assert!(failures.is_empty(), "git decoration colors are hard to read:\n{}", failures.join("\n"));
}

/// Each marker's letter must be distinct and a single character (otherwise two states look the same).
#[test]
fn git_mark_letters_are_distinct() {
    use crate::git::Mark;
    let marks = [Mark::Untracked, Mark::Renamed, Mark::Added, Mark::Modified, Mark::Deleted, Mark::Conflict];
    let letters: std::collections::HashSet<&str> = marks.iter().map(|m| m.letter()).collect();
    assert_eq!(letters.len(), marks.len());
    assert!(letters.iter().all(|l| l.chars().count() == 1));
    assert_eq!(Mark::Untracked.letter(), "N", "new files read N (green), modified M (orange)");
    assert_eq!(Mark::Modified.letter(), "M");
}

/// Color pairings added in this release meet AA (4.5) for small text on every embedded theme: commit hash,
/// branch/tag chips, activity bar badge and welcome card.
#[test]
fn new_text_and_background_pairs_are_readable_in_every_theme() {
    use crate::graph::RefKind;
    use crate::ui::{ensure_contrast, mix};
    use theme::themes::contrast;
    let mut failures = Vec::new();
    let mut check = |theme: &str, what: &str, fg: u32, bg: u32| {
        let r = contrast(fg, bg);
        if r < 4.5 {
            failures.push(format!("{theme}: {what}: {r:.2}"));
        }
    };
    for t in theme::themes::builtin() {
        let p = &t.palette;
        for (name, bg) in [("bg", p.bg), ("surface", p.surface), ("surface-hover", p.surface_hover), ("surface-active", p.surface_active)] {
            // Commit hash (`commit_lines`): softened accent color, then the contrast guarantee.
            let hash = ensure_contrast(mix(p.text_muted, p.accent, 0.6), p.text, &[p.bg, p.surface, p.surface_hover, p.surface_active], 4.6);
            check(&t.name, &format!("commit hash on {name}"), hash, bg);
        }
        // Chips (HEAD filled accent; others colored text + a background lightly mixed with that color).
        for (what, kind) in [("HEAD chip", RefKind::Head), ("local chip", RefKind::Local), ("remote chip", RefKind::Remote), ("tag chip", RefKind::Tag)] {
            // A branch chip wears its line's color: every color a line can have must be readable.
            for lane in 0..=crate::graph::HEAD_COLOR {
                let (fg, bg) = crate::graph::ref_chip_colors(kind, crate::graph::lane_color(p, lane), p);
                check(&t.name, &format!("{what} on lane color {lane}"), fg, bg);
            }
        }
        check(&t.name, "activity badge (on-accent on accent)", p.on_accent, p.accent);
        // Welcome card: text, on the card background and the hover background.
        check(&t.name, "card text", p.text, p.surface);
        check(&t.name, "card text on hover", p.text, p.surface_hover);
        check(&t.name, "card shortcut chip", p.text_muted, p.surface);
        // Settings page: key name (small, single-spaced) and the "Reset" link.
        check(&t.name, "settings key name", p.line_number, p.bg);
        check(&t.name, "settings Reset link", p.accent, p.bg);
        // Focused explorer selection: name text, on the accent-leaning fill.
        check(&t.name, "focused tree selection", p.text, crate::ui::focused_selection(p));
    }
    assert!(failures.is_empty(), "hard-to-read pairs:\n{}", failures.join("\n"));
}
