//! Everything the explorer filter looks through: the paths of a whole workspace, arranged the way the tree lists them. Built
//! once on a worker thread (sorting 50,000 paths takes tens of milliseconds); each keystroke then only scans the names. Pure.

use std::collections::{HashMap, HashSet};

use proto::{DirEntry, Kind};

use super::matching::{self, Query};
use super::{SortOrder, sort_entries};

/// One file or folder, in tree order.
pub struct Item {
    /// Path below the root, `/`-separated.
    pub rel: String,
    /// Where the last component starts in `rel`.
    name_at: usize,
    /// The name in comparison form (`matching::folded`).
    folded: String,
    pub depth: usize,
    pub is_dir: bool,
    /// Position of the folder that holds it; always smaller than its own.
    parent: Option<usize>,
}

impl Item {
    pub fn name(&self) -> &str {
        &self.rel[self.name_at..]
    }
}

#[derive(Default)]
pub struct Universe {
    items: Vec<Item>,
}

/// What a query leaves: the positions to show (matches and the folders around them) and how many are matches.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct Matches {
    pub shown: Vec<usize>,
    pub hits: usize,
}

impl Universe {
    /// Arrange `(path below the root, is it a folder)` pairs like the tree does. A path may come twice (from the file list and from a
    /// folder already opened); folders that only appear as part of a path are added.
    pub fn build(entries: impl IntoIterator<Item = (String, bool)>, order: SortOrder) -> Universe {
        let mut seen: HashSet<String> = HashSet::new();
        let mut by_dir: HashMap<String, Vec<DirEntry>> = HashMap::new();
        let mut add = |rel: &str, is_dir: bool, seen: &mut HashSet<String>| -> bool {
            if rel.is_empty() || !seen.insert(rel.to_string()) {
                return false;
            }
            let (dir, name) = rel.rsplit_once('/').unwrap_or(("", rel));
            let kind = if is_dir { Kind::Dir } else { Kind::File };
            by_dir.entry(dir.to_string()).or_default().push(DirEntry { name: name.to_string(), kind, target_kind: None });
            true
        };
        for (rel, is_dir) in entries {
            // A folder first seen as part of an earlier path may now be listed itself: it is already there.
            add(&rel, is_dir, &mut seen);
            let mut dir = rel.as_str();
            while let Some((parent, _)) = dir.rsplit_once('/') {
                if !add(parent, true, &mut seen) {
                    break;
                }
                dir = parent;
            }
        }
        for list in by_dir.values_mut() {
            sort_entries(list, order);
        }
        let mut universe = Universe::default();
        universe.arrange("", None, 0, &by_dir);
        universe
    }

    fn arrange(&mut self, dir: &str, parent: Option<usize>, depth: usize, by_dir: &HashMap<String, Vec<DirEntry>>) {
        let Some(list) = by_dir.get(dir) else { return };
        for entry in list {
            let rel = if dir.is_empty() { entry.name.clone() } else { format!("{dir}/{}", entry.name) };
            let name_at = rel.len() - entry.name.len();
            let at = self.items.len();
            self.items.push(Item { name_at, folded: matching::folded(&entry.name), rel: rel.clone(), depth, is_dir: entry.is_dir(), parent });
            if entry.is_dir() {
                self.arrange(&rel, Some(at), depth + 1, by_dir);
            }
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn item(&self, at: usize) -> &Item {
        &self.items[at]
    }

    /// The items whose name holds the query, and the folders that lead to them.
    pub fn matching(&self, query: &Query) -> Matches {
        let mut keep: Vec<bool> = self.items.iter().map(|item| query.matches_folded(&item.folded)).collect();
        let hits = keep.iter().filter(|k| **k).count();
        // A folder always comes before what it holds, so one backwards pass reaches every ancestor.
        for at in (0..keep.len()).rev() {
            if let (true, Some(parent)) = (keep[at], self.items[at].parent) {
                keep[parent] = true;
            }
        }
        Matches { shown: keep.iter().enumerate().filter(|(_, k)| **k).map(|(at, _)| at).collect(), hits }
    }
}

#[cfg(test)]
#[path = "tree_universe_tests.rs"]
mod tests;
