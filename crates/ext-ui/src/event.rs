//! What the user did in a view, sent from the host to the extension. `path` is the element's `KeyPath`.

use serde::{Deserialize, Serialize};

use crate::{Key, KeyPath, PageToken};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Event {
    /// A button was pressed (click, Enter or Space).
    Click { path: KeyPath },
    /// A text-input, checkbox or select changed. Text inputs send this when the user pauses or leaves the field, not on
    /// every keystroke.
    Change { path: KeyPath, value: Value },
    /// Enter in a single-line text-input.
    Submit { path: KeyPath, text: String },
    /// The selected rows of a list, table or tree, all of them.
    Select { path: KeyPath, rows: Vec<Key> },
    /// Load the next page of a tree level: of the item `item`, or of the top level when none. `page` is the token
    /// from that level's `more`.
    Expand { path: KeyPath, item: Option<Key>, page: PageToken },
    /// A link without a URL was activated.
    Link { path: KeyPath },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Value {
    Text(String),
    Checked(bool),
    Choice(Key),
}

impl Event {
    /// The JSON form the extension gets in `request.ui-event` (reference/ui.md): `{"click": {"path": ["deploy"]}}`.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("an event always serializes")
    }

    pub fn from_json(s: &str) -> Result<Event, String> {
        serde_json::from_str(s).map_err(|e| e.to_string())
    }
}
