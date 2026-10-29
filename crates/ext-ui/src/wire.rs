//! The two forms of a tree. Binary (postcard): its nodes in pre-order, each with its number of children; flat, so a
//! hostile frame cannot make the decoder recurse, and the nesting depth is refused while the tree is rebuilt. JSON (what
//! extensions send in `response.view`): nested, `{"key": …, "element": {…}, "children": [ … ]}`, as authors and SDKs
//! write it; serde_json refuses nesting deeper than 128 levels before the stack is at risk, and `validate` refuses a
//! tree deeper than `limits::UI_DEPTH` elements.

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

/// One node in the JSON form.
#[derive(Serialize)]
struct NestedRef<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    key: &'a Option<Key>,
    element: &'a Element,
    #[serde(skip_serializing_if = "<[Node]>::is_empty")]
    children: &'a [Node],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Nested {
    #[serde(default)]
    key: Option<Key>,
    element: Element,
    #[serde(default)]
    children: Vec<Node>,
}

impl Serialize for Node {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if s.is_human_readable() {
            return NestedRef { key: &self.key, element: &self.element, children: &self.children }.serialize(s);
        }
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
        if d.is_human_readable() {
            let n = Nested::deserialize(d)?;
            return Ok(Node { key: n.key, element: n.element, children: n.children });
        }
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

/// `ImageSource::Bytes`: base64 in JSON, raw bytes in postcard.
pub(crate) mod bytes {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(b: &[u8], s: S) -> Result<S::Ok, S::Error> {
        if s.is_human_readable() { s.serialize_str(&STANDARD.encode(b)) } else { s.serialize_bytes(b) }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        if d.is_human_readable() {
            let text = String::deserialize(d)?;
            STANDARD.decode(text.as_bytes()).map_err(|e| D::Error::custom(format!("the image is not base64: {e}")))
        } else {
            Vec::<u8>::deserialize(d)
        }
    }
}
