//! Snippets: a few built-in language sets plus the user's
//! `<settings folder>/snippets/<lang>.json` files (VS Code format). A prefix plus Tab
//! expands it; bodies support `$1`, `${2:name}`, `$0` tab stops (the editor already
//! handles stop navigation).

#[cfg(not(test))]
use std::sync::RwLock;

use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snippet {
    /// Language id ("rust", "python"…); empty: every file.
    pub lang: String,
    pub prefix: String,
    pub body: String,
    pub description: String,
    pub user: bool,
    /// The extension that contributed it (`publisher.name`); `None` for built-in and the user's own.
    pub origin: Option<String>,
}

#[cfg(not(test))]
static USER: RwLock<Vec<Snippet>> = RwLock::new(Vec::new());

// Tests run in parallel, each on its own thread: keep the list thread-local, or another
// test's settings reload would wipe it.
#[cfg(test)]
thread_local! {
    static USER_TL: std::cell::RefCell<Vec<Snippet>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Tests mutate the global snippet list, so they must not overlap.
#[cfg(test)]
pub static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn set_user(list: Vec<Snippet>) {
    #[cfg(not(test))]
    {
        *USER.write().unwrap_or_else(|e| e.into_inner()) = list;
    }
    #[cfg(test)]
    USER_TL.with(|u| *u.borrow_mut() = list);
}

fn user_snippets() -> Vec<Snippet> {
    #[cfg(not(test))]
    {
        USER.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
    #[cfg(test)]
    USER_TL.with(|u| u.borrow().clone())
}

/// Language id from a file path (by extension; using VS Code's language names).
pub fn lang_of_path(path: &str) -> &'static str {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "rs" => "rust",
        "py" | "pyi" => "python",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "ts" | "mts" | "cts" | "tsx" => "typescript",
        "go" => "go",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" => "cpp",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "swift" => "swift",
        "rb" => "ruby",
        "php" => "php",
        "lua" => "lua",
        "sh" | "bash" | "zsh" => "shellscript",
        "html" | "htm" | "vue" => "html",
        "css" | "scss" | "less" => "css",
        "json" | "jsonc" => "json",
        "md" | "markdown" => "markdown",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "sql" => "sql",
        // A language an installed extension adds for files no built-in language claims.
        _ => crate::ext::language_for(path).map_or("plaintext", |l| l.id),
    }
}

fn b(lang: &str, prefix: &str, body: &str, description: &str) -> Snippet {
    Snippet { lang: lang.into(), prefix: prefix.into(), body: body.into(), description: description.into(), user: false, origin: None }
}

