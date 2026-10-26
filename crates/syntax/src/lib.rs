//! Syntax highlighting: tree-sitter, incremental. Only the visible byte range
//! is queried; language queries compile on first use (an unused language costs no memory).

use std::ops::Range;
use std::sync::{Mutex, OnceLock};

use ropey::Rope;
use tree_sitter::{InputEdit, Parser, Point, Query, QueryCursor, StreamingIterator, TextProvider, Tree};

pub mod structural;

/// Temadaki syntax rolleri.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Keyword,
    String,
    Number,
    Function,
    Type,
    Comment,
    Tag,
    Punct,
}

/// Capture name -> role, by prefix: "function.method" -> Function.
fn role_for_capture(name: &str) -> Option<Role> {
    // Markdown (nvim-treesitter names): heading, code, link.
    match name {
        "text.title" => return Some(Role::Keyword),
        "text.literal" => return Some(Role::String),
        "text.uri" => return Some(Role::Function),
        "text.reference" => return Some(Role::Type),
        _ => {}
    }
    let head = name.split('.').next().unwrap_or(name);
    Some(match head {
        "keyword" | "conditional" | "repeat" | "include" | "exception" | "storageclass" => Role::Keyword,
        "string" | "escape" | "char" | "character" => Role::String,
        "number" | "float" | "boolean" | "constant" => Role::Number,
        "function" | "method" => Role::Function,
        "type" | "constructor" => Role::Type,
        "comment" => Role::Comment,
        "tag" | "attribute" | "label" => Role::Tag,
        "punctuation" | "operator" | "delimiter" => Role::Punct,
        _ => return None,
    })
}

pub struct Language {
    pub name: &'static str,
    ts: tree_sitter::Language,
    query_src: &'static [&'static str],
    compiled: OnceLock<Option<(Query, Vec<Option<Role>>)>>,
}

impl Language {
    fn query(&self) -> Option<&(Query, Vec<Option<Role>>)> {
        self.compiled
            .get_or_init(|| {
                let src = self.query_src.concat();
                match Query::new(&self.ts, &src) {
                    Ok(q) => {
                        let roles = q.capture_names().iter().map(|n| role_for_capture(n)).collect();
                        Some((q, roles))
                    }
                    Err(e) => {
                        eprintln!("syntax: {} query failed to compile: {e}", self.name);
                        None
                    }
                }
            })
            .as_ref()
    }
}

// The `tree-sitter-dockerfile` crate also pulls in an old tree-sitter runtime (duplicate symbols), so
// the parser's C source lives under `grammars/dockerfile` (MIT, camdencheek) and is built by `build.rs`.
unsafe extern "C" {
    fn tree_sitter_dockerfile() -> *const ();
}
// SAFETY: `tree_sitter_dockerfile` is the generated parser entry point built by `build.rs`; it takes no
// arguments and returns a pointer to a static `TSLanguage`.
const DOCKERFILE: tree_sitter_language::LanguageFn = unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_dockerfile) };

macro_rules! lang {
    ($name:literal, $ts:expr, [$($q:expr),+]) => {{
        static L: OnceLock<Language> = OnceLock::new();
        L.get_or_init(|| Language { name: $name, ts: $ts.into(), query_src: &[$($q),+], compiled: OnceLock::new() })
    }};
}

