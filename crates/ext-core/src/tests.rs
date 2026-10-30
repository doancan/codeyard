use std::path::PathBuf;

use ext_manifest::{Clipboard, ExtId, Permissions};
use ext_pkg::Entry;

use crate::*;

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let d = std::env::temp_dir().join(format!("ext-core-{name}-{}", std::process::id()));
        let _ = remove_tree(&d);
        std::fs::create_dir_all(&d).unwrap();
        Dir(d)
    }

    fn store(&self) -> Store {
        Store::new(self.0.join("data/extensions"), self.0.join("config/extensions.lock.jsonc"))
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = remove_tree(&self.0);
    }
}

fn package(id: &str, version: &str, extra: &str) -> Vec<u8> {
    let manifest = format!("[extension]\nid = \"{id}\"\nversion = \"{version}\"\nname = \"N\"\nlicense = \"MIT\"\n{extra}");
    let files = vec![
        Entry { path: ext_manifest::FILE_NAME.into(), data: manifest.into_bytes(), executable: false },
        Entry { path: "themes/t.json".into(), data: format!("{{\"v\":\"{version}\"}}").into_bytes(), executable: false },
    ];
    ext_pkg::write(files).unwrap().0
}

fn theme(id: &str, version: &str) -> Vec<u8> {
    package(id, version, "[[themes]]\nlabel = \"T\"\npath = \"themes/t.json\"\n")
}

fn src() -> Source {
    Source::File { path: "/tmp/x.cyx".into() }
}

fn id(s: &str) -> ExtId {
    ExtId::parse(s).unwrap()
}

