use std::collections::BTreeMap;

use ext_manifest::Version;
use ext_pkg::ContentDigest;

use super::*;
use crate::Source;
use crate::testing::github;
use crate::yard::{Entry, FORMAT};

const NOW: u64 = 1_800_000_000;
const DAY: u64 = 24 * 3600;

fn release(version: &str, status: Status, code: bool, published: u64) -> Release {
    Release {
        version: Version::parse(version).unwrap(),
        digest: "00".repeat(32),
        sha256: "00".repeat(32),
        size: 1,
        permissions: "00".repeat(32),
        code,
        published,
        status,
        downloads: 0,
        immediate: false,
        details: None,
        targets: Default::default(),
    }
}

fn index(versions: Vec<Release>) -> Index {
    let entry = Entry { name: "A".into(), description: String::new(), categories: Vec::new(), publisher: github(7, 42), versions };
    Index { format: FORMAT, registry: "t".into(), version: 1, expires: NOW + DAY, extensions: BTreeMap::from([(id(), entry)]) }
}

fn id() -> ExtId {
    ExtId::parse("you.a").unwrap()
}

fn on(version: &str, pre_release: bool) -> Locked {
    Locked {
        version: Version::parse(version).unwrap(),
        digest: ContentDigest([1; 32]),
        previous: None,
        source: Source::Registry { registry: "t".into(), publisher: github(7, 42) },
        disabled: false,
        pre_release,
        targets: Default::default(),
    }
}

fn next_version(c: &Check) -> Option<(&'static str, String)> {
    match &c.next {
        Next::Nothing => None,
        Next::RollBack(r) => Some(("back", r.version.to_string())),
        Next::Update { release, .. } => Some(("up", release.version.to_string())),
    }
}

#[test]
fn data_updates_are_ready_at_once_and_code_after_48_hours() {
    let ok = Status::Ok;
    let i = index(vec![release("1.0.0", ok.clone(), false, NOW - 9 * DAY), release("1.1.0", ok.clone(), false, NOW - 60)]);
    let c = check(&i, &id(), &on("1.0.0", false), NOW);
    assert_eq!((c.standing, c.next), (Standing::Ok, Next::Update { release: i.extensions[&id()].versions[1].clone(), ready_at: NOW - 60 }));

    let mut code = release("1.1.0", ok.clone(), true, NOW - 60);
    assert_eq!(ready_at(&code), NOW - 60 + CODE_WAIT_SECS);
    code.immediate = true;
    assert_eq!(ready_at(&code), NOW - 60, "an advisory fix the registry cleared");
    let beta = release("1.1.0-beta.1", ok, true, NOW - 60);
    assert_eq!(ready_at(&beta), NOW - 60, "the pre-release channel has no wait");
}

#[test]
fn pre_releases_come_only_on_the_channel() {
    let ok = Status::Ok;
    let i = index(vec![release("1.0.0", ok.clone(), false, 0), release("1.1.0-beta.1", ok.clone(), false, 0)]);
    assert_eq!(next_version(&check(&i, &id(), &on("1.0.0", false), NOW)), None);
    assert_eq!(next_version(&check(&i, &id(), &on("1.0.0", true), NOW)), Some(("up", "1.1.0-beta.1".into())));
    // Off the channel, a release that passes the pre-release still arrives.
    let i = index(vec![release("1.1.0-beta.1", ok.clone(), false, 0), release("1.1.0", ok, false, 0)]);
    assert_eq!(next_version(&check(&i, &id(), &on("1.1.0-beta.1", false), NOW)), Some(("up", "1.1.0".into())));
}

#[test]
fn a_yanked_version_goes_back_to_the_newest_earlier_one() {
    let ok = Status::Ok;
    let i = index(vec![
        release("1.0.0", ok.clone(), false, 0),
        release("1.1.0", Status::Revoked { reason: "Bad.".into() }, false, 0),
        release("1.2.0-beta.1", ok.clone(), false, 0),
        release("1.2.0", Status::Yanked, false, 0),
    ]);
    let c = check(&i, &id(), &on("1.2.0", false), NOW);
    assert_eq!(c.standing, Standing::Yanked);
    assert_eq!(next_version(&c), Some(("back", "1.0.0".into())), "not revoked 1.1.0, nor a pre-release off the channel");
    assert_eq!(next_version(&check(&i, &id(), &on("1.2.0", true), NOW)), Some(("back", "1.2.0-beta.1".into())));
    // Nothing earlier to go to: it stays, and the user is told.
    let i = index(vec![release("1.0.0", Status::Yanked, false, 0)]);
    assert_eq!(check(&i, &id(), &on("1.0.0", false), NOW), Check { standing: Standing::Yanked, next: Next::Nothing });
}

