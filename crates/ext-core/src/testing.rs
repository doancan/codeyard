//! A stand-in for Sigstore in tests (`feature = "testing"`): a CA playing Fulcio and a key playing Rekor, issuing
//! real bundles that `publisher::verify` accepts against the pins it hands out. Lets the registry, `cyx` and the app
//! test publishing and installing end to end without the network. Never compiled into a release.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair as _};
use serde_json::json;
use sha2::{Digest as _, Sha256};

use crate::der;
use crate::publisher::{self, Pins};
use crate::yard::Publisher;

const ECDSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
const GITHUB_ACTIONS: &str = "https://token.actions.githubusercontent.com";
const LOG: &str = "rekor.test";

/// The test Fulcio CA and Rekor log.
pub struct Authority {
    ca: EcdsaKeyPair,
    ca_cert: Vec<u8>,
    rekor: EcdsaKeyPair,
    rekor_spki: Vec<u8>,
    rng: SystemRandom,
}

/// A GitHub Actions publisher for `owner/repo` with numeric ids derived from the names, publishing from
/// `.github/workflows/publish.yml`.
pub fn github(owner_id: u64, repository_id: u64) -> Publisher {
    Publisher {
        issuer: GITHUB_ACTIONS.into(),
        subject: None,
        owner_id: Some(owner_id),
        repository_id: Some(repository_id),
        workflow: Some(".github/workflows/publish.yml".into()),
    }
}

impl Default for Authority {
    fn default() -> Self {
        Self::new()
    }
}

