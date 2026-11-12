//! Tests of the dependency logic. The network is a fake `Fetch`; the sparse-index samples in `testdata/cargo_deps/` are real
//! lines captured from index.crates.io (serde, tokio, cc) and real answers of the crates.io API.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use ext_api::value::Tone;
use semver::Version;

use super::cache::{DiskCache, Stored};
use super::config::{self, Source};
use super::context::{Ctx, context_at, kind_at};
use super::edit::{self, EditError};
use super::fetch::{Fetch, FetchError, Response};
use super::hints::{self, Lookups};
use super::index::{self, CrateInfo};
use super::items;
use super::lock::Lock;
use super::manifest::{self, Kind, Source as DepSource};
use super::registry::{Endpoints, Failure, Lookup, Registry, valid_crate_name};
use super::version::{Status, Update, classify, rewrite, updated_requirement};

pub(crate) const SERDE_INDEX: &str = include_str!("../testdata/cargo_deps/serde.idx");
pub(crate) const TOKIO_INDEX: &str = include_str!("../testdata/cargo_deps/tokio.idx");
pub(crate) const CC_INDEX: &str = include_str!("../testdata/cargo_deps/cc.idx");
pub(crate) const SEARCH: &str = include_str!("../testdata/cargo_deps/search-serde.json");
pub(crate) const CRATE_JSON: &str = include_str!("../testdata/cargo_deps/crate-serde.json");
pub(crate) const VERSION_JSON: &str = include_str!("../testdata/cargo_deps/version-serde-1.0.219.json");

fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

fn serde() -> CrateInfo {
    index::parse_index("serde", SERDE_INDEX)
}

// --- a fake registry -------------------------------------------------------------------------------------------------

#[derive(Default)]
pub(crate) struct Fake {
    routes: Mutex<HashMap<String, Vec<Result<Response, FetchError>>>>,
    pub(crate) calls: Mutex<Vec<(String, Option<String>)>>,
    in_flight: AtomicUsize,
    peak: AtomicUsize,
    delay_ms: u64,
}

impl Fake {
    pub(crate) fn route(&self, url: &str, answer: Result<Response, FetchError>) {
        self.routes.lock().unwrap().entry(url.to_string()).or_default().push(answer);
    }

    /// Replace what the URL answers from now on.
    pub(crate) fn set(&self, url: &str, answer: Result<Response, FetchError>) {
        self.routes.lock().unwrap().insert(url.to_string(), vec![answer]);
    }

    pub(crate) fn ok(&self, url: &str, body: &str, etag: &str) {
        self.route(url, Ok(Response { status: 200, etag: Some(etag.into()), body: body.as_bytes().to_vec() }));
    }

    pub(crate) fn count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

impl Fetch for Fake {
    fn get(&self, url: &str, etag: Option<&str>) -> Result<Response, FetchError> {
        self.calls.lock().unwrap().push((url.to_string(), etag.map(str::to_string)));
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        if self.delay_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(self.delay_ms));
        }
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        let mut routes = self.routes.lock().unwrap();
        match routes.get_mut(url) {
            Some(list) if list.len() > 1 => list.remove(0),
            Some(list) => list[0].clone(),
            None => Ok(Response { status: 404, etag: None, body: Vec::new() }),
        }
    }
}

fn registry(fake: &Arc<Fake>, dir: Option<std::path::PathBuf>, now: Arc<AtomicUsize>) -> Arc<Registry> {
    let clock = Arc::new(move || now.load(Ordering::SeqCst) as u64);
    Registry::with_clock(fake.clone(), DiskCache::new(dir), Endpoints::default(), clock)
}

const SERDE_URL: &str = "https://index.crates.io/se/rd/serde";

fn temp_dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cy-deps-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&d).ok();
    std::fs::create_dir_all(&d).unwrap();
    d
}

// --- sparse index ----------------------------------------------------------------------------------------------------

#[test]
fn index_paths_follow_the_sparse_layout() {
    assert_eq!(index::index_path("a"), "1/a");
    assert_eq!(index::index_path("cc"), "2/cc");
    assert_eq!(index::index_path("log"), "3/l/log");
    assert_eq!(index::index_path("serde"), "se/rd/serde");
    assert_eq!(index::index_path("Tokio-Util"), "to/ki/tokio-util");
}

#[test]
fn real_serde_lines_parse_with_yanked_and_prerelease_versions() {
    let info = serde();
    assert_eq!(info.latest_stable().unwrap().version, v("1.0.229"));
    let yanked: Vec<String> = info.versions.iter().filter(|x| x.yanked).map(|x| x.version.to_string()).collect();
    assert_eq!(yanked, ["0.7.6", "1.0.31", "1.0.95"]);
    assert!(info.versions.iter().any(|x| x.version == v("0.9.0-rc1")));
    // Sorted oldest first by semantic order, not by text.
    assert!(info.versions.windows(2).all(|w| w[0].version < w[1].version));
    let features = info.latest().unwrap().features.as_ref().unwrap();
    assert_eq!(features["default"], ["std"]);
    assert_eq!(features["derive"], ["serde_derive"]);
    // `serde_derive` is optional but named by a feature through `serde_derive`, so it is no extra implicit feature... it is: no `dep:`.
    assert!(features.contains_key("serde_derive") || features["derive"] == ["serde_derive"]);
}

#[test]
fn offered_versions_are_newest_first_with_releases_before_prereleases_and_yanked_last() {
    let order: Vec<String> = serde().offered().iter().map(|x| x.version.to_string()).collect();
    assert_eq!(order[0], "1.0.229");
    let pos = |s: &str| order.iter().position(|x| x == s).unwrap();
    assert!(pos("1.0.228") < pos("1.0.227") && pos("1.0.227") < pos("0.2.0"));
    // The pre-releases come after every release that is not yanked.
    assert!(pos("0.2.0") < pos("0.9.0-rc1") && pos("0.9.0-rc1") < pos("0.8.0-rc1"));
    // Yanked releases are last of all.
    assert!(pos("0.8.0-rc1") < pos("1.0.95") && pos("1.0.95") < pos("1.0.31"));
}

#[test]
fn tokio_features2_and_implicit_optional_dependency_features_are_listed() {
    let info = index::parse_index("tokio", TOKIO_INDEX);
    let features = info.latest().unwrap().features.as_ref().unwrap();
    assert!(features.contains_key("full") && features.contains_key("rt-multi-thread"));
    assert_eq!(features["full"].first().map(String::as_str), Some("fs"));
    // `features2` entries (the `dep:` syntax) are merged in.
    assert!(features.values().flatten().any(|e| e.starts_with("dep:")));
}

#[test]
fn a_yanked_latest_is_never_offered_as_the_latest() {
    let info = index::parse_index("cc", CC_INDEX);
    let yanked: Vec<_> = info.versions.iter().filter(|x| x.yanked).collect();
    assert!(!yanked.is_empty());
    assert!(!info.latest().unwrap().yanked);
}

#[test]
fn bad_index_lines_are_skipped_not_fatal() {
    let body = "not json\n{\"vers\":\"banana\"}\n{\"name\":\"x\",\"vers\":\"1.0.0\",\"deps\":[],\"features\":{},\"yanked\":false}\n\n";
    assert_eq!(index::parse_index("x", body).versions.len(), 1);
}

#[test]
fn the_cache_form_round_trips() {
    let info = index::parse_index("tokio", TOKIO_INDEX);
    assert_eq!(index::from_json(&index::to_json(&info)), Some(info));
}

