use std::path::PathBuf;

use crate::testing::{DirFetch, TestRegistry, github};

use super::*;

const NOW: u64 = 1_790_000_000;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ext-registry-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn package(id: &str, version: &str) -> Vec<u8> {
    let manifest =
        format!("[extension]\nid = \"{id}\"\nversion = \"{version}\"\nname = \"Night\"\nlicense = \"MIT\"\n[[themes]]\nlabel = \"Night\"\npath = \"t.json\"\n");
    let entries = vec![
        ext_pkg::Entry { path: ext_manifest::FILE_NAME.into(), data: manifest.into_bytes(), executable: false },
        ext_pkg::Entry { path: "t.json".into(), data: br#"{ "name": "Night", "appearance": "dark", "color": {} }"#.to_vec(), executable: false },
    ];
    ext_pkg::write(entries).unwrap().0
}

struct Setup {
    root: PathBuf,
    yard: TestRegistry,
    fetch: DirFetch,
    cache: PathBuf,
}

fn setup(name: &str) -> Setup {
    let root = tmp(name);
    let mut yard = TestRegistry::new(&root.join("yard"), "test", NOW);
    yard.publish(&package("you.night", "1.0.0"), &github(7, 42), NOW);
    yard.publish(&package("you.night", "1.1.0"), &github(7, 42), NOW);
    let fetch = DirFetch::new(&yard);
    let cache = root.join("cache");
    Setup { root, yard, fetch, cache }
}

impl Drop for Setup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn id(s: &str) -> ExtId {
    ExtId::parse(s).unwrap()
}

#[test]
fn the_index_is_fetched_at_most_once_a_day_and_a_304_reads_nothing_new() {
    let s = setup("daily");
    let reg = s.yard.registry();
    let v = index(&reg, &s.cache, &s.fetch, NOW, false).unwrap();
    assert_eq!(v.index.extensions.len(), 1);
    assert_eq!(s.fetch.count(), 4, "index, then root, its signature and the index signature");
    // Within a day: nothing is asked.
    index(&reg, &s.cache, &s.fetch, NOW + 3600, false).unwrap();
    assert_eq!(s.fetch.count(), 4);
    assert_eq!(cached(&reg, &s.cache, NOW).unwrap().index.version, v.index.version);
    // A day later the index is asked again with its ETag; unchanged, it is the only request.
    index(&reg, &s.cache, &s.fetch, NOW + REFRESH_SECS, false).unwrap();
    assert_eq!(s.fetch.count(), 5);
    assert_eq!(fetched_at(&s.cache), Some(NOW + REFRESH_SECS));
    // `force` (the user opened the Market, or asked for an install) asks now.
    index(&reg, &s.cache, &s.fetch, NOW + REFRESH_SECS + 1, true).unwrap();
    assert_eq!(s.fetch.count(), 6);
}

#[test]
fn a_new_index_is_read_and_an_older_copy_is_refused() {
    let mut s = setup("rollback");
    let reg = s.yard.registry();
    index(&reg, &s.cache, &s.fetch, NOW, true).unwrap();
    let old: Vec<Vec<u8>> = ["index.json", "index.json.sig"].iter().map(|f| std::fs::read(s.yard.dir.join(f)).unwrap()).collect();
    s.yard.publish(&package("you.day", "1.0.0"), &github(7, 43), NOW);
    assert_eq!(index(&reg, &s.cache, &s.fetch, NOW, true).unwrap().index.extensions.len(), 2);
    // Someone serves the older, still validly signed copy.
    std::fs::write(s.yard.dir.join("index.json"), &old[0]).unwrap();
    std::fs::write(s.yard.dir.join("index.json.sig"), &old[1]).unwrap();
    let e = index(&reg, &s.cache, &s.fetch, NOW, true).unwrap_err();
    assert!(matches!(e, Error::Index(yard::Error::Rollback { .. })), "{e}");
    // The cache keeps the newer one.
    assert_eq!(cached(&reg, &s.cache, NOW).unwrap().index.extensions.len(), 2);
}

#[test]
fn a_stale_index_is_marked_not_refused() {
    let s = setup("stale");
    let v = index(&s.yard.registry(), &s.cache, &s.fetch, NOW + yard::INDEX_LIFETIME_SECS, true).unwrap();
    assert!(v.stale);
}

#[test]
fn a_package_is_checked_against_the_index_and_its_signature() {
    let s = setup("download");
    let reg = s.yard.registry();
    let v = index(&reg, &s.cache, &s.fetch, NOW, true).unwrap();
    let d = download(&reg, &v, &id("you.night"), None, false, &s.fetch, NOW).unwrap();
    assert_eq!(d.manifest.version.to_string(), "1.1.0");
    assert_eq!(d.signed.publisher, github(7, 42));
    let one = download(&reg, &v, &id("you.night"), Some(&Version::parse("1.0.0").unwrap()), false, &s.fetch, NOW).unwrap();
    assert_eq!(one.release.version.to_string(), "1.0.0");
    assert!(matches!(download(&reg, &v, &id("you.none"), None, false, &s.fetch, NOW), Err(Error::NotFound(_))));
    assert!(matches!(download(&reg, &v, &id("you.night"), Some(&Version::parse("9.0.0").unwrap()), false, &s.fetch, NOW), Err(Error::NotFound(_))));
}

#[test]
fn tampered_packages_and_signatures_install_nothing() {
    let s = setup("tamper");
    let reg = s.yard.registry();
    let v = index(&reg, &s.cache, &s.fetch, NOW, true).unwrap();
    let digest = v.index.latest(&id("you.night"), false).unwrap().digest.clone();
    // A different package under the same name.
    let original = std::fs::read(s.yard.dir.join(format!("packages/{digest}.cyx"))).unwrap();
    std::fs::write(s.yard.dir.join(format!("packages/{digest}.cyx")), package("you.night", "1.0.0")).unwrap();
    let e = download(&reg, &v, &id("you.night"), None, false, &s.fetch, NOW).unwrap_err();
    assert!(matches!(&e, Error::Package(m) if m.contains("does not match the registry's index")), "{e}");
    std::fs::write(s.yard.dir.join(format!("packages/{digest}.cyx")), &original).unwrap();
    // A signature for another version.
    let other = v.index.release(&id("you.night"), &Version::parse("1.0.0").unwrap()).unwrap().digest.clone();
    std::fs::copy(s.yard.dir.join(format!("packages/{other}.sigstore.json")), s.yard.dir.join(format!("packages/{digest}.sigstore.json"))).unwrap();
    let e = download(&reg, &v, &id("you.night"), None, false, &s.fetch, NOW).unwrap_err();
    assert!(matches!(e, Error::Signature(_)), "{e}");
}

#[test]
fn the_signer_must_be_the_listed_publisher() {
    let mut s = setup("signer");
    // The registry lists one publisher, but the package was signed by another.
    s.yard.publish(&package("you.dusk", "1.0.0"), &github(7, 42), NOW);
    let bytes = package("you.dusk", "1.1.0");
    s.yard.publish(&bytes, &github(9, 9), NOW);
    let reg = s.yard.registry();
    let v = index(&reg, &s.cache, &s.fetch, NOW, true).unwrap();
    let e = download(&reg, &v, &id("you.dusk"), None, false, &s.fetch, NOW).unwrap_err();
    assert!(matches!(&e, Error::Package(m) if m.contains("the registry lists")), "{e}");
}

#[test]
fn yanked_and_revoked_versions_are_not_installed() {
    let mut s = setup("status");
    s.yard.set_status("you.night", "1.1.0", Status::Yanked, NOW);
    let reg = s.yard.registry();
    let v = index(&reg, &s.cache, &s.fetch, NOW, true).unwrap();
    // The newest installable is the one before.
    assert_eq!(download(&reg, &v, &id("you.night"), None, false, &s.fetch, NOW).unwrap().release.version.to_string(), "1.0.0");
    let e = download(&reg, &v, &id("you.night"), Some(&Version::parse("1.1.0").unwrap()), false, &s.fetch, NOW).unwrap_err();
    assert!(e.to_string().contains("withdrawn by its publisher"), "{e}");
    s.yard.set_status("you.night", "1.0.0", Status::Revoked { reason: "Malware.".into() }, NOW);
    let v = index(&reg, &s.cache, &s.fetch, NOW, true).unwrap();
    let e = download(&reg, &v, &id("you.night"), Some(&Version::parse("1.0.0").unwrap()), false, &s.fetch, NOW).unwrap_err();
    assert!(e.to_string().contains("revoked by the registry: Malware."), "{e}");
}

#[test]
fn another_registrys_keys_refuse_everything() {
    let s = setup("keys");
    let mut reg = s.yard.registry();
    reg.keys = RootKeys { keys: vec![yard::SigningKey::generate().unwrap().0.public()], threshold: 1 };
    assert!(matches!(index(&reg, &s.cache, &s.fetch, NOW, true), Err(Error::Index(yard::Error::Signature { .. }))));
    assert!(cached(&reg, &s.cache, NOW).is_none());
}

#[test]
fn pins_come_from_the_same_registry() {
    let locked = |source: crate::Source| crate::Locked {
        version: Version::parse("1.0.0").unwrap(),
        digest: ext_pkg::ContentDigest([0; 32]),
        previous: None,
        source,
        disabled: false,
        pre_release: false,
        targets: Default::default(),
    };
    let me = github(7, 42);
    let from_test = locked(crate::Source::Registry { registry: "test".into(), publisher: me.clone() });
    assert_eq!(pin(Some(&from_test), "test", &me), publisher::Pin::Same);
    assert!(matches!(pin(Some(&from_test), "test", &github(8, 1)), publisher::Pin::Changed { .. }));
    assert_eq!(pin(Some(&from_test), "other", &me), publisher::Pin::New);
    assert_eq!(pin(Some(&locked(crate::Source::File { path: "x.cyx".into() })), "test", &me), publisher::Pin::New, "a sideloaded copy has no pin");
    assert_eq!(pin(None, "test", &me), publisher::Pin::New);
}

#[test]
fn settings_entries_and_specs() {
    let key = "ab".repeat(32);
    let ok = serde_json::json!({ "name": "acme", "url": "https://yard.acme.test/", "root-keys": [key], "threshold": 1 });
    let r = Registry::from_setting(&ok).unwrap();
    assert_eq!((r.name.as_str(), r.url.as_str(), r.keys.threshold), ("acme", "https://yard.acme.test", 1));
    for bad in [
        serde_json::json!({ "name": "Acme", "url": "https://x", "root-keys": [key] }),
        serde_json::json!({ "name": "acme", "url": "http://x", "root-keys": [key] }),
        serde_json::json!({ "name": "acme", "url": "https://x", "root-keys": [] }),
        serde_json::json!({ "name": "acme", "url": "https://x", "root-keys": ["zz"] }),
        serde_json::json!({ "name": "acme", "url": "https://x", "root-keys": [key], "threshold": 2 }),
    ] {
        assert!(Registry::from_setting(&bad).is_err(), "{bad}");
    }
    assert_eq!(parse_spec("you.night").unwrap(), (id("you.night"), None));
    assert_eq!(parse_spec("you.night@1.2.0-beta.1").unwrap().1.unwrap().to_string(), "1.2.0-beta.1");
    assert!(parse_spec("you.night@x").is_err());
    assert!(parse_spec("night").is_err());
}

#[test]
fn a_registry_that_moved_on_without_us_is_still_read_through_the_retry() {
    // The index and its signature disagree once (written between two reads); the second attempt reads both again.
    let s = setup("retry");
    let reg = s.yard.registry();
    s.yard.tamper("index.json.sig", |b| b.reverse());
    let e = index(&reg, &s.cache, &s.fetch, NOW, true).unwrap_err();
    assert!(matches!(e, Error::Index(_)), "{e}");
    assert_eq!(s.fetch.count(), 8, "two attempts, four files each");
}

#[test]
fn a_page_is_fetched_once_checked_against_the_index_and_cached() {
    let s = setup("page");
    let reg = s.yard.registry();
    let v = index(&reg, &s.cache, &s.fetch, NOW, true).unwrap();
    let r = v.index.latest(&id("you.night"), false).unwrap();
    let dir = cache_dir(&s.cache, &reg);
    let before = s.fetch.count();
    let p = page(&reg, &dir, &id("you.night"), r, &s.fetch).unwrap();
    assert_eq!((p.manifest.name.as_str(), p.manifest.version.to_string().as_str()), ("Night", "1.1.0"));
    assert_eq!(s.fetch.count(), before + 1, "one request, and not for the package");
    assert!(s.fetch.requests.lock().unwrap().last().unwrap().ends_with(".details.json"));
    page(&reg, &dir, &id("you.night"), r, &s.fetch).unwrap();
    assert_eq!(s.fetch.count(), before + 1, "opened again: from the cache");
}

#[test]
fn a_changed_page_or_one_for_another_package_is_refused() {
    let s = setup("page-tamper");
    let reg = s.yard.registry();
    let v = index(&reg, &s.cache, &s.fetch, NOW, true).unwrap();
    let dir = cache_dir(&s.cache, &reg);
    let new = v.index.latest(&id("you.night"), false).unwrap();
    let old = v.index.release(&id("you.night"), &Version::parse("1.0.0").unwrap()).unwrap();
    s.yard.tamper(&format!("packages/{}.details.json", new.digest), |b| b.extend_from_slice(b" "));
    let e = page(&reg, &dir, &id("you.night"), new, &s.fetch).unwrap_err();
    assert!(e.to_string().contains("does not match the registry's index"), "{e}");
    // The page of 1.0.0 served for 1.1.0, with the index saying so: the manifest gives it away.
    let mut lying = new.clone();
    lying.details = old.details.clone();
    lying.digest = old.digest.clone();
    let e = page(&reg, &dir, &id("you.night"), &lying, &s.fetch).unwrap_err();
    assert!(e.to_string().contains("is for another package"), "{e}");
    // A damaged cache copy is fetched again, not believed.
    page(&reg, &dir, &id("you.night"), old, &s.fetch).unwrap();
    let cached = dir.join("details").join(format!("{}.json", old.details.as_deref().unwrap()));
    std::fs::write(&cached, b"{}").unwrap();
    assert_eq!(page(&reg, &dir, &id("you.night"), old, &s.fetch).unwrap().manifest.version.to_string(), "1.0.0");
    let mut none = old.clone();
    none.details = None;
    assert!(matches!(page(&reg, &dir, &id("you.night"), &none, &s.fetch), Err(Error::NotFound(_))));
}

/// #146: a release with native programs has one package per platform; the client takes the workspace host's and gets
/// every platform's digest for the lock.
#[test]
fn a_native_release_is_downloaded_for_one_platform() {
    let (linux, mac) = ("x86_64-unknown-linux-musl", "aarch64-apple-darwin");
    let mut s = setup("native");
    s.yard.publish(&crate::testing::native_package("you.hello", "1.0.0", linux), &github(7, 42), NOW);
    let r = s.yard.publish(&crate::testing::native_package("you.hello", "1.0.0", mac), &github(7, 42), NOW);
    assert_eq!(r.targets.len(), 2);
    let reg = s.yard.registry();
    let v = index(&reg, &s.cache, &s.fetch, NOW, true).unwrap();
    let d = download_for(&reg, &v, &id("you.hello"), None, false, mac, &s.fetch, NOW).unwrap();
    assert_eq!(ext_pkg::read(&d.bytes).unwrap().target.as_deref(), Some(mac));
    assert_eq!(d.targets.len(), 2);
    assert_eq!(d.targets[linux].hex(), r.targets[linux].digest);
    let e = download_for(&reg, &v, &id("you.hello"), None, false, "x86_64-pc-windows-msvc", &s.fetch, NOW).unwrap_err();
    assert!(matches!(e, Error::NotFound(_)) && e.to_string().contains("not built for"), "{e}");
    // The index lists the Linux package under the Mac: its TARGET says otherwise, so nothing is installed.
    let swapped = r.targets[linux].clone();
    s.yard.edit_release("you.hello", "1.0.0", NOW, |r| drop(r.targets.insert(mac.into(), swapped)));
    let v = index(&reg, &s.cache, &s.fetch, NOW, true).unwrap();
    let e = download_for(&reg, &v, &id("you.hello"), None, false, mac, &s.fetch, NOW).unwrap_err();
    assert!(e.to_string().contains("platform"), "{e}");
}

/// e2e trial 2: with only pre-releases published, the install said "no version that can be installed".
#[test]
fn an_extension_with_only_pre_releases_says_so() {
    let mut s = setup("pre-only");
    s.yard.publish(&package("you.beta", "0.2.0-beta.1"), &github(7, 42), NOW);
    let reg = s.yard.registry();
    let v = index(&reg, &s.cache, &s.fetch, NOW, true).unwrap();
    let e = download_for(&reg, &v, &id("you.beta"), None, false, "aarch64-apple-darwin", &s.fetch, NOW).unwrap_err().to_string();
    assert!(e.contains("only pre-releases") && e.contains("you.beta@0.2.0-beta.1"), "{e}");
    assert!(download_for(&reg, &v, &id("you.beta"), None, true, "aarch64-apple-darwin", &s.fetch, NOW).is_ok());
}
