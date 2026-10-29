//! Workspace files (`*.code-workspace`): a named list of folders with shared `settings`, `tasks` and `launch` sections.
//!
//! The document keeps the exact text it was read from. Reading builds a model (folders with their display names, how
//! each one was written and where it resolves); changing it edits only the spans that change, so comments, unknown
//! fields, sections this editor does not use and the file's layout stay as they were. Every edit is parsed again and
//! checked before anyone may write it: a document that cannot be read safely is never replaced.
//!
//! Parsing, path resolution and editing live here and have no side effects; reading and writing go through the
//! workbench's host, so the same rules apply on this computer and over SSH.

use serde_json::{Map, Value};

use crate::settings::{Seg, array_items, members, value_range};

/// Version of the model below. A change in what is read or written bumps it.
pub const MODEL_VERSION: u32 = 1;
/// Larger files are refused: a workspace file is a few kilobytes.
pub const MAX_BYTES: usize = 4 << 20;
/// More entries than this in `folders` are refused.
pub const MAX_FOLDERS: usize = 1024;

/// How a folder is written in the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Path(String),
    Uri(String),
}

/// Where a folder entry leads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Absolute, normalized path on the host that holds the workspace file.
    Path(String),
    /// Not opened: another URI scheme, a network share the user did not choose, or a malformed path. It stays in the file.
    Unsupported,
}

/// One entry of `folders`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folder {
    /// Position in the file's `folders` list.
    pub item: usize,
    /// `name`: shown instead of the folder's own name.
    pub name: Option<String>,
    pub source: Source,
    pub target: Target,
}

impl Folder {
    pub fn path(&self) -> Option<&str> {
        match &self.target {
            Target::Path(p) => Some(p),
            Target::Unsupported => None,
        }
    }

    /// Display name: the entry's `name`, else the folder's own name, else what the file says.
    pub fn label(&self) -> String {
        if let Some(n) = self.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            return n.to_string();
        }
        match (&self.target, &self.source) {
            (Target::Path(p), _) => crate::ui::file_name(p).to_string(),
            (Target::Unsupported, Source::Path(s) | Source::Uri(s)) => s.clone(),
        }
    }
}

/// A change to the folder list: keep an existing entry (by `Folder::item`) or add a folder by its absolute path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edit {
    Keep(usize),
    Add { path: String, name: Option<String> },
}

/// A parsed workspace file.
#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    #[cfg_attr(not(test), allow(dead_code))]
    version: u32,
    text: String,
    /// Folder of the file, normalized: relative folder paths resolve against it.
    dir: String,
    folders: Vec<Folder>,
    /// Entries in `folders`, including ones that are not folders this editor understands.
    items: usize,
    settings: Option<Map<String, Value>>,
    tasks: Option<Value>,
    launch: Option<Value>,
}

impl Document {
    /// An empty workspace file for `dir`.
    pub fn empty(dir: &str) -> Document {
        // A fixed, valid text: parsing it cannot fail.
        Document::parse("{\n  \"folders\": []\n}\n", dir).unwrap_or_else(|_| Document {
            version: MODEL_VERSION,
            text: "{\n  \"folders\": []\n}\n".into(),
            dir: base(dir),
            folders: Vec::new(),
            items: 0,
            settings: None,
            tasks: None,
            launch: None,
        })
    }