#[test]
fn only_the_newest_versions_keep_their_feature_lists() {
    let body: String = (0..40).map(|i| format!("{{\"name\":\"x\",\"vers\":\"1.0.{i}\",\"deps\":[],\"features\":{{\"a\":[]}},\"yanked\":false}}\n")).collect();
    let info = index::parse_index("x", &body);
    assert_eq!(info.versions.iter().filter(|v| v.features.is_some()).count(), 30);
    assert!(info.versions[0].features.is_none() && info.versions[39].features.is_some());
}

// --- requirements ----------------------------------------------------------------------------------------------------

#[test]
fn classify_up_to_date_outdated_yanked_and_no_match() {
    let info = serde();
    let latest = v("1.0.229");
    assert_eq!(classify("1", &info, None), Some(Status::UpToDate { latest: latest.clone() }));
    assert_eq!(classify("1.0.100", &info, None), Some(Status::UpToDate { latest: latest.clone() }));
    assert_eq!(classify("^1.0", &info, None), Some(Status::UpToDate { latest: latest.clone() }));
    assert_eq!(classify("0.2", &info, None), Some(Status::Outdated { latest: latest.clone() }));
    assert_eq!(classify("=1.0.226", &info, None), Some(Status::Outdated { latest: latest.clone() }));
    assert_eq!(classify("=1.0.95", &info, None), Some(Status::Yanked { version: v("1.0.95") }));
    assert_eq!(classify("9", &info, None), Some(Status::NoMatch { latest: Some(latest) }));
    assert_eq!(classify("not a version", &info, None), None);
}

#[test]
fn a_locked_yanked_version_is_flagged_even_when_the_requirement_allows_newer_ones() {
    let info = serde();
    assert_eq!(classify("1", &info, Some(&v("1.0.31"))), Some(Status::Yanked { version: v("1.0.31") }));
    assert_eq!(classify("1", &info, Some(&v("1.0.200"))), Some(Status::UpToDate { latest: v("1.0.229") }));
}

#[test]
fn a_prerelease_requirement_is_matched_by_cargo_rules() {
    let info = serde();
    assert!(matches!(classify("0.9.0-rc1", &info, None), Some(Status::Outdated { .. })));
}

#[test]
fn rewriting_keeps_operator_spacing_and_precision() {
    let to = v("2.3.4");
    for (old, new) in [
        ("1", "2"),
        ("1.0", "2.3"),
        ("1.0.100", "2.3.4"),
        ("^1.0", "^2.3"),
        ("~1.2", "~2.3"),
        ("=1.2.3", "=2.3.4"),
        (">= 1.2", ">= 2.3"),
        ("1.*", "2.*"),
        ("1.2.*", "2.3.*"),
        (">=1, <2", "2.3.4"),
        ("<2", "2.3.4"),
        ("1.0.0-beta.1", "2.3.4"),
    ] {
        assert_eq!(rewrite(old, &to).as_deref(), Some(new), "{old}");
    }
    assert_eq!(rewrite("*", &to), None);
    assert_eq!(rewrite("1", &v("2.0.0-rc.1")).as_deref(), Some("2.0.0-rc.1"));
}

#[test]
fn compatible_updates_raise_the_written_minimum_but_never_change_the_major() {
    let info = serde();
    assert_eq!(updated_requirement("1.0.100", &info, Update::Compatible).as_deref(), Some("1.0.229"));
    assert_eq!(updated_requirement("1", &info, Update::Compatible), None);
    assert_eq!(updated_requirement("0.9", &info, Update::Compatible), None, "0.9.0-rc1 is a pre-release and 0.9 has no release");
    assert_eq!(updated_requirement("1.0.100", &info, Update::Latest).as_deref(), Some("1.0.229"));
}

#[test]
fn latest_updates_cross_a_major_version() {
    let body = "{\"name\":\"y\",\"vers\":\"1.4.0\",\"deps\":[],\"features\":{},\"yanked\":false}\n{\"name\":\"y\",\"vers\":\"2.1.5\",\"deps\":[],\"features\":{},\"yanked\":false}\n{\"name\":\"y\",\"vers\":\"3.0.0\",\"deps\":[],\"features\":{},\"yanked\":true}\n";
    let info = index::parse_index("y", body);
    assert_eq!(updated_requirement("1.2", &info, Update::Latest).as_deref(), Some("2.1"));
    assert_eq!(updated_requirement("1.2", &info, Update::Compatible).as_deref(), Some("1.4"));
}

// --- manifest --------------------------------------------------------------------------------------------------------

const MANIFEST: &str = r#"[package]
name = "demo"

[dependencies]
serde = "1.0"
tokio = { version = "1", features = ["rt", "macros"] }
local = { path = "../local", version = "0.1" }
g = { git = "https://example.com/g.git" }
inherited = { workspace = true, features = ["x"] }
other = { version = "2", registry = "company" }
renamed = { package = "real-name", version = "0.3" }
dotted.version = "0.4"

[dependencies.regex]
version = "1.5"
features = ["unicode"]

[dev-dependencies]
tempfile = "3"

[build-dependencies]
cc = '1.0'

[target.'cfg(unix)'.dependencies]
libc = "0.2"

[workspace.dependencies]
anyhow = "1"
"#;

fn dep<'a>(m: &'a manifest::Manifest, key: &str) -> &'a manifest::Dep {
    m.deps.iter().find(|d| d.key == key).unwrap_or_else(|| panic!("no dependency {key}"))
}

#[test]
fn every_dependency_form_is_found_with_its_requirement_span() {
    let m = manifest::parse(MANIFEST).unwrap();
    let keys: Vec<&str> = m.deps.iter().map(|d| d.key.as_str()).collect();
    assert_eq!(keys, ["serde", "tokio", "local", "g", "inherited", "other", "renamed", "dotted", "regex", "tempfile", "cc", "libc", "anyhow"]);
    for (key, req) in [
        ("serde", "1.0"),
        ("tokio", "1"),
        ("local", "0.1"),
        ("renamed", "0.3"),
        ("dotted", "0.4"),
        ("regex", "1.5"),
        ("tempfile", "3"),
        ("cc", "1.0"),
        ("libc", "0.2"),
        ("anyhow", "1"),
    ] {
        let r = dep(&m, key).req.clone().unwrap_or_else(|| panic!("{key} has no requirement"));
        assert_eq!(r.text, req, "{key}");
        assert_eq!(&MANIFEST[r.span.clone()], req, "{key} span");
    }
    assert!(dep(&m, "inherited").req.is_none());
}

#[test]
fn kinds_sources_targets_and_the_workspace_table_are_told_apart() {
    let m = manifest::parse(MANIFEST).unwrap();
    assert_eq!(dep(&m, "serde").source, DepSource::CratesIo);
    assert_eq!(dep(&m, "local").source, DepSource::Path);
    assert_eq!(dep(&m, "g").source, DepSource::Git);
    assert_eq!(dep(&m, "inherited").source, DepSource::Workspace);
    assert_eq!(dep(&m, "other").source, DepSource::Registry("company".into()));
    assert_eq!((dep(&m, "tempfile").kind, dep(&m, "cc").kind), (Kind::Dev, Kind::Build));
    assert_eq!(dep(&m, "libc").target.as_deref(), Some("cfg(unix)"));
    assert!(dep(&m, "anyhow").in_workspace_table && !dep(&m, "serde").in_workspace_table);
    assert_eq!(dep(&m, "renamed").package, "real-name");
}

#[test]
fn only_crates_io_dependencies_with_a_requirement_are_looked_up() {
    let m = manifest::parse(MANIFEST).unwrap();
    let names: Vec<&str> = hints::wanted(&m).iter().map(|d| d.key.as_str()).collect();
    assert_eq!(names, ["serde", "tokio", "renamed", "dotted", "regex", "tempfile", "cc", "libc", "anyhow"]);
}

