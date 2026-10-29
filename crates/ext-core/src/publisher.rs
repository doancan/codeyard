//! Publisher signatures (docs/plugin-architecture.md §6.3, §8): a Sigstore bundle proves that the publisher's CI (or
//! the author's browser login) signed a package, checked offline against the Fulcio CA and Rekor key that the
//! registry's root pins. A compromised registry cannot forge one: it would need a certificate for the publisher's
//! identity, which only the publisher's OIDC token gets, and the signing is logged in public.
//!
//! What is signed is a statement, not the package: `codeyard-yard/v1/package\n<id> <version> <content digest>\n`, so a
//! signature for one extension or version can never be presented for another.
//!
//! Verification follows the Sigstore client spec for message signatures logged in Rekor v1 (bundle v0.1 to v0.3):
//! 1. the certificate chains to a pinned Fulcio CA, for code signing, at the time Rekor logged the entry;
//! 2. the signed entry timestamp is Rekor's signature over that entry and time;
//! 3. the inclusion proof leads to its root hash and to a checkpoint Rekor signed (both required from bundle v0.2;
//!    v0.1 bundles may lack either, and the signed entry timestamp promises inclusion);
//! 4. the logged entry has this certificate, this signature and the statement's hash;
//! 5. the signature is the certificate key's over the statement.
//!
//! Rekor v2 entries carry no time of their own (a timestamp authority provides it); public-good Sigstore keeps v1 as
//! its default log, and a freeze is announced a year ahead, so v2 is added when that happens. SCTs are not checked:
//! the CA is pinned, so they would add a second log, not a second trust decision.

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use ext_manifest::{ExtId, Version};
use ext_pkg::ContentDigest;
use ring::signature::{ECDSA_P256_SHA256_ASN1, ECDSA_P384_SHA384_ASN1, UnparsedPublicKey, VerificationAlgorithm};
use rustls_pki_types::{CertificateDer, UnixTime};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::der;
use crate::yard::Publisher;

pub const MAX_BUNDLE_BYTES: usize = 1 << 20;
/// Clock difference allowed between Rekor and this machine.
const SKEW_SECS: u64 = 300;
const GITHUB_ACTIONS: &str = "https://token.actions.githubusercontent.com";
/// Extended key usage "code signing", which Fulcio puts in every certificate.
const EKU_CODE_SIGNING: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x03];
/// Fulcio's certificate extensions are `1.3.6.1.4.1.57264.1.<n>`.
const FULCIO_OID: &[u8] = &[0x2b, 0x06, 0x01, 0x04, 0x01, 0x83, 0xbf, 0x30, 0x01];
const EXT_ISSUER_V1: u8 = 1;
const EXT_ISSUER: u8 = 8;
const EXT_BUILD_SIGNER_URI: u8 = 9;
const EXT_SOURCE_REPOSITORY_ID: u8 = 15;
const EXT_SOURCE_REPOSITORY_OWNER_ID: u8 = 17;

const MEDIA_TYPES: &[&str] = &[
    "application/vnd.dev.sigstore.bundle+json;version=0.1",
    "application/vnd.dev.sigstore.bundle+json;version=0.2",
    "application/vnd.dev.sigstore.bundle+json;version=0.3",
    "application/vnd.dev.sigstore.bundle.v0.3+json",
];

/// The bytes a publisher signs for one package.
pub fn statement(id: &ExtId, version: &Version, digest: &ContentDigest) -> Vec<u8> {
    format!("codeyard-yard/v1/package\n{id} {version} {}\n", digest.hex()).into_bytes()
}

/// The Fulcio CA certificates and Rekor keys a registry's root pins.
#[derive(Clone, Debug)]
pub struct Pins {
    fulcio: Vec<Vec<u8>>,
    rekor: Vec<RekorKey>,
}

#[derive(Clone, Debug)]
struct RekorKey {
    /// sha256 of the key's SubjectPublicKeyInfo: the log id in entries and the start of the checkpoint key hint.
    id: [u8; 32],
    curve: &'static [u8],
    point: Vec<u8>,
}

