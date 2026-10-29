//! The registry ("Yard", docs/plugin-architecture.md §8): the signed index and its offline verification. No network:
//! the caller fetches the four files and keeps what was last accepted.
//!
//! ```text
//! root.json       registry name, online key and its expiry, the pinned Fulcio CA and Rekor key
//! root.json.sig   signatures by the offline root keys (a threshold of them, 2 of 3 for the public registry)
//! index.json      every extension and version with its digests and status; expires after 72 h
//! index.json.sig  signature by the online key
//! ```
//!
//! The signed bytes are the files exactly as served, after a context line that says which file it is, so there is no
//! canonical JSON to agree on and a signature over one file can never pass for the other. The root keys are not in
//! any file: the app ships them, and `extensions.registries` adds another registry with its own.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use ext_manifest::{ExtId, Version};
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair as _, UnparsedPublicKey};
use serde::{Deserialize, Serialize};

/// The file format both files carry; another one is refused with "update Codeyard". Fields added later within a
/// format are ignored by older clients, so only a change that old clients would misread raises it.
pub const FORMAT: u32 = 1;
pub const ROOT_FILE: &str = "root.json";
pub const INDEX_FILE: &str = "index.json";
/// Appended to a file name for its signatures.
pub const SIG_SUFFIX: &str = ".sig";
pub const MAX_ROOT_BYTES: usize = 64 << 10;
/// About 200 000 versions; the compressed transfer is a fraction of it. Index deltas come when it passes 5 MB (§13).
pub const MAX_INDEX_BYTES: usize = 64 << 20;
pub const MAX_SIG_BYTES: usize = 16 << 10;
/// How long an index is good for after it is signed (the registry re-signs daily).
pub const INDEX_LIFETIME_SECS: u64 = 72 * 3600;

/// Which file a signature is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Root,
    Index,
}

impl Role {
    pub fn file(self) -> &'static str {
        match self {
            Role::Root => ROOT_FILE,
            Role::Index => INDEX_FILE,
        }
    }

    /// The bytes that are signed: a context line, then the file.
    pub fn message(self, bytes: &[u8]) -> Vec<u8> {
        let mut m = format!("codeyard-yard/v{FORMAT}/{}\n", self.file()).into_bytes();
        m.extend_from_slice(bytes);
        m
    }
}

/// An Ed25519 public key, written as 64 lowercase hex digits.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PublicKey(pub [u8; 32]);

impl PublicKey {
    pub fn parse(s: &str) -> Option<PublicKey> {
        parse_hex::<32>(s).map(PublicKey)
    }

    pub fn hex(&self) -> String {
        hex(&self.0)
    }

    fn verifies(&self, role: Role, bytes: &[u8], sig: &[u8; 64]) -> bool {
        UnparsedPublicKey::new(&ED25519, &self.0).verify(&role.message(bytes), sig).is_ok()
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({})", self.hex())
    }
}

impl Serialize for PublicKey {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.hex())
    }
}

impl<'de> Deserialize<'de> for PublicKey {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        PublicKey::parse(&s).ok_or_else(|| serde::de::Error::custom("a key is 64 lowercase hex digits"))
    }
}

/// The keys a client trusts for one registry's root: the app ships them for the public registry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootKeys {
    pub keys: Vec<PublicKey>,
    /// How many different keys must sign `root.json`.
    pub threshold: usize,
}

/// One signature in a `.sig` file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signature {
    pub key: PublicKey,
    /// 128 lowercase hex digits.
    pub sig: String,
}

/// A `.sig` file. Root holders each add theirs to the same file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signatures {
    pub signatures: Vec<Signature>,
}

/// `root.json`: signed by the root keys, it names the key that signs the index.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Root {
    pub format: u32,
    /// Never goes back; a new online key comes with a higher version.
    pub version: u64,
    /// The registry's name, also in its index ("yard.codeyard.dev").
    pub registry: String,
    pub online: OnlineKey,
    /// Pinned for publisher signatures (PEM): the Fulcio CA certificate and the Rekor public key.
    pub fulcio: String,
    pub rekor: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnlineKey {
    pub key: PublicKey,
    /// Unix seconds; 90 days after the root signs it.
    pub expires: u64,
}

/// `index.json`: every extension in the registry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    pub format: u32,
    pub registry: String,
    /// Higher on every signing; a client never accepts a lower one than it has seen.
    pub version: u64,
    /// Unix seconds.
    pub expires: u64,
    pub extensions: BTreeMap<ExtId, Entry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub categories: Vec<String>,
    pub publisher: Publisher,
    /// Oldest first.
    pub versions: Vec<Release>,
}

