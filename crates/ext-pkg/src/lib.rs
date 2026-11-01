//! `.cyx` extension packages: write them reproducibly, read them strictly, and name them by a content digest.
//!
//! A package is a gzip-compressed POSIX ustar archive of regular files, sorted by path, with mtime 0, owner 0 and mode
//! 644 or 755. It holds `codeyard-extension.toml` and `CONTENTS.sha256` (one `sha256  path` line per other file). The
//! content digest is the SHA-256 of the canonical uncompressed tar; the store keys installs by it, the registry and the
//! lock file record it. The same reader serves the app, `cyx` and the registry, so they can never disagree about what a
//! package contains. No UI.
//!
//! Design: docs/plugin-architecture.md §4, §8.

mod component;
pub mod rules;
mod tar;

#[cfg(test)]
mod component_tests;
#[cfg(test)]
mod fuzz_tests;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::io::Read;
use std::path::Path;

use ext_manifest::{Diagnostic, Manifest, limits};
use sha2::{Digest as _, Sha256};

pub use component::{COMPONENT, Component, check_component};
pub use rules::{EXCLUDED, EXCLUDED_AT_ROOT, IGNORE_FILE};
pub use tar::Entry;

/// The list of file hashes inside every package.
pub const CONTENTS: &str = "CONTENTS.sha256";

/// In a per-platform package (one whose `run` names programs inside it), the target triple it was built for, on one
/// line. It is a file like any other, so the content digest covers it and a package cannot pass for another platform.
pub const TARGET: &str = "TARGET";

/// The folder of an extension's source that holds native programs, one subfolder per target triple:
/// `native/x86_64-unknown-linux-musl/bin/server` is `bin/server` in that platform's package. Never packaged as such.
pub const NATIVE_DIR: &str = "native";

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
    /// A code extension's `extension.wasm` is missing or fails the import check.
    Component(Vec<Diagnostic>),
    /// The manifest refers to a file the package does not have.
    MissingFile {
        field: String,
        path: String,
    },
    /// A file the package needs, or the manifest names, is left out by a `.cyxignore` line.
    Ignored {
        /// The manifest field that names the file; none for a file every package needs (the manifest, the component).
        field: Option<String>,
        path: String,
        by: rules::IgnoreHit,
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
            Error::Component(d) => {
                write!(f, "{COMPONENT} cannot run")?;
                for e in d {
                    write!(f, "\n{e}")?;
                }
                Ok(())
            }
            Error::MissingFile { field, path } => {
                write!(f, "{field} points to '{path}', which is not in the package")?;
                match rules::left_out_because(path) {
                    Some(why) => write!(f, ": {why}; move the file"),
                    None => Ok(()),
                }
            }
            Error::Ignored { field: Some(field), path, by } => write!(
                f,
                "{field} points to '{path}', which {IGNORE_FILE} line {} ('{}') leaves out of the package\n   = fix: remove or narrow that line, or stop naming the file in {}",
                by.line,
                by.pattern,
                ext_manifest::FILE_NAME
            ),
            Error::Ignored { field: None, path, by } => write!(
                f,
                "'{path}' is left out of the package by {IGNORE_FILE} line {} ('{}'), and every package needs it\n   = fix: remove or narrow that line",
                by.line, by.pattern
            ),
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
    /// What a code extension's component imports; none for declarative extensions.
    pub component: Option<Component>,
    /// The target triple of a per-platform package (its `TARGET` file); none for a package without native programs.
    pub target: Option<String>,
}

impl Package {
    /// The bytes of a native program (`run = ["./bin/server"]`) for this package's platform.
    pub fn program(&self, run: &str) -> Option<&Entry> {
        self.files.get(&ext_manifest::program_file(run, self.target.as_deref()?))
    }
}

/// The file name `cyx package` writes: `<id>-<version>.cyx`, or `<id>-<version>-<triple>.cyx` for one platform.
pub fn file_name(m: &Manifest, target: Option<&str>) -> String {
    match target {
        Some(t) => format!("{}-{}-{t}.cyx", m.id, m.version),
        None => format!("{}-{}.cyx", m.id, m.version),
    }
}