impl Pins {
    /// From the PEM text in `root.json`: one or more `CERTIFICATE` blocks for Fulcio, one or more `PUBLIC KEY` blocks
    /// for Rekor.
    pub fn from_pem(fulcio: &str, rekor: &str) -> Result<Pins, Error> {
        let bad = |what: &str| Error::Pins(what.to_string());
        let fulcio = pem_blocks(fulcio, "CERTIFICATE").ok_or_else(|| bad("the Fulcio CA is not PEM"))?;
        for cert in &fulcio {
            webpki::anchor_from_trusted_cert(&CertificateDer::from(cert.as_slice())).map_err(|e| bad(&format!("a Fulcio CA certificate: {e}")))?;
        }
        let mut keys = Vec::new();
        for spki in pem_blocks(rekor, "PUBLIC KEY").ok_or_else(|| bad("the Rekor key is not PEM"))? {
            let (curve, point) = der::ec_point(&spki).ok_or_else(|| bad("the Rekor key is not an EC public key"))?;
            let curve = [der::OID_P256, der::OID_P384].into_iter().find(|c| *c == curve).ok_or_else(|| bad("the Rekor key is not P-256 or P-384"))?;
            keys.push(RekorKey { id: Sha256::digest(&spki).into(), curve, point: point.to_vec() });
        }
        if fulcio.is_empty() || keys.is_empty() {
            return Err(bad("a Fulcio CA certificate and a Rekor key are both needed"));
        }
        Ok(Pins { fulcio, rekor: keys })
    }
}

/// A verified signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signed {
    pub publisher: Publisher,
    /// When Rekor logged it (Unix seconds).
    pub logged: u64,
    pub log_index: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The pinned material in the registry's root is unusable.
    Pins(String),
    /// Not a bundle this version reads.
    Bundle(String),
    /// The certificate does not chain to the pinned CA at the logged time, or lacks a publisher identity.
    Certificate(String),
    /// The transparency log evidence is missing or does not hold.
    Log(String),
    /// The signature is not over this package.
    Signature(String),
}

impl Error {
    /// The registry's range of the `CY` catalogue (docs/registry.md).
    pub fn code(&self) -> &'static str {
        match self {
            Error::Pins(_) => "CY0810",
            Error::Bundle(_) => "CY0811",
            Error::Certificate(_) => "CY0812",
            Error::Log(_) => "CY0813",
            Error::Signature(_) => "CY0814",
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (what, fix) = match self {
            Error::Pins(m) => (format!("the registry's pinned signing roots: {m}"), "update Codeyard; the registry's root is not usable"),
            Error::Bundle(m) => (format!("the publisher signature: {m}"), "nothing was installed; the publisher should publish again with a current cyx"),
            Error::Certificate(m) => (format!("the publisher certificate: {m}"), "nothing was installed; the package was not signed by a known publisher"),
            Error::Log(m) => (format!("the transparency log record: {m}"), "nothing was installed; the signature was not logged as it claims"),
            Error::Signature(m) => (format!("the publisher signature: {m}"), "nothing was installed; the package is not the one the publisher signed"),
        };
        write!(f, "error[{}]: {what}\n   = fix: {fix}", self.code())
    }
}

impl std::error::Error for Error {}

/// Check a bundle for `id`'s `version` with this content digest.
pub fn verify_package(bundle: &[u8], id: &ExtId, version: &Version, digest: &ContentDigest, pins: &Pins, now: u64) -> Result<Signed, Error> {
    verify(bundle, &statement(id, version, digest), pins, now)
}

