//! `cyx publish` end to end, without the network: a stand-in Fulcio and Rekor answering in their real formats (backed
//! by the test Sigstore authority), and a real registry (`yard`) on a local port.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD as B64URL};
use ext_core::testing::{Authority, github};
use ext_core::yard::{Publisher, RootKeys, Seen, Status, StatusChange};
use serde_json::{Value, json};

use crate::{Kind, now, publish, scaffold, sigstore};

/// The publisher each test token stands for, inside the token's claims.
fn token(sub: &str, p: &Publisher) -> String {
    let claims = json!({ "sub": sub, "test_publisher": p });
    format!("h.{}.s", B64URL.encode(claims.to_string()))
}

/// Answer one request at a time: `POST /api/v2/signingCert` like Fulcio, `POST /api/v1/log/entries` like Rekor.
fn fake_sigstore(authority: Arc<Authority>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let path = line.split(' ').nth(1).unwrap_or_default().to_string();
            let mut len = 0;
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                if h.trim().is_empty() {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; len];
            reader.read_exact(&mut body).unwrap();
            let (status, answer) = match path.as_str() {
                "/api/v2/signingCert" => fulcio(&authority, &body),
                "/api/v1/log/entries" => (201, authority.rekor_response(&body, now()).to_string()),
                _ => (404, "not found".into()),
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                answer.len()
            );
        }
    });
    url
}

/// Fulcio's checks, in short: a token naming a publisher, a P-256 key, and proof that the key signed the subject.
fn fulcio(a: &Authority, body: &[u8]) -> (u16, String) {
    let req: Value = serde_json::from_slice(body).unwrap();
    let token = req["credentials"]["oidcIdentityToken"].as_str().unwrap();
    let claims: Value = serde_json::from_slice(&B64URL.decode(token.split('.').nth(1).unwrap()).unwrap()).unwrap();
    let publisher: Publisher = serde_json::from_value(claims["test_publisher"].clone()).unwrap();
    let pem = req["publicKeyRequest"]["publicKey"]["content"].as_str().unwrap();
    let der = B64.decode(pem.lines().filter(|l| !l.starts_with("-----")).collect::<String>()).unwrap();
    let point = &der[der.len() - 65..];
    let proof = B64.decode(req["publicKeyRequest"]["proofOfPossession"].as_str().unwrap()).unwrap();
    let ok =
        ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_ASN1, point).verify(claims["sub"].as_str().unwrap().as_bytes(), &proof);
    if ok.is_err() {
        return (400, "proof of possession does not verify".into());
    }
    let cert = a.certificate_for(point, &publisher, now());
    (201, json!({ "signedCertificateEmbeddedSct": { "chain": { "certificates": [ext_core::testing::pem("CERTIFICATE", &cert)] } } }).to_string())
}

struct World {
    dir: PathBuf,
    registry: String,
    endpoints: sigstore::Endpoints,
}

