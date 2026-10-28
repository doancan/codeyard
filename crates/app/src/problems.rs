//! Problem extraction from task output (VS Code "problem matcher"). Terminal text is scanned;
//! recognized compiler lines feed the Problems panel and the editor squiggles.
//!
//! Recognized formats (no per-task configuration needed):
//! - gcc / clang / Go / javac / mypy: `file:line:col: error: message` (Go has no severity,
//!   a column counts as an error)
//! - rustc / cargo: `error[E0425]: message` followed by ` --> file:line:col`
//! - tsc / MSBuild: `file(line,col): error TS2304: message`
//! - eslint (stylish): a `file` line, under it `  12:5  error  message  rule`
//!
//! Plain text only: files aren't checked for existence (costly on remote
//! hosts); a path that won't open is reported on click.

use std::{borrow::Cow, sync::LazyLock};

use regex::Regex;

use crate::lsp::Diagnostic;

/// Per-task cap, so runaway output can't bloat memory.
pub const MAX_PROBLEMS: usize = 1000;

static COLON: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?P<file>(?:[A-Za-z]:[\\/])?[^\s:()][^:()]*?):(?P<line>\d+):(?:(?P<col>\d+):)?\s*(?:(?P<sev>fatal error|error|warning|note|info|hint)\s*(?:\[[^\]]*\])?\s*:?\s*)?(?P<msg>\S.*)$").unwrap()
});
static PAREN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?P<file>[^\s()][^()]*?)\((?P<line>\d+)(?:,(?P<col>\d+))?\):\s*(?P<sev>error|warning|info)\s+(?P<code>[A-Za-z]+\d+)\s*:\s*(?P<msg>.*)$")
        .unwrap()
});
static RUSTC_HEAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?P<sev>error|warning)(?:\[(?P<code>[A-Za-z0-9]+)\])?:\s*(?P<msg>\S.*)$").unwrap());
static RUSTC_AT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*-->\s+(?P<file>.+?):(?P<line>\d+):(?P<col>\d+)\s*$").unwrap());
static STYLISH_ITEM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s+(?P<line>\d+):(?P<col>\d+)\s+(?P<sev>error|warning)\s+(?P<msg>.*?)(?:\s{2,}(?P<rule>\S+))?\s*$").unwrap());

/// Rust test panic: new format puts the location in the header, the message on the next line.
static RUST_PANIC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^thread '.*'(?: \(\d+\))? panicked at (?P<file>.+?):(?P<line>\d+):(?P<col>\d+):?\s*$").unwrap());
/// Old format: `panicked at 'message', file:line:col`.
static RUST_PANIC_OLD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^thread '.*'(?: \(\d+\))? panicked at '(?P<msg>.*)', (?P<file>.+?):(?P<line>\d+):(?P<col>\d+)\s*$").unwrap());
/// `go test`: indented `a_test.go:12: message`.
static GO_TEST_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s+(?P<file>[\w./-]+_test\.go):(?P<line>\d+):\s*(?P<msg>\S.*)$").unwrap());
/// pytest trace: `file.py:12: AssertionError`.
static PYTEST_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?P<file>[\w./-]+\.py):(?P<line>\d+): (?P<msg>\w*(?:Error|Exception|Failed|Exit)\b.*)$").unwrap());

fn severity(word: &str) -> u8 {
    match word {
        "error" | "fatal error" => 1,
        "warning" => 2,
        "note" | "info" => 3,
        _ => 4,
    }
}

