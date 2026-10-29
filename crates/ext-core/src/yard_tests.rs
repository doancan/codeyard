use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::*;

const NOW: u64 = 1_800_000_000;
const DIGEST: &str = "1111111111111111111111111111111111111111111111111111111111111111";

/// One change to a valid index.
type Change = Box<dyn Fn(&mut Value)>;

struct Yard {
    roots: Vec<SigningKey>,
    online: SigningKey,
    root: Vec<u8>,
    root_sig: Vec<u8>,
    index: Vec<u8>,
    index_sig: Vec<u8>,
}

fn key() -> SigningKey {
    SigningKey::generate().unwrap().0
}

fn root_json(online: &SigningKey, version: u64, expires: u64) -> Vec<u8> {
    serde_json::to_vec_pretty(&json!({
        "format": 1, "version": version, "registry": "yard.test",
        "online": { "key": online.public().hex(), "expires": expires },
        "fulcio": "-----BEGIN CERTIFICATE-----\n…\n-----END CERTIFICATE-----\n", "rekor": "-----BEGIN PUBLIC KEY-----\n…\n-----END PUBLIC KEY-----\n",
    }))
    .unwrap()
}

fn index_value(version: u64) -> Value {
    let release = |v: &str, status: Value| {
        json!({ "version": v, "digest": DIGEST, "sha256": DIGEST, "size": 1200, "permissions": DIGEST, "published": NOW - 100, "status": status })
    };
    json!({
        "format": 1, "registry": "yard.test", "version": version, "expires": NOW + INDEX_LIFETIME_SECS,
        "extensions": {
            "you.night": {
                "name": "Night", "description": "A dark theme.", "categories": ["themes"],
                "publisher": { "issuer": "https://token.actions.githubusercontent.com", "owner_id": 7, "repository_id": 42,
                               "workflow": "you/night/.github/workflows/publish.yml" },
                "versions": [release("1.0.0", json!("ok")), release("1.1.0", json!("yanked")), release("1.2.0-beta.1", json!("ok"))],
            },
            "you.old": {
                "name": "Old", "publisher": { "issuer": "https://github.com/login/oauth", "owner_id": 8 },
                "versions": [release("0.1.0", json!({ "deprecated": { "replacement": "you.night" } })),
                             release("0.2.0", json!({ "revoked": { "reason": "Sent files to an unknown host." } }))],
            },
        },
    })
}

fn sig_file(keys: &[&SigningKey], role: Role, bytes: &[u8]) -> Vec<u8> {
    let mut text = String::new();
    for k in keys {
        text = add_signature(&text, k, role, bytes).unwrap();
    }
    text.into_bytes()
}

impl Yard {
    fn new() -> Yard {
        let roots = vec![key(), key(), key()];
        let online = key();
        let root = root_json(&online, 1, NOW + 90 * 86400);
        let root_sig = sig_file(&[&roots[0], &roots[2]], Role::Root, &root);
        let index = serde_json::to_vec(&index_value(5)).unwrap();
        let index_sig = sig_file(&[&online], Role::Index, &index);
        Yard { roots, online, root, root_sig, index, index_sig }
    }

    fn trust(&self) -> RootKeys {
        RootKeys { keys: self.roots.iter().map(SigningKey::public).collect(), threshold: 2 }
    }

    fn files(&self) -> [&[u8]; 4] {
        [&self.root, &self.root_sig, &self.index, &self.index_sig]
    }

    fn verify(&self) -> Result<Verified, Error> {
        verify(&self.trust(), self.files(), NOW, Seen::default())
    }

    fn set_index(&mut self, index: Vec<u8>) {
        self.index_sig = sig_file(&[&self.online], Role::Index, &index);
        self.index = index;
    }
}

