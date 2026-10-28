//! `cyx import-vscode`: turn the data parts of a VS Code extension (a folder or a `.vsix`) into a Codeyard extension.
//! Colour themes, snippets, languages (with their comment tokens) and local JSON schemas convert; TextMate grammars, key
//! bindings to VS Code commands and anything that runs code do not, and the report says so for each.

use std::path::{Path, PathBuf};

use serde_json::Value;

#[derive(Debug)]
pub struct ImportReport {
    pub out: PathBuf,
    pub converted: Vec<String>,
    pub skipped: Vec<String>,
}

/// VS Code's JSON allows comments and trailing commas in extension files.
fn parse_json(text: &str) -> Result<Value, String> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_str = false;
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            if c == '\\' {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_str = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut prev = ' ';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
            }
            _ => out.push(c),
        }
    }
    // Trailing commas before } or ].
    let mut cleaned = String::with_capacity(out.len());
    let bytes: Vec<char> = out.chars().collect();
    let mut in_str = false;
    for (i, &c) in bytes.iter().enumerate() {
        if c == '"' && (i == 0 || bytes[i - 1] != '\\') {
            in_str = !in_str;
        }
        if c == ',' && !in_str {
            let next = bytes[i + 1..].iter().find(|c| !c.is_whitespace());
            if matches!(next, Some('}') | Some(']')) {
                continue;
            }
        }
        cleaned.push(c);
    }
    serde_json::from_str(&cleaned).map_err(|e| e.to_string())
}

fn rel(p: &str) -> String {
    p.trim_start_matches("./").replace('\\', "/")
}

/// Unpack a `.vsix` (a zip) into a temporary folder with the system `tar` (bsdtar reads zip on macOS and Windows; on
/// Linux `unzip` is tried too). Returns the folder holding `package.json`.
fn unpack_vsix(file: &Path) -> Result<(PathBuf, PathBuf), String> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let tmp = std::env::temp_dir().join(format!("cyx-unpack-{}-{nanos:x}", std::process::id()));
    std::fs::create_dir(&tmp).map_err(|e| format!("error: cannot create {}: {e}", tmp.display()))?;
    let ok = std::process::Command::new("tar").arg("-xf").arg(file).arg("-C").arg(&tmp).status().is_ok_and(|s| s.success())
        || std::process::Command::new("unzip").arg("-q").arg(file).arg("-d").arg(&tmp).status().is_ok_and(|s| s.success());
    if !ok {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(format!("error: cannot unpack {}\n   = fix: unzip it yourself and pass the folder with package.json", file.display()));
    }
    let ext = tmp.join("extension");
    Ok((if ext.join("package.json").is_file() { ext } else { tmp.clone() }, tmp))
}

/// Removes the unpacked `.vsix` when the import ends, whatever the outcome.
struct TempDir(Option<PathBuf>);