/// Language from a file name. Unknown: `None` (plain text).
pub fn language_for_path(path: &str) -> Option<&'static Language> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    use tree_sitter_javascript as js;
    Some(match ext.as_str() {
        "rs" => lang!("Rust", tree_sitter_rust::LANGUAGE, [tree_sitter_rust::HIGHLIGHTS_QUERY]),
        "js" | "mjs" | "cjs" => lang!("JavaScript", js::LANGUAGE, [js::HIGHLIGHT_QUERY]),
        "jsx" => lang!("JavaScript JSX", js::LANGUAGE, [js::HIGHLIGHT_QUERY, js::JSX_HIGHLIGHT_QUERY]),
        // The TS query holds only TS-specific rules; it is combined with the JS query.
        "ts" | "mts" | "cts" => {
            lang!("TypeScript", tree_sitter_typescript::LANGUAGE_TYPESCRIPT, [tree_sitter_typescript::HIGHLIGHTS_QUERY, js::HIGHLIGHT_QUERY])
        }
        "tsx" => lang!(
            "TypeScript JSX",
            tree_sitter_typescript::LANGUAGE_TSX,
            [tree_sitter_typescript::HIGHLIGHTS_QUERY, js::JSX_HIGHLIGHT_QUERY, js::HIGHLIGHT_QUERY]
        ),
        "py" | "pyi" => lang!("Python", tree_sitter_python::LANGUAGE, [tree_sitter_python::HIGHLIGHTS_QUERY]),
        "go" => lang!("Go", tree_sitter_go::LANGUAGE, [tree_sitter_go::HIGHLIGHTS_QUERY]),
        "json" | "jsonc" => lang!("JSON", tree_sitter_json::LANGUAGE, [tree_sitter_json::HIGHLIGHTS_QUERY]),
        "sh" | "bash" | "zsh" => lang!("Shell", tree_sitter_bash::LANGUAGE, [tree_sitter_bash::HIGHLIGHT_QUERY]),
        "css" => lang!("CSS", tree_sitter_css::LANGUAGE, [tree_sitter_css::HIGHLIGHTS_QUERY]),
        "html" | "htm" => lang!("HTML", tree_sitter_html::LANGUAGE, [tree_sitter_html::HIGHLIGHTS_QUERY]),
        "toml" => lang!("TOML", tree_sitter_toml_ng::LANGUAGE, [tree_sitter_toml_ng::HIGHLIGHTS_QUERY]),
        "yaml" | "yml" => lang!("YAML", tree_sitter_yaml::LANGUAGE, [tree_sitter_yaml::HIGHLIGHTS_QUERY]),
        // The C++ query layers on top of C's (nvim-treesitter's `; inherits: c`).
        "c" | "h" => lang!("C", tree_sitter_c::LANGUAGE, [tree_sitter_c::HIGHLIGHT_QUERY]),
        "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" | "ipp" => {
            lang!("C++", tree_sitter_cpp::LANGUAGE, [tree_sitter_c::HIGHLIGHT_QUERY, tree_sitter_cpp::HIGHLIGHT_QUERY])
        }
        "java" => lang!("Java", tree_sitter_java::LANGUAGE, [tree_sitter_java::HIGHLIGHTS_QUERY]),
        "cs" => lang!("C#", tree_sitter_c_sharp::LANGUAGE, [tree_sitter_c_sharp::HIGHLIGHTS_QUERY]),
        "rb" | "rake" | "gemspec" => lang!("Ruby", tree_sitter_ruby::LANGUAGE, [tree_sitter_ruby::HIGHLIGHTS_QUERY]),
        "php" => lang!("PHP", tree_sitter_php::LANGUAGE_PHP, [tree_sitter_php::HIGHLIGHTS_QUERY]),
        "swift" => lang!("Swift", tree_sitter_swift::LANGUAGE, [tree_sitter_swift::HIGHLIGHTS_QUERY]),
        "kt" | "kts" => lang!("Kotlin", tree_sitter_kotlin_ng::LANGUAGE, [include_str!("../queries/kotlin.scm")]),
        "sql" => lang!("SQL", tree_sitter_sequel::LANGUAGE, [tree_sitter_sequel::HIGHLIGHTS_QUERY]),
        "lua" => lang!("Lua", tree_sitter_lua::LANGUAGE, [tree_sitter_lua::HIGHLIGHTS_QUERY]),
        "dockerfile" => lang!("Dockerfile", DOCKERFILE, [include_str!("../queries/dockerfile.scm")]),
        "mk" | "mak" => lang!("Makefile", tree_sitter_make::LANGUAGE, [tree_sitter_make::HIGHLIGHTS_QUERY]),
        // Block grammar: headings, code blocks, lists, quotes, link targets.
        "md" | "markdown" => lang!("Markdown", tree_sitter_md::LANGUAGE, [tree_sitter_md::HIGHLIGHT_QUERY_BLOCK]),
        _ => match name {
            ".bashrc" | ".zshrc" | ".profile" | ".bash_profile" => {
                lang!("Shell", tree_sitter_bash::LANGUAGE, [tree_sitter_bash::HIGHLIGHT_QUERY])
            }
            "Makefile" | "makefile" | "GNUmakefile" => {
                lang!("Makefile", tree_sitter_make::LANGUAGE, [tree_sitter_make::HIGHLIGHTS_QUERY])
            }
            "Rakefile" | "Gemfile" | "Podfile" => {
                lang!("Ruby", tree_sitter_ruby::LANGUAGE, [tree_sitter_ruby::HIGHLIGHTS_QUERY])
            }
            n if n == "Dockerfile" || n.starts_with("Dockerfile.") || n == "Containerfile" => {
                lang!("Dockerfile", DOCKERFILE, [include_str!("../queries/dockerfile.scm")])
            }
            "Cargo.lock" => lang!("TOML", tree_sitter_toml_ng::LANGUAGE, [tree_sitter_toml_ng::HIGHLIGHTS_QUERY]),
            _ => return None,
        },
    })
}

