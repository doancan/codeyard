//! A language server's `WorkspaceEdit` as a list of operations, in the order the server gave them. Nothing here touches a
//! file: `plan` turns the operations into a preview, `apply` writes it.
//!
//! Both shapes are read: `documentChanges` (text edits with the version of the document they were computed for, and the
//! create / rename / delete operations between them; it wins when present, as the protocol says) and the older `changes`
//! map. Everything a server sends is untrusted, so a malformed edit does not become a guess: the text edit is kept with the
//! reason it is unusable and the plan refuses that file.

use serde_json::Value;

/// A position is (line, UTF-16 code unit in the line), the protocol's default.
pub type Position = (u32, u32);

/// One text edit as sent: a range and its replacement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub start: Position,
    pub end: Position,
    pub new_text: String,
}

/// The text edits for one document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextOp {
    pub path: String,
    /// The version of the document the server computed against (`null` when it does not track one).
    pub version: Option<i64>,
    pub edits: Vec<TextEdit>,
    /// Why this entry cannot be used (an edit without a range, say). The plan refuses the file with this text.
    pub malformed: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Text(TextOp),
    Create { path: String, overwrite: bool, ignore_if_exists: bool },
    Rename { from: String, to: String, overwrite: bool, ignore_if_exists: bool },
    Delete { path: String, recursive: bool, ignore_if_not_exists: bool },
}

impl Op {
    #[cfg(test)]
    pub fn is_resource(&self) -> bool {
        !matches!(self, Op::Text(_))
    }
}

/// The operations of an edit and the entries that were skipped.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    pub ops: Vec<Op>,
    /// Entries that are not usable at all (a URI that is not a file, an operation of an unknown kind), already translated.
    pub skipped: Vec<String>,
}

impl Parsed {
    #[cfg(test)]
    pub fn has_resource_ops(&self) -> bool {
        self.ops.iter().any(Op::is_resource)
    }

    pub fn edit_count(&self) -> usize {
        self.ops.iter().map(|op| if let Op::Text(t) = op { t.edits.len() } else { 1 }).sum()
    }
}

pub fn parse(edit: &Value) -> Parsed {
    let mut out = Parsed::default();
    if let Some(changes) = edit["documentChanges"].as_array() {
        for entry in changes {
            parse_entry(entry, &mut out);
        }
    } else if let Some(changes) = edit["changes"].as_object() {
        for (uri, edits) in changes {
            match crate::lsp::uri_to_path(uri) {
                Some(path) => out.ops.push(Op::Text(text_op(path, None, edits))),
                None => out.skipped.push(crate::i18n::tf("Not a file: {}", &[&uri])),
            }
        }
    }
    out
}

fn parse_entry(entry: &Value, out: &mut Parsed) {
    let path_of = |uri: &Value, out: &mut Parsed| {
        let uri = uri.as_str().unwrap_or_default();
        let path = crate::lsp::uri_to_path(uri);
        if path.is_none() {
            out.skipped.push(crate::i18n::tf("Not a file: {}", &[&uri]));
        }
        path
    };
    let flag = |v: &Value, name: &str| v["options"][name].as_bool().unwrap_or(false);
    match entry["kind"].as_str() {
        None => {
            if let Some(path) = path_of(&entry["textDocument"]["uri"], out) {
                let version = entry["textDocument"]["version"].as_i64();
                out.ops.push(Op::Text(text_op(path, version, &entry["edits"])));
            }
        }
        Some("create") => {
            if let Some(path) = path_of(&entry["uri"], out) {
                out.ops.push(Op::Create { path, overwrite: flag(entry, "overwrite"), ignore_if_exists: flag(entry, "ignoreIfExists") });
            }
        }
        Some("rename") => {
            let (from, to) = (path_of(&entry["oldUri"], out), path_of(&entry["newUri"], out));
            if let (Some(from), Some(to)) = (from, to) {
                out.ops.push(Op::Rename { from, to, overwrite: flag(entry, "overwrite"), ignore_if_exists: flag(entry, "ignoreIfExists") });
            }
        }
        Some("delete") => {
            if let Some(path) = path_of(&entry["uri"], out) {
                out.ops.push(Op::Delete { path, recursive: flag(entry, "recursive"), ignore_if_not_exists: flag(entry, "ignoreIfNotExists") });
            }
        }
        Some(other) => out.skipped.push(crate::i18n::tf("Unknown file operation “{}”", &[&other])),
    }
}

fn text_op(path: String, version: Option<i64>, edits: &Value) -> TextOp {
    let mut op = TextOp { path, version, edits: Vec::new(), malformed: None };
    let Some(list) = edits.as_array() else {
        op.malformed = Some(crate::i18n::t("The edit list is missing.").into());
        return op;
    };
    for (i, e) in list.iter().enumerate() {
        match text_edit(e) {
            Some(edit) => op.edits.push(edit),
            None => {
                op.malformed = Some(crate::i18n::tf("Edit {} of the language server is malformed.", &[&(i + 1)]));
                op.edits.clear();
                break;
            }
        }
    }
    op
}

fn text_edit(e: &Value) -> Option<TextEdit> {
    // A column past the end of the line means the end of the line (servers send very large numbers for it): it is clamped here
    // so it fits, and `plan` clamps it to the line.
    let position = |v: &Value| Some((u32::try_from(v["line"].as_u64()?).ok()?, v["character"].as_u64()?.min(u64::from(u32::MAX)) as u32));
    Some(TextEdit { start: position(&e["range"]["start"])?, end: position(&e["range"]["end"])?, new_text: e["newText"].as_str()?.to_string() })
}

#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;
