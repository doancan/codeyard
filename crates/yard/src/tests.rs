use std::io::{Read as _, Write as _};
use std::path::PathBuf;

use ext_core::testing::{Authority, github};
use ext_core::yard::{self as y, Publisher, RootKeys, Seen, Status, StatusChange};
use ext_manifest::{ExtId, Version};

use super::*;

const NOW: u64 = 1_790_000_000;

struct Test {
    dir: PathBuf,
    authority: Authority,
    yard: Yard,
}

fn manifest(id: &str, version: &str, name: &str, extra: &str) -> String {
    format!(
        "[extension]\nid = \"{id}\"\nversion = \"{version}\"\nname = \"{name}\"\nlicense = \"MIT\"\ndescription = \"A test.\"\ncategories = [\"themes\"]\n{extra}"
    )
}

const THEME: &str = r##"{ "name": "Night", "type": "dark", "colors": { "editor.background": "#000000" } }"##;

/// A theme package: (bytes, content digest).
fn package(id: &str, version: &str, name: &str) -> (Vec<u8>, ext_pkg::ContentDigest) {
    package_with(&manifest(id, version, name, "[[themes]]\nlabel = \"Night\"\npath = \"t.json\"\n"), &[("t.json", THEME)])
}

fn package_with(manifest: &str, files: &[(&str, &str)]) -> (Vec<u8>, ext_pkg::ContentDigest) {
    let mut entries = vec![ext_pkg::Entry { path: ext_manifest::FILE_NAME.into(), data: manifest.as_bytes().to_vec(), executable: false }];
    entries.extend(files.iter().map(|(p, t)| ext_pkg::Entry { path: (*p).into(), data: t.as_bytes().to_vec(), executable: false }));
    ext_pkg::write(entries).unwrap()
}