/// Who may publish an id: the identity in the publisher's signing certificate (docs/plugin-architecture.md §6.3).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Publisher {
    /// The OIDC issuer, "https://token.actions.githubusercontent.com" for GitHub Actions.
    pub issuer: String,
    /// Numeric ids survive renames, unlike `owner/repo`.
    pub owner_id: u64,
    /// `None` for a browser login, which signs as the account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_id: Option<u64>,
    /// The workflow file and ref, `owner/repo/.github/workflows/publish.yml@refs/tags/v1.2.0` without the tag part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub version: Version,
    /// Content digest (sha256 of the canonical tar), as in the lock.
    pub digest: String,
    /// sha256 of the `.cyx` file as downloaded.
    pub sha256: String,
    pub size: u64,
    /// sha256 of the manifest's permission set, so an update can tell "same permissions" without downloading.
    pub permissions: String,
    /// Runs code: updates wait 48 h.
    #[serde(default)]
    pub code: bool,
    /// Unix seconds.
    pub published: u64,
    #[serde(default)]
    pub status: Status,
    /// Downloads including updates, counted per day without logs.
    #[serde(default)]
    pub downloads: u64,
}

impl Release {
    /// A pre-release is a version with a suffix (`1.2.0-beta.1`); users get it only when they opt in per extension.
    pub fn is_pre_release(&self) -> bool {
        self.version.pre.is_some()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    #[default]
    Ok,
    Deprecated {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replacement: Option<ExtId>,
    },
    /// Hidden from installs; users on it go back to the previous version.
    Yanked,
    /// Stops at once, with its dependents.
    Revoked { reason: String },
}

/// What a client last accepted from a registry, so an older copy is refused.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    pub root: u64,
    pub index: u64,
}

/// An index that passed every check.
#[derive(Clone, Debug)]
pub struct Verified {
    pub root: Root,
    pub index: Index,
    /// Past its expiry: installed extensions keep running, but installs and updates wait for the user (§8).
    pub stale: bool,
}