impl Authority {
    pub fn new() -> Authority {
        let rng = SystemRandom::new();
        let key = || {
            let doc = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).expect("key");
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, doc.as_ref(), &rng).expect("key")
        };
        let (ca, rekor) = (key(), key());
        let name = name("sigstore-test");
        let tbs = tbs(
            &name,
            &name,
            1,
            (0, 4_000_000_000),
            &spki(ca.public_key().as_ref()),
            &[ext(&[0x55, 0x1d, 0x13], true, &seq(&[tlv(der::BOOLEAN, &[0xff])])), ext(&[0x55, 0x1d, 0x0f], true, &tlv(der::BIT_STRING, &[0x01, 0x06]))],
        );
        let ca_cert = sign_cert(&ca, &rng, tbs);
        let rekor_spki = spki(rekor.public_key().as_ref());
        Authority { ca, ca_cert, rekor, rekor_spki, rng }
    }

    pub fn fulcio_pem(&self) -> String {
        pem("CERTIFICATE", &self.ca_cert)
    }

    pub fn rekor_pem(&self) -> String {
        pem("PUBLIC KEY", &self.rekor_spki)
    }

    pub fn pins(&self) -> Pins {
        Pins::from_pem(&self.fulcio_pem(), &self.rekor_pem()).expect("test pins")
    }

    /// A bundle (v0.3) over a package statement, signed as `publisher` at `at` (Unix seconds).
    pub fn sign_package(
        &self,
        id: &ext_manifest::ExtId,
        version: &ext_manifest::Version,
        digest: &ext_pkg::ContentDigest,
        publisher: &Publisher,
        at: u64,
    ) -> Vec<u8> {
        self.sign(&publisher::statement(id, version, digest), publisher, at)
    }

    /// A bundle (v0.3) over `artifact`, signed as `publisher` at `at`: a fresh key, a 10-minute certificate for it, and
    /// a log entry with its signed timestamp, a one-leaf inclusion proof and a checkpoint.
    pub fn sign(&self, artifact: &[u8], publisher: &Publisher, at: u64) -> Vec<u8> {
        let doc = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &self.rng).expect("key");
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, doc.as_ref(), &self.rng).expect("key");
        let cert = self.certificate_for(key.public_key().as_ref(), publisher, at);
        let signature = key.sign(&self.rng, artifact).expect("sign").as_ref().to_vec();
        let hash = Sha256::digest(artifact);
        let body = hashedrekord(&hash, &signature, &cert);
        let e = self.log(&body, at);
        serde_json::to_vec(&json!({
            "mediaType": "application/vnd.dev.sigstore.bundle.v0.3+json",
            "verificationMaterial": {
                "certificate": { "rawBytes": B64.encode(&cert) },
                "tlogEntries": [{
                    "logIndex": "0", "logId": { "keyId": B64.encode(e.log_id) },
                    "kindVersion": { "kind": "hashedrekord", "version": "0.0.1" },
                    "integratedTime": at.to_string(),
                    "inclusionPromise": { "signedEntryTimestamp": B64.encode(&e.set) },
                    "inclusionProof": { "logIndex": "0", "rootHash": B64.encode(e.root), "treeSize": "1", "hashes": [], "checkpoint": { "envelope": e.checkpoint } },
                    "canonicalizedBody": B64.encode(&body),
                }],
            },
            "messageSignature": { "messageDigest": { "algorithm": "SHA2_256", "digest": B64.encode(hash) }, "signature": B64.encode(&signature) },
        }))
        .unwrap()
    }

    /// What Rekor's `POST /api/v1/log/entries` answers for this entry body, in its own format (hex hashes and log id,
    /// numbers as numbers), so a client's conversion into a bundle is tested too.
    pub fn rekor_response(&self, body: &[u8], at: u64) -> serde_json::Value {
        let e = self.log(body, at);
        json!({ hex(&Sha256::digest(body)): {
            "body": B64.encode(body), "integratedTime": at, "logID": hex(&e.log_id), "logIndex": 0,
            "verification": {
                "signedEntryTimestamp": B64.encode(&e.set),
                "inclusionProof": { "logIndex": 0, "rootHash": hex(&e.root), "treeSize": 1, "hashes": [], "checkpoint": e.checkpoint },
            },
        }})
    }

    /// Log `body` at `at` as entry 0 of a one-leaf tree.
    fn log(&self, body: &[u8], at: u64) -> Logged {
        let body_b64 = B64.encode(body);
        let log_id: [u8; 32] = Sha256::digest(&self.rekor_spki).into();
        let set_payload = format!(r#"{{"body":"{body_b64}","integratedTime":{at},"logID":"{}","logIndex":0}}"#, hex(&log_id));
        let set = self.rekor.sign(&self.rng, set_payload.as_bytes()).expect("sign").as_ref().to_vec();
        let mut leaf = Sha256::new();
        leaf.update([0]);
        leaf.update(body);
        let root: [u8; 32] = leaf.finalize().into();
        let note = format!("{LOG} - 1\n1\n{}\n", B64.encode(root));
        let mut hinted = log_id[..4].to_vec();
        hinted.extend_from_slice(self.rekor.sign(&self.rng, note.as_bytes()).expect("sign").as_ref());
        let checkpoint = format!("{note}\n\u{2014} {LOG} {}\n", B64.encode(hinted));
        Logged { log_id, set, root, checkpoint }
    }

    /// What Fulcio would issue to this public key (an uncompressed P-256 point) for this publisher: code signing, valid
    /// from a minute before `at` for ten minutes.
    pub fn certificate_for(&self, point: &[u8], p: &Publisher, at: u64) -> Vec<u8> {
        let fulcio = |n: u8, value: &str| {
            let mut oid = vec![0x2b, 0x06, 0x01, 0x04, 0x01, 0x83, 0xbf, 0x30, 0x01, n];
            oid.shrink_to_fit();
            ext(&oid, false, &tlv(der::UTF8_STRING, value.as_bytes()))
        };
        let san = match (&p.subject, &p.workflow) {
            (Some(email), _) => tlv(der::SAN_EMAIL, email.as_bytes()),
            (None, Some(w)) => tlv(der::SAN_URI, format!("https://github.com/o/r/{w}@refs/tags/v1").as_bytes()),
            (None, None) => tlv(der::SAN_URI, b"https://example.test"),
        };
        let mut exts = vec![
            ext(&[0x55, 0x1d, 0x0f], true, &tlv(der::BIT_STRING, &[0x07, 0x80])),
            ext(&[0x55, 0x1d, 0x25], false, &seq(&[tlv(der::OID, &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x03])])),
            ext(der::OID_SUBJECT_ALT_NAME, true, &seq(&[san])),
            fulcio(8, &p.issuer),
        ];
        if let Some(w) = &p.workflow {
            exts.push(fulcio(9, &format!("https://github.com/o/r/{w}@refs/tags/v1")));
        }
        if let Some(id) = p.repository_id {
            exts.push(fulcio(15, &id.to_string()));
        }
        if let Some(id) = p.owner_id {
            exts.push(fulcio(17, &id.to_string()));
        }
        let tbs = tbs(&name("sigstore-test"), &seq(&[]), at, (at - 60, at + 540), &spki(point), &exts);
        sign_cert(&self.ca, &self.rng, tbs)
    }
}