/// Feeds the rope to tree-sitter chunk by chunk, without copying.
struct RopeText<'a>(&'a Rope);

impl<'a> TextProvider<&'a [u8]> for RopeText<'a> {
    type I = Box<dyn Iterator<Item = &'a [u8]> + 'a>;
    fn text(&mut self, node: tree_sitter::Node) -> Self::I {
        let slice = self.0.byte_slice(node.byte_range());
        Box::new(slice.chunks().map(str::as_bytes))
    }
}

/// Route tree-sitter's C code to mimalloc. The system malloc doesn't return
/// freed small blocks to the OS, so the parser's temporary buffers (~7 MB on a
/// large file) would become permanent. Call once at startup, before the first
/// parse.
///
/// # Safety
/// No tree-sitter parser, tree or query may exist yet, and this must not run concurrently with
/// one being created: memory allocated by the old allocator would be freed by mimalloc.
pub unsafe fn use_mimalloc() {
    use libmimalloc_sys as mi;
    // SAFETY: the caller guarantees no tree-sitter object exists yet; all four
    // functions belong to the same allocator.
    unsafe {
        tree_sitter::set_allocator(Some(tree_sitter::Allocator { malloc: mi::mi_malloc, calloc: mi::mi_calloc, realloc: mi::mi_realloc, free: mi::mi_free }));
    }
}

/// All files share one parser so its internal buffers (megabytes on a large
/// file) are paid once, not per file. Measurement: examples/parser_vs_tree.rs
static PARSER: Mutex<Option<Parser>> = Mutex::new(None);

pub struct Syntax {
    lang: &'static Language,
    /// `None`: dropped from memory for a background tab (`unload`).
    tree: Option<Tree>,
}

/// (line, byte within line)
pub type Pos = (usize, usize);

/// An edit, in the form tree-sitter expects.
#[derive(Debug, Clone, Copy)]
pub struct Edit {
    pub start_byte: usize,
    pub old_end_byte: usize,
    pub new_end_byte: usize,
    pub start: Pos,
    pub old_end: Pos,
    pub new_end: Pos,
}

fn pt((row, column): Pos) -> Point {
    Point { row, column }
}

impl Syntax {
    pub fn new(lang: &'static Language, rope: &Rope) -> Option<Self> {
        let mut s = Syntax { lang, tree: None };
        s.reparse(rope);
        s.tree.is_some().then_some(s)
    }

    /// Drop the tree (tab went to the background). A tree takes ~30x the source size.
    pub fn unload(&mut self) {
        self.tree = None;
    }

    pub fn is_loaded(&self) -> bool {
        self.tree.is_some()
    }

    /// Reparse from scratch if it was dropped.
    pub fn ensure_loaded(&mut self, rope: &Rope) {
        if self.tree.is_none() {
            self.reparse(rope);
        }
    }

