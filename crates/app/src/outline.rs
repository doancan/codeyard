//! Document outlines (OUTLINE): functions, types, modules and headings from line patterns,
//! no language server needed. Works over SSH and for languages without a server.

use regex::Regex;
use std::sync::OnceLock;

#[path = "outline_model.rs"]
pub mod model;

/// What a symbol is. The order is the one the Outline groups by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Module,
    Class,
    Type,
    Interface,
    Enum,
    Function,
    Method,
    Property,
    Field,
    Variable,
    Constant,
    Heading,
    Section,
    Other,
}

impl Kind {
    /// The mark drawn in the icon.
    pub fn glyph(self) -> &'static str {
        match self {
            Kind::Module => "▪",
            Kind::Class => "C",
            Kind::Type => "◇",
            Kind::Interface => "I",
            Kind::Enum => "E",
            Kind::Function | Kind::Method => "ƒ",
            Kind::Property => "p",
            Kind::Field => "f",
            Kind::Variable => "v",
            Kind::Constant => "K",
            Kind::Heading => "#",
            Kind::Section => "§",
            Kind::Other => "•",
        }
    }

    /// The kind in words (for the accessibility tree and the sibling picker).
    pub fn label(self) -> &'static str {
        match self {
            Kind::Module => crate::i18n::t("module"),
            Kind::Class => crate::i18n::t("class"),
            Kind::Type => crate::i18n::t("type"),
            Kind::Interface => crate::i18n::t("interface"),
            Kind::Enum => crate::i18n::t("enum"),
            Kind::Function => crate::i18n::t("function"),
            Kind::Method => crate::i18n::t("method"),
            Kind::Property => crate::i18n::t("property"),
            Kind::Field => crate::i18n::t("field"),
            Kind::Variable => crate::i18n::t("variable"),
            Kind::Constant => crate::i18n::t("constant"),
            Kind::Heading => crate::i18n::t("heading"),
            Kind::Section => crate::i18n::t("section"),
            Kind::Other => crate::i18n::t("symbol"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: Kind,
    /// 0-based line.
    pub line: usize,
    /// Column of the name (UTF-16, as a language server counts); 0 when the symbol comes from a line pattern.
    pub col: usize,
    /// Last line of the symbol's range.
    pub end_line: usize,
    /// Nesting (from indentation or heading level).
    pub depth: usize,
}

struct Rule {
    re: Regex,
    kind: Kind,
    /// Capture group for the name.
    name: usize,
}

fn rule(re: &str, kind: Kind, name: usize) -> Rule {
    Rule { re: Regex::new(re).expect("anahat deseni"), kind, name }
}

fn rules_for(ext: &str) -> Option<&'static [Rule]> {
    static RUST: OnceLock<Vec<Rule>> = OnceLock::new();
    static PY: OnceLock<Vec<Rule>> = OnceLock::new();
    static JS: OnceLock<Vec<Rule>> = OnceLock::new();
    static GO: OnceLock<Vec<Rule>> = OnceLock::new();
    static CLIKE: OnceLock<Vec<Rule>> = OnceLock::new();
    static RUBY: OnceLock<Vec<Rule>> = OnceLock::new();
    static MD: OnceLock<Vec<Rule>> = OnceLock::new();
    static SH: OnceLock<Vec<Rule>> = OnceLock::new();
    static TOML: OnceLock<Vec<Rule>> = OnceLock::new();
    static LUA: OnceLock<Vec<Rule>> = OnceLock::new();
    Some(match ext {
        "rs" => RUST.get_or_init(|| {
            vec![
                rule(r"^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?(?:async[ \t]+|unsafe[ \t]+|const[ \t]+|extern[ \t]+(?:\x22[^\x22]*\x22[ \t]+)?)*fn[ \t]+([A-Za-z_][A-Za-z0-9_]*)", Kind::Function, 1),
                rule(r"^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?(?:struct|enum|trait|union|type)[ \t]+([A-Za-z_][A-Za-z0-9_]*)", Kind::Type, 1),
                rule(r"^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?impl(?:<[^>]*>)?[ \t]+([^{]+?)[ \t]*(?:where[ \t].*)?\{?[ \t]*$", Kind::Type, 1),
                rule(r"^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?mod[ \t]+([A-Za-z_][A-Za-z0-9_]*)[ \t]*\{", Kind::Module, 1),
            ]
        }),
        "py" | "pyi" => PY.get_or_init(|| vec![rule(r"^[ \t]*(?:async[ \t]+)?def[ \t]+([A-Za-z_][A-Za-z0-9_]*)", Kind::Function, 1), rule(r"^[ \t]*class[ \t]+([A-Za-z_][A-Za-z0-9_]*)", Kind::Type, 1)]),
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" => JS.get_or_init(|| {
            vec![
                rule(r"^[ \t]*(?:export[ \t]+)?(?:default[ \t]+)?(?:async[ \t]+)?function[ \t]*\*?[ \t]*([A-Za-z_$][[A-Za-z0-9_]$]*)", Kind::Function, 1),
                rule(r"^[ \t]*(?:export[ \t]+)?(?:default[ \t]+)?(?:abstract[ \t]+)?(?:class|interface|enum)[ \t]+([A-Za-z_$][[A-Za-z0-9_]$]*)", Kind::Type, 1),
                rule(r"^[ \t]*(?:export[ \t]+)?type[ \t]+([A-Za-z_$][[A-Za-z0-9_]$]*)[ \t]*(?:<[^>]*>)?[ \t]*=", Kind::Type, 1),
                rule(r"^[ \t]*(?:export[ \t]+)?(?:const|let|var)[ \t]+([A-Za-z_$][[A-Za-z0-9_]$]*)[ \t]*(?::[^=]+)?=[ \t]*(?:async[ \t]*)?(?:\([^)]*\)|[A-Za-z_$][[A-Za-z0-9_]$]*)[ \t]*(?::[^=]+)?=>", Kind::Function, 1),
                rule(r"^[ \t]+(?:public[ \t]+|private[ \t]+|protected[ \t]+|static[ \t]+|async[ \t]+|get[ \t]+|set[ \t]+)*([A-Za-z_$][[A-Za-z0-9_]$]*)[ \t]*\([^)]*\)[ \t]*(?::[^{]+)?\{[ \t]*$", Kind::Function, 1),
            ]
        }),
        "go" => GO.get_or_init(|| {
            vec![
                rule(r"^func[ \t]+(?:\([^)]*\)[ \t]*)?([A-Za-z_][A-Za-z0-9_]*)", Kind::Function, 1),
                rule(r"^type[ \t]+([A-Za-z_][A-Za-z0-9_]*)[ \t]+(?:struct|interface)", Kind::Type, 1),
            ]
        }),
        "java" | "kt" | "kts" | "swift" | "cs" | "scala" | "dart" | "c" | "h" | "cc" | "cpp" | "hpp" | "cxx" | "zig" | "php" => CLIKE.get_or_init(|| {
            vec![
                rule(r"^[ \t]*(?:(?:public|private|protected|internal|open|abstract|final|sealed|static|data|partial)[ \t]+)*(?:class|interface|enum|struct|object|trait|protocol|extension|record)[ \t]+([A-Za-z_][A-Za-z0-9_]*)", Kind::Type, 1),
                rule(r"^[ \t]*(?:(?:public|private|protected|internal|open|override|static|final|abstract|suspend|inline|async)[ \t]+)*(?:fun|func|fn|function)[ \t]+(?:<[^>]*>[ \t]*)?([A-Za-z_][A-Za-z0-9_]*)", Kind::Function, 1),
                rule(r"^[A-Za-z_][[A-Za-z0-9_]:<>,[ \t]\*&]*?[ \t]+\**([A-Za-z_][A-Za-z0-9_]*)[ \t]*\([^;{]*\)[ \t]*(?:const[ \t]*)?\{?[ \t]*$", Kind::Function, 1),
            ]
        }),
        "rb" => RUBY.get_or_init(|| vec![rule(r"^[ \t]*def[ \t]+(?:self\.)?([A-Za-z_][A-Za-z0-9_]*[?!=]?)", Kind::Function, 1), rule(r"^[ \t]*(?:class|module)[ \t]+([A-Z][[A-Za-z0-9_]:]*)", Kind::Type, 1)]),
        "md" | "markdown" | "mdx" => MD.get_or_init(|| vec![rule(r"^(#{1,6})[ \t]+(.+?)[ \t]*#*[ \t]*$", Kind::Heading, 2)]),
        "sh" | "bash" | "zsh" => SH.get_or_init(|| vec![rule(r"^[ \t]*(?:function[ \t]+)?([A-Za-z_][[A-Za-z0-9_]-]*)[ \t]*\(\)[ \t]*\{?", Kind::Function, 1), rule(r"^[ \t]*function[ \t]+([A-Za-z_][[A-Za-z0-9_]-]*)", Kind::Function, 1)]),
        "toml" => TOML.get_or_init(|| vec![rule(r"^\[\[?([^\]]+)\]\]?[ \t]*$", Kind::Section, 1)]),
        "lua" => LUA.get_or_init(|| vec![rule(r"^[ \t]*(?:local[ \t]+)?function[ \t]+([A-Za-z_][[A-Za-z0-9_].:]*)", Kind::Function, 1)]),
        _ => return None,
    })
}