impl Drop for TempDir {
    fn drop(&mut self) {
        if let Some(d) = &self.0 {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

pub fn import(src: &Path, out: Option<&Path>) -> Result<ImportReport, String> {
    let (root, _cleanup) = if src.extension().is_some_and(|e| e.eq_ignore_ascii_case("vsix")) {
        let (root, tmp) = unpack_vsix(src)?;
        (root, TempDir(Some(tmp)))
    } else {
        (src.to_path_buf(), TempDir(None))
    };
    let pkg_path = root.join("package.json");
    let pkg_text = std::fs::read_to_string(&pkg_path)
        .map_err(|e| format!("error: cannot read {}: {e}\n   = fix: pass a VS Code extension folder or .vsix", pkg_path.display()))?;
    let pkg = parse_json(&pkg_text).map_err(|e| format!("error: {}: {e}", pkg_path.display()))?;
    let s = |k: &str| pkg.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let name = crate::slug(&s("name")).unwrap_or_else(|| "imported".into());
    let publisher = crate::slug(&s("publisher")).unwrap_or_else(|| "imported".into());
    let display = Some(s("displayName")).filter(|d| !d.is_empty()).unwrap_or_else(|| s("name"));
    let version = ext_manifest::Version::parse(&s("version")).map(|v| v.to_string()).unwrap_or_else(|_| "0.1.0".into());
    let license = Some(s("license")).filter(|l| !l.is_empty()).unwrap_or_else(|| "UNLICENSED".into());
    let out = out.map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(format!("{name}-codeyard")));
    if out.exists() {
        return Err(format!("error: {}/ already exists\n   = fix: pass -o with another folder", out.display()));
    }
    let contributes = pkg.get("contributes").cloned().unwrap_or(Value::Null);
    let (mut converted, mut skipped) = (Vec::new(), Vec::new());
    let mut manifest = format!(
        "# Converted from the VS Code extension {publisher}.{name} by `cyx import-vscode`.\n[extension]\nid      = \"{publisher}.{name}\"\nversion = \"{version}\"\nname    = {}\nlicense = {}\n",
        toml_str(&display.chars().take(ext_manifest::limits::NAME_LEN).collect::<String>()),
        toml_str(&license)
    );
    let mut copies: Vec<String> = Vec::new();
    let mut copy = |p: &str, skipped: &mut Vec<String>| -> Option<String> {
        let r = rel(p);
        if ext_pkg::check_path(&r).is_err() || !root.join(&r).is_file() {
            skipped.push(format!("{p}: missing or outside the extension"));
            return None;
        }
        copies.push(r.clone());
        Some(r)
    };

    for t in contributes.get("themes").and_then(Value::as_array).into_iter().flatten() {
        let label = t.get("label").and_then(Value::as_str).unwrap_or("Theme");
        let Some(path) = t.get("path").and_then(Value::as_str) else { continue };
        let text = std::fs::read_to_string(root.join(rel(path))).unwrap_or_default();
        if parse_json(&text).is_ok_and(|v| v.get("include").is_some()) {
            skipped.push(format!("theme “{label}”: it includes another theme file; merge them into one and import again"));
            continue;
        }
        if let Some(r) = copy(path, &mut skipped) {
            manifest.push_str(&format!("\n[[themes]]\nlabel = {}\npath  = {}\n", toml_str(label), toml_str(&r)));
            converted.push(format!("theme “{label}”"));
        }
    }
    for t in contributes.get("iconThemes").and_then(Value::as_array).into_iter().flatten() {
        skipped.push(format!("icon theme “{}”: icon themes are not supported yet", t.get("label").and_then(Value::as_str).unwrap_or("?")));
    }
    for sn in contributes.get("snippets").and_then(Value::as_array).into_iter().flatten() {
        let (Some(lang), Some(path)) = (sn.get("language").and_then(Value::as_str), sn.get("path").and_then(Value::as_str)) else { continue };
        if let Some(r) = copy(path, &mut skipped) {
            manifest.push_str(&format!("\n[[snippets]]\nlanguage = {}\npath     = {}\n", toml_str(lang), toml_str(&r)));
            converted.push(format!("{lang} snippets"));
        }
    }
    for l in contributes.get("languages").and_then(Value::as_array).into_iter().flatten() {
        let Some(id) = l.get("id").and_then(Value::as_str) else { continue };
        if ext_manifest::check_id_part(id).is_err() {
            skipped.push(format!("language “{id}”: its id is not lowercase a-z, 0-9 and -"));
            continue;
        }
        let list =
            |k: &str| -> Vec<String> { l.get(k).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect() };
        let extensions: Vec<String> =
            list("extensions").into_iter().map(|e| e.trim_start_matches('.').to_string()).filter(|e| !e.is_empty() && !e.contains('/')).collect();
        let filenames: Vec<String> = list("filenames").into_iter().filter(|f| !f.contains('/')).collect();
        if extensions.is_empty() && filenames.is_empty() {
            skipped.push(format!("language “{id}”: it names no files"));
            continue;
        }
        let mut entry = format!("\n[[languages]]\nid = {}\n", toml_str(id));
        if !extensions.is_empty() {
            entry.push_str(&format!("extensions = {}\n", toml_list(&extensions)));
        }
        if !filenames.is_empty() {
            entry.push_str(&format!("filenames = {}\n", toml_list(&filenames)));
        }
        if let Some(conf) = l.get("configuration").and_then(Value::as_str)
            && let Ok(c) = std::fs::read_to_string(root.join(rel(conf))).map_err(|e| e.to_string()).and_then(|t| parse_json(&t))
        {
            if let Some(line) = c.pointer("/comments/lineComment").and_then(Value::as_str) {
                entry.push_str(&format!("line-comment = {}\n", toml_str(line)));
            }
            if let Some([a, b]) = c.pointer("/comments/blockComment").and_then(Value::as_array).map(|v| v.as_slice())
                && let (Some(a), Some(b)) = (a.as_str(), b.as_str())
            {
                entry.push_str(&format!("block-comment = [{}, {}]\n", toml_str(a), toml_str(b)));
            }
        }
        manifest.push_str(&entry);
        converted.push(format!("language “{id}”"));
    }
    if contributes.get("grammars").is_some() {
        skipped.push("TextMate grammars: Codeyard highlights with tree-sitter; add grammar = \"<built-in language>\" to a language to borrow one".into());
    }
    for j in contributes.get("jsonValidation").and_then(Value::as_array).into_iter().flatten() {
        let Some(url) = j.get("url").and_then(Value::as_str) else { continue };
        if url.contains("://") {
            skipped.push(format!("JSON schema {url}: remote schemas come from SchemaStore or your settings"));
            continue;
        }
        let globs: Vec<String> = match j.get("fileMatch") {
            Some(Value::String(g)) => vec![g.clone()],
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
            _ => Vec::new(),
        };
        if let Some(r) = copy(url, &mut skipped).filter(|_| !globs.is_empty()) {
            manifest.push_str(&format!("\n[[json-schemas]]\nfile-match = {}\npath = {}\n", toml_list(&globs), toml_str(&r)));
            converted.push(format!("JSON schema for {}", globs.join(", ")));
        }
    }
    if contributes.get("keybindings").is_some() {
        skipped.push("key bindings: they call VS Code commands; Codeyard's command names differ".into());
    }
    if pkg.get("main").is_some() || pkg.get("browser").is_some() {
        skipped.push("extension code (main/browser): Codeyard runs code extensions from a later version".into());
    }
    if converted.is_empty() {
        return Err(format!("error: nothing in {} converts: {}", pkg_path.display(), skipped.join("; ")));
    }
    std::fs::create_dir_all(&out).map_err(|e| format!("error: cannot create {}: {e}", out.display()))?;
    for r in &copies {
        let to = out.join(r);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("error: {e}"))?;
        }
        std::fs::copy(root.join(r), &to).map_err(|e| format!("error: cannot copy {r}: {e}"))?;
    }
    std::fs::write(out.join(ext_manifest::FILE_NAME), manifest).map_err(|e| format!("error: {e}"))?;
    if let Err(e) = crate::check(&out) {
        skipped.push(format!("the converted folder needs a fix before it installs:\n{e}"));
    }
    Ok(ImportReport { out, converted, skipped })
}

fn toml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn toml_list(items: &[String]) -> String {
    format!("[{}]", items.iter().map(|s| toml_str(s)).collect::<Vec<_>>().join(", "))
}