#[test]
fn hint_lines_follow_the_requirement_and_features_are_listed_with_spans() {
    let m = manifest::parse(MANIFEST).unwrap();
    let line_of = |needle: &str| MANIFEST[..MANIFEST.find(needle).unwrap()].matches('\n').count();
    assert_eq!(dep(&m, "serde").line, line_of("serde = "));
    assert_eq!(dep(&m, "regex").line, line_of("version = \"1.5\""));
    let tokio = dep(&m, "tokio");
    assert_eq!(tokio.features.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["rt", "macros"]);
    assert_eq!(&MANIFEST[tokio.features[1].span.clone()], "macros");
}

#[test]
fn dep_at_finds_the_entry_under_an_offset() {
    let m = manifest::parse(MANIFEST).unwrap();
    let at = |needle: &str| MANIFEST.find(needle).unwrap() + 2;
    assert_eq!(m.dep_at(at("tokio")).unwrap().key, "tokio");
    assert_eq!(m.dep_at(at("features = [\"unicode")).unwrap().key, "regex");
    assert!(m.dep_at(at("[package]")).is_none());
}

#[test]
fn an_unfinished_document_is_an_error_not_a_panic() {
    assert!(manifest::parse("[dependencies]\nserde = \"1").is_err());
    assert!(manifest::parse("").unwrap().deps.is_empty());
    assert!(manifest::parse("[dependencies]\nx = 5\ny = [1]\n").is_ok());
}

#[test]
fn crate_names_are_validated_before_they_reach_a_url() {
    assert!(valid_crate_name("serde_json") && valid_crate_name("tokio-util") && valid_crate_name("a"));
    for bad in ["", "../etc/passwd", "a b", "a/b", "ü", "a?b=c", &"x".repeat(65)] {
        assert!(!valid_crate_name(bad), "{bad}");
    }
}

// --- the cursor's context --------------------------------------------------------------------------------------------

fn ctx(text: &str) -> Option<Ctx> {
    // `|` marks the cursor.
    let at = text.find('|').unwrap();
    context_at(&text.replacen('|', "", 1), at)
}

#[test]
fn a_new_key_in_a_dependency_table_completes_crate_names() {
    for header in ["[dependencies]", "[dev-dependencies]", "[build-dependencies]", "[workspace.dependencies]", "[target.'cfg(unix)'.dependencies]"] {
        let c = ctx(&format!("{header}\nserde = \"1\"\nreq|"));
        assert!(matches!(&c, Some(Ctx::CrateName { prefix, .. }) if prefix == "req"), "{header}: {c:?}");
    }
    assert_eq!(ctx("[package]\nname = \"x\"\nver|"), None);
    assert_eq!(ctx("[dependencies]\nserde = \"1\" # req|"), None);
}

#[test]
fn the_replaced_range_of_a_crate_name_is_the_typed_prefix() {
    let text = "[dependencies]\n  tok";
    assert_eq!(context_at(text, text.len()), Some(Ctx::CrateName { prefix: "tok".into(), replace: text.len() - 3..text.len() }));
}

#[test]
fn versions_are_completed_inside_the_quotes_in_every_form() {
    let cases = [
        ("[dependencies]\nserde = \"1.|\"", "serde", "1."),
        ("[dependencies]\nserde = \"|", "serde", ""),
        ("[dependencies]\nserde = { version = \"1|\" }", "serde", "1"),
        ("[dependencies]\nserde = { features = [\"derive\"], version = \"|\" }", "serde", ""),
        ("[dependencies]\nserde.version = \"0.|", "serde", "0."),
        ("[dependencies.serde]\nfeatures = [\"derive\"]\nversion = \"1.|\"", "serde", "1."),
        ("[dev-dependencies]\ntokio = '1|'", "tokio", "1"),
        ("[target.'cfg(unix)'.dependencies]\nlibc = \"0.|\"", "libc", "0."),
        ("[dependencies]\n\"my-crate\" = \"1|\"", "my-crate", "1"),
    ];
    for (text, package, typed) in cases {
        match ctx(text) {
            Some(Ctx::Version { package: p, prefix, .. }) => assert_eq!((p.as_str(), prefix.as_str()), (package, typed), "{text}"),
            other => panic!("{text}: {other:?}"),
        }
    }
}

#[test]
fn features_are_completed_in_arrays_of_every_form_including_multi_line() {
    let cases = [
        ("[dependencies]\ntokio = { version = \"1\", features = [\"rt\", \"mac|\"] }", "tokio", "mac"),
        ("[dependencies]\ntokio = { features = [|", "tokio", ""),
        ("[dependencies]\ntokio = { features = [\"|", "tokio", ""),
        ("[dependencies]\ntokio = { version = \"1\", features = [\n    \"rt\",\n    \"fu|\n] }", "tokio", "fu"),
        ("[dependencies]\ntokio.features = [\"fu|", "tokio", "fu"),
        ("[dependencies.tokio]\nversion = \"1\"\nfeatures = [\"rt\", \"s|", "tokio", "s"),
    ];
    for (text, package, typed) in cases {
        match ctx(text) {
            Some(Ctx::Feature { package: p, prefix, .. }) => assert_eq!((p.as_str(), prefix.as_str()), (package, typed), "{text}"),
            None if text.contains("[|") => {} // before the opening quote there is nothing to replace yet
            other => panic!("{text}: {other:?}"),
        }
    }
}

#[test]
fn no_context_outside_dependency_strings_or_in_other_tables() {
    assert_eq!(ctx("[package]\nname = \"se|\""), None);
    assert_eq!(ctx("[features]\ndefault = [\"se|\"]"), None);
    assert_eq!(ctx("[dependencies]\nserde = { path = \"../se|\" }"), None);
    assert_eq!(ctx("[dependencies]\nserde = { package = \"se|\" }"), None);
    assert_eq!(ctx("[dependencies]\n# serde = \"1|\""), None);
}

#[test]
fn a_closing_quote_after_the_cursor_is_noticed() {
    assert!(matches!(ctx("[dependencies]\nserde = \"1|\""), Some(Ctx::Version { closed: true, .. })));
    assert!(matches!(ctx("[dependencies]\nserde = \"1|"), Some(Ctx::Version { closed: false, .. })));
}

#[test]
fn the_context_survives_unicode_and_odd_offsets() {
    let text = "[dependencies]\nçş = \"1\"\nserde = \"é";
    for i in 0..=text.len() {
        let _ = context_at(text, i); // no panic on any offset, even inside a character
    }
    assert!(kind_at("[dev-dependencies]\nx", 20).is_some_and(|k| k == Kind::Dev));
    assert_eq!(kind_at("[target.'cfg(unix)'.build-dependencies]\nx", 45), Some(Kind::Build));
    assert_eq!(kind_at("[package]\nx", 10), None);
}

// --- items -----------------------------------------------------------------------------------------------------------

#[test]
fn version_items_are_newest_first_stable_before_prerelease_and_labelled() {
    let list = items::version_items(&serde(), false);
    assert_eq!(list[0].label, "1.0.229");
    assert_eq!(list[0].insert, "1.0.229\"", "an unclosed quote is closed");
    assert_eq!(list[0].detail.as_deref(), Some("latest"));
    assert!(items::version_items(&serde(), true)[0].insert == "1.0.229");
    let sorted: Vec<&str> = {
        let mut l = list.iter().collect::<Vec<_>>();
        l.sort_by(|a, b| a.sort.cmp(&b.sort));
        l.iter().map(|i| i.label.as_str()).collect()
    };
    assert_eq!(sorted, list.iter().map(|i| i.label.as_str()).collect::<Vec<_>>());
    assert!(list.iter().find(|i| i.label == "1.0.95").unwrap().detail.as_deref() == Some("yanked"));
}

