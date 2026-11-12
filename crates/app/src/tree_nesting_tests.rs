use super::*;
use proto::Kind;
use serde_json::json;

fn entry(name: &str, kind: Kind, target_kind: Option<Kind>) -> DirEntry {
    DirEntry { name: name.into(), kind, target_kind }
}

fn files(names: &[&str]) -> Vec<DirEntry> {
    names.iter().map(|n| entry(n, Kind::File, None)).collect()
}

/// `parent -> [children]` by name, parents in listing order.
fn nested(nesting: &Nesting, entries: &[DirEntry]) -> Vec<(String, Vec<String>)> {
    let groups = nesting.group(entries);
    let mut parents: Vec<usize> = groups.children.keys().copied().collect();
    parents.sort_unstable();
    parents.into_iter().map(|p| (entries[p].name.clone(), groups.children[&p].iter().map(|&c| entries[c].name.clone()).collect())).collect()
}

fn custom(value: serde_json::Value) -> Nesting {
    let (nesting, warnings) = Nesting::from_setting(true, &value);
    assert!(warnings.is_empty(), "{warnings:?}");
    nesting
}

#[test]
fn a_typescript_file_holds_its_tests_declarations_and_maps() {
    let entries = files(&["a.d.ts", "a.js.map", "a.test.ts", "a.ts", "b.ts"]);
    assert_eq!(nested(&Nesting::default(), &entries), [("a.ts".to_string(), vec!["a.d.ts".to_string(), "a.js.map".into(), "a.test.ts".into()])]);
}

#[test]
fn manifests_hold_their_lock_files() {
    let entries = files(&["Cargo.lock", "Cargo.toml", "README.md", "package-lock.json", "package.json", "yarn.lock"]);
    assert_eq!(
        nested(&Nesting::default(), &entries),
        [("Cargo.toml".to_string(), vec!["Cargo.lock".to_string()]), ("package.json".to_string(), vec!["package-lock.json".to_string(), "yarn.lock".into()])]
    );
}

#[test]
fn a_file_without_its_parent_stays_in_the_list() {
    let groups = Nesting::default().group(&files(&["Cargo.lock", "x.test.ts", "x.d.ts"]));
    assert!(groups.children.is_empty());
    assert!(groups.is_child.iter().all(|c| !c));
}

#[test]
fn nesting_off_groups_nothing() {
    let entries = files(&["Cargo.lock", "Cargo.toml"]);
    assert!(Nesting::off().group(&entries).children.is_empty());
    let (nesting, _) = Nesting::from_setting(false, &json!({ "Cargo.toml": "Cargo.lock" }));
    assert!(nesting.group(&entries).children.is_empty());
}

#[test]
fn folders_and_links_to_folders_never_nest() {
    let entries = vec![
        entry("Cargo.toml", Kind::File, None),
        entry("Cargo.lock", Kind::Dir, None),
        entry("a.ts", Kind::File, None),
        entry("a.d.ts", Kind::Symlink, Some(Kind::Dir)),
    ];
    assert!(Nesting::default().group(&entries).children.is_empty());
    // A folder named like a pattern is no parent either.
    let entries = vec![entry("Cargo.toml", Kind::Dir, None), entry("Cargo.lock", Kind::File, None)];
    assert!(Nesting::default().group(&entries).children.is_empty());
}

#[test]
fn a_link_to_a_file_nests_like_a_file() {
    let entries = vec![entry("Cargo.toml", Kind::File, None), entry("Cargo.lock", Kind::Symlink, Some(Kind::File))];
    assert_eq!(nested(&Nesting::default(), &entries), [("Cargo.toml".to_string(), vec!["Cargo.lock".to_string()])]);
    // A broken link is still a name in the folder.
    let entries = vec![entry("Cargo.toml", Kind::File, None), entry("Cargo.lock", Kind::Symlink, None)];
    assert_eq!(nested(&Nesting::default(), &entries).len(), 1);
}

#[test]
fn the_capture_keeps_the_text_the_star_matched() {
    let nesting = custom(json!({ "*.component.ts": "${capture}.component.html, ${capture}.component.css" }));
    let entries = files(&["app.component.css", "app.component.html", "app.component.ts", "nav.component.html", "other.component.html"]);
    assert_eq!(nested(&nesting, &entries), [("app.component.ts".to_string(), vec!["app.component.css".to_string(), "app.component.html".into()])]);
}

#[test]
fn the_parenthesis_spelling_of_the_capture_works_too() {
    let nesting = custom(json!({ "*.ts": "$(capture).js.map" }));
    assert_eq!(nested(&nesting, &files(&["a.js.map", "a.ts"])).len(), 1);
}

#[test]
fn children_can_be_a_list_of_texts() {
    let nesting = custom(json!({ "*.c": ["${capture}.h", "${capture}.o, ${capture}.d"] }));
    let entries = files(&["m.c", "m.d", "m.h", "m.o"]);
    assert_eq!(nested(&nesting, &entries), [("m.c".to_string(), vec!["m.d".to_string(), "m.h".into(), "m.o".into()])]);
}

#[test]
fn the_first_rule_decides_when_two_claim_a_file() {
    // `a.js` could hold `a.d.ts`, and `a.ts` could too: the `*.ts` rule comes first, so `a.js` is its child and holds nothing.
    let nesting = custom(json!({ "*.ts": "${capture}.js", "*.js": "${capture}.d.ts" }));
    let entries = files(&["a.d.ts", "a.js", "a.ts"]);
    assert_eq!(nested(&nesting, &entries), [("a.ts".to_string(), vec!["a.js".to_string()])]);
    let groups = nesting.group(&entries);
    assert!(!groups.is_child[0], "a.d.ts is not nested: its would-be parent is itself a child");
}

