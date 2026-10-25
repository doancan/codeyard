//! "Test: Run Test at Cursor" / "Run Tests in File": builds the shell command that runs the
//! test at the cursor (or all in the file). No test runner of our own; the project's own
//! command (cargo, go, jest / vitest, pytest) runs in the task terminal, so output, rerun
//! and Problems scanning take the same path as tasks.
//!
//! Plain text only (no file reads): lookup uses the root-relative path and the file text.

use regex::Regex;
use std::sync::LazyLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestRun {
    pub label: String,
    pub command: String,
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// A shell word: left as it is when nothing in it means anything to a shell, quoted otherwise. Folder and file names come from the
/// repository, and `crates/$(cmd)/src/a.rs` is a legal path.
fn word(s: &str) -> String {
    let plain = !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | ':' | '-' | '+' | ',' | '@' | '='));
    if plain && !s.starts_with('-') { s.to_string() } else { quote(s) }
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

static RUST_FN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+(\w+)").unwrap());
static RUST_MOD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*\{").unwrap());
static GO_TEST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^func\s+(Test\w*)\s*\(").unwrap());
static JS_CASE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*(it|test|describe)(?:\.\w+)*\(\s*(?:'((?:[^'\\]|\\.)*)'|"((?:[^"\\]|\\.)*)"|`([^`]*)`)"#).unwrap());
static PY_DEF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\s*)(?:async\s+)?def\s+(test\w*)\s*\(").unwrap());
static PY_CLASS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\s*)class\s+(Test\w*)").unwrap());

/// `rel`: root-relative path, `text`: file contents, `cursor`: 0-based line.
/// `whole_file`: all tests in the file. `package_json`: the root package.json (for JS).
pub fn plan(rel: &str, text: &str, cursor: usize, whole_file: bool, package_json: Option<&str>) -> Option<TestRun> {
    let lines: Vec<&str> = text.lines().collect();
    let cursor = cursor.min(lines.len().saturating_sub(1));
    let ext = rel.rsplit_once('.').map(|(_, e)| e)?;
    match ext {
        "rs" => rust(rel, &lines, cursor, whole_file),
        "go" if rel.ends_with("_test.go") => go(rel, &lines, cursor, whole_file),
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => js(rel, &lines, cursor, whole_file, package_json),
        "py" => python(rel, &lines, cursor, whole_file),
        _ => None,
    }
}

fn rust(rel: &str, lines: &[&str], cursor: usize, whole_file: bool) -> Option<TestRun> {
    // `crates/app/src/x.rs` → `-p app`; `tests/x.rs` → `--test x`.
    let parts: Vec<&str> = rel.split('/').collect();
    let (pkg, inner) = match parts.as_slice() {
        ["crates", name, rest @ ..] => (Some(*name), rest.to_vec()),
        rest => (None, rest.to_vec()),
    };
    let mut base = String::from("cargo test");
    if let Some(p) = pkg {
        base.push_str(&format!(" -p {}", word(p)));
    }
    let stem = inner.last()?.trim_end_matches(".rs");
    let integration = inner.first() == Some(&"tests") && inner.len() == 2;
    if integration {
        base.push_str(&format!(" --test {}", word(stem)));
    }
    // The file's module path: src/foo/bar.rs → foo::bar; lib.rs / main.rs / mod.rs yield their own folder.
    let module: Vec<&str> = if integration || inner.first() != Some(&"src") {
        Vec::new()
    } else {
        let mut m: Vec<&str> = inner[1..inner.len() - 1].to_vec();
        if !matches!(stem, "lib" | "main" | "mod") {
            m.push(stem);
        }
        m
    };
    if whole_file {
        let command = if module.is_empty() { base } else { format!("{base} {}", word(&format!("{}::", module.join("::")))) };
        return Some(TestRun { label: format!("test: {rel}"), command });
    }
    // The nearest #[test] function at (or above) the cursor.
    for i in (0..=cursor).rev() {
        let Some(c) = RUST_FN.captures(lines[i]) else { continue };
        let is_test = (0..i)
            .rev()
            .map(|j| lines[j].trim())
            .take_while(|l| l.starts_with("#[") || l.starts_with("///") || l.is_empty() || l.starts_with("//"))
            .any(|l| l.starts_with("#[") && (l == "#[test]" || l.ends_with("::test]") || l.starts_with("#[test_case") || l.starts_with("#[rstest")));
        if !is_test {
            // Cursor is in a non-test function: running a test above it would be wrong.
            return None;
        }
        // Collect enclosing `mod x {` blocks, outermost first.
        let mut path: Vec<String> = module.iter().map(|s| s.to_string()).collect();
        let (mut at, mut want) = (i, indent(lines[i]));
        let mut mods: Vec<String> = Vec::new();
        while at > 0 && want > 0 {
            at -= 1;
            if let Some(m) = RUST_MOD.captures(lines[at])
                && indent(lines[at]) < want
            {
                mods.insert(0, m[1].to_string());
                want = indent(lines[at]);
            }
        }
        path.extend(mods);
        path.push(c[1].to_string());
        return Some(TestRun { label: format!("test: {}", &c[1]), command: format!("{base} {} -- --exact", word(&path.join("::"))) });
    }
    None
}

