//! The wire form of a tree: its nodes in pre-order, each with its number of children. Flat, so a hostile frame cannot
//! make the decoder recurse, and the nesting depth is refused while the tree is rebuilt.

use ext_manifest::limits;
use serde::de::Error as _;
use serde::ser::SerializeSeq;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{Element, Key, Node};

#[derive(Serialize)]
struct FlatRef<'a> {
    key: &'a Option<Key>,
    element: &'a Element,
    children: u32,
}

#[derive(Deserialize)]
struct Flat {
    key: Option<Key>,
    element: Element,
    children: u32,
}

impl Serialize for Node {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(self.count()))?;
        let mut stack = vec![self];
        while let Some(n) = stack.pop() {
            let children = u32::try_from(n.children.len()).map_err(|_| serde::ser::Error::custom("too many children"))?;
            seq.serialize_element(&FlatRef { key: &n.key, element: &n.element, children })?;
            stack.extend(n.children.iter().rev());
        }
        seq.end()
    }
}

impl<'de> Deserialize<'de> for Node {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Node, D::Error> {
        build(Vec::<Flat>::deserialize(d)?).map_err(D::Error::custom)
    }
}

/// Decode one tree from `bytes`, with readable errors (postcard drops the text of custom serde errors).
pub(crate) fn decode(bytes: &[u8]) -> Result<Node, String> {
    let (flat, rest) = postcard::take_from_bytes::<Vec<Flat>>(bytes).map_err(|e| e.to_string())?;
    if !rest.is_empty() {
        return Err(format!("{} bytes after the view", rest.len()));
    }
    build(flat)
}

fn build(flat: Vec<Flat>) -> Result<Node, String> {
    let mut items = flat.into_iter();
    let mut open: Vec<(Node, u32)> = Vec::new();
    loop {
        // Close every node that has all its children, attaching it to its parent.
        while let Some((_, 0)) = open.last() {
            let (done, _) = open.pop().expect("checked by the loop condition");
            match open.last_mut() {
                Some((parent, remaining)) => {
                    parent.children.push(done);
                    *remaining -= 1;
                }
                None if items.as_slice().is_empty() => return Ok(done),
                None => return Err(format!("{} nodes after the root", items.len())),
            }
        }
        let Some(f) = items.next() else {
            return Err(if open.is_empty() { "the view is empty".into() } else { "the view ends before all children are given".into() });
        };
        if open.len() >= limits::UI_DEPTH {
            return Err(format!("nested deeper than {} levels", limits::UI_DEPTH));
        }
        open.push((Node { key: f.key, element: f.element, children: Vec::new() }, f.children));
    }
}