    /// Read `text`, a workspace file kept in `dir`. `Err` is a short reason for "Workspace file could not be read: {}".
    pub fn parse(text: &str, dir: &str) -> Result<Document, String> {
        if text.len() > MAX_BYTES {
            return Err(crate::i18n::t("the file is too large").into());
        }
        check_terminated(text)?;
        let body = text.strip_prefix('\u{feff}').unwrap_or(text);
        let value: Value = serde_json::from_str(&theme::themes::strip_jsonc(body)).map_err(|e| e.to_string())?;
        let Value::Object(obj) = value else { return Err(crate::i18n::t("expected an object { … }").into()) };
        // The spans edited later must be the ones the parser read: if the two ever disagree, nothing is edited.
        let root = value_range(text, &[]).ok_or_else(unreadable)?;
        if members(text, root.clone()).is_none_or(|m| m.len() < obj.len()) {
            return Err(unreadable());
        }
        let dir = base(dir);
        let (mut folders, mut items) = (Vec::new(), 0);
        match obj.get("folders") {
            None => {}
            Some(Value::Array(list)) => {
                if list.len() > MAX_FOLDERS {
                    return Err(crate::i18n::tf("more than {} folders", &[&MAX_FOLDERS]));
                }
                let spans = value_range(text, &[Seg::Key("folders")]).and_then(|r| array_items(text, r)).ok_or_else(unreadable)?;
                if spans.len() != list.len() {
                    return Err(unreadable());
                }
                items = list.len();
                for (item, entry) in list.iter().enumerate() {
                    let Some(source) = (match (entry.get("path").and_then(Value::as_str), entry.get("uri").and_then(Value::as_str)) {
                        (Some(p), _) => Some(Source::Path(p.to_string())),
                        (None, Some(u)) => Some(Source::Uri(u.to_string())),
                        _ => None,
                    }) else {
                        continue;
                    };
                    let name = entry.get("name").and_then(Value::as_str).map(str::to_string);
                    let target = resolve(&source, &dir);
                    folders.push(Folder { item, name, source, target });
                }
            }
            Some(_) => return Err(crate::i18n::t("“folders” must be a list").into()),
        }
        let settings = obj.get("settings").and_then(Value::as_object).cloned();
        Ok(Document {
            version: MODEL_VERSION,
            text: text.to_string(),
            dir,
            folders,
            items,
            settings,
            tasks: obj.get("tasks").cloned(),
            launch: obj.get("launch").cloned(),
        })
    }

    /// `MODEL_VERSION` of the reader that built this document.
    #[cfg(test)]
    pub fn version(&self) -> u32 {
        self.version
    }

    /// The text to write: exactly what was read, plus the edits made through this model.
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn dir(&self) -> &str {
        &self.dir
    }

    pub fn folders(&self) -> &[Folder] {
        &self.folders
    }

    /// The shared `settings` section as JSON text (a settings layer), if there is one.
    pub fn settings_json(&self) -> Option<String> {
        self.settings.as_ref().map(|s| Value::Object(s.clone()).to_string())
    }

    /// The shared `tasks` section as JSON text (a task file).
    pub fn tasks_json(&self) -> Option<String> {
        self.tasks.as_ref().map(Value::to_string)
    }

    /// The shared `launch` section as JSON text (a launch file).
    pub fn launch_json(&self) -> Option<String> {
        self.launch.as_ref().map(Value::to_string)
    }

    /// Whether the file can make programs run (tasks or debug configurations).
    pub fn defines_commands(&self) -> bool {
        self.tasks.is_some() || self.launch.is_some()
    }

