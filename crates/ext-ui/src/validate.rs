//! Every rule a view must keep before it is diffed or drawn, with the caps from `ext_manifest::limits`.

use std::collections::HashSet;
use std::fmt;

use ext_manifest::{is_deceptive, limits};

use crate::{Element, HttpsUrl, IconName, ImageSource, Key, Kind, Node, PageToken, Space};

/// Reporting stops after this many problems; the first ones are enough to fix the view.
pub const MAX_PROBLEMS: usize = 100;

/// One broken rule. `at` names the element: kinds from the root down, with the key in brackets or the position after
/// `#` (`column > list[files] > row[a.txt] > button[open]`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub at: String,
    pub message: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.at, self.message)
    }
}

/// Check a whole view. Every problem is reported (up to `MAX_PROBLEMS`), not just the first.
pub fn validate(view: &Node) -> Result<(), Vec<Problem>> {
    let mut v = Validator { problems: Vec::new(), at: Vec::new() };
    let count = view.count();
    if count > limits::UI_NODES {
        v.at.push(view.kind().name().to_string());
        v.problem(format!("has {count} elements; a view may have at most {}", limits::UI_NODES));
        return Err(v.problems);
    }
    v.at.push(view.kind().name().to_string());
    v.node(view, None, 1);
    if v.problems.is_empty() { Ok(()) } else { Err(v.problems) }
}

struct Validator {
    problems: Vec<Problem>,
    /// Segments of `Problem::at` for the node being checked.
    at: Vec<String>,
}

impl Validator {
    fn problem(&mut self, message: impl Into<String>) {
        if self.problems.len() < MAX_PROBLEMS {
            self.problems.push(Problem { at: self.at.join(" > "), message: message.into() });
        }
    }

    fn node(&mut self, node: &Node, parent: Option<Kind>, depth: usize) {
        if self.problems.len() >= MAX_PROBLEMS {
            return;
        }
        if depth > limits::UI_DEPTH {
            self.problem(format!("is nested deeper than {} levels", limits::UI_DEPTH));
            return;
        }
        let kind = node.kind();
        if let Some(k) = &node.key {
            self.key("the key", k);
        } else if kind.is_interactive() && !(kind == Kind::Tree && parent == Some(Kind::Tree)) {
            self.problem(format!("a {kind} needs a key"));
        }
        self.element(&node.element);
        self.children(node, kind);
        if kind == Kind::Tree && parent != Some(Kind::Tree) {
            self.tree_keys(node);
        }
        // Keys are unique among the keyed elements under one keyed ancestor, so a key path names one element.
        if node.key.is_some() || parent.is_none() {
            let mut seen = HashSet::new();
            self.scope_keys(node, &mut seen);
        }
        for (i, child) in node.children.iter().enumerate() {
            self.at.push(match &child.key {
                Some(k) => format!("{}[{}]", child.kind().name(), k.as_str().escape_debug()),
                None => format!("{}#{i}", child.kind().name()),
            });
            self.node(child, Some(kind), depth + 1);
            self.at.pop();
        }
    }

