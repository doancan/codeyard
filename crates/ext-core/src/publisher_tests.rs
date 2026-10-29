use std::path::{Path, PathBuf};

use serde_json::Value;

use super::*;

const NOW: u64 = 1_800_000_000;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/sigstore")
}

/// Pins from a Sigstore `trusted_root.json`: every CA certificate and every EC log key, as a registry root would hold
/// them in PEM.
fn pins_from(trusted_root: &Path) -> Pins {
    let root: Value = serde_json::from_slice(&std::fs::read(trusted_root).unwrap()).unwrap();
    let pem = |label: &str, b64: &str| format!("-----BEGIN {label}-----\n{b64}\n-----END {label}-----\n");
    let mut fulcio = String::new();
    for ca in root["certificateAuthorities"].as_array().unwrap() {
        for c in ca["certChain"]["certificates"].as_array().unwrap() {
            fulcio += &pem("CERTIFICATE", c["rawBytes"].as_str().unwrap());
        }
    }
    let mut rekor = String::new();
    for log in root["tlogs"].as_array().unwrap() {
        if log["publicKey"]["keyDetails"].as_str().unwrap().starts_with("PKIX_ECDSA") {
            rekor += &pem("PUBLIC KEY", log["publicKey"]["rawBytes"].as_str().unwrap());
        }
    }
    Pins::from_pem(&fulcio, &rekor).unwrap()
}

fn case(name: &str) -> Result<Signed, Error> {
    let dir = fixtures().join(name);
    let artifact = std::fs::read(if dir.join("artifact").exists() { dir.join("artifact") } else { fixtures().join("a.txt") }).unwrap();
    let root = if dir.join("trusted_root.json").exists() { dir.join("trusted_root.json") } else { fixtures().join("trusted_root.json") };
    verify(&std::fs::read(dir.join("bundle.sigstore.json")).unwrap(), &artifact, &pins_from(&root), NOW)
}