#[test]
fn a_valid_registry_verifies() {
    let y = Yard::new();
    let v = y.verify().unwrap();
    assert!(!v.stale);
    assert_eq!(v.seen(), Seen { root: 1, index: 5 });
    let night = ExtId::parse("you.night").unwrap();
    assert_eq!(v.index.extensions[&night].publisher.repository_id, Some(42));
    // Latest skips yanked versions and pre-releases unless asked for.
    assert_eq!(v.index.latest(&night, false).unwrap().version, Version::parse("1.0.0").unwrap());
    assert_eq!(v.index.latest(&night, true).unwrap().version, Version::parse("1.2.0-beta.1").unwrap());
    let old = ExtId::parse("you.old").unwrap();
    assert_eq!(v.index.latest(&old, false).unwrap().version, Version::parse("0.1.0").unwrap(), "deprecated is installable, revoked is not");
    assert!(matches!(&v.index.release(&old, &Version::parse("0.2.0").unwrap()).unwrap().status, Status::Revoked { reason } if reason.contains("unknown host")));
}

#[test]
fn the_root_needs_a_threshold_of_different_trusted_keys() {
    let mut y = Yard::new();
    y.root_sig = sig_file(&[&y.roots[1]], Role::Root, &y.root);
    assert_eq!(y.verify().unwrap_err(), Error::Signature { file: ROOT_FILE, valid: 1, needed: 2 });
    // The same key twice is one signer.
    let one = String::from_utf8(sig_file(&[&y.roots[1]], Role::Root, &y.root)).unwrap();
    let mut sigs: Signatures = serde_json::from_str(&one).unwrap();
    sigs.signatures.push(sigs.signatures[0].clone());
    y.root_sig = serde_json::to_vec(&sigs).unwrap();
    assert!(matches!(y.verify(), Err(Error::Signature { valid: 1, .. })));
    // An unknown key does not count.
    let stranger = key();
    y.root_sig = sig_file(&[&y.roots[1], &stranger], Role::Root, &y.root);
    assert!(matches!(y.verify(), Err(Error::Signature { valid: 1, .. })));
    // A threshold larger than the keys can never be met.
    y.root_sig = sig_file(&[&y.roots[0], &y.roots[1], &y.roots[2]], Role::Root, &y.root);
    let trust = RootKeys { threshold: 4, ..y.trust() };
    assert!(matches!(verify(&trust, y.files(), NOW, Seen::default()), Err(Error::Signature { needed: 4, .. })));
}

#[test]
fn a_signature_is_bound_to_its_file_and_its_bytes() {
    let mut y = Yard::new();
    // The online key's signature over the index does not pass as a root signature, nor the other way round.
    let swapped = sig_file(&[&y.roots[0], &y.roots[1]], Role::Index, &y.root);
    y.root_sig = swapped;
    assert!(matches!(y.verify(), Err(Error::Signature { file: ROOT_FILE, .. })));
    let mut y = Yard::new();
    y.index_sig = sig_file(&[&y.online], Role::Root, &y.index);
    assert!(matches!(y.verify(), Err(Error::Signature { file: INDEX_FILE, .. })));
    // One changed byte, even whitespace, breaks it.
    let mut y = Yard::new();
    y.index.push(b'\n');
    assert!(matches!(y.verify(), Err(Error::Signature { file: INDEX_FILE, valid: 0, .. })));
    // A root key cannot sign the index: only the online key named in the root can.
    let mut y = Yard::new();
    y.index_sig = sig_file(&[&y.roots[0]], Role::Index, &y.index);
    assert!(matches!(y.verify(), Err(Error::Signature { file: INDEX_FILE, .. })));
}

#[test]
fn expiry_and_rollback() {
    let y = Yard::new();
    // An expired online key refuses everything.
    assert!(matches!(verify(&y.trust(), y.files(), NOW + 90 * 86400, Seen::default()), Err(Error::KeyExpired { .. })));
    // An expired index is returned, marked stale.
    assert!(verify(&y.trust(), y.files(), NOW + INDEX_LIFETIME_SECS, Seen::default()).unwrap().stale);
    // Older than what was accepted before.
    assert_eq!(
        verify(&y.trust(), y.files(), NOW, Seen { root: 1, index: 6 }).unwrap_err(),
        Error::Rollback { file: INDEX_FILE, seen: 6, found: 5 }
    );
    assert!(matches!(verify(&y.trust(), y.files(), NOW, Seen { root: 2, index: 0 }), Err(Error::Rollback { file: ROOT_FILE, .. })));
    // The same copy again (a 304) is fine.
    assert!(verify(&y.trust(), y.files(), NOW, Seen { root: 1, index: 5 }).is_ok());
}

