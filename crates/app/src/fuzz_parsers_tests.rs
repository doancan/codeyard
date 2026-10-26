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

/// `t` alone and behind the start of a real document of the kind, so parsers get past their first check.
fn framed<'a>(t: &'a str, frames: &'a [&'a str]) -> impl Iterator<Item = String> + 'a {
    std::iter::once(t.to_string()).chain(frames.iter().map(move |f| format!("{f}{t}")))
}

/// Files a cloned repository brings: `.http`, launch and task files, test files the Testing view reads, projects and snippets.
#[test]
fn project_files_survive_hostile_text() {
    every_text("project files", |t| {
        let env = |_: &str| Some("v".to_string());
        let uuid = String::new;
        let random = |a: i64, _: i64| a;
        let responses = std::collections::HashMap::new();
        for text in framed(t, &["GET http://a/{{x}}\n", "@x = {{y}}\n@y = {{x}}\nPOST https://a/\n\n< ", "GET https://a/\nH: {{$processEnv "]) {
            let p = crate::http_client::parse(&text);
            let ctx = crate::http_client::Context { vars: &p.vars, responses: &responses, now: 0, env: &env, uuid: &uuid, random: &random };
            for r in p.requests.iter().take(50) {
                let _ = crate::http_client::sensitive_parts(r, &p.vars);
                let _ = crate::http_client::prepare(r, &ctx, &|_| Ok(Vec::new()));
            }
        }
        for text in framed(t, &["{\"configurations\":[{\"name\":\"a\",\"type\":\"lldb\",\"program\":", "{\"compounds\":[{\"configurations\":"]) {
            let _ = crate::debug_config::parse_launch_json(&text, "/w", Some("/w/a.rs"), &[]);
            let _ = crate::debug_config::parse_compounds(&text, "/w", None, &[]);
            let tasks = crate::workbench::tasks::parse_task_file(&text, "tasks.jsonc");
            let _ = crate::workbench::tasks::expand_variables_for_shell(&text, "/w", Some("/w/it's.rs"));
            let _ = tasks.len();
        }
        for text in framed(t, &["{\"scripts\":{", "all:\n\t", ".PHONY: "]) {
            let _ = crate::workbench::tasks::npm_tasks(&text, "npm");
            let _ = crate::workbench::tasks::make_tasks(&text);
        }
        let pj = r#"{"devDependencies":{"vitest":"1"}}"#;
        for (rel, frame) in [("src/a.rs", "#[test]\nfn "), ("a_test.go", "func Test"), ("test_a.py", "class TestA:\n    def test_"), ("a.test.ts", "it(\"")] {
            for text in framed(t, &[frame]) {
                let cases = crate::testrun::cases(rel, &text, Some(pj));
                let lines = text.lines().count();
                for line in [0, lines / 2, lines.saturating_sub(1), lines + 5] {
                    let _ = crate::testrun::plan(rel, &text, line, false, Some(pj));
                }
                let _ = crate::testrun::plan(rel, &text, 0, true, Some(pj));
                assert!(cases.len() <= text.lines().count(), "{rel}: more tests than lines");
            }
        }
        let _ = crate::workbench::projects::parse_projects(t);
        let _ = crate::snippets::parse_user_file("rust", t);
        let _ = crate::debug_model::parse_breakpoint_meta(t);
    });
}

/// What programs print, which the app reads back: test runners, compilers, git, port scanners, GitHub, containers.
#[test]
fn program_output_survives_hostile_text() {
    every_text("program output", |t| {
        use crate::testexp::{Kind, parse_results};
        for text in framed(t, &["test a::b ... ", "---- a stdout ----\nthread 'a' panicked at ", "=== RUN   T\n    ", "--- FAIL: ", "FAILED x.py::t - "]) {
            for kind in [Kind::Rust, Kind::Go, Kind::Python, Kind::Js] {
                let _ = parse_results(kind, &text);
            }
            let _ = crate::problems::parse_output(&text, "/w");
            let _ = crate::testexp::test_executables(&text);
            let _ = crate::testexp::cargo_select_args(&text);
        }
        for text in framed(t, &["LISTEN 0 128 ", "  0: 0100007F:", "p1\ncnode\nn"]) {
            let _ = crate::ports::parse_ss(&text);
            let _ = crate::ports::parse_proc_net_entries(&text);
            let _ = crate::ports::parse_lsof_entries(&text);
        }
        for text in framed(t, &["## main...origin/main [ahead ", "1 .M N... ", "\u{0}R  a\u{0}"]) {
            let _ = crate::git::parse_status("/w", text.as_bytes());
            let _ = crate::git::parse_show_ref(&text);
            let _ = crate::git::parse_log(&text);
            let _ = crate::git::parse_blame(&text, "a.rs");
            let _ = crate::git::parse_branches(&text);
            let _ = crate::git::parse_worktrees(&text);
            let _ = crate::git::parse_remotes(&text);
            let _ = crate::git::parse_branch_infos(&text);
            let _ = crate::git::parse_track(&text);
            let _ = crate::git::parse_remote_tags(&text);
            let _ = crate::graph::parse_refs(&text);
            let _ = crate::graph::parse_graph_log(&text);
            let _ = crate::graph::parse_name_status(&text);
            let _ = crate::graph::parse_detail_head(&text, "abc");
            let _ = crate::graph::parse_changes(&text);
        }
        let _ = crate::github::parse_remote(t);
        let _ = crate::github::parse_gh(t);
        let _ = crate::github::parse_api(t);
        let _ = crate::github::parse_iso(t);
        let _ = crate::http_client::parse_output(t.as_bytes());
        let _ = crate::http_client::parse_output(format!("HTTP/1.1 200 OK\r\n{t}{}", crate::http_client::MARKER).as_bytes());
        let _ = host::ssh::parse_containers(t);
        let _ = host::ssh::parse_wsl_list(t.as_bytes());
    });
}