    /// The document with the folder list replaced by `edits`, in that order. Kept entries keep their own text (comments
    /// and unknown fields included); entries this editor does not understand stay at the end. Without a change the
    /// text is not touched at all.
    pub fn with_folders(&self, edits: &[Edit]) -> Result<Document, String> {
        let unchanged = edits.len() == self.folders.len() && edits.iter().zip(&self.folders).all(|(e, f)| *e == Edit::Keep(f.item));
        if unchanged {
            return Ok(self.clone());
        }
        let nl = if self.text.contains("\r\n") { "\r\n" } else { "\n" };
        let spans = value_range(&self.text, &[Seg::Key("folders")]).and_then(|r| Some((r.clone(), array_items(&self.text, r)?)));
        let (comments, tail) = match &spans {
            Some((range, items)) => item_comments(&self.text, range.clone(), items),
            None => (Vec::new(), String::new()),
        };
        // (comments before, the entry, comment after on the same line)
        let kept = |i: usize| -> Option<Part> {
            let span = spans.as_ref()?.1.get(i)?;
            let (before, after) = comments.get(i).cloned().unwrap_or_default();
            Some(Part { before, body: self.text[span.clone()].to_string(), after })
        };
        let mut parts: Vec<Part> = Vec::new();
        for e in edits {
            match e {
                Edit::Keep(item) => {
                    if !self.folders.iter().any(|f| f.item == *item) {
                        return Err(unreadable());
                    }
                    parts.push(kept(*item).ok_or_else(unreadable)?);
                }
                Edit::Add { path, name } => {
                    // Relative when the folder shares more than the root with the file (a project moved as a whole keeps working).
                    let abs = base(path);
                    let shared = split_root(&self.dir).1.split('/').next().filter(|s| !s.is_empty()) == split_root(&abs).1.split('/').next();
                    let written = relative_to(&self.dir, &abs).filter(|_| shared).unwrap_or(abs);
                    let mut o = Map::new();
                    if let Some(n) = name {
                        o.insert("name".into(), Value::String(n.clone()));
                    }
                    o.insert("path".into(), Value::String(written));
                    let body: Vec<String> = o.iter().map(|(k, v)| format!("{}: {}", Value::String(k.clone()), v)).collect();
                    parts.push(Part { before: String::new(), body: format!("{{ {} }}", body.join(", ")), after: String::new() });
                }
            }
        }
        if let Some((_, items)) = &spans {
            for i in 0..items.len() {
                if !self.folders.iter().any(|f| f.item == i) {
                    parts.push(kept(i).ok_or_else(unreadable)?);
                }
            }
        }
        let list = |outer: &str, inner: &str| -> String {
            if parts.is_empty() && tail.is_empty() {
                return "[]".to_string();
            }
            let mut out = String::from("[");
            for (k, p) in parts.iter().enumerate() {
                out.push_str(nl);
                out.push_str(inner);
                if !p.before.is_empty() {
                    out.push_str(&p.before);
                    // A line comment or several lines end the line; a short block comment stays in front of the entry.
                    if p.before.contains("//") || p.before.contains('\n') {
                        out.push_str(nl);
                        out.push_str(inner);
                    } else {
                        out.push(' ');
                    }
                }
                out.push_str(&p.body);
                if k + 1 < parts.len() {
                    out.push(',');
                }
                if !p.after.is_empty() {
                    out.push(' ');
                    out.push_str(&p.after);
                }
            }
            if !tail.is_empty() {
                out.push_str(nl);
                out.push_str(inner);
                out.push_str(&tail);
            }
            out.push_str(nl);
            out.push_str(outer);
            out.push(']');
            out
        };
        let text = match &spans {
            Some((range, items)) => {
                let outer = line_indent(&self.text, range.start);
                let inner = items.first().map(|s| line_indent(&self.text, s.start)).filter(|i| i.len() > outer.len()).unwrap_or_else(|| format!("{outer}  "));
                format!("{}{}{}", &self.text[..range.start], list(&outer, &inner), &self.text[range.end..])
            }
            None => {
                let root = value_range(&self.text, &[]).ok_or_else(unreadable)?;
                let has_members = members(&self.text, root.clone()).is_some_and(|m| !m.is_empty());
                let comma = if has_members { "," } else { "" };
                format!("{}{nl}  \"folders\": {}{comma}{}", &self.text[..=root.start], list("  ", "    "), &self.text[root.start + 1..])
            }
        };
        let next = Document::parse(&text, &self.dir)?;
        if next.folders.len() != edits.len() || next.items != parts.len() {
            return Err(unreadable());
        }
        Ok(next)
    }

    /// The same document kept in `new_dir` instead: relative folder paths are rewritten so they still lead to the same
    /// folders; absolute paths, URIs, comments and everything else stay as written.
    pub fn rebased(&self, new_dir: &str) -> Result<Document, String> {
        let new_base = base(new_dir);
        let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
        for f in &self.folders {
            let (Source::Path(raw), Target::Path(abs)) = (&f.source, &f.target) else { continue };
            if crate::ui::is_absolute_path(raw) {
                continue;
            }
            let mut rel = relative_to(&new_base, abs).unwrap_or_else(|| abs.clone());
            if raw.contains('\\') && !raw.contains('/') && !crate::ui::is_absolute_path(&rel) {
                rel = rel.replace('/', "\\");
            }
            if rel == *raw {
                continue;
            }
            let span = value_range(&self.text, &[Seg::Key("folders"), Seg::Index(f.item), Seg::Key("path")]).ok_or_else(unreadable)?;
            edits.push((span, Value::String(rel).to_string()));
        }
        let mut text = self.text.clone();
        edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
        for (range, value) in edits {
            text.replace_range(range, &value);
        }
        let next = Document::parse(&text, &new_base)?;
        let same = next.folders.len() == self.folders.len() && next.folders.iter().zip(&self.folders).all(|(a, b)| a.target == b.target && a.name == b.name);
        if !same {
            return Err(unreadable());
        }
        Ok(next)
    }
}

