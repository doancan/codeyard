//! The keyed diff: what changed between two renders of a view, as a list of operations the renderer applies to the
//! tree it draws. Children are matched by key; children without a key are matched by their order among the unkeyed
//! siblings. Rows that keep their order are not touched, so a list of 10k rows with one row moved is one `Move`.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{Element, Key, Node};

/// One change. Paths are child indices from the root, valid in the tree as it is when the operation applies; the
/// operations apply in order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    /// Put `node` (with its children) in place of the element at `at`; the root when `at` is empty. Used when the
    /// kind changes.
    Replace {
        at: Vec<u32>,
        node: Node,
    },
    /// New properties for the element at `at`, same kind and key; its children stay.
    Update {
        at: Vec<u32>,
        element: Element,
    },
    Insert {
        parent: Vec<u32>,
        index: u32,
        node: Node,
    },
    Remove {
        parent: Vec<u32>,
        index: u32,
    },
    /// Take the child at `from` out, then put it at `to` in the shortened list.
    Move {
        parent: Vec<u32>,
        from: u32,
        to: u32,
    },
}

/// `apply` met an operation that does not fit the tree: a path or index out of range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PatchError {
    /// Index of the operation in the list.
    pub op: usize,
}

impl fmt::Display for PatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "operation {} does not fit the tree", self.op)
    }
}

impl std::error::Error for PatchError {}

/// The operations that turn `old` into `new`: `apply(&mut old, diff(&old, &new))` leaves `old == new`.
pub fn diff(old: &Node, new: &Node) -> Vec<Op> {
    let mut ops = Vec::new();
    node(old, new, &mut Vec::new(), &mut ops);
    ops
}

/// Apply `ops` from `diff` in order. On error the tree is left as the earlier operations made it.
pub fn apply(root: &mut Node, ops: impl IntoIterator<Item = Op>) -> Result<(), PatchError> {
    for (i, op) in ops.into_iter().enumerate() {
        let err = PatchError { op: i };
        match op {
            Op::Replace { at, node } => *root.get_mut(&at).ok_or(err)? = node,
            Op::Update { at, element } => root.get_mut(&at).ok_or(err)?.element = element,
            Op::Insert { parent, index, node } => {
                let children = &mut root.get_mut(&parent).ok_or(err)?.children;
                if index as usize > children.len() {
                    return Err(err);
                }
                children.insert(index as usize, node);
            }
            Op::Remove { parent, index } => {
                let children = &mut root.get_mut(&parent).ok_or(err)?.children;
                if index as usize >= children.len() {
                    return Err(err);
                }
                children.remove(index as usize);
            }
            Op::Move { parent, from, to } => {
                let children = &mut root.get_mut(&parent).ok_or(err)?.children;
                if from as usize >= children.len() || to as usize >= children.len() {
                    return Err(err);
                }
                let n = children.remove(from as usize);
                children.insert(to as usize, n);
            }
        }
    }
    Ok(())
}

/// How a child is matched between renders.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Id<'a> {
    Key(&'a Key),
    /// The n-th child without a key.
    Nth(usize),
}

/// The ids of `children`, or none when two share a key (an unvalidated tree; the parent is then replaced whole).
fn ids(children: &[Node]) -> Option<Vec<Id<'_>>> {
    let mut seen = HashMap::with_capacity(children.len());
    let mut nth = 0;
    let mut out = Vec::with_capacity(children.len());
    for c in children {
        let id = match &c.key {
            Some(k) => Id::Key(k),
            None => {
                nth += 1;
                Id::Nth(nth - 1)
            }
        };
        if seen.insert(id, ()).is_some() {
            return None;
        }
        out.push(id);
    }
    Some(out)
}

fn node(old: &Node, new: &Node, path: &mut Vec<u32>, ops: &mut Vec<Op>) {
    let (Some(old_ids), Some(new_ids)) = (ids(&old.children), ids(&new.children)) else {
        if old != new {
            ops.push(Op::Replace { at: path.clone(), node: new.clone() });
        }
        return;
    };
    if old.key != new.key || old.kind() != new.kind() {
        ops.push(Op::Replace { at: path.clone(), node: new.clone() });
        return;
    }
    if old.element != new.element {
        ops.push(Op::Update { at: path.clone(), element: new.element.clone() });
    }
    children(old, new, &old_ids, &new_ids, path, ops);
}

fn children(old: &Node, new: &Node, old_ids: &[Id], new_ids: &[Id], path: &mut Vec<u32>, ops: &mut Vec<Op>) {
    if old_ids == new_ids {
        for (i, (o, n)) in old.children.iter().zip(&new.children).enumerate() {
            path.push(i as u32);
            node(o, n, path, ops);
            path.pop();
        }
        return;
    }
    let new_at: HashMap<Id, usize> = new_ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let old_at: HashMap<Id, usize> = old_ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();

    // Remove what is gone, from the end so earlier indices stay valid.
    for (i, id) in old_ids.iter().enumerate().rev() {
        if !new_at.contains_key(id) {
            ops.push(Op::Remove { parent: path.clone(), index: i as u32 });
        }
    }
    let mut cur: Vec<Id> = old_ids.iter().copied().filter(|id| new_at.contains_key(id)).collect();

    // Children whose new positions rise in their current order stay put; every other one moves once.
    let mut stable = vec![false; new_ids.len()];
    for i in rising(&cur.iter().map(|id| new_at[id]).collect::<Vec<_>>()) {
        stable[i] = true;
    }

    // From the back, put each child right before the one that follows it in the new order.
    for i in (0..new_ids.len()).rev() {
        if stable[i] {
            continue;
        }
        let id = new_ids[i];
        let before = match new_ids.get(i + 1) {
            Some(next) => cur.iter().position(|c| c == next).unwrap_or(cur.len()),
            None => cur.len(),
        };
        match cur.iter().position(|c| *c == id) {
            Some(from) => {
                let to = if from < before { before - 1 } else { before };
                if from != to {
                    ops.push(Op::Move { parent: path.clone(), from: from as u32, to: to as u32 });
                    cur.remove(from);
                    cur.insert(to, id);
                }
            }
            None => {
                ops.push(Op::Insert { parent: path.clone(), index: before as u32, node: new.children[i].clone() });
                cur.insert(before, id);
            }
        }
    }
    debug_assert!(cur == new_ids);

    for (i, id) in new_ids.iter().enumerate() {
        if let Some(&o) = old_at.get(id) {
            path.push(i as u32);
            node(&old.children[o], &new.children[i], path, ops);
            path.pop();
        }
    }
}

/// The values of one longest strictly rising run (not necessarily contiguous) in `seq`.
fn rising(seq: &[usize]) -> Vec<usize> {
    // tails[k]: index in `seq` of the smallest last value of a rising run of length k + 1.
    let mut tails: Vec<usize> = Vec::new();
    let mut prev = vec![usize::MAX; seq.len()];
    for (i, &v) in seq.iter().enumerate() {
        let k = tails.partition_point(|&t| seq[t] < v);
        if k > 0 {
            prev[i] = tails[k - 1];
        }
        if k == tails.len() {
            tails.push(i);
        } else {
            tails[k] = i;
        }
    }
    let mut out = Vec::with_capacity(tails.len());
    let mut i = tails.last().copied().unwrap_or(usize::MAX);
    while i != usize::MAX {
        out.push(seq[i]);
        i = prev[i];
    }
    out
}
