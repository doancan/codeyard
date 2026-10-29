//! `.cyx` extension packages: write them reproducibly, read them strictly, and name them by a content digest.
//!
//! A package is a gzip-compressed POSIX ustar archive of regular files, sorted by path, with mtime 0, owner 0 and mode
//! 644 or 755. It holds `codeyard-extension.toml` and `CONTENTS.sha256` (one `sha256  path` line per other file). The
//! content digest is the SHA-256 of the canonical uncompressed tar; the store keys installs by it, the registry and the
//! lock file record it. The same reader serves the app, `cyx` and the registry, so they can never disagree about what a
//! package contains. No UI.
//!
//! Design: docs/plugin-architecture.md §4, §8.

mod tar;

#[cfg(test)]
mod fuzz_tests;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::io::Read;
use std::path::Path;

use ext_manifest::{Diagnostic, Manifest, limits};
use sha2::{Digest as _, Sha256};

pub use tar::Entry;

/// The list of file hashes inside every package.
pub const CONTENTS: &str = "CONTENTS.sha256";

/// Folder and file names `pack_dir` leaves out: version control, build output, dependencies, OS litter.
pub const EXCLUDED: &[&str] = &[".git", ".github", ".hg", ".svn", "target", "node_modules", ".DS_Store", "Thumbs.db", "desktop.ini"];

#[derive(Debug)]
pub enum Error {
    /// Not a gzip stream, or gzip around something that is not a package tar.
    Format(String),
    /// A file's path or type is not allowed in a package.
    Path {
        path: String,
        reason: String,
    },
    /// Over one of the limits in `ext_manifest::limits`.
    TooLarge(String),
    /// `CONTENTS.sha256` is missing, malformed or does not match the files.
    Contents(String),
    /// The manifest is missing or invalid.
    Manifest(Vec<Diagnostic>),
    /// The manifest refers to a file the package does not have.
    MissingFile {
        field: String,
        path: String,
    },
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Format(m) | Error::TooLarge(m) | Error::Contents(m) => f.write_str(m),
            Error::Path { path, reason } => write!(f, "'{path}' {reason}"),
            Error::Manifest(d) => {
                write!(f, "{} is invalid", ext_manifest::FILE_NAME)?;
                for e in d {
                    write!(f, "\n{e}")?;
                }
                Ok(())
            }
            Error::MissingFile { field, path } => write!(f, "{field} points to '{path}', which is not in the package"),
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

/// SHA-256 of the canonical uncompressed tar.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentDigest(pub [u8; 32]);

impl ContentDigest {
    pub fn hex(&self) -> String {
        hex(&self.0)
    }

    pub fn parse(s: &str) -> Option<ContentDigest> {
        if s.len() != 64 {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, b) in out.iter_mut().enumerate() {
            *b = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
        }
        (hex(&out) == s).then_some(ContentDigest(out))
    }
}

impl fmt::Debug for ContentDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentDigest({})", self.hex())
    }
}

impl fmt::Display for ContentDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hex())
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// A package that passed every check.
#[derive(Debug)]
pub struct Package {
    pub manifest: Manifest,
    /// Every file, `CONTENTS.sha256` and the manifest included, by path.
    pub files: BTreeMap<String, Entry>,
    pub digest: ContentDigest,
}

/// A path allowed inside a package: the manifest's portable-path rules, printable ASCII only (one spelling per name,
/// so Unicode normalisation cannot make two names one file on macOS), and at most 255 bytes (the ustar limit).
pub fn check_path(path: &str) -> Result<(), String> {
    if let Some(c) = path.chars().find(|c| !(c.is_ascii_graphic() || *c == ' ')) {
        return Err(format!("contains '{}'; package paths are printable ASCII", c.escape_default()));
    }
    if path.len() > 255 {
        return Err("is longer than 255 bytes".into());
    }
    ext_manifest::check_package_path(path)
}

