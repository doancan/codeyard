//! Every code snippet in `docs/extensions/` is checked against the code it shows (docs/plugin-architecture.md §9):
//!
//! - `rust` and `ts` snippets are copied from an extension in `examples/extensions/`, which CI builds and tests. A
//!   snippet must appear there line by line (indentation aside), so it compiles as written. Short API tours live in
//!   `examples/extensions/doc-snippets` (Rust) and `doc-snippets-ts` (TypeScript).
//! - `toml` snippets are manifests, checked by `ext-manifest`: whole ones must be valid, and fragments may only lack
//!   what the rest of a manifest would add. A snippet whose first line is `# Cargo.toml` is checked as TOML.
//! - `json` snippets parse; `cyx test` fixtures parse as fixtures (`ext-ui`'s tests check the view reference's).
//! - `sh` snippets run only `cyx` commands and options that `cyx --help` lists.

use std::path::{Path, PathBuf};

use ext_manifest::Code;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn files_under(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if !p.file_name().is_some_and(|n| n == "target" || n == "node_modules") {
                files_under(&p, ext, out);
            }
        } else if p.extension().is_some_and(|e| e == ext) {
            out.push(p);
        }
    }
}

struct Snippet {
    /// `docs/extensions/x.md:12`.
    place: String,
    lang: String,
    text: String,
}

fn snippets() -> Vec<Snippet> {
    let mut docs = Vec::new();
    files_under(&repo().join("docs/extensions"), "md", &mut docs);
    docs.sort();
    let mut out = Vec::new();
    for doc in docs {
        let text = std::fs::read_to_string(&doc).unwrap().replace("\r\n", "\n");
        let name = doc.strip_prefix(repo()).unwrap_or(&doc).display().to_string();
        let mut open: Option<(usize, String, usize, Vec<&str>)> = None;
        for (i, line) in text.lines().enumerate() {
            let t = line.trim_start();
            match (&mut open, t.strip_prefix("```")) {
                (None, Some(info)) => open = Some((i + 1, info.split_whitespace().next().unwrap_or("").to_string(), line.len() - t.len(), Vec::new())),
                (Some((start, lang, indent, body)), Some("")) => {
                    let body = body.iter().map(|l| l.get(*indent..).unwrap_or(l.trim_start())).collect::<Vec<_>>().join("\n");
                    out.push(Snippet { place: format!("{name}:{start}"), lang: std::mem::take(lang), text: body });
                    open = None;
                }
                (Some((.., body)), _) => body.push(line),
                (None, None) => {}
            }
        }
        assert!(open.is_none(), "{name}: a code block is not closed");
    }
    out
}

/// The non-blank lines of `text`, without indentation or trailing spaces.
fn lines(text: &str) -> Vec<&str> {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).collect()
}

/// The files that hold `ext` snippets, with their lines.
fn sources(ext: &str) -> Vec<(PathBuf, String)> {
    let mut files = Vec::new();
    files_under(&repo().join("examples/extensions"), ext, &mut files);
    files.into_iter().map(|p| (p.clone(), std::fs::read_to_string(&p).unwrap())).collect()
}

fn appears_in(snippet: &[&str], file: &[&str]) -> bool {
    !snippet.is_empty() && file.windows(snippet.len()).any(|w| w == snippet)
}

/// A manifest snippet: a whole manifest must be valid; a fragment gets a header and may only lack the rest of a manifest.
fn check_manifest(s: &Snippet) -> Result<(), String> {
    if s.text.trim_start().starts_with("# Cargo.toml") {
        return s.text.parse::<toml_edit::DocumentMut>().map(drop).map_err(|e| e.to_string());
    }
    let whole = s.text.contains("[extension]");
    let text = if whole {
        s.text.clone()
    } else {
        let code = if s.text.contains("[code]") { "" } else { "[code]\nentry = \"Cargo.toml\"\n" };
        format!("[extension]\nid = \"you.example\"\nversion = \"0.1.0\"\nname = \"Example\"\nlicense = \"MIT\"\n{code}{}\n", s.text)
    };
    let Err(errors) = ext_manifest::parse(&text) else { return Ok(()) };
    // What a fragment cannot show: the commands, permissions and `why` of the rest of its manifest.
    let incomplete = [Code::NeverActivates, Code::NotDeclared, Code::WhyRequired];
    let real: Vec<String> = errors.iter().filter(|d| whole || !incomplete.contains(&d.code)).map(|d| d.render(ext_manifest::FILE_NAME, &text)).collect();
    if real.is_empty() { Ok(()) } else { Err(real.join("\n")) }
}

fn check_json(s: &Snippet) -> Result<(), String> {
    let value: serde_json::Value = serde_json::from_str(&s.text).map_err(|e| e.to_string())?;
    if ["http", "files", "commands"].iter().any(|k| value.get(k).is_some()) {
        crate::fixtures::Fixtures::parse(&s.text)?;
    }
    Ok(())
}

