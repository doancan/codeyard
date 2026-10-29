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
        let cfg = init(&dir, "yard.test", &authority.fulcio_pem(), &authority.rekor_pem(), NOW).unwrap();
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
    let pkg =
        package_with(&manifest("you.code", "1.0.0", "Code", "[code]\nentry = \"main.ts\"\n[[commands]]\nid = \"go\"\ntitle = \"Go\"\n"), &[("main.ts", "")]);
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
    assert!(init(&t.dir, "yard.test", &t.authority.fulcio_pem(), &t.authority.rekor_pem(), NOW).unwrap_err().contains("already has a registry"));
    let mut cfg = t.yard.config().clone();
    cfg.online_key = t.dir.join("keys/root.key");
    assert!(Yard::open(cfg).err().unwrap().contains("is not the online key"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(t.dir.join("keys/online.key")).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert!(init(&t.dir.join("x"), "yard.test", "nope", "nope", NOW).is_err());
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