impl Verified {
    pub fn seen(&self) -> Seen {
        Seen { root: self.root.version, index: self.index.version }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// A file is larger than its limit.
    TooLarge { file: &'static str, limit: usize },
    /// Not valid JSON, or a field is missing, unknown or out of range.
    Malformed { file: &'static str, reason: String },
    /// Written in a newer format.
    Format { file: &'static str, found: u32 },
    /// Not signed by enough trusted keys.
    Signature { file: &'static str, valid: usize, needed: usize },
    /// The online key is past its expiry.
    KeyExpired { expired: u64 },
    /// An older copy than one already accepted.
    Rollback { file: &'static str, seen: u64, found: u64 },
    /// The index names another registry than its root.
    WrongRegistry { root: String, index: String },
}

impl Error {
    /// Stable codes in the `CY` catalogue (docs/extensions/developer-guide.md); the 08xx range is the registry's.
    pub fn code(&self) -> &'static str {
        match self {
            Error::TooLarge { .. } => "CY0801",
            Error::Malformed { .. } => "CY0802",
            Error::Format { .. } => "CY0803",
            Error::Signature { .. } => "CY0804",
            Error::KeyExpired { .. } => "CY0805",
            Error::Rollback { .. } => "CY0806",
            Error::WrongRegistry { .. } => "CY0807",
        }
    }

    pub fn fix(&self) -> &'static str {
        match self {
            Error::TooLarge { .. } | Error::Malformed { .. } | Error::Signature { .. } => {
                "nothing was installed; the registry's files are damaged or were changed on the way, so try again later"
            }
            Error::Format { .. } => "update Codeyard to read this registry",
            Error::KeyExpired { .. } => "the registry has not renewed its signing key; try again later or update Codeyard",
            Error::Rollback { .. } => "an older copy of the registry was served (by a cache or an attacker); try again later",
            Error::WrongRegistry { .. } => "check the registry's address in extensions.registries",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "error[{}]: ", self.code())?;
        match self {
            Error::TooLarge { file, limit } => write!(f, "{file} is larger than {limit} bytes")?,
            Error::Malformed { file, reason } => write!(f, "{file} is not valid: {reason}")?,
            Error::Format { file, found } => write!(f, "{file} is format {found}; this Codeyard reads format {FORMAT}")?,
            Error::Signature { file, valid, needed } => {
                write!(f, "{file} has {valid} valid signature(s) from trusted keys; {needed} needed")?
            }
            Error::KeyExpired { expired } => write!(f, "the registry's online key expired at {expired} (Unix time)")?,
            Error::Rollback { file, seen, found } => write!(f, "{file} is version {found}, older than version {seen} seen before")?,
            Error::WrongRegistry { root, index } => write!(f, "{INDEX_FILE} is for “{index}”, but {ROOT_FILE} is for “{root}”")?,
        }
        write!(f, "\n   = fix: {}", self.fix())
    }
}

impl std::error::Error for Error {}

/// Check a registry's four files against the root keys the client trusts, at `now` (Unix seconds), never accepting
/// an older root or index than `seen`. A stale index is returned, marked; everything else wrong is an error.
pub fn verify(keys: &RootKeys, files: [&[u8]; 4], now: u64, seen: Seen) -> Result<Verified, Error> {
    let [root_bytes, root_sig, index_bytes, index_sig] = files;
    limit(ROOT_FILE, root_bytes, MAX_ROOT_BYTES)?;
    limit(INDEX_FILE, index_bytes, MAX_INDEX_BYTES)?;
    const ROOT_SIG: &str = "root.json.sig";
    const INDEX_SIG: &str = "index.json.sig";
    limit(ROOT_SIG, root_sig, MAX_SIG_BYTES)?;
    limit(INDEX_SIG, index_sig, MAX_SIG_BYTES)?;

    let needed = keys.threshold.max(1);
    let valid = valid_signers(Role::Root, root_bytes, &parse_json(ROOT_SIG, root_sig)?, &keys.keys);
    if valid < needed || keys.keys.len() < needed {
        return Err(Error::Signature { file: ROOT_FILE, valid, needed });
    }
    let root: Root = parse_json(ROOT_FILE, root_bytes)?;
    if root.format != FORMAT {
        return Err(Error::Format { file: ROOT_FILE, found: root.format });
    }
    if root.version < seen.root {
        return Err(Error::Rollback { file: ROOT_FILE, seen: seen.root, found: root.version });
    }
    check_text(ROOT_FILE, "registry", &root.registry, 253)?;
    if now >= root.online.expires {
        return Err(Error::KeyExpired { expired: root.online.expires });
    }

    let valid = valid_signers(Role::Index, index_bytes, &parse_json(INDEX_SIG, index_sig)?, &[root.online.key]);
    if valid < 1 {
        return Err(Error::Signature { file: INDEX_FILE, valid, needed: 1 });
    }
    let index: Index = parse_json(INDEX_FILE, index_bytes)?;
    if index.format != FORMAT {
        return Err(Error::Format { file: INDEX_FILE, found: index.format });
    }
    if index.registry != root.registry {
        return Err(Error::WrongRegistry { root: root.registry, index: index.registry });
    }
    if index.version < seen.index {
        return Err(Error::Rollback { file: INDEX_FILE, seen: seen.index, found: index.version });
    }
    check_index(&index)?;
    // A signed index never outlives the key that signed it.
    let stale = now >= index.expires.min(root.online.expires);
    Ok(Verified { root, index, stale })
}

fn limit(file: &'static str, bytes: &[u8], limit: usize) -> Result<(), Error> {
    if bytes.len() > limit { Err(Error::TooLarge { file, limit }) } else { Ok(()) }
}

fn parse_json<T: serde::de::DeserializeOwned>(file: &'static str, bytes: &[u8]) -> Result<T, Error> {
    serde_json::from_slice(bytes).map_err(|e| Error::Malformed { file, reason: e.to_string() })
}

/// How many different keys of `trusted` signed the file; unknown keys and repeats do not count.
fn valid_signers(role: Role, bytes: &[u8], sigs: &Signatures, trusted: &[PublicKey]) -> usize {
    let mut ok = BTreeSet::new();
    for s in &sigs.signatures {
        if !trusted.contains(&s.key) || ok.contains(&s.key) {
            continue;
        }
        if parse_hex::<64>(&s.sig).is_some_and(|sig| s.key.verifies(role, bytes, &sig)) {
            ok.insert(s.key);
        }
    }
    ok.len()
}

fn check_text(file: &'static str, field: &str, s: &str, max: usize) -> Result<(), Error> {
    if s.is_empty() || s.len() > max || s.chars().any(char::is_control) {
        return Err(Error::Malformed { file, reason: format!("{field} is empty, longer than {max} bytes or has control characters") });
    }
    Ok(())
}

/// What the client relies on later: hashes are hashes, versions are unique, text is bounded.
fn check_index(index: &Index) -> Result<(), Error> {
    let bad = |reason: String| Error::Malformed { file: INDEX_FILE, reason };
    for (id, e) in &index.extensions {
        check_text(INDEX_FILE, &format!("{id}: name"), &e.name, 100)?;
        if e.description.len() > 1000 || e.description.chars().any(char::is_control) {
            return Err(bad(format!("{id}: description is longer than 1000 bytes or has control characters")));
        }
        if e.categories.len() > 10 || e.categories.iter().any(|c| c.is_empty() || c.len() > 40) {
            return Err(bad(format!("{id}: at most 10 categories of up to 40 bytes")));
        }
        check_text(INDEX_FILE, &format!("{id}: publisher issuer"), &e.publisher.issuer, 200)?;
        if e.versions.is_empty() {
            return Err(bad(format!("{id}: no versions")));
        }
        for pair in e.versions.windows(2) {
            if pair[0].version >= pair[1].version {
                return Err(bad(format!("{id}: versions must be unique and oldest first ({} before {})", pair[0].version, pair[1].version)));
            }
        }
        for r in &e.versions {
            for (field, value) in [("digest", &r.digest), ("sha256", &r.sha256), ("permissions", &r.permissions)] {
                if parse_hex::<32>(value).is_none() {
                    return Err(bad(format!("{id} {}: {field} is not 64 lowercase hex digits", r.version)));
                }
            }
            if let Status::Revoked { reason } = &r.status {
                check_text(INDEX_FILE, &format!("{id} {}: revoked reason", r.version), reason, 500)?;
            }
        }
    }
    Ok(())
}

impl Index {
    /// The newest version a user may install: `ok` or `deprecated`, pre-releases only when asked for.
    pub fn latest(&self, id: &ExtId, pre_release: bool) -> Option<&Release> {
        let e = self.extensions.get(id)?;
        e.versions.iter().rev().find(|r| matches!(r.status, Status::Ok | Status::Deprecated { .. }) && (pre_release || !r.is_pre_release()))
    }

    pub fn release(&self, id: &ExtId, version: &Version) -> Option<&Release> {
        self.extensions.get(id)?.versions.iter().find(|r| &r.version == version)
    }
}

/// A key pair for signing, kept in a file that only its holder can read.
pub struct SigningKey(Ed25519KeyPair);

/// Shows only the public half.
impl fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SigningKey({})", self.public().hex())
    }
}

/// The key file: the PKCS#8 document and, for people, the public key it holds.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyFile {
    #[serde(rename = "type")]
    kind: String,
    public: PublicKey,
    pkcs8: String,
}

const KEY_FILE_TYPE: &str = "codeyard-yard-key";

impl SigningKey {
    /// A new key and its file contents.
    pub fn generate() -> Result<(SigningKey, String), String> {
        let rng = ring::rand::SystemRandom::new();
        let doc = Ed25519KeyPair::generate_pkcs8(&rng).map_err(|_| "cannot generate a key".to_string())?;
        let pair = Ed25519KeyPair::from_pkcs8(doc.as_ref()).map_err(|_| "cannot read the new key".to_string())?;
        let key = SigningKey(pair);
        let file = KeyFile { kind: KEY_FILE_TYPE.into(), public: key.public(), pkcs8: hex(doc.as_ref()) };
        let text = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
        Ok((key, text + "\n"))
    }