impl Test {
    fn new(name: &str) -> Test {
        let dir = std::env::temp_dir().join(format!("yard-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let authority = Authority::new();
        let cfg = init(&dir, "yard.test", &authority.fulcio_pem(), &authority.rekor_pem(), None, NOW).unwrap();
        let yard = Yard::open(Config::load(&cfg).unwrap()).unwrap();
        Test { dir, authority, yard }
    }

    /// Sign and upload a package as `who`.
    fn publish(&self, (bytes, digest): &(Vec<u8>, ext_pkg::ContentDigest), who: &Publisher) -> Result<Published, Error> {
        let m = ext_pkg::read(bytes).unwrap().manifest;
        let bundle = self.authority.sign_package(&m.id, &m.version, digest, who, NOW);
        self.yard.publish(&y::frame(&bundle, bytes), NOW + 60)
    }

    fn status(&self, id: &str, version: &str, change: StatusChange, who: &Publisher, signed_at: u64) -> Result<(ExtId, Version, Status), Error> {
        let statement = change.statement(&ExtId::parse(id).unwrap(), &Version::parse(version).unwrap());
        let bundle = self.authority.sign(&statement, who, signed_at);
        self.yard.change_status(&y::frame(&bundle, &statement), NOW + 60)
    }

    /// The public folder as a client sees it, verified with the registry's root key.
    fn index(&self) -> y::Verified {
        let read = |n: &str| std::fs::read(self.dir.join("public").join(n)).unwrap();
        let key = y::PublicKey::parse(std::fs::read_to_string(self.dir.join("root-key.txt")).unwrap().trim()).unwrap();
        let files = [read("root.json"), read("root.json.sig"), read("index.json"), read("index.json.sig")];
        y::verify(&RootKeys { keys: vec![key], threshold: 1 }, [&files[0], &files[1], &files[2], &files[3]], NOW + 60, Seen::default()).unwrap()
    }

    fn release(&self, id: &str, version: &str) -> y::Release {
        self.index().index.release(&ExtId::parse(id).unwrap(), &Version::parse(version).unwrap()).cloned().unwrap()
    }
}

impl Drop for Test {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn a_published_package_is_in_the_signed_index_and_on_disk() {
    let t = Test::new("publish");
    let me = github(7, 42);
    let pkg = package("you.night", "1.0.0", "Night");
    let p = t.publish(&pkg, &me).unwrap();
    assert_eq!((p.id.as_str(), p.version.to_string().as_str(), p.publisher.clone()), ("you.night", "1.0.0", me.clone()));
    assert_eq!(p.available, NOW + 60, "data-only versions are available at once");
    let v = t.index();
    let entry = &v.index.extensions[&ExtId::parse("you.night").unwrap()];
    assert_eq!((entry.name.as_str(), entry.description.as_str(), entry.categories.clone()), ("Night", "A test.", vec!["themes".to_string()]));
    assert_eq!(entry.publisher, me);
    let r = &entry.versions[0];
    assert_eq!(r.digest, pkg.1.hex());
    assert_eq!(r.status, Status::Ok);
    assert!(!r.code);
    let on_disk = std::fs::read(t.dir.join("public/packages").join(format!("{}.cyx", r.digest))).unwrap();
    assert_eq!(on_disk, pkg.0);
    assert_eq!(r.sha256, hex(&Sha256::digest(&on_disk)));
    assert_eq!(r.size, on_disk.len() as u64);
    assert_eq!(r.permissions, y::permission_digest(&ext_pkg::read(&on_disk).unwrap().manifest));
    assert!(v.index.expires <= NOW + 60 + y::INDEX_LIFETIME_SECS && !v.stale);
}

#[test]
fn the_market_page_is_the_manifest_and_readme_with_its_digest_in_the_index() {
    let t = Test::new("details");
    let me = github(7, 42);
    let long = "A line of the README.\n".repeat(y::MAX_README_BYTES / 10);
    let m = manifest("you.night", "1.0.0", "Night", "[[themes]]\nlabel = \"Night\"\npath = \"t.json\"\n");
    let pkg = package_with(&m, &[("t.json", THEME), ("README.md", &long)]);
    t.publish(&pkg, &me).unwrap();
    let r = t.release("you.night", "1.0.0");
    let bytes = std::fs::read(t.dir.join("public/packages").join(format!("{}.details.json", r.digest))).unwrap();
    assert_eq!(r.details.as_deref(), Some(y::Details::digest(&bytes).as_str()));
    let d: y::Details = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(d.manifest, m);
    let readme = d.readme.unwrap();
    assert!(d.readme_cut && readme.len() <= y::MAX_README_BYTES && readme.ends_with('\n') && long.starts_with(&readme));
    assert!(bytes.len() <= y::MAX_DETAILS_BYTES);
    // Without a README there is none, and nothing is cut.
    t.publish(&package("you.day", "1.0.0", "Day"), &me).unwrap();
    let r = t.release("you.day", "1.0.0");
    let d: y::Details = serde_json::from_slice(&std::fs::read(t.dir.join("public/packages").join(format!("{}.details.json", r.digest))).unwrap()).unwrap();
    assert_eq!((d.readme, d.readme_cut), (None, false));
}

#[test]
fn versions_published_before_details_get_them_on_the_next_start() {
    let t = Test::new("details-later");
    let pkg = package("you.night", "1.0.0", "Night");
    t.publish(&pkg, &github(7, 42)).unwrap();
    let digest = pkg.1.hex();
    let details = t.dir.join("public/packages").join(format!("{digest}.details.json"));
    // As a registry from before details files: no file, no digest.
    std::fs::remove_file(&details).unwrap();
    {
        let mut conn = t.yard.lock();
        let tx = conn.transaction().unwrap();
        assert!(db::releases_without_details(&tx).unwrap().is_empty());
        let mut release: y::Release = serde_json::from_str(&tx.query_row("SELECT release FROM versions", [], |r| r.get::<_, String>(0)).unwrap()).unwrap();
        release.details = None;
        db::put_release(&tx, &ExtId::parse("you.night").unwrap(), &release).unwrap();
        assert_eq!(db::releases_without_details(&tx).unwrap().len(), 1);
        tx.commit().unwrap();
    }
    let cfg = Config::load(&t.dir.join("yard.json")).unwrap();
    let again = Yard::open(cfg).unwrap();
    again.resign(NOW + 120).unwrap();
    let r = t.release("you.night", "1.0.0");
    assert_eq!(r.details.as_deref(), Some(y::Details::digest(&std::fs::read(&details).unwrap()).as_str()));
}

#[test]
fn versions_are_never_reused_and_come_out_in_order() {
    let t = Test::new("versions");
    let me = github(7, 42);
    t.publish(&package("you.night", "1.1.0", "Night"), &me).unwrap();
    t.publish(&package("you.night", "1.0.0", "Night"), &me).unwrap();
    let again = t.publish(&package("you.night", "1.1.0", "Night v2"), &me).unwrap_err();
    assert!(matches!(&again, Error::Conflict(m) if m.contains("never reused")), "{again:?}");
    let versions: Vec<String> = t.index().index.extensions.values().next().unwrap().versions.iter().map(|r| r.version.to_string()).collect();
    assert_eq!(versions, ["1.0.0", "1.1.0"]);
}

#[test]
fn a_publisher_name_belongs_to_its_first_owner() {
    let t = Test::new("owners");
    t.publish(&package("you.night", "1.0.0", "Night"), &github(7, 42)).unwrap();
    // Another repository of the same owner may add an extension under the name.
    t.publish(&package("you.day", "1.0.0", "Day"), &github(7, 43)).unwrap();
    // But not take over one published from elsewhere.
    let e = t.publish(&package("you.night", "2.0.0", "Night"), &github(7, 43)).unwrap_err();
    assert!(matches!(&e, Error::Forbidden(m) if m.contains("Publish from the same repository")), "{e:?}");
    // And another owner cannot publish under it at all.
    let e = t.publish(&package("you.dusk", "1.0.0", "Dusk"), &github(8, 1)).unwrap_err();
    assert!(matches!(&e, Error::Forbidden(m) if m.contains("belongs to another publisher")), "{e:?}");
}

#[test]
fn confusable_names_are_refused() {
    let t = Test::new("confusable");
    t.publish(&package("microsoft.python", "1.0.0", "Python"), &github(1, 1)).unwrap();
    for (id, name) in [("rnicrosoft.python", "Snake"), ("microsft.tools", "Tools"), ("other.snake", "Pyth0n")] {
        let e = t.publish(&package(id, "1.0.0", name), &github(2, 2)).unwrap_err();
        assert!(matches!(&e, Error::Conflict(m) if m.contains("too close")), "{id}: {e:?}");
    }
    // The owner may use similar names of their own.
    t.publish(&package("microsoft.python-tools", "1.0.0", "Python tools"), &github(1, 3)).unwrap();
}

#[test]
fn what_is_signed_must_be_what_is_uploaded() {
    let t = Test::new("signature");
    let me = github(7, 42);
    let (bytes, _) = package("you.night", "1.0.0", "Night");
    let (_, other_digest) = package("you.night", "1.0.0", "Other");
    let bundle = t.authority.sign_package(&ExtId::parse("you.night").unwrap(), &Version::parse("1.0.0").unwrap(), &other_digest, &me, NOW);
    let e = t.yard.publish(&y::frame(&bundle, &bytes), NOW).unwrap_err();
    assert!(matches!(&e, Error::Forbidden(m) if m.contains("CY0814")), "{e:?}");
    // A bundle from another Sigstore than the one pinned.
    let stranger = Authority::new();
    let (bytes, digest) = package("you.night", "1.0.0", "Night");
    let bundle = stranger.sign_package(&ExtId::parse("you.night").unwrap(), &Version::parse("1.0.0").unwrap(), &digest, &me, NOW);
    assert!(matches!(t.yard.publish(&y::frame(&bundle, &bytes), NOW), Err(Error::Forbidden(_))));
    // Garbage.
    assert!(matches!(t.yard.publish(b"xx", NOW), Err(Error::Invalid(_))));
    assert!(matches!(t.yard.publish(&y::frame(b"{}", b"not a package"), NOW), Err(Error::Invalid(_))));
    assert!(!t.dir.join("public/index.json").exists(), "nothing was signed");
}

#[test]
fn code_waits_for_phase_2() {
    let t = Test::new("code");
    let manifest = manifest("you.code", "1.0.0", "Code", "[code]\nentry = \"main.ts\"\n[[commands]]\nid = \"go\"\ntitle = \"Go\"\n");
    let pkg = ext_pkg::write(vec![
        ext_pkg::Entry { path: ext_manifest::FILE_NAME.into(), data: manifest.into_bytes(), executable: false },
        ext_pkg::Entry { path: "main.ts".into(), data: vec![], executable: false },
        ext_pkg::Entry { path: ext_pkg::COMPONENT.into(), data: ext_pkg::testing::COMPONENT.to_vec(), executable: false },
    ])
    .unwrap();
    let e = t.publish(&pkg, &github(7, 42)).unwrap_err();
    assert!(matches!(&e, Error::Invalid(m) if m.contains("data-only")), "{e:?}");
}

#[test]
fn publishers_yank_restore_and_deprecate_with_signed_statements() {
    let t = Test::new("status");
    let me = github(7, 42);
    t.publish(&package("you.night", "1.0.0", "Night"), &me).unwrap();
    t.publish(&package("you.day", "1.0.0", "Day"), &me).unwrap();
    t.status("you.night", "1.0.0", StatusChange::Yank, &me, NOW).unwrap();
    assert_eq!(t.release("you.night", "1.0.0").status, Status::Yanked);
    t.status("you.night", "1.0.0", StatusChange::Restore, &me, NOW).unwrap();
    assert_eq!(t.release("you.night", "1.0.0").status, Status::Ok);
    let replacement = Some(ExtId::parse("you.day").unwrap());
    t.status("you.night", "1.0.0", StatusChange::Deprecate { replacement: replacement.clone() }, &me, NOW).unwrap();
    assert_eq!(t.release("you.night", "1.0.0").status, Status::Deprecated { replacement });

    let gone = StatusChange::Deprecate { replacement: Some(ExtId::parse("you.gone").unwrap()) };
    assert!(matches!(t.status("you.night", "1.0.0", gone, &me, NOW), Err(Error::Invalid(_))));
    assert!(matches!(t.status("you.night", "1.0.0", StatusChange::Yank, &github(7, 43), NOW), Err(Error::Forbidden(_))), "another repository");
    let old = t.status("you.night", "1.0.0", StatusChange::Yank, &me, NOW - STATEMENT_MAX_AGE_SECS);
    assert!(matches!(&old, Err(Error::Forbidden(m)) if m.contains("15 minutes")), "{old:?}");
    assert!(matches!(t.status("you.night", "9.0.0", StatusChange::Yank, &me, NOW), Err(Error::NotFound(_))));
    assert!(matches!(t.status("you.none", "1.0.0", StatusChange::Yank, &me, NOW), Err(Error::NotFound(_))));
    // A status statement is not a package signature and the other way round.
    let (bytes, digest) = package("you.night", "2.0.0", "Night");
    let pkg_bundle = t.authority.sign_package(&ExtId::parse("you.night").unwrap(), &Version::parse("2.0.0").unwrap(), &digest, &me, NOW);
    assert!(t.yard.change_status(&y::frame(&pkg_bundle, &bytes), NOW).is_err());
}

#[test]
fn the_security_role_revokes_and_clears_advisory_fixes() {
    let t = Test::new("operator");
    let me = github(7, 42);
    t.publish(&package("you.night", "1.0.0", "Night"), &me).unwrap();
    t.publish(&package("you.night", "1.0.1", "Night"), &me).unwrap();
    let (id, v1) = (ExtId::parse("you.night").unwrap(), Version::parse("1.0.0").unwrap());
    assert!(matches!(t.yard.revoke(&id, &v1, "", NOW), Err(Error::Invalid(_))));
    t.yard.revoke(&id, &v1, "Sent files to an unknown host.", NOW).unwrap();
    assert_eq!(t.release("you.night", "1.0.0").status, Status::Revoked { reason: "Sent files to an unknown host.".into() });
    // The publisher cannot undo it.
    assert!(matches!(t.status("you.night", "1.0.0", StatusChange::Restore, &me, NOW), Err(Error::Forbidden(_))));
    t.yard.clear_for_immediate_update(&id, &Version::parse("1.0.1").unwrap(), NOW).unwrap();
    assert!(t.release("you.night", "1.0.1").immediate);
    assert!(matches!(t.yard.revoke(&id, &Version::parse("3.0.0").unwrap(), "x", NOW), Err(Error::NotFound(_))));
}

#[test]
fn every_signing_raises_the_index_version_and_counts_downloads() {
    let t = Test::new("counter");
    let pkg = package("you.night", "1.0.0", "Night");
    t.publish(&pkg, &github(7, 42)).unwrap();
    let first = t.index().index.version;
    let digest = pkg.1.hex();
    for _ in 0..3 {
        assert!(t.yard.download(&digest, NOW).unwrap().is_file());
    }
    t.yard.download(&digest, NOW + 86400).unwrap();
    assert!(matches!(t.yard.download(&"0".repeat(64), NOW), Err(Error::NotFound(_))));
    assert!(matches!(t.yard.download("../../etc/passwd", NOW), Err(Error::NotFound(_))));
    assert_eq!(t.yard.resign(NOW + 60).unwrap(), first + 1);
    let v = t.index();
    assert_eq!(v.index.version, first + 1);
    assert_eq!(v.index.extensions.values().next().unwrap().versions[0].downloads, 4);
}

#[test]
fn the_index_never_outlives_the_online_key() {
    let t = Test::new("expiry");
    let root: Root = serde_json::from_slice(&std::fs::read(t.dir.join("root.json")).unwrap()).unwrap();
    t.yard.resign(root.online.expires - 3600).unwrap();
    let index: Index = serde_json::from_slice(&std::fs::read(t.dir.join("public/index.json")).unwrap()).unwrap();
    assert_eq!(index.expires, root.online.expires);
}

#[test]
fn opening_checks_the_online_key_and_init_refuses_twice() {
    let t = Test::new("open");
    assert!(init(&t.dir, "yard.test", &t.authority.fulcio_pem(), &t.authority.rekor_pem(), None, NOW).unwrap_err().contains("already has a registry"));
    let mut cfg = t.yard.config().clone();
    cfg.online_key = t.dir.join("keys/root.key");
    assert!(Yard::open(cfg).err().unwrap().contains("is not the online key"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(t.dir.join("keys/online.key")).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert!(init(&t.dir.join("x"), "yard.test", "nope", "nope", None, NOW).is_err());
}

/// A raw request; the response's head (status line and headers), lower-cased header names.
fn raw(addr: std::net::SocketAddr, request: &str) -> String {
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    s.write_all(request.as_bytes()).unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    let end = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or(out.len());
    let head = String::from_utf8_lossy(&out[..end]).into_owned();
    head.lines().map(|l| l.split_once(": ").map_or(l.to_string(), |(k, v)| format!("{}: {v}", k.to_ascii_lowercase()))).collect::<Vec<_>>().join("\n")
}

/// The production path: the root keys are made off the server, the root waits for a threshold of their signatures,
/// and every 90 days the next online key goes live only once the holders signed the root that names it.
#[test]
fn the_root_key_ceremony_and_an_online_key_rotation() {
    let dir = std::env::temp_dir().join(format!("yard-ceremony-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let authority = Authority::new();
    let holders: Vec<SigningKey> = (0..3).map(|_| SigningKey::generate().unwrap().0).collect();
    let trust = RootKeys { keys: holders.iter().map(SigningKey::public).collect(), threshold: 2 };
    let cfg = init(&dir, "yard.test", &authority.fulcio_pem(), &authority.rekor_pem(), Some(trust.clone()), NOW).unwrap();
    assert!(!dir.join("keys/root.key").exists() && !dir.join("root-key.txt").exists(), "no root key on the server");
    assert_eq!(Config::load(&cfg).unwrap().trust(), Some(trust.clone()));
    let sign = |file: &std::path::Path, who: &[usize]| {
        let bytes = std::fs::read(file).unwrap();
        let mut sig = std::fs::read_to_string(sig_path(file)).unwrap();
        for &i in who {
            sig = y::add_signature(&sig, &holders[i], y::Role::Root, &bytes).unwrap();
        }
        std::fs::write(sig_path(file), sig).unwrap();
    };
    let client = |at: u64| {
        let read = |n: &str| std::fs::read(dir.join("public").join(n)).unwrap();
        let files = [read("root.json"), read("root.json.sig"), read("index.json"), read("index.json.sig")];
        y::verify(&trust, [&files[0], &files[1], &files[2], &files[3]], at, Seen::default())
    };
    // Unsigned, clients refuse it; with two of the three holders, they accept it.
    Yard::open(Config::load(&cfg).unwrap()).unwrap().resign(NOW).unwrap();
    assert!(matches!(client(NOW).unwrap_err(), y::Error::Signature { valid: 0, needed: 2, .. }));
    sign(&dir.join("root.json"), &[0, 2]);
    Yard::open(Config::load(&cfg).unwrap()).unwrap().resign(NOW).unwrap();
    assert_eq!(client(NOW).unwrap().root.version, 1);

    // 80 days later: rotate. One signature is not enough to promote; two are.
    let later = NOW + 80 * 86400;
    let next = rotate(&cfg, later).unwrap();
    assert!(rotate(&cfg, later).unwrap_err().contains("rotation is under way"));
    sign(&next, &[1]);
    assert!(promote(&cfg, later).unwrap_err().contains("1 valid signature"), "one holder alone cannot change the root");
    assert_eq!(client(later).unwrap().root.version, 1, "nothing changed");
    sign(&next, &[2]);
    assert_eq!(promote(&cfg, later).unwrap(), 2);
    // Past the first key's 90 days the service still signs, and clients accept it.
    let past = NOW + 95 * 86400;
    Yard::open(Config::load(&cfg).unwrap()).unwrap().resign(past).unwrap();
    let v = client(past).unwrap();
    assert_eq!(v.root.version, 2);
    assert!(!v.stale);
    assert!(!next.exists() && !dir.join("keys/online.next.key").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn promote_needs_the_root_keys_in_the_config() {
    let t = Test::new("promote-dev");
    rotate(&t.dir.join("yard.json"), NOW).unwrap();
    assert!(promote(&t.dir.join("yard.json"), NOW).unwrap_err().contains("no root-keys"));
}

/// One HTTP/1.1 request on a fresh connection: (status, body).
fn request(addr: std::net::SocketAddr, method: &str, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    write!(s, "{method} {path} HTTP/1.1\r\nHost: yard\r\nConnection: close\r\nContent-Length: {}\r\n\r\n", body.len()).unwrap();
    s.write_all(body).unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    let split = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&out[..split]).into_owned();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, out[split + 4..].to_vec())
}

#[test]
fn the_http_service_end_to_end() {
    let t = Test::new("http");
    let dir = t.dir.clone();
    let yard = std::sync::Arc::new(Yard::open(Config::load(&dir.join("yard.json")).unwrap()).unwrap());
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || rt.block_on(http::serve(yard, listener)));

    assert_eq!(request(addr, "GET", "/healthz", b""), (200, b"ok\n".to_vec()));
    let me = github(7, 42);
    let (bytes, digest) = package("you.night", "1.0.0", "Night");
    let bundle = t.authority.sign_package(&ExtId::parse("you.night").unwrap(), &Version::parse("1.0.0").unwrap(), &digest, &me, http::now());
    let (status, body) = request(addr, "POST", "/v1/publish", &y::frame(&bundle, &bytes));
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let p: Published = serde_json::from_slice(&body).unwrap();
    assert_eq!(p.digest, digest.hex());
    let (status, body) = request(addr, "POST", "/v1/publish", &y::frame(&bundle, &bytes));
    assert_eq!(status, 409, "{}", String::from_utf8_lossy(&body));

    let (status, index) = request(addr, "GET", "/index.json", b"");
    assert_eq!(status, 200);
    assert_eq!(index, std::fs::read(dir.join("public/index.json")).unwrap());
    assert_eq!(request(addr, "GET", "/root.json.sig", b"").0, 200);
    // The daily conditional request: unchanged, nothing is sent; no cache may answer without asking.
    let head = raw(addr, "GET /index.json HTTP/1.1\r\nHost: yard\r\nConnection: close\r\n\r\n");
    assert!(head.contains("cache-control: no-cache"), "{head}");
    let tag = head.lines().find_map(|l| l.strip_prefix("etag: ")).unwrap().trim().to_string();
    let again = raw(addr, &format!("GET /index.json HTTP/1.1\r\nHost: yard\r\nIf-None-Match: {tag}\r\nConnection: close\r\n\r\n"));
    assert!(again.starts_with("HTTP/1.1 304"), "{again}");
    let pkg_head = raw(addr, &format!("GET /packages/{}.cyx HTTP/1.1\r\nHost: yard\r\nConnection: close\r\n\r\n", digest.hex()));
    assert!(pkg_head.contains("cache-control: private"), "a CDN must not keep packages, or downloads go uncounted: {pkg_head}");
    let (status, pkg) = request(addr, "GET", &format!("/packages/{}.cyx", digest.hex()), b"");
    assert_eq!((status, pkg), (200, bytes));
    let (status, served) = request(addr, "GET", &format!("/packages/{}.sigstore.json", digest.hex()), b"");
    assert_eq!((status, served), (200, bundle.clone()), "the publisher signature, as uploaded");
    let (status, page) = request(addr, "GET", &format!("/packages/{}.details.json", digest.hex()), b"");
    assert_eq!((status, page), (200, std::fs::read(dir.join(format!("public/packages/{}.details.json", digest.hex()))).unwrap()));
    assert_eq!(request(addr, "GET", "/packages/zz.details.json", b"").0, 404);
    assert_eq!(request(addr, "GET", "/packages/../yard.sigstore.json", b"").0, 404);
    assert_eq!(request(addr, "GET", "/packages/zz.cyx", b"").0, 404);
    assert_eq!(request(addr, "GET", "/yard.sqlite", b"").0, 404, "only the public files are served");
    assert_eq!(request(addr, "DELETE", "/index.json", b"").0, 405);

    let statement = StatusChange::Yank.statement(&p.id, &p.version);
    let (status, body) = request(addr, "POST", "/v1/status", &y::frame(&t.authority.sign(&statement, &me, http::now()), &statement));
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert!(String::from_utf8_lossy(&body).contains("yanked"));

    // An announced length over the limit is refused before the body is read.
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    write!(s, "POST /v1/publish HTTP/1.1\r\nHost: yard\r\nConnection: close\r\nContent-Length: {}\r\n\r\n", MAX_UPLOAD_BYTES + 1).unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    assert!(out.starts_with("HTTP/1.1 413"), "{out}");
}

#[test]
fn the_production_pins_load() {
    Pins::from_pem(SIGSTORE_FULCIO, SIGSTORE_REKOR).unwrap();
}

const LINUX: &str = "x86_64-unknown-linux-musl";
const MAC: &str = "aarch64-apple-darwin";

impl Test {
    /// A registry whose policy invites the publisher names `invited` to publish code and native programs.
    fn inviting(name: &str, invited: &[&str]) -> Test {
        let mut t = Test::new(name);
        let path = t.dir.join("yard.json");
        let mut cfg: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        cfg["invited"] = serde_json::json!(invited);
        std::fs::write(&path, serde_json::to_vec_pretty(&cfg).unwrap()).unwrap();
        t.yard = Yard::open(Config::load(&path).unwrap()).unwrap();
        t
    }

    fn native(&self, version: &str, target: &str) -> Result<Published, Error> {
        let bytes = ext_core::testing::native_package("you.hello", version, target);
        let digest = ext_pkg::read(&bytes).unwrap().digest;
        self.publish(&(bytes, digest), &github(7, 42))
    }

    fn listed(&self, version: &str) -> Option<y::Release> {
        self.index().index.release(&ExtId::parse("you.hello").unwrap(), &Version::parse(version).unwrap()).cloned()
    }
}

/// #147: a component built against API 0.1 still runs during the move to 1.0, but the registry refuses new uploads of one.
#[test]
fn a_0_1_component_is_refused() {
    let t = Test::inviting("transition", &["you"]);
    let manifest = manifest("you.code", "1.0.0", "Code", "[code]\nentry = \"main.ts\"\n[[commands]]\nid = \"go\"\ntitle = \"Go\"\n");
    let entries = |component: &[u8]| {
        vec![
            ext_pkg::Entry { path: ext_manifest::FILE_NAME.into(), data: manifest.clone().into_bytes(), executable: false },
            ext_pkg::Entry { path: "main.ts".into(), data: vec![], executable: false },
            ext_pkg::Entry { path: ext_pkg::COMPONENT.into(), data: component.to_vec(), executable: false },
        ]
    };
    let e = t.publish(&ext_pkg::write(entries(ext_pkg::testing::COMPONENT_0_1)).unwrap(), &github(7, 42)).unwrap_err();
    assert!(matches!(&e, Error::Invalid(m) if m.contains("built against API 0.1.0") && m.contains("migrating-to-1.0.md")), "{e:?}");
    t.publish(&ext_pkg::write(entries(ext_pkg::testing::COMPONENT)).unwrap(), &github(7, 42)).unwrap();
}

/// #146: without an invitation the registry keeps Phase 1's policy: no code, and no native programs either.
#[test]
fn native_programs_need_an_invitation() {
    let t = Test::new("native-default");
    let e = t.native("1.0.0", LINUX).unwrap_err();
    assert!(matches!(&e, Error::Forbidden(m) if m.contains("invited publishers")), "{e:?}");
    // Another publisher name is invited: still refused.
    let t = Test::inviting("native-other", &["someone"]);
    assert!(matches!(t.native("1.0.0", LINUX), Err(Error::Forbidden(_))));
    // Data-only publishing is open to everyone as before.
    assert!(t.publish(&package("you.theme", "1.0.0", "Theme"), &github(7, 42)).is_ok());
}

/// AC3: a publisher's first native program stays unlisted, and out of the public folder, until a reviewer approves it.
#[test]
fn a_first_native_program_waits_for_review() {
    let t = Test::inviting("native-review", &["you"]);
    let p = t.native("1.0.0", LINUX).unwrap();
    assert!(p.review, "the first native upload is held");
    assert!(p.available >= NOW + CODE_WAIT_SECS, "native programs are code: updates wait");
    assert!(t.native("1.0.0", MAC).unwrap().review, "its other platform waits with it");
    assert!(t.listed("1.0.0").is_none());
    assert!(!t.index().index.extensions.contains_key(&ExtId::parse("you.hello").unwrap()), "an extension with nothing listed is not in the index");
    assert!(!t.dir.join(format!("public/packages/{}.cyx", p.digest)).exists());
    assert!(t.yard.download(&p.digest, NOW).is_err(), "a waiting package is not served");
    // The reviewer's queue shows it with both platforms and what the install sheet will say.
    let queue = t.yard.review_queue().unwrap();
    assert_eq!(queue.len(), 1);
    assert_eq!((queue[0].id.as_str(), queue[0].version.to_string()), ("you.hello", "1.0.0".to_string()));
    assert_eq!(queue[0].packages.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(), [MAC, LINUX]);
    assert!(queue[0].packages.iter().all(|(_, path)| path.is_file()));
    assert!(queue[0].permissions.iter().any(|l| l.contains("./bin/hello")), "{:?}", queue[0].permissions);
    // Approved: listed with both platforms, served, and the queue is empty.
    t.yard.approve(&ExtId::parse("you.hello").unwrap(), &Version::parse("1.0.0").unwrap(), NOW + 60).unwrap();
    let r = t.listed("1.0.0").unwrap();
    assert_eq!(r.targets.keys().map(String::as_str).collect::<Vec<_>>(), [MAC, LINUX]);
    assert!(t.dir.join(format!("public/packages/{}.cyx", r.targets[MAC].digest)).is_file());
    assert!(t.yard.download(&r.targets[MAC].digest, NOW).is_ok(), "each platform's package counts its downloads");
    assert!(t.yard.review_queue().unwrap().is_empty());
    assert!(t.yard.approve(&ExtId::parse("you.hello").unwrap(), &Version::parse("1.0.0").unwrap(), NOW).is_err());
    // The publisher's later native programs are listed without review.
    let next = t.native("1.1.0", LINUX).unwrap();
    assert!(!next.review);
    assert!(t.listed("1.1.0").is_some());
    // A platform once published is never replaced, and a version without native programs takes none.
    assert!(matches!(t.native("1.1.0", LINUX), Err(Error::Conflict(_))));
}

#[test]
fn a_rejected_native_program_is_never_listed() {
    let t = Test::inviting("native-reject", &["you"]);
    let p = t.native("1.0.0", LINUX).unwrap();
    let (id, v) = (ExtId::parse("you.hello").unwrap(), Version::parse("1.0.0").unwrap());
    assert!(t.yard.reject(&id, &v, "").is_err(), "a reason is required");
    t.yard.reject(&id, &v, "The program downloads code at run time.").unwrap();
    assert!(t.yard.review_queue().unwrap().is_empty());
    assert!(t.listed("1.0.0").is_none());
    assert!(!t.dir.join(format!("review/{}.cyx", p.digest)).exists(), "its files are deleted");
    // Its version number stays used, for every platform; the publisher is still not trusted for native programs.
    assert!(matches!(t.native("1.0.0", MAC), Err(Error::Forbidden(_))));
    assert!(t.native("1.0.1", LINUX).unwrap().review);
}