fn go(rel: &str, lines: &[&str], cursor: usize, whole_file: bool) -> Option<TestRun> {
    let dir = rel.rsplit_once('/').map_or(".", |(d, _)| d);
    let pkg = if dir == "." { "./.".to_string() } else { word(&format!("./{dir}")) };
    let all: Vec<String> = lines.iter().filter_map(|l| GO_TEST.captures(l).map(|c| c[1].to_string())).collect();
    if whole_file {
        if all.is_empty() {
            return None;
        }
        return Some(TestRun { label: format!("test: {rel}"), command: format!("go test {pkg} -run {}", quote(&format!("^({})$", all.join("|")))) });
    }
    let name = (0..=cursor).rev().find_map(|i| GO_TEST.captures(lines[i]).map(|c| c[1].to_string()))?;
    Some(TestRun { label: format!("test: {name}"), command: format!("go test {pkg} -run {}", quote(&format!("^{name}$"))) })
}

fn js(rel: &str, lines: &[&str], cursor: usize, whole_file: bool, package_json: Option<&str>) -> Option<TestRun> {
    let pkg: serde_json::Value = package_json.and_then(|p| serde_json::from_str(p).ok()).unwrap_or_default();
    let has = |n: &str| ["dependencies", "devDependencies"].iter().any(|k| pkg[*k].get(n).is_some());
    let runner = if has("vitest") {
        "npx vitest run"
    } else if has("jest") || has("ts-jest") {
        "npx jest"
    } else if pkg["scripts"]["test"].is_string() {
        "npm test --"
    } else {
        return None;
    };
    if whole_file {
        return Some(TestRun { label: format!("test: {rel}"), command: format!("{runner} {}", quote(rel)) });
    }
    // Nearest it/test; if none, the nearest describe.
    let case = |kinds: &[&str]| {
        (0..=cursor).rev().find_map(|i| {
            let c = JS_CASE.captures(lines[i])?;
            kinds.contains(&&c[1]).then(|| c.get(2).or(c.get(3)).or(c.get(4)).map(|m| m.as_str().to_string()).unwrap_or_default())
        })
    };
    let name = case(&["it", "test"]).or_else(|| case(&["describe"]))?;
    Some(TestRun { label: format!("test: {name}"), command: format!("{runner} {} -t {}", quote(rel), quote(&name)) })
}