/// A whole registry in a folder, as `yard` would write it: root, signed index, packages and their publisher
/// signatures. Serves the client's tests through [`DirFetch`] without a server or the network.
pub struct TestRegistry {
    pub authority: Authority,
    pub dir: std::path::PathBuf,
    pub name: String,
    root_key: crate::yard::SigningKey,
    online: crate::yard::SigningKey,
    index: crate::yard::Index,
}

impl TestRegistry {
    pub fn new(dir: &std::path::Path, name: &str, now: u64) -> TestRegistry {
        use crate::yard::{self, Role};
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir.join("packages")).unwrap();
        let authority = Authority::new();
        let root_key = yard::SigningKey::generate().unwrap().0;
        let online = yard::SigningKey::generate().unwrap().0;
        let root = yard::Root {
            format: yard::FORMAT,
            version: 1,
            registry: format!("{name}.test"),
            online: yard::OnlineKey { key: online.public(), expires: now + 90 * 86400 },
            fulcio: authority.fulcio_pem(),
            rekor: authority.rekor_pem(),
        };
        let bytes = serde_json::to_vec(&root).unwrap();
        std::fs::write(dir.join("root.json"), &bytes).unwrap();
        std::fs::write(dir.join("root.json.sig"), yard::add_signature("", &root_key, Role::Root, &bytes).unwrap()).unwrap();
        let index = yard::Index { format: yard::FORMAT, registry: root.registry, version: 0, expires: 0, extensions: Default::default() };
        let mut r = TestRegistry { authority, dir: dir.to_path_buf(), name: name.into(), root_key, online, index };
        r.sign(now);
        r
    }

    /// The client's view of it. [`DirFetch`] answers its address, `http://localhost/<name>`, from the folder.
    pub fn registry(&self) -> crate::registry::Registry {
        crate::registry::Registry {
            name: self.name.clone(),
            url: format!("http://localhost/{}", self.name),
            keys: crate::yard::RootKeys { keys: vec![self.root_key.public()], threshold: 1 },
        }
    }

    /// Publish a package signed by `publisher` at `now`, and sign a new index.
    pub fn publish(&mut self, bytes: &[u8], publisher: &Publisher, now: u64) -> crate::yard::Release {
        let pkg = ext_pkg::read(bytes).expect("test package");
        let m = &pkg.manifest;
        let bundle = self.authority.sign_package(&m.id, &m.version, &pkg.digest, publisher, now);
        let digest = pkg.digest.hex();
        std::fs::write(self.dir.join(format!("packages/{digest}.cyx")), bytes).unwrap();
        std::fs::write(self.dir.join(format!("packages/{digest}.sigstore.json")), &bundle).unwrap();
        let details = crate::yard::Details::of(&pkg).to_bytes();
        std::fs::write(self.dir.join(format!("packages/{digest}.details.json")), &details).unwrap();
        let release = crate::yard::Release {
            version: m.version.clone(),
            digest,
            sha256: hex(&Sha256::digest(bytes)),
            size: bytes.len() as u64,
            permissions: crate::yard::permission_digest(m),
            code: !m.is_declarative() || m.has_native_programs(),
            published: now,
            status: crate::yard::Status::Ok,
            downloads: 0,
            immediate: false,
            details: Some(crate::yard::Details::digest(&details)),
            targets: Default::default(),
        };
        let entry = self.index.extensions.entry(m.id.clone()).or_insert_with(|| crate::yard::Entry {
            name: m.name.clone(),
            description: m.description.clone().unwrap_or_default(),
            categories: m.categories.clone(),
            publisher: publisher.clone(),
            versions: Vec::new(),
        });
        // A per-platform package joins its version's other platforms, as the registry does.
        if let Some(t) = &pkg.target {
            let target = crate::yard::TargetPackage { digest: release.digest.clone(), sha256: release.sha256.clone(), size: release.size };
            if let Some(r) = entry.versions.iter_mut().find(|r| r.version == m.version) {
                r.targets.insert(t.clone(), target);
                let r = r.clone();
                self.sign(now);
                return r;
            }
            let mut release = release;
            release.targets.insert(t.clone(), target);
            entry.versions.push(release.clone());
            entry.versions.sort_by(|a, b| a.version.cmp(&b.version));
            self.sign(now);
            return release;
        }
        entry.versions.push(release.clone());
        entry.versions.sort_by(|a, b| a.version.cmp(&b.version));
        self.sign(now);
        release
    }

    /// Change one version's index entry (download counts, status, …) and sign a new index.
    pub fn edit_release(&mut self, id: &str, version: &str, now: u64, f: impl FnOnce(&mut crate::yard::Release)) {
        let id = ext_manifest::ExtId::parse(id).unwrap();
        let v = ext_manifest::Version::parse(version).unwrap();
        f(self.index.extensions.get_mut(&id).unwrap().versions.iter_mut().find(|r| r.version == v).unwrap());
        self.sign(now);
    }

    /// Hand an id to another publisher (what the registry's admin does after a transfer) and sign a new index.
    pub fn set_publisher(&mut self, id: &str, publisher: &Publisher, now: u64) {
        let id = ext_manifest::ExtId::parse(id).unwrap();
        self.index.extensions.get_mut(&id).unwrap().publisher = publisher.clone();
        self.sign(now);
    }

    pub fn set_status(&mut self, id: &str, version: &str, status: crate::yard::Status, now: u64) {
        self.edit_release(id, version, now, |r| r.status = status);
    }

    /// Sign the index again (a new version, expiring 72 h after `now`).
    pub fn sign(&mut self, now: u64) {
        self.index.version += 1;
        self.index.expires = now + crate::yard::INDEX_LIFETIME_SECS;
        let bytes = serde_json::to_vec(&self.index).unwrap();
        std::fs::write(self.dir.join("index.json.sig"), crate::yard::add_signature("", &self.online, crate::yard::Role::Index, &bytes).unwrap()).unwrap();
        std::fs::write(self.dir.join("index.json"), bytes).unwrap();
    }

    /// The `extensions.registries` entry a user would write for it.
    pub fn setting(&self) -> serde_json::Value {
        let r = self.registry();
        json!({ "name": r.name, "url": r.url, "root-keys": r.keys.keys.iter().map(|k| k.hex()).collect::<Vec<_>>(), "threshold": 1 })
    }

    /// Change a published file (for tests of what the client refuses).
    pub fn tamper(&self, rel: &str, f: impl FnOnce(&mut Vec<u8>)) {
        let path = self.dir.join(rel);
        let mut bytes = std::fs::read(&path).unwrap();
        f(&mut bytes);
        std::fs::write(path, bytes).unwrap();
    }
}

