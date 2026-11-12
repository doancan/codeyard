use super::*;

fn files(paths: &[&str]) -> Vec<(String, bool)> {
    paths.iter().map(|p| (p.to_string(), false)).collect()
}

fn listing(universe: &Universe) -> Vec<String> {
    (0..universe.len()).map(|i| format!("{}{}", "  ".repeat(universe.item(i).depth), universe.item(i).name())).collect()
}

fn shown(universe: &Universe, text: &str) -> Vec<String> {
    let m = universe.matching(&Query::new(text).unwrap());
    m.shown.iter().map(|&i| universe.item(i).rel.clone()).collect()
}

#[test]
fn paths_are_arranged_like_the_tree_lists_them() {
    let u = Universe::build(files(&["z.txt", "src/main.rs", "src/lib.rs", "docs/a.md", "b.txt", "src/deep/x.rs"]), SortOrder::Default);
    assert_eq!(listing(&u), ["docs", "  a.md", "src", "  deep", "    x.rs", "  lib.rs", "  main.rs", "b.txt", "z.txt"]);
}

#[test]
fn the_sort_order_setting_applies() {
    let u = Universe::build(files(&["b.txt", "a/x.txt"]), SortOrder::FilesFirst);
    assert_eq!(listing(&u), ["b.txt", "a", "  x.txt"]);
    let u = Universe::build(files(&["b.txt", "a/x.txt", "c/y.txt"]), SortOrder::Mixed);
    assert_eq!(listing(&u), ["a", "  x.txt", "b.txt", "c", "  y.txt"]);
}

#[test]
fn numbers_sort_naturally() {
    let u = Universe::build(files(&["f10.txt", "f2.txt", "f1.txt"]), SortOrder::Default);
    assert_eq!(listing(&u), ["f1.txt", "f2.txt", "f10.txt"]);
}

#[test]
fn a_path_listed_twice_appears_once_and_folders_listed_by_themselves_are_kept() {
    let entries = vec![("src/a.rs".to_string(), false), ("src".to_string(), true), ("src/a.rs".to_string(), false), ("empty".to_string(), true)];
    let u = Universe::build(entries, SortOrder::Default);
    assert_eq!(listing(&u), ["empty", "src", "  a.rs"]);
}

#[test]
fn an_empty_list_is_an_empty_universe() {
    assert_eq!(Universe::build(Vec::new(), SortOrder::Default).len(), 0);
}

#[test]
fn a_match_keeps_the_folders_that_lead_to_it() {
    let u = Universe::build(files(&["src/deep/needle.rs", "src/other.rs", "docs/a.md"]), SortOrder::Default);
    assert_eq!(shown(&u, "needle"), ["src", "src/deep", "src/deep/needle.rs"]);
    assert_eq!(u.matching(&Query::new("needle").unwrap()).hits, 1);
}

#[test]
fn a_folder_that_matches_is_shown_without_what_does_not() {
    let u = Universe::build(files(&["src/a.rs", "src/b.rs", "docs/a.md"]), SortOrder::Default);
    assert_eq!(shown(&u, "src"), ["src"]);
    assert_eq!(shown(&u, "a"), ["docs", "docs/a.md", "src", "src/a.rs"]);
}

#[test]
fn nothing_matching_shows_nothing() {
    let u = Universe::build(files(&["a.rs"]), SortOrder::Default);
    assert_eq!(u.matching(&Query::new("zzz").unwrap()), Matches::default());
}

#[test]
fn matching_ignores_case_and_the_turkish_i() {
    let u = Universe::build(files(&["Notlar/İstanbul.md", "Notlar/ırmak.md", "Notlar/ankara.md", "ISTANBUL.txt"]), SortOrder::Default);
    assert_eq!(shown(&u, "istanbul"), ["Notlar", "Notlar/İstanbul.md", "ISTANBUL.txt"]);
    assert_eq!(shown(&u, "IRMAK"), ["Notlar", "Notlar/ırmak.md"]);
}

#[test]
fn only_the_name_is_searched_not_the_folders_above_it() {
    let u = Universe::build(files(&["needle/a.rs", "other/needle.rs"]), SortOrder::Default);
    assert_eq!(shown(&u, "needle"), ["needle", "other", "other/needle.rs"]);
}

#[test]
fn fifty_thousand_paths_are_built_and_scanned_quickly() {
    let entries: Vec<(String, bool)> = (0..50_000).map(|i| (format!("pkg{}/mod{}/file{i}.rs", i % 50, i % 500), false)).collect();
    let started = std::time::Instant::now();
    let u = Universe::build(entries, SortOrder::Default);
    let built = started.elapsed();
    let started = std::time::Instant::now();
    let m = u.matching(&Query::new("file4999").unwrap());
    let scanned = started.elapsed();
    assert_eq!(m.hits, 11);
    assert!(built < std::time::Duration::from_secs(5), "build took {built:?}");
    assert!(scanned < std::time::Duration::from_millis(500), "scan took {scanned:?}");
}