impl World {
    fn new(name: &str) -> World {
        let dir = std::env::temp_dir().join(format!("cyx-publish-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let authority = Arc::new(Authority::new());
        let cfg = ::yard::init(&dir.join("registry"), "yard.test", &authority.fulcio_pem(), &authority.rekor_pem(), None, now()).unwrap();
        let y = Arc::new(::yard::Yard::open(::yard::Config::load(&cfg).unwrap()).unwrap());
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
        let registry = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        std::thread::spawn(move || rt.block_on(::yard::http::serve(y, listener)));
        let sig = fake_sigstore(authority);
        // Wait for the registry's first signing.
        for _ in 0..100 {
            if dir.join("registry/public/index.json").exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        World { endpoints: sigstore::Endpoints { fulcio: sig.clone(), rekor: sig, oidc: "http://127.0.0.1:1".into() }, dir, registry }
    }

    fn extension(&self, name: &str, version: &str) -> PathBuf {
        let d = self.dir.join(name);
        let _ = std::fs::remove_dir_all(&d);
        scaffold::create(&d, Kind::Theme, name, "you").unwrap();
        let m = d.join(ext_manifest::FILE_NAME);
        let text = std::fs::read_to_string(&m).unwrap().replace("\"0.1.0\"", &format!("\"{version}\""));
        std::fs::write(&m, text).unwrap();
        d
    }

    fn publish(&self, dir: &std::path::Path, who: &Publisher, dry_run: bool, pre_release: bool) -> Result<String, String> {
        let p = publish::Publish { dir, registry: &self.registry, dry_run, pre_release };
        publish::publish(&p, &self.endpoints, || Ok(token("repo:you/night:ref:refs/tags/v1", who)), now())
    }

    /// The registry's index as a client verifies it.
    fn index(&self) -> ext_core::yard::Index {
        let r = self.dir.join("registry");
        let key = ext_core::yard::PublicKey::parse(std::fs::read_to_string(r.join("root-key.txt")).unwrap().trim()).unwrap();
        let f = |n: &str| std::fs::read(r.join("public").join(n)).unwrap();
        let files = [f("root.json"), f("root.json.sig"), f("index.json"), f("index.json.sig")];
        ext_core::yard::verify(&RootKeys { keys: vec![key], threshold: 1 }, [&files[0], &files[1], &files[2], &files[3]], now(), Seen::default()).unwrap().index
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn publish_signs_keyless_and_the_registry_lists_it() {
    let w = World::new("publish");
    let me = github(7, 42);
    let dir = w.extension("night", "1.0.0");
    let out = w.publish(&dir, &me, false, false).unwrap();
    assert!(out.contains("Published you.night 1.0.0 as repository 42 of owner 7, .github/workflows/publish.yml."), "{out}");
    assert!(out.ends_with("Users get it now."), "{out}");
    let index = w.index();
    let entry = &index.extensions[&ext_manifest::ExtId::parse("you.night").unwrap()];
    assert_eq!(entry.publisher, me);
    assert_eq!(entry.versions[0].version.to_string(), "1.0.0");
    // The same version again is refused by the registry, with its reason.
    let again = w.publish(&dir, &me, false, false).unwrap_err();
    assert!(again.contains("never reused"), "{again}");
}

#[test]
fn dry_run_shows_the_install_sheet_and_sends_nothing() {
    let w = World::new("dry");
    let dir = w.extension("tools", "1.0.0");
    let m = dir.join(ext_manifest::FILE_NAME);
    let text = std::fs::read_to_string(&m).unwrap() + "\n[permissions]\nrun = [\"bash\"]\nwhy = \"Runs the build script.\"\n";
    std::fs::write(&m, text).unwrap();
    let out = w.publish(&dir, &github(7, 42), true, false).unwrap();
    assert!(out.contains("Would publish you.tools 1.0.0"), "{out}");
    assert!(out.contains("• Run bash with any arguments"), "{out}");
    assert!(out.contains("High risk: this extension can run programs as you."), "{out}");
    assert!(out.contains("“Runs the build script.” (the author)"), "{out}");
    assert!(out.ends_with("Nothing was signed or sent."), "{out}");
    assert!(w.index().extensions.is_empty());
}

/// #187: a placeholder `client-id` would fail every sign-in, so publishing refuses it (`cyx check` only warns).
#[test]
fn a_placeholder_client_id_is_not_published() {
    let w = World::new("placeholder");
    let dir = w.extension("forge", "1.0.0");
    let m = dir.join(ext_manifest::FILE_NAME);
    let oauth = "\n[permissions]\nauth = [\"forge\"]\nwhy = \"Signs in to the forge.\"\n\n[[oauth]]\nid = \"forge\"\nname = \"Forge\"\nauthorize-url = \"https://forge.example.com/a\"\ntoken-url = \"https://forge.example.com/t\"\nclient-id = \"your-client-id\"\nhosts = [\"forge.example.com\"]\n";
    std::fs::write(&m, std::fs::read_to_string(&m).unwrap() + oauth).unwrap();
    let e = w.publish(&dir, &github(7, 42), true, false).unwrap_err();
    assert!(e.contains("the client id of [[oauth]] forge is a placeholder"), "{e}");
    assert!(w.index().extensions.is_empty());
}

#[test]
fn pre_releases_need_a_suffix() {
    let w = World::new("pre");
    let plain = w.extension("night", "1.0.0");
    assert!(w.publish(&plain, &github(7, 42), true, true).unwrap_err().contains("--pre-release needs a version with a suffix, like 1.0.0-beta.1"));
    let beta = w.extension("dawn", "1.1.0-beta.1");
    let out = w.publish(&beta, &github(7, 42), false, false).unwrap();
    assert!(out.contains("is a pre-release"), "{out}");
    assert!(w.publish(&w.extension("dusk", "2.0.0-rc.1"), &github(7, 42), false, true).is_ok());
}

#[test]
fn yank_restore_and_deprecate_from_the_folder() {
    let w = World::new("status");
    let me = github(7, 42);
    let dir = w.extension("night", "1.0.0");
    w.publish(&dir, &me, false, false).unwrap();
    let v = ext_manifest::Version::parse("1.0.0").unwrap();
    let change = |c: StatusChange, who: &Publisher| publish::change_status(&dir, &w.registry, &v, c, &w.endpoints, || Ok(token("s", who)), now());
    assert!(change(StatusChange::Yank, &me).unwrap().starts_with("Yanked you.night 1.0.0"));
    assert_eq!(w.index().extensions.values().next().unwrap().versions[0].status, Status::Yanked);
    change(StatusChange::Restore, &me).unwrap();
    assert_eq!(w.index().extensions.values().next().unwrap().versions[0].status, Status::Ok);
    let err = change(StatusChange::Yank, &github(7, 43)).unwrap_err();
    assert!(err.contains("the registry refused") && err.contains("is published from"), "{err}");
    change(StatusChange::Deprecate { replacement: None }, &me).unwrap();
}

#[test]
fn a_signature_the_registry_would_refuse_is_caught_before_upload() {
    let w = World::new("roots");
    // Point signing at another Sigstore than the one the registry pins.
    let other = fake_sigstore(Arc::new(Authority::new()));
    let ep = sigstore::Endpoints { fulcio: other.clone(), rekor: other, oidc: String::new() };
    let dir = w.extension("night", "1.0.0");
    let p = publish::Publish { dir: &dir, registry: &w.registry, dry_run: false, pre_release: false };
    let err = publish::publish(&p, &ep, || Ok(token("s", &github(7, 42))), now()).unwrap_err();
    assert!(err.contains("nothing was uploaded"), "{err}");
    assert!(w.index().extensions.is_empty());
    // A token problem stops before anything is signed.
    let err = publish::publish(&p, &w.endpoints, || Err("error: no token".into()), now()).unwrap_err();
    assert_eq!(err, "error: no token");
}

/// A code extension: the probe component the exthost tests use, with the permissions its imports need.
const PROBE: &[u8] = include_bytes!("../../ext-host/testdata/probe.wasm");
const PROBE_MANIFEST: &str = r#"[extension]
id = "you.probe"
version = "1.0.0"
name = "Probe"
license = "MIT"
description = "Tests the host."

[code]
entry = "Cargo.toml"

[permissions]
read = ["a.txt"]
net = ["api.example.com"]
auth = ["github"]
secrets = true
why = "Tests every part of the host."

[[commands]]
id = "notice"
title = "Probe: Notice"
"#;

#[test]
fn a_code_extension_is_flagged_and_a_data_only_registry_says_so() {
    let w = World::new("code");
    let dir = w.dir.join("probe");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(ext_manifest::FILE_NAME), PROBE_MANIFEST).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "").unwrap();
    std::fs::write(dir.join(ext_pkg::COMPONENT), PROBE).unwrap();
    let out = w.publish(&dir, &github(7, 42), true, false).unwrap();
    assert!(out.contains("note: you.probe runs code: the index marks it as code"), "{out}");
    // This registry (Phase 1) refuses code; cyx says what that means instead of passing the refusal on.
    let err = w.publish(&dir, &github(7, 42), false, false).unwrap_err();
    assert!(err.starts_with(&format!("error: {} accepts data-only extensions; you.probe 1.0.0 runs code", w.registry)), "{err}");
    assert!(err.contains("`cyx package`"), "{err}");
    assert!(w.index().extensions.is_empty());
}

#[test]
fn registry_urls() {
    assert_eq!(publish::registry_url(Some("http://localhost:8080/")), "http://localhost:8080");
    assert!(publish::registry_url(None).starts_with("https://"));
}