    pub fn language_name(&self) -> &'static str {
        self.lang.name
    }

    pub fn edit(&mut self, e: &Edit) {
        if let Some(tree) = &mut self.tree {
            tree.edit(&InputEdit {
                start_byte: e.start_byte,
                old_end_byte: e.old_end_byte,
                new_end_byte: e.new_end_byte,
                start_position: pt(e.start),
                old_end_position: pt(e.old_end),
                new_end_position: pt(e.new_end),
            });
        }
    }

    /// Reparse using the previous tree (only the changed part is processed).
    pub fn reparse(&mut self, rope: &Rope) {
        let mut read = |byte: usize, _: Point| -> &[u8] {
            if byte >= rope.len_bytes() {
                return &[];
            }
            let (chunk, start, _, _) = rope.chunk_at_byte(byte);
            &chunk.as_bytes()[byte - start..]
        };
        let mut guard = PARSER.lock().unwrap_or_else(|e| e.into_inner());
        let parser = guard.get_or_insert_with(Parser::new);
        if parser.set_language(&self.lang.ts).is_err() {
            self.tree = None;
            return;
        }
        self.tree = parser.parse_with_options(&mut read, self.tree.as_ref(), None);
    }

    /// Smallest node strictly containing the range (for expanding the selection, bytes).
    pub fn enclosing_node(&self, bytes: Range<usize>) -> Option<Range<usize>> {
        let tree = self.tree.as_ref()?;
        let mut node = tree.root_node().descendant_for_byte_range(bytes.start, bytes.end)?;
        loop {
            let r = node.byte_range();
            if r.start <= bytes.start && r.end >= bytes.end && r != bytes {
                return Some(r);
            }
            node = node.parent()?;
        }
    }

    /// Colored spans within the given byte range. For nested nodes the narrow one
    /// (e.g. an escape inside a string) overrides the wide one, so spans come back
    /// wide -> narrow and the caller can apply them in order.
    pub fn highlights(&self, rope: &Rope, range: Range<usize>) -> Vec<(Range<usize>, Role)> {
        let (Some(tree), Some((query, roles))) = (&self.tree, self.lang.query()) else { return Vec::new() };
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(range.clone());
        let mut out: Vec<(Range<usize>, Role, usize)> = Vec::new();
        let mut captures = cursor.captures(query, tree.root_node(), RopeText(rope));
        while let Some((m, ix)) = captures.next() {
            let cap = m.captures()[*ix];
            if let Some(role) = roles[cap.index as usize] {
                let r = cap.node.byte_range();
                if r.end > range.start && r.start < range.end {
                    out.push((r, role, m.pattern_index));
                }
            }
        }
        // Wide first; for an identical range the pattern that comes first in the query wins (it is put last).
        out.sort_by(|a, b| (b.0.end - b.0.start).cmp(&(a.0.end - a.0.start)).then(b.2.cmp(&a.2)));
        out.into_iter().map(|(r, role, _)| (r, role)).collect()
    }
}

