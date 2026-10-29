//! Extension views: a tree of 22 elements that Codeyard draws itself, with the user's theme, keyboard and screen
//! reader. Extensions pick tones and spacing steps, never colours or fonts. This crate holds the tree, its wire form,
//! validation with size caps, the keyed diff, the input state the host keeps, accessibility roles and the events sent
//! back to the extension. No UI and no I/O; the GPUI renderer lives in `app`.
//!
//! Design: docs/plugin-architecture.md §5.3.

mod access;
mod diff;
mod event;
mod state;
mod validate;
mod wire;

#[cfg(test)]
mod fuzz_tests;
#[cfg(test)]
mod json_tests;
#[cfg(test)]
mod tests;

use std::fmt;

use ext_manifest::limits;
use serde::{Deserialize, Serialize};

pub use access::{Access, Role};
pub use diff::{Op, PatchError, apply, diff};
pub use event::{Event, Value};
pub use state::{InputState, State};
pub use validate::{MAX_PROBLEMS, Problem, validate};

/// Identifies an element among its siblings, so the diff can follow it and the host can keep its input state. Required
/// on interactive elements and on every row of a list, table or tree. Any text without control characters. Keys are
/// unique among siblings, where the children of an element without a key count as its siblings' siblings.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Key(String);

impl Key {
    pub fn new(s: impl Into<String>) -> Key {
        Key(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for Key {
    fn from(s: &str) -> Key {
        Key(s.to_string())
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The keys from the root down to an element, skipping elements without a key. Validation makes it unique for every
/// keyed element, so it names the element in events and in the host's input state.
pub type KeyPath = Vec<Key>;

/// Opaque to the host: the extension's cursor for the next page of a tree level, sent back in `Event::Expand`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PageToken(pub String);

/// A product icon by name (`refresh`, `git-branch`): lowercase `a-z 0-9 -`. The renderer draws an unknown name as a
/// neutral placeholder, so a newer icon set never breaks an older app.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IconName(pub String);

/// An `https:` URL. The form is checked by `validate`; whether the host is one the extension declared under `net` is
/// checked by the broker when the image is fetched.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HttpsUrl(String);

impl HttpsUrl {
    pub fn parse(s: &str) -> Result<HttpsUrl, String> {
        if s.len() > limits::UI_URL_LEN {
            return Err(format!("is longer than {} characters", limits::UI_URL_LEN));
        }
        let Some(rest) = s.strip_prefix("https://") else {
            return Err("must start with https://".into());
        };
        if let Some(c) = s.chars().find(|c| c.is_whitespace() || c.is_control() || ext_manifest::is_deceptive(*c)) {
            return Err(format!("contains the character U+{:04X}", c as u32));
        }
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if authority.contains('@') {
            return Err("must not contain a user name or password".into());
        }
        let host = authority.rsplit_once(':').map_or(authority, |(h, port)| if port.bytes().all(|b| b.is_ascii_digit()) { h } else { authority });
        if host.is_empty() || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.') {
            return Err("has no host name".into());
        }
        Ok(HttpsUrl(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The host name, lowercased and without the port, as the broker matches it against `net`.
    pub fn host(&self) -> String {
        let rest = self.0.strip_prefix("https://").unwrap_or(&self.0);
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        authority.rsplit_once(':').map_or(authority, |(h, _)| h).to_ascii_lowercase()
    }
}

/// The meaning of an element's colour. The theme decides the colour, so high contrast works without author effort.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tone {
    #[default]
    Neutral,
    Accent,
    Success,
    Warning,
    Danger,
    Muted,
}

/// A spacing step, 0 (none) to 4 (widest). The renderer maps steps to the theme's spacing scale.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Space(pub u8);

impl Space {
    pub const MAX: u8 = 4;
}

/// How many rows of a list, table or tree the user can select.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Selection {
    #[default]
    None,
    One,
    Many,
}

/// One option of a `select`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub key: Key,
    pub label: String,
}

/// One line of a `key-value` element.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pair {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImageSource {
    /// PNG, JPEG, GIF, WebP or SVG bytes, at most `limits::UI_IMAGE_BYTES`. Base64 in JSON.
    Bytes(#[serde(with = "wire::bytes")] Vec<u8>),
    /// Fetched by the host through the broker, only from a host the extension declared under `net`.
    Url(HttpsUrl),
}

/// The 22 elements and their properties. Children are on `Node`; only column, row, list, tree, table and empty-state
/// take them. In JSON an element is `{"<kind>": {properties}}` with the kind's name (`text-input`); properties marked
/// `#[serde(default)]` may be left out.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum Element {
    Column {
        #[serde(default)]
        gap: Space,
        #[serde(default)]
        pad: Space,
    },
    Row {
        #[serde(default)]
        gap: Space,
        #[serde(default)]
        pad: Space,
    },
    Text {
        text: String,
        #[serde(default)]
        tone: Tone,
    },
    /// `level` 1 to 3.
    Heading {
        text: String,
        level: u8,
    },
    /// Without a label the icon is decorative and hidden from screen readers.
    Icon {
        icon: IconName,
        #[serde(default)]
        tone: Tone,
        #[serde(default)]
        label: Option<String>,
    },
    Image {
        source: ImageSource,
        alt: String,
    },
    /// `tone` accent marks the primary action.
    Button {
        label: String,
        #[serde(default)]
        icon: Option<IconName>,
        #[serde(default)]
        tone: Tone,
        #[serde(default)]
        disabled: bool,
    },
    /// `value` is the starting value; after that the host keeps what the user types (see `State`).
    TextInput {
        label: String,
        #[serde(default)]
        placeholder: String,
        #[serde(default)]
        value: String,
        #[serde(default)]
        multiline: bool,
        #[serde(default)]
        disabled: bool,
    },
    Checkbox {
        label: String,
        #[serde(default)]
        checked: bool,
        #[serde(default)]
        disabled: bool,
    },
    Select {
        label: String,
        options: Vec<Choice>,
        #[serde(default)]
        selected: Option<Key>,
        #[serde(default)]
        disabled: bool,
    },
    /// Each child is a row, with a key. Virtualised by the renderer.
    List {
        label: String,
        #[serde(default)]
        selection: Selection,
    },
    /// A tree, or, inside a tree, one of its items. Children are the items of the level loaded so far; `more` asks
    /// the extension for the next page (`Event::Expand`). An item with no children and `more` set shows as collapsed
    /// and loads its first page when expanded. `selection` counts only on the outermost tree.
    Tree {
        label: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        icon: Option<IconName>,
        #[serde(default)]
        more: Option<PageToken>,
        #[serde(default)]
        selection: Selection,
    },
    /// Each child is a `row` with a key and one child per column.
    Table {
        label: String,
        columns: Vec<String>,
        #[serde(default)]
        selection: Selection,
    },
    Badge {
        text: String,
        #[serde(default)]
        tone: Tone,
    },
    /// `percent` 0 to 100; none means the end is unknown.
    Progress {
        label: String,
        #[serde(default)]
        percent: Option<u8>,
    },
    /// CommonMark, rendered natively. Remote images are not loaded; links open only on a user click.
    Markdown {
        source: String,
    },
    Code {
        source: String,
        #[serde(default)]
        language: Option<String>,
    },
    KeyValue {
        pairs: Vec<Pair>,
    },
    Banner {
        text: String,
        #[serde(default)]
        tone: Tone,
    },
    /// With `url` Codeyard opens it in the browser after the click; without, the extension gets `Event::Link`.
    Link {
        label: String,
        #[serde(default)]
        url: Option<HttpsUrl>,
    },
    Divider,
    /// Shown when a view has nothing yet; children are its actions (buttons and links).
    EmptyState {
        title: String,
        text: String,
        #[serde(default)]
        icon: Option<IconName>,
    },
}

/// The element kinds, without properties.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Column,
    Row,
    Text,
    Heading,
    Icon,
    Image,
    Button,
    TextInput,
    Checkbox,
    Select,
    List,
    Tree,
    Table,
    Badge,
    Progress,
    Markdown,
    Code,
    KeyValue,
    Banner,
    Link,
    Divider,
    EmptyState,
}

impl Kind {
    pub const ALL: [Kind; 22] = [
        Kind::Column,
        Kind::Row,
        Kind::Text,
        Kind::Heading,
        Kind::Icon,
        Kind::Image,
        Kind::Button,
        Kind::TextInput,
        Kind::Checkbox,
        Kind::Select,
        Kind::List,
        Kind::Tree,
        Kind::Table,
        Kind::Badge,
        Kind::Progress,
        Kind::Markdown,
        Kind::Code,
        Kind::KeyValue,
        Kind::Banner,
        Kind::Link,
        Kind::Divider,
        Kind::EmptyState,
    ];

    /// The name authors see in docs and errors.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Column => "column",
            Kind::Row => "row",
            Kind::Text => "text",
            Kind::Heading => "heading",
            Kind::Icon => "icon",
            Kind::Image => "image",
            Kind::Button => "button",
            Kind::TextInput => "text-input",
            Kind::Checkbox => "checkbox",
            Kind::Select => "select",
            Kind::List => "list",
            Kind::Tree => "tree",
            Kind::Table => "table",
            Kind::Badge => "badge",
            Kind::Progress => "progress",
            Kind::Markdown => "markdown",
            Kind::Code => "code",
            Kind::KeyValue => "key-value",
            Kind::Banner => "banner",
            Kind::Link => "link",
            Kind::Divider => "divider",
            Kind::EmptyState => "empty-state",
        }
    }