/// Check a bundle over `artifact` against the pins, at `now` (Unix seconds).
pub fn verify(bundle: &[u8], artifact: &[u8], pins: &Pins, now: u64) -> Result<Signed, Error> {
    let bundle_err = |m: &str| Error::Bundle(m.to_string());
    if bundle.len() > MAX_BUNDLE_BYTES {
        return Err(bundle_err("the bundle is larger than 1 MiB"));
    }
    let b: Bundle = serde_json::from_slice(bundle).map_err(|e| Error::Bundle(format!("not valid JSON: {e}")))?;
    let version = MEDIA_TYPES.iter().position(|m| *m == b.media_type).ok_or_else(|| Error::Bundle(format!("unknown bundle format “{}”", b.media_type)))?;
    let sig = b.message_signature.as_ref().ok_or_else(|| bundle_err("only message signatures are accepted (no DSSE envelopes)"))?;
    let signature = b64(&sig.signature).ok_or_else(|| bundle_err("the signature is not base64"))?;
    let m = &b.verification_material;
    // v0.3 carries the signing certificate alone; v0.1 and v0.2 a chain. A bundle must have the shape of the version
    // it names, so the same signature cannot be passed off under another format.
    let v3 = version >= 2;
    let leaf = match (&m.certificate, &m.x509_certificate_chain) {
        (Some(c), None) if v3 => b64(&c.raw_bytes),
        (Some(_), None) => return Err(bundle_err("a single certificate is a v0.3 field, but the bundle says it is older")),
        (None, Some(_)) if v3 => return Err(bundle_err("a v0.3 bundle carries the signing certificate, not a chain")),
        // Only the signing certificate counts, and a CA in the chain means a confused signer.
        (None, Some(chain)) if chain.certificates.len() == 1 => b64(&chain.certificates[0].raw_bytes),
        (None, Some(chain)) if chain.certificates.is_empty() => return Err(bundle_err("the certificate chain is empty")),
        (None, Some(_)) => return Err(bundle_err("the certificate chain holds more than the signing certificate")),
        _ => return Err(bundle_err("a bundle has exactly one signing certificate")),
    }
    .ok_or_else(|| bundle_err("the certificate is not base64"))?;
    let [entry] = m.tlog_entries.as_slice() else { return Err(bundle_err("a bundle has exactly one transparency log entry")) };

    // 2. Rekor's promise: which key, which entry, which time.
    let log = |m: &str| Error::Log(m.to_string());
    let key_id = b64(&entry.log_id.key_id).ok_or_else(|| log("the log id is not base64"))?;
    let key = pins.rekor.iter().find(|k| k.id[..] == key_id[..]).ok_or_else(|| log("the entry is from a log the registry does not pin"))?;
    let log_index: u64 = entry.log_index.parse().map_err(|_| log("the log index is not a whole number"))?;
    let logged: u64 = entry.integrated_time.parse().map_err(|_| log("the logged time is not a whole number"))?;
    if logged > now + SKEW_SECS {
        return Err(log("the entry was logged in the future"));
    }
    let set = entry.inclusion_promise.as_ref().ok_or_else(|| log("there is no signed entry timestamp"))?;
    let set_sig = b64(&set.signed_entry_timestamp).ok_or_else(|| log("the signed entry timestamp is not base64"))?;
    let payload = serde_json::to_vec(&SetPayload { body: &entry.canonicalized_body, integrated_time: logged, log_id: hex(&key.id), log_index })
        .map_err(|e| Error::Log(e.to_string()))?;
    if !key.verifies(&payload, &set_sig) {
        return Err(log("the signed entry timestamp does not verify with the pinned Rekor key"));
    }
    let body = b64(&entry.canonicalized_body).ok_or_else(|| log("the entry is not base64"))?;

    // 3. Inclusion in a checkpoint Rekor signed.
    match &entry.inclusion_proof {
        Some(proof) if proof.checkpoint.is_none() && version >= 1 => return Err(log("the inclusion proof has no checkpoint (required from bundle v0.2)")),
        Some(proof) => proof.check(&body, key)?,
        None if version >= 1 => return Err(log("there is no inclusion proof (required from bundle v0.2)")),
        None => {}
    }

    // 1. The certificate, at the time it was logged.
    let anchors: Vec<_> =
        pins.fulcio.iter().filter_map(|c| webpki::anchor_from_trusted_cert(&CertificateDer::from(c.as_slice())).ok().map(|a| a.to_owned())).collect();
    let cert_der = CertificateDer::from(leaf.as_slice());
    let ee = webpki::EndEntityCert::try_from(&cert_der).map_err(|e| Error::Certificate(format!("not a certificate: {e}")))?;
    ee.verify_for_usage(
        ALGORITHMS,
        &anchors,
        &[],
        UnixTime::since_unix_epoch(Duration::from_secs(logged)),
        webpki::KeyUsage::required(EKU_CODE_SIGNING),
        None,
        None,
    )
    .map_err(|e| Error::Certificate(format!("does not chain to the pinned Fulcio CA when it was logged: {e:?}")))?;

    // 4. The entry is about this certificate, signature and artifact.
    let artifact_hash = Sha256::digest(artifact);
    let rekord: Rekord = serde_json::from_slice(&body).map_err(|e| Error::Log(format!("the entry is not a hashedrekord: {e}")))?;
    if rekord.kind != "hashedrekord"
        || rekord.api_version != "0.0.1"
        || (entry.kind_version.kind.as_str(), entry.kind_version.version.as_str()) != ("hashedrekord", "0.0.1")
    {
        return Err(log("the entry is not a hashedrekord 0.0.1"));
    }
    let spec = &rekord.spec;
    if spec.data.hash.algorithm != "sha256" || spec.data.hash.value != hex(&artifact_hash) {
        return Err(Error::Signature("the logged entry is for another artifact".into()));
    }
    if b64(&spec.signature.content).as_deref() != Some(signature.as_slice()) {
        return Err(Error::Signature("the logged entry has another signature".into()));
    }
    let logged_cert = b64(&spec.signature.public_key.content).and_then(|pem| pem_blocks(&String::from_utf8_lossy(&pem), "CERTIFICATE"));
    if logged_cert.as_deref() != Some(std::slice::from_ref(&leaf)) {
        return Err(Error::Signature("the logged entry has another certificate".into()));
    }
    let d = &sig.message_digest;
    if d.algorithm != "SHA2_256" || b64(&d.digest).as_deref() != Some(&artifact_hash[..]) {
        return Err(Error::Signature("the message digest is not this artifact's".into()));
    }

    // 5. The signature itself.
    let spki = der::spki_of_cert(&leaf).ok_or_else(|| Error::Certificate("no public key".into()))?;
    let (curve, point) = der::ec_point(spki).ok_or_else(|| Error::Certificate("the key is not an EC key".into()))?;
    let alg = ecdsa(curve).ok_or_else(|| Error::Certificate("the key's curve is not P-256 or P-384".into()))?;
    if UnparsedPublicKey::new(alg, point).verify(artifact, &signature).is_err() {
        return Err(Error::Signature("the signature does not verify with the certificate's key".into()));
    }

    Ok(Signed { publisher: identity(&leaf)?, logged, log_index })
}

