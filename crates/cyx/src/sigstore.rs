//! Keyless signing (docs/registry.md, "Publisher signatures"): a fresh P-256 key that lives for one signature, a
//! 10-minute Fulcio certificate binding it to the identity token, the signature logged in Rekor, and the whole put
//! together as a Sigstore bundle (v0.3). No author ever holds a key.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair as _};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::net;

pub const DEFAULT_FULCIO: &str = "https://fulcio.sigstore.dev";
pub const DEFAULT_REKOR: &str = "https://rekor.sigstore.dev";

/// Where signing happens; the environment can point them elsewhere (a private Sigstore, or tests).
pub struct Endpoints {
    pub fulcio: String,
    pub rekor: String,
    pub oidc: String,
}

impl Endpoints {
    pub fn from_env() -> Endpoints {
        let var = |name: &str, default: &str| std::env::var(name).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string());
        Endpoints {
            fulcio: var("CODEYARD_FULCIO_URL", DEFAULT_FULCIO),
            rekor: var("CODEYARD_REKOR_URL", DEFAULT_REKOR),
            oidc: var("CODEYARD_OIDC_URL", crate::oidc::DEFAULT_ISSUER),
        }
    }
}

/// Sign `artifact` with the identity in `token`: the bundle, ready to upload.
pub fn sign(artifact: &[u8], token: &str, ep: &Endpoints) -> Result<Vec<u8>, String> {
    let rng = SystemRandom::new();
    let doc = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).map_err(|_| "error: cannot make a signing key")?;
    let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, doc.as_ref(), &rng).map_err(|_| "error: cannot make a signing key")?;
    let cert = certificate(&key, token, &rng, ep)?;
    let signature = key.sign(&rng, artifact).map_err(|_| "error: cannot sign")?.as_ref().to_vec();
    let hash = Sha256::digest(artifact);
    let entry = log(&hash, &signature, &cert, ep)?;
    bundle(&cert, &signature, &hash, &entry)
}

/// Fulcio v2: the public key, proof that this key signed the token's subject, and the token.
fn certificate(key: &EcdsaKeyPair, token: &str, rng: &SystemRandom, ep: &Endpoints) -> Result<Vec<u8>, String> {
    let subject = crate::oidc::subject(token)?;
    let proof = key.sign(rng, subject.as_bytes()).map_err(|_| "error: cannot sign")?;
    let body = json!({
        "credentials": { "oidcIdentityToken": token },
        "publicKeyRequest": {
            "publicKey": { "algorithm": "ECDSA", "content": pem("PUBLIC KEY", &spki(key.public_key().as_ref())) },
            "proofOfPossession": B64.encode(proof.as_ref()),
        },
    });
    let url = format!("{}/api/v2/signingCert", ep.fulcio);
    let res = net::request("POST", &url, &[("content-type", "application/json"), ("accept", "application/json")], body.to_string().into_bytes())?;
    if !res.ok() {
        return Err(format!("error: Fulcio refused a certificate ({}): {}", res.status, res.text()));
    }
    let v: Value = serde_json::from_slice(&res.body).map_err(|e| format!("error: Fulcio's answer: {e}"))?;
    let chain = v["signedCertificateEmbeddedSct"]["chain"]["certificates"].as_array().or(v["signedCertificateDetachedSct"]["chain"]["certificates"].as_array());
    let leaf = chain.and_then(|c| c.first()).and_then(Value::as_str).ok_or("error: Fulcio's answer has no certificate")?;
    first_pem(leaf).ok_or_else(|| "error: Fulcio's certificate is not PEM".into())
}

/// What Rekor answered for our entry.
#[derive(Debug)]
pub struct Entry {
    pub body: Vec<u8>,
    pub integrated_time: u64,
    pub log_id: Vec<u8>,
    pub log_index: u64,
    pub set: Vec<u8>,
    pub proof: Value,
}

/// Rekor v1: log a `hashedrekord` of this signature and certificate.
fn log(hash: &[u8], signature: &[u8], cert: &[u8], ep: &Endpoints) -> Result<Entry, String> {
    let body = json!({
        "apiVersion": "0.0.1", "kind": "hashedrekord",
        "spec": { "data": { "hash": { "algorithm": "sha256", "value": hex(hash) } },
                  "signature": { "content": B64.encode(signature), "publicKey": { "content": B64.encode(pem("CERTIFICATE", cert)) } } },
    });
    let url = format!("{}/api/v1/log/entries", ep.rekor);
    let res = net::request("POST", &url, &[("content-type", "application/json"), ("accept", "application/json")], body.to_string().into_bytes())?;
    if !res.ok() {
        return Err(format!("error: Rekor refused the log entry ({}): {}", res.status, res.text()));
    }
    let v: Value = serde_json::from_slice(&res.body).map_err(|e| format!("error: Rekor's answer: {e}"))?;
    entry_from_rekor(&v)
}