    pub fn takes_children(self) -> bool {
        matches!(self, Kind::Column | Kind::Row | Kind::List | Kind::Tree | Kind::Table | Kind::EmptyState)
    }

    /// Rows of these kinds are virtualised, need a key each and are capped at `limits::UI_ROWS_PER_PAGE`.
    pub fn has_rows(self) -> bool {
        matches!(self, Kind::List | Kind::Tree | Kind::Table)
    }

    /// The user acts on it, so it sends events and needs a key.
    pub fn is_interactive(self) -> bool {
        matches!(self, Kind::Button | Kind::TextInput | Kind::Checkbox | Kind::Select | Kind::List | Kind::Tree | Kind::Table | Kind::Link)
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl Element {
    pub fn kind(&self) -> Kind {
        match self {
            Element::Column { .. } => Kind::Column,
            Element::Row { .. } => Kind::Row,
            Element::Text { .. } => Kind::Text,
            Element::Heading { .. } => Kind::Heading,
            Element::Icon { .. } => Kind::Icon,
            Element::Image { .. } => Kind::Image,
            Element::Button { .. } => Kind::Button,
            Element::TextInput { .. } => Kind::TextInput,
            Element::Checkbox { .. } => Kind::Checkbox,
            Element::Select { .. } => Kind::Select,
            Element::List { .. } => Kind::List,
            Element::Tree { .. } => Kind::Tree,
            Element::Table { .. } => Kind::Table,
            Element::Badge { .. } => Kind::Badge,
            Element::Progress { .. } => Kind::Progress,
            Element::Markdown { .. } => Kind::Markdown,
            Element::Code { .. } => Kind::Code,
            Element::KeyValue { .. } => Kind::KeyValue,
            Element::Banner { .. } => Kind::Banner,
            Element::Link { .. } => Kind::Link,
            Element::Divider => Kind::Divider,
            Element::EmptyState { .. } => Kind::EmptyState,
        }
    }
}

/// One element of a view with its key and children. Serialized flat (see `wire`), so decoding never recurses.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Node {
    pub key: Option<Key>,
    pub element: Element,
    pub children: Vec<Node>,
}

impl Node {
    pub fn new(element: Element) -> Node {
        Node { key: None, element, children: Vec::new() }
    }

