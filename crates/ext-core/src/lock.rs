//! `extensions.lock.jsonc`: the only record of what is installed. One entry per extension: the version, the content
//! digest of the package in the store, the digest it replaced (for rollback without network), where it came from and
//! whether it is turned off. Users may sync it with their dotfiles and read it; Codeyard writes it.

use std::collections::BTreeMap;

use ext_manifest::{ExtId, Version};
use ext_pkg::ContentDigest;
use serde::{Deserialize, Serialize};

/// The format of the lock file. A newer format is refused rather than rewritten, as the state database does. Format 2
/// adds registry sources, format 3 the pre-release channel and format 4 per-platform digests; a lock is written in the
/// lowest format that holds it, so an older Codeyard keeps reading it.
pub const FORMAT: u32 = 4;

const HEADER: &str = "// Installed extensions. Written by Codeyard; change it with the Extensions view or `codeyard ext`.\n";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Locked {
    pub version: Version,
    #[serde(with = "digest_hex")]
    pub digest: ContentDigest,
    /// The digest this one replaced, kept in the store until the next change so a rollback needs no network.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "opt_digest_hex")]
    pub previous: Option<ContentDigest>,
    pub source: Source,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
    /// The user opted into the registry's pre-releases of this extension (on its page).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pre_release: bool,
    /// An extension with native programs: the content digest of each platform's package of this version, by target
    /// triple, as the registry's index listed them (or as packaged from a folder). `digest` is one of them; another
    /// platform's package is fetched when a workspace on that platform needs it, and must match its digest here.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty", with = "targets_hex")]
    pub targets: BTreeMap<String, ContentDigest>,
}

/// Where a package came from: shown in the Extensions view and used to update it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum Source {
    /// A `.cyx` file the user installed (its path when installed).
    File { path: String },
    /// A folder packaged on install (`codeyard ext install ./folder`).
    Folder { path: String },
    /// A registry, by its name in `extensions.registries`, with the publisher identity pinned at the first install from
    /// it: a later version signed by anyone else is a new trust decision.
    Registry { registry: String, publisher: crate::yard::Publisher },
}

impl Source {
    pub fn registry(&self) -> Option<(&str, &crate::yard::Publisher)> {
        match self {
            Source::Registry { registry, publisher } => Some((registry, publisher)),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Lock {
    pub format: u32,
    pub extensions: BTreeMap<ExtId, Locked>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LockError {
    /// Written by a newer Codeyard.
    Newer(u32),
    Invalid(String),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::Newer(v) => write!(f, "extensions.lock.jsonc was written by a newer Codeyard (format {v}); update Codeyard"),
            LockError::Invalid(e) => write!(f, "extensions.lock.jsonc is not valid: {e}"),
        }
    }
}

impl Lock {
    pub fn parse(text: &str) -> Result<Lock, LockError> {
        let json = strip_comments(text);
        if json.trim().is_empty() {
            return Ok(Lock { format: FORMAT, ..Default::default() });
        }
        #[derive(Deserialize)]
        struct Head {
            format: u32,
        }
        let head: Head = serde_json::from_str(&json).map_err(|e| LockError::Invalid(e.to_string()))?;
        if head.format > FORMAT {
            return Err(LockError::Newer(head.format));
        }
        serde_json::from_str(&json).map_err(|e| LockError::Invalid(e.to_string()))
    }

    pub fn render(&self) -> String {
        let mut out = String::from(HEADER);
        let needs = if self.extensions.values().any(|l| !l.targets.is_empty()) {
            4
        } else if self.extensions.values().any(|l| l.pre_release) {
            3
        } else if self.extensions.values().any(|l| l.source.registry().is_some()) {
            2
        } else {
            1
        };
        let lock = Lock { format: needs, extensions: self.extensions.clone() };
        out.push_str(&serde_json::to_string_pretty(&lock).expect("lock serializes"));
        out.push('\n');
        out
    }

    /// Every digest the store must keep: current and previous of each extension, and the current version's other
    /// platforms.
    pub fn digests(&self) -> impl Iterator<Item = ContentDigest> + '_ {
        self.extensions.values().flat_map(|l| std::iter::once(l.digest).chain(l.previous).chain(l.targets.values().copied()))
    }
}

/// Remove `//` and `/* */` comments outside strings, keeping line breaks so error positions stay right.
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_str = false;
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            if c == '\\' {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_str = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut prev = ' ';
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                    }
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
            }
            _ => out.push(c),
        }
    }
    out
}

mod digest_hex {
    use ext_pkg::ContentDigest;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(d: &ContentDigest, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&d.hex())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<ContentDigest, D::Error> {
        let s = String::deserialize(d)?;
        ContentDigest::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("'{s}' is not a sha256 digest")))
    }
}

mod opt_digest_hex {
    use ext_pkg::ContentDigest;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(d: &Option<ContentDigest>, s: S) -> Result<S::Ok, S::Error> {
        match d {
            Some(d) => s.serialize_str(&d.hex()),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<ContentDigest>, D::Error> {
        match Option::<String>::deserialize(d)? {
            None => Ok(None),
            Some(s) => ContentDigest::parse(&s).map(Some).ok_or_else(|| serde::de::Error::custom(format!("'{s}' is not a sha256 digest"))),
        }
    }
}

mod targets_hex {
    use std::collections::BTreeMap;

    use ext_pkg::ContentDigest;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(m: &BTreeMap<String, ContentDigest>, s: S) -> Result<S::Ok, S::Error> {
        s.collect_map(m.iter().map(|(t, d)| (t, d.hex())))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<BTreeMap<String, ContentDigest>, D::Error> {
        let raw = BTreeMap::<String, String>::deserialize(d)?;
        raw.into_iter()
            .map(|(t, s)| {
                if !ext_manifest::is_target(&t) {
                    return Err(serde::de::Error::custom(format!("'{t}' is not a target triple")));
                }
                ContentDigest::parse(&s).map(|d| (t, d)).ok_or_else(|| serde::de::Error::custom(format!("'{s}' is not a sha256 digest")))
            })
            .collect()
    }
}