/// What a signed identity means for an extension the user has: the pin is the identity of its first registry install.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pin {
    /// Not pinned yet (new, or sideloaded): the consent sheet shows the identity.
    New,
    Same,
    /// Another identity publishes this id now: a new trust decision, shown with the full consent sheet.
    Changed {
        was: Publisher,
    },
}

pub fn check_pin(pinned: Option<&Publisher>, signed: &Publisher) -> Pin {
    match pinned {
        None => Pin::New,
        Some(p) if p == signed => Pin::Same,
        Some(p) => Pin::Changed { was: p.clone() },
    }
}

/// The publisher identity Fulcio wrote into the certificate.
fn identity(cert: &[u8]) -> Result<Publisher, Error> {
    let bad = |m: &str| Error::Certificate(m.to_string());
    let exts = der::extensions(cert).ok_or_else(|| bad("its extensions are not readable"))?;
    let fulcio =
        |n: u8| exts.iter().find(|(oid, _)| oid.len() == FULCIO_OID.len() + 1 && oid.starts_with(FULCIO_OID) && oid[FULCIO_OID.len()] == n).map(|e| e.1);
    // Version 2 extensions hold a DER UTF8String; the old issuer extension holds the bare text.
    let text = |n: u8| fulcio(n).and_then(|v| der::only(v, der::UTF8_STRING)).and_then(|t| String::from_utf8(t.value.to_vec()).ok());
    let issuer =
        text(EXT_ISSUER).or_else(|| fulcio(EXT_ISSUER_V1).and_then(|v| String::from_utf8(v.to_vec()).ok())).ok_or_else(|| bad("it names no OIDC issuer"))?;
    if issuer == GITHUB_ACTIONS {
        let number = |n: u8, what: &str| text(n).and_then(|s| s.parse::<u64>().ok()).ok_or_else(|| bad(&format!("it has no {what}")));
        let signer = text(EXT_BUILD_SIGNER_URI).ok_or_else(|| bad("it has no workflow"))?;
        return Ok(Publisher {
            issuer,
            subject: None,
            owner_id: Some(number(EXT_SOURCE_REPOSITORY_OWNER_ID, "repository owner id")?),
            repository_id: Some(number(EXT_SOURCE_REPOSITORY_ID, "repository id")?),
            workflow: Some(workflow_path(&signer).ok_or_else(|| bad("its workflow is not a file in a GitHub repository"))?),
        });
    }
    let san = exts.iter().find(|(oid, _)| *oid == der::OID_SUBJECT_ALT_NAME).ok_or_else(|| bad("it has no subject"))?.1;
    let names = der::children(der::only(san, der::SEQUENCE).ok_or_else(|| bad("its subject is not readable"))?.value)
        .ok_or_else(|| bad("its subject is not readable"))?;
    let subject = names
        .iter()
        .find(|n| n.tag == der::SAN_EMAIL || n.tag == der::SAN_URI)
        .and_then(|n| String::from_utf8(n.value.to_vec()).ok())
        .ok_or_else(|| bad("it has no email or URI subject"))?;
    Ok(Publisher { issuer, subject: Some(subject), owner_id: None, repository_id: None, workflow: None })
}