/// Color run for one line: `(byte length, role)` pieces covering the whole line.
/// `spans` must be ordered wide -> narrow (the output of `highlights`).
pub fn line_runs(line_start: usize, line_len: usize, spans: &[(Range<usize>, Role)]) -> Vec<(usize, Option<Role>)> {
    let mut roles: Vec<Option<Role>> = vec![None; line_len];
    let line_end = line_start + line_len;
    for (r, role) in spans {
        let a = r.start.max(line_start);
        let b = r.end.min(line_end);
        for slot in roles.iter_mut().take(b.saturating_sub(line_start)).skip(a.saturating_sub(line_start)) {
            *slot = Some(*role);
        }
    }
    let mut runs: Vec<(usize, Option<Role>)> = Vec::new();
    for role in roles {
        match runs.last_mut() {
            Some((len, last)) if *last == role => *len += 1,
            _ => runs.push((1, role)),
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roles_of(path: &str, src: &str) -> Vec<(String, Role)> {
        let rope = Rope::from_str(src);
        let s = Syntax::new(language_for_path(path).unwrap(), &rope).unwrap();
        let mut spans = s.highlights(&rope, 0..rope.len_bytes());
        spans.sort_by_key(|(r, _)| r.start);
        spans.into_iter().map(|(r, role)| (src[r].to_string(), role)).collect()
    }

    /// Every arm of `language_for_path`, not a hand-kept list: the arms are read from this file's own source, so a language added later is
    /// compiled by this test without anyone remembering to add it. A query that fails to compile makes the language silently plain text.
    #[test]
    fn every_language_query_compiles() {
        let src = include_str!("lib.rs");
        let body = src.split("pub fn language_for_path").nth(1).and_then(|t| t.split("/// Feeds the rope").next()).expect("language_for_path");
        let (by_ext, by_name) = body.split_once("_ => match name {").expect("the file-name arms");
        // Left of `=>`, the quoted tokens of each arm: extensions in the first half, whole file names in the second.
        let tokens = |text: &str| -> Vec<String> {
            let mut out = Vec::new();
            for line in text.lines().filter(|l| l.contains("=>")) {
                let left = line.split("=>").next().unwrap_or("");
                out.extend(left.split('"').skip(1).step_by(2).map(str::to_string));
            }
            out
        };
        let mut paths: Vec<String> = tokens(by_ext).into_iter().map(|e| format!("a.{e}")).collect();
        // `starts_with("Dockerfile.")` is a prefix, not a name.
        paths.extend(tokens(by_name).into_iter().map(|n| if n.ends_with('.') { format!("{n}dev") } else { n }));
        assert!(paths.len() > 55, "only {} paths were derived from language_for_path; did its shape change?", paths.len());
        let mut names = std::collections::BTreeSet::new();
        for p in &paths {
            let l = language_for_path(p).unwrap_or_else(|| panic!("{p} is listed in language_for_path but resolves to no language"));
            assert!(l.query().is_some(), "the {} query ({p}) does not compile", l.name);
            names.insert(l.name);
        }
        // The derivation above must really reach the languages that ship their own query file.
        for must in ["Kotlin", "Dockerfile", "Markdown", "TypeScript JSX", "C++"] {
            assert!(names.contains(must), "{must} was not reached ({names:?})");
        }
        assert!(language_for_path("README").is_none());
    }

    /// A query that compiles but captures nothing the theme knows would still pass the test above and leave the file uncoloured.
    #[test]
    #[allow(clippy::type_complexity)]
    fn older_languages_color_comment_string_number() {
        let cases: &[(&str, &str, &[(&str, Role)])] = &[
            (
                "a.go",
                "package main\n// c\nfunc f() string { return \"x\" }\nvar n = 42\n",
                &[("// c", Role::Comment), ("func", Role::Keyword), ("\"x\"", Role::String), ("42", Role::Number)],
            ),
            ("a.json", "{\"a\": 1, \"b\": true}\n", &[("\"a\"", Role::String), ("1", Role::Number)]),
            ("a.sh", "# c\nif true; then echo \"x\"; fi\n", &[("# c", Role::Comment), ("if", Role::Keyword), ("\"x\"", Role::String)]),
            ("a.css", "/* c */\na { color: red; margin: 4px; }\n", &[("/* c */", Role::Comment), ("a", Role::Tag), ("4px", Role::Number)]),
            ("a.html", "<!-- c -->\n<p class=\"a\">x</p>\n", &[("<!-- c -->", Role::Comment), ("p", Role::Tag), ("a", Role::String)]),
            ("a.toml", "# c\na = \"x\"\nn = 1\n", &[("# c", Role::Comment), ("\"x\"", Role::String), ("1", Role::Number)]),
            ("a.yaml", "# c\na: \"x\"\nn: 1\n", &[("# c", Role::Comment), ("\"x\"", Role::String), ("1", Role::Number)]),
            ("a.py", "# c\ndef f():\n    return \"x\" + 1\n", &[("# c", Role::Comment), ("def", Role::Keyword), ("\"x\"", Role::String), ("1", Role::Number)]),
            (
                "a.js",
                "// c\nfunction f() { return \"x\" + 1 }\n",
                &[("// c", Role::Comment), ("function", Role::Keyword), ("\"x\"", Role::String), ("1", Role::Number)],
            ),
        ];
        for (path, src, want) in cases {
            let r = roles_of(path, src);
            for w in *want {
                assert!(r.iter().any(|(t, role)| t == w.0 && *role == w.1), "{path}: expected {w:?}\n{r:?}");
            }
        }
    }

    /// A small snippet per new language: a comment, a string and a keyword must all get colored.
    #[test]
    #[allow(clippy::type_complexity)]
    fn added_languages_highlight_comment_string_keyword() {
        let cases: &[(&str, &str, &[(&str, Role)])] = &[
            ("a.c", "// c\nint main() { return 0; } char *s = \"x\";\n", &[("// c", Role::Comment), ("return", Role::Keyword), ("\"x\"", Role::String)]),
            (
                "a.cpp",
                "// c\nclass A { public: int f() { return 1; } }; auto s = \"x\";\n",
                &[("// c", Role::Comment), ("class", Role::Keyword), ("\"x\"", Role::String)],
            ),
            (
                "A.java",
                "// c\nclass A { String s = \"x\"; int f() { return 1; } }\n",
                &[("// c", Role::Comment), ("class", Role::Keyword), ("\"x\"", Role::String)],
            ),
            (
                "a.cs",
                "// c\nclass A { string s = \"x\"; int F() { return 1; } }\n",
                &[("// c", Role::Comment), ("class", Role::Keyword), ("\"x\"", Role::String)],
            ),
            ("a.rb", "# c\ndef f\n  puts \"x\"\nend\n", &[("# c", Role::Comment), ("def", Role::Keyword), ("\"x\"", Role::String)]),
            ("a.php", "<?php\n// c\nfunction f() { return \"x\"; }\n", &[("// c", Role::Comment), ("function", Role::Keyword), ("\"x\"", Role::String)]),
            ("a.swift", "// c\nfunc f() -> String { return \"x\" }\n", &[("// c", Role::Comment), ("func", Role::Keyword), ("x", Role::String)]),
            ("a.kt", "// c\nfun f(): String { return \"x\" }\n", &[("// c", Role::Comment), ("fun", Role::Keyword), ("x", Role::String)]),
            ("a.sql", "-- c\nSELECT a FROM t WHERE b = 'x';\n", &[("-- c", Role::Comment), ("SELECT", Role::Keyword), ("'x'", Role::String)]),
            ("a.lua", "-- c\nlocal function f() return \"x\" end\n", &[("-- c", Role::Comment), ("function", Role::Keyword), ("\"x\"", Role::String)]),
            ("Dockerfile", "# c\nFROM alpine\nENV A=\"x\"\n", &[("# c", Role::Comment), ("FROM", Role::Keyword), ("\"x\"", Role::String)]),
            ("Makefile", "# c\nall: dep\n\techo \"x\"\nifeq ($(A),b)\nendif\n", &[("# c", Role::Comment), ("ifeq", Role::Keyword)]),
        ];
        for (path, src, want) in cases {
            let r = roles_of(path, src);
            for w in *want {
                assert!(r.iter().any(|(t, role)| t == w.0 && *role == w.1), "{path}: expected {w:?}\n{r:?}");
            }
        }
    }

    #[test]
    fn kotlin_highlights() {
        let src = r#"package app.demo

import kotlin.collections.List

/** Doc comment. */
@Serializable
data class User(val name: String, var age: Int = 3)

// line comment
interface Greeter { fun greet(user: User): String }

enum class Mode { FAST, SLOW }

fun main() {
    val user = User("Ada", 36)
    val note: String? = null
    val ok = true && user.age >= 18
    println("Hi ${user.name}!\n")
    val c = 'x'
    val f = listOf(1, 2).map { it * 2.5 }
    val kind = when (mode) {
        Mode.FAST -> "fast"
        else -> "slow"
    }
    if (user is User && ok) return
    outer@ for (i in 1..3) { break@outer }
}
"#;
        let r = roles_of("x.kt", src);
        let has = |t: &str, role: Role| r.iter().any(|(x, y)| x == t && *y == role);
        for (t, role) in [
            ("/** Doc comment. */", Role::Comment),
            ("// line comment", Role::Comment),
            ("package", Role::Keyword),
            ("import", Role::Keyword),
            ("data", Role::Keyword),
            ("class", Role::Keyword),
            ("interface", Role::Keyword),
            ("enum", Role::Keyword),
            ("val", Role::Keyword),
            ("fun", Role::Keyword),
            ("when", Role::Keyword),
            ("else", Role::Keyword),
            ("if", Role::Keyword),
            ("for", Role::Keyword),
            ("return", Role::Keyword),
            ("is", Role::Keyword),
            ("in", Role::Keyword),
            ("@Serializable", Role::Tag),
            ("outer@", Role::Tag),
            ("User", Role::Type),
            ("Greeter", Role::Type),
            ("String", Role::Type),
            ("FAST", Role::Number),
            ("main", Role::Function),
            ("greet", Role::Function),
            ("println", Role::Function),
            ("listOf", Role::Function),
            ("map", Role::Function),
            ("fast", Role::String),
            ("Hi ", Role::String),
            ("'x'", Role::String),
            ("\\n", Role::String),
            ("null", Role::Number),
            ("true", Role::Number),
            ("36", Role::Number),
            ("2.5", Role::Number),
            ("->", Role::Punct),
            ("&&", Role::Punct),
            ("${", Role::Punct),
        ] {
            assert!(has(t, role), "{t:?} should be {role:?}\n{r:?}");
        }
    }

    #[test]
    fn file_names_pick_the_language() {
        for (path, name) in [
            ("/p/Makefile", "Makefile"),
            ("/p/GNUmakefile", "Makefile"),
            ("/p/x.mk", "Makefile"),
            ("/p/Dockerfile", "Dockerfile"),
            ("/p/Dockerfile.prod", "Dockerfile"),
            ("/p/Containerfile", "Dockerfile"),
            ("/p/Gemfile", "Ruby"),
            ("/p/x.h", "C"),
            ("/p/x.hpp", "C++"),
            ("/p/x.kts", "Kotlin"),
        ] {
            assert_eq!(language_for_path(path).map(|l| l.name), Some(name), "{path}");
        }
    }

    #[test]
    fn markdown_headings_code_and_links() {
        let r = roles_of("x.md", "# Başlık\n\n- madde\n\n```rust\nfn x() {}\n```\n\n[ad](https://ornek.dev)\n");
        assert!(r.contains(&("Başlık".into(), Role::Keyword)), "{r:?}");
        assert!(r.iter().any(|(t, role)| t.contains("fn x()") && *role == Role::String), "{r:?}");
        assert!(r.contains(&("-".into(), Role::Punct)) || r.iter().any(|(t, role)| t.starts_with('-') && *role == Role::Punct), "{r:?}");
    }

    #[test]
    fn rust_highlights() {
        let r = roles_of("x.rs", "// not\nfn main() { let s = \"ğ\"; 42 }\n");
        assert!(r.contains(&("// not".into(), Role::Comment)), "{r:?}");
        assert!(r.contains(&("fn".into(), Role::Keyword)));
        assert!(r.contains(&("main".into(), Role::Function)));
        assert!(r.iter().any(|(t, role)| t.contains('ğ') && *role == Role::String));
        assert!(r.contains(&("42".into(), Role::Number)) || r.iter().any(|(t, _)| t == "42"));
    }

    #[test]
    fn typescript_gets_js_rules_too() {
        let r = roles_of("x.ts", "const x: number = 1; // c\n");
        assert!(r.contains(&("const".into(), Role::Keyword)), "{r:?}");
        assert!(r.contains(&("// c".into(), Role::Comment)));
    }

    #[test]
    fn incremental_edit_matches_full_parse() {
        let mut rope = Rope::from_str("fn a() {}\n");
        let mut s = Syntax::new(language_for_path("x.rs").unwrap(), &rope).unwrap();
        // "fn a() {}" → "fn ab() {}" : bayt 4'e "b"
        rope.insert(4, "b");
        s.edit(&Edit { start_byte: 4, old_end_byte: 4, new_end_byte: 5, start: (0, 4), old_end: (0, 4), new_end: (0, 5) });
        s.reparse(&rope);
        let spans = s.highlights(&rope, 0..rope.len_bytes());
        assert!(spans.iter().any(|(r, role)| rope.byte_slice(r.clone()) == "ab" && *role == Role::Function), "{spans:?}");
    }

    #[test]
    fn unload_and_reload() {
        let rope = Rope::from_str("fn a() {}\n");
        let mut s = Syntax::new(language_for_path("x.rs").unwrap(), &rope).unwrap();
        s.unload();
        assert!(!s.is_loaded());
        assert!(s.highlights(&rope, 0..rope.len_bytes()).is_empty(), "empty when dropped");
        s.ensure_loaded(&rope);
        assert!(!s.highlights(&rope, 0..rope.len_bytes()).is_empty());
    }

    #[test]
    fn shared_parser_switches_languages() {
        let a = Rope::from_str("fn a() {}\n");
        let b = Rope::from_str("def b():\n    pass\n");
        let sa = Syntax::new(language_for_path("x.rs").unwrap(), &a).unwrap();
        let sb = Syntax::new(language_for_path("x.py").unwrap(), &b).unwrap();
        assert!(sa.highlights(&a, 0..a.len_bytes()).iter().any(|(_, r)| *r == Role::Keyword));
        assert!(sb.highlights(&b, 0..b.len_bytes()).iter().any(|(_, r)| *r == Role::Keyword));
    }

    #[test]
    fn line_runs_cover_line_and_inner_wins() {
        // "a \"x\\n\" b": string 2..8, escape 4..6 (narrow, applied after)
        let spans = vec![(2..8, Role::String), (4..6, Role::Number)];
        let runs = line_runs(0, 10, &spans);
        assert_eq!(runs, vec![(2, None), (2, Some(Role::String)), (2, Some(Role::Number)), (2, Some(Role::String)), (2, None)]);
        assert_eq!(runs.iter().map(|r| r.0).sum::<usize>(), 10);
    }
}