fn store_names(s: &Store) -> Vec<String> {
    let mut v: Vec<String> =
        std::fs::read_dir(s.root.join("store")).map(|r| r.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    v.sort();
    v
}

#[test]
fn install_load_and_restart() {
    let d = Dir::new("install");
    let s = d.store();
    let i = s.install(&theme("a.one", "1.0.0"), src()).unwrap();
    assert_eq!(i.manifest.id.as_str(), "a.one");
    assert!(s.store_dir(&i.digest).join("themes/t.json").is_file());
    assert!(s.data_dir(&id("a.one")).is_dir());
    let lock = std::fs::read_to_string(d.0.join("config/extensions.lock.jsonc")).unwrap();
    assert!(lock.starts_with("// Installed extensions."), "{lock}");
    assert!(lock.contains(&i.digest.hex()));
    // A new process: the index is current, no manifest is parsed.
    let loaded = d.store().load().unwrap();
    assert!(!loaded.rebuilt);
    assert_eq!(loaded.extensions.len(), 1);
    assert_eq!(loaded.extensions[0].manifest.contributions.themes[0].path, "themes/t.json");
    assert!(loaded.problems.is_empty());
}

#[test]
fn a_missing_lock_is_an_empty_set() {
    let d = Dir::new("empty");
    let loaded = d.store().load().unwrap();
    assert!(loaded.extensions.is_empty() && loaded.problems.is_empty());
}

#[test]
fn installed_files_are_read_only() {
    let d = Dir::new("readonly");
    let s = d.store();
    let i = s.install(&theme("a.one", "1.0.0"), src()).unwrap();
    let f = s.store_dir(&i.digest).join("themes/t.json");
    assert!(std::fs::metadata(&f).unwrap().permissions().readonly());
    assert!(std::fs::write(&f, "x").is_err());
}

#[test]
fn updating_keeps_the_previous_version_for_rollback() {
    let d = Dir::new("rollback");
    let s = d.store();
    let one = s.install(&theme("a.one", "1.0.0"), src()).unwrap();
    let two = s.install(&theme("a.one", "2.0.0"), src()).unwrap();
    assert_eq!(s.locked().unwrap()[&id("a.one")].previous, Some(one.digest));
    assert_eq!(store_names(&s), {
        let mut v = vec![one.digest.hex(), two.digest.hex()];
        v.sort();
        v
    });
    // Back without network: the old package is still in the store.
    let back = s.rollback(&id("a.one")).unwrap();
    assert_eq!(back.manifest.version.to_string(), "1.0.0");
    let l = &s.locked().unwrap()[&id("a.one")];
    assert_eq!((l.digest, l.previous), (one.digest, Some(two.digest)));
    assert_eq!(s.load().unwrap().extensions[0].manifest.version.to_string(), "1.0.0");
    // A third version drops the oldest one from the store.
    let three = s.install(&theme("a.one", "3.0.0"), src()).unwrap();
    assert_eq!(store_names(&s), {
        let mut v = vec![one.digest.hex(), three.digest.hex()];
        v.sort();
        v
    });
    assert!(matches!(d.store().rollback(&id("b.none")), Err(Error::NotInstalled(_))));
}

#[test]
fn reinstalling_the_same_package_changes_nothing() {
    let d = Dir::new("same");
    let s = d.store();
    let p = theme("a.one", "1.0.0");
    let first = s.install(&p, src()).unwrap();
    s.install(&p, src()).unwrap();
    let l = &s.locked().unwrap()[&id("a.one")];
    assert_eq!((l.digest, l.previous), (first.digest, None));
    assert!(matches!(s.rollback(&id("a.one")), Err(Error::NoPrevious(_))));
}

#[test]
fn uninstall_removes_store_data_and_entry() {
    let d = Dir::new("uninstall");
    let s = d.store();
    let i = s.install(&theme("a.one", "1.0.0"), src()).unwrap();
    s.install(&theme("b.two", "1.0.0"), src()).unwrap();
    std::fs::write(s.data_dir(&id("a.one")).join("state"), "x").unwrap();
    s.uninstall(&id("a.one")).unwrap();
    assert!(!s.store_dir(&i.digest).exists());
    assert!(!s.data_dir(&id("a.one")).exists());
    let loaded = s.load().unwrap();
    assert_eq!(loaded.extensions.iter().map(|e| e.manifest.id.as_str()).collect::<Vec<_>>(), vec!["b.two"]);
    assert!(matches!(s.uninstall(&id("a.one")), Err(Error::NotInstalled(_))));
}

#[test]
fn disable_and_enable() {
    let d = Dir::new("disable");
    let s = d.store();
    s.install(&theme("a.one", "1.0.0"), src()).unwrap();
    s.set_disabled(&id("a.one"), true).unwrap();
    assert!(s.load().unwrap().extensions[0].disabled);
    // An update keeps it off.
    s.install(&theme("a.one", "1.1.0"), src()).unwrap();
    assert!(s.load().unwrap().extensions[0].disabled);
    s.set_disabled(&id("a.one"), false).unwrap();
    assert!(!s.load().unwrap().extensions[0].disabled);
}

#[test]
fn a_crash_at_any_step_leaves_a_consistent_state() {
    for step in [Step::Unpack, Step::Rename, Step::Lock, Step::Index] {
        let d = Dir::new(&format!("crash-{step:?}"));
        let s = d.store();
        let one = s.install(&theme("a.one", "1.0.0"), src()).unwrap();
        s.fail_at.set(Some(step));
        let r = s.install(&theme("a.one", "2.0.0"), src());
        s.fail_at.set(None);
        let after = d.store();
        let loaded = after.load().unwrap();
        assert!(loaded.problems.is_empty(), "{step:?}: {:?}", loaded.problems);
        let v = loaded.extensions[0].manifest.version.to_string();
        match step {
            // Before the lock is written: the old set, and no temporary folder survives the next transaction.
            Step::Unpack | Step::Rename | Step::Lock => {
                assert!(r.is_err(), "{step:?}");
                assert_eq!(v, "1.0.0", "{step:?}");
                assert_eq!(loaded.extensions[0].digest, one.digest);
                after.install(&theme("b.two", "1.0.0"), src()).unwrap();
                assert!(store_names(&after).iter().all(|n| !n.starts_with(".tmp-")), "{step:?}: {:?}", store_names(&after));
            }
            // After the lock: the new set; the stale index is noticed and rebuilt.
            Step::Index => {
                assert!(r.is_ok());
                assert_eq!(v, "2.0.0");
                assert!(loaded.rebuilt);
                assert!(!after.load().unwrap().rebuilt, "rebuilt once, then current");
            }
        }
    }
}

#[test]
fn a_dead_temporary_folder_is_swept() {
    let d = Dir::new("sweep");
    let s = d.store();
    std::fs::create_dir_all(s.root.join("store/.tmp-dead-1/themes")).unwrap();
    std::fs::write(s.root.join("store/.tmp-dead-1/themes/t.json"), "x").unwrap();
    s.install(&theme("a.one", "1.0.0"), src()).unwrap();
    assert!(!s.root.join("store/.tmp-dead-1").exists());
}

#[test]
fn a_lock_edited_elsewhere_is_picked_up() {
    let d = Dir::new("synced");
    let s = d.store();
    s.install(&theme("a.one", "1.0.0"), src()).unwrap();
    s.install(&theme("b.two", "1.0.0"), src()).unwrap();
    assert!(!s.load().unwrap().rebuilt);
    // Another machine's lock (dotfiles) without b.two.
    let lock_path = d.0.join("config/extensions.lock.jsonc");
    let mut lock = Lock::parse(&std::fs::read_to_string(&lock_path).unwrap()).unwrap();
    lock.extensions.remove(&id("b.two"));
    std::fs::write(&lock_path, lock.render()).unwrap();
    let loaded = s.load().unwrap();
    assert!(loaded.rebuilt);
    assert_eq!(loaded.extensions.len(), 1);
}

#[test]
fn missing_or_damaged_store_folders_are_reported_not_fatal() {
    let d = Dir::new("damaged");
    let s = d.store();
    let a = s.install(&theme("a.one", "1.0.0"), src()).unwrap();
    s.install(&theme("b.two", "1.0.0"), src()).unwrap();
    remove_tree(&s.store_dir(&a.digest)).unwrap();
    let _ = std::fs::remove_file(s.index_path());
    let loaded = s.load().unwrap();
    assert_eq!(loaded.extensions.len(), 1);
    assert_eq!(loaded.problems.len(), 1);
    assert!(loaded.problems[0].starts_with("a.one: the installed files are missing"), "{:?}", loaded.problems);
    // Reinstalling repairs it.
    s.install(&theme("a.one", "1.0.0"), src()).unwrap();
    assert!(s.load().unwrap().problems.is_empty());
}

#[test]
fn verify_finds_changed_files() {
    let d = Dir::new("verify");
    let s = d.store();
    let i = s.install(&theme("a.one", "1.0.0"), src()).unwrap();
    assert!(s.verify(&i.digest).unwrap().is_empty());
    let f = s.store_dir(&i.digest).join("themes/t.json");
    let mut p = std::fs::metadata(&f).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    p.set_readonly(false);
    std::fs::set_permissions(&f, p).unwrap();
    std::fs::write(&f, "tampered").unwrap();
    assert_eq!(s.verify(&i.digest).unwrap(), vec!["themes/t.json"]);
}

#[test]
fn invalid_packages_install_nothing() {
    let d = Dir::new("invalid");
    let s = d.store();
    assert!(matches!(s.install(b"not a package", src()), Err(Error::Package(_))));
    assert!(s.load().unwrap().extensions.is_empty());
    assert!(!d.0.join("config/extensions.lock.jsonc").exists());
}

#[test]
fn folders_install_as_packages() {
    let d = Dir::new("folder");
    let ext = d.0.join("src-ext");
    std::fs::create_dir_all(ext.join("themes")).unwrap();
    std::fs::write(
        ext.join(ext_manifest::FILE_NAME),
        "[extension]\nid = \"a.dir\"\nversion = \"0.1.0\"\nname = \"N\"\nlicense = \"MIT\"\n[[themes]]\nlabel = \"T\"\npath = \"themes/t.json\"\n",
    )
    .unwrap();
    std::fs::write(ext.join("themes/t.json"), "{}").unwrap();
    let i = d.store().install_dir(&ext).unwrap();
    assert!(matches!(i.source, Source::Folder { .. }));
}

#[test]
fn lock_files_parse_with_comments_and_refuse_newer_formats() {
    let d = Dir::new("lockfmt");
    let s = d.store();
    s.install(&theme("a.one", "1.0.0"), src()).unwrap();
    let text = std::fs::read_to_string(d.0.join("config/extensions.lock.jsonc")).unwrap();
    let commented = text.replacen("{", "{ // mine\n /* block \"with quotes\" */", 1);
    assert_eq!(Lock::parse(&commented).unwrap(), Lock::parse(&text).unwrap());
    assert_eq!(Lock::parse("{\"format\": 9, \"extensions\": {}}"), Err(LockError::Newer(9)));
    assert!(matches!(Lock::parse("{\"format\": 1, \"extensions\": {\"Bad\": {}}}"), Err(LockError::Invalid(_))));
    assert!(matches!(Lock::parse("{\"format\": 1, \"extensions\": {}, \"extra\": 1}"), Err(LockError::Invalid(_))));
    assert_eq!(Lock::parse("").unwrap().extensions.len(), 0);
    let url =
        "{\"format\": 1, \"extensions\": {\"a.b\": {\"version\": \"1.0.0\", \"digest\": \"zz\", \"source\": {\"kind\": \"file\", \"path\": \"http://x//y\"}}}}";
    assert!(matches!(Lock::parse(url), Err(LockError::Invalid(e)) if e.contains("digest")));
}

#[test]
fn a_symlinked_lock_stays_a_link() {
    #[cfg(unix)]
    {
        let d = Dir::new("symlink");
        std::fs::create_dir_all(d.0.join("dotfiles")).unwrap();
        std::fs::create_dir_all(d.0.join("config")).unwrap();
        std::fs::write(d.0.join("dotfiles/lock.jsonc"), "").unwrap();
        std::os::unix::fs::symlink(d.0.join("dotfiles/lock.jsonc"), d.0.join("config/extensions.lock.jsonc")).unwrap();
        d.store().install(&theme("a.one", "1.0.0"), src()).unwrap();
        assert!(std::fs::symlink_metadata(d.0.join("config/extensions.lock.jsonc")).unwrap().file_type().is_symlink());
        assert!(std::fs::read_to_string(d.0.join("dotfiles/lock.jsonc")).unwrap().contains("a.one"));
    }
}

#[test]
fn two_stores_on_one_folder_do_not_interleave() {
    let d = Dir::new("concurrent");
    let root = d.0.clone();
    let handles: Vec<_> = (0..4)
        .map(|n| {
            let root = root.clone();
            std::thread::spawn(move || {
                let s = Store::new(root.join("data/extensions"), root.join("config/extensions.lock.jsonc"));
                for v in 0..3 {
                    s.install(&theme(&format!("p.e{n}"), &format!("1.0.{v}")), src()).unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let loaded = d.store().load().unwrap();
    assert_eq!(loaded.extensions.len(), 4, "every writer's last install is in the lock");
    assert!(loaded.extensions.iter().all(|e| e.manifest.version.to_string() == "1.0.2"));
}

#[test]
fn a_hundred_extensions_load_without_parsing() {
    let d = Dir::new("hundred");
    let s = d.store();
    for n in 0..100 {
        s.install(&theme(&format!("p.e{n}"), "1.0.0"), src()).unwrap();
    }
    let t = std::time::Instant::now();
    let loaded = d.store().load().unwrap();
    let took = t.elapsed();
    assert!(!loaded.rebuilt);
    assert_eq!(loaded.extensions.len(), 100);
    // Debug build on a shared runner; the release-build row is in perf/metrics.json.
    assert!(took < std::time::Duration::from_millis(250), "{took:?}");
}

// --- trust

fn manifest(extra: &str, code: bool) -> ext_manifest::Manifest {
    let code = if code { "[code]\nentry = \"Cargo.toml\"\n[[commands]]\nid = \"go\"\ntitle = \"Go\"\n" } else { "" };
    // Top-level keys (`untrusted`) must come before the first table.
    let (top, extra) = extra.split_once("\n[").map_or(("", extra.to_string()), |(t, rest)| (t, format!("[{rest}")));
    ext_manifest::parse(&format!("{top}\n[extension]\nid = \"a.b\"\nversion = \"1.0.0\"\nname = \"N\"\nlicense = \"MIT\"\n{code}{extra}")).unwrap()
}

#[test]
fn untrusted_folders_read_and_show_only() {
    let m = manifest(
        "\n[permissions]\nread = [\"vercel.json\"]\nwrite = [\"out.txt\"]\nnet = [\"api.x.com\"]\nrun = [\"vercel\"]\nclipboard = \"read-write\"\nterminal = true\nwhy = \"x\"\n",
        true,
    );
    let t = effective(&m, &Permissions::default(), Trust::Trusted);
    assert!(t.code_runs && t.programs_start);
    assert_eq!(t.permissions.run, vec!["vercel"]);
    let u = effective(&m, &Permissions::default(), Trust::Untrusted);
    assert!(u.code_runs && !u.programs_start);
    assert_eq!(u.permissions.read, vec!["vercel.json"]);
    assert!(u.permissions.write.is_empty() && u.permissions.net.is_empty() && u.permissions.run.is_empty() && !u.permissions.terminal);
    assert_eq!(u.permissions.clipboard, Clipboard::Read);
}

#[test]
fn opting_out_of_untrusted_folders_stops_the_code() {
    let m = manifest("untrusted = false\n[permissions]\nread = [\"x.json\"]\n", true);
    let u = effective(&m, &Permissions::default(), Trust::Untrusted);
    assert!(!u.code_runs);
    assert!(u.permissions.read.is_empty());
    assert!(effective(&m, &Permissions::default(), Trust::Trusted).code_runs);
}

#[test]
fn user_switches_remove_single_permissions() {
    let m = manifest("\n[permissions]\nnet = [\"a.x.com\", \"b.x.com\"]\nclipboard = \"read-write\"\nwhy = \"Posts what you copy to your team.\"\n", true);
    let off = Permissions { net: vec!["a.x.com".into()], clipboard: Clipboard::Read, ..Default::default() };
    let e = effective(&m, &off, Trust::Trusted);
    assert_eq!(e.permissions.net, vec!["b.x.com"]);
    assert_eq!(e.permissions.clipboard, Clipboard::Write);
}

#[test]
fn declarative_extensions_never_run_code() {
    let m = manifest("\n[permissions]\nrun = [\"taplo\"]\nwhy = \"Starts taplo.\"\n[[language-servers]]\ncommand = \"taplo\"\nlanguages = [\"toml\"]\n", false);
    let t = effective(&m, &Permissions::default(), Trust::Trusted);
    assert!(!t.code_runs && t.programs_start);
    assert!(!effective(&m, &Permissions::default(), Trust::Untrusted).programs_start);
}

#[test]
fn the_lock_is_format_1_until_a_registry_source_needs_2_a_pre_release_channel_3_and_platforms_4() {
    let locked = |source: Source| Locked {
        version: ext_manifest::Version::parse("1.0.0").unwrap(),
        digest: ContentDigest([1; 32]),
        previous: None,
        source,
        disabled: false,
        pre_release: false,
        targets: Default::default(),
    };
    let mut lock = Lock { format: lock::FORMAT, ..Default::default() };
    lock.extensions.insert(ExtId::parse("you.a").unwrap(), locked(Source::File { path: "a.cyx".into() }));
    let text = lock.render();
    assert!(text.contains("\"format\": 1"), "{text}");
    assert_eq!(Lock::parse(&text).unwrap().extensions, lock.extensions);
    let p = crate::testing::github(7, 42);
    lock.extensions.insert(ExtId::parse("you.b").unwrap(), locked(Source::Registry { registry: "codeyard".into(), publisher: p.clone() }));
    let text = lock.render();
    assert!(text.contains("\"format\": 2") && text.contains("\"kind\": \"registry\""), "{text}");
    let back = Lock::parse(&text).unwrap();
    assert_eq!(back.extensions[&ExtId::parse("you.b").unwrap()].source.registry(), Some(("codeyard", &p)));
    lock.extensions.get_mut(&ExtId::parse("you.b").unwrap()).unwrap().pre_release = true;
    let text = lock.render();
    assert!(text.contains("\"format\": 3") && text.contains("\"pre-release\": true"), "{text}");
    assert!(Lock::parse(&text).unwrap().extensions[&ExtId::parse("you.b").unwrap()].pre_release);
    // Per-platform digests of an extension with native programs need format 4.
    let t = "x86_64-unknown-linux-musl".to_string();
    lock.extensions.get_mut(&ExtId::parse("you.b").unwrap()).unwrap().targets.insert(t.clone(), ContentDigest([2; 32]));
    let text = lock.render();
    assert!(text.contains("\"format\": 4") && text.contains(&format!("\"{t}\": \"{}\"", ContentDigest([2; 32]).hex())), "{text}");
    assert_eq!(Lock::parse(&text).unwrap().extensions[&ExtId::parse("you.b").unwrap()].targets[&t], ContentDigest([2; 32]));
    assert!(Lock::parse(&text.replace(&t, "x86_64-unknown-plan9")).is_err(), "an unknown target is refused");
    // What an older Codeyard says about a newer format.
    assert_eq!(Lock::parse(&text.replace("\"format\": 4", "\"format\": 5")).unwrap_err(), LockError::Newer(5));
}

#[test]
fn revocations_and_the_pre_release_channel_are_kept_and_follow_the_lock() {
    let d = Dir::new("revoked");
    let s = d.store();
    let a1 = s.install(&theme("you.a", "1.0.0"), src()).unwrap();
    let mark = |r: &str| Some(Revoked { id: id("you.a"), version: a1.manifest.version.clone(), reason: r.into() });
    assert!(s.revoked().is_empty());
    assert!(s.set_revoked(&[(a1.digest, mark("Malware."))]).unwrap());
    assert!(!s.set_revoked(&[(a1.digest, mark("Malware."))]).unwrap(), "unchanged: not written again");
    assert_eq!(s.revoked()[&a1.digest.hex()].reason, "Malware.");
    // Kept while the version is in the store (current or previous), dropped with it.
    let a2 = s.install(&theme("you.a", "2.0.0"), src()).unwrap();
    s.set_revoked(&[]).unwrap();
    assert!(s.revoked().contains_key(&a1.digest.hex()), "the previous version stays revoked for a rollback");
    s.install(&theme("you.a", "3.0.0"), src()).unwrap();
    s.set_revoked(&[(a2.digest, None)]).unwrap();
    assert!(s.revoked().is_empty());

    s.set_pre_release(&id("you.a"), true).unwrap();
    assert!(s.locked().unwrap()[&id("you.a")].pre_release);
    s.install(&theme("you.a", "4.0.0"), src()).unwrap();
    assert!(s.locked().unwrap()[&id("you.a")].pre_release, "an update keeps the channel");
    s.rollback(&id("you.a")).unwrap();
    assert!(s.locked().unwrap()[&id("you.a")].pre_release, "and so does a rollback");
    assert!(matches!(s.set_pre_release(&id("you.zz"), true), Err(Error::NotInstalled(_))));
}

const LINUX: &str = "x86_64-unknown-linux-musl";
const MAC: &str = "aarch64-apple-darwin";

/// #146: one platform's package installs with the other platforms' digests in the lock; another platform's package is
/// added later only if it is the one the lock lists.
#[test]
fn platform_packages_are_kept_by_their_digests() {
    let d = Dir::new("targets");
    let s = d.store();
    let (linux, mac) = (crate::testing::native_package("you.hello", "1.0.0", LINUX), crate::testing::native_package("you.hello", "1.0.0", MAC));
    let mac_digest = ext_pkg::read(&mac).unwrap().digest;
    let listed = BTreeMap::from([(MAC.to_string(), mac_digest)]);
    let i = s.install_with_targets(&linux, src(), listed).unwrap();
    let l = &s.locked().unwrap()[&id("you.hello")];
    assert_eq!(l.targets.len(), 2, "its own platform is added: {:?}", l.targets);
    assert_eq!(l.targets[LINUX], i.digest);
    assert_eq!(s.target_dir(&id("you.hello"), LINUX), Some(s.store_dir(&i.digest)));
    assert_eq!(s.target_dir(&id("you.hello"), MAC), None, "listed, not fetched yet");
    // Another package for the Mac (another version's, or a changed one) is refused; the listed one is kept.
    let other = crate::testing::native_package("you.hello", "1.0.1", MAC);
    assert!(s.add_target(&id("you.hello"), &other).is_err());
    let changed = {
        let mut files: Vec<ext_pkg::Entry> = ext_pkg::read(&mac).unwrap().files.into_values().filter(|e| e.path != ext_pkg::CONTENTS).collect();
        files.iter_mut().find(|e| e.path == "bin/hello").unwrap().data.extend_from_slice(b"# changed\n");
        ext_pkg::write(files).unwrap().0
    };
    assert!(s.add_target(&id("you.hello"), &changed).unwrap_err().to_string().contains("does not match"));
    let dir = s.add_target(&id("you.hello"), &mac).unwrap();
    assert_eq!(s.target_dir(&id("you.hello"), MAC), Some(dir.clone()));
    assert!(dir.join("bin/hello").is_file());
    // Garbage collection keeps every platform of the current version.
    s.set_disabled(&id("you.hello"), true).unwrap();
    assert!(dir.is_dir());
    // A package whose own digest differs from the one listed for its platform is refused at install.
    let wrong = BTreeMap::from([(LINUX.to_string(), mac_digest)]);
    assert!(s.install_with_targets(&linux, src(), wrong).is_err());
}

/// A folder with native programs installs every platform in `native/`.
#[test]
fn a_native_folder_installs_every_platform() {
    let d = Dir::new("native-folder");
    let s = d.store();
    let folder = d.0.join("src");
    for t in [LINUX, MAC] {
        let bin = folder.join("native").join(t).join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("hello"), format!("#!/bin/sh\necho {t}\n")).unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(bin.join("hello"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    }
    std::fs::write(folder.join(ext_manifest::FILE_NAME), crate::testing::native_manifest("you.hello", "1.0.0")).unwrap();
    if !cfg!(unix) {
        return; // No executable bit to read from the folder.
    }
    let i = s.install_dir(&folder).unwrap();
    let l = &s.locked().unwrap()[&id("you.hello")];
    assert_eq!(l.targets.keys().map(String::as_str).collect::<Vec<_>>(), [MAC, LINUX]);
    for t in [LINUX, MAC] {
        let dir = s.target_dir(&id("you.hello"), t).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("bin/hello")).unwrap(), format!("#!/bin/sh\necho {t}\n"));
    }
    if let Some(here) = ext_manifest::this_target().filter(|t| [LINUX, MAC].contains(t)) {
        assert_eq!(l.targets[here], i.digest, "this machine's platform is the one installed");
    }
}