/// Built-in snippets.
pub fn builtin() -> Vec<Snippet> {
    vec![
        b("rust", "fn", "fn ${1:name}(${2}) ${3:-> ${4:()} }{\n\t$0\n}", "function"),
        b("rust", "pfn", "pub fn ${1:name}(${2}) ${3:-> ${4:()} }{\n\t$0\n}", "public function"),
        b("rust", "test", "#[test]\nfn ${1:name}() {\n\t$0\n}", "unit test"),
        b("rust", "impl", "impl ${1:Type} {\n\t$0\n}", "impl block"),
        b("rust", "struct", "struct ${1:Name} {\n\t${2:field}: ${3:Type},\n}", "struct"),
        b("rust", "enum", "enum ${1:Name} {\n\t${2:Variant},\n}", "enum"),
        b("rust", "match", "match ${1:value} {\n\t${2:pattern} => ${3:expr},\n\t_ => $0,\n}", "match expression"),
        b("rust", "for", "for ${1:item} in ${2:iter} {\n\t$0\n}", "for loop"),
        b("rust", "derive", "#[derive(${1:Debug, Clone})]", "derive attribute"),
        b("rust", "mod", "#[cfg(test)]\nmod tests {\n\tuse super::*;\n\n\t#[test]\n\tfn ${1:name}() {\n\t\t$0\n\t}\n}", "test module"),
        b("python", "def", "def ${1:name}(${2}):\n\t${3:pass}", "function"),
        b("python", "class", "class ${1:Name}:\n\tdef __init__(self${2}):\n\t\t${3:pass}", "class"),
        b("python", "ifmain", "if __name__ == \"__main__\":\n\t${1:main()}", "main guard"),
        b("python", "for", "for ${1:item} in ${2:items}:\n\t${3:pass}", "for loop"),
        b("python", "with", "with ${1:open(path)} as ${2:f}:\n\t${3:pass}", "with statement"),
        b("python", "try", "try:\n\t${1:pass}\nexcept ${2:Exception} as ${3:e}:\n\t${4:raise}", "try/except"),
        b("javascript", "fn", "function ${1:name}(${2}) {\n\t$0\n}", "function"),
        b("javascript", "af", "const ${1:name} = (${2}) => {\n\t$0\n};", "arrow function"),
        b("javascript", "log", "console.log(${1});", "console.log"),
        b("javascript", "imp", "import ${1:name} from '${2:module}';", "import"),
        b("javascript", "for", "for (const ${1:item} of ${2:items}) {\n\t$0\n}", "for…of"),
        b("javascript", "try", "try {\n\t$1\n} catch (${2:err}) {\n\t$0\n}", "try/catch"),
        b("typescript", "fn", "function ${1:name}(${2}): ${3:void} {\n\t$0\n}", "function"),
        b("typescript", "af", "const ${1:name} = (${2}): ${3:void} => {\n\t$0\n};", "arrow function"),
        b("typescript", "log", "console.log(${1});", "console.log"),
        b("typescript", "imp", "import { ${1} } from '${2:module}';", "import"),
        b("typescript", "interface", "interface ${1:Name} {\n\t${2:field}: ${3:string};\n}", "interface"),
        b("typescript", "type", "type ${1:Name} = ${2};", "type alias"),
        b("go", "func", "func ${1:name}(${2}) ${3}{\n\t$0\n}", "function"),
        b("go", "iferr", "if err != nil {\n\t${1:return err}\n}", "error check"),
        b("go", "for", "for ${1:i} := 0; $1 < ${2:n}; $1++ {\n\t$0\n}", "for loop"),
        b("go", "struct", "type ${1:Name} struct {\n\t${2:Field} ${3:string}\n}", "struct"),
        b("go", "main", "package main\n\nfunc main() {\n\t$0\n}", "main package"),
        b("c", "main", "int main(int argc, char **argv) {\n\t$0\n\treturn 0;\n}", "main"),
        b("c", "for", "for (int ${1:i} = 0; $1 < ${2:n}; $1++) {\n\t$0\n}", "for loop"),
        b("c", "inc", "#include <${1:stdio.h}>", "include"),
        b("java", "main", "public static void main(String[] args) {\n\t$0\n}", "main"),
        b("java", "sout", "System.out.println(${1});", "println"),
        b(
            "html",
            "html5",
            "<!DOCTYPE html>\n<html lang=\"${1:en}\">\n<head>\n\t<meta charset=\"UTF-8\">\n\t<meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\">\n\t<title>${2:Document}</title>\n</head>\n<body>\n\t$0\n</body>\n</html>",
            "HTML5 skeleton",
        ),
        b("html", "link", "<link rel=\"stylesheet\" href=\"${1:style.css}\">", "stylesheet link"),
        b("html", "script", "<script src=\"${1:main.js}\"></script>", "script tag"),
        b("shellscript", "if", "if [ ${1:condition} ]; then\n\t$0\nfi", "if"),
        b("shellscript", "for", "for ${1:i} in ${2:list}; do\n\t$0\ndone", "for loop"),
        b("shellscript", "bash", "#!/usr/bin/env bash\nset -euo pipefail\n\n$0", "bash header"),
        b("markdown", "link", "[${1:text}](${2:url})", "link"),
        b("markdown", "code", "```${1:lang}\n$0\n```", "code block"),
        b("markdown", "table", "| ${1:Column} | ${2:Column} |\n|---|---|\n| $0 | |", "table"),
        b("sql", "sel", "SELECT ${1:*}\nFROM ${2:table}\nWHERE ${3:condition};", "select"),
        b("", "todo", "TODO: $0", "TODO comment text"),
        b("", "lorem", "Lorem ipsum dolor sit amet, consectetur adipiscing elit.", "placeholder text"),
    ]
}

/// Snippets for this file: user's first, then built-in.
pub fn for_path(path: &str) -> Vec<Snippet> {
    let lang = lang_of_path(path);
    let fits = |s: &Snippet| s.lang.is_empty() || s.lang == lang;
    let mut out: Vec<Snippet> = user_snippets().into_iter().filter(|s| fits(s)).collect();
    out.extend(builtin().into_iter().filter(|s| fits(s)));
    out
}

/// Exact prefix match; a user snippet beats a built-in one.
pub fn find(path: &str, prefix: &str) -> Option<Snippet> {
    for_path(path).into_iter().find(|s| s.prefix == prefix)
}

/// VS Code format: { "Name": { "prefix": "x" | ["x","y"], "body": "…" | ["line", …], "description": "…" } }.
/// `lang`: language from the file name ("global": every file). Malformed entries are skipped with a warning.
pub fn parse_user_file(lang: &str, text: &str) -> (Vec<Snippet>, Vec<String>) {
    parse_file(lang, text, None, &format!("snippets/{lang}.json"))
}