    /// Collect the keys of the keyed elements in this scope: children, and through unkeyed children their children.
    fn scope_keys<'a>(&mut self, node: &'a Node, seen: &mut HashSet<&'a Key>) {
        for child in &node.children {
            match &child.key {
                Some(k) => {
                    if !seen.insert(k) {
                        self.problem(format!("the key '{}' is used twice; keys must be unique among siblings", k.as_str().escape_debug()));
                    }
                }
                None => self.scope_keys(child, seen),
            }
        }
    }

    /// Items of one tree are selected and expanded by key, so their keys are unique in the whole tree.
    fn tree_keys(&mut self, tree: &Node) {
        let mut seen = HashSet::new();
        let mut stack: Vec<&Node> = tree.children.iter().collect();
        while let Some(item) = stack.pop() {
            if item.kind() != Kind::Tree {
                continue;
            }
            if let Some(k) = &item.key
                && !seen.insert(k)
            {
                self.problem(format!("the tree item key '{}' is used twice; item keys must be unique in the whole tree", k.as_str().escape_debug()));
            }
            stack.extend(&item.children);
        }
    }

    fn children(&mut self, node: &Node, kind: Kind) {
        let n = node.children.len();
        if n > 0 && !kind.takes_children() {
            self.problem(format!("a {kind} has no children"));
            return;
        }
        if kind.has_rows() {
            if n > limits::UI_ROWS_PER_PAGE {
                self.problem(too_many_rows(kind, n));
            }
            if node.children.iter().any(|c| c.key.is_none()) {
                self.problem(format!("every row of a {kind} needs a key"));
            }
        }
        match &node.element {
            Element::Table { columns, .. } => {
                for (i, row) in node.children.iter().enumerate() {
                    if row.kind() != Kind::Row {
                        self.problem(format!("row {i} is a {}; the rows of a table are rows", row.kind()));
                    } else if row.children.len() != columns.len() {
                        self.problem(format!("row {i} has {} cells for {} columns", row.children.len(), columns.len()));
                    }
                }
            }
            Element::Tree { .. } => {
                if let Some(c) = node.children.iter().find(|c| c.kind() != Kind::Tree) {
                    self.problem(format!("holds a {}; the items of a tree are trees", c.kind()));
                }
            }
            Element::EmptyState { .. } => {
                if let Some(c) = node.children.iter().find(|c| !matches!(c.kind(), Kind::Button | Kind::Link)) {
                    self.problem(format!("holds a {}; an empty-state takes only buttons and links", c.kind()));
                }
            }
            _ => {}
        }
    }

    fn element(&mut self, element: &Element) {
        match element {
            Element::Column { gap, pad } | Element::Row { gap, pad } => {
                self.space("gap", *gap);
                self.space("pad", *pad);
            }
            Element::Text { text, .. } | Element::Banner { text, .. } => self.body("the text", text),
            Element::Heading { text, level } => {
                self.label("the text", text);
                if !(1..=3).contains(level) {
                    self.problem(format!("the level is {level}; use 1, 2 or 3"));
                }
            }
            Element::Icon { icon, label, .. } => {
                self.icon(icon);
                if let Some(l) = label {
                    self.label("the label", l);
                }
            }
            Element::Image { source, alt } => {
                if !alt.is_empty() {
                    self.label("the alt text", alt);
                }
                match source {
                    ImageSource::Bytes(b) if b.is_empty() => self.problem("the image has no bytes"),
                    ImageSource::Bytes(b) if b.len() > limits::UI_IMAGE_BYTES => {
                        self.problem(format!("the image is {} bytes; the limit is {}", b.len(), limits::UI_IMAGE_BYTES))
                    }
                    ImageSource::Bytes(_) => {}
                    ImageSource::Url(u) => self.url(u),
                }
            }
            Element::Button { label, icon, .. } => {
                self.label("the label", label);
                if let Some(i) = icon {
                    self.icon(i);
                }
            }
            Element::TextInput { label, placeholder, value, .. } => {
                self.label("the label", label);
                self.body("the placeholder", placeholder);
                self.body("the value", value);
            }
            Element::Checkbox { label, .. } | Element::List { label, .. } => self.label("the label", label),
            Element::Select { label, options, selected, .. } => {
                self.label("the label", label);
                if options.len() > limits::UI_CHOICES {
                    self.problem(format!("has {} options; the limit is {}", options.len(), limits::UI_CHOICES));
                }
                let mut seen = HashSet::new();
                for o in options.iter().take(limits::UI_CHOICES) {
                    self.key("an option key", &o.key);
                    self.label("an option label", &o.label);
                    if !seen.insert(&o.key) {
                        self.problem(format!("the option key '{}' is used twice", o.key.as_str().escape_debug()));
                    }
                }
                if let Some(s) = selected
                    && !seen.contains(s)
                {
                    self.problem(format!("the selected option '{}' is not one of the options", s.as_str().escape_debug()));
                }
            }
            Element::Tree { label, description, icon, more, .. } => {
                // The outermost tree's label names the tree for screen readers; an item's label is its text.
                self.label("the label", label);
                if let Some(d) = description {
                    self.label("the description", d);
                }
                if let Some(i) = icon {
                    self.icon(i);
                }
                if let Some(t) = more {
                    self.token(t);
                }
            }
            Element::Table { label, columns, .. } => {
                self.label("the label", label);
                if columns.is_empty() || columns.len() > limits::UI_COLUMNS {
                    self.problem(format!("has {} columns; use 1 to {}", columns.len(), limits::UI_COLUMNS));
                }
                for c in columns.iter().take(limits::UI_COLUMNS) {
                    self.label("a column header", c);
                }
            }
            Element::Badge { text, .. } => self.label("the text", text),
            Element::Progress { label, percent } => {
                self.label("the label", label);
                if let Some(p) = percent
                    && *p > 100
                {
                    self.problem(format!("the percent is {p}; use 0 to 100"));
                }
            }
            Element::Markdown { source } => self.document("the markdown", source),
            Element::Code { source, language } => {
                self.document("the code", source);
                if let Some(l) = language {
                    self.label("the language", l);
                }
            }
            Element::KeyValue { pairs } => {
                if pairs.len() > limits::UI_CHOICES {
                    self.problem(format!("has {} pairs; the limit is {}", pairs.len(), limits::UI_CHOICES));
                }
                for p in pairs.iter().take(limits::UI_CHOICES) {
                    self.label("a name", &p.name);
                    self.body("a value", &p.value);
                }
            }
            Element::Link { label, url } => {
                self.label("the label", label);
                if let Some(u) = url {
                    self.url(u);
                }
            }
            Element::Divider => {}
            Element::EmptyState { title, text, icon } => {
                self.label("the title", title);
                self.body("the text", text);
                if let Some(i) = icon {
                    self.icon(i);
                }
            }
        }
    }

    /// Text shown as a label, name or title: one line, not empty, and nothing that disguises it (the same rule as text
    /// on the consent sheet).
    fn label(&mut self, what: &str, s: &str) {
        if s.trim().is_empty() {
            self.problem(format!("{what} is empty"));
        } else if s.chars().count() > limits::UI_LABEL_LEN {
            self.problem(format!("{what} is longer than {} characters", limits::UI_LABEL_LEN));
        } else if let Some(c) = s.chars().find(|&c| is_deceptive(c)) {
            self.problem(format!("{what} contains the invisible or control character U+{:04X}", c as u32));
        }
    }

    /// Running text: may be empty and span lines. Zero-width joiners stay allowed, since emoji and some scripts need them.
    fn body(&mut self, what: &str, s: &str) {
        if s.chars().count() > limits::UI_TEXT_LEN {
            self.problem(format!("{what} is longer than {} characters", limits::UI_TEXT_LEN));
        } else {
            self.controls(what, s);
        }
    }

    fn document(&mut self, what: &str, s: &str) {
        if s.len() > limits::UI_DOCUMENT_BYTES {
            self.problem(format!("{what} is {} bytes; the limit is {}", s.len(), limits::UI_DOCUMENT_BYTES));
        } else {
            self.controls(what, s);
        }
    }

    fn controls(&mut self, what: &str, s: &str) {
        if let Some(c) = s.chars().find(|&c| c.is_control() && !matches!(c, '\n' | '\t' | '\r')) {
            self.problem(format!("{what} contains the control character U+{:04X}", c as u32));
        }
    }

    fn key(&mut self, what: &str, k: &Key) {
        let s = k.as_str();
        if s.is_empty() {
            self.problem(format!("{what} is empty"));
        } else if s.len() > limits::UI_KEY_LEN {
            self.problem(format!("{what} is longer than {} bytes", limits::UI_KEY_LEN));
        } else if let Some(c) = s.chars().find(|c| c.is_control()) {
            self.problem(format!("{what} contains the control character U+{:04X}", c as u32));
        }
    }

    fn token(&mut self, t: &PageToken) {
        if t.0.len() > limits::UI_KEY_LEN {
            self.problem(format!("the page token is longer than {} bytes", limits::UI_KEY_LEN));
        }
    }

    fn space(&mut self, what: &str, s: Space) {
        if s.0 > Space::MAX {
            self.problem(format!("the {what} is step {}; use 0 to {}", s.0, Space::MAX));
        }
    }

    fn icon(&mut self, icon: &IconName) {
        if let Err(e) = ext_manifest::check_id_part(&icon.0) {
            self.problem(format!("the icon name '{}' {e}", icon.0.escape_debug()));
        }
    }

    fn url(&mut self, u: &HttpsUrl) {
        if let Err(e) = HttpsUrl::parse(u.as_str()) {
            self.problem(format!("the URL {e}"));
        }
    }
}

/// A list or table has no pages, so the fix is fewer rows; a tree sends the rest a page at a time (e2e trial: "send at
/// most 10000 at a time" for a list, which cannot).
fn too_many_rows(kind: Kind, n: usize) -> String {
    let max = limits::UI_ROWS_PER_PAGE;
    match kind {
        Kind::Tree => format!("has {n} rows; a tree takes at most {max} at a time: send the rest as later pages with `more`"),
        _ => format!("has {n} rows; a {kind} takes at most {max}: show fewer, such as the newest, or let the user filter them"),
    }
}