#[test]
fn feature_items_come_from_the_published_list_without_the_ones_already_listed() {
    let info = index::parse_index("tokio", TOKIO_INDEX);
    let all = items::feature_items(&info, None, &[], true);
    assert!(all.iter().any(|i| i.label == "full") && all.iter().any(|i| i.label == "rt"));
    let full = all.iter().find(|i| i.label == "full").unwrap();
    assert!(full.doc.as_deref().unwrap().starts_with("Enables: fs,"));
    let fewer = items::feature_items(&info, None, &["full".to_string()], true);
    assert!(fewer.iter().all(|i| i.label != "full"));
    let serde_features = items::feature_items(&serde(), None, &[], false);
    let default = serde_features.iter().find(|i| i.label == "default").unwrap();
    assert_eq!(default.insert, "default\"");
    assert_eq!(serde_features[0].label, "default", "default sorts first");
    assert_eq!(serde_features.iter().find(|i| i.label == "std").unwrap().detail.as_deref(), Some("enabled by default"));
}

#[test]
fn the_name_that_was_typed_ranks_before_crates_io_relevance_order() {
    let hit = |name: &str| super::registry::SearchHit { name: name.into(), version: "1.0.0".into(), description: String::new() };
    let hits = [hit("serde-saphyr"), hit("my-serde"), hit("serde_json"), hit("Serde"), hit("serde")];
    let names: Vec<&str> = items::ranked(&hits, "serde").iter().map(|h| h.name.as_str()).collect();
    assert_eq!(names, ["Serde", "serde", "serde-saphyr", "serde_json", "my-serde"], "exact (any case), then prefix, then the rest, each in crates.io order");
}

#[test]
fn crate_items_write_the_whole_entry_and_carry_the_description() {
    let hits = super::registry::parse_search(SEARCH);
    assert_eq!(hits[0].name, "serde");
    let list = items::crate_items(&hits, "serde");
    assert_eq!(list[0].insert, format!("serde = \"{}\"", hits[0].version));
    assert_eq!(list[0].doc.as_deref(), Some("A generic serialization/deserialization framework"));
    assert!(list[0].sort < list[1].sort);
}

// --- the registry ----------------------------------------------------------------------------------------------------

#[test]
fn a_lookup_asks_once_then_answers_from_memory() {
    let fake = Arc::new(Fake::default());
    fake.ok(SERDE_URL, SERDE_INDEX, "\"e1\"");
    let reg = registry(&fake, None, Arc::new(AtomicUsize::new(1000)));
    let Lookup::Found { info, stale: false } = reg.crate_info("serde") else { panic!() };
    assert_eq!(info.latest().unwrap().version, v("1.0.229"));
    assert!(matches!(reg.crate_info("SERDE"), Lookup::Found { .. }));
    assert_eq!(fake.count(), 1, "the second lookup needs no request");
}

