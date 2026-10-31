//! The view format as data, for checkers that cannot call this crate: every element with its properties (the type,
//! the rule `validate` applies, and whether it may be left out or be `null`), the kinds that take children, have rows
//! or need a key, the events, and the limits. The TypeScript SDK's mock host checks views with it
//! (sdk/typescript/src/testing/view-check.ts), so `npm test` refuses what Codeyard refuses.
//!
//! `schema_tests.rs` keeps it honest: each element's properties are compared with what serde accepts, and the
//! TypeScript copy (sdk/typescript/src/view-schema.ts) and the shared cases (sdk/typescript/test/view-cases.json) must be
//! what this crate generates.

use ext_manifest::limits;
use serde_json::{Value, json};

use crate::{Kind, MAX_PROBLEMS, Space};

/// What a property holds and the rule `validate` checks it with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    /// A spacing step, 0 to `Space::MAX`.
    Space,
    Tone,
    Selection,
    Bool,
    /// One line, not empty, at most `UI_LABEL_LEN` characters, no invisible characters.
    Label,
    /// Running text: at most `UI_TEXT_LEN` characters, no control characters but newline and tab.
    Body,
    /// Markdown or code: at most `UI_DOCUMENT_BYTES` bytes, no control characters but newline and tab.
    Document,
    /// An image's alt text: empty, or a label.
    Alt,
    /// A heading level, 1 to 3.
    Level,
    /// 0 to 100.
    Percent,
    /// A product icon name (an id part: `a-z 0-9 -`).
    Icon,
    /// An `https:` URL.
    Url,
    /// A tree page token: at most `UI_KEY_LEN` bytes.
    Page,
    /// The key of one of the select's options.
    OptionKey,
    /// A select's options: `[{"key", "label"}]`, at most `UI_CHOICES`, keys unique.
    Choices,
    /// `[{"name", "value"}]`, at most `UI_CHOICES`.
    Pairs,
    /// Table headers, 1 to `UI_COLUMNS` labels.
    Columns,
    /// `{"bytes": base64}` or `{"url": "https://…"}`.
    Image,
}

impl Type {
    pub fn name(self) -> &'static str {
        match self {
            Type::Space => "space",
            Type::Tone => "tone",
            Type::Selection => "selection",
            Type::Bool => "bool",
            Type::Label => "label",
            Type::Body => "body",
            Type::Document => "document",
            Type::Alt => "alt",
            Type::Level => "level",
            Type::Percent => "percent",
            Type::Icon => "icon",
            Type::Url => "url",
            Type::Page => "page",
            Type::OptionKey => "option-key",
            Type::Choices => "choices",
            Type::Pairs => "pairs",
            Type::Columns => "columns",
            Type::Image => "image",
        }
    }
}

/// One property of an element.
#[derive(Clone, Copy, Debug)]
pub struct Property {
    pub name: &'static str,
    pub ty: Type,
    /// Must be given.
    pub required: bool,
    /// May be `null` (the same as leaving it out).
    pub nullable: bool,
    /// How problems name it ("the label", "the gap").
    pub what: &'static str,
}

const fn req(name: &'static str, ty: Type, what: &'static str) -> Property {
    Property { name, ty, required: true, nullable: false, what }
}

const fn opt(name: &'static str, ty: Type, what: &'static str) -> Property {
    Property { name, ty, required: false, nullable: false, what }
}

const fn null(name: &'static str, ty: Type, what: &'static str) -> Property {
    Property { name, ty, required: false, nullable: true, what }
}

