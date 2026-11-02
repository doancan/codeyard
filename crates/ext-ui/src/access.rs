//! Accessibility: the role and label of every element, for the renderer to hand to AccessKit. Authors write no
//! accessibility code; it comes from the element kinds and their text.

use ext_manifest::limits;

use crate::{Element, Kind, Node, Tone};

/// Screen reader roles. Each maps to one AccessKit role in the renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// Not announced: layout containers without a label and decorative icons.
    Hidden,
    Group,
    Label,
    /// Level 1 to 3.
    Heading(u8),
    Image,
    Button,
    TextInput,
    MultilineTextInput,
    CheckBox,
    ComboBox,
    List,
    ListItem,
    Tree,
    TreeItem,
    Table,
    Row,
    Cell,
    Status,
    /// A warning or danger banner, announced when it appears.
    Alert,
    ProgressIndicator,
    Document,
    Code,
    DescriptionList,
    Link,
    Separator,
}

/// What a screen reader announces for one element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Access {
    pub role: Role,
    pub label: String,
    pub description: Option<String>,
}

impl Node {
    /// Role and label of this node. `ancestors` are the kinds above it, its parent last: rows of a list, table or tree
    /// are announced as list items, rows, cells and tree items, labelled with their text.
    pub fn access(&self, ancestors: &[Kind]) -> Access {
        let plain = |role: Role, label: &str| Access { role, label: label.to_string(), description: None };
        let parent = ancestors.last().copied();
        match (ancestors.len().checked_sub(2).map(|i| ancestors[i]), parent) {
            (_, Some(Kind::List)) => return plain(Role::ListItem, &self.text()),
            (_, Some(Kind::Table)) => return plain(Role::Row, &self.text()),
            (Some(Kind::Table), Some(Kind::Row)) => return plain(Role::Cell, &self.text()),
            _ => {}
        }
        match &self.element {
            Element::Column { .. } | Element::Row { .. } => plain(Role::Hidden, ""),
            Element::Text { text, .. } => plain(Role::Label, text),
            Element::Heading { text, level } => plain(Role::Heading(*level), text),
            Element::Icon { label: Some(l), .. } => plain(Role::Image, l),
            Element::Icon { label: None, .. } => plain(Role::Hidden, ""),
            Element::Image { alt, .. } if alt.is_empty() => plain(Role::Hidden, ""),
            Element::Image { alt, .. } => plain(Role::Image, alt),
            Element::Button { label, .. } => plain(Role::Button, label),
            Element::TextInput { label, placeholder, multiline, .. } => Access {
                role: if *multiline { Role::MultilineTextInput } else { Role::TextInput },
                label: label.clone(),
                description: (!placeholder.is_empty()).then(|| placeholder.clone()),
            },
            Element::Checkbox { label, .. } => plain(Role::CheckBox, label),
            Element::Select { label, .. } => plain(Role::ComboBox, label),
            Element::List { label, .. } => plain(Role::List, label),
            Element::Tree { label, description, .. } => {
                Access { role: if parent == Some(Kind::Tree) { Role::TreeItem } else { Role::Tree }, label: label.clone(), description: description.clone() }
            }
            Element::Table { label, .. } => plain(Role::Table, label),
            Element::Badge { text, .. } => plain(Role::Status, text),
            Element::Progress { label, percent } => {
                Access { role: Role::ProgressIndicator, label: label.clone(), description: percent.map(|p| format!("{p}%")) }
            }
            Element::Markdown { source } => plain(Role::Document, &clip(source)),
            Element::Code { source, language } => Access { role: Role::Code, label: clip(source), description: language.clone() },
            Element::KeyValue { pairs } => {
                let text: Vec<String> = pairs.iter().map(|p| format!("{}: {}", p.name, p.value)).collect();
                plain(Role::DescriptionList, &clip(&text.join(", ")))
            }
            Element::Banner { text, tone: Tone::Warning | Tone::Danger } => plain(Role::Alert, text),
            Element::Banner { text, .. } => plain(Role::Status, text),
            Element::Link { label, url } => Access { role: Role::Link, label: label.clone(), description: url.as_ref().map(|u| u.host()) },
            Element::Divider => plain(Role::Separator, ""),
            Element::EmptyState { title, text, .. } => {
                Access { role: Role::Group, label: title.clone(), description: (!text.is_empty()).then(|| text.clone()) }
            }
        }
    }

    /// The text a row shows, joined with ", " and cut at `limits::UI_LABEL_LEN` characters; labels a list item, table
    /// row or cell for a screen reader.
    pub fn text(&self) -> String {
        let mut parts = Vec::new();
        let mut stack = vec![self];
        while let Some(n) = stack.pop() {
            let t = match &n.element {
                Element::Text { text, .. } | Element::Heading { text, .. } | Element::Badge { text, .. } | Element::Banner { text, .. } => Some(text),
                Element::Button { label, .. } | Element::Link { label, .. } | Element::Checkbox { label, .. } => Some(label),
                Element::Icon { label: Some(l), .. } => Some(l),
                Element::Tree { label, .. } => Some(label),
                _ => None,
            };
            if let Some(t) = t.filter(|t| !t.is_empty()) {
                parts.push(t.as_str());
            }
            stack.extend(n.children.iter().rev());
        }
        clip(&parts.join(", "))
    }
}

/// Cut to `limits::UI_LABEL_LEN` characters; screen readers read labels whole.
fn clip(s: &str) -> String {
    match s.char_indices().nth(limits::UI_LABEL_LEN) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}
