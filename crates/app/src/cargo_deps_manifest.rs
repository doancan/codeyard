//! The dependency entries of a `Cargo.toml`, with the byte ranges needed to show hints, answer hovers and edit the text.
//! Parsed with `toml_edit` (spans, no regular expressions); an unfinished document is an error, and the caller keeps what it
//! showed before.

use std::ops::Range;

use toml_edit::{Document, Item, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Normal,
    Dev,
    Build,
}

/// Where a dependency comes from. Only `CratesIo` is looked up; the rest is skipped without a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    CratesIo,
    /// Another registry (`registry = "…"` or `registry-index = "…"`).
    Registry(String),
    Path,
    Git,
    /// `workspace = true`: the requirement lives in `[workspace.dependencies]`.
    Workspace,
}

/// The version requirement as written: its text without quotes, and where that text is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Requirement {
    pub text: String,
    pub span: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeatureName {
    pub name: String,
    /// The string's content, without quotes.
    pub span: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dep {
    /// The key as written (the local name when the crate is renamed).
    pub key: String,
    /// The crate on the registry: `package = "…"` or the key.
    pub package: String,
    pub kind: Kind,
    /// The `cfg(…)` or triple of a `[target.….dependencies]` table.
    pub target: Option<String>,
    /// In `[workspace.dependencies]`.
    pub in_workspace_table: bool,
    pub source: Source,
    pub req: Option<Requirement>,
    pub features: Vec<FeatureName>,
    /// The `features` array including brackets, when there is one.
    pub features_span: Option<Range<usize>>,
    pub key_span: Range<usize>,
    /// From the key to the end of the entry (the whole `[dependencies.name]` table for that form).
    pub entry_span: Range<usize>,
    /// The line the hint is shown on: the version's, else the key's (0-based).
    pub line: usize,
}

impl Dep {
    pub fn is_registry_lookup(&self) -> bool {
        self.source == Source::CratesIo && crate::cargo_project::deps::valid_crate_name(&self.package)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub deps: Vec<Dep>,
    pub lines: usize,
}

impl Manifest {
    /// The dependency whose entry holds `offset`.
    pub fn dep_at(&self, offset: usize) -> Option<&Dep> {
        // The innermost entry wins (a table entry never nests in another, but spans of dotted keys can touch).
        self.deps.iter().filter(|d| d.entry_span.contains(&offset) || d.entry_span.end == offset).min_by_key(|d| d.entry_span.len())
    }
}

/// Line starts of `text`, for turning offsets into 0-based lines.
fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect()
}

fn line_of(starts: &[usize], offset: usize) -> usize {
    starts.partition_point(|&s| s <= offset).saturating_sub(1)
}

/// The text of a one-line string value and the range of its content; `None` for multi-line strings.
fn string_content(text: &str, value: &Value) -> Option<(String, Range<usize>)> {
    let s = value.as_str()?;
    let span = value.span()?;
    let raw = text.get(span.clone())?;
    let quote = raw.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    if raw.len() < 2 || raw.starts_with(&format!("{quote}{quote}{quote}")) {
        return None;
    }
    Some((s.to_string(), span.start + 1..span.end - 1))
}

/// Where an entry's value is: its own span (a table's may cover only its header) widened by what it holds. A dotted key's
/// table has no span of its own at all.
fn item_span(item: &Item) -> Option<Range<usize>> {
    let mut spans: Vec<Range<usize>> = item.span().into_iter().collect();
    if let Some(table) = item.as_table_like() {
        spans.extend(table.iter().filter_map(|(_, v)| item_span(v)));
    }
    Some(spans.iter().map(|s| s.start).min()?..spans.iter().map(|s| s.end).max()?)
}

struct Walk<'a> {
    text: &'a str,
    starts: Vec<usize>,
    deps: Vec<Dep>,
}

impl Walk<'_> {
    fn table(&mut self, item: &Item, kind: Kind, target: Option<&str>, in_workspace_table: bool) {
        let Some(table) = item.as_table_like() else { return };
        for (name, entry) in table.iter() {
            let Some((key, _)) = table.get_key_value(name) else { continue };
            let Some(key_span) = key.span() else { continue };
            if let Some(dep) = self.dep(name, key_span, entry, kind, target, in_workspace_table) {
                self.deps.push(dep);
            }
        }
    }

    fn dep(&self, key: &str, key_span: Range<usize>, item: &Item, kind: Kind, target: Option<&str>, in_workspace_table: bool) -> Option<Dep> {
        let field = |name: &str| item.get(name);
        let str_field = |name: &str| field(name).and_then(Item::as_str);
        let mut req = None;
        if let Some(value) = item.as_value().filter(|v| v.is_str()) {
            req = string_content(self.text, value).map(|(text, span)| Requirement { text, span });
        } else if let Some(value) = field("version").and_then(Item::as_value) {
            req = string_content(self.text, value).map(|(text, span)| Requirement { text, span });
        }
        let source = if item.as_value().is_some_and(Value::is_str) {
            Source::CratesIo
        } else if field("workspace").and_then(Item::as_bool) == Some(true) {
            Source::Workspace
        } else if field("path").is_some() {
            Source::Path
        } else if field("git").is_some() {
            Source::Git
        } else if let Some(name) = str_field("registry").or_else(|| str_field("registry-index")) {
            Source::Registry(name.to_string())
        } else {
            Source::CratesIo
        };
        let (mut features, mut features_span) = (Vec::new(), None);
        if let Some(array) = field("features").and_then(Item::as_array) {
            features_span = array.span();
            for v in array.iter() {
                if let Some((name, span)) = string_content(self.text, v) {
                    features.push(FeatureName { name, span });
                }
            }
        }
        let entry = item_span(item).unwrap_or(key_span.clone());
        let entry_span = key_span.start..entry.end.max(key_span.end);
        let line = line_of(&self.starts, req.as_ref().map_or(key_span.start, |r| r.span.start));
        Some(Dep {
            key: key.to_string(),
            package: str_field("package").unwrap_or(key).to_string(),
            kind,
            target: target.map(str::to_string),
            in_workspace_table,
            source,
            req,
            features,
            features_span,
            key_span,
            entry_span,
            line,
        })
    }
}

const DEP_TABLES: [(&str, Kind); 5] = [
    ("dependencies", Kind::Normal),
    ("dev-dependencies", Kind::Dev),
    ("build-dependencies", Kind::Build),
    // The old spellings Cargo still reads.
    ("dev_dependencies", Kind::Dev),
    ("build_dependencies", Kind::Build),
];

/// Every dependency of the manifest, in file order.
pub fn parse(text: &str) -> Result<Manifest, String> {
    let doc = Document::parse(text).map_err(|e| e.to_string())?;
    let mut walk = Walk { text, starts: line_starts(text), deps: Vec::new() };
    let root = doc.as_table();
    for (name, kind) in DEP_TABLES.iter() {
        if let Some(t) = root.get(name) {
            walk.table(t, *kind, None, false);
        }
    }
    if let Some(t) = root.get("workspace").and_then(|w| w.get("dependencies")) {
        walk.table(t, Kind::Normal, None, true);
    }
    if let Some(targets) = root.get("target").and_then(Item::as_table_like) {
        for (cfg, entry) in targets.iter() {
            for (name, kind) in DEP_TABLES.iter() {
                if let Some(t) = entry.get(name) {
                    walk.table(t, *kind, Some(cfg), false);
                }
            }
        }
    }
    walk.deps.sort_by_key(|d| d.key_span.start);
    Ok(Manifest { deps: walk.deps, lines: walk.starts.len() })
}