/// `https://github.com/OWNER/REPO/.github/workflows/publish.yml@refs/tags/v1` → `.github/workflows/publish.yml`: the
/// repository is pinned by id, and the ref changes with every release.
fn workflow_path(uri: &str) -> Option<String> {
    let path = uri.strip_prefix("https://github.com/")?.split('@').next()?;
    let mut parts = path.splitn(3, '/');
    let (_, _, file) = (parts.next()?, parts.next()?, parts.next()?);
    (file.starts_with(".github/workflows/") && !file.contains("..")).then(|| file.to_string())
}

const ALGORITHMS: &[&dyn rustls_pki_types::SignatureVerificationAlgorithm] = &[
    webpki::ring::ECDSA_P256_SHA256,
    webpki::ring::ECDSA_P256_SHA384,
    webpki::ring::ECDSA_P384_SHA256,
    webpki::ring::ECDSA_P384_SHA384,
    webpki::ring::ED25519,
];

fn ecdsa(curve: &[u8]) -> Option<&'static dyn VerificationAlgorithm> {
    match curve {
        der::OID_P256 => Some(&ECDSA_P256_SHA256_ASN1),
        der::OID_P384 => Some(&ECDSA_P384_SHA384_ASN1),
        _ => None,
    }
}

impl RekorKey {
    fn verifies(&self, message: &[u8], sig: &[u8]) -> bool {
        ecdsa(self.curve).is_some_and(|alg| UnparsedPublicKey::new(alg, &self.point).verify(message, sig).is_ok())
    }
}

impl InclusionProof {
    /// RFC 9162 §2.1.3.2, then the checkpoint: Rekor's signed note naming the same tree size and root.
    fn check(&self, body: &[u8], key: &RekorKey) -> Result<(), Error> {
        let log = |m: &str| Error::Log(m.to_string());
        let index: u64 = self.log_index.parse().map_err(|_| log("the proof's index is not a whole number"))?;
        let size: u64 = self.tree_size.parse().map_err(|_| log("the tree size is not a whole number"))?;
        let root = b64(&self.root_hash).filter(|r| r.len() == 32).ok_or_else(|| log("the root hash is not 32 bytes"))?;
        let hashes =
            self.hashes.iter().map(|h| b64(h).filter(|h| h.len() == 32)).collect::<Option<Vec<_>>>().ok_or_else(|| log("a proof hash is not 32 bytes"))?;
        let leaf = node(&[0], body, &[]);
        if index >= size {
            return Err(log("the proof's index is outside the tree"));
        }
        let (mut fnode, mut snode, mut r) = (index, size - 1, leaf);
        for p in &hashes {
            if snode == 0 {
                return Err(log("the inclusion proof is too long"));
            }
            if fnode & 1 == 1 || fnode == snode {
                r = node(&[1], p, &r);
                while fnode & 1 == 0 && fnode != 0 {
                    fnode >>= 1;
                    snode >>= 1;
                }
            } else {
                r = node(&[1], &r, p);
            }
            fnode >>= 1;
            snode >>= 1;
        }
        if snode != 0 || r[..] != root[..] {
            return Err(log("the inclusion proof does not lead to its root hash"));
        }
        match &self.checkpoint {
            Some(c) => c.check(size, &root, key),
            None => Ok(()),
        }
    }
}

fn node(prefix: &[u8], a: &[u8], b: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(prefix);
    h.update(a);
    h.update(b);
    h.finalize().into()
}

