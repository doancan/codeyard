//! Validated editor-group geometry, independent of documents, windows and rendering.
//! Group identifiers remain stable when neighboring groups split or close.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

pub type GroupId = u8;
pub const MAX_GROUPS: usize = 32;
const VERSION: u8 = 1;
const MAX_JSON_BYTES: usize = 32 * 1024;
const MIN_RATIO: f32 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    fn axis(self) -> Axis {
        match self {
            Self::Left | Self::Right => Axis::Horizontal,
            Self::Up | Self::Down => Axis::Vertical,
        }
    }
    fn before(self) -> bool {
        matches!(self, Self::Left | Self::Up)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Node {
    Group { id: GroupId },
    Split { id: u8, axis: Axis, ratio: f32, first: Box<Node>, second: Box<Node> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    version: u8,
    root: Node,
    focused: GroupId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    MissingGroup,
    GroupLimit,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Default for Layout {
    fn default() -> Self {
        Self { version: VERSION, root: Node::Group { id: 0 }, focused: 0 }
    }
}

impl Layout {
    pub fn root(&self) -> &Node {
        &self.root
    }
    pub fn focused(&self) -> GroupId {
        self.focused
    }

    pub fn groups(&self) -> Vec<GroupId> {
        fn visit(node: &Node, out: &mut Vec<GroupId>) {
            match node {
                Node::Group { id } => out.push(*id),
                Node::Split { first, second, .. } => {
                    visit(first, out);
                    visit(second, out);
                }
            }
        }
        let mut groups = Vec::new();
        visit(&self.root, &mut groups);
        groups
    }

    pub fn focus(&mut self, group: GroupId) -> bool {
        if self.groups().contains(&group) {
            self.focused = group;
            true
        } else {
            false
        }
    }

    pub fn split(&mut self, group: GroupId, direction: Direction) -> Result<GroupId, Error> {
        let groups = self.groups();
        if !groups.contains(&group) {
            return Err(Error::MissingGroup);
        }
        if groups.len() >= MAX_GROUPS {
            return Err(Error::GroupLimit);
        }
        let new = (0..MAX_GROUPS as u8).find(|id| !groups.contains(id)).ok_or(Error::GroupLimit)?;
        let mut used_splits = HashSet::new();
        collect_split_ids(&self.root, &mut used_splits);
        let split_id = (0..MAX_GROUPS as u8).find(|id| !used_splits.contains(id)).ok_or(Error::GroupLimit)?;
        fn replace(node: &mut Node, group: GroupId, new: GroupId, split_id: u8, direction: Direction) -> bool {
            match node {
                Node::Group { id } if *id == group => {
                    let (old, added) = (Box::new(Node::Group { id: group }), Box::new(Node::Group { id: new }));
                    let (first, second) = if direction.before() { (added, old) } else { (old, added) };
                    *node = Node::Split { id: split_id, axis: direction.axis(), ratio: 0.5, first, second };
                    true
                }
                Node::Split { first, second, .. } => replace(first, group, new, split_id, direction) || replace(second, group, new, split_id, direction),
                _ => false,
            }
        }
        replace(&mut self.root, group, new, split_id, direction);
        self.focused = new;
        Ok(new)
    }

    /// Remove an empty group after its caller has moved or closed its tabs. The last
    /// group stays, so there is always a destination for a newly opened document.
    pub fn remove(&mut self, group: GroupId) -> bool {
        let groups = self.groups();
        if groups.len() == 1 || !groups.contains(&group) {
            return false;
        }
        fn remove(node: &mut Node, group: GroupId) -> bool {
            if let Node::Split { first, second, .. } = node {
                if matches!(first.as_ref(), Node::Group { id } if *id == group) {
                    *node = second.as_ref().clone();
                    return true;
                }
                if matches!(second.as_ref(), Node::Group { id } if *id == group) {
                    *node = first.as_ref().clone();
                    return true;
                }
                return remove(first, group) || remove(second, group);
            }
            false
        }
        remove(&mut self.root, group);
        if self.focused == group {
            let index = groups.iter().position(|id| *id == group).unwrap_or(0);
            let remaining = self.groups();
            self.focused = remaining[index.min(remaining.len() - 1)];
        }
        true
    }

    pub fn set_ratio(&mut self, split_id: u8, ratio: f32) -> bool {
        if !ratio.is_finite() {
            return false;
        }
        fn set(node: &mut Node, split_id: u8, value: f32) -> bool {
            match node {
                Node::Split { id, ratio, .. } if *id == split_id => {
                    *ratio = value;
                    true
                }
                Node::Split { first, second, .. } => set(first, split_id, value) || set(second, split_id, value),
                _ => false,
            }
        }
        set(&mut self.root, split_id, ratio.clamp(MIN_RATIO, 1. - MIN_RATIO))
    }

    pub fn rects(&self) -> Vec<(GroupId, Rect)> {
        fn visit(node: &Node, bounds: Rect, out: &mut Vec<(GroupId, Rect)>) {
            match node {
                Node::Group { id } => out.push((*id, bounds)),
                Node::Split { axis, ratio, first, second, .. } => {
                    let (a, b) = match axis {
                        Axis::Horizontal => (
                            Rect { width: bounds.width * ratio, ..bounds },
                            Rect { x: bounds.x + bounds.width * ratio, width: bounds.width * (1. - ratio), ..bounds },
                        ),
                        Axis::Vertical => (
                            Rect { height: bounds.height * ratio, ..bounds },
                            Rect { y: bounds.y + bounds.height * ratio, height: bounds.height * (1. - ratio), ..bounds },
                        ),
                    };
                    visit(first, a, out);
                    visit(second, b, out);
                }
            }
        }
        let mut out = Vec::new();
        visit(&self.root, Rect { x: 0., y: 0., width: 1., height: 1. }, &mut out);
        out
    }

    pub fn neighbor(&self, direction: Direction) -> Option<GroupId> {
        let rects = self.rects();
        let from = rects.iter().find(|(id, _)| *id == self.focused)?.1;
        let horizontal = direction.axis() == Axis::Horizontal;
        let axis = |r: Rect| {
            if horizontal { (r.x + r.width / 2., r.y + r.height / 2., r.y, r.height) } else { (r.y + r.height / 2., r.x + r.width / 2., r.x, r.width) }
        };
        let (f, center, start, length) = axis(from);
        rects
            .into_iter()
            .filter_map(|(id, rect)| {
                if id == self.focused {
                    return None;
                }
                let (to, other_center, other_start, other_length) = axis(rect);
                let forward = if direction.before() { f - to } else { to - f };
                if forward <= 0. {
                    return None;
                }
                let aligned = start < other_start + other_length && other_start < start + length;
                Some((id, !aligned, forward, (center - other_center).abs()))
            })
            .min_by(|a, b| a.1.cmp(&b.1).then_with(|| a.2.total_cmp(&b.2)).then_with(|| a.3.total_cmp(&b.3)).then_with(|| a.0.cmp(&b.0)))
            .map(|item| item.0)
    }

    pub fn to_json(&self) -> Result<String, Error> {
        serde_json::to_string(self).map_err(|_| Error::Invalid)
    }

    pub fn from_json(text: &str) -> Result<Self, Error> {
        if text.len() > MAX_JSON_BYTES {
            return Err(Error::Invalid);
        }
        let layout: Self = serde_json::from_str(text).map_err(|_| Error::Invalid)?;
        layout.validate()?;
        Ok(layout)
    }

    fn validate(&self) -> Result<(), Error> {
        fn visit(node: &Node, depth: usize, groups: &mut HashSet<u8>, splits: &mut HashSet<u8>) -> bool {
            if depth > MAX_GROUPS {
                return false;
            }
            match node {
                Node::Group { id } => usize::from(*id) < MAX_GROUPS && groups.insert(*id),
                Node::Split { id, axis: _, ratio, first, second } => {
                    usize::from(*id) < MAX_GROUPS
                        && splits.insert(*id)
                        && ratio.is_finite()
                        && (MIN_RATIO..=1. - MIN_RATIO).contains(ratio)
                        && visit(first, depth + 1, groups, splits)
                        && visit(second, depth + 1, groups, splits)
                }
            }
        }
        let mut groups = HashSet::new();
        let mut splits = HashSet::new();
        if self.version != VERSION || !visit(&self.root, 0, &mut groups, &mut splits) || groups.len() > MAX_GROUPS || !groups.contains(&self.focused) {
            Err(Error::Invalid)
        } else {
            Ok(())
        }
    }
}

fn collect_split_ids(node: &Node, out: &mut HashSet<u8>) {
    if let Node::Split { id, first, second, .. } = node {
        out.insert(*id);
        collect_split_ids(first, out);
        collect_split_ids(second, out);
    }
}

#[cfg(test)]
#[path = "editor_layout_tests.rs"]
mod tests;
