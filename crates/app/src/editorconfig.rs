//! EditorConfig (https://editorconfig.org): finds `.editorconfig` files from the opened file up
//! to the root, matches sections by glob, and derives the rules to apply to the file.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use host::Host;

/// Rules to apply to the file; unspecified ones are `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rules {
    pub indent_tabs: Option<bool>,
    /// `Some(0)`: "tab" (use the tab width).
    pub indent_size: Option<usize>,
    pub tab_width: Option<usize>,
    pub eol: Option<buffer::LineEnding>,
    pub charset: Option<String>,
    pub trim_trailing_whitespace: Option<bool>,
    pub insert_final_newline: Option<bool>,
    pub max_line_length: Option<usize>,
}

impl Rules {
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        *self == Rules::default()
    }

    /// Indent unit (`"\t"` or n spaces). `detected`: the unit guessed from the file;
    /// the style is taken from it only if `indent_size` was given.
    pub fn indent_unit(&self, detected: &str) -> Option<String> {
        let width = self.tab_width.unwrap_or(4).max(1);
        let size = self.indent_size.map(|n| if n == 0 { width } else { n });
        let tabs = match self.indent_tabs {
            Some(t) => t,
            None if size.is_some() => detected == "\t",
            None => return None,
        };
        Some(if tabs { "\t".into() } else { " ".repeat(size.unwrap_or(width).clamp(1, 16)) })
    }

    fn set(&mut self, key: &str, value: &str) {
        let v = value.trim().to_ascii_lowercase();
        let unset = v == "unset";
        let flag = |v: &str| match v {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        };
        let num = |v: &str| v.parse::<usize>().ok().filter(|n| (1..=1000).contains(n));
        match key {
            "indent_style" => {
                self.indent_tabs = if unset {
                    None
                } else {
                    match v.as_str() {
                        "tab" => Some(true),
                        "space" => Some(false),
                        _ => self.indent_tabs,
                    }
                }
            }
            "indent_size" => {
                self.indent_size = if unset {
                    None
                } else if v == "tab" {
                    Some(0)
                } else {
                    num(&v).or(self.indent_size)
                }
            }
            "tab_width" => self.tab_width = if unset { None } else { num(&v).or(self.tab_width) },
            "end_of_line" => {
                self.eol = if unset {
                    None
                } else {
                    match v.as_str() {
                        "lf" => Some(buffer::LineEnding::Lf),
                        "crlf" => Some(buffer::LineEnding::Crlf),
                        "cr" => Some(buffer::LineEnding::Cr),
                        _ => self.eol,
                    }
                }
            }
            "charset" => self.charset = if unset { None } else { Some(v) },
            "trim_trailing_whitespace" => self.trim_trailing_whitespace = if unset { None } else { flag(&v).or(self.trim_trailing_whitespace) },
            "insert_final_newline" => self.insert_final_newline = if unset { None } else { flag(&v).or(self.insert_final_newline) },
            "max_line_length" => self.max_line_length = if unset || v == "off" { None } else { num(&v).or(self.max_line_length) },
            _ => {}
        }
    }
}

/// A section: the compiled glob (`None` if invalid or too long) and its properties.
#[derive(Clone, Debug)]
pub struct Section {
    pub re: Option<regex::Regex>,
    pub props: Vec<(String, String)>,
}

/// A single parsed `.editorconfig`.
#[derive(Clone, Debug, Default)]
pub struct File {
    pub root: bool,
    pub sections: Vec<Section>,
}

/// A glob longer than this is ignored; at most this many sections are read from one file
/// (so a malicious or huge `.editorconfig` doesn't slow down opening files).
const MAX_GLOB: usize = 1024;
const MAX_SECTIONS: usize = 500;

pub fn parse(text: &str) -> File {
    let mut file = File::default();
    for raw in text.lines() {
        let line = raw.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(inner) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            if file.sections.len() >= MAX_SECTIONS {
                break;
            }
            let re = glob_regex(inner);
            file.sections.push(Section { re, props: Vec::new() });
        } else if let Some((k, v)) = line.split_once('=') {
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_string());
            match file.sections.last_mut() {
                Some(sec) => sec.props.push((k, v)),
                None if k == "root" => file.root = v.eq_ignore_ascii_case("true"),
                None => {}
            }
        }
    }
    file
}

/// Maximum byte length of the regular expression generated from a glob.
const MAX_GLOB_REGEX: usize = 8 * 1024;