#[test]
fn a_new_online_key_comes_with_a_new_root() {
    let mut y = Yard::new();
    let rotated = key();
    y.root = root_json(&rotated, 2, NOW + 90 * 86400);
    y.root_sig = sig_file(&[&y.roots[1], &y.roots[2]], Role::Root, &y.root);
    // The old online key no longer signs.
    assert!(matches!(y.verify(), Err(Error::Signature { file: INDEX_FILE, .. })));
    y.online = rotated;
    let index = y.index.clone();
    y.set_index(index);
    assert_eq!(verify(&y.trust(), y.files(), NOW, Seen { root: 1, index: 5 }).unwrap().seen(), Seen { root: 2, index: 5 });
}

#[test]
fn the_index_is_checked_after_its_signature() {
    let cases: Vec<(&str, Change)> = vec![
        ("format", Box::new(|v| v["format"] = json!(2))),
        ("registry", Box::new(|v| v["registry"] = json!("other.test"))),
        ("digest", Box::new(|v| v["extensions"]["you.night"]["versions"][0]["digest"] = json!("XYZ"))),
        ("order", Box::new(|v| v["extensions"]["you.night"]["versions"].as_array_mut().unwrap().reverse())),
        ("duplicate", Box::new(|v| v["extensions"]["you.old"]["versions"][1]["version"] = json!("0.1.0"))),
        ("no versions", Box::new(|v| v["extensions"]["you.old"]["versions"] = json!([]))),
        ("bad id", Box::new(|v| v["extensions"]["Not An Id"] = v["extensions"]["you.old"].clone())),
        ("control", Box::new(|v| v["extensions"]["you.old"]["name"] = json!("Old\u{1b}[2J"))),
        ("status", Box::new(|v| v["extensions"]["you.old"]["versions"][0]["status"] = json!("trusted"))),
    ];
    for (what, change) in cases {
        let mut y = Yard::new();
        let mut v = index_value(5);
        change(&mut v);
        y.set_index(serde_json::to_vec(&v).unwrap());
        let err = y.verify().expect_err(what);
        let expected = match what {
            "format" => "CY0803",
            "registry" => "CY0807",
            _ => "CY0802",
        };
        assert_eq!(err.code(), expected, "{what}: {err}");
    }
}

#[test]
fn fields_added_later_are_ignored() {
    let mut y = Yard::new();
    let mut v = index_value(5);
    v["mirrors"] = json!(["https://b.test"]);
    v["extensions"]["you.old"]["versions"][0]["sbom"] = json!({ "url": "x" });
    y.set_index(serde_json::to_vec(&v).unwrap());
    assert!(y.verify().is_ok());
}

#[test]
fn errors_read_like_the_rest_of_the_catalogue() {
    let e = Error::Rollback { file: INDEX_FILE, seen: 9, found: 4 };
    assert_eq!(
        e.to_string(),
        "error[CY0806]: index.json is version 4, older than version 9 seen before\n   = fix: an older copy of the registry was served (by a cache or an attacker); try again later"
    );
    // The registry's codes do not collide with the manifest's.
    for c in ext_manifest::diagnostics::Code::ALL {
        assert!(!c.as_str().starts_with("CY08"), "{}", c.as_str());
    }
}

#[test]
fn limits_are_checked_before_parsing() {
    let mut y = Yard::new();
    y.index = vec![b' '; MAX_INDEX_BYTES + 1];
    assert_eq!(y.verify().unwrap_err(), Error::TooLarge { file: INDEX_FILE, limit: MAX_INDEX_BYTES });
}