/// One entry of a rewritten folder list with the comments that belong to it.
struct Part {
    /// Comments between the previous entry and this one.
    before: String,
    body: String,
    /// A comment after the entry (and its comma) on the same line.
    after: String,
}

/// (comments before, comment after on the same line) for each element of the array at `range`, and the comments after
/// the last one. Commas are not part of either.
fn item_comments(text: &str, range: std::ops::Range<usize>, items: &[std::ops::Range<usize>]) -> (Vec<(String, String)>, String) {
    let b = text.as_bytes();
    let blank = |i: usize| matches!(b.get(i), Some(b' ' | b'\t'));
    let clean = |s: &str| s.trim().trim_start_matches(',').trim().to_string();
    let mut cursor = range.start + 1;
    let mut out = Vec::with_capacity(items.len());
    for s in items {
        let before = clean(text.get(cursor..s.start).unwrap_or(""));
        let mut j = s.end;
        while blank(j) {
            j += 1;
        }
        if b.get(j) == Some(&b',') {
            j += 1;
            while blank(j) {
                j += 1;
            }
        }
        let after_start = j;
        let after_end = if b.get(j..j + 2) == Some(b"//") {
            text[j..].find('\n').map_or(text.len(), |n| j + n)
        } else if b.get(j..j + 2) == Some(b"/*") {
            match text[j + 2..].find("*/") {
                Some(n) if !text[j..j + 2 + n].contains('\n') => j + 2 + n + 2,
                _ => j,
            }
        } else {
            j
        };
        let after = text[after_start..after_end].trim_end().to_string();
        // The comma left between here and the next comments is dropped by `clean`.
        cursor = if after.is_empty() { s.end } else { after_end };
        out.push((before, after));
    }
    let tail = clean(text.get(cursor..range.end.saturating_sub(1)).unwrap_or(""));
    (out, tail)
}

fn unreadable() -> String {
    crate::i18n::t("its structure could not be read safely").into()
}

/// `strip_jsonc` silently drops an unterminated `/* …`: the rest of the file would vanish on the next save.
fn check_terminated(text: &str) -> Result<(), String> {
    let b = text.as_bytes();
    let (mut i, mut in_str) = (0, false);
    while i < b.len() {
        match (in_str, b[i]) {
            (true, b'\\') => i += 1,
            (_, b'"') => in_str = !in_str,
            (false, b'/') if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            (false, b'/') if b.get(i + 1) == Some(&b'*') => match text[i + 2..].find("*/") {
                Some(end) => i += end + 3,
                None => return Err(crate::i18n::t("a comment is not closed").into()),
            },
            _ => {}
        }
        i += 1;
    }
    if in_str { Err(crate::i18n::t("a string is not closed").into()) } else { Ok(()) }
}

/// Leading whitespace of the line that holds byte `at`.
fn line_indent(text: &str, at: usize) -> String {
    let line = text[..at].rfind('\n').map_or(0, |i| i + 1);
    text[line..at].chars().take_while(|c| *c == ' ' || *c == '\t').collect()
}

/// The document's folder as a normalized absolute path with `/` separators.
fn base(dir: &str) -> String {
    let dir = if windows_path(dir) { dir.replace('\\', "/") } else { dir.to_string() };
    normalize(&dir)
}

/// Where a folder entry leads. A network share (`//server/share`) is only taken when the workspace file itself is on that
/// server: the file may come from a cloned repository, and looking at another share makes Windows offer the user's
/// credentials to it. Nothing here touches the disk.
fn resolve(source: &Source, base_dir: &str) -> Target {
    let raw = match source {
        Source::Path(p) => p.clone(),
        Source::Uri(u) => match schema::file_uri::to_path(u) {
            Some(p) => p,
            None => return Target::Unsupported,
        },
    };
    if raw.contains('\0') || raw.trim().is_empty() {
        return Target::Unsupported;
    }
    let raw = if windows_path(base_dir) || windows_path(&raw) { raw.replace('\\', "/") } else { raw };
    let abs = normalize(&crate::ui::join(base_dir, &raw));
    if let Some(host) = unc_host(&abs)
        && !unc_host(base_dir).is_some_and(|own| own.eq_ignore_ascii_case(host))
    {
        return Target::Unsupported;
    }
    Target::Path(abs)
}