/// A snippet file in VS Code format; `origin` is the extension that ships it, `label` names the file in warnings.
pub fn parse_file(lang: &str, text: &str, origin: Option<&str>, label: &str) -> (Vec<Snippet>, Vec<String>) {
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    let value: Value = match serde_json::from_str(&theme::themes::strip_jsonc(text)) {
        Ok(v) => v,
        Err(e) => return (out, vec![format!("{label}: {e}")]),
    };
    let Some(map) = value.as_object() else { return (out, vec![format!("{label}: expected an object")]) };
    let lang_id = if lang == "global" { String::new() } else { lang.to_string() };
    for (name, def) in map {
        let prefixes: Vec<String> = match def.get("prefix") {
            Some(Value::String(p)) => vec![p.clone()],
            Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
            _ => Vec::new(),
        };
        let body = match def.get("body") {
            Some(Value::String(b)) => Some(b.clone()),
            Some(Value::Array(a)) => Some(a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join("\n")),
            _ => None,
        };
        let (Some(body), false) = (body, prefixes.is_empty()) else {
            warnings.push(format!("{label}: \"{name}\" needs a \"prefix\" and a \"body\""));
            continue;
        };
        let description = def.get("description").and_then(Value::as_str).unwrap_or(name).to_string();
        for prefix in prefixes {
            out.push(Snippet {
                lang: lang_id.clone(),
                prefix,
                body: body.clone(),
                description: description.clone(),
                user: true,
                origin: origin.map(str::to_string),
            });
        }
    }
    (out, warnings)
}

/// Template for a new snippet file.
pub fn template(lang: &str) -> String {
    format!(
        "// {lang} snippets. Type the prefix and press Tab.\n// $1, ${{2:default}} are tab stops; $0 is where the cursor ends.\n{{\n  // \"Print to console\": {{\n  //   \"prefix\": \"log\",\n  //   \"body\": [\"console.log($1);\", \"$0\"],\n  //   \"description\": \"Log output to the console\"\n  // }}\n}}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_follow_the_extension() {
        assert_eq!(lang_of_path("/a/b.rs"), "rust");
        assert_eq!(lang_of_path("x.TSX"), "typescript");
        assert_eq!(lang_of_path("Makefile"), "plaintext");
    }

    #[test]
    fn builtins_are_found_per_language_and_user_ones_win() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let rust = for_path("a.rs");
        assert!(rust.iter().any(|s| s.prefix == "fn") && rust.iter().any(|s| s.prefix == "todo"));
        assert!(!rust.iter().any(|s| s.prefix == "def"));
        set_user(vec![Snippet { lang: "rust".into(), prefix: "fn".into(), body: "mine".into(), description: String::new(), user: true, origin: None }]);
        assert_eq!(find("a.rs", "fn").unwrap().body, "mine");
        set_user(Vec::new());
        assert!(find("a.rs", "fn").unwrap().body.starts_with("fn ${1:name}"));
        assert!(find("a.rs", "zzz").is_none());
    }

    #[test]
    fn every_builtin_body_parses_into_stops() {
        for s in builtin() {
            let (text, _stops) = crate::lsp::parse_snippet(&s.body);
            assert!(!text.contains('$'), "{} {}: {text}", s.lang, s.prefix);
        }
    }

    #[test]
    fn user_files_use_the_vscode_format() {
        let src = r#"{
          // yorum
          "Log": { "prefix": ["log", "cl"], "body": ["console.log($1);", "$0"], "description": "yaz" },
          "Bozuk": { "prefix": "x" },
        }"#;
        let (s, w) = parse_user_file("javascript", src);
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].prefix.as_str(), s[0].body.as_str(), s[0].lang.as_str()), ("log", "console.log($1);\n$0", "javascript"));
        assert_eq!(s[1].prefix, "cl");
        assert_eq!(w.len(), 1, "{w:?}");
        let (g, _) = parse_user_file("global", r#"{ "T": { "prefix": "t", "body": "x" } }"#);
        assert_eq!(g[0].lang, "");
        assert!(parse_user_file("go", "{ bozuk").1.len() == 1);
    }
}

#[cfg(test)]
mod hardening {
    use super::*;

    #[test]
    fn random_snippet_bodies_never_panic() {
        let alphabet = ["$", "${", "}", "$1", "${1:", "${2|a,b|}", ":", "|", "\\", "\n", "\t", " ", "x", "ç", "$0", "$$", "${", "1", "}}"];
        let mut x = 5u64;
        for _ in 0..3000 {
            let n = 1 + (x % 25) as usize;
            let s: String = (0..n)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    alphabet[(x % alphabet.len() as u64) as usize]
                })
                .collect();
            let (text, stops) = crate::lsp::parse_snippet(&s);
            let len = text.chars().count();
            assert!(stops.iter().all(|(_, r)| r.start <= r.end && r.end <= len), "{s:?} -> {text:?} {stops:?}");
        }
    }

    #[test]
    fn broken_user_files_only_warn() {
        for bad in
            ["", "[]", "null", "{ \"a\": 5 }", "{ \"a\": { \"prefix\": 3, \"body\": [1, 2] } }", "\u{feff}{}", "{ \"a\": { \"prefix\": [], \"body\": \"x\" } }"]
        {
            let (list, _) = parse_user_file("rust", bad);
            assert!(list.is_empty(), "{bad:?}");
        }
    }
}
