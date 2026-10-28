//! The facade the editor uses: diagnostics, completion, hover and schema matching. All positions are byte offsets
//! into the document text; the caller converts them to the editor's char/UTF-16 positions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::cursor::{self, Ctx, Slot};
use crate::doc::{self, Parsed};
use crate::store::{FileSource, Store, glob_match};
use crate::validate::{SchemaSet, range_of};
use crate::walk::{Kind, Seg};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Json,
    Yaml,
}

/// Format from the file name and language name.
pub fn format_of(path: &str) -> Option<Format> {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    match ext {
        "json" | "jsonc" | "json5" | "webmanifest" | "geojson" | "code-workspace" => Some(Format::Json),
        "yaml" | "yml" => Some(Format::Yaml),
        _ if matches!(name.as_str(), ".babelrc" | ".eslintrc" | ".prettierrc" | ".swcrc" | ".stylelintrc") => Some(Format::Json),
        _ => None,
    }
}

/// Document size cap: larger ones aren't validated (parsing on every keystroke gets expensive).
pub const MAX_BYTES: usize = 1_500_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diag {
    pub start: usize,
    pub end: usize,
    pub message: String,
    /// 1 error, 2 warning.
    pub severity: u8,
}

/// Snaps a byte range to char boundaries and widens it to at least one character.
fn clamp(text: &str, start: usize, end: usize) -> (usize, usize) {
    let mut s = start.min(text.len());
    while !text.is_char_boundary(s) {
        s -= 1;
    }
    let mut e = end.max(s).min(text.len());
    while !text.is_char_boundary(e) {
        e += 1;
    }
    if e == s {
        e = text[s..].chars().next().map_or(s, |c| s + c.len_utf8());
        if e == s {
            s = text[..s].char_indices().next_back().map_or(0, |(i, _)| i);
            e = text.len().max(s);
        }
    }
    (s, e)
}

fn parse_docs(text: &str, format: Format) -> Vec<(usize, Result<Parsed, doc::SyntaxError>)> {
    match format {
        Format::Json => vec![(0, doc::parse_json(text))],
        Format::Yaml => doc::parse_yaml(text),
    }
}

/// An unexpected panic (external library) must not take the editor down: the work runs guarded and `default` is returned.
fn guarded<T>(default: T, f: impl FnOnce() -> T) -> T {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or(default)
}

/// Syntax errors always; schema violations too if a schema is given.
pub fn diagnose(text: &str, format: Format, set: Option<&SchemaSet>) -> Vec<Diag> {
    let (s, e) = clamp(text, 0, text.find('\n').unwrap_or(text.len()));
    guarded(vec![Diag { start: s, end: e, message: "The schema check failed unexpectedly; the file itself is not affected.".into(), severity: 2 }], || {
        diagnose_inner(text, format, set)
    })
}

pub(crate) fn diagnose_inner(text: &str, format: Format, set: Option<&SchemaSet>) -> Vec<Diag> {
    let mut out = Vec::new();
    if text.len() > MAX_BYTES {
        // Say why instead of skipping silently (hint level: not counted in Problems).
        let (s, e) = clamp(text, 0, text.find('\n').unwrap_or(text.len()));
        out.push(Diag { start: s, end: e, message: format!("Schema checks are skipped for files over {:.1} MB.", MAX_BYTES as f64 / 1_000_000.), severity: 4 });
        return out;
    }
    if text.trim().is_empty() {
        return out;
    }
    for (_, parsed) in parse_docs(text, format) {
        match parsed {
            Err(e) => {
                let (s, en) = clamp(text, e.offset, e.offset + 1);
                out.push(Diag { start: s, end: en, message: e.message, severity: 1 });
            }
            Ok(p) => {
                if p.value.is_null() {
                    continue;
                }
                let Some(set) = set else { continue };
                match set.validate(&p) {
                    Ok(violations) => {
                        for v in violations {
                            let (a, b) = range_of(&p, text, &v.pointer);
                            let (s, en) = clamp(text, a, b);
                            out.push(Diag { start: s, end: en, message: v.message, severity: 1 });
                        }
                    }
                    // Schema failed to compile: not a document error; report one warning on the first line.
                    Err(e) => {
                        let (s, en) = clamp(text, 0, text.find('\n').unwrap_or(text.len()));
                        out.push(Diag { start: s, end: en, message: e, severity: 2 });
                        break;
                    }
                }
            }
        }
    }
    out
}