/// `a/./b/../c` → `a/c`; for absolute and relative paths (joined onto the root if one is given).
fn resolve(root: &str, file: &str) -> String {
    let file = file.trim();
    // A Linux/SSH file may legally contain a backslash. Only Windows roots/absolute paths imply separators.
    let windows = [root, file].iter().any(|p| p.starts_with("//") || p.starts_with(r"\\") || p.as_bytes().get(1) == Some(&b':'));
    let root = output_path(root, windows);
    let file = output_path(file, windows);
    let joined =
        if file.starts_with('/') || file.as_bytes().get(1) == Some(&b':') { file.to_string() } else { format!("{}/{file}", root.trim_end_matches('/')) };
    let mut parts: Vec<&str> = Vec::new();
    for part in joined.split('/') {
        match part {
            "." => {}
            ".." => {
                if parts.len() > 1 {
                    parts.pop();
                }
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

fn output_path(path: &str, windows: bool) -> Cow<'_, str> {
    if !windows {
        return Cow::Borrowed(path);
    }
    let path = if let Some(unc) = path.strip_prefix(r"\\?\UNC\").or_else(|| path.strip_prefix("//?/UNC/")) {
        format!("//{unc}")
    } else {
        path.strip_prefix(r"\\?\").or_else(|| path.strip_prefix("//?/")).unwrap_or(path).to_string()
    };
    Cow::Owned(path.replace('\\', "/"))
}

fn diagnostic(line: &str, col: Option<&str>, sev: u8, message: &str) -> Diagnostic {
    let line: u32 = line.parse().unwrap_or(1);
    // A single character if the column is known, the whole line otherwise.
    let (c0, c1) = match col.and_then(|c| c.parse::<u32>().ok()) {
        Some(c) if c > 0 => (c - 1, c),
        _ => (0, 10_000),
    };
    Diagnostic {
        start: (line.saturating_sub(1), c0),
        end: (line.saturating_sub(1), c1),
        severity: sev,
        message: message.trim().to_string(),
        source: Some("task".into()),
        ..Diagnostic::default()
    }
}

/// Does it look like a file name? (has an extension, no spaces): filters out lines like "Compiling foo v1.0".
fn looks_like_path(file: &str) -> bool {
    let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
    !file.contains("://")
        && !file.chars().any(char::is_whitespace)
        && name.rsplit_once('.').is_some_and(|(stem, ext)| !stem.is_empty() && !ext.is_empty() && ext.len() <= 8)
}

/// Scan output into (absolute path, diagnostic). Same location+message is returned once; order kept.
pub fn parse_output(text: &str, root: &str) -> Vec<(String, Diagnostic)> {
    let mut out: Vec<(String, Diagnostic)> = Vec::new();
    let mut push = |file: &str, d: Diagnostic| {
        if out.len() >= MAX_PROBLEMS {
            return;
        }
        let path = resolve(root, file);
        if !out.iter().any(|(p, o)| *p == path && o.start == d.start && o.message == d.message) {
            out.push((path, d));
        }
    };
    let mut st = State::default();
    for raw in text.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            st.stylish_file = None;
            continue;
        }
        // A shell prompt can stick to the first line of terminal output (`# src/a.c:1:2: error…`):
        // if the line doesn't match as is, strip the prompt and retry.
        if consume(line, &mut st, &mut push) {
            continue;
        }
        if line.contains(':')
            && let Some(rest) = strip_prompt(line)
            && consume(rest, &mut st, &mut push)
        {
            continue;
        }
        // eslint file header: an unindented, space-free line with an extension.
        st.stylish_file = (!line.starts_with(char::is_whitespace) && looks_like_path(line)).then(|| resolve(root, line));
    }
    out
}

/// Cross-line state: rustc header wants the next `-->` line, eslint file header its items,
/// a Rust panic the message on the next non-empty line.
#[derive(Default)]
struct State {
    pending: Option<(u8, String)>,
    stylish_file: Option<String>,
    panic_at: Option<(String, String, String)>,
}

/// Leading shell prompt (`# `, `$ `, `% `, `user@host:~$ `, `sh-3.2$ `): the first space-free chunk ending in `prompt[#$%>] `.
fn strip_prompt(line: &str) -> Option<&str> {
    static PROMPT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\S*[#$%>] +(\S.*)$").unwrap());
    PROMPT.captures(line).and_then(|c| c.get(1)).map(|m| m.as_str())
}

/// Handle the line if it is (part of) a problem; returns `true` if so.
fn consume(line: &str, st: &mut State, push: &mut dyn FnMut(&str, Diagnostic)) -> bool {
    if let Some((file, l, c)) = st.panic_at.take() {
        push(&file, diagnostic(&l, Some(&c), 1, &crate::i18n::tf("test panicked: {}", &[&line.trim()])));
        return true;
    }
    // Every format below has a colon; most output lines have none, and skipping the ten patterns keeps a rescan of a long
    // task terminal cheap.
    if !line.contains(':') {
        return false;
    }
    // The inline-message format must win before the modern path pattern accepts its spaces and comma.
    if let Some(c) = RUST_PANIC_OLD.captures(line) {
        push(&c["file"], diagnostic(&c["line"], Some(&c["col"]), 1, &crate::i18n::tf("test panicked: {}", &[&&c["msg"]])));
        return true;
    }
    if let Some(c) = RUST_PANIC.captures(line) {
        st.panic_at = Some((c["file"].to_string(), c["line"].to_string(), c["col"].to_string()));
        return true;
    }
    if let Some(c) = GO_TEST_LINE.captures(line) {
        push(&c["file"], diagnostic(&c["line"], None, 1, &crate::i18n::tf("test failed: {}", &[&&c["msg"]])));
        return true;
    }
    if let Some(c) = PYTEST_LINE.captures(line) {
        push(&c["file"], diagnostic(&c["line"], None, 1, &c["msg"]));
        return true;
    }
    if let Some(c) = RUSTC_AT.captures(line) {
        if let Some((sev, msg)) = st.pending.take() {
            push(&c["file"], diagnostic(&c["line"], Some(&c["col"]), sev, &msg));
        }
        return true;
    }
    if let Some(c) = RUSTC_HEAD.captures(line) {
        let code = c.name("code").map(|m| format!(" [{}]", m.as_str())).unwrap_or_default();
        st.pending = Some((severity(&c["sev"]), format!("{}{code}", &c["msg"])));
        return true;
    }
    if let Some(c) = PAREN.captures(line)
        && looks_like_path(&c["file"])
    {
        let msg = format!("{} [{}]", c["msg"].trim(), &c["code"]);
        push(&c["file"], diagnostic(&c["line"], c.name("col").map(|m| m.as_str()), severity(&c["sev"]), &msg));
        return true;
    }
    if let Some(c) = STYLISH_ITEM.captures(line)
        && let Some(file) = &st.stylish_file
    {
        let msg = match c.name("rule") {
            Some(r) => format!("{} [{}]", c["msg"].trim(), r.as_str()),
            None => c["msg"].trim().to_string(),
        };
        push(file, diagnostic(&c["line"], Some(&c["col"]), severity(&c["sev"]), &msg));
        return true;
    }
    if let Some(c) = COLON.captures(line) {
        let file = &c["file"];
        // A line with no column and no severity ("file.txt:3: text") does not count as a problem.
        let explicit = c.name("sev").is_some();
        if looks_like_path(file) && (explicit || c.name("col").is_some()) {
            let sev = c.name("sev").map_or(1, |m| severity(m.as_str()));
            push(file, diagnostic(&c["line"], c.name("col").map(|m| m.as_str()), sev, &c["msg"]));
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(text: &str) -> (String, Diagnostic) {
        let mut v = parse_output(text, "/proj");
        assert_eq!(v.len(), 1, "{v:?}");
        v.remove(0)
    }

    #[test]
    fn gcc_clang_and_go_lines() {
        let (p, d) = one("src/main.c:12:5: error: expected ';' before 'return'");
        assert_eq!((p.as_str(), d.start, d.severity, d.message.as_str()), ("/proj/src/main.c", (11, 4), 1, "expected ';' before 'return'"));
        let (_, d) = one("src/main.c:3:1: warning: unused variable 'x' [-Wunused-variable]");
        assert_eq!(d.severity, 2);
        // Go: no severity, has a column → error; "./" and ".." are simplified.
        let (p, d) = one("./cmd/../main.go:10:2: undefined: foo");
        assert_eq!((p.as_str(), d.start, d.severity), ("/proj/main.go", (9, 1), 1));
        // No column but has a severity (javac, mypy).
        let (_, d) = one("app.py:7: error: Incompatible types");
        assert_eq!((d.start, d.severity), ((6, 0), 1));
        assert_eq!(d.end.1, 10_000, "whole line when there is no column");
    }

    #[test]
    fn a_shell_prompt_glued_to_the_first_output_line_is_ignored() {
        for (line, file) in [
            ("# src/a.c:1:27: error: x undeclared", "/proj/src/a.c"),
            ("$ src/a.c:1:27: error: x undeclared", "/proj/src/a.c"),
            ("user@host:~/p$ src/a.c:1:27: error: x undeclared", "/proj/src/a.c"),
            ("sh-3.2$ ./cmd/main.go:3:2: undefined: foo", "/proj/cmd/main.go"),
            ("% src/app.ts(15,3): error TS2304: Cannot find name 'foo'.", "/proj/src/app.ts"),
        ] {
            let v = parse_output(line, "/proj");
            assert_eq!(v.len(), 1, "{line}: {v:?}");
            assert_eq!(v[0].0, file, "{line}");
        }
        // The command itself (prompt + typed command) is not a problem.
        assert!(parse_output("# printf 'src/a.c:1:27: error: x'", "/proj").is_empty());
        assert!(parse_output("$ cargo build", "/proj").is_empty());
    }

    #[test]
    fn ordinary_output_is_not_a_problem() {
        for line in [
            "Compiling foo v0.1.0 (/proj)",
            "Finished dev [unoptimized] target(s) in 0.2s",
            "notes.txt:3: remember the milk",
            "see https://example.com:8080/x.html:10:5: ok",
            "12:30:45: server started",
            "$ cargo build",
            "    Blocking waiting for file lock on build directory",
        ] {
            assert!(parse_output(line, "/proj").is_empty(), "{line}");
        }
    }

    #[test]
    fn rustc_header_pairs_with_the_next_arrow() {
        let text = "   Compiling app v0.1.0\nerror[E0425]: cannot find value `y` in this scope\n --> src/main.rs:4:9\n  |\n4 |     y + 1\n  |     ^ not found\n\nwarning: unused variable: `x`\n  --> src/lib.rs:2:5\n\nerror: could not compile `app` (bin \"app\") due to 1 previous error\n";
        let v = parse_output(text, "/proj");
        assert_eq!(v.len(), 2, "{v:?}");
        assert_eq!(v[0].0, "/proj/src/main.rs");
        assert_eq!((v[0].1.start, v[0].1.severity), ((3, 8), 1));
        assert_eq!(v[0].1.message, "cannot find value `y` in this scope [E0425]");
        assert_eq!((v[1].0.as_str(), v[1].1.severity), ("/proj/src/lib.rs", 2));
    }

    #[test]
    fn tsc_and_msbuild() {
        let (p, d) = one("src/app.ts(15,3): error TS2304: Cannot find name 'foo'.");
        assert_eq!((p.as_str(), d.start, d.severity), ("/proj/src/app.ts", (14, 2), 1));
        assert_eq!(d.message, "Cannot find name 'foo'. [TS2304]");
        let (_, d) = one("Program.cs(7,9): warning CS0168: The variable 'e' is declared but never used");
        assert_eq!(d.severity, 2);
    }

    #[test]
    fn eslint_stylish_blocks() {
        let text = "/proj/src/a.js\n  1:10  error    'x' is defined but never used  no-unused-vars\n  4:1   warning  Unexpected console statement    no-console\n\n/proj/src/b.js\n  9:2  error  Parsing error: Unexpected token\n\n✖ 3 problems (2 errors, 1 warning)\n";
        let v = parse_output(text, "/proj");
        assert_eq!(v.len(), 3, "{v:?}");
        assert_eq!((v[0].0.as_str(), v[0].1.start, v[0].1.message.as_str()), ("/proj/src/a.js", (0, 9), "'x' is defined but never used [no-unused-vars]"));
        assert_eq!(v[1].1.severity, 2);
        assert_eq!((v[2].0.as_str(), v[2].1.message.as_str()), ("/proj/src/b.js", "Parsing error: Unexpected token"));
    }

    #[test]
    fn repeated_lines_collapse_and_output_is_capped() {
        let line = "a.c:1:1: error: boom\n";
        assert_eq!(parse_output(&line.repeat(5), "/proj").len(), 1);
        let many: String = (1..5000).map(|i| format!("a.c:{i}:1: error: boom\n")).collect();
        assert_eq!(parse_output(&many, "/proj").len(), MAX_PROBLEMS);
    }

    #[test]
    fn windows_paths_stay_absolute() {
        let (p, _) = one("C:\\proj\\main.c:3:1: error: x");
        assert!(p.ends_with("main.c") && p.starts_with("C:"), "{p}");
    }

    #[test]
    fn windows_rust_diagnostics_match_workspace_paths_without_changing_posix_names() {
        for (root, file, expected) in [
            ("C:/work/demo", r"src\lib.rs", "C:/work/demo/src/lib.rs"),
            (r"C:\work\demo", r".\src\..\src\lib.rs", "C:/work/demo/src/lib.rs"),
            ("/unused", r"C:\work\demo\src\lib.rs", "C:/work/demo/src/lib.rs"),
            ("/unused", r"\\?\C:\work\demo\src\lib.rs", "C:/work/demo/src/lib.rs"),
            ("//server/share/demo", r"src\lib.rs", "//server/share/demo/src/lib.rs"),
            (r"\\?\UNC\server\share\demo", r"src\lib.rs", "//server/share/demo/src/lib.rs"),
            ("/srv/demo", r"src/name\literal.rs", r"/srv/demo/src/name\literal.rs"),
        ] {
            let found = parse_output(&format!("error[E0308]: mismatched types\n --> {file}:3:5\n"), root);
            assert_eq!(found.len(), 1, "{root}, {file}: {found:?}");
            assert_eq!(found[0].0, expected, "{root}, {file}");
            assert_eq!(found[0].1.start, (2, 4));
        }
    }

    #[test]
    fn windows_rust_panic_locations_keep_drive_letters_and_spaces() {
        for output in [
            "thread 'tests::works' (123) panicked at C:\\work space\\src\\lib.rs:12:9:\nassertion failed\n",
            "thread 'tests::works' panicked at 'assertion failed', C:\\work space\\src\\lib.rs:12:9\n",
        ] {
            let found = parse_output(output, "C:/work space");
            assert_eq!(found.len(), 1, "{found:?}");
            assert_eq!(found[0].0, "C:/work space/src/lib.rs");
            assert_eq!(found[0].1.start, (11, 8));
            assert_eq!(found[0].1.message, "test panicked: assertion failed");
        }
    }

    #[test]
    fn test_failures_from_cargo_go_and_pytest() {
        let rust = "running 1 test\ntest tests::adds ... FAILED\n\nfailures:\n\n---- tests::adds stdout ----\nthread 'tests::adds' (4242) panicked at src/lib.rs:12:9:\nassertion `left == right` failed\n  left: 1\n right: 2\n";
        let v = parse_output(rust, "/proj");
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!((v[0].0.as_str(), v[0].1.start), ("/proj/src/lib.rs", (11, 8)));
        assert_eq!(v[0].1.message, "test panicked: assertion `left == right` failed");
        let old = parse_output("thread 'main' panicked at 'boom', src/main.rs:3:5\n", "/proj");
        assert_eq!((old[0].0.as_str(), old[0].1.message.as_str()), ("/proj/src/main.rs", "test panicked: boom"));
        let go = "--- FAIL: TestA (0.00s)\n    a_test.go:14: expected 2, got 3\nFAIL\n";
        let v = parse_output(go, "/proj/pkg");
        assert_eq!((v[0].0.as_str(), v[0].1.start.0, v[0].1.message.as_str()), ("/proj/pkg/a_test.go", 13, "test failed: expected 2, got 3"));
        let py = "t/test_u.py:7: AssertionError\n";
        let v = parse_output(py, "/proj");
        assert_eq!((v[0].0.as_str(), v[0].1.message.as_str()), ("/proj/t/test_u.py", "AssertionError"));
    }
}