/// `cyx` lines of a shell snippet: the command and each option must be in the help.
fn check_shell(s: &Snippet) -> Result<(), String> {
    let help = format!("{}\n{}", crate::HELP, crate::yard::HELP);
    for line in s.text.lines() {
        let line = line.trim().trim_start_matches("$ ");
        let line = line.split('#').next().unwrap_or_default();
        for part in line.split("&&") {
            let words: Vec<&str> = part.split_whitespace().collect();
            if words.first() != Some(&"cyx") {
                continue;
            }
            let Some(cmd) = words.get(1) else { continue };
            if !help.contains(&format!("cyx {cmd}")) && !help.contains(&format!("| {cmd}")) {
                return Err(format!("`cyx {cmd}` is not a cyx command"));
            }
            for w in words.iter().filter(|w| w.starts_with("--")) {
                if !help.contains(*w) {
                    return Err(format!("`cyx {cmd}` has no option {w}"));
                }
            }
        }
    }
    Ok(())
}

#[test]
fn every_snippet_in_the_extension_docs_is_checked() {
    let rust = sources("rs");
    let ts = sources("ts");
    let as_lines = |files: &[(PathBuf, String)]| files.iter().map(|(_, t)| lines(t).into_iter().map(String::from).collect::<Vec<_>>()).collect::<Vec<_>>();
    let (rust_lines, ts_lines) = (as_lines(&rust), as_lines(&ts));
    let mut failures = Vec::new();
    let mut checked = 0;
    for s in snippets() {
        let result = match s.lang.as_str() {
            "rust" | "ts" => {
                let files = if s.lang == "rust" { &rust_lines } else { &ts_lines };
                let want = lines(&s.text);
                let found = files.iter().any(|f| appears_in(&want, &f.iter().map(String::as_str).collect::<Vec<_>>()));
                if found {
                    Ok(())
                } else {
                    Err(format!("not found in examples/extensions; copy it into a built example (doc-snippets{})", if s.lang == "ts" { "-ts" } else { "" }))
                }
            }
            "toml" => check_manifest(&s),
            "json" => check_json(&s),
            "sh" => check_shell(&s),
            _ => continue,
        };
        checked += 1;
        if let Err(e) = result {
            failures.push(format!("{} ({}): {e}", s.place, s.lang));
        }
    }
    assert!(checked > 50, "only {checked} snippets found: is docs/extensions there?");
    assert!(failures.is_empty(), "{} snippet(s) do not match the code:\n\n{}", failures.len(), failures.join("\n\n"));
}

#[test]
fn snippets_match_line_by_line_ignoring_indentation() {
    let file = lines("fn a() {\n    let x = 1;\n\n    ctx.notice(\"x\");\n}\n");
    assert!(appears_in(&lines("let x = 1;\nctx.notice(\"x\");"), &file));
    assert!(!appears_in(&lines("let x = 2;"), &file));
    assert!(!appears_in(&lines("ctx.notice(\"x\");\nlet x = 1;"), &file));
    let fragment = Snippet { place: String::new(), lang: "toml".into(), text: "[[commands]]\nid = \"go\"\ntitle = \"Go\"\n".into() };
    assert!(check_manifest(&fragment).is_ok());
    let typo = Snippet { place: String::new(), lang: "toml".into(), text: "[[commands]]\nid = \"go\"\ntitel = \"Go\"\n".into() };
    assert!(check_manifest(&typo).unwrap_err().contains("CY0101"));
    let sh = |t: &str| check_shell(&Snippet { place: String::new(), lang: "sh".into(), text: t.into() });
    assert!(sh("cyx check --fix\ncd x && cyx publish --dry-run").is_ok());
    assert!(sh("cyx dev --as-someone").is_err());
    assert!(sh("cyx frobnicate").is_err());
}

/// GitHub's anchor for a heading: lowercase, punctuation dropped, spaces as `-`.
fn slug(heading: &str) -> String {
    heading
        .trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect()
}

fn anchors(md: &str) -> Vec<String> {
    let mut fenced = false;
    let mut out = Vec::new();
    for line in md.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        } else if !fenced && line.starts_with('#') {
            out.push(slug(line.trim_start_matches('#')));
        }
    }
    out
}

/// Links between the extension docs, and from them into the repository, lead somewhere: the file exists and, for a
/// page, so does the heading the link names.
#[test]
fn every_link_in_the_extension_docs_resolves() {
    let mut docs = Vec::new();
    files_under(&repo().join("docs/extensions"), "md", &mut docs);
    let mut broken = Vec::new();
    for doc in &docs {
        let text = std::fs::read_to_string(doc).unwrap();
        let mut fenced = false;
        for (n, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("```") {
                fenced = !fenced;
            }
            if fenced {
                continue;
            }
            for (i, _) in line.match_indices("](") {
                let Some(target) = line[i + 2..].split(')').next() else { continue };
                if target.contains("://") || target.starts_with("mailto:") {
                    continue;
                }
                let (path, anchor) = target.split_once('#').map_or((target, None), |(p, a)| (p, Some(a)));
                let file = if path.is_empty() { doc.clone() } else { doc.parent().unwrap().join(path) };
                let place = format!("{}:{}", doc.strip_prefix(repo()).unwrap_or(doc).display(), n + 1);
                if !file.exists() {
                    broken.push(format!("{place}: {target}: no such file"));
                } else if let Some(a) = anchor.filter(|_| file.extension().is_some_and(|e| e == "md"))
                    && !anchors(&std::fs::read_to_string(&file).unwrap()).iter().any(|h| h == a)
                {
                    broken.push(format!("{place}: {target}: no heading with that anchor"));
                }
            }
        }
    }
    assert!(broken.is_empty(), "broken links:\n{}", broken.join("\n"));
}