#[test]
fn a_cached_crate_is_not_asked_for_again_across_restarts_until_it_is_old() {
    let dir = temp_dir("restart");
    let (fake, now) = (Arc::new(Fake::default()), Arc::new(AtomicUsize::new(10_000)));
    fake.ok(SERDE_URL, SERDE_INDEX, "\"e1\"");
    registry(&fake, Some(dir.clone()), now.clone()).crate_info("serde");
    let second = registry(&fake, Some(dir.clone()), now.clone());
    assert!(matches!(second.crate_info("serde"), Lookup::Found { stale: false, .. }));
    assert_eq!(fake.count(), 1, "a fresh disk entry needs no request");
    // Past the time to live the entry is revalidated with its ETag, and a 304 keeps it.
    now.fetch_add(super::registry::INDEX_FRESH.as_secs() as usize + 1, Ordering::SeqCst);
    fake.set(SERDE_URL, Ok(Response { status: 304, etag: None, body: Vec::new() }));
    let third = registry(&fake, Some(dir.clone()), now.clone());
    assert!(matches!(third.crate_info("serde"), Lookup::Found { stale: false, .. }));
    assert_eq!(fake.calls.lock().unwrap().last().unwrap().1.as_deref(), Some("\"e1\""), "conditional request");
    assert_eq!(fake.count(), 2);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn offline_with_a_cache_serves_the_old_entry_and_without_one_says_offline() {
    let dir = temp_dir("offline");
    let (fake, now) = (Arc::new(Fake::default()), Arc::new(AtomicUsize::new(10_000)));
    fake.ok(SERDE_URL, SERDE_INDEX, "\"e1\"");
    registry(&fake, Some(dir.clone()), now.clone()).crate_info("serde");
    now.fetch_add(10 * 24 * 3600, Ordering::SeqCst);
    fake.set(SERDE_URL, Err(FetchError::Offline));
    let reg = registry(&fake, Some(dir.clone()), now.clone());
    assert!(matches!(reg.crate_info("serde"), Lookup::Found { stale: true, .. }));
    // Another crate has no cache: offline, and the pause means no second request is even attempted.
    let before = fake.count();
    assert_eq!(reg.crate_info("tokio"), Lookup::Offline);
    assert_eq!(reg.crate_info("rand"), Lookup::Offline);
    assert_eq!(fake.count(), before, "no request during the offline pause");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_timeout_counts_as_offline_for_the_pause_and_other_errors_are_reported() {
    let fake = Arc::new(Fake::default());
    fake.route("https://index.crates.io/to/ki/tokio", Err(FetchError::Timeout));
    let reg = registry(&fake, None, Arc::new(AtomicUsize::new(1)));
    assert_eq!(reg.crate_info("tokio"), Lookup::Failed("the request timed out".into()));
    assert_eq!(reg.crate_info("rand"), Lookup::Offline, "after a timeout nothing is tried during the pause");
    let fake = Arc::new(Fake::default());
    fake.route(SERDE_URL, Err(FetchError::Other("curl: (60) certificate".into())));
    let reg = registry(&fake, None, Arc::new(AtomicUsize::new(1)));
    assert_eq!(reg.crate_info("serde"), Lookup::Failed("curl: (60) certificate".into()));
}

#[test]
fn an_unknown_crate_is_not_found_and_remembered() {
    let fake = Arc::new(Fake::default());
    let reg = registry(&fake, None, Arc::new(AtomicUsize::new(1)));
    assert_eq!(reg.crate_info("no-such-crate-zzz"), Lookup::NotFound);
    let first = fake.count();
    assert_eq!(reg.crate_info("no-such-crate-zzz"), Lookup::NotFound);
    assert_eq!(fake.count(), first, "a missing crate is not asked for again within the hour");
}

#[test]
fn hyphen_and_underscore_spellings_are_tried_once() {
    let fake = Arc::new(Fake::default());
    fake.ok("https://index.crates.io/to/ki/tokio-util", SERDE_INDEX, "\"u\"");
    let reg = registry(&fake, None, Arc::new(AtomicUsize::new(1)));
    assert!(matches!(reg.crate_info("tokio_util"), Lookup::Found { .. }));
    assert_eq!(fake.count(), 2);
    assert!(matches!(reg.crate_info("tokio_util"), Lookup::Found { .. }));
    assert_eq!(fake.count(), 2, "the alias is remembered");
    // Neither spelling exists: two requests, then nothing.
    let fake = Arc::new(Fake::default());
    let reg = registry(&fake, None, Arc::new(AtomicUsize::new(1)));
    assert_eq!(reg.crate_info("a_b_c"), Lookup::NotFound);
    assert_eq!(fake.count(), 2);
}

#[test]
fn invalid_names_never_reach_the_network() {
    let fake = Arc::new(Fake::default());
    let reg = registry(&fake, None, Arc::new(AtomicUsize::new(1)));
    assert_eq!(reg.crate_info("../../etc/passwd"), Lookup::NotFound);
    assert!(reg.meta("a/b").is_err());
    assert_eq!(fake.count(), 0);
}

#[test]
fn many_lookups_run_with_bounded_parallelism_and_keep_their_order() {
    let fake = Arc::new(Fake { delay_ms: 15, ..Default::default() });
    let names: Vec<String> = (0..16).map(|i| format!("crate-number-{i}")).collect();
    for n in &names {
        fake.ok(&format!("https://index.crates.io/{}", index::index_path(n)), SERDE_INDEX, "\"e\"");
    }
    let reg = registry(&fake, None, Arc::new(AtomicUsize::new(1)));
    let answers = reg.lookup_many(&names);
    assert_eq!(answers.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(), names);
    assert!(answers.iter().all(|(_, l)| matches!(l, Lookup::Found { .. })));
    assert!(fake.peak.load(Ordering::SeqCst) <= super::registry::MAX_PARALLEL, "peak {}", fake.peak.load(Ordering::SeqCst));
    assert!(fake.peak.load(Ordering::SeqCst) >= 2, "it does run in parallel");
}

#[test]
fn search_parses_the_real_answer_and_is_cached_in_memory() {
    let fake = Arc::new(Fake::default());
    let url = "https://crates.io/api/v1/crates?q=serde&per_page=20";
    fake.ok(url, SEARCH, "\"s\"");
    let reg = registry(&fake, None, Arc::new(AtomicUsize::new(1)));
    let hits = reg.search("serde").unwrap();
    assert_eq!(hits.len(), 3);
    assert_eq!((hits[0].name.as_str(), hits[0].description.as_str()), ("serde", "A generic serialization/deserialization framework"));
    assert_eq!(reg.search("Serde").unwrap(), hits);
    assert_eq!(fake.count(), 1);
    assert_eq!(reg.search("  ").unwrap(), Vec::new());
}

#[test]
fn search_queries_are_percent_encoded() {
    let fake = Arc::new(Fake::default());
    let reg = registry(&fake, None, Arc::new(AtomicUsize::new(1)));
    let _ = reg.search("a&b=c d/é");
    assert_eq!(fake.calls.lock().unwrap()[0].0, "https://crates.io/api/v1/crates?q=a%26b%3Dc%20d%2F%C3%A9&per_page=20");
}

#[test]
fn meta_combines_the_crate_and_its_version_and_is_cached_on_disk() {
    let dir = temp_dir("meta");
    let fake = Arc::new(Fake::default());
    fake.ok("https://crates.io/api/v1/crates/serde?include=", CRATE_JSON, "\"c\"");
    fake.ok("https://crates.io/api/v1/crates/serde/1.0.229", &VERSION_JSON.replace("1.0.219", "1.0.229"), "\"v\"");
    let reg = registry(&fake, Some(dir.clone()), Arc::new(AtomicUsize::new(5)));
    let meta = reg.meta("serde").unwrap();
    assert_eq!(meta.description.as_deref(), Some("A generic serialization/deserialization framework"));
    assert_eq!(meta.license.as_deref(), Some("MIT OR Apache-2.0"));
    assert_eq!(meta.documentation, "https://docs.rs/serde");
    assert_eq!(meta.repository.as_deref(), Some("https://github.com/serde-rs/serde"));
    assert_eq!(fake.count(), 2);
    let again = registry(&fake, Some(dir.clone()), Arc::new(AtomicUsize::new(6))).meta("serde").unwrap();
    assert_eq!(again, meta);
    assert_eq!(fake.count(), 2, "served from the disk cache");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_mirror_has_no_api() {
    let fake = Arc::new(Fake::default());
    let reg = Registry::new(fake.clone(), DiskCache::new(None), Endpoints { index: "https://mirror.example/index".into(), api: None });
    assert_eq!(reg.search("serde"), Err(Failure::Unavailable));
    assert_eq!(reg.meta("serde"), Err(Failure::Unavailable));
    fake.ok("https://mirror.example/index/se/rd/serde", SERDE_INDEX, "\"m\"");
    assert!(matches!(reg.crate_info("serde"), Lookup::Found { .. }));
}

#[test]
fn a_damaged_cache_file_is_ignored() {
    let dir = temp_dir("damaged");
    std::fs::create_dir_all(dir.join("index")).unwrap();
    std::fs::write(dir.join("index/serde.json"), "{ not json").unwrap();
    let fake = Arc::new(Fake::default());
    fake.ok(SERDE_URL, SERDE_INDEX, "\"e\"");
    let reg = registry(&fake, Some(dir.clone()), Arc::new(AtomicUsize::new(1)));
    assert!(matches!(reg.crate_info("serde"), Lookup::Found { .. }));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn pruning_removes_old_entries_only() {
    let dir = temp_dir("prune");
    let cache = DiskCache::new(Some(dir.clone()));
    let stored = |fetched| Stored { etag: None, fetched, payload: serde_json::json!({}) };
    cache.write("index", "old", &stored(1));
    cache.write("index", "new", &stored(10_000_000));
    cache.prune(10_000_000);
    assert!(cache.read("index", "old").is_none() && cache.read("index", "new").is_some());
    assert!(cache.read("index", "../x").is_none(), "no path escape");
    std::fs::remove_dir_all(dir).ok();
}

// --- hints and problems ----------------------------------------------------------------------------------------------

fn lookups_for(manifest: &manifest::Manifest) -> Lookups {
    let info = Arc::new(serde());
    hints::wanted(manifest).iter().map(|d| (d.package.to_ascii_lowercase(), Lookup::Found { info: info.clone(), stale: false })).collect()
}

#[test]
fn hints_say_up_to_date_outdated_yanked_not_found_and_offline() {
    let text = "[dependencies]\na = \"1\"\nb = \"0.2\"\nc = \"=1.0.95\"\nd = \"1\"\ne = \"1\"\nf = \"99\"\n";
    let m = manifest::parse(text).unwrap();
    let info = Arc::new(serde());
    let found = Lookup::Found { info, stale: false };
    let mut lookups: Lookups = ["a", "b", "c", "f"].iter().map(|k| (k.to_string(), found.clone())).collect();
    lookups.insert("d".into(), Lookup::NotFound);
    lookups.insert("e".into(), Lookup::Offline);
    let list = hints::hints(&m, &lookups, None);
    let shown: Vec<(&str, Tone)> = list.iter().map(|h| (h.text.as_str(), h.tone)).collect();
    assert_eq!(
        shown,
        [
            ("up to date (latest 1.0.229)", Tone::Success),
            ("update available: 1.0.229", Tone::Warning),
            ("yanked: 1.0.95", Tone::Warning),
            ("not found on crates.io", Tone::Danger),
            ("offline", Tone::Muted),
            ("no version matches", Tone::Danger),
        ]
    );
    assert_eq!(list.iter().map(|h| h.line).collect::<Vec<_>>(), [1, 2, 3, 4, 5, 6]);
    assert!(list[1].detail.contains("“0.2”") && list[1].detail.contains("1.0.229"));
}

#[test]
fn skipped_sources_get_no_hint_and_no_problem() {
    let m = manifest::parse(MANIFEST).unwrap();
    let lookups = lookups_for(&m);
    let keys: Vec<String> = hints::hints(&m, &lookups, None).into_iter().map(|h| h.dep).collect();
    for skipped in ["local", "g", "inherited", "other"] {
        assert!(!keys.iter().any(|k| k == skipped), "{skipped}");
    }
}

#[test]
fn a_yanked_version_is_a_problem_pointing_at_the_requirement() {
    let text = "[dependencies]\nserde = \"=1.0.95\"\ntokio = \"1\"\n";
    let m = manifest::parse(text).unwrap();
    let lookups = lookups_for(&m);
    let problems = hints::problems(&m, &lookups, None);
    assert_eq!(problems.len(), 1);
    assert_eq!(&text[problems[0].span.clone()], "=1.0.95");
    assert!(problems[0].message.contains("1.0.95") && problems[0].message.contains("yanked"));
}

#[test]
fn the_lock_file_decides_yanked_for_a_requirement_that_allows_newer_versions() {
    let lock = Lock::parse(
        "version = 4\n[[package]]\nname = \"serde\"\nversion = \"1.0.31\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n\n[[package]]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"localdep\"\nversion = \"9.9.9\"\nsource = \"git+https://example.com/x\"\n",
    )
    .unwrap();
    let m = manifest::parse("[dependencies]\nserde = \"1\"\nlocaldep = \"9\"\n").unwrap();
    let lookups = lookups_for(&m);
    let hint = hints::hints(&m, &lookups, Some(&lock));
    assert_eq!(hint[0].text, "yanked: 1.0.31");
    assert_eq!(hints::problems(&m, &lookups, Some(&lock)).len(), 1);
    assert!(
        Lock::parse("[[package]]\nname = \"x\"\nversion = \"1.0.0\"\nsource = \"sparse+https://index.crates.io/\"\n").unwrap().resolved("x", None).is_some()
    );
    assert!(Lock::parse("not toml {").is_none());
}

#[test]
fn hover_shows_description_license_lock_version_and_links() {
    let m = manifest::parse("[dependencies]\nserde = { version = \"1\", features = [\"derive\", \"nope\"] }\n").unwrap();
    let lookups = lookups_for(&m);
    let lookup = &lookups["serde"];
    let fake = Arc::new(Fake::default());
    fake.ok("https://crates.io/api/v1/crates/serde?include=", CRATE_JSON, "\"c\"");
    fake.ok("https://crates.io/api/v1/crates/serde/1.0.229", VERSION_JSON, "\"v\"");
    let meta = registry(&fake, None, Arc::new(AtomicUsize::new(1))).meta("serde").unwrap();
    let lock =
        Lock::parse("[[package]]\nname = \"serde\"\nversion = \"1.0.200\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n").unwrap();
    let d = &m.deps[0];
    let text = hints::hover_text(d, lookup, Ok(&meta), Some(&lock), d.key_span.start);
    for expected in [
        "serde 1.0.229",
        "A generic serialization/deserialization framework",
        "License: MIT OR Apache-2.0",
        "Requirement “1”, Cargo.lock has 1.0.200",
        "Up to date: the latest release is 1.0.229",
        "Documentation: https://docs.rs/serde",
        "Repository: https://github.com/serde-rs/serde",
    ] {
        assert!(text.contains(expected), "{expected} not in\n{text}");
    }
    let derive = &d.features[0];
    assert_eq!(hints::hover_text(d, lookup, Ok(&meta), None, derive.span.start + 1), "feature derive of serde: enables serde_derive");
    let nope = &d.features[1];
    assert_eq!(hints::hover_text(d, lookup, Ok(&meta), None, nope.span.start), "serde has no feature named nope");
}

#[test]
fn hover_without_metadata_still_links_docs_and_explains_skipped_sources() {
    let m = manifest::parse(MANIFEST).unwrap();
    let lookups = lookups_for(&m);
    let failure = Failure::Offline;
    let serde_hover = hints::hover_text(dep(&m, "serde"), &lookups["serde"], Err(&failure), None, 0);
    assert!(serde_hover.contains("https://docs.rs/serde"));
    let local = hints::hover_text(dep(&m, "local"), &Lookup::Offline, Err(&failure), None, 0);
    assert!(local.contains("A path dependency"));
    let inherited = hints::hover_text(dep(&m, "inherited"), &Lookup::Offline, Err(&failure), None, 0);
    assert!(inherited.contains("[workspace.dependencies]"));
}

// --- editing ---------------------------------------------------------------------------------------------------------

#[test]
fn updating_replaces_only_the_requirement_text() {
    let text = "# top comment\n[dependencies]\nserde = \"1.0.100\" # keep me\ntokio   =   { version = '0.9', features = [\"rt\"] }\n\n[dependencies.regex]\n# about regex\nversion = \"0\"\n";
    let m = manifest::parse(text).unwrap();
    let lookups = lookups_for(&m);
    let edits = edit::update_edits(&m, &lookups, Update::Compatible, None);
    let out = edit::apply(text, &edits);
    assert_eq!(out, text.replace("\"1.0.100\"", "\"1.0.229\""), "only serde changed; tokio has no 0.9 release in this sample and regex's 0 is current");
    let all = edit::update_edits(&m, &lookups, Update::Latest, None);
    let out = edit::apply(text, &all);
    assert!(out.contains("serde = \"1.0.229\" # keep me") && out.contains("tokio   =   { version = '1.0', features = [\"rt\"] }"), "{out}");
    assert!(out.contains("# about regex\nversion = \"1\"\n"), "{out}");
    let only = edit::update_edits(&m, &lookups, Update::Latest, Some(dep(&m, "tokio")));
    assert_eq!(only.len(), 1);
}

#[test]
fn updating_skips_unknown_offline_and_foreign_entries() {
    let m = manifest::parse(MANIFEST).unwrap();
    let mut lookups = lookups_for(&m);
    lookups.insert("tokio".into(), Lookup::Offline);
    let out = edit::apply(MANIFEST, &edit::update_edits(&m, &lookups, Update::Latest, None));
    assert!(out.contains("tokio = { version = \"1\", features"), "an offline lookup changes nothing");
    assert!(out.contains("local = { path = \"../local\", version = \"0.1\" }") && out.contains("other = { version = \"2\", registry = \"company\" }"));
}

#[test]
fn adding_keeps_comments_and_sorts_only_a_sorted_table() {
    let text = "[package]\nname = \"demo\"\n\n[dependencies]\n# serialisation\nserde = \"1\"\ntokio = \"1\" # runtime\n";
    let out = edit::add_dependency(text, &["dependencies".to_string()], "anyhow", "1.0.100").unwrap();
    assert!(out.contains("# serialisation\nserde = \"1\""), "{out}");
    assert!(out.contains("tokio = \"1\" # runtime"), "{out}");
    assert!(out.find("anyhow").unwrap() < out.find("serde").unwrap(), "sorted table stays sorted:\n{out}");
    let unsorted = "[dependencies]\nzzz = \"1\"\naaa = \"1\"\n";
    let out = edit::add_dependency(unsorted, &["dependencies".to_string()], "mmm", "2").unwrap();
    assert!(out.ends_with("mmm = \"2\"\n"), "{out}");
    assert_eq!(edit::add_dependency(text, &["dependencies".to_string()], "serde", "1"), Err(EditError::Exists("serde".into())));
    assert!(matches!(edit::add_dependency("[dependencies\n", &["dependencies".to_string()], "x", "1"), Err(EditError::Invalid(_))));
}

#[test]
fn adding_creates_the_table_and_targets_a_virtual_workspace_root() {
    let out = edit::add_dependency("[package]\nname = \"demo\"\n", &["dev-dependencies".to_string()], "tempfile", "3").unwrap();
    assert!(out.contains("[dev-dependencies]\ntempfile = \"3\""), "{out}");
    let root = "[workspace]\nmembers = [\"a\"]\n";
    assert_eq!(edit::add_target(root, Kind::Normal), ["workspace", "dependencies"]);
    assert_eq!(edit::add_target("[package]\nname = \"x\"\n", Kind::Dev), ["dev-dependencies"]);
    let out = edit::add_dependency(root, &edit::add_target(root, Kind::Normal), "anyhow", "1").unwrap();
    let m = manifest::parse(&out).unwrap();
    assert!(m.deps[0].in_workspace_table && m.deps[0].key == "anyhow", "{out}");
}

#[test]
fn removing_takes_the_entry_with_its_comment_and_leaves_the_rest() {
    let text = "[dependencies]\n# serialisation\nserde = \"1\"\ntokio = { version = \"1\", features = [\"rt\"] } # runtime\n\n[dependencies.regex]\nversion = \"1\"\n\n[dev-dependencies]\ntempfile = \"3\"\n";
    let m = manifest::parse(text).unwrap();
    let out = edit::remove_dependency(text, dep(&m, "tokio")).unwrap();
    assert!(!out.contains("tokio") && out.contains("# serialisation\nserde = \"1\""), "{out}");
    let out = edit::remove_dependency(text, dep(&m, "regex")).unwrap();
    assert!(!out.contains("regex") && out.contains("tokio"), "{out}");
    let out = edit::remove_dependency(text, dep(&m, "tempfile")).unwrap();
    assert!(!out.contains("tempfile") && out.contains("[dev-dependencies]"), "{out}");
    let mut ghost = dep(&m, "serde").clone();
    ghost.key = "ghost".into();
    assert_eq!(edit::remove_dependency(text, &ghost), Err(EditError::Missing("ghost".into())));
}

#[test]
fn removing_works_in_target_and_workspace_tables() {
    let m = manifest::parse(MANIFEST).unwrap();
    let out = edit::remove_dependency(MANIFEST, dep(&m, "libc")).unwrap();
    assert!(!out.contains("libc"));
    let out = edit::remove_dependency(MANIFEST, dep(&m, "anyhow")).unwrap();
    assert!(!out.contains("anyhow") && out.contains("[workspace.dependencies]"));
}

#[test]
fn a_minimal_edit_turns_old_into_new_and_respects_characters() {
    for (old, new) in
        [("abc", "abXc"), ("serde = \"1\"\n", "serde = \"1\"\nx = \"2\"\n"), ("é1", "é2"), ("aé", "aè"), ("same", "same"), ("", "new"), ("old", "")]
    {
        match edit::minimal_edit(old, new) {
            Some(e) => assert_eq!(edit::apply(old, &[e]), new, "{old:?} -> {new:?}"),
            None => assert_eq!(old, new),
        }
    }
    let e = edit::minimal_edit("a = \"1\"\nb = \"2\"\n", "a = \"1\"\nb = \"3\"\n").unwrap();
    assert_eq!((e.range.len(), e.text.as_str()), (1, "3"));
}

// --- Cargo configuration ---------------------------------------------------------------------------------------------

#[test]
fn a_sparse_mirror_replaces_the_index_and_switches_the_api_off() {
    let cfg = "[source.crates-io]\nreplace-with = \"corp\"\n\n[source.corp]\nregistry = \"sparse+https://cargo.corp.example/index/\"\n";
    assert_eq!(config::resolve(&[cfg.into()]), Source::Endpoints(Endpoints { index: "https://cargo.corp.example/index".into(), api: None }));
    assert_eq!(config::resolve(&[]), Source::Endpoints(Endpoints::default()));
    assert_eq!(config::resolve(&["[build]\njobs = 2\n".into()]), Source::Endpoints(Endpoints::default()));
}

#[test]
fn mirrors_that_cannot_be_asked_turn_the_lookups_off() {
    let vendored = "[source.crates-io]\nreplace-with = \"vendored\"\n[source.vendored]\ndirectory = \"vendor\"\n";
    assert_eq!(config::resolve(&[vendored.into()]), Source::Unsupported);
    let git = "[source.crates-io]\nreplace-with = \"m\"\n[source.m]\nregistry = \"https://example.com/index.git\"\n";
    assert_eq!(config::resolve(&[git.into()]), Source::Unsupported);
    let http = "[source.crates-io]\nreplace-with = \"m\"\n[source.m]\nregistry = \"sparse+http://insecure.example/\"\n";
    assert_eq!(config::resolve(&[http.into()]), Source::Unsupported, "plain http is never used");
    let undefined = "[source.crates-io]\nreplace-with = \"nowhere\"\n";
    assert_eq!(config::resolve(&[undefined.into()]), Source::Unsupported);
}

#[test]
fn the_closest_configuration_wins_and_the_replacement_may_be_defined_further_up() {
    let near = "[source.crates-io]\nreplace-with = \"near\"\n";
    let far = "[source.near]\nregistry = \"sparse+https://near.example/i/\"\n[source.crates-io]\nreplace-with = \"far\"\n";
    assert_eq!(config::resolve(&[near.into(), far.into()]), Source::Endpoints(Endpoints { index: "https://near.example/i".into(), api: None }));
}

#[test]
fn cargo_home_comes_from_the_environment_or_the_home_folder() {
    use std::path::PathBuf;
    assert_eq!(config::cargo_home(Some("/opt/cargo".into()), Some("/home/u".into())), Some(PathBuf::from("/opt/cargo")));
    assert_eq!(config::cargo_home(None, Some("/home/u".into())), Some(PathBuf::from("/home/u/.cargo")));
    assert_eq!(config::cargo_home(Some("".into()), Some("/home/u".into())), Some(PathBuf::from("/home/u/.cargo")));
}

#[test]
fn the_configuration_chain_is_read_closest_first_and_project_files_are_optional() {
    let dir = temp_dir("chain");
    let (home, project) = (dir.join("home"), dir.join("a/b"));
    std::fs::create_dir_all(home.clone()).unwrap();
    std::fs::create_dir_all(project.join(".cargo")).unwrap();
    std::fs::create_dir_all(dir.join("a/.cargo")).unwrap();
    std::fs::write(home.join("config.toml"), "home").unwrap();
    std::fs::write(project.join(".cargo/config.toml"), "inner").unwrap();
    std::fs::write(dir.join("a/.cargo/config"), "outer").unwrap();
    assert_eq!(config::read_chain(&project, Some(&home), true), ["inner", "outer", "home"]);
    assert_eq!(config::read_chain(&project, Some(&home), false), ["home"], "an untrusted project's files are not read");
    std::fs::remove_dir_all(dir).ok();
}

// --- the curl boundary against a real local server -------------------------------------------------------------------

#[test]
fn curl_fetch_reads_status_etag_and_body_and_sends_the_conditional_header() {
    use std::io::{Read, Write};
    let curl = crate::update::system_tool("curl");
    if std::process::Command::new(&curl).arg("--version").output().is_err() {
        eprintln!("SKIPPED: curl is not installed");
        return;
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let log = seen.clone();
    let server = std::thread::spawn(move || {
        for answer in [
            "HTTP/1.1 200 OK\r\nETag: \"v1\"\r\nContent-Length: 6\r\nConnection: close\r\n\r\nhello\n",
            "HTTP/1.1 304 Not Modified\r\nETag: \"v1\"\r\nConnection: close\r\n\r\n",
            "HTTP/1.1 301 Moved\r\nLocation: http://127.0.0.1:1/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        ] {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap();
            log.lock().unwrap().push(String::from_utf8_lossy(&buf[..n]).into_owned());
            s.write_all(answer.as_bytes()).unwrap();
        }
    });
    let fetch = super::fetch::CurlFetch::for_test();
    let url = format!("http://127.0.0.1:{port}/se/rd/serde");
    let first = fetch.get(&url, None).unwrap();
    assert_eq!((first.status, first.etag.as_deref(), first.body.as_slice()), (200, Some("\"v1\""), &b"hello\n"[..]));
    let second = fetch.get(&url, Some("\"v1\"")).unwrap();
    assert_eq!((second.status, second.body.len()), (304, 0));
    assert_eq!(fetch.get(&url, None).unwrap().status, 301, "redirects are not followed");
    server.join().unwrap();
    let requests = seen.lock().unwrap();
    let lower = requests[1].to_ascii_lowercase();
    assert!(
        lower.contains("if-none-match: \"v1\"") && lower.contains(&format!("user-agent: {}", super::fetch::user_agent().to_ascii_lowercase())),
        "{}",
        requests[1]
    );
    assert!(!lower.contains("cookie") && !lower.contains("authorization"), "nothing but the lookup is sent:\n{}", requests[1]);
}

#[test]
fn curl_fetch_reports_an_unreachable_server_as_offline() {
    let curl = crate::update::system_tool("curl");
    if std::process::Command::new(&curl).arg("--version").output().is_err() {
        eprintln!("SKIPPED: curl is not installed");
        return;
    }
    // A port that was just closed: the connection is refused.
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let fetch = super::fetch::CurlFetch::for_test();
    assert_eq!(fetch.get(&format!("http://127.0.0.1:{port}/x"), None), Err(FetchError::Offline));
}

#[test]
fn the_app_client_refuses_plain_http() {
    let curl = crate::update::system_tool("curl");
    if std::process::Command::new(&curl).arg("--version").output().is_err() {
        return;
    }
    let result = super::fetch::CurlFetch::new().get("http://127.0.0.1:9/x", None);
    assert!(matches!(result, Err(FetchError::Other(_))), "{result:?}");
}

// --- review fixes ----------------------------------------------------------------------------------------------------

#[test]
fn a_crate_name_follows_crates_io_rules_so_an_option_is_no_name() {
    for bad in ["--git", "-x", "--config=/x.toml", "1abc", "_x", "", "a b", "a;b"] {
        assert!(!valid_crate_name(bad), "{bad}");
    }
    assert!(valid_crate_name("a") && valid_crate_name("serde_json") && valid_crate_name(&format!("a{}", "b".repeat(63))));
    assert!(!valid_crate_name(&format!("a{}", "b".repeat(64))));
}

#[test]
fn the_memory_of_a_registry_is_bounded() {
    let fake = Arc::new(Fake::default());
    let reg = registry(&fake, None, Arc::new(AtomicUsize::new(1)));
    for i in 0..400 {
        let name = format!("crate-{i}");
        fake.ok(&format!("https://index.crates.io/{}", index::index_path(&name)), SERDE_INDEX, "\"e\"");
        reg.crate_info(&name);
    }
    assert!(reg.memory_len() <= 256, "{}", reg.memory_len());
    // The newest stay, the oldest were forgotten (and are asked for again).
    let before = fake.count();
    reg.crate_info("crate-399");
    assert_eq!(fake.count(), before);
    reg.crate_info("crate-0");
    assert_eq!(fake.count(), before + 1);
}

#[test]
fn the_least_recently_used_entry_is_the_one_forgotten() {
    let mut lru = super::registry::lru_for_test(64);
    for i in 0..100 {
        lru.insert(format!("q{i}"), i);
    }
    assert_eq!(lru.len(), 64);
    assert!(lru.get("q99").is_some() && lru.get("q0").is_none());
    lru.get("q36");
    lru.insert("fresh".into(), 0);
    assert!(lru.get("q36").is_some(), "a use keeps an entry");
    assert!(lru.get("q37").is_none());
}

#[test]
fn an_answer_that_is_not_an_index_file_is_not_cached_as_a_crate_without_versions() {
    let dir = temp_dir("portal");
    let fake = Arc::new(Fake::default());
    fake.ok(SERDE_URL, "<html>Sign in to the Wi-Fi</html>", "\"p\"");
    let reg = registry(&fake, Some(dir.clone()), Arc::new(AtomicUsize::new(1)));
    assert!(matches!(reg.crate_info("serde"), Lookup::Failed(m) if m.contains("not an index file")));
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "nothing was written");
    // The pause keeps the next lookups quiet, and a later good answer is used.
    assert_eq!(reg.crate_info("tokio"), Lookup::Offline);
    let fresh = registry(&fake, Some(dir.clone()), Arc::new(AtomicUsize::new(1)));
    fake.set(SERDE_URL, Ok(Response { status: 200, etag: None, body: SERDE_INDEX.as_bytes().to_vec() }));
    assert!(matches!(fresh.crate_info("serde"), Lookup::Found { .. }));
    fake.set(SERDE_URL, Ok(Response { status: 200, etag: None, body: Vec::new() }));
    let empty = registry(&fake, None, Arc::new(AtomicUsize::new(1)));
    assert!(matches!(empty.crate_info("serde"), Lookup::Failed(_)), "an empty body is no index file either");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_mirror_address_with_credentials_is_never_used() {
    for url in ["sparse+https://user:pass@mirror.example/index/", "sparse+https://token@mirror.example/"] {
        let cfg = format!("[source.crates-io]\nreplace-with = \"m\"\n[source.m]\nregistry = \"{url}\"\n");
        assert_eq!(config::resolve(&[cfg]), Source::Unsupported, "{url}");
    }
    let ok = "[source.crates-io]\nreplace-with = \"m\"\n[source.m]\nregistry = \"sparse+https://mirror.example/a@b/\"\n";
    assert!(matches!(config::resolve(&[ok.into()]), Source::Endpoints(_)), "an @ in the path is no userinfo");
}

#[test]
fn edits_keep_a_crlf_file_crlf() {
    let text = "[package]\r\nname = \"x\"\r\n\r\n[dependencies]\r\nserde = \"1\"\r\n";
    let added = edit::add_dependency(text, &["dependencies".to_string()], "anyhow", "1").unwrap();
    assert!(added.contains("anyhow = \"1\"\r\n") && !added.replace("\r\n", "").contains('\n'), "{added:?}");
    let m = manifest::parse(text).unwrap();
    let removed = edit::remove_dependency(text, &m.deps[0]).unwrap();
    assert!(!removed.replace("\r\n", "").contains('\n') && !removed.contains("serde"), "{removed:?}");
    // A file with LF endings is left as toml_edit wrote it.
    assert!(!edit::add_dependency("[dependencies]\n", &["dependencies".to_string()], "a", "1").unwrap().contains('\r'));
}

#[test]
fn curl_fetch_stops_a_server_that_never_stops_without_waiting_for_the_timeout() {
    use std::io::{Read, Write};
    let curl = crate::update::system_tool("curl");
    if std::process::Command::new(&curl).arg("--version").output().is_err() {
        eprintln!("SKIPPED: curl is not installed");
        return;
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let _ = s.read(&mut buf);
        s.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n").ok();
        let block = [b'x'; 64 * 1024];
        // No Content-Length: only reading the bytes shows how much there is.
        while s.write_all(&block).is_ok() {}
    });
    let started = std::time::Instant::now();
    let result = super::fetch::CurlFetch::for_test().get(&format!("http://127.0.0.1:{port}/endless"), None);
    assert_eq!(result, Err(FetchError::Other("the answer was too large".into())));
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
    server.join().unwrap();
}