/// A document's outline. Unsupported extension: `None`.
pub fn extract(path: &str, lines: &[String]) -> Option<Vec<Symbol>> {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())?;
    let rules = rules_for(&ext)?;
    let is_md = matches!(ext.as_str(), "md" | "markdown" | "mdx");
    let mut out = Vec::new();
    let mut in_fence = false;
    for (i, l) in lines.iter().enumerate() {
        if is_md && l.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || l.len() > 400 {
            continue;
        }
        for r in rules {
            let Some(c) = r.re.captures(l) else { continue };
            let name = c.get(r.name).map_or("", |m| m.as_str()).trim().to_string();
            if name.is_empty() || matches!(name.as_str(), "if" | "for" | "while" | "switch" | "return" | "else" | "catch" | "match") {
                continue;
            }
            let depth = if is_md {
                c.get(1).map_or(1, |m| m.as_str().len()).saturating_sub(1)
            } else {
                let indent = l.chars().take_while(|c| *c == ' ' || *c == '\t').fold(0, |n, c| n + if c == '\t' { 4 } else { 1 });
                (indent / 4).min(6)
            };
            out.push(Symbol { name, kind: r.kind, line: i, col: 0, end_line: i, depth });
            break;
        }
    }
    model::fill_ends(&mut out, lines.len());
    Some(out)
}