fn python(rel: &str, lines: &[&str], cursor: usize, whole_file: bool) -> Option<TestRun> {
    if whole_file {
        return Some(TestRun { label: format!("test: {rel}"), command: format!("python3 -m pytest {}", quote(rel)) });
    }
    let (i, def) = (0..=cursor).rev().find_map(|i| PY_DEF.captures(lines[i]).map(|c| (i, (c[1].len(), c[2].to_string()))))?;
    // If inside a class (indent > 0), the nearest less-indented `class Test…` above.
    let mut id = rel.to_string();
    if def.0 > 0
        && let Some(class) = (0..i).rev().find_map(|j| PY_CLASS.captures(lines[j]).filter(|c| c[1].len() < def.0).map(|c| c[2].to_string()))
    {
        id.push_str(&format!("::{class}"));
    }
    id.push_str(&format!("::{}", def.1));
    Some(TestRun { label: format!("test: {}", def.1), command: format!("python3 -m pytest {}", quote(&id)) })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUST: &str = "pub fn helper() {}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn adds_up() {\n        assert_eq!(1 + 1, 2);\n    }\n\n    fn not_a_test() {}\n\n    #[tokio::test]\n    async fn async_one() {}\n}\n";

    #[test]
    fn rust_test_at_cursor_uses_full_path_and_package() {
        let r = plan("crates/app/src/problems.rs", RUST, 8, false, None).unwrap();
        assert_eq!(r.command, "cargo test -p app problems::tests::adds_up -- --exact");
        let r = plan("src/lib.rs", RUST, 14, false, None).unwrap();
        assert_eq!(r.command, "cargo test tests::async_one -- --exact", "lib.rs is the root module; async attribute ends in ::test");
        let r = plan("src/foo/bar.rs", RUST, 7, false, None).unwrap();
        assert_eq!(r.command, "cargo test foo::bar::tests::adds_up -- --exact");
        let r = plan("tests/api.rs", "#[test]\nfn works() {}\n", 1, false, None).unwrap();
        assert_eq!(r.command, "cargo test --test api works -- --exact");
    }

    #[test]
    fn rust_cursor_outside_a_test_finds_nothing_and_file_mode_filters_by_module() {
        assert_eq!(plan("src/lib.rs", RUST, 0, false, None), None, "not a test function");
        assert_eq!(plan("src/lib.rs", RUST, 11, false, None), None, "not_a_test is not a test");
        assert_eq!(plan("crates/app/src/find.rs", RUST, 0, true, None).unwrap().command, "cargo test -p app find::");
        assert_eq!(plan("src/main.rs", RUST, 0, true, None).unwrap().command, "cargo test");
    }

    /// Paths come from the repository: whatever a folder is called, it stays one word of the command.
    #[test]
    fn hostile_path_segments_are_quoted() {
        let r = plan("crates/$(touch x)/src/a.rs", "#[test]\nfn t() {}\n", 1, false, None).unwrap();
        assert_eq!(r.command, "cargo test -p '$(touch x)' a::t -- --exact");
        let r = plan("crates/app/src/$(id)/`x`.rs", "#[test]\nfn t() {}\n", 1, true, None).unwrap();
        assert_eq!(r.command, "cargo test -p app '$(id)::`x`::'");
        let r = plan("tests/a;b.rs", "#[test]\nfn t() {}\n", 1, false, None).unwrap();
        assert_eq!(r.command, "cargo test --test 'a;b' t -- --exact");
        let r = plan("pkg/$(id)/a_test.go", "func TestA(t *testing.T) {}\n", 0, false, None).unwrap();
        assert_eq!(r.command, "go test './pkg/$(id)' -run '^TestA$'");
    }

    #[test]
    fn go_tests() {
        let src = "package x\n\nfunc TestA(t *testing.T) {\n\tt.Log(1)\n}\n\nfunc TestB(t *testing.T) {}\n";
        assert_eq!(plan("pkg/x/a_test.go", src, 3, false, None).unwrap().command, "go test ./pkg/x -run '^TestA$'");
        assert_eq!(plan("pkg/x/a_test.go", src, 0, true, None).unwrap().command, "go test ./pkg/x -run '^(TestA|TestB)$'");
        assert_eq!(plan("a_test.go", src, 3, false, None).unwrap().command, "go test ./. -run '^TestA$'");
        assert_eq!(plan("pkg/x/a.go", src, 3, false, None), None, "not a _test.go file");
    }

    #[test]
    fn js_picks_runner_from_package_json_and_escapes_names() {
        let src = "describe('math', () => {\n  it(\"adds it's\", () => {\n    expect(1).toBe(1);\n  });\n  test.only(`subtracts`, () => {});\n});\n";
        let vitest = r#"{ "devDependencies": { "vitest": "^1" } }"#;
        let jest = r#"{ "devDependencies": { "jest": "^29" } }"#;
        assert_eq!(plan("src/m.test.ts", src, 2, false, Some(vitest)).unwrap().command, r"npx vitest run 'src/m.test.ts' -t 'adds it'\''s'");
        assert_eq!(plan("src/m.test.ts", src, 4, false, Some(jest)).unwrap().command, "npx jest 'src/m.test.ts' -t 'subtracts'");
        assert_eq!(plan("src/m.test.ts", src, 0, false, Some(jest)).unwrap().command, "npx jest 'src/m.test.ts' -t 'math'", "describe when there is no it");
        assert_eq!(plan("src/m.test.ts", src, 0, true, Some(r#"{ "scripts": { "test": "mocha" } }"#)).unwrap().command, "npm test -- 'src/m.test.ts'");
        assert_eq!(plan("src/m.test.ts", src, 2, false, Some("{}")), None, "unknown runner");
        assert_eq!(plan("src/m.test.ts", src, 2, false, None), None);
    }

    #[test]
    fn python_class_and_function() {
        let src = "def test_top():\n    pass\n\nclass TestUser:\n    def helper(self):\n        pass\n\n    def test_name(self):\n        assert True\n";
        assert_eq!(plan("t/test_u.py", src, 8, false, None).unwrap().command, "python3 -m pytest 't/test_u.py::TestUser::test_name'");
        assert_eq!(plan("t/test_u.py", src, 1, false, None).unwrap().command, "python3 -m pytest 't/test_u.py::test_top'");
        assert_eq!(plan("t/test_u.py", src, 0, true, None).unwrap().command, "python3 -m pytest 't/test_u.py'");
    }

    #[test]
    fn unknown_languages_have_no_plan() {
        assert_eq!(plan("README.md", "# x", 0, false, None), None);
        assert_eq!(plan("Makefile", "all:", 0, true, None), None);
    }
}