#[test]
fn the_conformance_suite() {
    let mut names: Vec<String> = std::fs::read_dir(fixtures())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert!(names.len() >= 20, "{names:?}");
    let mut wrong = Vec::new();
    for name in &names {
        let result = case(name);
        let should_pass = !name.ends_with("_fail");
        if result.is_ok() != should_pass {
            wrong.push(format!("{name}: {result:?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn the_identity_comes_from_the_certificate() {
    let signed = case("happy-path-v0.3").unwrap();
    let p = &signed.publisher;
    assert_eq!(p.issuer, "https://token.actions.githubusercontent.com");
    assert_eq!(p.subject, None);
    assert_eq!(p.workflow.as_deref(), Some(".github/workflows/extremely-dangerous-oidc-beacon.yml"));
    assert!(p.repository_id.is_some() && p.owner_id.is_some(), "{p:?}");
    assert!(signed.logged > 1_700_000_000 && signed.logged < NOW);
}

#[test]
fn failures_say_which_check() {
    let code = |name: &str| case(name).unwrap_err().code();
    assert_eq!(code("set-invalid-signature_fail"), "CY0813");
    assert_eq!(code("bundle-from-wrong-instance_fail"), "CY0813");
    assert_eq!(code("invalid-inclusion-proof_fail"), "CY0813");
    assert_eq!(code("signature-mismatch_fail"), "CY0814");
    assert_eq!(code("wrong-material_fail"), "CY0814");
    assert_eq!(code("bundle-unknown-version_fail"), "CY0811");
    let e = case("integrated-time-in-future_fail").unwrap_err();
    assert!(matches!(e, Error::Certificate(_) | Error::Log(_)), "{e}");
    assert!(e.to_string().starts_with("error[CY08"), "{e}");
}

#[test]
fn a_statement_binds_id_version_and_digest() {
    let id = ExtId::parse("you.night").unwrap();
    let v = Version::parse("1.2.0").unwrap();
    let d = ContentDigest([0xab; 32]);
    let s = String::from_utf8(statement(&id, &v, &d)).unwrap();
    assert_eq!(s, format!("codeyard-yard/v1/package\nyou.night 1.2.0 {}\n", "ab".repeat(32)));
    // A real bundle over another artifact never passes for a package.
    let dir = fixtures().join("happy-path-v0.3");
    let pins = pins_from(&fixtures().join("trusted_root.json"));
    let err = verify_package(&std::fs::read(dir.join("bundle.sigstore.json")).unwrap(), &id, &v, &d, &pins, NOW).unwrap_err();
    assert_eq!(err.code(), "CY0814");
}

#[test]
fn pins_compare_whole_identities() {
    let ci =
        Publisher { issuer: GITHUB_ACTIONS.into(), subject: None, owner_id: Some(1), repository_id: Some(2), workflow: Some(".github/workflows/p.yml".into()) };
    assert_eq!(check_pin(None, &ci), Pin::New);
    assert_eq!(check_pin(Some(&ci), &ci), Pin::Same);
    let moved = Publisher { repository_id: Some(3), ..ci.clone() };
    assert_eq!(check_pin(Some(&ci), &moved), Pin::Changed { was: ci.clone() });
    let other_workflow = Publisher { workflow: Some(".github/workflows/other.yml".into()), ..ci.clone() };
    assert!(matches!(check_pin(Some(&ci), &other_workflow), Pin::Changed { .. }));
}

#[test]
fn workflow_paths_drop_the_repository_and_ref() {
    assert_eq!(workflow_path("https://github.com/a/b/.github/workflows/p.yml@refs/tags/v1").as_deref(), Some(".github/workflows/p.yml"));
    assert_eq!(workflow_path("https://github.com/a/b/src/p.yml@refs/heads/main"), None);
    assert_eq!(workflow_path("https://gitlab.com/a/b/.github/workflows/p.yml@x"), None);
    assert_eq!(workflow_path("https://github.com/a/b/.github/workflows/../p.yml"), None);
}

#[test]
fn pins_must_be_usable() {
    assert_eq!(Pins::from_pem("", "").unwrap_err().code(), "CY0810");
    assert!(Pins::from_pem("-----BEGIN CERTIFICATE-----\n!!\n-----END CERTIFICATE-----", "").is_err());
}

#[test]
fn der_reader_refuses_what_it_does_not_read() {
    assert_eq!(der::read(&[0x30, 0x80]), None, "indefinite length");
    assert_eq!(der::read(&[0x30, 0x81, 0x05, 0, 0, 0, 0, 0]), None, "long form for a short length");
    assert_eq!(der::read(&[0x1f, 0x01, 0x00]), None, "multi-byte tag");
    assert_eq!(der::read(&[0x04, 0x03, 1, 2]), None, "truncated");
    let (t, rest) = der::read(&[0x04, 0x02, 1, 2, 9]).unwrap();
    assert_eq!((t.tag, t.value, rest), (0x04, &[1u8, 2][..], &[9u8][..]));
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

/// Hostile bundles and certificates: changed bytes in the JSON, in the base64 fields and in the certificate never
/// panic, and only the untouched bundle verifies. Knobs as in `ext-pkg`'s fuzz tests.
#[test]
fn fuzz_bundle_verification_never_panics() {
    let extra: u64 = std::env::var("CODEYARD_FUZZ_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    let deadline = std::env::var("CODEYARD_FUZZ_SECS").ok().and_then(|s| s.parse().ok()).map(|s| std::time::Instant::now() + std::time::Duration::from_secs(s));
    let seed = (0xb0_5d1e ^ extra.wrapping_mul(0x9e37_79b9_7f4a_7c15)).max(1);
    let mut rng = Rng(seed);
    let pins = pins_from(&fixtures().join("trusted_root.json"));
    let artifact = std::fs::read(fixtures().join("a.txt")).unwrap();
    let base = std::fs::read(fixtures().join("happy-path-v0.3/bundle.sigstore.json")).unwrap();
    let json: Value = serde_json::from_slice(&base).unwrap();
    let cert = B64.decode(json["verificationMaterial"]["certificate"]["rawBytes"].as_str().unwrap()).unwrap();
    let mut round = 0usize;
    while round < 300 || deadline.is_some_and(|d| std::time::Instant::now() < d) {
        round += 1;
        let bytes = match rng.below(3) {
            // Bytes of the JSON text.
            0 => {
                let mut b = base.clone();
                for _ in 0..=rng.below(3) {
                    let i = rng.below(b.len());
                    b[i] = rng.next() as u8;
                }
                b
            }
            // Bytes inside the certificate, re-encoded, so the DER reader and webpki see them.
            1 => {
                let mut c = cert.clone();
                for _ in 0..=rng.below(3) {
                    let i = rng.below(c.len());
                    if rng.below(4) == 0 {
                        c.truncate(i.max(1));
                    } else {
                        c[i] = rng.next() as u8;
                    }
                }
                let mut j = json.clone();
                j["verificationMaterial"]["certificate"]["rawBytes"] = Value::String(B64.encode(&c));
                serde_json::to_vec(&j).unwrap()
            }
            // Decoded fields replaced by random base64.
            _ => {
                let mut j = json.clone();
                let field = ["signature", "logId", "body", "set", "root"][rng.below(5)];
                let junk = Value::String(B64.encode((0..rng.below(80)).map(|_| rng.next() as u8).collect::<Vec<_>>()));
                let e = &mut j["verificationMaterial"]["tlogEntries"][0];
                match field {
                    "logId" => e["logId"]["keyId"] = junk,
                    "body" => e["canonicalizedBody"] = junk,
                    "set" => e["inclusionPromise"]["signedEntryTimestamp"] = junk,
                    "root" => e["inclusionProof"]["rootHash"] = junk,
                    _ => j["messageSignature"]["signature"] = junk,
                }
                serde_json::to_vec(&j).unwrap()
            }
        };
        let result = std::panic::catch_unwind(|| verify(&bytes, &artifact, &pins, NOW));
        let Ok(result) = result else { panic!("fuzz: panicked at round {round} of seed {seed:#x}") };
        // Changes that keep the meaning (whitespace, an escaped character) may still verify; anything else must not:
        // every field the bundle has is checked.
        if result.is_ok() {
            let back: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(back, json, "fuzz: a changed bundle verified at round {round} of seed {seed:#x}");
        }
        let _ = der::extensions(&bytes);
        let _ = der::ec_point(&bytes);
    }
}