/// Symbol the cursor is in: the innermost one whose range holds the cursor's line, else the last one starting before it
/// (so the Outline still marks where you are between two functions).
pub fn current(symbols: &[Symbol], cursor_line: usize) -> Option<usize> {
    model::path_at(symbols, cursor_line).last().copied().or_else(|| symbols.iter().rposition(|s| s.line <= cursor_line))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(str::to_string).collect()
    }

    fn names(path: &str, src: &str) -> Vec<(String, usize, usize)> {
        extract(path, &lines(src)).unwrap().into_iter().map(|s| (s.name, s.line, s.depth)).collect()
    }

    #[test]
    fn rust_items_are_found_with_nesting() {
        let src = "use x;\npub struct Foo {\n    a: u8,\n}\n\nimpl Foo {\n    pub async fn bar(&self) {}\n    fn baz() {}\n}\n\npub(crate) mod m {\n    pub fn inner() {}\n}\nimpl<T> Trait for Vec<T> where T: Copy {\n}\n";
        let n = names("a.rs", src);
        assert_eq!(n[0], ("Foo".into(), 1, 0));
        assert_eq!(n[1], ("Foo".into(), 5, 0));
        assert_eq!(n[2], ("bar".into(), 6, 1));
        assert_eq!(n[3], ("baz".into(), 7, 1));
        assert_eq!(n[4], ("m".into(), 10, 0));
        assert_eq!(n[5], ("inner".into(), 11, 1));
        assert_eq!(n[6].0, "Trait for Vec<T>");
    }

    #[test]
    fn python_js_go_and_markdown() {
        assert_eq!(
            names("a.py", "class A:\n    def f(self):\n        pass\nasync def g(): pass\n"),
            [("A".into(), 0, 0), ("f".into(), 1, 1), ("g".into(), 3, 0)]
        );
        let js = names("a.ts", "export function one() {}\nexport const two = async (x) => {\n}\nclass C {\n  method(a) {\n  }\n}\ninterface I {}\n");
        assert_eq!(js.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["one", "two", "C", "method", "I"]);
        assert_eq!(
            names("a.go", "func (s *S) Run() {}\ntype T struct {\n}\nfunc main() {}\n").iter().map(|x| x.0.as_str()).collect::<Vec<_>>(),
            ["Run", "T", "main"]
        );
        let md = names("a.md", "# Top\n\ntext\n## Sub\n```\n# not a heading\n```\n### Deep\n");
        assert_eq!(md, [("Top".into(), 0, 0), ("Sub".into(), 3, 1), ("Deep".into(), 7, 2)]);
    }

    #[test]
    fn control_keywords_are_not_functions() {
        let c = names("a.c", "int main(void) {\n    if (x) {\n    }\n    while (y) {\n    }\n}\n");
        assert_eq!(c.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["main"]);
    }

    #[test]
    fn unknown_extensions_have_no_outline_and_current_symbol_follows_the_cursor() {
        assert!(extract("a.bin", &lines("x")).is_none());
        let syms = extract("a.rs", &lines("fn a() {}\n\nfn b() {}\n")).unwrap();
        assert_eq!(current(&syms, 0), Some(0));
        assert_eq!(current(&syms, 1), Some(0));
        assert_eq!(current(&syms, 2), Some(1));
        assert_eq!(current(&[], 3), None);
    }
}

