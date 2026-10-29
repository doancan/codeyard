//! Market search (docs/plugin-architecture.md §8): every extension of the user's registries, searched locally on the
//! name, id, description and categories of the signed index. No request is made while searching; the index was fetched
//! (at most daily) before. The text is lowered once when the catalogue is built, so a keystroke is a scan of plain
//! bytes: well under a millisecond for 10 000 extensions, run off the UI thread by the app.

use ext_manifest::{ExtId, Version};

use crate::yard::{Index, Publisher, Status};

/// One extension of one registry, as the Market lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// The registry's name (`extensions.registries`), so two registries listing one id stay apart.
    pub registry: String,
    pub id: ExtId,
    pub name: String,
    pub description: String,
    pub categories: Vec<String>,
    pub publisher: Publisher,
    /// The newest version a user may install (no pre-release); `None` when every version is yanked or revoked.
    pub latest: Option<Version>,
    /// The newest version is deprecated, with its replacement if named.
    pub deprecated: Option<Option<ExtId>>,
    /// Downloads of every version, updates included.
    pub downloads: u64,
}

/// Lowered text of one item, searched per keystroke.
struct Hay {
    name: String,
    id: String,
    description: String,
    categories: Vec<String>,
}

/// Every extension of the given indexes, ready to search.
pub struct Catalog {
    items: Vec<Item>,
    hay: Vec<Hay>,
    /// Item indexes, most downloaded first: the order of an empty search and of ties.
    popular: Vec<u32>,
}

/// How well an item matches a query; lower is better.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Rank {
    ExactId,
    ExactName,
    NamePrefix,
    NameWord,
    Name,
    Id,
    Category,
    Description,
}

impl Catalog {
    /// `(registry name, index)` pairs, in the user's order.
    pub fn new<'a>(indexes: impl IntoIterator<Item = (&'a str, &'a Index)>) -> Catalog {
        let mut items = Vec::new();
        for (registry, index) in indexes {
            for (id, e) in &index.extensions {
                let installable = e.versions.iter().rev().find(|r| matches!(r.status, Status::Ok | Status::Deprecated { .. }) && !r.is_pre_release());
                items.push(Item {
                    registry: registry.to_string(),
                    id: id.clone(),
                    name: e.name.clone(),
                    description: e.description.clone(),
                    categories: e.categories.clone(),
                    publisher: e.publisher.clone(),
                    latest: installable.map(|r| r.version.clone()),
                    deprecated: installable.and_then(|r| match &r.status {
                        Status::Deprecated { replacement } => Some(replacement.clone()),
                        _ => None,
                    }),
                    downloads: e.versions.iter().map(|r| r.downloads).sum(),
                });
            }
        }
        let hay = items
            .iter()
            .map(|i| Hay {
                name: i.name.to_lowercase(),
                id: i.id.as_str().to_string(),
                description: i.description.to_lowercase(),
                categories: i.categories.iter().map(|c| c.to_lowercase()).collect(),
            })
            .collect();
        let mut popular: Vec<u32> = (0..items.len() as u32).collect();
        popular.sort_by(|&a, &b| {
            let (a, b) = (&items[a as usize], &items[b as usize]);
            b.downloads.cmp(&a.downloads).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())).then_with(|| a.id.cmp(&b.id))
        });
        Catalog { items, hay, popular }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn item(&self, i: usize) -> &Item {
        &self.items[i]
    }

    pub fn find(&self, registry: &str, id: &ExtId) -> Option<usize> {
        self.items.iter().position(|i| i.registry == registry && &i.id == id)
    }

    /// Items matching every word of `query` (ignoring case) in the name, id, categories or description, best first:
    /// the id or name itself, then names that start with or contain the words, then ids, categories, descriptions;
    /// ties go to the most downloaded. An empty query lists everything, most downloaded first.
    pub fn search(&self, query: &str) -> Vec<usize> {
        let q = query.trim().to_lowercase();
        let words: Vec<&str> = q.split_whitespace().collect();
        if words.is_empty() {
            return self.popular.iter().map(|&i| i as usize).collect();
        }
        let mut hits: Vec<(Rank, u32)> = Vec::new();
        for (pos, &i) in self.popular.iter().enumerate() {
            if let Some(rank) = self.rank(&self.hay[i as usize], &q, &words) {
                hits.push((rank, pos as u32));
            }
        }
        // `pos` is the place in the popularity order, so sorting by (rank, pos) puts ties most downloaded first.
        hits.sort_unstable();
        hits.into_iter().map(|(_, pos)| self.popular[pos as usize] as usize).collect()
    }

    fn rank(&self, h: &Hay, q: &str, words: &[&str]) -> Option<Rank> {
        let mut worst = Rank::ExactId;
        for w in words {
            let r = if h.name.contains(w) {
                if h.name.starts_with(w) {
                    Rank::NamePrefix
                } else if h.name.split(|c: char| !c.is_alphanumeric()).any(|part| part.starts_with(w)) {
                    Rank::NameWord
                } else {
                    Rank::Name
                }
            } else if h.id.contains(w) {
                Rank::Id
            } else if h.categories.iter().any(|c| c.contains(w)) {
                Rank::Category
            } else if h.description.contains(w) {
                Rank::Description
            } else {
                return None;
            };
            worst = worst.max(r);
        }
        if h.id == q {
            Some(Rank::ExactId)
        } else if h.name == q {
            Some(Rank::ExactName)
        } else {
            Some(worst)
        }
    }
}

#[cfg(test)]
#[path = "market_tests.rs"]
mod tests;
