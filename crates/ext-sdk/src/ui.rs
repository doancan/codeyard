//! Build a view's tree (docs/extensions/views.md) in the form Codeyard draws (docs/extensions/reference/ui.md):
//!
//! ```ignore
//! use codeyard_extension::ui;
//!
//! ctx.view("main", async |ctx| {
//!     Ok(ui::column().gap(2).child(ui::heading("Clicks", 2)).child(ui::text("3")).child(ui::button("add", "Add one")))
//! });
//! ```
//!
//! Each function makes one element with the properties it needs; the optional ones are methods. Interactive elements
//! (button, text input, checkbox, select, list, table, tree, link) take their key first, since Codeyard needs one;
//! give any other element a key with `.key(…)`, as every row of a list or table needs. Containers (column, row, list,
//! table, tree, empty-state) take children with `.child(…)` and `.children(…)`. A tree serializes to
//! `{"key": …, "element": {"<kind>": {…}}, "children": [ … ]}`; `codeyard-extension-test` checks it with Codeyard's own
//! rules when a test calls `render`.

use std::marker::PhantomData;

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use serde_json::{Map, Value, json};

use crate::types::Tone;

/// How many rows of a list, table or tree the user can select.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Selection {
    #[default]
    None,
    One,
    Many,
}

impl Selection {
    fn name(self) -> &'static str {
        match self {
            Selection::None => "none",
            Selection::One => "one",
            Selection::Many => "many",
        }
    }
}

fn tone(t: Tone) -> &'static str {
    match t {
        Tone::Neutral => "neutral",
        Tone::Accent => "accent",
        Tone::Success => "success",
        Tone::Warning => "warning",
        Tone::Danger => "danger",
        Tone::Muted => "muted",
    }
}

/// One element of a view with its key and children, whatever its kind.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    key: Option<String>,
    kind: &'static str,
    props: Map<String, Value>,
    children: Vec<Node>,
}

impl Node {
    /// The tree as JSON, as Codeyard reads it.
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).expect("a view always serializes")
    }
}

impl Serialize for Node {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        struct Element<'a>(&'a Node);
        impl Serialize for Element<'_> {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                if self.0.kind == "divider" {
                    return s.serialize_str("divider");
                }
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry(self.0.kind, &self.0.props)?;
                m.end()
            }
        }
        let mut m = s.serialize_map(None)?;
        if let Some(k) = &self.key {
            m.serialize_entry("key", k)?;
        }
        m.serialize_entry("element", &Element(self))?;
        if !self.children.is_empty() {
            m.serialize_entry("children", &self.children)?;
        }
        m.end()
    }
}

/// An element of kind `K` (one of the types in [`kind`]), with the methods for its optional properties.
#[derive(Clone, Debug, PartialEq)]
pub struct El<K> {
    node: Node,
    _kind: PhantomData<K>,
}

impl<K> El<K> {
    fn new(kind: &'static str, props: Value) -> El<K> {
        let Value::Object(props) = props else { unreachable!("properties are an object") };
        El { node: Node { key: None, kind, props, children: Vec::new() }, _kind: PhantomData }
    }

    fn set(mut self, name: &str, value: impl Into<Value>) -> Self {
        self.node.props.insert(name.into(), value.into());
        self
    }

    /// Names the element among its siblings. Keys stay the same across renders, so Codeyard keeps what the user typed,
    /// ticked, selected and expanded; every row of a list, table or tree needs one.
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.node.key = Some(key.into());
        self
    }
}

impl<K> From<El<K>> for Node {
    fn from(e: El<K>) -> Node {
        e.node
    }
}

impl<K> Serialize for El<K> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.node.serialize(s)
    }
}