pub const NOT_THIS_REGISTRY: &str = "not this test registry";

/// Serves a [`TestRegistry`]'s address from its folder, with ETags, and counts the requests to it. Another address is
/// an error saying so ([`NOT_THIS_REGISTRY`]), so tests running side by side can each route to their own.
pub struct DirFetch {
    pub dir: std::path::PathBuf,
    base: String,
    pub requests: std::sync::Mutex<Vec<String>>,
}

impl DirFetch {
    pub fn new(r: &TestRegistry) -> DirFetch {
        DirFetch { dir: r.dir.clone(), base: format!("{}/", r.registry().url), requests: Default::default() }
    }

    pub fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

impl crate::registry::Fetch for DirFetch {
    fn get(&self, url: &str, etag: Option<&str>, limit: usize) -> Result<crate::registry::Got, String> {
        let rel = url.strip_prefix(&self.base).ok_or_else(|| format!("{url}: {NOT_THIS_REGISTRY}"))?;
        self.requests.lock().unwrap().push(url.to_string());
        let bytes = std::fs::read(self.dir.join(rel)).map_err(|_| format!("404: {rel}"))?;
        if bytes.len() > limit {
            return Err(format!("{rel} is larger than {limit} bytes"));
        }
        let tag = hex(&Sha256::digest(&bytes));
        if etag == Some(tag.as_str()) {
            return Ok(crate::registry::Got::NotModified);
        }
        Ok(crate::registry::Got::Body { bytes, etag: Some(tag) })
    }
}

struct Logged {
    log_id: [u8; 32],
    set: Vec<u8>,
    root: [u8; 32],
    checkpoint: String,
}

/// The manifest of a test extension with a native program: `./bin/hello`, which a command of its code may run.
pub fn native_manifest(id: &str, version: &str) -> String {
    format!(
        "[extension]\nid = \"{id}\"\nversion = \"{version}\"\nname = \"Hello\"\nlicense = \"MIT\"\n\n[permissions]\nrun = [\"./bin/hello\"]\nwhy = \"Runs its own helper.\"\n"
    )
}

/// One platform's package of [`native_manifest`]: `bin/hello` is a shell script that prints its first argument, so a
/// test can run it on a Unix machine whatever the platform it is labelled for.
pub fn native_package(id: &str, version: &str, target: &str) -> Vec<u8> {
    let program = ext_manifest::program_file("./bin/hello", target);
    let entries = vec![
        ext_pkg::Entry { path: ext_manifest::FILE_NAME.into(), data: native_manifest(id, version).into_bytes(), executable: false },
        ext_pkg::Entry { path: program, data: format!("#!/bin/sh\necho \"hello from {target}: $1\"\n").into_bytes(), executable: true },
        ext_pkg::Entry { path: ext_pkg::TARGET.into(), data: format!("{target}\n").into_bytes(), executable: false },
    ];
    ext_pkg::write(entries).expect("native test package").0
}

/// A Rekor `hashedrekord` 0.0.1 entry body.
pub fn hashedrekord(hash: &[u8], signature: &[u8], cert: &[u8]) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "apiVersion": "0.0.1", "kind": "hashedrekord",
        "spec": { "data": { "hash": { "algorithm": "sha256", "value": hex(hash) } },
                  "signature": { "content": B64.encode(signature), "publicKey": { "content": B64.encode(pem("CERTIFICATE", cert)) } } },
    }))
    .unwrap()
}