// --- Tamamlama ---

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub label: String,
    pub detail: Option<String>,
    pub doc: Option<String>,
    /// Text that replaces the range.
    pub insert: String,
    /// Cursor position within `insert` (chars); `None`: at the end.
    pub cursor: Option<usize>,
    /// Range to replace (bytes).
    pub start: usize,
    pub end: usize,
    /// LSP CompletionItemKind: 10 property, 20 enum member, 12 value.
    pub kind: u8,
    pub sort: String,
    pub deprecated: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Completions {
    /// Start of the typed prefix used for filtering (byte); just after the quote when quoted.
    pub filter_start: usize,
    pub items: Vec<Item>,
}

fn json_string(s: &str) -> String {
    Value::String(s.to_string()).to_string()
}

/// Text that would turn into another type (or syntax) if written plain in YAML gets quoted.
fn yaml_scalar(s: &str) -> String {
    let special_start = s.starts_with(|c: char| "-?:,[]{}#&*!|>'\"%@`".contains(c) || c.is_whitespace());
    let risky = s.is_empty() || special_start || s.ends_with(' ') || s.contains(": ") || s.contains(" #") || !matches!(doc::plain_scalar(s), Value::String(_));
    if risky { json_string(s) } else { s.to_string() }
}

fn literal(format: Format, v: &Value) -> String {
    match (format, v) {
        (Format::Json, Value::String(s)) => json_string(s),
        (Format::Yaml, Value::String(s)) => yaml_scalar(s),
        (_, other) => other.to_string(),
    }
}

pub fn complete(set: &SchemaSet, format: Format, text: &str, offset: usize) -> Option<Completions> {
    guarded(None, || complete_inner(set, format, text, offset))
}

pub(crate) fn complete_inner(set: &SchemaSet, format: Format, text: &str, offset: usize) -> Option<Completions> {
    if text.len() > MAX_BYTES {
        return None;
    }
    let ctx: Ctx = match format {
        Format::Json => cursor::json_ctx(text, offset)?,
        Format::Yaml => cursor::yaml_ctx(text, offset)?,
    };
    let filter_start = if ctx.quoted { ctx.start + 1 } else { ctx.start };
    let mut items = Vec::new();
    match ctx.slot {
        Slot::Key => {
            for p in set.property_candidates(&ctx.path, &ctx.existing) {
                let (insert, cursor) = key_insert(format, &ctx, &p.name, p.kind, &p.detail);
                items.push(Item {
                    label: p.name.clone(),
                    detail: Some(match (p.detail.is_empty(), p.required) {
                        (false, true) => format!("{} · required", p.detail),
                        (false, false) => p.detail.clone(),
                        (true, true) => "required".into(),
                        (true, false) => String::new(),
                    })
                    .filter(|d| !d.is_empty()),
                    doc: p.doc,
                    insert,
                    cursor,
                    start: ctx.start,
                    end: ctx.end,
                    kind: 10,
                    sort: format!("{}{}", if p.required { "0" } else { "1" }, p.name),
                    deprecated: p.deprecated,
                });
            }
        }
        Slot::Value => {
            for (n, c) in set.value_candidates_at(&ctx.path).into_iter().enumerate() {
                items.push(Item {
                    label: c.label.clone(),
                    detail: c.doc.clone(),
                    doc: None,
                    insert: literal(format, &c.value),
                    cursor: None,
                    start: ctx.start,
                    end: ctx.end,
                    kind: 20,
                    sort: format!("{n:04}"),
                    deprecated: false,
                });
            }
        }
    }
    (!items.is_empty()).then_some(Completions { filter_start, items })
}

fn key_insert(format: Format, ctx: &Ctx, name: &str, kind: Kind, detail: &str) -> (String, Option<usize>) {
    match format {
        Format::Json => {
            let key = json_string(name);
            if ctx.colon_follows {
                return (key, None);
            }
            let (tail, back) = match (kind, detail) {
                (Kind::Object, _) => ("{}", 1),
                (Kind::Array, _) => ("[]", 1),
                (_, "string") => ("\"\"", 1),
                _ => ("", 0),
            };
            let s = format!("{key}: {tail}");
            let n = s.chars().count();
            (s, Some(n - back))
        }
        Format::Yaml => {
            let key = if name.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/')) { name.to_string() } else { json_string(name) };
            if ctx.colon_follows {
                return (key, None);
            }
            let pad = " ".repeat(ctx.indent);
            match kind {
                Kind::Object => (format!("{key}:\n{pad}  "), None),
                Kind::Array => (format!("{key}:\n{pad}  - "), None),
                Kind::Scalar => (format!("{key}: "), None),
            }
        }
    }
}

// --- Hover ---