#[test]
fn a_yanked_version_moves_forward_when_the_fix_may_install_now() {
    let ok = Status::Ok;
    let mut i = index(vec![release("1.0.0", ok.clone(), false, 0), release("1.1.0", Status::Yanked, false, 0), release("1.2.0", ok, true, NOW - 60)]);
    // The fix runs code and waits 48 h: back to 1.0.0 meanwhile.
    assert_eq!(next_version(&check(&i, &id(), &on("1.1.0", false), NOW)), Some(("back", "1.0.0".into())));
    assert_eq!(next_version(&check(&i, &id(), &on("1.1.0", false), NOW + CODE_WAIT_SECS)), Some(("up", "1.2.0".into())));
    i.extensions.get_mut(&id()).unwrap().versions[2].immediate = true;
    assert_eq!(next_version(&check(&i, &id(), &on("1.1.0", false), NOW)), Some(("up", "1.2.0".into())));
}

#[test]
fn revoked_deprecated_and_unlisted_versions_say_so() {
    let ok = Status::Ok;
    let revoked = Status::Revoked { reason: "Steals tokens.".into() };
    let i = index(vec![release("1.0.0", revoked, false, 0)]);
    assert_eq!(check(&i, &id(), &on("1.0.0", false), NOW), Check { standing: Standing::Revoked { reason: "Steals tokens.".into() }, next: Next::Nothing });
    // A newer version replaces a revoked one like any update.
    let i = index(vec![release("1.0.0", Status::Revoked { reason: "x".into() }, false, 0), release("1.0.1", ok.clone(), false, 0)]);
    assert_eq!(next_version(&check(&i, &id(), &on("1.0.0", false), NOW)), Some(("up", "1.0.1".into())));

    let b = ExtId::parse("you.b").unwrap();
    let i = index(vec![release("1.0.0", Status::Deprecated { replacement: Some(b.clone()) }, false, 0)]);
    assert_eq!(check(&i, &id(), &on("1.0.0", false), NOW).standing, Standing::Deprecated { replacement: Some(b) });
    assert_eq!(check(&i, &id(), &on("0.9.0", false), NOW).standing, Standing::Unlisted);
    assert_eq!(AutoUpdate::parse("notify"), Some(AutoUpdate::Notify));
    assert_eq!(AutoUpdate::parse("always"), None);
    assert_eq!(AutoUpdate::default(), AutoUpdate::Install);
}

#[test]
fn a_revoked_version_stops_with_everything_that_depends_on_it() {
    use crate::{Installed, Revoked, Stopped, stopped};
    let ext = |id: &str, deps: &[&str], d: u8| {
        let deps: String = deps.iter().map(|d| format!("\"{d}\" = \"1\"\n")).collect();
        let text = format!("[extension]\nid = \"{id}\"\nversion = \"1.0.0\"\nname = \"X\"\nlicense = \"MIT\"\n[dependencies]\n{deps}");
        Installed { manifest: ext_manifest::parse(&text).unwrap(), digest: ContentDigest([d; 32]), disabled: false, source: Source::File { path: "x".into() } }
    };
    let all = [ext("you.base", &[], 1), ext("you.mid", &["you.base"], 2), ext("you.top", &["you.mid"], 3), ext("you.free", &[], 4)];
    let revoked = BTreeMap::from([(
        ContentDigest([1; 32]).hex(),
        Revoked { id: ExtId::parse("you.base").unwrap(), version: Version::parse("1.0.0").unwrap(), reason: "Malware.".into() },
    )]);
    let s = stopped(&all, &revoked);
    let get = |id: &str| s.get(&ExtId::parse(id).unwrap()).cloned();
    assert_eq!(get("you.base"), Some(Stopped::Revoked { reason: "Malware.".into() }));
    assert_eq!(get("you.mid"), Some(Stopped::Dependency { on: ExtId::parse("you.base").unwrap() }));
    assert_eq!(get("you.top"), Some(Stopped::Dependency { on: ExtId::parse("you.mid").unwrap() }));
    assert_eq!(get("you.free"), None);
}