/// PEM text of one DER block.
pub fn pem(label: &str, der: &[u8]) -> String {
    let b64 = B64.encode(der);
    let lines: Vec<&str> = b64.as_bytes().chunks(64).map(|c| std::str::from_utf8(c).unwrap()).collect();
    format!("-----BEGIN {label}-----\n{}\n-----END {label}-----\n", lines.join("\n"))
}

fn sign_cert(ca: &EcdsaKeyPair, rng: &SystemRandom, tbs: Vec<u8>) -> Vec<u8> {
    let sig = ca.sign(rng, &tbs).expect("sign");
    let mut bits = vec![0];
    bits.extend_from_slice(sig.as_ref());
    seq(&[tbs, seq(&[tlv(der::OID, ECDSA_SHA256)]), tlv(der::BIT_STRING, &bits)])
}

fn tbs(issuer: &[u8], subject: &[u8], serial: u64, (from, to): (u64, u64), spki: &[u8], exts: &[Vec<u8>]) -> Vec<u8> {
    let mut serial_bytes = vec![0];
    serial_bytes.extend_from_slice(&serial.max(1).to_be_bytes());
    seq(&[
        tlv(0xa0, &tlv(0x02, &[2])),
        tlv(0x02, &serial_bytes),
        seq(&[tlv(der::OID, ECDSA_SHA256)]),
        issuer.to_vec(),
        seq(&[time(from), time(to)]),
        subject.to_vec(),
        spki.to_vec(),
        tlv(der::EXTENSIONS, &seq(exts)),
    ])
}