/// Convert a pointer to a schema path: a numeric segment is an index if the container is an array.
pub fn path_of(parsed: &Parsed, pointer: &str) -> Vec<Seg> {
    let mut out = Vec::new();
    let mut prefix = String::new();
    for raw in pointer.split('/').skip(1) {
        let seg = raw.replace("~1", "/").replace("~0", "~");
        let parent_is_array = matches!(parsed.value.pointer(&prefix), Some(Value::Array(_)));
        out.push(match (parent_is_array, seg.parse::<usize>()) {
            (true, Ok(i)) => Seg::Index(i),
            _ => Seg::Key(seg),
        });
        prefix.push('/');
        prefix.push_str(raw);
    }
    out
}

/// Schema description of the node under the cursor and the range to highlight.
pub fn hover(set: &SchemaSet, format: Format, text: &str, offset: usize) -> Option<(String, usize, usize)> {
    guarded(None, || hover_inner(set, format, text, offset))
}

pub(crate) fn hover_inner(set: &SchemaSet, format: Format, text: &str, offset: usize) -> Option<(String, usize, usize)> {
    if text.len() > MAX_BYTES {
        return None;
    }
    for (_, parsed) in parse_docs(text, format) {
        let Ok(p) = parsed else { continue };
        // Narrowest enclosing node; the key span is checked before the value span.
        let mut best: Option<(usize, String, (usize, usize))> = None;
        for (pointer, span) in &p.spans {
            let hit = span
                .key
                .as_ref()
                .filter(|k| k.contains(&offset))
                .map(|k| (k.end - k.start, (k.start, k.end)))
                .or_else(|| span.value.contains(&offset).then(|| (span.value.end - span.value.start, (span.value.start, span.value.end))));
            if let Some((len, range)) = hit
                && best.as_ref().is_none_or(|(l, ..)| len < *l)
            {
                best = Some((len, pointer.clone(), range));
            }
        }
        if let Some((_, pointer, (a, b))) = best
            && let Some(h) = set.hover(&path_of(&p, &pointer))
        {
            return Some((h, a, b));
        }
    }
    None
}

// --- Schema matching ---

/// The document's own declaration: root `$schema` in JSON, a `# yaml-language-server: $schema=...` line in YAML.
pub fn declared_schema(text: &str, format: Format) -> Option<String> {
    match format {
        Format::Json => {
            // The cut may fall inside a multibyte character (any Turkish text will do): back up to a boundary.
            let mut cut = text.len().min(4096);
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            let head = &text[..cut];
            let at = head.find("\"$schema\"")? + 9;
            let rest = head[at..].trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
            Some(rest[..rest.find('"')?].to_string())
        }
        Format::Yaml => text.lines().take(30).find_map(|l| {
            let l = l.trim_start().strip_prefix('#')?.trim_start();
            let rest = l.strip_prefix("yaml-language-server:")?.trim_start().strip_prefix("$schema=")?;
            Some(rest.trim().to_string())
        }),
    }
}

/// The user's mappings: (file patterns, schema URL), from the `json.schemas` / `yaml.schemas` settings.
pub type UserAssociations = [(Vec<String>, String)];

fn resolve_relative(source: &str, file: &str) -> String {
    if source.contains("://") || Path::new(source).is_absolute() {
        return source.to_string();
    }
    let dir = Path::new(file).parent().map(Path::to_path_buf).unwrap_or_default();
    normalize(&dir.join(source))
}

/// Lexically collapse `.` and `..` segments (`/a/./b/../c` -> `/a/c`) so the cache key and displayed path are clean.
fn normalize(path: &Path) -> String {
    use std::path::Component;
    let mut parts: Vec<String> = Vec::new();
    let mut absolute = false;
    let mut prefix = String::new();
    for c in path.components() {
        match c {
            Component::RootDir => absolute = true,
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.last().is_some_and(|p| p != "..") {
                    parts.pop();
                } else if !absolute {
                    parts.push("..".into());
                }
            }
            Component::Normal(n) => parts.push(n.to_string_lossy().into_owned()),
            Component::Prefix(p) => {
                let p = p.as_os_str().to_string_lossy();
                prefix = if let Some(rest) = p.strip_prefix(r"\\?\UNC\") {
                    format!("//{rest}").replace('\\', "/")
                } else {
                    p.strip_prefix(r"\\?\").unwrap_or(&p).replace('\\', "/")
                };
            }
        }
    }
    format!("{prefix}{}{}", if absolute { "/" } else { "" }, parts.join("/"))
}