#[cfg(test)]
mod hardening {
    use super::*;

    #[test]
    fn random_text_never_panics_or_hangs() {
        let alphabet = [
            "fn ",
            "def ",
            "class ",
            "function ",
            "func ",
            "#",
            "## ",
            "(",
            ")",
            "{",
            "}",
            "<",
            ">",
            "=>",
            " ",
            "\t",
            "a",
            "ö",
            "::",
            "pub ",
            "impl ",
            "\n",
            "async ",
            "export ",
            "const ",
            "=",
        ];
        let mut x = 12345u64;
        for round in 0..300 {
            let lines: Vec<String> = (0..20)
                .map(|_| {
                    (0..(round % 30 + 1))
                        .map(|_| {
                            x ^= x << 13;
                            x ^= x >> 7;
                            x ^= x << 17;
                            alphabet[(x % alphabet.len() as u64) as usize]
                        })
                        .collect()
                })
                .collect();
            for ext in ["rs", "py", "ts", "go", "java", "md", "sh", "toml", "rb", "lua", "c"] {
                let _ = extract(&format!("f.{ext}"), &lines);
            }
        }
    }

    #[test]
    fn very_long_lines_are_skipped_quickly() {
        let long = format!("fn {}", "a".repeat(100_000));
        let started = std::time::Instant::now();
        assert!(extract("a.rs", &[long]).unwrap().is_empty());
        assert!(started.elapsed().as_millis() < 500 * u128::from(crate::test_sshd::timeout_scale()));
    }
}
