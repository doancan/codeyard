//! Input state lives in the host, not in the extension: what the user typed, ticked, chose, selected, expanded and
//! scrolled stays across renders, so a re-render never loses a keystroke. Entries are keyed by `KeyPath` and dropped
//! when their element is gone.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::{Element, Key, KeyPath, Kind, Node, Selection};

/// The state of one element. Only the fields of its kind are used.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputState {
    /// `text-input`: the text in the field.
    pub text: String,
    /// `checkbox`.
    pub checked: bool,
    /// `select`: the chosen option.
    pub choice: Option<Key>,
    /// `list`, `table`, `tree`: the selected rows (tree: items anywhere in the tree).
    pub selected: BTreeSet<Key>,
    /// `tree`: the expanded items.
    pub expanded: BTreeSet<Key>,
    /// `list`, `table`, `tree`: the first visible row, so scrolling survives a re-render.
    pub first_row: u32,
}

/// Every stateful element of one view.
#[derive(Clone, Debug, Default)]
pub struct State {
    entries: HashMap<KeyPath, (Kind, InputState)>,
}

impl State {
    pub fn new() -> State {
        State::default()
    }

    pub fn get(&self, path: &[Key]) -> Option<&InputState> {
        self.entries.get(path).map(|(_, s)| s)
    }

    /// The renderer writes user input here.
    pub fn get_mut(&mut self, path: &[Key]) -> Option<&mut InputState> {
        self.entries.get_mut(path).map(|(_, s)| s)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Bring the state in line with a new render. `old` is the previous render (none for the first). New elements
    /// start from their properties; kept elements keep the user's input unless the extension changed the starting
    /// property itself (a new `value`, `checked` or `selected`); entries of elements that are gone, or whose kind
    /// changed, are dropped. Selections and expanded items that no longer exist are forgotten.
    pub fn sync(&mut self, old: Option<&Node>, new: &Node) {
        let before: HashMap<KeyPath, &Node> = old.map(stateful).unwrap_or_default().into_iter().collect();
        let now = stateful(new);
        let kinds: HashMap<&KeyPath, Kind> = now.iter().map(|(p, n)| (p, n.kind())).collect();
        self.entries.retain(|p, (kind, _)| kinds.get(p) == Some(kind));
        for (path, node) in &now {
            let prev = before.get(path).filter(|o| o.kind() == node.kind());
            let entry = self.entries.entry(path.clone()).or_insert_with(|| (node.kind(), start(node)));
            update(&mut entry.1, prev.map(|o| &o.element), node);
        }
    }
}

/// The starting state from an element's properties.
fn start(node: &Node) -> InputState {
    let mut s = InputState::default();
    match &node.element {
        Element::TextInput { value, .. } => s.text = value.clone(),
        Element::Checkbox { checked, .. } => s.checked = *checked,
        Element::Select { selected, .. } => s.choice = selected.clone(),
        _ => {}
    }
    s
}

fn update(s: &mut InputState, old: Option<&Element>, node: &Node) {
    match (&node.element, old) {
        (Element::TextInput { value, .. }, Some(Element::TextInput { value: was, .. })) if value != was => s.text = value.clone(),
        (Element::Checkbox { checked, .. }, Some(Element::Checkbox { checked: was, .. })) if checked != was => s.checked = *checked,
        (Element::Select { options, selected, .. }, old) => {
            let changed = matches!(old, Some(Element::Select { selected: was, .. }) if was != selected);
            let gone = s.choice.as_ref().is_some_and(|c| !options.iter().any(|o| &o.key == c));
            if changed || gone {
                s.choice = selected.clone();
            }
        }
        (Element::List { selection, .. } | Element::Table { selection, .. }, _) => {
            let rows: HashSet<&Key> = node.children.iter().filter_map(|c| c.key.as_ref()).collect();
            s.selected.retain(|k| rows.contains(k));
            limit(&mut s.selected, *selection);
        }
        (Element::Tree { selection, .. }, _) => {
            let mut items = HashSet::new();
            let mut stack: Vec<&Node> = node.children.iter().collect();
            while let Some(item) = stack.pop() {
                if let Some(k) = &item.key {
                    items.insert(k);
                }
                stack.extend(&item.children);
            }
            s.selected.retain(|k| items.contains(k));
            s.expanded.retain(|k| items.contains(k));
            limit(&mut s.selected, *selection);
        }
        _ => {}
    }
}

fn limit(selected: &mut BTreeSet<Key>, selection: Selection) {
    match selection {
        Selection::None => selected.clear(),
        Selection::One => {
            while selected.len() > 1 {
                selected.pop_last();
            }
        }
        Selection::Many => {}
    }
}

/// The elements that keep state, with their key paths: inputs, lists, tables and outermost trees (items keep theirs in
/// the tree's entry). Elements without a key have no entry.
fn stateful(root: &Node) -> Vec<(KeyPath, &Node)> {
    let mut out = Vec::new();
    let mut stack: Vec<(&Node, KeyPath, Option<Kind>)> = vec![(root, Vec::new(), None)];
    while let Some((node, mut path, parent)) = stack.pop() {
        let kind = node.kind();
        if let Some(k) = &node.key {
            path.push(k.clone());
            let holds = matches!(kind, Kind::TextInput | Kind::Checkbox | Kind::Select | Kind::List | Kind::Table)
                || (kind == Kind::Tree && parent != Some(Kind::Tree));
            if holds {
                out.push((path.clone(), node));
            }
        }
        for c in &node.children {
            stack.push((c, path.clone(), Some(kind)));
        }
    }
    out
}