/// Convert an EditorConfig glob to a regular expression. A pattern without `/` matches the file name
/// at any depth; those containing one match relative to the `.editorconfig` folder.
pub fn glob_regex(glob: &str) -> Option<regex::Regex> {
    if glob.len() > MAX_GLOB {
        return None;
    }
    let chars: Vec<char> = glob.chars().collect();
    let anchored = chars.contains(&'/');
    let mut out = String::from("^");
    if !anchored {
        out.push_str("(?:.*/)?");
    }
    let mut i = if chars.first() == Some(&'/') { 1 } else { 0 };
    let mut braces = 0usize;
    while i < chars.len() {
        match chars[i] {
            '*' if chars.get(i + 1) == Some(&'*') => {
                out.push_str(".*");
                i += 1;
            }
            '*' => out.push_str("[^/]*"),
            '?' => out.push_str("[^/]"),
            '[' => match chars[i + 1..].iter().position(|&c| c == ']') {
                Some(end) => {
                    let body: String = chars[i + 1..i + 1 + end].iter().collect();
                    let (neg, body) = body.strip_prefix('!').map_or((false, body.as_str()), |b| (true, b));
                    if body.is_empty() {
                        return None;
                    }
                    out.push('[');
                    if neg {
                        out.push('^');
                    }
                    for c in body.chars() {
                        if matches!(c, '\\' | '[' | ']' | '^') {
                            out.push('\\');
                        }
                        out.push(c);
                    }
                    out.push(']');
                    i += end + 1;
                }
                None => out.push_str("\\["),
            },
            '{' => match chars[i + 1..].iter().position(|&c| c == '}') {
                Some(end) if chars[i + 1..i + 1 + end].contains(&',') || range_of(&chars[i + 1..i + 1 + end]).is_some() => {
                    let body: String = chars[i + 1..i + 1 + end].iter().collect();
                    if let Some((a, b)) = range_of(&chars[i + 1..i + 1 + end]) {
                        if a.abs_diff(b) > 2000 {
                            return None; // huge range: the pattern is treated as invalid (not silently truncated)
                        }
                        let alts: Vec<String> = (a.min(b)..=a.max(b)).map(|n| n.to_string()).collect();
                        out.push_str(&format!("(?:{})", alts.join("|")));
                    } else {
                        // Sub-patterns: simple patterns (nested braces are not supported).
                        let parts: Vec<String> = body.split(',').map(glob_regex_body).collect::<Option<_>>()?;
                        out.push_str(&format!("(?:{})", parts.join("|")));
                    }
                    i += end + 1;
                    braces += 1;
                }
                _ => out.push_str("\\{"),
            },
            '\\' if i + 1 < chars.len() => {
                i += 1;
                out.push_str(&regex::escape(&chars[i].to_string()));
            }
            c => out.push_str(&regex::escape(&c.to_string())),
        }
        i += 1;
    }
    let _ = braces;
    out.push('$');
    // A hostile .editorconfig must not make us compile huge expressions with hundreds of `{0..2000}` (compile time and memory).
    if out.len() > MAX_GLOB_REGEX {
        return None;
    }
    regex::RegexBuilder::new(&out).size_limit(256 * 1024).dfa_size_limit(1024 * 1024).build().ok()
}

/// Regular expression for one part inside `{a,b}` (`*`, `?` and letters).
fn glob_regex_body(part: &str) -> Option<String> {
    let re = glob_regex(part)?;
    let s = re.as_str();
    let s = s.strip_prefix('^')?.strip_suffix('$')?;
    Some(s.strip_prefix("(?:.*/)?").unwrap_or(s).to_string())
}

fn range_of(body: &[char]) -> Option<(i64, i64)> {
    let s: String = body.iter().collect();
    let (a, b) = s.split_once("..")?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// `files`: starting from the one closest to the root (outer → inner order) `(folder, file)`.
/// `path`: absolute file path.
pub fn rules_for(files: &[(String, Arc<File>)], path: &str) -> Rules {
    let mut rules = Rules::default();
    for (dir, file) in files {
        let prefix = format!("{}/", dir.trim_end_matches('/'));
        let Some(rel) = path.strip_prefix(&prefix) else { continue };
        for sec in &file.sections {
            if sec.re.as_ref().is_some_and(|re| re.is_match(rel)) {
                for (k, v) in &sec.props {
                    rules.set(k, v);
                }
            }
        }
    }
    rules
}

type Cached = (Instant, Option<Arc<File>>);
static CACHE: Mutex<Option<HashMap<String, Cached>>> = Mutex::new(None);
const TTL: Duration = Duration::from_secs(3);

fn read_file(host: &dyn Host, dir: &str) -> Option<Arc<File>> {
    // The same path is a different folder on another machine (local `/srv/app` vs `/srv/app` over SSH).
    let key = format!("{}\u{1}{dir}", host.label());
    {
        let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, f)) = g.get_or_insert_with(HashMap::new).get(&key)
            && at.elapsed() < TTL
        {
            return f.clone();
        }
    }
    let parsed = host
        .read(&format!("{}/.editorconfig", dir.trim_end_matches('/')))
        .ok()
        .filter(|(b, _)| b.len() <= 256 * 1024)
        .map(|(b, _)| Arc::new(parse(&String::from_utf8_lossy(&b))));
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    if map.len() > 512 {
        map.clear();
    }
    map.insert(key, (Instant::now(), parsed.clone()));
    parsed
}