fn spki(point: &[u8]) -> Vec<u8> {
    let mut bits = vec![0];
    bits.extend_from_slice(point);
    seq(&[seq(&[tlv(der::OID, der::OID_EC_PUBLIC_KEY), tlv(der::OID, der::OID_P256)]), tlv(der::BIT_STRING, &bits)])
}

fn name(cn: &str) -> Vec<u8> {
    seq(&[tlv(0x31, &seq(&[tlv(der::OID, &[0x55, 0x04, 0x03]), tlv(der::UTF8_STRING, cn.as_bytes())]))])
}

fn ext(oid: &[u8], critical: bool, value: &[u8]) -> Vec<u8> {
    let mut parts = vec![tlv(der::OID, oid)];
    if critical {
        parts.push(tlv(der::BOOLEAN, &[0xff]));
    }
    parts.push(tlv(der::OCTET_STRING, value));
    seq(&parts)
}

/// UTCTime before 2050, GeneralizedTime after, as RFC 5280 requires.
fn time(unix: u64) -> Vec<u8> {
    let days = (unix / 86400) as i64;
    let secs = unix % 86400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let hms = format!("{:02}{:02}{:02}{:02}{:02}Z", m, d, secs / 3600, secs / 60 % 60, secs % 60);
    if y < 2050 { tlv(0x17, format!("{:02}{hms}", y % 100).as_bytes()) } else { tlv(0x18, format!("{y:04}{hms}").as_bytes()) }
}

fn seq(parts: &[Vec<u8>]) -> Vec<u8> {
    tlv(der::SEQUENCE, &parts.concat())
}

fn tlv(tag: u8, value: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let n = value.len();
    if n < 0x80 {
        out.push(n as u8);
    } else {
        let bytes: Vec<u8> = n.to_be_bytes().into_iter().skip_while(|b| *b == 0).collect();
        out.push(0x80 | bytes.len() as u8);
        out.extend(bytes);
    }
    out.extend_from_slice(value);
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// An identity token for the stand-in Fulcio ([`serve_sigstore`]): `sub` is what the key proves it holds, and the
/// publisher the certificate names travels in the claims. Only the stand-in reads it; a real Fulcio refuses it.
pub fn identity_token(sub: &str, publisher: &Publisher) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
    let claims = json!({ "sub": sub, "test_publisher": publisher });
    format!("h.{}.s", B64URL.encode(claims.to_string()))
}

/// A stand-in Fulcio and Rekor on a local port, answering in their real formats from `authority`: `POST
/// /api/v2/signingCert` like Fulcio, `POST /api/v1/log/entries` like Rekor. Returns its base URL (for both
/// `CODEYARD_FULCIO_URL` and `CODEYARD_REKOR_URL`). Serves on a thread until the process ends; one request at a time.
pub fn serve_sigstore(authority: std::sync::Arc<Authority>) -> String {
    serve_sigstore_on(authority, "127.0.0.1:0").expect("a local port")
}