#[test]
fn key_files_round_trip_and_are_checked() {
    let (k, text) = SigningKey::generate().unwrap();
    assert_eq!(SigningKey::from_file(&text).unwrap().public(), k.public());
    let other = SigningKey::generate().unwrap().1;
    let mixed: Value = serde_json::from_str(&text).unwrap();
    let mut forged: Value = serde_json::from_str(&other).unwrap();
    forged["public"] = mixed["public"].clone();
    assert!(SigningKey::from_file(&forged.to_string()).unwrap_err().contains("does not match"));
    assert!(SigningKey::from_file("{}").is_err());
    // Re-signing replaces the key's older signature instead of adding a second one.
    let once = add_signature("", &k, Role::Root, b"a").unwrap();
    let twice = add_signature(&once, &k, Role::Root, b"a").unwrap();
    assert_eq!(serde_json::from_str::<Signatures>(&twice).unwrap().signatures.len(), 1);
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn random_value(rng: &mut Rng) -> Value {
    match rng.below(8) {
        0 => Value::Null,
        1 => json!(rng.next().is_multiple_of(3)),
        2 => json!(rng.next()),
        3 => json!(-(rng.below(1000) as i64)),
        4 => json!(["1.0.0", "x", DIGEST][rng.below(3)]),
        5 => json!("\u{202e}\u{0}é".repeat(rng.below(4))),
        6 => json!([]),
        _ => json!({ "ok": {} }),
    }
}

/// Replace one random node of `v` with a random value.
fn mutate(v: &mut Value, rng: &mut Rng) {
    let mut node = v;
    loop {
        let children = match node {
            Value::Object(m) => m.len(),
            Value::Array(a) => a.len(),
            _ => 0,
        };
        if children == 0 || rng.below(4) == 0 {
            *node = random_value(rng);
            return;
        }
        let i = rng.below(children);
        node = match node {
            Value::Object(m) => m.values_mut().nth(i).unwrap(),
            Value::Array(a) => &mut a[i],
            _ => unreachable!(),
        };
    }
}

/// Signed but hostile: any index the online key signs must be refused or verified, never panic, and what verifies
/// keeps the promises the client relies on. Unsigned changes never verify. See `ext-pkg`'s fuzz tests for the knobs.
#[test]
fn fuzz_verify_never_panics() {
    let extra: u64 = std::env::var("CODEYARD_FUZZ_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    let deadline = std::env::var("CODEYARD_FUZZ_SECS").ok().and_then(|s| s.parse().ok()).map(|s| Instant::now() + Duration::from_secs(s));
    let seed = (0x5eed_7a4d ^ extra.wrapping_mul(0x9e37_79b9_7f4a_7c15)).max(1);
    let mut rng = Rng(seed);
    let base = Yard::new();
    let mut round = 0usize;
    while round < 400 || deadline.is_some_and(|d| Instant::now() < d) {
        round += 1;
        let resign = rng.below(3) != 0;
        let mut bytes = if rng.below(2) == 0 {
            let mut v = index_value(5);
            for _ in 0..=rng.below(3) {
                mutate(&mut v, &mut rng);
            }
            serde_json::to_vec(&v).unwrap()
        } else {
            let mut b = base.index.clone();
            for _ in 0..=rng.below(4) {
                let i = rng.below(b.len());
                match rng.below(3) {
                    0 => b[i] = rng.next() as u8,
                    1 => b.truncate(i),
                    _ => b.insert(i, b"{}[]\",:0e"[rng.below(9)]),
                }
                if b.is_empty() {
                    break;
                }
            }
            b
        };
        if bytes.is_empty() {
            bytes.push(b'{');
        }
        let changed = bytes != base.index;
        let sig = if resign { sig_file(&[&base.online], Role::Index, &bytes) } else { base.index_sig.clone() };
        let trust = base.trust();
        let files: [&[u8]; 4] = [&base.root, &base.root_sig, &bytes, &sig];
        let result = std::panic::catch_unwind(|| verify(&trust, files, NOW, Seen::default()));
        let Ok(result) = result else { panic!("fuzz: panicked at round {round} of seed {seed:#x}") };
        if let Ok(v) = result {
            assert!(resign || !changed, "fuzz: an unsigned change verified at round {round} of seed {seed:#x}");
            for e in v.index.extensions.values() {
                assert!(!e.versions.is_empty() && e.versions.windows(2).all(|p| p[0].version < p[1].version));
                assert!(e.versions.iter().all(|r| parse_hex::<32>(&r.digest).is_some()));
            }
        }
    }
}