/// SSH config files, host targets typed by the user and settings values the user writes.
#[test]
fn ssh_config_and_setting_values_survive_hostile_text() {
    every_text("ssh and settings values", |t| {
        for text in framed(t, &["Host ", "Host a\n  HostName ", "Include "]) {
            let _ = host::ssh_hosts::parse_config(&text);
        }
        let _ = host::ssh_config::parse_target(t);
        let _ = crate::terminal_osc::parse_cwd(t);
        let _ = crate::port_forwards::Protocol::parse(t);
        if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(t) {
            let mut warn = Vec::new();
            let _ = crate::terminal_profiles::parse_profiles("terminal.profiles.macos", &map, &mut warn);
            let _ = crate::port_forwards::parse_rules(&map, &mut warn);
        }
        let mut map = serde_json::Map::new();
        map.insert(t.chars().take(64).collect(), serde_json::json!({ "path": t, "args": [t], "env": { "K": t }, "label": t, "onForward": t }));
        let mut warn = Vec::new();
        let _ = crate::terminal_profiles::parse_profiles("terminal.profiles.linux", &map, &mut warn);
        let _ = crate::port_forwards::parse_rules(&map, &mut warn);
    });
}

/// A terminal's output stream, cut into random chunks: the shell-integration scanner sees every split, and pastes of any text
/// are prepared without panicking.
#[test]
fn the_terminal_stream_survives_hostile_bytes_in_any_chunking() {
    let mut rng = Rng(11);
    let pieces: &[&[u8]] =
        &[b"\x1b]133;A\x07", b"\x1b]133;D;", b"\x1b]7;file://h/", b"\x1b\\", b"\x1b]", b"\x07", b"\x1b[200~", b"\x1b[201~", b"\xff\xfe", b"\n", b"%2"];
    for _ in 0..300 {
        let mut data = Vec::new();
        for _ in 0..(rng.next() % 200) {
            if rng.next().is_multiple_of(3) {
                data.extend_from_slice(pieces[(rng.next() as usize) % pieces.len()]);
            } else {
                data.push(rng.next() as u8);
            }
        }
        let mut scanner = crate::terminal_osc::Scanner::default();
        let mut at = 0;
        while at < data.len() {
            let n = 1 + (rng.next() as usize) % 17;
            let end = (at + n).min(data.len());
            for (i, _) in scanner.scan(&data[at..end]) {
                assert!(i <= end - at, "an event index past its chunk");
            }
            at = end;
        }
        let text = String::from_utf8_lossy(&data);
        for policy in [crate::terminal_paste::Sanitize::Auto, crate::terminal_paste::Sanitize::Always, crate::terminal_paste::Sanitize::Off] {
            for bracketed in [false, true] {
                let p = crate::terminal_paste::prepare(&text, bracketed, policy);
                let _ = crate::terminal_paste::needs_confirm(&p, bracketed, crate::terminal_paste::WarnMultiline::Always);
            }
        }
    }
}

/// Finding every test in a file must not look at the whole file once per test: 4,000 tests in one file took half a minute
/// when it did. Each file kind here has to be done well within the budget.
#[test]
fn finding_the_tests_of_a_huge_test_file_is_linear() {
    let n = 4_000;
    let rust = "#[cfg(test)]\nmod tests {\n".to_string()
        + &(0..n).map(|i| format!("    #[test]\n    fn t{i}() {{\n        assert!(true);\n    }}\n")).collect::<String>()
        + "}\n";
    let py = "class TestA:\n".to_string() + &(0..n).map(|i| format!("    def test_{i}(self):\n        pass\n")).collect::<String>();
    let go: String = (0..n).map(|i| format!("func Test{i}(t *testing.T) {{\n}}\n")).collect();
    let js = "describe('a', () => {\n".to_string() + &(0..n).map(|i| format!("  it('t{i}', () => {{}});\n")).collect::<String>() + "});\n";
    for (rel, text) in [("src/a.rs", rust), ("test_a.py", py), ("a_test.go", go), ("a.test.ts", js)] {
        let started = Instant::now();
        let found = crate::testrun::cases(rel, &text, Some(r#"{"devDependencies":{"jest":"1"}}"#));
        assert_eq!(found.len(), n, "{rel}");
        assert!(started.elapsed() < BUDGET / 5, "{rel}: {:?} for {n} tests", started.elapsed());
    }
}