/// [`serve_sigstore`] on a given address.
pub fn serve_sigstore_on(authority: std::sync::Arc<Authority>, addr: &str) -> std::io::Result<String> {
    use std::io::{BufRead as _, BufReader, Read as _, Write as _};
    let listener = std::net::TcpListener::bind(addr)?;
    let url = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Ok(read) = stream.try_clone() else { continue };
            let mut reader = BufReader::new(read);
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let path = line.split(' ').nth(1).unwrap_or_default().to_string();
            let mut len = 0;
            loop {
                let mut h = String::new();
                if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0; len];
            if reader.read_exact(&mut body).is_err() {
                continue;
            }
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let (status, answer) = match path.as_str() {
                "/api/v2/signingCert" => fulcio(&authority, &body, now),
                "/api/v1/log/entries" => (201, authority.rekor_response(&body, now).to_string()),
                _ => (404, "not found".into()),
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                answer.len()
            );
        }
    });
    Ok(url)
}

/// Fulcio's checks, in short: a token naming a publisher, a P-256 key, and proof that the key signed the subject.
fn fulcio(a: &Authority, body: &[u8], now: u64) -> (u16, String) {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
    let bad = |why: &str| (400, why.to_string());
    let Ok(req) = serde_json::from_slice::<serde_json::Value>(body) else { return bad("not JSON") };
    let Some(token) = req["credentials"]["oidcIdentityToken"].as_str() else { return bad("no token") };
    let Some(claims) = token.split('.').nth(1).and_then(|c| B64URL.decode(c).ok()).and_then(|c| serde_json::from_slice::<serde_json::Value>(&c).ok()) else {
        return bad("not a token");
    };
    let Ok(publisher) = serde_json::from_value::<Publisher>(claims["test_publisher"].clone()) else { return bad("no test publisher in the token") };
    let key_pem = req["publicKeyRequest"]["publicKey"]["content"].as_str().unwrap_or_default();
    let Ok(der) = B64.decode(key_pem.lines().filter(|l| !l.starts_with("-----")).collect::<String>()) else { return bad("not a PEM key") };
    if der.len() < 65 {
        return bad("not a P-256 key");
    }
    let point = &der[der.len() - 65..];
    let Ok(proof) = B64.decode(req["publicKeyRequest"]["proofOfPossession"].as_str().unwrap_or_default()) else { return bad("no proof") };
    let sub = claims["sub"].as_str().unwrap_or_default();
    if ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_ASN1, point).verify(sub.as_bytes(), &proof).is_err() {
        return bad("proof of possession does not verify");
    }
    let cert = a.certificate_for(point, &publisher, now);
    (201, json!({ "signedCertificateEmbeddedSct": { "chain": { "certificates": [pem("CERTIFICATE", &cert)] } } }).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: u64 = 1_790_000_000;

    #[test]
    fn its_bundles_verify_and_carry_the_identity() {
        let a = Authority::new();
        let p = github(7, 42);
        let bundle = a.sign(b"hello", &p, AT);
        let signed = publisher::verify(&bundle, b"hello", &a.pins(), AT + 100).unwrap();
        assert_eq!(signed.publisher, p);
        assert_eq!(signed.logged, AT);
        let browser = Publisher {
            issuer: "https://github.com/login/oauth".into(),
            subject: Some("me@example.com".into()),
            owner_id: None,
            repository_id: None,
            workflow: None,
        };
        assert_eq!(publisher::verify(&a.sign(b"x", &browser, AT), b"x", &a.pins(), AT).unwrap().publisher, browser);
    }

    #[test]
    fn another_authority_is_not_trusted() {
        let (a, b) = (Authority::new(), Authority::new());
        let bundle = a.sign(b"hello", &github(1, 2), AT);
        assert!(publisher::verify(&bundle, b"hello", &b.pins(), AT).is_err());
        assert!(publisher::verify(&bundle, b"other", &a.pins(), AT).is_err());
    }

    #[test]
    fn dates_encode_as_rfc_5280_says() {
        assert_eq!(time(0), tlv(0x17, b"700101000000Z"));
        assert_eq!(time(1_790_000_000), tlv(0x17, b"260921141320Z"));
        assert_eq!(time(2_524_608_000), tlv(0x18, b"20500101000000Z"));
    }
}
