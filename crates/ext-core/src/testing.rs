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
        let cert = self.certificate(&key, publisher, at);
        let signature = key.sign(&self.rng, artifact).expect("sign").as_ref().to_vec();
        let hash = Sha256::digest(artifact);
        let body = serde_json::to_vec(&json!({
            "apiVersion": "0.0.1", "kind": "hashedrekord",
            "spec": { "data": { "hash": { "algorithm": "sha256", "value": hex(&hash) } },
                      "signature": { "content": B64.encode(&signature), "publicKey": { "content": B64.encode(pem("CERTIFICATE", &cert)) } } },
        }))
        .unwrap();
        let body_b64 = B64.encode(&body);
        let log_id: [u8; 32] = Sha256::digest(&self.rekor_spki).into();
        let set_payload = format!(r#"{{"body":"{body_b64}","integratedTime":{at},"logID":"{}","logIndex":0}}"#, hex(&log_id));
        let set = self.rekor.sign(&self.rng, set_payload.as_bytes()).expect("sign");
        let mut leaf = Sha256::new();
        leaf.update([0]);
        leaf.update(&body);
        let root = B64.encode(leaf.finalize());
        let note = format!("{LOG} - 1\n1\n{root}\n");
        let mut hinted = log_id[..4].to_vec();
        hinted.extend_from_slice(self.rekor.sign(&self.rng, note.as_bytes()).expect("sign").as_ref());
        let envelope = format!("{note}\n\u{2014} {LOG} {}\n", B64.encode(hinted));
        serde_json::to_vec(&json!({
            "mediaType": "application/vnd.dev.sigstore.bundle.v0.3+json",
            "verificationMaterial": {
                "certificate": { "rawBytes": B64.encode(&cert) },
                "tlogEntries": [{
                    "logIndex": "0", "logId": { "keyId": B64.encode(log_id) },
                    "kindVersion": { "kind": "hashedrekord", "version": "0.0.1" },
                    "integratedTime": at.to_string(),
                    "inclusionPromise": { "signedEntryTimestamp": B64.encode(set.as_ref()) },
                    "inclusionProof": { "logIndex": "0", "rootHash": root, "treeSize": "1", "hashes": [], "checkpoint": { "envelope": envelope } },
                    "canonicalizedBody": body_b64,
                }],
            },
            "messageSignature": { "messageDigest": { "algorithm": "SHA2_256", "digest": B64.encode(hash) }, "signature": B64.encode(&signature) },
        }))
        .unwrap()
    }

    /// What Fulcio would issue for this publisher: code signing, valid from a minute before `at` for ten minutes.
    fn certificate(&self, key: &EcdsaKeyPair, p: &Publisher, at: u64) -> Vec<u8> {
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
        let tbs = tbs(&name("sigstore-test"), &seq(&[]), at, (at - 60, at + 540), &spki(key.public_key().as_ref()), &exts);
        sign_cert(&self.ca, &self.rng, tbs)
    }
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

fn pem(label: &str, der: &[u8]) -> String {
    let b64 = B64.encode(der);
    let lines: Vec<&str> = b64.as_bytes().chunks(64).map(|c| std::str::from_utf8(c).unwrap()).collect();
    format!("-----BEGIN {label}-----\n{}\n-----END {label}-----\n", lines.join("\n"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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