    pub fn from_file(text: &str) -> Result<SigningKey, String> {
        let file: KeyFile = serde_json::from_str(text).map_err(|e| format!("not a key file: {e}"))?;
        if file.kind != KEY_FILE_TYPE {
            return Err(format!("not a key file: type is “{}”", file.kind));
        }
        let der = parse_hex_vec(&file.pkcs8).ok_or("not a key file: pkcs8 is not hex")?;
        let pair = Ed25519KeyPair::from_pkcs8_maybe_unchecked(&der).map_err(|_| "not a key file: pkcs8 is not an Ed25519 key".to_string())?;
        let key = SigningKey(pair);
        if key.public() != file.public {
            return Err("not a key file: the public key does not match the private key".into());
        }
        Ok(key)
    }

    pub fn public(&self) -> PublicKey {
        let mut k = [0u8; 32];
        k.copy_from_slice(self.0.public_key().as_ref());
        PublicKey(k)
    }

    pub fn sign(&self, role: Role, bytes: &[u8]) -> Signature {
        Signature { key: self.public(), sig: hex(self.0.sign(&role.message(bytes)).as_ref()) }
    }
}

/// Add `key`'s signature to a `.sig` file's text (empty for a new one), replacing an older signature by the same key.
pub fn add_signature(sig_file: &str, key: &SigningKey, role: Role, bytes: &[u8]) -> Result<String, String> {
    let mut sigs: Signatures =
        if sig_file.trim().is_empty() { Signatures::default() } else { serde_json::from_str(sig_file).map_err(|e| format!("not a signature file: {e}"))? };
    let sig = key.sign(role, bytes);
    sigs.signatures.retain(|s| s.key != sig.key);
    sigs.signatures.push(sig);
    Ok(serde_json::to_string_pretty(&sigs).map_err(|e| e.to_string())? + "\n")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn parse_hex_vec(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

fn parse_hex<const N: usize>(s: &str) -> Option<[u8; N]> {
    parse_hex_vec(s).filter(|v| v.len() == N).map(|v| v.try_into().expect("length checked"))
}

#[cfg(test)]
#[path = "yard_tests.rs"]
mod tests;
