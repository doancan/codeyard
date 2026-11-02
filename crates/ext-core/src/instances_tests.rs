use std::path::PathBuf;

use ext_manifest::ExtId;

use super::*;
use crate::yard::Publisher;

struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn store(name: &str) -> (Dir, Store) {
    let d = std::env::temp_dir().join(format!("ext-core-instances-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let s = Store::new(d.join("data/extensions"), d.join("config/extensions.lock.jsonc"));
    (Dir(d), s)
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

const MANIFEST: &str = r#"
[extension]
id = "you.forge"
version = "0.1.0"
name = "Forge"
license = "MIT"

[code]
entry = "Cargo.toml"

[permissions]
auth = ["forge", "fixed"]
why = "x"

[[oauth]]
id = "forge"
name = "Forge"
authorize-url = "https://forge.example.com/oauth/authorize"
token-url = "https://forge.example.com/oauth/token"
client-id = "cy-1"
hosts = ["forge.example.com"]
instances = true

[[oauth]]
id = "fixed"
name = "Fixed"
authorize-url = "https://fixed.example.com/oauth/authorize"
token-url = "https://fixed.example.com/oauth/token"
client-id = "cy-2"
hosts = ["fixed.example.com"]

[[pull-requests]]
id = "forge"
label = "Forge"
hosts = ["forge.example.com"]
auth = "forge"
"#;

fn manifest() -> Manifest {
    ext_manifest::parse(MANIFEST).unwrap()
}

#[test]
fn what_the_user_types_is_checked() {
    let m = manifest();
    let i = Instance::new(&m, "forge", " HTTPS://Git.Corp.Example/ ", " abc123 ").unwrap();
    assert_eq!((i.host.as_str(), i.client_id.as_str(), i.account()), ("git.corp.example", "abc123", "forge@git.corp.example".to_string()));
    assert_eq!(i.oauth(&m).unwrap().token_url, "https://git.corp.example/oauth/token");
    for (provider, host, client) in [
        ("fixed", "git.corp.example", "abc"),      // the manifest does not offer instances
        ("nope", "git.corp.example", "abc"),       // no such provider
        ("forge", "forge.example.com", "abc"),     // its own host
        ("forge", "10.0.0.1", "abc"),              // an address, not a host name
        ("forge", "git.corp.example:8443", "abc"), // a port
        ("forge", "git.corp.example/gitlab", "abc"),
        ("forge", "*.corp.example", "abc"),
        ("forge", "git.corp.example", "two words"),
        ("forge", "git.corp.example", ""),
    ] {
        assert!(Instance::new(&m, provider, host, client).is_err(), "{provider} {host} {client:?}");
    }
}

#[test]
fn instances_are_kept_per_extension_and_publisher() {
    let (_d, s) = store("save");
    let id = ExtId::parse("you.forge").unwrap();
    let m = manifest();
    let a = Instance::new(&m, "forge", "git.corp.example", "abc").unwrap();
    let b = Instance::new(&m, "forge", "code.other.example", "def").unwrap();
    s.add_instance(&id, &registry(7), a.clone()).unwrap();
    s.add_instance(&id, &registry(7), b.clone()).unwrap();
    assert_eq!(s.instances(&id, &registry(7)), [a.clone(), b.clone()]);
    // The same host again replaces it.
    let a2 = Instance { client_id: "xyz".into(), ..a.clone() };
    s.add_instance(&id, &registry(7), a2.clone()).unwrap();
    assert_eq!(s.instances(&id, &registry(7)), [b.clone(), a2.clone()]);
    // Another publisher with the same id, or a sideloaded copy, gets none of them.
    assert!(s.instances(&id, &registry(8)).is_empty());
    assert!(s.instances(&id, &Source::File { path: "/dl/forge.cyx".into() }).is_empty());
    s.remove_instance(&id, "forge@code.other.example").unwrap();
    assert_eq!(s.instances(&id, &registry(7)), [a2]);
    s.remove_instance(&id, "forge@git.corp.example").unwrap();
    assert!(s.instances(&id, &registry(7)).is_empty());
    // A record of another publisher is replaced, not merged.
    s.add_instance(&id, &registry(7), a.clone()).unwrap();
    s.add_instance(&id, &registry(8), b.clone()).unwrap();
    assert!(s.instances(&id, &registry(7)).is_empty());
    assert_eq!(s.instances(&id, &registry(8)), [b]);
}

#[test]
fn hosts_allowed_for_net_from_setting_are_kept_per_publisher() {
    let m = ext_manifest::parse(
        "[extension]\nid = \"you.ghe\"\nversion = \"0.1.0\"\nname = \"GHE\"\nlicense = \"MIT\"\n[code]\nentry = \"Cargo.toml\"\n\
         [permissions]\nnet-from-setting = \"server\"\n[settings.server]\ntype = \"string\"\ndescription = \"Server\"\n\
         [[commands]]\nid = \"go\"\ntitle = \"Go\"\n",
    )
    .unwrap();
    assert_eq!(user_host(&m, " https://GHE.example.com/ "), Ok("ghe.example.com".into()));
    assert!(user_host(&m, "ghe.example.com:8443").is_err());
    assert!(user_host(&m, "ghe.example.com/api").is_err());
    assert!(user_host(&m, "10.0.0.1").is_err(), "no IP literals, as in net");
    let plain = ext_manifest::parse(&MANIFEST.replace("you.forge", "you.plain")).unwrap();
    assert!(user_host(&plain, "ghe.example.com").unwrap_err().contains("does not connect to servers named in its settings"));

    let (_d, s) = store("hosts");
    let id = ExtId::parse("you.ghe").unwrap();
    s.add_user_host(&id, &registry(9), "ghe.example.com").unwrap();
    s.add_user_host(&id, &registry(9), "ghe.example.com").unwrap();
    // Nothing is installed yet, so no record matches an installed identity.
    assert!(s.all_user_hosts().is_empty());
    let text = std::fs::read_to_string(s.instances_path()).unwrap();
    assert_eq!(text.matches("ghe.example.com").count(), 1, "{text}");
    // Another publisher with the same id starts over.
    s.add_user_host(&id, &registry(10), "other.example.com").unwrap();
    let text = std::fs::read_to_string(s.instances_path()).unwrap();
    assert!(!text.contains("ghe.example.com") && text.contains("other.example.com"), "{text}");
    s.remove_user_host(&id, "other.example.com").unwrap();
    assert!(!std::fs::read_to_string(s.instances_path()).unwrap().contains("you.ghe"));
}