/// Find the rules for the file (at most 16 parent folders; `root = true` stops).
pub fn resolve(host: &dyn Host, path: &str) -> Rules {
    let mut found: Vec<(String, Arc<File>)> = Vec::new();
    let mut dir = path.rsplit_once('/').map(|(d, _)| d.to_string());
    for _ in 0..16 {
        let Some(d) = dir else { break };
        if let Some(f) = read_file(host, if d.is_empty() { "/" } else { &d }) {
            let root = f.root;
            found.push((d.clone(), f));
            if root {
                break;
            }
        }
        dir = d.rsplit_once('/').map(|(p, _)| p.to_string()).filter(|_| !d.is_empty());
    }
    found.reverse();
    rules_for(&found, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_expansion_is_bounded() {
        let many = (0..110).map(|_| "{0..2000}").collect::<Vec<_>>().join("");
        assert!(glob_regex(&many).is_none(), "a pattern whose regex would be megabytes is refused");
        assert!(glob_regex(&"a".repeat(MAX_GLOB + 1)).is_none(), "the limit is the function's own, not only the parser's");
        let started = std::time::Instant::now();
        for g in [
            "{a,".repeat(MAX_GLOB / 3),
            "{".repeat(MAX_GLOB),
            "[".repeat(MAX_GLOB),
            "{a,b}".repeat(MAX_GLOB / 5),
            "{1..9}".repeat(MAX_GLOB / 6),
            "*".repeat(MAX_GLOB),
            "**/".repeat(MAX_GLOB / 3),
            "{{{{a,b},c},d},e}".repeat(60),
            "\\".repeat(MAX_GLOB / 2),
        ] {
            let _ = glob_regex(&g);
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "worst-case globs took {:?}", started.elapsed());
        assert!(glob_regex("*.{js,ts}").is_some());
        assert!(glob_regex("{0..99}.txt").is_some());
    }

    fn rules(cfg: &str, path: &str) -> Rules {
        rules_for(&[("/r".into(), Arc::new(parse(cfg)))], path)
    }

    #[test]
    fn parses_sections_and_properties() {
        let r = rules("root = true\n# c\n[*]\nindent_style = space\nindent_size = 2\n[*.md]\ntrim_trailing_whitespace = false\n", "/r/a/b.md");
        assert_eq!((r.indent_tabs, r.indent_size, r.trim_trailing_whitespace), (Some(false), Some(2), Some(false)));
        assert!(parse("root = true\n[*]\n").root);
    }

    #[test]
    fn later_sections_override_and_unset_clears() {
        let r = rules("[*]\nindent_size = 2\n[*.py]\nindent_size = 8\n[a.py]\nindent_size = unset\n", "/r/a.py");
        assert_eq!(r.indent_size, None);
        let r = rules("[*]\nindent_size = 2\n[*.py]\nindent_size = 8\n", "/r/x/a.py");
        assert_eq!(r.indent_size, Some(8));
    }

    #[test]
    fn glob_semantics() {
        let m = |g: &str, p: &str| glob_regex(g).unwrap().is_match(p);
        assert!(m("*.rs", "a.rs") && m("*.rs", "src/deep/a.rs"), "pattern without / matches at every depth");
        assert!(!m("*.rs", "a.rsx"));
        assert!(m("src/*.rs", "src/a.rs") && !m("src/*.rs", "src/x/a.rs"), "* does not match /");
        assert!(m("src/**.rs", "src/x/y/a.rs"));
        assert!(m("/Makefile", "Makefile") && !m("/Makefile", "sub/Makefile"));
        assert!(m("*.{js,ts}", "a.ts") && m("*.{js,ts}", "x/a.js") && !m("*.{js,ts}", "a.rs"));
        assert!(m("a?.txt", "ab.txt") && !m("a?.txt", "a/.txt"));
        assert!(m("[ab].c", "a.c") && !m("[!ab].c", "a.c") && m("[!ab].c", "z.c"));
        assert!(m("f{1..3}.x", "f2.x") && !m("f{1..3}.x", "f4.x"));
        assert!(m("{package.json,.travis.yml}", "package.json"));
        assert!(m("a\\*b", "a*b") && !m("a\\*b", "axb"));
        assert!(m("lib/**.js", "lib/a/b/c.js"));
    }

    #[test]
    fn indent_unit_resolution() {
        let r = rules("[*]\nindent_style = tab\n", "/r/a");
        assert_eq!(r.indent_unit("    "), Some("\t".into()));
        let r = rules("[*]\nindent_style = space\nindent_size = 3\n", "/r/a");
        assert_eq!(r.indent_unit("\t"), Some("   ".into()));
        let r = rules("[*]\nindent_size = 2\n", "/r/a");
        assert_eq!(r.indent_unit("    "), Some("  ".into()), "no style: guessed");
        assert_eq!(r.indent_unit("\t"), Some("\t".into()), "a tab-indented file keeps tabs");
        let r = rules("[*]\nindent_style = space\nindent_size = tab\ntab_width = 6\n", "/r/a");
        assert_eq!(r.indent_unit("\t"), Some("      ".into()));
        assert_eq!(Rules::default().indent_unit("    "), None);
    }

    #[test]
    fn invalid_values_are_ignored() {
        let r = rules("[*]\nindent_size = abc\nend_of_line = weird\ninsert_final_newline = maybe\nmax_line_length = 0\n", "/r/a");
        assert!(r.is_empty(), "{r:?}");
        let r = rules("[*]\nend_of_line = CRLF\nmax_line_length = 100\n[*]\nmax_line_length = off\n", "/r/a");
        assert_eq!((r.eol, r.max_line_length), (Some(buffer::LineEnding::Crlf), None));
    }

    #[test]
    fn nested_directories_inner_wins_and_root_stops() {
        let outer = ("/r".to_string(), Arc::new(parse("[*]\nindent_size = 2\ntrim_trailing_whitespace = true\n")));
        let inner = ("/r/sub".to_string(), Arc::new(parse("[*]\nindent_size = 4\n")));
        let r = rules_for(&[outer, inner], "/r/sub/a.rs");
        assert_eq!((r.indent_size, r.trim_trailing_whitespace), (Some(4), Some(true)));
        // Does not apply to a file outside the folder.
        assert!(rules_for(&[("/r/sub".into(), Arc::new(parse("[*]\nindent_size = 4\n")))], "/r/other/a.rs").is_empty());
    }

    #[test]
    fn resolve_walks_up_and_honours_root() {
        let dir = std::env::temp_dir().join(format!("codeyard-ec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join(".editorconfig"), "[*]\nindent_size = 8\ninsert_final_newline = true\n").unwrap();
        std::fs::write(dir.join("a/.editorconfig"), "root = true\n[*.rs]\nindent_style = tab\n").unwrap();
        let host = host::local();
        let root = dir.to_string_lossy().into_owned();
        let r = resolve(&host, &format!("{root}/a/b/x.rs"));
        assert_eq!(r.indent_tabs, Some(true));
        assert_eq!(r.insert_final_newline, None, "root = true cuts off the parent file");
        let r = resolve(&host, &format!("{root}/y.txt"));
        assert_eq!((r.indent_size, r.insert_final_newline), (Some(8), Some(true)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hostile_globs_do_not_panic() {
        for g in ["[", "]", "{", "}", "{a", "[a", "[]", "[!]", "{,}", "{1..}", "\\", "**", "{{a,b},c}", "a{b", "[\\]"] {
            let _ = glob_regex(g);
        }
    }

    #[test]
    fn hostile_files_are_cheap() {
        let t = std::time::Instant::now();
        // Huge unclosed `[`, very long glob, too many sections, huge range.
        let mut cfg = String::from("[");
        cfg.push_str(&"[".repeat(200_000));
        cfg.push_str("]\nindent_size = 3\n");
        cfg.push_str("[f{1..999999999}]\nindent_size = 5\n");
        for i in 0..5000 {
            cfg.push_str(&format!("[f{i}.x]\nindent_size = 2\n"));
        }
        let f = parse(&cfg);
        assert!(f.sections.len() <= 500);
        assert!(f.sections[0].re.is_none(), "overlong glob is invalid");
        assert!(f.sections[1].re.is_none(), "huge range is invalid, not truncated into a wrong match");
        assert!(rules_for(&[("/r".into(), Arc::new(f))], "/r/f5000.x").is_empty());
        // A quadratic blow-up on these inputs takes minutes, so the limit only has to be far from the ~0.4 s a laptop needs;
        // CI runners are several times slower.
        let limit = 5_000 * u128::from(crate::test_sshd::timeout_scale());
        assert!(t.elapsed().as_millis() < limit, "{:?}", t.elapsed());
    }
}
