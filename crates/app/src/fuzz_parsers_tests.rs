//! Hostile input for the parsers that read files and settings from the outside world: deeply nested, enormous and random text. None
//! may panic, overflow the stack (that kills the process, so each parser has a test of its own) or take more than a few seconds.
//! Every call runs on a thread with the stack size of the main thread.

use std::time::{Duration, Instant};

const STACK: usize = 8 << 20;
const BUDGET: Duration = Duration::from_secs(5);

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

/// Texts that have broken parsers elsewhere: nesting of every kind of opener, long repetitions, and noise.
fn hostile_texts() -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = Vec::new();
    let mut add = |name: String, text: String| v.push((name, text));
    for n in [1_000usize, 20_000, 300_000] {
        for opener in [
            "[",
            "{\"a\":",
            "(",
            "<a>",
            "- ",
            "> ",
            "[a](",
            "![x](",
            "`",
            "*",
            "_",
            "\\",
            "$",
            "{",
            "<<<<<<<\n",
            "\"",
            "'",
            "<!--",
            "/*",
            "#",
            "|",
            "~~",
            "[[",
            "&",
            "%{",
            "${",
        ] {
            add(format!("{opener:?} x {n}"), opener.repeat(n));
        }
        add(format!("balanced brackets x {n}"), format!("{}1{}", "[".repeat(n), "]".repeat(n)));
        add(format!("balanced objects x {n}"), format!("{}1{}", "{\"a\":".repeat(n), "}".repeat(n)));
        add(format!("emmet chain x {n}"), "div>".repeat(n) + "p");
        add(format!("emmet sibling x {n}"), "a+".repeat(n) + "b");
        add(format!("emmet multiply x {n}"), "ul>li*".repeat(n.min(5_000)) + "2");
        add(format!("a space b x {n}"), "a b ".repeat(n));
        add(format!("one long word x {n}"), "a".repeat(n * 4));
        add(format!("newlines x {n}"), "\n".repeat(n));
        add(format!("equals x {n}"), "a=".repeat(n));
        add(format!("tables x {n}"), "|a|b|\n|-|-|\n".repeat(n / 10));
    }
    add("emmet huge multiplier".into(), "div*99999999999999999999".into());
    add("emmet huge nested multiplier".into(), "div*999>div*999>div*999>p*999".into());
    add("css numbers".into(), "w99999999999999999999999999px".into());
    let mut rng = Rng(42);
    for case in 0..40 {
        let alphabet: Vec<char> = "{}[]()<>\"'`\\/*-+=:;,.#$%&|~^!? \t\n\r\0aé中😀\u{202e}\u{0301}\u{feff}01_".chars().collect();
        let len = 200 + (rng.next() % 4_000) as usize;
        add(format!("noise {case}"), (0..len).map(|_| alphabet[(rng.next() as usize) % alphabet.len()]).collect());
    }
    v
}

/// Runs `f` on every hostile text on a big-stack thread, naming the text that panics or takes too long.
fn every_text(parser: &'static str, f: impl Fn(&str) + Send + Sync + 'static) {
    let f = std::sync::Arc::new(f);
    for (name, text) in hostile_texts() {
        let f = f.clone();
        let started = Instant::now();
        let outcome = std::thread::Builder::new().stack_size(STACK).spawn(move || f(&text)).unwrap().join();
        assert!(outcome.is_ok(), "{parser} panicked on {name}");
        assert!(started.elapsed() < BUDGET, "{parser} took {:?} on {name}", started.elapsed());
    }
}

#[test]
fn data_formats_survive_hostile_text() {
    every_text("datalens::parse", |t| {
        for format in crate::datalens::Format::ALL {
            let _ = crate::datalens::parse(t, format);
        }
        let _ = crate::datalens::parse_any(t, None);
        let _ = crate::datalens::sniff(t);
    });
}

#[test]
fn the_query_language_survives_hostile_programs_and_documents() {
    every_text("datalens_query", |t| {
        if let Ok(q) = crate::datalens_query::parse(t) {
            let input = serde_json::json!({"a": [1, 2, {"b": null}], "c": "text"});
            let _ = q.run(&input, &|| false);
        }
        // And the text as the document, under programs that walk everything.
        if let Ok(doc) = crate::datalens::parse(t, crate::datalens::Format::Json) {
            for program in [".", "..", "[..]|length", "[paths]|length", "tostring", "tojson", "walk(.)", "[leaf_paths]"] {
                if let Ok(q) = crate::datalens_query::parse(program) {
                    let _ = q.run(&doc.value, &|| false);
                }
            }
        }
    });
}

#[test]
fn the_json_formatter_survives_hostile_text() {
    every_text("jsonfmt", |t| {
        let _ = crate::jsonfmt::format(t, "  ");
        let _ = crate::jsonfmt::minify(t);
    });
}

#[test]
fn markdown_survives_hostile_text() {
    every_text("markdown::parse", |t| {
        let blocks = crate::markdown::parse(t);
        let _ = crate::markdown::plain_text(&blocks);
    });
}

#[test]
fn emmet_survives_hostile_abbreviations() {
    every_text("emmet", |t| {
        let _ = crate::emmet::expand_html(t, false);
        let _ = crate::emmet::expand_html(t, true);
        let _ = crate::emmet::expand_css(t);
    });
}

#[test]
fn editorconfig_survives_hostile_files_and_globs() {
    every_text("editorconfig", |t| {
        let _ = crate::editorconfig::parse(t);
        let _ = crate::editorconfig::glob_regex(t);
        let one_line = t.lines().next().unwrap_or("");
        let _ = crate::editorconfig::glob_regex(one_line);
    });
}

#[test]
fn colour_links_fuzzy_and_conflict_scanners_survive_hostile_text() {
    every_text("scanners", |t| {
        let _ = crate::color::find_colors(t, true);
        let _ = crate::fuzzy::score(t, t);
        let _ = crate::fuzzy::score("ab", t);
        let _ = crate::links::parse(t);
        let cells: Vec<char> = t.chars().take(5_000).collect();
        for col in [0, cells.len() / 2, cells.len().saturating_sub(1)] {
            let _ = crate::links::link_at(&cells, col);
        }
        let _ = crate::conflict::find_in_lines(t.lines().enumerate());
    });
}

#[test]
fn settings_keybindings_and_themes_survive_hostile_text() {
    every_text("config files", |t| {
        let _ = crate::settings::parse_keybindings(t);
        let _ = crate::settings::parse_layers(t, &[]);
        let _ = theme::themes::parse(t, theme::themes::Source::Builtin);
        let _ = theme::themes::strip_jsonc(t);
    });
}

#[test]
fn file_uris_and_workspace_files_survive_hostile_text() {
    every_text("uris and workspace files", |t| {
        let _ = schema::file_uri::to_path(t);
        let _ = crate::workspace_file::folders(t, "/work", |_| true);
        let _ = crate::workspace_file::folders(t, r"C:\work", |_| true);
        let _ = crate::workspace_file::folders(&format!("{{\"folders\":[{{\"path\":{t:?}}},{{\"uri\":{t:?}}}]}}"), "/work", |_| true);
    });
}

#[test]
fn text_encodings_survive_random_bytes() {
    let mut rng = Rng(7);
    for case in 0..200 {
        let len = (rng.next() % 6_000) as usize;
        let bytes: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        let _ = crate::encoding::decode(&bytes, crate::encoding::Encoding::Utf8);
        let _ = crate::hexdump::dump(&bytes);
        let _ = crate::datalens_decode::decode(&String::from_utf8_lossy(&bytes));
        assert!(case < 1000);
    }
}