/// Rekor answers `{ uuid: { body, integratedTime, logID, logIndex, verification } }` with hex hashes; the bundle wants
/// base64 and strings.
pub fn entry_from_rekor(v: &Value) -> Result<Entry, String> {
    let bad = |what: &str| format!("error: Rekor's answer has no valid {what}");
    let (_, e) = v.as_object().and_then(|o| o.iter().next()).ok_or_else(|| bad("entry"))?;
    let body = e["body"].as_str().and_then(|b| B64.decode(b).ok()).ok_or_else(|| bad("body"))?;
    let proof = &e["verification"]["inclusionProof"];
    let hashes = proof["hashes"].as_array().ok_or_else(|| bad("inclusion proof"))?;
    let hashes: Vec<String> =
        hashes.iter().map(|h| h.as_str().and_then(unhex).map(|b| B64.encode(b))).collect::<Option<_>>().ok_or_else(|| bad("proof hash"))?;
    let root = proof["rootHash"].as_str().and_then(unhex).ok_or_else(|| bad("root hash"))?;
    Ok(Entry {
        body,
        integrated_time: e["integratedTime"].as_u64().ok_or_else(|| bad("time"))?,
        log_id: e["logID"].as_str().and_then(unhex).ok_or_else(|| bad("log id"))?,
        log_index: e["logIndex"].as_u64().ok_or_else(|| bad("log index"))?,
        set: e["verification"]["signedEntryTimestamp"].as_str().and_then(|s| B64.decode(s).ok()).ok_or_else(|| bad("signed entry timestamp"))?,
        proof: json!({
            "logIndex": proof["logIndex"].as_u64().ok_or_else(|| bad("proof index"))?.to_string(),
            "rootHash": B64.encode(root),
            "treeSize": proof["treeSize"].as_u64().ok_or_else(|| bad("tree size"))?.to_string(),
            "hashes": hashes,
            "checkpoint": { "envelope": proof["checkpoint"].as_str().ok_or_else(|| bad("checkpoint"))? },
        }),
    })
}

pub fn bundle(cert: &[u8], signature: &[u8], hash: &[u8], e: &Entry) -> Result<Vec<u8>, String> {
    serde_json::to_vec(&json!({
        "mediaType": "application/vnd.dev.sigstore.bundle.v0.3+json",
        "verificationMaterial": {
            "certificate": { "rawBytes": B64.encode(cert) },
            "tlogEntries": [{
                "logIndex": e.log_index.to_string(),
                "logId": { "keyId": B64.encode(&e.log_id) },
                "kindVersion": { "kind": "hashedrekord", "version": "0.0.1" },
                "integratedTime": e.integrated_time.to_string(),
                "inclusionPromise": { "signedEntryTimestamp": B64.encode(&e.set) },
                "inclusionProof": e.proof,
                "canonicalizedBody": B64.encode(&e.body),
            }],
        },
        "messageSignature": { "messageDigest": { "algorithm": "SHA2_256", "digest": B64.encode(hash) }, "signature": B64.encode(signature) },
    }))
    .map_err(|e| e.to_string())
}

/// SubjectPublicKeyInfo of an uncompressed P-256 point.
fn spki(point: &[u8]) -> Vec<u8> {
    // SEQUENCE { SEQUENCE { ecPublicKey, prime256v1 }, BIT STRING { 0, point } }, the fixed 26-byte prefix.
    let mut out = vec![
        0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42,
        0x00,
    ];
    out.extend_from_slice(point);
    out
}

fn pem(label: &str, der: &[u8]) -> String {
    let b64 = B64.encode(der);
    let lines: Vec<&str> = b64.as_bytes().chunks(64).map(|c| std::str::from_utf8(c).unwrap_or_default()).collect();
    format!("-----BEGIN {label}-----\n{}\n-----END {label}-----\n", lines.join("\n"))
}

fn first_pem(text: &str) -> Option<Vec<u8>> {
    let start = text.find("-----BEGIN CERTIFICATE-----")? + "-----BEGIN CERTIFICATE-----".len();
    let end = start + text[start..].find("-----END CERTIFICATE-----")?;
    B64.decode(text[start..end].chars().filter(|c| !c.is_whitespace()).collect::<String>()).ok()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spki_is_what_the_verifier_reads() {
        let rng = SystemRandom::new();
        let doc = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, doc.as_ref(), &rng).unwrap();
        let der = spki(key.public_key().as_ref());
        assert_eq!(der.len(), 91);
        assert!(ext_core::publisher::Pins::from_pem(&ext_core::testing::Authority::new().fulcio_pem(), &pem("PUBLIC KEY", &der)).is_ok());
    }

    #[test]
    fn rekor_answers_convert_and_bad_ones_are_named() {
        let a = ext_core::testing::Authority::new();
        let v = a.rekor_response(b"{}", 1_790_000_000);
        let e = entry_from_rekor(&v).unwrap();
        assert_eq!((e.integrated_time, e.log_index, e.body.as_slice()), (1_790_000_000, 0, &b"{}"[..]));
        assert_eq!(e.log_id.len(), 32);
        let mut broken = v.clone();
        let k = broken.as_object().unwrap().keys().next().unwrap().clone();
        broken[&k]["verification"]["inclusionProof"]["rootHash"] = json!("zz");
        assert!(entry_from_rekor(&broken).unwrap_err().contains("root hash"));
        assert!(entry_from_rekor(&json!({})).is_err());
    }
}
