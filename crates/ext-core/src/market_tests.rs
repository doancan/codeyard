use std::collections::BTreeMap;

use crate::testing::github;
use crate::yard::{Entry, FORMAT, Index, Release, Status};

use super::*;

fn release(version: &str, status: Status, downloads: u64) -> Release {
    Release {
        version: Version::parse(version).unwrap(),
        digest: "00".repeat(32),
        sha256: "00".repeat(32),
        size: 1,
        permissions: "00".repeat(32),
        code: false,
        published: 0,
        status,
        downloads,
        immediate: false,
        details: None,
    }
}

fn entry(name: &str, description: &str, categories: &[&str], versions: Vec<Release>) -> Entry {
    Entry {
        name: name.into(),
        description: description.into(),
        categories: categories.iter().map(|c| c.to_string()).collect(),
        publisher: github(7, 42),
        versions,
    }
}

fn index(entries: Vec<(&str, Entry)>) -> Index {
    let extensions: BTreeMap<ExtId, Entry> = entries.into_iter().map(|(id, e)| (ExtId::parse(id).unwrap(), e)).collect();
    Index { format: FORMAT, registry: "t.test".into(), version: 1, expires: 0, extensions }
}

fn ids(c: &Catalog, q: &str) -> Vec<String> {
    c.search(q).into_iter().map(|i| c.item(i).id.to_string()).collect()
}

fn sample() -> Index {
    index(vec![
        ("you.night", entry("Night Owl", "A dark theme for late hours.", &["themes"], vec![release("1.0.0", Status::Ok, 50)])),
        ("acme.rust-snippets", entry("Rust Snippets", "Snippets for Rust.", &["snippets"], vec![release("2.0.0", Status::Ok, 900)])),
        ("acme.owl", entry("Owl", "Keymap like the owl editor.", &["keymaps"], vec![release("0.1.0", Status::Ok, 5)])),
        ("night.tools", entry("Tools", "Helpers.", &["other"], vec![release("1.0.0", Status::Ok, 1)])),
        ("you.dusk", entry("Dusk", "Evening colors, a night variant.", &["themes"], vec![release("1.0.0", Status::Ok, 7)])),
    ])
}

#[test]
fn an_empty_query_lists_everything_most_downloaded_first() {
    let i = sample();
    let c = Catalog::new([("t", &i)]);
    assert_eq!(ids(&c, "  "), ["acme.rust-snippets", "you.night", "you.dusk", "acme.owl", "night.tools"]);
}

#[test]
fn names_rank_above_ids_categories_and_descriptions() {
    let i = sample();
    let c = Catalog::new([("t", &i)]);
    // Name prefix, then id, then description; within a rank the more downloaded first.
    assert_eq!(ids(&c, "night"), ["you.night", "night.tools", "you.dusk"]);
    assert_eq!(ids(&c, "owl"), ["acme.owl", "you.night"], "the exact name first, then a word of a name");
    assert_eq!(ids(&c, "THEMES"), ["you.night", "you.dusk"], "categories, ignoring case");
    assert_eq!(ids(&c, "acme.owl"), ["acme.owl"], "the exact id");
}

#[test]
fn every_word_must_match_somewhere() {
    let i = sample();
    let c = Catalog::new([("t", &i)]);
    assert_eq!(ids(&c, "night dark"), ["you.night"]);
    assert_eq!(ids(&c, "rust owl"), Vec::<String>::new());
    assert_eq!(ids(&c, "zzz"), Vec::<String>::new());
}

#[test]
fn the_listed_version_is_the_newest_installable_one() {
    let i = index(vec![
        (
            "you.a",
            entry(
                "A",
                "",
                &[],
                vec![
                    release("1.0.0", Status::Ok, 1),
                    release("1.1.0", Status::Deprecated { replacement: Some(ExtId::parse("you.b").unwrap()) }, 2),
                    release("1.2.0", Status::Yanked, 3),
                    release("2.0.0-beta.1", Status::Ok, 4),
                ],
            ),
        ),
        ("you.b", entry("B", "", &[], vec![release("1.0.0", Status::Revoked { reason: "Malware.".into() }, 0)])),
    ]);
    let c = Catalog::new([("t", &i)]);
    let a = c.item(c.find("t", &ExtId::parse("you.a").unwrap()).unwrap());
    assert_eq!(a.latest.as_ref().map(ToString::to_string).as_deref(), Some("1.1.0"));
    assert_eq!(a.deprecated, Some(Some(ExtId::parse("you.b").unwrap())));
    assert_eq!(a.downloads, 10, "every version, updates included");
    let b = c.item(c.find("t", &ExtId::parse("you.b").unwrap()).unwrap());
    assert_eq!(b.latest, None);
}

#[test]
fn two_registries_listing_one_id_stay_apart() {
    let (a, b) = (sample(), sample());
    let c = Catalog::new([("one", &a), ("two", &b)]);
    assert_eq!(c.len(), 10);
    let hits = c.search("owl editor");
    assert_eq!(hits.len(), 2);
    let regs: Vec<&str> = hits.iter().map(|&i| c.item(i).registry.as_str()).collect();
    assert!(regs.contains(&"one") && regs.contains(&"two"));
}

/// The perf row (`market.search_ms`, 16 ms ceiling) measures the app end to end; this keeps the search itself far
/// below it. Timed only in optimised builds.
#[test]
fn ten_thousand_extensions_search_in_well_under_a_frame() {
    let words = ["night", "owl", "rust", "theme", "snippet", "keymap", "dark", "light", "go", "python", "solar", "forest"];
    let entries: Vec<(String, Entry)> = (0..10_000)
        .map(|n| {
            let w = |k: usize| words[(n * 7 + k * 3) % words.len()];
            let e = entry(
                &format!("{} {} {n}", w(0), w(1)),
                &format!("A {} extension with {} and {}.", w(2), w(3), w(4)),
                &[w(5)],
                vec![release("1.0.0", Status::Ok, (n * 37 % 1000) as u64)],
            );
            (format!("p{}.ext{n}", n % 300), e)
        })
        .collect();
    let i = index(entries.iter().map(|(id, e)| (id.as_str(), e.clone())).collect());
    let c = Catalog::new([("t", &i)]);
    assert_eq!(c.len(), 10_000);
    let mut worst = std::time::Duration::ZERO;
    for q in ["n", "ni", "nig", "nigh", "night", "night o", "night ow", "night owl", "rust snip", "ext42", "zzz", ""] {
        let t = std::time::Instant::now();
        let hits = c.search(q);
        worst = worst.max(t.elapsed());
        assert!(q != "zzz" || hits.is_empty());
    }
    if !cfg!(debug_assertions) {
        assert!(worst < std::time::Duration::from_millis(4), "slowest search took {worst:?}");
    }
}