/// The compressed size limit of a package: per-platform packages may be larger (registry policy, §8).
fn size_limit(target: Option<&str>) -> u64 {
    if target.is_some() { limits::TARGET_PACKAGE_BYTES } else { limits::PACKAGE_BYTES }
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

/// The files the manifest names, with the field that names each.
fn named_files(m: &Manifest) -> Vec<(&'static str, &str)> {
    let c = &m.contributions;
    let mut refs: Vec<(&'static str, &str)> = Vec::new();
    refs.extend(c.themes.iter().map(|t| ("themes.path", t.path.as_str())));
    refs.extend(c.keymap_presets.iter().map(|t| ("keymap-preset.path", t.path.as_str())));
    refs.extend(c.snippets.iter().map(|s| ("snippets.path", s.path.as_str())));
    refs.extend(c.json_schemas.iter().map(|s| ("json-schemas.path", s.path.as_str())));
    refs.extend(c.yaml_schemas.iter().map(|s| ("yaml-schemas.path", s.path.as_str())));
    if let Some(i) = &m.icon {
        refs.push(("extension.icon", i));
    }
    refs
}

/// Every file the manifest names must be in the package.
fn check_references(m: &Manifest, files: &BTreeMap<String, Entry>) -> Result<(), Error> {
    for (field, path) in named_files(m) {
        if !files.contains_key(path) {
            return Err(Error::MissingFile { field: field.into(), path: path.into() });
        }
    }
    // A program inside the package (`run = ["./bin/server"]`) is built per platform: the package says which one, and
    // the program for it must be there and be executable (Windows has no executable bit).
    let target = target_of(files)?;
    let mut programs = m.permissions.package_programs().peekable();
    match (&target, programs.peek()) {
        (None, Some(p)) => {
            return Err(Error::Path {
                path: (*p).into(),
                reason: format!(
                    "is a program inside the package, so the extension is packaged per platform: put it in {NATIVE_DIR}/<triple>/ and run `cyx package --target <triple>`"
                ),
            });
        }
        (Some(t), None) => {
            return Err(Error::Path { path: TARGET.into(), reason: format!("names {t}, but permissions.run lists no program inside the package") });
        }
        _ => {}
    }
    if let Some(t) = &target {
        for p in programs {
            let file = ext_manifest::program_file(p, t);
            match files.get(&file) {
                None => return Err(Error::MissingFile { field: "permissions.run".into(), path: file }),
                Some(e) if !e.executable && !t.contains("-windows-") => {
                    return Err(Error::Path { path: file, reason: "is in permissions.run but not executable (mode 755)".into() });
                }
                Some(_) => {}
            }
        }
    }
    Ok(())
}

/// The `TARGET` file: one known triple and a newline, nothing else.
fn target_of(files: &BTreeMap<String, Entry>) -> Result<Option<String>, Error> {
    let Some(e) = files.get(TARGET) else { return Ok(None) };
    let text = std::str::from_utf8(&e.data).unwrap_or_default();
    let triple = text.strip_suffix('\n').unwrap_or_default();
    if !ext_manifest::is_target(triple) || e.executable {
        let known = ext_manifest::TARGETS.join(", ");
        return Err(Error::Path { path: TARGET.into(), reason: format!("must be one target triple and a newline, one of: {known}") });
    }
    Ok(Some(triple.to_string()))
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
    component_of(&manifest, &files)?;
    let tar = canonical_tar(&entries)?;
    let digest = ContentDigest(sha256(&tar));
    let mut gz = flate2::GzBuilder::new().mtime(0).operating_system(255).write(Vec::new(), flate2::Compression::best());
    std::io::Write::write_all(&mut gz, &tar).map_err(Error::Io)?;
    let bytes = gz.finish().map_err(Error::Io)?;
    let limit = size_limit(target_of(&files)?.as_deref());
    if bytes.len() as u64 > limit {
        return Err(Error::TooLarge(format!("the package is larger than {} MiB compressed", limit >> 20)));
    }
    Ok((bytes, digest))
}

/// A code extension's component, checked against its manifest (the import check).
fn component_of(manifest: &Manifest, files: &BTreeMap<String, Entry>) -> Result<Option<Component>, Error> {
    match (&manifest.code, files.get(COMPONENT)) {
        (Some(_), Some(wasm)) => check_component(&wasm.data, manifest).map(Some).map_err(Error::Component),
        (Some(_), None) => Err(Error::Component(vec![
            Diagnostic::new(ext_manifest::Code::BadComponent, format!("the manifest has [code] but there is no {COMPONENT}"))
                .fix("run `cyx package` or `cyx test` first: they build extension.wasm (Rust with cargo, TypeScript with esbuild and ComponentizeJS)"),
        ])),
        (None, _) => Ok(None),
    }
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
    // The larger per-platform limit first; a package without a `TARGET` is held to the smaller one once it is read.
    if bytes.len() as u64 > limits::TARGET_PACKAGE_BYTES {
        return Err(Error::TooLarge(format!("the package is larger than {} MiB compressed", limits::TARGET_PACKAGE_BYTES >> 20)));
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
    // Its mode is in the digest but in no hash list: only the canonical one is accepted, so one set of files has one
    // digest (found by fuzzing).
    if listed.executable {
        return Err(Error::Contents(format!("{CONTENTS} must not be executable (mode 644)")));
    }
    let manifest = manifest_of(&files)?;
    check_references(&manifest, &files)?;
    let target = target_of(&files)?;
    let limit = size_limit(target.as_deref());
    if bytes.len() as u64 > limit {
        return Err(Error::TooLarge(format!("the package is larger than {} MiB compressed", limit >> 20)));
    }
    let component = component_of(&manifest, &files)?;
    let digest = ContentDigest(sha256(&canonical_tar(&entries)?));
    Ok(Package { manifest, files, digest, component, target })
}

/// Collect an extension folder for `write`: the files a package holds (`rules`: no secrets, version control,
/// dependencies, build input, tests, packages such as the one `cyx package` wrote last time, nor what `.cyxignore`
/// names), with their executable bit. The `native/` folder is not collected as such. Symlinks are refused rather than
/// followed: a package must not depend on what a link points to on the author's disk.
pub fn collect_dir(root: &Path) -> Result<Vec<Entry>, Error> {
    collect_dir_for(root, None)
}

/// [`collect_dir`] for one platform: with `target`, the files of `native/<target>/` join the package at their path
/// inside that folder, and a `TARGET` file names the platform.
pub fn collect_dir_for(root: &Path, target: Option<&str>) -> Result<Vec<Entry>, Error> {
    collect(root, target).map(|c| c.files)
}

/// An extension folder collected for a package.
#[derive(Debug)]
pub struct Collected {
    pub files: Vec<Entry>,
    /// Paths left out because their name says they hold a secret (`.env`, `signing.key`), sorted: `cyx` tells the
    /// author.
    pub secrets: Vec<String>,
}

/// [`collect_dir_for`], with the secrets it left out. A file whose contents hold a private key is refused, whatever
/// its name.
pub fn collect(root: &Path, target: Option<&str>) -> Result<Collected, Error> {
    let ignore = match std::fs::read_to_string(root.join(IGNORE_FILE)) {
        Ok(text) => rules::Ignore::parse(&text).map_err(Error::Contents)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => rules::Ignore::default(),
        Err(e) => return Err(Error::Io(e)),
    };
    let mut c = Collected { files: Vec::new(), secrets: Vec::new() };
    collect_tree(root, true, &ignore, &mut c)?;
    check_not_ignored(root, &ignore, &c.files)?;
    if let Some(t) = target {
        if !ext_manifest::is_target(t) {
            return Err(Error::Path {
                path: t.into(),
                reason: format!("is not a target Codeyard builds for; use one of {}", ext_manifest::TARGETS.join(", ")),
            });
        }
        let dir = root.join(NATIVE_DIR).join(t);
        if !dir.is_dir() {
            return Err(Error::MissingFile { field: "--target".into(), path: format!("{NATIVE_DIR}/{t}/") });
        }
        let mut native = Collected { files: Vec::new(), secrets: Vec::new() };
        collect_tree(&dir, false, &rules::Ignore::default(), &mut native)?;
        c.secrets.extend(native.secrets.into_iter().map(|s| format!("{NATIVE_DIR}/{t}/{s}")));
        for mut e in native.files {
            if c.files.iter().any(|o| o.path == e.path) {
                return Err(Error::Path { path: format!("{NATIVE_DIR}/{t}/{}", e.path), reason: "is also outside native/; keep one of them".into() });
            }
            // Windows has no executable bit to read; a program built for Windows is one by its target.
            e.executable |= t.contains("-windows-") && e.path.to_ascii_lowercase().ends_with(".exe");
            c.files.push(e);
        }
        c.files.push(Entry { path: TARGET.into(), data: format!("{t}\n").into_bytes(), executable: false });
    }
    if c.files.len() > limits::PACKAGE_FILES {
        return Err(Error::TooLarge(format!("the folder has more than {} files", limits::PACKAGE_FILES)));
    }
    c.secrets.sort();
    Ok(c)
}

/// A file the package needs (the manifest, the component of a code extension, anything the manifest names) that a
/// `.cyxignore` line leaves out is named with that line, instead of surfacing later as a missing file.
fn check_not_ignored(root: &Path, ignore: &rules::Ignore, files: &[Entry]) -> Result<(), Error> {
    if ignore.is_empty() {
        return Ok(());
    }
    let mut needed: Vec<(Option<String>, String)> = vec![(None, ext_manifest::FILE_NAME.to_string())];
    let manifest = std::fs::read_to_string(root.join(ext_manifest::FILE_NAME))
        .ok()
        .and_then(|text| ext_manifest::parse_with(&text, ext_manifest::Options { require_why: false }).ok());
    if let Some(m) = &manifest {
        if m.code.is_some() {
            needed.push((None, COMPONENT.to_string()));
        }
        needed.extend(named_files(m).into_iter().map(|(field, path)| (Some(field.to_string()), path.to_string())));
    }
    for (field, path) in needed {
        if files.iter().any(|e| e.path == path) {
            continue;
        }
        if let Some(by) = ignore.explain(&path) {
            return Err(Error::Ignored { field, path, by });
        }
    }
    Ok(())
}

/// The target triples an extension folder has programs for (`native/<triple>/`), sorted.
pub fn native_targets(root: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(root.join(NATIVE_DIR))
        .map(|d| d.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    out.retain(|t| ext_manifest::is_target(t));
    out.sort();
    out
}

/// The platform to check or load an extension folder for: none without `native/`, else this machine's when the
/// folder has it, else the first.
pub fn folder_target(root: &Path) -> Option<String> {
    let targets = native_targets(root);
    let here = ext_manifest::this_target();
    targets.iter().find(|t| Some(t.as_str()) == here).or(targets.first()).cloned()
}

/// Walk `root`; `top` is false for a `native/<triple>/` folder, where the top-of-folder rules do not apply.
fn collect_tree(root: &Path, top: bool, ignore: &rules::Ignore, out: &mut Collected) -> Result<(), Error> {
    fn walk(root: &Path, dir: &Path, top: bool, ignore: &rules::Ignore, out: &mut Collected) -> Result<(), Error> {
        let mut names: Vec<_> = std::fs::read_dir(dir).map_err(Error::Io)?.collect::<Result<_, _>>().map_err(Error::Io)?;
        names.sort_by_key(|e| e.file_name());
        for item in names {
            let path = item.path();
            let rel = path.strip_prefix(root).expect("walked under root").to_string_lossy().replace('\\', "/");
            let meta = std::fs::symlink_metadata(&path).map_err(Error::Io)?;
            if top && dir == root && item.file_name() == NATIVE_DIR && meta.is_dir() {
                continue;
            }
            match rules::rule(&rel, meta.is_dir(), top, ignore) {
                rules::Rule::Keep => {}
                rules::Rule::Skip => continue,
                rules::Rule::Secret => {
                    out.secrets.push(rel);
                    continue;
                }
            }
            if meta.file_type().is_symlink() {
                return Err(Error::Path { path: rel, reason: "is a symbolic link; copy the file into the extension instead".into() });
            }
            if meta.is_dir() {
                walk(root, &path, top, ignore, out)?;
            } else if meta.is_file() {
                if meta.len() > limits::PACKAGE_FILE_BYTES {
                    return Err(Error::TooLarge(format!("'{rel}' is larger than {} MiB", limits::PACKAGE_FILE_BYTES >> 20)));
                }
                let data = std::fs::read(&path).map_err(Error::Io)?;
                if let Some(leak) = rules::leak_in(item.file_name().to_string_lossy().as_ref(), &data) {
                    let (what, fix) = match leak {
                        rules::Leak::PrivateKey => ("holds a private key".to_string(), "move it out of the extension folder"),
                        rules::Leak::Token(kind) => (format!("holds what looks like a {kind} access token"), "remove it"),
                    };
                    return Err(Error::Path {
                        path: rel,
                        reason: format!("{what}, and a package must never hold one: {fix}, or list the file in .cyxignore to leave it out"),
                    });
                }
                out.files.push(Entry { path: rel, data, executable: is_executable(&meta) });
                if out.files.len() > limits::PACKAGE_FILES {
                    return Err(Error::TooLarge(format!("the folder has more than {} files", limits::PACKAGE_FILES)));
                }
            } else {
                return Err(Error::Path { path: rel, reason: "is not a regular file".into() });
            }
        }
        Ok(())
    }
    walk(root, root, top, ignore, out)
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