/// Schema for a file: the document's declaration, then user mappings, then the SchemaStore catalog. The catalog may hit
/// the network (cached), so call this in the background.
pub fn associate(path: &str, text: &str, format: Format, user: &UserAssociations, root: Option<&str>, store: &Store) -> Option<String> {
    if let Some(s) = declared_schema(text, format) {
        return Some(resolve_relative(&s, path));
    }
    for (globs, url) in user {
        if globs.iter().any(|g| glob_match(g, path)) {
            // A relative path in settings is relative to the workspace folder (like VS Code); without a root, the file's folder.
            return Some(match root {
                Some(r) if !url.contains("://") && !Path::new(url).is_absolute() => resolve_relative(url, &format!("{}/_", r.trim_end_matches('/'))),
                _ => resolve_relative(url, path),
            });
        }
    }
    store.catalog().ok().and_then(|c| c.find(path).map(|e| e.url.clone()))
}

/// Service that keeps schema instances in memory (one per app).
pub struct Service {
    pub store: Arc<Store>,
    sets: Mutex<HashMap<String, Arc<SchemaSet>>>,
    /// Folders on this machine whose schemas are always read here, also for a remote workspace (installed extensions).
    local_only: Mutex<Vec<PathBuf>>,
}

pub struct Analysis {
    pub set: Option<Arc<SchemaSet>>,
    pub diags: Vec<Diag>,
    pub source: Option<String>,
}

impl Service {
    pub fn new(cache: Option<PathBuf>, download: bool) -> Arc<Service> {
        Arc::new(Service { store: Store::new(cache, download), sets: Mutex::new(HashMap::new()), local_only: Mutex::new(Vec::new()) })
    }

    pub fn with_store(store: Arc<Store>) -> Arc<Service> {
        Arc::new(Service { store, sets: Mutex::new(HashMap::new()), local_only: Mutex::new(Vec::new()) })
    }

    /// Folders whose schema files are read on this machine even when `files` reads a remote workspace.
    pub fn set_local_only(&self, dirs: Vec<PathBuf>) {
        *self.local_only.lock().unwrap_or_else(|e| e.into_inner()) = dirs;
    }

    pub fn clear(&self) {
        self.sets.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.store.clear();
    }

    /// Schema for a source (URL or path); reloaded if the local file changed. `files`: local paths are read on the
    /// workspace's host (SSH), not on this machine.
    pub fn set_for(&self, source: &str, files: Option<&Arc<dyn FileSource>>) -> Result<Arc<SchemaSet>, String> {
        let local = !source.contains("://") && self.local_only.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|d| Path::new(source).starts_with(d));
        let files = files.filter(|_| !local);
        // Before the key below stats the path: that alone would reach a network share.
        if !source.contains("://") {
            self.store.check_local(source)?;
        }
        let key = match (source.contains("://"), files) {
            (true, _) => source.to_string(),
            (false, Some(f)) => format!("remote:{source}@{}", f.mtime(source).unwrap_or(0)),
            (false, None) => {
                let mtime = std::fs::metadata(source)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_nanos());
                format!("{source}@{mtime}")
            }
        };
        if let Some(s) = self.sets.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            return Ok(s.clone());
        }
        let set = SchemaSet::load_with(source, &self.store, files.cloned())?;
        let mut sets = self.sets.lock().unwrap_or_else(|e| e.into_inner());
        if sets.len() > 24 {
            sets.clear();
        }
        sets.insert(key, set.clone());
        Ok(set)
    }

    /// Match, load, validate (blocks: call in the background).
    pub fn analyze(&self, path: &str, text: &str, format: Format, user: &UserAssociations, root: Option<&str>, files: Option<Arc<dyn FileSource>>) -> Analysis {
        let source = associate(path, text, format, user, root, &self.store);
        let loaded = source.as_deref().map(|s| self.set_for(s, files.as_ref()));
        let (set, load_error) = match loaded {
            Some(Ok(s)) => (Some(s), None),
            Some(Err(e)) => (None, Some(e)),
            None => (None, None),
        };
        let mut diags = diagnose(text, format, set.as_deref());
        // If the declared schema can't be fetched, say why (mistyped URL, no network). JSON Schema's own meta-schemas
        // (`json-schema.org`) produce no warning offline: that file is a schema, so it isn't an error.
        if let (Some(e), Some(src)) = (load_error, source.as_deref())
            && declared_schema(text, format).is_some()
            && !is_meta_schema(src)
        {
            let (s, en) = clamp(text, 0, text.find('\n').unwrap_or(text.len()));
            diags.push(Diag { start: s, end: en, message: format!("Could not load the schema: {e}"), severity: 2 });
        }
        Analysis { set, diags, source }
    }
}