/// Marker types for [`El`].
pub mod kind {
    macro_rules! kinds {
        ($($k:ident),*) => { $(#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub struct $k;)* };
    }
    kinds!(
        Column, Row, Text, Heading, Icon, Image, Button, TextInput, Checkbox, Select, List, Tree, Table, Badge, Progress, Markdown, Code, KeyValue, Banner,
        Link, Divider, EmptyState
    );

    /// Kinds that take children.
    pub trait Container {}
    impl Container for Column {}
    impl Container for Row {}
    impl Container for List {}
    impl Container for Tree {}
    impl Container for Table {}
    impl Container for EmptyState {}
}

use kind::*;

impl<K: Container> El<K> {
    /// Add a child: for a list, table or tree a row (with a key); for an empty-state a button or link.
    pub fn child(mut self, child: impl Into<Node>) -> Self {
        self.node.children.push(child.into());
        self
    }

    pub fn children<C: Into<Node>>(mut self, children: impl IntoIterator<Item = C>) -> Self {
        self.node.children.extend(children.into_iter().map(Into::into));
        self
    }
}

macro_rules! setters {
    ($k:ty { $($(#[$doc:meta])* $method:ident($name:literal, $arg:ty) => $conv:expr;)* }) => {
        impl El<$k> {
            $($(#[$doc])* pub fn $method(self, v: $arg) -> Self {
                #[allow(clippy::redundant_closure_call)]
                let value: Value = ($conv)(v);
                self.set($name, value)
            })*
        }
    };
}

fn text_of(s: impl Into<String>) -> Value {
    Value::String(s.into())
}

setters!(Column {
    /// The space between children, a step from 0 (none) to 4.
    gap("gap", u8) => Value::from;
    /// The space around the children, a step from 0 to 4.
    pad("pad", u8) => Value::from;
});
setters!(Row {
    gap("gap", u8) => Value::from;
    pad("pad", u8) => Value::from;
});
setters!(Text { tone("tone", Tone) => |t| Value::from(tone(t)); });
setters!(Icon {
    tone("tone", Tone) => |t| Value::from(tone(t));
    /// Without a label the icon is decorative and screen readers skip it.
    label("label", &str) => text_of;
});
setters!(Button {
    icon("icon", &str) => text_of;
    /// `Tone::Accent` marks the primary action.
    tone("tone", Tone) => |t| Value::from(tone(t));
    disabled("disabled", bool) => Value::from;
});
setters!(TextInput {
    placeholder("placeholder", &str) => text_of;
    /// The starting text; after that Codeyard keeps what the user types. A different value replaces it.
    value("value", &str) => text_of;
    multiline("multiline", bool) => Value::from;
    disabled("disabled", bool) => Value::from;
});
setters!(Checkbox {
    checked("checked", bool) => Value::from;
    disabled("disabled", bool) => Value::from;
});
setters!(Select {
    /// The key of the chosen option.
    selected("selected", &str) => text_of;
    disabled("disabled", bool) => Value::from;
});
setters!(List { selection("selection", Selection) => |s: Selection| Value::from(s.name()); });
setters!(Tree {
    description("description", &str) => text_of;
    icon("icon", &str) => text_of;
    /// The token Codeyard sends back in an `expand` event when it wants this level's next page.
    more("more", &str) => text_of;
    /// Counts only on the outermost tree.
    selection("selection", Selection) => |s: Selection| Value::from(s.name());
});
setters!(Table { selection("selection", Selection) => |s: Selection| Value::from(s.name()); });
setters!(Badge { tone("tone", Tone) => |t| Value::from(tone(t)); });
setters!(Progress {
    /// 0 to 100; leave it out while the end is unknown.
    percent("percent", u8) => Value::from;
});
setters!(Code { language("language", &str) => text_of; });
setters!(Banner { tone("tone", Tone) => |t| Value::from(tone(t)); });
setters!(Link {
    /// An `https:` address Codeyard opens after the user agrees; without one the extension gets a `link` event.
    url("url", &str) => text_of;
});
setters!(EmptyState { icon("icon", &str) => text_of; });

/// Children top to bottom.
pub fn column() -> El<Column> {
    El::new("column", json!({}))
}

/// Children left to right; inside a table, one row (with a key and a child per column).
pub fn row() -> El<Row> {
    El::new("row", json!({}))
}

pub fn text(text: impl Into<String>) -> El<Text> {
    El::new("text", json!({ "text": text.into() }))
}

/// `level` 1 to 3.
pub fn heading(text: impl Into<String>, level: u8) -> El<Heading> {
    El::new("heading", json!({ "text": text.into(), "level": level }))
}

/// A product icon by name (`refresh`, `git-branch`).
pub fn icon(name: &str) -> El<Icon> {
    El::new("icon", json!({ "icon": name }))
}

/// An image Codeyard fetches from `url` through the `net` permission. An empty `alt` marks it decorative.
pub fn image_url(url: &str, alt: &str) -> El<Image> {
    El::new("image", json!({ "source": { "url": url }, "alt": alt }))
}

/// A PNG, JPEG, GIF, WebP or SVG image of at most 1 MB.
pub fn image_bytes(bytes: &[u8], alt: &str) -> El<Image> {
    El::new("image", json!({ "source": { "bytes": base64(bytes) }, "alt": alt }))
}

/// Sends `{"click": {"path": [… "key"]}}`.
pub fn button(key: impl Into<String>, label: impl Into<String>) -> El<Button> {
    El::new("button", json!({ "label": label.into() })).key(key)
}

/// Sends `change` when the user pauses typing, and `submit` on Enter in a single-line input.
pub fn text_input(key: impl Into<String>, label: impl Into<String>) -> El<TextInput> {
    El::new("text-input", json!({ "label": label.into() })).key(key)
}

pub fn checkbox(key: impl Into<String>, label: impl Into<String>) -> El<Checkbox> {
    El::new("checkbox", json!({ "label": label.into() })).key(key)
}

/// `options` are (key, label) pairs, at most 256.
pub fn select<K: Into<String>, L: Into<String>>(key: impl Into<String>, label: impl Into<String>, options: impl IntoIterator<Item = (K, L)>) -> El<Select> {
    let options: Vec<Value> = options.into_iter().map(|(k, l)| json!({ "key": k.into(), "label": l.into() })).collect();
    El::new("select", json!({ "label": label.into(), "options": options })).key(key)
}

/// Each child is a row with a key; at most 10,000.
pub fn list(key: impl Into<String>, label: impl Into<String>) -> El<List> {
    El::new("list", json!({ "label": label.into() })).key(key)
}

/// A tree, or one of its items: its children are its items, which are trees too.
pub fn tree(key: impl Into<String>, label: impl Into<String>) -> El<Tree> {
    El::new("tree", json!({ "label": label.into() })).key(key)
}

/// Each child is a [`row`] with a key and one child per column.
pub fn table<C: Into<String>>(key: impl Into<String>, label: impl Into<String>, columns: impl IntoIterator<Item = C>) -> El<Table> {
    let columns: Vec<String> = columns.into_iter().map(Into::into).collect();
    El::new("table", json!({ "label": label.into(), "columns": columns })).key(key)
}

pub fn badge(text: impl Into<String>) -> El<Badge> {
    El::new("badge", json!({ "text": text.into() }))
}

pub fn progress(label: impl Into<String>) -> El<Progress> {
    El::new("progress", json!({ "label": label.into() }))
}

/// CommonMark, drawn natively. Remote images are not loaded.
pub fn markdown(source: impl Into<String>) -> El<Markdown> {
    El::new("markdown", json!({ "source": source.into() }))
}

pub fn code(source: impl Into<String>) -> El<Code> {
    El::new("code", json!({ "source": source.into() }))
}

/// (name, value) lines, at most 256.
pub fn key_value<N: Into<String>, V: Into<String>>(pairs: impl IntoIterator<Item = (N, V)>) -> El<KeyValue> {
    let pairs: Vec<Value> = pairs.into_iter().map(|(n, v)| json!({ "name": n.into(), "value": v.into() })).collect();
    El::new("key-value", json!({ "pairs": pairs }))
}

pub fn banner(text: impl Into<String>) -> El<Banner> {
    El::new("banner", json!({ "text": text.into() }))
}

pub fn link(key: impl Into<String>, label: impl Into<String>) -> El<Link> {
    El::new("link", json!({ "label": label.into() })).key(key)
}

pub fn divider() -> El<Divider> {
    El::new("divider", json!({}))
}

/// Shown when a view has nothing yet; its children are its actions, buttons and links.
pub fn empty_state(title: impl Into<String>, text: impl Into<String>) -> El<EmptyState> {
    El::new("empty-state", json!({ "title": title.into(), "text": text.into() }))
}

/// Standard base64 with padding, as Codeyard reads image bytes.
fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}