/// Check paths, types, sizes, duplicates and case-insensitive collisions of a set of files, then sort them.
fn normalise(mut entries: Vec<Entry>) -> Result<Vec<Entry>, Error> {
    if entries.len() > limits::PACKAGE_FILES {
        return Err(Error::TooLarge(format!("the package has more than {} files", limits::PACKAGE_FILES)));
    }
    let mut total = 0u64;
    let mut folded: HashMap<String, &str> = HashMap::new();
    for e in &entries {
        check_path(&e.path).map_err(|reason| Error::Path { path: e.path.clone(), reason })?;
        let size = e.data.len() as u64;
        if size > limits::PACKAGE_FILE_BYTES {
            return Err(Error::TooLarge(format!("'{}' is larger than {} MiB", e.path, limits::PACKAGE_FILE_BYTES >> 20)));
        }
        total += size;
        // Two names that differ only in case are one file on macOS and Windows.
        if let Some(other) = folded.insert(e.path.to_ascii_lowercase(), &e.path) {
            return Err(Error::Path { path: e.path.clone(), reason: format!("and '{other}' are the same file on macOS and Windows") });
        }
    }
    // A file and a folder of the same name cannot both exist on disk.
    for e in &entries {
        let lower = e.path.to_ascii_lowercase();
        let mut dir = lower.as_str();
        while let Some((parent, _)) = dir.rsplit_once('/') {
            if let Some(file) = folded.get(parent) {
                return Err(Error::Path { path: e.path.clone(), reason: format!("is inside '{file}', which is a file") });
            }
            dir = parent;
        }
    }
    drop(folded);
    if total > limits::PACKAGE_UNPACKED_BYTES {
        return Err(Error::TooLarge(format!("the files add up to more than {} MiB", limits::PACKAGE_UNPACKED_BYTES >> 20)));
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

/// `CONTENTS.sha256` for `entries` (without itself): `sha256  path`, sorted by path, as `sha256sum` prints it.
fn contents_list(entries: &[Entry]) -> String {
    entries.iter().filter(|e| e.path != CONTENTS).map(|e| format!("{}  {}\n", hex(&sha256(&e.data)), e.path)).collect()
}

fn canonical_tar(entries: &[Entry]) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    tar::write(entries, &mut out)?;
    Ok(out)
}

/// Every file the manifest names must be in the package.
fn check_references(m: &Manifest, files: &BTreeMap<String, Entry>) -> Result<(), Error> {
    let c = &m.contributions;
    let mut refs: Vec<(&str, &str)> = Vec::new();
    refs.extend(c.themes.iter().map(|t| ("themes.path", t.path.as_str())));
    refs.extend(c.keymap_presets.iter().map(|t| ("keymap-preset.path", t.path.as_str())));
    refs.extend(c.snippets.iter().map(|s| ("snippets.path", s.path.as_str())));
    refs.extend(c.json_schemas.iter().map(|s| ("json-schemas.path", s.path.as_str())));
    refs.extend(c.yaml_schemas.iter().map(|s| ("yaml-schemas.path", s.path.as_str())));
    if let Some(i) = &m.icon {
        refs.push(("extension.icon", i));
    }
    for (field, path) in refs {
        if !files.contains_key(path) {
            return Err(Error::MissingFile { field: field.into(), path: path.into() });
        }
    }
    // A program inside the package (`run = ["bin/server"]`) must be there and be executable.
    for p in m.permissions.run.iter().filter(|p| p.contains('/')) {
        let p = p.trim_start_matches("./");
        match files.get(p) {
            None => return Err(Error::MissingFile { field: "permissions.run".into(), path: p.into() }),
            Some(e) if !e.executable => return Err(Error::Path { path: p.into(), reason: "is in permissions.run but not executable (mode 755)".into() }),
            Some(_) => {}
        }
    }
    Ok(())
}

/// Build a package from files (`CONTENTS.sha256` is added). The bytes depend only on the paths, contents and
/// executable bits, never on order, time or the machine.
pub fn write(entries: Vec<Entry>) -> Result<(Vec<u8>, ContentDigest), Error> {
    let mut entries: Vec<Entry> = entries.into_iter().filter(|e| e.path != CONTENTS).collect();
    let list = contents_list(&normalise(entries.clone())?);
    entries.push(Entry { path: CONTENTS.into(), data: list.into_bytes(), executable: false });
    let entries = normalise(entries)?;
    let files: BTreeMap<String, Entry> = entries.iter().map(|e| (e.path.clone(), e.clone())).collect();
    let manifest = manifest_of(&files)?;
    check_references(&manifest, &files)?;
    let tar = canonical_tar(&entries)?;
    let digest = ContentDigest(sha256(&tar));
    let mut gz = flate2::GzBuilder::new().mtime(0).operating_system(255).write(Vec::new(), flate2::Compression::best());
    std::io::Write::write_all(&mut gz, &tar).map_err(Error::Io)?;
    let bytes = gz.finish().map_err(Error::Io)?;
    if bytes.len() as u64 > limits::PACKAGE_BYTES {
        return Err(Error::TooLarge(format!("the package is larger than {} MiB compressed", limits::PACKAGE_BYTES >> 20)));
    }
    Ok((bytes, digest))
}

fn manifest_of(files: &BTreeMap<String, Entry>) -> Result<Manifest, Error> {
    let m = files
        .get(ext_manifest::FILE_NAME)
        .ok_or_else(|| Error::Manifest(vec![Diagnostic::new(ext_manifest::Code::Syntax, format!("the package has no {}", ext_manifest::FILE_NAME))]))?;
    let text = std::str::from_utf8(&m.data).map_err(|_| Error::Manifest(vec![Diagnostic::new(ext_manifest::Code::Syntax, "the manifest is not UTF-8")]))?;
    ext_manifest::parse(text).map_err(Error::Manifest)
}

/// Read and check a package. Nothing is trusted: the size is bounded before and while it is decompressed, every header
/// field is checked, and the digest is computed from the canonical form, so two archives with the same files have the
/// same digest and no archive can carry anything the digest does not cover.
pub fn read(bytes: &[u8]) -> Result<Package, Error> {
    if bytes.len() as u64 > limits::PACKAGE_BYTES {
        return Err(Error::TooLarge(format!("the package is larger than {} MiB compressed", limits::PACKAGE_BYTES >> 20)));
    }
    if !bytes.starts_with(&[0x1f, 0x8b]) {
        return Err(Error::Format("not a .cyx package (no gzip header)".into()));
    }
    // Headers and padding on top of the files: one block per file plus rounding, and the end-of-archive blocks.
    let cap = limits::PACKAGE_UNPACKED_BYTES + (limits::PACKAGE_FILES as u64 + 2) * 2 * tar::BLOCK as u64;
    let mut tar = Vec::new();
    let mut gz = flate2::bufread::GzDecoder::new(bytes);
    (&mut gz).take(cap + 1).read_to_end(&mut tar).map_err(|e| Error::Format(format!("the gzip stream is damaged: {e}")))?;
    if tar.len() as u64 > cap {
        return Err(Error::TooLarge(format!("the package unpacks to more than {} MiB", limits::PACKAGE_UNPACKED_BYTES >> 20)));
    }
    // A second gzip member or trailing bytes would be content the digest never covers. The bufread decoder hands back
    // exactly the bytes after its member.
    let rest = gz.into_inner();
    if !rest.is_empty() {
        return Err(Error::Format("there is data after the gzip stream".into()));
    }
    let mut files_seen = 0usize;
    let mut total = 0u64;
    let entries = tar::read(&tar, |path, size| {
        files_seen += 1;
        total = total.saturating_add(size);
        if files_seen > limits::PACKAGE_FILES {
            return Err(Error::TooLarge(format!("the package has more than {} files", limits::PACKAGE_FILES)));
        }
        if size > limits::PACKAGE_FILE_BYTES || total > limits::PACKAGE_UNPACKED_BYTES {
            return Err(Error::TooLarge(format!("'{path}' takes the package over its size limit")));
        }
        Ok(())
    })?;
    let entries = normalise(entries)?;
    let files: BTreeMap<String, Entry> = entries.iter().map(|e| (e.path.clone(), e.clone())).collect();
    let listed = files.get(CONTENTS).ok_or_else(|| Error::Contents(format!("the package has no {CONTENTS}")))?;
    if listed.data != contents_list(&entries).as_bytes() {
        return Err(Error::Contents(format!("{CONTENTS} does not match the files in the package")));
    }
    let manifest = manifest_of(&files)?;
    check_references(&manifest, &files)?;
    let digest = ContentDigest(sha256(&canonical_tar(&entries)?));
    Ok(Package { manifest, files, digest })
}

/// Collect an extension folder for `write`: every regular file except `EXCLUDED` names, with its executable bit.
/// Symlinks are refused rather than followed: a package must not depend on what a link points to on the author's disk.
pub fn collect_dir(root: &Path) -> Result<Vec<Entry>, Error> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<Entry>) -> Result<(), Error> {
        let mut names: Vec<_> = std::fs::read_dir(dir).map_err(Error::Io)?.collect::<Result<_, _>>().map_err(Error::Io)?;
        names.sort_by_key(|e| e.file_name());
        for item in names {
            let name = item.file_name();
            if EXCLUDED.iter().any(|x| name == *x) {
                continue;
            }
            let path = item.path();
            let rel = path.strip_prefix(root).expect("walked under root").to_string_lossy().replace('\\', "/");
            let meta = std::fs::symlink_metadata(&path).map_err(Error::Io)?;
            if meta.file_type().is_symlink() {
                return Err(Error::Path { path: rel, reason: "is a symbolic link; copy the file into the extension instead".into() });
            }
            if meta.is_dir() {
                walk(root, &path, out)?;
            } else if meta.is_file() {
                if meta.len() > limits::PACKAGE_FILE_BYTES {
                    return Err(Error::TooLarge(format!("'{rel}' is larger than {} MiB", limits::PACKAGE_FILE_BYTES >> 20)));
                }
                let data = std::fs::read(&path).map_err(Error::Io)?;
                out.push(Entry { path: rel, data, executable: is_executable(&meta) });
                if out.len() > limits::PACKAGE_FILES {
                    return Err(Error::TooLarge(format!("the folder has more than {} files", limits::PACKAGE_FILES)));
                }
            } else {
                return Err(Error::Path { path: rel, reason: "is not a regular file".into() });
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(root, root, &mut out)?;
    Ok(out)
}

#[cfg(unix)]
fn is_executable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_: &std::fs::Metadata) -> bool {
    false
}