/// `target` relative to `dir` (both normalized), or `None` when they have different roots (drive, share, host).
pub fn relative_to(dir: &str, target: &str) -> Option<String> {
    let (dir_root, dir_rest) = split_root(dir);
    let (target_root, target_rest) = split_root(target);
    let windows = dir_root != "/";
    let same = |a: &str, b: &str| if windows { a.eq_ignore_ascii_case(b) } else { a == b };
    if !same(&dir_root, &target_root) {
        return None;
    }
    let a: Vec<&str> = dir_rest.split('/').filter(|s| !s.is_empty()).collect();
    let b: Vec<&str> = target_rest.split('/').filter(|s| !s.is_empty()).collect();
    let common = a.iter().zip(&b).take_while(|(x, y)| same(x, y)).count();
    let mut parts: Vec<&str> = std::iter::repeat_n("..", a.len() - common).collect();
    parts.extend(&b[common..]);
    Some(if parts.is_empty() { ".".to_string() } else { parts.join("/") })
}

/// (root, rest) of a normalized path: `C:/`, `//server/share` or `/`.
fn split_root(path: &str) -> (String, &str) {
    // Bytes, not `&str` slices: a path may start with a letter followed by a multi-byte character.
    if path.len() >= 3 && path.as_bytes()[0].is_ascii_alphabetic() && path.as_bytes()[1..3] == *b":/" {
        return (path[..3].to_string(), &path[3..]);
    }
    if let Some(rest) = path.strip_prefix("//") {
        let mut it = rest.splitn(3, '/');
        let (server, share) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
        let tail = it.next().unwrap_or("");
        return (format!("//{server}/{share}"), tail);
    }
    ("/".to_string(), path.strip_prefix('/').unwrap_or(path))
}

/// `folders[].path` or `folders[].uri` (file://) → absolute, normalized folder paths that `exists` accepts; duplicates
/// dropped, order kept. Unreadable text gives none.
#[cfg(test)]
pub fn folders(text: &str, base_dir: &str, exists: impl Fn(&str) -> bool) -> Vec<String> {
    let Ok(doc) = Document::parse(text, base_dir) else { return Vec::new() };
    let mut out: Vec<String> = Vec::new();
    for f in doc.folders {
        if let Target::Path(abs) = f.target
            && !out.contains(&abs)
            && exists(&abs)
        {
            out.push(abs);
        }
    }
    out
}

/// Normalize an absolute path lexically, including drive and UNC roots.
pub(crate) fn normalize(path: &str) -> String {
    let path = if let Some(rest) = path.strip_prefix("//?/UNC/") { format!("//{rest}") } else { path.strip_prefix("//?/").unwrap_or(path).to_string() };
    let drive = path.len() >= 3 && path.as_bytes()[0].is_ascii_alphabetic() && path.as_bytes()[1..3] == *b":/";
    let unc = path.starts_with("//");
    let (prefix, rest, floor) = if drive {
        (format!("{}/", &path[..2]), &path[3..], 0)
    } else if unc {
        ("//".to_string(), &path[2..], 2)
    } else {
        ("/".to_string(), path.as_str(), 0)
    };
    let mut parts: Vec<&str> = Vec::new();
    for p in rest.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                if parts.len() > floor {
                    parts.pop();
                }
            }
            p => parts.push(p),
        }
    }
    format!("{prefix}{}", parts.join("/"))
}

/// The server of a normalized `//server/share/...` path.
fn unc_host(path: &str) -> Option<&str> {
    path.strip_prefix("//").map(|rest| rest.split('/').next().unwrap_or(""))
}

fn windows_path(path: &str) -> bool {
    path.starts_with(['\\']) || path.starts_with("//") || path.len() >= 2 && path.as_bytes()[0].is_ascii_alphabetic() && path.as_bytes()[1] == b':'
}

pub fn is_workspace_file(path: &str) -> bool {
    path.ends_with(".code-workspace")
}

#[cfg(test)]
#[path = "workspace_file_tests.rs"]
mod tests;