    pub fn key(mut self, key: impl Into<Key>) -> Node {
        self.key = Some(key.into());
        self
    }

    pub fn child(mut self, child: Node) -> Node {
        self.children.push(child);
        self
    }

    pub fn children(mut self, children: impl IntoIterator<Item = Node>) -> Node {
        self.children.extend(children);
        self
    }

    pub fn kind(&self) -> Kind {
        self.element.kind()
    }

    /// This node and all below it.
    pub fn count(&self) -> usize {
        let mut n = 0;
        let mut stack = vec![self];
        while let Some(node) = stack.pop() {
            n += 1;
            stack.extend(&node.children);
        }
        n
    }

    /// The node at `path`, a list of child indices from this node.
    pub fn get(&self, path: &[u32]) -> Option<&Node> {
        path.iter().try_fold(self, |n, &i| n.children.get(i as usize))
    }

    pub fn get_mut(&mut self, path: &[u32]) -> Option<&mut Node> {
        path.iter().try_fold(self, |n, &i| n.children.get_mut(i as usize))
    }
}

impl From<Element> for Node {
    fn from(element: Element) -> Node {
        Node::new(element)
    }
}

/// Why `decode` refused a view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// Larger than `limits::UI_VIEW_BYTES`; not looked at.
    TooLarge(usize),
    /// Not a view in the wire form.
    Malformed(String),
    /// Well formed but breaks a rule; every problem found, up to `MAX_PROBLEMS`.
    Invalid(Vec<Problem>),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::TooLarge(n) => write!(f, "the view is {n} bytes; the limit is {}", limits::UI_VIEW_BYTES),
            DecodeError::Malformed(e) => write!(f, "the view is malformed: {e}"),
            DecodeError::Invalid(problems) => {
                for (i, p) in problems.iter().enumerate() {
                    if i > 0 {
                        f.write_str("\n")?;
                    }
                    write!(f, "{p}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for DecodeError {}

/// The postcard form of a view, as `ext-proto` frames carry it.
pub fn encode(view: &Node) -> Vec<u8> {
    postcard::to_stdvec(view).expect("a view always serializes: every sequence has a known length")
}

/// Read a view the extension sent: size cap, wire form, then `validate`. What comes back is safe to diff and draw.
pub fn decode(bytes: &[u8]) -> Result<Node, DecodeError> {
    if bytes.len() > limits::UI_VIEW_BYTES {
        return Err(DecodeError::TooLarge(bytes.len()));
    }
    let view = wire::decode(bytes).map_err(DecodeError::Malformed)?;
    validate(&view).map_err(DecodeError::Invalid)?;
    Ok(view)
}

/// The JSON form of a view, as extensions send it in `response.view` (docs/extensions/reference/ui.md).
pub fn encode_json(view: &Node) -> String {
    serde_json::to_string(view).expect("a view always serializes")
}

/// Read the JSON a view's `render` answered: size cap, JSON form, then `validate`.
pub fn decode_json(text: &str) -> Result<Node, DecodeError> {
    if text.len() > limits::UI_VIEW_BYTES {
        return Err(DecodeError::TooLarge(text.len()));
    }
    let view: Node = serde_json::from_str(text).map_err(|e| DecodeError::Malformed(e.to_string()))?;
    validate(&view).map_err(DecodeError::Invalid)?;
    Ok(view)
}