/// JSON Schema's own definition schemas (as `$schema` values).
pub fn is_meta_schema(url: &str) -> bool {
    let rest = url.split_once("://").map_or("", |(_, r)| r);
    rest.starts_with("json-schema.org/") || rest.starts_with("www.json-schema.org/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn local_only_folders_are_read_here_for_a_remote_workspace() {
        struct Remote;
        impl FileSource for Remote {
            fn read(&self, _: &str) -> Result<String, String> {
                Err("remote host has no such file".into())
            }
            fn mtime(&self, _: &str) -> Option<i64> {
                None
            }
        }
        let dir = std::env::temp_dir().join(format!("schema-local-only-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let schema = dir.join("s.json");
        std::fs::write(&schema, r#"{"type":"object","required":["name"]}"#).unwrap();
        let svc = Service::new(None, false);
        let remote: Arc<dyn FileSource> = Arc::new(Remote);
        let src = schema.to_string_lossy().into_owned();
        assert!(svc.set_for(&src, Some(&remote)).is_err(), "without the folder, the remote host is asked");
        svc.set_local_only(vec![dir.clone()]);
        assert!(svc.set_for(&src, Some(&remote)).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_multibyte_character_across_the_scan_limit_does_not_panic() {
        let text = format!("{{\"a\":\"{}ş\"}}", "x".repeat(4089));
        assert!(!text.is_char_boundary(4096), "the cut would land inside ş");
        assert_eq!(declared_schema(&text, Format::Json), None);
        let declared = format!("{{\"$schema\":\"https://example.com/s.json\",\"a\":\"{}ş\"}}", "x".repeat(4050));
        assert_eq!(declared_schema(&declared, Format::Json).as_deref(), Some("https://example.com/s.json"));
    }

    fn store() -> Arc<Store> {
        Store::new(None, false)
    }

    fn set() -> Arc<SchemaSet> {
        SchemaSet::from_value(
            json!({
                "type": "object",
                "required": ["name"],
                "additionalProperties": false,
                "properties": {
                    "name": {"type": "string", "description": "The name"},
                    "private": {"type": "boolean"},
                    "license": {"enum": ["MIT", "ISC", "true", "a: b"]},
                    "scripts": {"type": "object", "additionalProperties": {"type": "string"}},
                    "files": {"type": "array", "items": {"type": "string"}}
                }
            }),
            "file:///s.json",
            &store(),
        )
    }

    fn marked(m: &str) -> (String, usize) {
        let i = m.find('|').unwrap();
        (m.replacen('|', "", 1), i)
    }

    /// Applies: `insert` in place of `start..end`, with the cursor position.
    fn apply(text: &str, c: &Completions, label: &str) -> (String, String) {
        let item =
            c.items.iter().find(|i| i.label == label).unwrap_or_else(|| panic!("no {label} in {:?}", c.items.iter().map(|i| &i.label).collect::<Vec<_>>()));
        let mut out = text[..item.start].to_string();
        out.push_str(&item.insert);
        out.push_str(&text[item.end..]);
        let at = item.start + item.insert.char_indices().nth(item.cursor.unwrap_or(item.insert.chars().count())).map_or(item.insert.len(), |(i, _)| i);
        (out.clone(), format!("{}|{}", &out[..at], &out[at..]))
    }

    #[test]
    fn json_key_completion_replaces_the_quoted_fragment_and_adds_colon_and_template() {
        let s = set();
        let (t, i) = marked("{\n  \"na|\"\n}");
        let c = complete(&s, Format::Json, &t, i).unwrap();
        assert_eq!(&t[c.filter_start..i], "na");
        let (_, shown) = apply(&t, &c, "name");
        assert_eq!(shown, "{\n  \"name\": \"|\"\n}", "string property gets quotes with the cursor between");
        let (_, shown) = apply(&t, &c, "scripts");
        assert_eq!(shown, "{\n  \"scripts\": {|}\n}");
        let (_, shown) = apply(&t, &c, "files");
        assert_eq!(shown, "{\n  \"files\": [|]\n}");
        let (_, shown) = apply(&t, &c, "private");
        assert_eq!(shown, "{\n  \"private\": |\n}");
        // Required properties sort first; existing keys are not offered again.
        let (t2, i2) = marked("{\"name\": \"x\", |}");
        let c2 = complete(&s, Format::Json, &t2, i2).unwrap();
        assert!(c2.items.iter().all(|i| i.label != "name"));
        assert_eq!(c2.items[0].sort.chars().next(), Some('1'));
        assert_eq!(c.items.iter().find(|i| i.label == "name").unwrap().detail.as_deref(), Some("string · required"));
        assert_eq!(c.items.iter().find(|i| i.label == "name").unwrap().doc.as_deref(), Some("The name"));
    }

    #[test]
    fn json_renaming_a_key_that_already_has_a_colon_keeps_the_rest() {
        let s = set();
        let (t, i) = marked("{\"na|\": 1}");
        let c = complete(&s, Format::Json, &t, i).unwrap();
        let (out, _) = apply(&t, &c, "name");
        assert_eq!(out, "{\"name\": 1}");
    }

    #[test]
    fn json_value_completion_quotes_strings_and_leaves_literals_bare() {
        let s = set();
        let (t, i) = marked("{\"license\": \"|\"}");
        let c = complete(&s, Format::Json, &t, i).unwrap();
        assert_eq!(apply(&t, &c, "ISC").0, "{\"license\": \"ISC\"}", "the auto-closed quote is consumed, not doubled");
        let (t, i) = marked("{\"private\": |}");
        let c = complete(&s, Format::Json, &t, i).unwrap();
        assert_eq!(apply(&t, &c, "true").0, "{\"private\": true}");
        let (t, i) = marked("{\"files\": [|]}");
        assert!(complete(&s, Format::Json, &t, i).is_none(), "array items are free strings: nothing to offer");
    }

    #[test]
    fn yaml_completion_builds_indented_templates() {
        let s = set();
        let (t, i) = marked("name: x\nsc|");
        let c = complete(&s, Format::Yaml, &t, i).unwrap();
        assert_eq!(apply(&t, &c, "scripts").0, "name: x\nscripts:\n  ");
        assert_eq!(apply(&t, &c, "files").0, "name: x\nfiles:\n  - ");
        let (t, i) = marked("name: x\nlicense: |");
        let c = complete(&s, Format::Yaml, &t, i).unwrap();
        assert_eq!(apply(&t, &c, "MIT").0, "name: x\nlicense: MIT");
        assert_eq!(apply(&t, &c, "true").0, "name: x\nlicense: \"true\"", "would otherwise become a boolean");
        assert_eq!(apply(&t, &c, "a: b").0, "name: x\nlicense: \"a: b\"");
        let (t, i) = marked("scripts:\n  bu|");
        assert!(complete(&s, Format::Yaml, &t, i).is_none(), "free-form maps have no key suggestions");
        let (t, i) = marked("scripts:\n  build: x\n  |");
        assert!(complete(&s, Format::Yaml, &t, i).is_none());
    }

    #[test]
    fn diagnostics_cover_syntax_errors_schema_violations_and_broken_schemas() {
        let s = set();
        let d = diagnose("{\"name\": 1, \"extra\": true}", Format::Json, Some(&s));
        assert_eq!(d.len(), 2, "{d:?}");
        assert!(d.iter().any(|x| x.message.contains("string")));
        assert!(d.iter().any(|x| x.message.contains("extra")));
        let bad = diagnose("{\"name\": }", Format::Json, Some(&s));
        assert_eq!(bad.len(), 1);
        assert!(bad[0].message.contains("Unexpected"));
        let y = diagnose("name: 1\nextra: 2\n", Format::Yaml, Some(&s));
        assert_eq!(y.len(), 2, "{y:?}");
        assert!(diagnose("", Format::Json, Some(&s)).is_empty());
        assert!(diagnose("{}", Format::Json, None).is_empty());
        // Multi-document YAML: each document is checked, an empty one is skipped.
        let multi = diagnose("---\nname: ok\n---\nname: 3\n---\n", Format::Yaml, Some(&s));
        assert_eq!(multi.len(), 1, "{multi:?}");
        // Diagnostic ranges are on character boundaries even with non-ASCII text.
        let u = diagnose("{\"ğ\": 1, \"name\": \"ü\"}", Format::Json, Some(&s));
        assert!(u.iter().all(|x| "{\"ğ\": 1, \"name\": \"ü\"}".is_char_boundary(x.start) && "{\"ğ\": 1, \"name\": \"ü\"}".is_char_boundary(x.end)));
        let broken =
            SchemaSet::from_value(json!({"properties": {"x": {"$ref": "https://nowhere.invalid/a.json"}}}), "https://nowhere.invalid/r.json", &store());
        let d = diagnose("{}", Format::Json, Some(&broken));
        assert_eq!((d.len(), d[0].severity), (1, 2));
    }

    #[test]
    fn hover_finds_the_property_under_the_cursor() {
        let s = set();
        let t = "{\n  \"name\": \"x\",\n  \"private\": true\n}";
        let i = t.find("name").unwrap() + 1;
        let (text, a, b) = hover(&s, Format::Json, t, i).unwrap();
        assert!(text.starts_with("The name"));
        assert_eq!(&t[a..b], "\"name\"");
        let (text, ..) = hover(&s, Format::Yaml, "name: x\n", 1).unwrap();
        assert!(text.contains("Type: string"));
        assert!(hover(&s, Format::Json, t, t.find("true").unwrap()).is_none(), "no description on `private`");
    }

    #[test]
    fn schemas_are_associated_by_declaration_settings_and_globs() {
        let st = store();
        let user = vec![(vec!["**/k8s/*.yaml".to_string()], "https://example.com/k8s.json".to_string())];
        assert_eq!(associate("/p/a.json", "{\"$schema\": \"https://x/s.json\", \"a\": 1}", Format::Json, &user, None, &st).unwrap(), "https://x/s.json");
        assert_eq!(associate("/p/a.json", "{\"$schema\": \"./local.schema.json\"}", Format::Json, &user, None, &st).unwrap(), "/p/local.schema.json");
        assert_eq!(
            associate("/p/c.yaml", "# yaml-language-server: $schema=https://y/s.json\nname: 1\n", Format::Yaml, &user, None, &st).unwrap(),
            "https://y/s.json"
        );
        assert_eq!(associate("/repo/k8s/dep.yaml", "kind: Pod\n", Format::Yaml, &user, None, &st).unwrap(), "https://example.com/k8s.json");
        assert!(associate("/repo/other.yaml", "a: 1\n", Format::Yaml, &user, None, &st).is_none(), "downloads off and no cache: no catalog");
        assert_eq!(declared_schema("{\"a\": 1}", Format::Json), None);
        assert_eq!(declared_schema("# yaml-language-server: $schema=/abs/s.json\n", Format::Yaml).unwrap(), "/abs/s.json");
    }

    #[test]
    fn paths_are_normalised_and_settings_paths_are_relative_to_the_workspace() {
        assert_eq!(normalize(Path::new("/a/./b/../c/x.json")), "/a/c/x.json");
        assert_eq!(normalize(Path::new("/../x")), "/x");
        assert_eq!(normalize(Path::new("a/../../b")), "../b");
        assert_eq!(resolve_relative("../schemas/s.json", "/repo/config/app.json"), "/repo/schemas/s.json");
        let st = store();
        let user = vec![(vec!["config/*.json".to_string()], "./schemas/app.schema.json".to_string())];
        // Settings: relative to the workspace folder, as in VS Code.
        assert_eq!(associate("/repo/config/app.json", "{}", Format::Json, &user, Some("/repo"), &st).unwrap(), "/repo/schemas/app.schema.json");
        // Without a workspace folder: relative to the file.
        assert_eq!(associate("/repo/config/app.json", "{}", Format::Json, &user, None, &st).unwrap(), "/repo/config/schemas/app.schema.json");
        // `$schema` in the file is always relative to the file.
        assert_eq!(associate("/repo/config/app.json", "{\"$schema\": \"../s.json\"}", Format::Json, &user, Some("/repo"), &st).unwrap(), "/repo/s.json");
    }

    struct MemFiles(Mutex<HashMap<String, (String, i64)>>);
    impl FileSource for MemFiles {
        fn read(&self, path: &str) -> Result<String, String> {
            self.0.lock().unwrap().get(path).map(|(t, _)| t.clone()).ok_or_else(|| format!("{path}: No such file or directory"))
        }
        fn mtime(&self, path: &str) -> Option<i64> {
            self.0.lock().unwrap().get(path).map(|(_, m)| *m)
        }
    }

    #[test]
    fn schemas_on_a_remote_host_are_read_through_the_file_source_including_sibling_refs() {
        let files = Arc::new(MemFiles(Mutex::new(HashMap::from([
            ("/srv/app/app.schema.json".to_string(), (r##"{"type":"object","properties":{"port":{"$ref":"./types.json#/definitions/port"}}}"##.to_string(), 1)),
            ("/srv/app/types.json".to_string(), (r#"{"definitions":{"port":{"type":"integer","minimum":1}}}"#.to_string(), 1)),
        ]))));
        let source: Arc<dyn FileSource> = files.clone();
        let svc = Service::new(None, false);
        let doc = "{\"$schema\": \"./app.schema.json\", \"port\": 0}";
        let a = svc.analyze("/srv/app/app.json", doc, Format::Json, &[], None, Some(source.clone()));
        assert!(a.set.is_some(), "{:?}", a.diags);
        assert_eq!(a.diags.len(), 1, "the sibling $ref was followed through the same source: {:?}", a.diags);
        assert!(a.diags[0].message.contains("minimum") || a.diags[0].message.contains("less than"), "{:?}", a.diags);
        // A change on the host (new mtime) is picked up; the old compiled schema is not reused.
        files.0.lock().unwrap().insert("/srv/app/types.json".to_string(), (r#"{"definitions":{"port":{"type":"string"}}}"#.to_string(), 2));
        files.0.lock().unwrap().insert(
            "/srv/app/app.schema.json".to_string(),
            ((r##"{"type":"object","properties":{"port":{"$ref":"./types.json#/definitions/port"}}}"##).to_string(), 2),
        );
        let b = svc.analyze("/srv/app/app.json", doc, Format::Json, &[], None, Some(source.clone()));
        assert!(b.diags.iter().any(|d| d.message.contains("string")), "{:?}", b.diags);
        // The same path on this machine is not consulted: with no source a missing file is reported.
        let local = svc.analyze("/srv/app/app.json", doc, Format::Json, &[], None, None);
        assert!(local.set.is_none() && local.diags.iter().any(|d| d.message.contains("Could not load the schema")), "{:?}", local.diags);
        // Unreachable file on the host: a warning on the first line, the document is not otherwise judged.
        let gone = svc.analyze("/srv/app/other.json", "{\"$schema\": \"./nope.json\"}", Format::Json, &[], None, Some(source));
        assert!(gone.diags.iter().any(|d| d.severity == 2 && d.message.contains("No such file")), "{:?}", gone.diags);
    }

    #[test]
    fn json_schema_org_meta_schemas_never_warn_offline_and_big_files_say_why_they_are_skipped() {
        let svc = Service::new(None, false);
        let a =
            svc.analyze("/p/my.schema.json", "{\"$schema\": \"http://json-schema.org/draft-07/schema#\", \"type\": \"object\"}", Format::Json, &[], None, None);
        assert!(a.diags.is_empty(), "a schema file is not nagged about its meta-schema: {:?}", a.diags);
        assert!(is_meta_schema("https://json-schema.org/draft/2020-12/schema") && !is_meta_schema("https://example.com/json-schema.org/x"));
        let big = format!("{{\"a\": \"{}\"}}", "x".repeat(MAX_BYTES));
        let d = diagnose(&big, Format::Json, None);
        assert_eq!((d.len(), d[0].severity), (1, 4), "{d:?}");
        assert!(d[0].message.contains("skipped"));
        assert_eq!(guarded(7, || -> i32 { panic!("boom") }), 7, "a panic inside a check is contained");
    }

    #[test]
    fn the_service_analyzes_with_a_local_schema_and_reports_a_missing_one() {
        let dir = std::env::temp_dir().join(format!("codeyard-schema-ide-şablon boşluk#{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("types.json"), r#"{"$defs":{"port":{"type":"integer"}}}"#).unwrap();
        std::fs::write(dir.join("app.schema.json"), r##"{"type":"object","properties":{"port":{"$ref":"./types.json#/$defs/port"}}}"##).unwrap();
        let svc = Service::new(None, false);
        let file = dir.join("app.json").to_string_lossy().into_owned();
        let a = svc.analyze(&file, "{\"$schema\": \"./app.schema.json\", \"port\": \"x\"}", Format::Json, &[], None, None);
        assert!(a.set.is_some(), "{:?}", a.diags);
        assert_eq!(a.diags.len(), 1, "{:?}", a.diags);
        assert!(a.diags[0].message.contains("integer"));
        let missing = svc.analyze(&file, "{\"$schema\": \"./nope.json\"}", Format::Json, &[], None, None);
        assert!(missing.set.is_none());
        assert!(missing.diags.iter().any(|d| d.severity == 2 && d.message.contains("Could not load the schema")), "{:?}", missing.diags);
        // Editing the schema file takes effect (cache keyed by mtime).
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(dir.join("app.schema.json"), r#"{"type":"object","properties":{"port":{"type":"string"}}}"#).unwrap();
        let again = svc.analyze(&file, "{\"$schema\": \"./app.schema.json\", \"port\": \"x\"}", Format::Json, &[], None, None);
        assert!(again.diags.is_empty(), "{:?}", again.diags);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn formats_are_recognized_by_file_name() {
        assert_eq!(format_of("/a/package.json"), Some(Format::Json));
        assert_eq!(format_of("/a/.eslintrc"), Some(Format::Json));
        assert_eq!(format_of("/a/ci.YML"), Some(Format::Yaml));
        assert_eq!(format_of("/a/main.rs"), None);
    }
}