#[test]
fn a_parent_is_never_also_a_child() {
    // Chains and cycles are impossible: a file that holds children cannot be claimed.
    let nesting = custom(json!({ "a": "b", "b": "a, c" }));
    let entries = files(&["a", "b", "c"]);
    let groups = nesting.group(&entries);
    assert_eq!(nested(&nesting, &entries), [("a".to_string(), vec!["b".to_string()])]);
    assert!(groups.is_child[1] && !groups.is_child[0]);
}

#[test]
fn a_file_never_nests_under_itself() {
    let nesting = custom(json!({ "*": "${capture}" }));
    assert!(nesting.group(&files(&["a", "b"])).children.is_empty());
}

#[test]
fn matching_is_case_sensitive() {
    assert!(Nesting::default().group(&files(&["cargo.toml", "Cargo.lock"])).children.is_empty());
}

#[test]
fn unicode_names_nest() {
    let nesting = custom(json!({ "*.ts": "${capture}.d.ts" }));
    let entries = files(&["İstanbul.d.ts", "İstanbul.ts", "ırmak.d.ts", "ırmak.ts", "çay.ts"]);
    let got = nested(&nesting, &entries);
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].0, "İstanbul.ts");
    assert_eq!(got[1].0, "ırmak.ts");
}

#[test]
fn the_star_must_match_something() {
    let nesting = custom(json!({ "*.ts": "x.d.ts" }));
    assert!(nesting.group(&files(&[".ts", "x.d.ts"])).children.is_empty());
}

#[test]
fn a_pattern_with_two_stars_is_reported_and_ignored() {
    let (nesting, warnings) = Nesting::from_setting(true, &json!({ "*.*": "x", "Cargo.toml": "Cargo.lock" }));
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("*.*"));
    assert_eq!(nested(&nesting, &files(&["Cargo.lock", "Cargo.toml"])).len(), 1);
}

#[test]
fn a_value_of_the_wrong_type_is_reported() {
    let (nesting, warnings) = Nesting::from_setting(true, &json!({ "Cargo.toml": 5, "go.mod": "go.sum" }));
    assert_eq!(warnings.len(), 1);
    assert_eq!(nested(&nesting, &files(&["go.mod", "go.sum"])).len(), 1);
    let (_, warnings) = Nesting::from_setting(true, &json!(["x"]));
    assert_eq!(warnings.len(), 1);
}

#[test]
fn blank_patterns_and_empty_child_lists_make_no_rule() {
    let (nesting, warnings) = Nesting::from_setting(true, &json!({ "": "x", " ": "y", "a": "", "b": " , ", "c": [] }));
    assert!(warnings.is_empty());
    assert_eq!(nesting, Nesting::default());
}

#[test]
fn an_empty_object_keeps_the_defaults() {
    let (nesting, warnings) = Nesting::from_setting(true, &json!({}));
    assert!(warnings.is_empty());
    assert_eq!(nesting, Nesting::default());
}

#[test]
fn an_entry_replaces_the_default_of_the_same_pattern() {
    let nesting = custom(json!({ "Cargo.toml": "Cargo.lock, deny.toml" }));
    let entries = files(&["Cargo.lock", "Cargo.toml", "deny.toml", "go.mod", "go.sum"]);
    assert_eq!(
        nested(&nesting, &entries),
        [("Cargo.toml".to_string(), vec!["Cargo.lock".to_string(), "deny.toml".into()]), ("go.mod".to_string(), vec!["go.sum".to_string()])],
        "the other defaults stay"
    );
}

#[test]
fn false_or_an_empty_list_removes_a_default() {
    let entries = files(&["Cargo.lock", "Cargo.toml", "go.mod", "go.sum"]);
    for off in [json!(false), json!(null), json!(""), json!([])] {
        let nesting = custom(json!({ "Cargo.toml": off }));
        assert_eq!(nested(&nesting, &entries), [("go.mod".to_string(), vec!["go.sum".to_string()])]);
    }
}

#[test]
fn removing_a_pattern_that_is_not_there_is_harmless() {
    let (nesting, warnings) = Nesting::from_setting(true, &json!({ "nothing": false }));
    assert!(warnings.is_empty());
    assert_eq!(nesting, Nesting::default());
}

#[test]
fn one_parent_holds_at_most_a_hundred_children() {
    let mut names: Vec<String> = (0..150).map(|i| format!("lib{i}.d.ts")).collect();
    names.push("lib.ts".into());
    let entries: Vec<DirEntry> = names.iter().map(|n| entry(n, Kind::File, None)).collect();
    let nesting = custom(json!({ "lib.ts": names[..150].to_vec() }));
    let groups = nesting.group(&entries);
    assert_eq!(groups.children.values().next().map(Vec::len), Some(MAX_CHILDREN));
}

#[test]
fn a_large_folder_is_grouped_quickly() {
    let mut names = Vec::new();
    for i in 0..50_000 {
        names.push(format!("file{i}.ts"));
    }
    for i in 0..10_000 {
        names.push(format!("file{i}.d.ts"));
    }
    let entries: Vec<DirEntry> = names.iter().map(|n| entry(n, Kind::File, None)).collect();
    let started = std::time::Instant::now();
    let groups = Nesting::default().group(&entries);
    let took = started.elapsed();
    assert_eq!(groups.children.len(), 10_000);
    assert!(took < std::time::Duration::from_secs(2), "grouping 60,000 entries took {took:?}");
}

#[test]
fn the_defaults_parse() {
    let nesting = Nesting::default();
    assert!(nesting.enabled);
    assert_eq!(nesting.rules.len(), DEFAULT_PATTERNS.len());
}
