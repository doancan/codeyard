//! Changing a `Cargo.toml` without disturbing it: version updates replace exactly the requirement's text, adding and removing
//! go through `toml_edit`, which keeps comments, order and spacing. The result is a list of edits for the editor's buffer (one
//! undo step) or a whole new text for a file on disk.

use std::ops::Range;

use toml_edit::{DocumentMut, Item, Table, Value};

use super::hints::Lookups;
use super::index::CrateInfo;
use super::manifest::{Dep, Kind, Manifest, Source};
use super::registry::Lookup;
use super::version::{Update, updated_requirement};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
}

/// The new requirement text of one dependency, from what the index says; `None` when it is current or cannot be decided.
pub fn new_requirement(dep: &Dep, info: &CrateInfo, how: Update) -> Option<Edit> {
    let req = dep.req.as_ref().filter(|_| dep.source == Source::CratesIo)?;
    let text = updated_requirement(&req.text, info, how)?;
    Some(Edit { range: req.span.clone(), text })
}

/// Update every dependency (or only `only`) the lookups know about.
pub fn update_edits(manifest: &Manifest, lookups: &Lookups, how: Update, only: Option<&Dep>) -> Vec<Edit> {
    manifest
        .deps
        .iter()
        .filter(|d| only.is_none_or(|o| d.key_span == o.key_span))
        .filter_map(|d| match lookups.get(&d.package.to_ascii_lowercase())? {
            Lookup::Found { info, .. } => new_requirement(d, info, how),
            _ => None,
        })
        .collect()
}

/// Where in the table tree a dependency lives.
fn table_path(dep: &Dep) -> Vec<String> {
    section_path(dep.kind, dep.target.as_deref(), dep.in_workspace_table)
}

fn section_path(kind: Kind, target: Option<&str>, workspace: bool) -> Vec<String> {
    let section = match kind {
        Kind::Normal => "dependencies",
        Kind::Dev => "dev-dependencies",
        Kind::Build => "build-dependencies",
    };
    match (target, workspace) {
        (_, true) => vec!["workspace".into(), "dependencies".into()],
        (Some(cfg), _) => vec!["target".into(), cfg.into(), section.into()],
        (None, _) => vec![section.into()],
    }
}

fn table_mut<'a>(doc: &'a mut DocumentMut, path: &[String], create: bool) -> Option<&'a mut Table> {
    let mut table = doc.as_table_mut();
    for key in path {
        if !table.contains_key(key) {
            if !create {
                return None;
            }
            let mut fresh = Table::new();
            fresh.set_implicit(path.last() != Some(key));
            table.insert(key, Item::Table(fresh));
        }
        table = table.get_mut(key)?.as_table_mut()?;
    }
    Some(table)
}

#[derive(Debug, PartialEq, Eq)]
pub enum EditError {
    /// The document is not valid TOML.
    Invalid(String),
    Exists(String),
    Missing(String),
    /// A `dependencies` entry that is not a table.
    Shape(String),
}

/// `cargo add` would write the dependency to the member's own table, or to `[workspace.dependencies]` in a manifest that is only
/// a workspace root.
pub fn add_target(text: &str, kind: Kind) -> Vec<String> {
    if is_virtual_root(text) {
        return vec!["workspace".into(), "dependencies".into()];
    }
    section_path(kind, None, false)
}

/// A manifest that is only a workspace root: no `[package]`, so `cargo add` has no package to add to.
pub fn is_virtual_root(text: &str) -> bool {
    text.parse::<DocumentMut>().is_ok_and(|d| !d.contains_key("package") && d.contains_key("workspace"))
}

/// Add `name = "version"` to the table at `path`, sorted when the table already is.
pub fn add_dependency(text: &str, path: &[String], name: &str, version: &str) -> Result<String, EditError> {
    let mut doc = text.parse::<DocumentMut>().map_err(|e| EditError::Invalid(e.to_string()))?;
    let table = table_mut(&mut doc, path, true).ok_or_else(|| EditError::Shape(path.join(".")))?;
    if table.contains_key(name) {
        return Err(EditError::Exists(name.into()));
    }
    let keys: Vec<String> = table.iter().map(|(k, _)| k.to_string()).collect();
    let was_sorted = keys.windows(2).all(|w| w[0] <= w[1]);
    table.insert(name, Item::Value(Value::from(version)));
    if was_sorted {
        table.sort_values();
    }
    Ok(keep_line_ending(text, doc.to_string()))
}

/// A file that uses `\r\n` throughout stays that way: lines `toml_edit` inserts come with `\n`.
fn keep_line_ending(original: &str, edited: String) -> String {
    let crlf = original.matches("\r\n").count();
    let bare = original.matches('\n').count() - crlf;
    if crlf > 0 && bare == 0 { edited.replace("\r\n", "\n").replace('\n', "\r\n") } else { edited }
}

/// Remove the entry `dep` describes (the whole table for `[dependencies.name]`).
pub fn remove_dependency(text: &str, dep: &Dep) -> Result<String, EditError> {
    let mut doc = text.parse::<DocumentMut>().map_err(|e| EditError::Invalid(e.to_string()))?;
    let table = table_mut(&mut doc, &table_path(dep), false).ok_or_else(|| EditError::Missing(dep.key.clone()))?;
    table.remove(&dep.key).ok_or_else(|| EditError::Missing(dep.key.clone()))?;
    Ok(keep_line_ending(text, doc.to_string()))
}

/// The smallest single replacement that turns `old` into `new` (common head and tail kept), so an editor applies a small
/// change and keeps its cursor and undo history.
pub fn minimal_edit(old: &str, new: &str) -> Option<Edit> {
    if old == new {
        return None;
    }
    let head = old.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
    let head = (0..=head).rev().find(|&i| old.is_char_boundary(i) && new.is_char_boundary(i))?;
    let tail = old.as_bytes()[head..].iter().rev().zip(new.as_bytes()[head..].iter().rev()).take_while(|(a, b)| a == b).count();
    let tail = (0..=tail).rev().find(|&i| old.is_char_boundary(old.len() - i) && new.is_char_boundary(new.len() - i))?;
    Some(Edit { range: head..old.len() - tail, text: new[head..new.len() - tail].to_string() })
}

/// Apply edits (non-overlapping, any order) to `text`.
#[cfg(test)]
pub fn apply(text: &str, edits: &[Edit]) -> String {
    let mut sorted: Vec<&Edit> = edits.iter().collect();
    sorted.sort_by_key(|e| std::cmp::Reverse(e.range.start));
    let mut out = text.to_string();
    for e in sorted {
        out.replace_range(e.range.clone(), &e.text);
    }
    out
}