/// The properties of `kind`, in the order `validate` checks them.
pub fn properties(kind: Kind) -> &'static [Property] {
    use Type::*;
    match kind {
        Kind::Column | Kind::Row => const { &[opt("gap", Space, "gap"), opt("pad", Space, "pad")] },
        Kind::Text | Kind::Banner => const { &[req("text", Body, "the text"), opt("tone", Tone, "the tone")] },
        Kind::Heading => const { &[req("text", Label, "the text"), req("level", Level, "the level")] },
        Kind::Icon => const { &[req("icon", Icon, "the icon name"), opt("tone", Tone, "the tone"), null("label", Label, "the label")] },
        Kind::Image => const { &[req("alt", Alt, "the alt text"), req("source", Image, "the image")] },
        Kind::Button => {
            const { &[req("label", Label, "the label"), null("icon", Icon, "the icon name"), opt("tone", Tone, "the tone"), opt("disabled", Bool, "disabled")] }
        }
        Kind::TextInput => {
            const {
                &[
                    req("label", Label, "the label"),
                    opt("placeholder", Body, "the placeholder"),
                    opt("value", Body, "the value"),
                    opt("multiline", Bool, "multiline"),
                    opt("disabled", Bool, "disabled"),
                ]
            }
        }
        Kind::Checkbox => const { &[req("label", Label, "the label"), opt("checked", Bool, "checked"), opt("disabled", Bool, "disabled")] },
        Kind::Select => {
            const {
                &[
                    req("label", Label, "the label"),
                    req("options", Choices, "the options"),
                    null("selected", OptionKey, "the selected option"),
                    opt("disabled", Bool, "disabled"),
                ]
            }
        }
        Kind::List => const { &[req("label", Label, "the label"), opt("selection", Selection, "the selection")] },
        Kind::Tree => {
            const {
                &[
                    req("label", Label, "the label"),
                    null("description", Label, "the description"),
                    null("icon", Icon, "the icon name"),
                    null("more", Page, "the page token"),
                    opt("selection", Selection, "the selection"),
                ]
            }
        }
        Kind::Table => const { &[req("label", Label, "the label"), req("columns", Columns, "a column header"), opt("selection", Selection, "the selection")] },
        Kind::Badge => const { &[req("text", Label, "the text"), opt("tone", Tone, "the tone")] },
        Kind::Progress => const { &[req("label", Label, "the label"), null("percent", Percent, "the percent")] },
        Kind::Markdown => const { &[req("source", Document, "the markdown")] },
        Kind::Code => const { &[req("source", Document, "the code"), null("language", Label, "the language")] },
        Kind::KeyValue => const { &[req("pairs", Pairs, "the pairs")] },
        Kind::Link => const { &[req("label", Label, "the label"), null("url", Url, "the URL")] },
        Kind::Divider => const { &[] },
        Kind::EmptyState => const { &[req("title", Label, "the title"), req("text", Body, "the text"), null("icon", Icon, "the icon name")] },
    }
}

/// The events Codeyard sends in `request.ui-event`, each with its fields: `path` (a key path), `value` (a change's
/// `{"text"|"checked"|"choice": …}`), `text`, `rows` (keys), `item` (a key or `null`) and `page` (a token).
pub fn events() -> Value {
    json!({
        "click": ["path"],
        "change": ["path", "value"],
        "submit": ["path", "text"],
        "select": ["path", "rows"],
        "expand": ["path", "item", "page"],
        "link": ["path"],
    })
}

/// The whole format as JSON.
pub fn schema() -> Value {
    let elements: Vec<Value> = Kind::ALL
        .iter()
        .map(|&k| {
            let props: Vec<Value> = properties(k)
                .iter()
                .map(|p| json!({"name": p.name, "type": p.ty.name(), "required": p.required, "nullable": p.nullable, "what": p.what}))
                .collect();
            json!({
                "kind": k.name(),
                "children": k.takes_children(),
                "rows": k.has_rows(),
                "interactive": k.is_interactive(),
                "properties": props,
            })
        })
        .collect();
    json!({
        "elements": elements,
        "tones": ["neutral", "accent", "success", "warning", "danger", "muted"],
        "selections": ["none", "one", "many"],
        "events": events(),
        "limits": {
            "viewBytes": limits::UI_VIEW_BYTES,
            "nodes": limits::UI_NODES,
            "depth": limits::UI_DEPTH,
            "rowsPerPage": limits::UI_ROWS_PER_PAGE,
            "labelLen": limits::UI_LABEL_LEN,
            "textLen": limits::UI_TEXT_LEN,
            "documentBytes": limits::UI_DOCUMENT_BYTES,
            "keyLen": limits::UI_KEY_LEN,
            "imageBytes": limits::UI_IMAGE_BYTES,
            "choices": limits::UI_CHOICES,
            "columns": limits::UI_COLUMNS,
            "urlLen": limits::UI_URL_LEN,
            "iconLen": limits::ID_PART_LEN,
            "spaceMax": Space::MAX,
            "problems": MAX_PROBLEMS,
        },
    })
}