impl Checkpoint {
    fn check(&self, size: u64, root: &[u8], key: &RekorKey) -> Result<(), Error> {
        let log = |m: &str| Error::Log(m.to_string());
        let split = self.envelope.find("\n\n").ok_or_else(|| log("the checkpoint is not a signed note"))?;
        let (text, sigs) = (&self.envelope[..split + 1], &self.envelope[split + 2..]);
        let lines: Vec<&str> = text.lines().collect();
        let [origin, tree_size, tree_root, ..] = lines.as_slice() else { return Err(log("the checkpoint lacks its origin, size or root")) };
        if tree_size.parse::<u64>().ok() != Some(size) || b64(tree_root).as_deref() != Some(root) {
            return Err(log("the checkpoint is for another tree"));
        }
        let name = origin.split(' ').next().unwrap_or_default();
        let signed = sigs.lines().filter_map(|l| l.strip_prefix("\u{2014} ")).any(|l| {
            let mut parts = l.splitn(2, ' ');
            let (signer, sig) = (parts.next().unwrap_or_default(), parts.next().and_then(b64).unwrap_or_default());
            signer == name && sig.len() > 4 && sig[..4] == key.id[..4] && key.verifies(text.as_bytes(), &sig[4..])
        });
        if !signed {
            return Err(log("the checkpoint is not signed by the pinned Rekor key"));
        }
        Ok(())
    }
}

/// PEM blocks with this label, decoded; `None` when a block is not base64.
fn pem_blocks(text: &str, label: &str) -> Option<Vec<Vec<u8>>> {
    let (begin, end) = (format!("-----BEGIN {label}-----"), format!("-----END {label}-----"));
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(&begin) {
        let after = &rest[start + begin.len()..];
        let stop = after.find(&end)?;
        let body: String = after[..stop].chars().filter(|c| !c.is_whitespace()).collect();
        out.push(B64.decode(body).ok()?);
        rest = &after[stop + end.len()..];
    }
    Some(out)
}

fn b64(s: &str) -> Option<Vec<u8>> {
    B64.decode(s).ok()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The signed entry timestamp's payload: canonical JSON, keys in this (sorted) order, no spaces.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SetPayload<'a> {
    body: &'a str,
    integrated_time: u64,
    #[serde(rename = "logID")]
    log_id: String,
    log_index: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Bundle {
    media_type: String,
    verification_material: Material,
    message_signature: Option<MessageSignature>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Material {
    certificate: Option<Raw>,
    x509_certificate_chain: Option<Chain>,
    #[serde(default)]
    tlog_entries: Vec<TlogEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Raw {
    raw_bytes: String,
}

#[derive(Deserialize)]
struct Chain {
    #[serde(default)]
    certificates: Vec<Raw>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessageSignature {
    /// Redundant with the signature, but every Sigstore client writes it; requiring it means nothing in a bundle goes
    /// unchecked.
    message_digest: MessageDigest,
    signature: String,
}

#[derive(Deserialize)]
struct MessageDigest {
    algorithm: String,
    digest: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TlogEntry {
    log_index: String,
    log_id: LogId,
    kind_version: KindVersion,
    integrated_time: String,
    inclusion_promise: Option<Promise>,
    inclusion_proof: Option<InclusionProof>,
    canonicalized_body: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LogId {
    key_id: String,
}

#[derive(Deserialize)]
struct KindVersion {
    kind: String,
    version: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Promise {
    signed_entry_timestamp: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InclusionProof {
    log_index: String,
    root_hash: String,
    tree_size: String,
    #[serde(default)]
    hashes: Vec<String>,
    /// Required from bundle v0.2.
    checkpoint: Option<Checkpoint>,
}

#[derive(Deserialize)]
struct Checkpoint {
    envelope: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rekord {
    api_version: String,
    kind: String,
    spec: RekordSpec,
}

#[derive(Deserialize)]
struct RekordSpec {
    data: RekordData,
    signature: RekordSignature,
}

#[derive(Deserialize)]
struct RekordData {
    hash: RekordHash,
}

#[derive(Deserialize)]
struct RekordHash {
    algorithm: String,
    value: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RekordSignature {
    content: String,
    public_key: RekordKey,
}

#[derive(Deserialize)]
struct RekordKey {
    content: String,
}

#[cfg(test)]
#[path = "publisher_tests.rs"]
mod tests;
