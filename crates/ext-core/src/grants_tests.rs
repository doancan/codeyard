use ext_manifest::{Clipboard, ExtId, Permissions};

use super::*;
use crate::yard::Publisher;

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let d = std::env::temp_dir().join(format!("ext-core-grants-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Dir(d)
    }

    fn store(&self) -> Store {
        Store::new(self.0.join("data/extensions"), self.0.join("config/extensions.lock.jsonc"))
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn id() -> ExtId {
    ExtId::parse("you.probe").unwrap()
}

fn registry(repo: u64) -> Source {
    let publisher = Publisher {
        issuer: "https://token.actions.githubusercontent.com".into(),
        subject: None,
        owner_id: Some(1),
        repository_id: Some(repo),
        workflow: Some(".github/workflows/release.yml".into()),
    };
    Source::Registry { registry: "yard".into(), publisher }
}

fn declared() -> Permissions {
    Permissions {
        read: vec!["a.txt".into()],
        net: vec!["api.example.com".into(), "cdn.example.com".into()],
        secrets: true,
        clipboard: Clipboard::ReadWrite,
        ..Default::default()
    }
}

#[test]
fn switches_are_saved_per_extension_and_publisher() {
    let d = Dir::new("save");
    let store = d.store();
    let net = Entry { kind: "net", value: "api.example.com".into() };
    let off = switch(&Permissions::default(), &net, false);
    store.set_revoked_permissions(&id(), &registry(7), &off).unwrap();
    assert!(store.permissions_path().is_file());
    assert_eq!(store.revoked_permissions(&id(), &registry(7)), off);
    // The same id from another repository, or sideloaded, starts with no decision carried over.
    assert_eq!(store.revoked_permissions(&id(), &registry(8)), Permissions::default());
    assert_eq!(store.revoked_permissions(&id(), &Source::File { path: "/dl/probe.cyx".into() }), Permissions::default());
    // Turning it on again removes the record.
    store.set_revoked_permissions(&id(), &registry(7), &switch(&off, &net, true)).unwrap();
    assert_eq!(store.revoked_permissions(&id(), &registry(7)), Permissions::default());
}

#[test]
fn a_damaged_file_is_no_switches() {
    let d = Dir::new("damaged");
    let store = d.store();
    std::fs::create_dir_all(store.permissions_path().parent().unwrap()).unwrap();
    std::fs::write(store.permissions_path(), "{ not json").unwrap();
    assert_eq!(store.revoked_permissions(&id(), &registry(7)), Permissions::default());
    assert!(store.all_revoked_permissions().is_empty());
}

#[test]
fn each_declared_entry_is_one_switch() {
    let e = entries(&declared());
    let lines: Vec<String> = e.iter().map(Entry::manifest_line).collect();
    assert_eq!(
        lines,
        [
            r#"read = ["a.txt"]"#,
            r#"net = ["api.example.com"]"#,
            r#"net = ["cdn.example.com"]"#,
            "secrets = true",
            r#"clipboard = "read""#,
            r#"clipboard = "write""#,
        ]
    );
    // Turning off one host keeps the other, and `effective` applies the switch.
    let off = switch(&Permissions::default(), &e[1], false);
    assert!(is_revoked(&off, &e[1]) && !is_revoked(&off, &e[2]));
    let text = "[extension]\nid = \"you.probe\"\nversion = \"1.0.0\"\nname = \"P\"\nlicense = \"MIT\"\n[code]\nentry = \"Cargo.toml\"\n\
                [permissions]\nnet = [\"api.example.com\", \"cdn.example.com\"]\nwhy = \"Shows deploys.\"\n[[commands]]\nid = \"go\"\ntitle = \"Go\"\n";
    let m = ext_manifest::parse(text).unwrap();
    let eff = crate::trust::effective(&m, &off, crate::Trust::Trusted);
    assert_eq!(eff.permissions.net, ["cdn.example.com"]);
    // Clipboard read and write are separate switches.
    let no_read = switch(&Permissions::default(), &e[4], false);
    assert_eq!(no_read.clipboard, Clipboard::Read);
    assert_eq!(switch(&no_read, &e[5], false).clipboard, Clipboard::ReadWrite);
    assert_eq!(switch(&no_read, &e[4], true).clipboard, Clipboard::None);
    let secrets = switch(&Permissions::default(), &e[3], false);
    assert!(secrets.secrets && is_revoked(&secrets, &e[3]));
}
