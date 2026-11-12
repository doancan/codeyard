//! Settings profiles: named bundles of `settings.jsonc` (which holds the theme), `keybindings.jsonc` and `snippets/`, in
//! `<config>/profiles/<name>/`. The default profile is the config folder itself, so nothing exists until a profile is
//! created and a user who never makes one sees no change. Which profile is active is one line in `<config>/active-profile`.
//!
//! A profile name becomes a folder name on every platform, so it is checked for Windows and macOS at once: no path
//! characters, no reserved device names (`CON`, `NUL`, …), no trailing dot or space, no combining marks (a decomposed
//! `é` and a precomposed one are one folder on macOS), and no name that differs from another profile only in case.
//! Nothing read from a file (the active-profile line, folder names) reaches a path before it passes the same check.

use std::io;
use std::path::{Path, PathBuf};

/// The folder under the config folder that holds the profiles.
pub const DIR: &str = "profiles";
/// The file in the config folder that names the active profile; absent means the default profile.
pub const POINTER: &str = "active-profile";
/// Longest name, in characters.
pub const MAX_NAME_CHARS: usize = 40;
/// What the default profile is called in lists. No profile may take this name, in any case.
pub const DEFAULT_NAME: &str = "Default";
/// Files copied by `duplicate`.
const FILES: [&str; 2] = ["settings.jsonc", "keybindings.jsonc"];
const SNIPPETS: &str = "snippets";
/// The pointer file is read up to this many bytes.
const POINTER_MAX_BYTES: u64 = 1024;
/// A copied snippets folder stops here: a profile is a few small files.
const MAX_COPIED_FILES: usize = 500;
const MAX_COPIED_BYTES: u64 = 16 * 1024 * 1024;

/// Windows device names, which cannot be file names even with an extension.
const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7",
    "LPT8", "LPT9",
];

#[derive(Debug, Clone, PartialEq)]
pub enum NameError {
    Empty,
    TooLong,
    /// A character that is not a letter, digit, space, `-`, `_` or `.` (path separators, `* ? : " < > |`, control and
    /// invisible characters, emoji).
    Character(char),
    /// A character that has another spelling with the same folder on macOS (the Kelvin sign and `K`, jamo and Hangul
    /// syllables, CJK compatibility ideographs).
    Equivalent(char),
    /// Combining marks make two spellings of one name; the precomposed letter is the one to type.
    CombiningMark,
    LeadingDot,
    TrailingDot,
    /// A Windows device name.
    Reserved(String),
    /// "Default" is the profile every user has.
    Default,
    /// The name of another profile, in other letter case.
    Exists(String),
}

impl NameError {
    /// What to tell the user, in their language.
    pub fn message(&self) -> String {
        use crate::i18n::{t, tf};
        match self {
            NameError::Empty => t("Type a name for the profile.").into(),
            NameError::TooLong => tf("A profile name has at most {} characters.", &[&MAX_NAME_CHARS]),
            NameError::Character(c) => tf("A profile name cannot contain “{}”. Use letters, digits, spaces, - and _.", &[&c.escape_default()]),
            NameError::Equivalent(c) => tf("“{}” has another spelling that is the same folder on macOS. Use the usual letter.", &[&c]),
            NameError::CombiningMark => t("A profile name cannot contain combining marks; type the letter as one character.").into(),
            NameError::LeadingDot => t("A profile name cannot start with a dot.").into(),
            NameError::TrailingDot => t("A profile name cannot end with a dot.").into(),
            NameError::Reserved(name) => tf("“{}” is a reserved name on Windows. Choose another.", &[name]),
            NameError::Default => t("“Default” is the profile everyone has. Choose another name.").into(),
            NameError::Exists(other) => tf("There is already a profile named “{}”. Names differ by more than letter case.", &[other]),
        }
    }
}

/// The name as it will be used (trimmed), or why it cannot be one. Does not look at the disk.
pub fn validate_name(name: &str) -> Result<String, NameError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(NameError::Empty);
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(NameError::TooLong);
    }
    for c in name.chars() {
        if ('\u{300}'..='\u{36f}').contains(&c) {
            return Err(NameError::CombiningMark);
        }
        if has_other_spelling(c) {
            return Err(NameError::Equivalent(c));
        }
        // Letters, ASCII digits and CJK numerals; not superscript digits (`COM¹` is a device name on Windows) or other digit forms.
        let allowed = c.is_alphabetic() || c.is_ascii_digit() || matches!(c, ' ' | '-' | '_' | '.');
        if !allowed {
            return Err(NameError::Character(c));
        }
    }
    if name.starts_with('.') {
        return Err(NameError::LeadingDot);
    }
    if name.ends_with('.') {
        return Err(NameError::TrailingDot);
    }
    // `con`, `NUL.txt` and `Com1.x` all name a device on Windows.
    let stem = name.split('.').next().unwrap_or(name).trim_end().to_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        return Err(NameError::Reserved(name.to_string()));
    }
    if same_folder(name, DEFAULT_NAME) {
        return Err(NameError::Default);
    }
    Ok(name.to_string())
}

/// Canonical equivalents that are single characters (what an NFC/NFD comparison would fold): macOS treats them as one folder.
/// Without a normalization library this covers the ranges that matter; names with other exotic spellings are not detected.
fn has_other_spelling(c: char) -> bool {
    matches!(c, '\u{212a}' | '\u{212b}' | '\u{2126}' | '\u{374}' | '\u{37e}' | '\u{387}' | '\u{1fef}' | '\u{1ffd}')
        || ('\u{1100}'..='\u{11ff}').contains(&c)
        || ('\u{a960}'..='\u{a97f}').contains(&c)
        || ('\u{d7b0}'..='\u{d7ff}').contains(&c)
        || ('\u{f900}'..='\u{faff}').contains(&c)
        || ('\u{2f800}'..='\u{2fa1f}').contains(&c)
}

/// Two names are one folder on a case-insensitive file system. `İ` lowercases to `i` plus a combining dot, which is
/// dropped so it meets `i` as it does on macOS.
fn same_folder(a: &str, b: &str) -> bool {
    let fold = |s: &str| s.to_lowercase().replace('\u{307}', "");
    fold(a) == fold(b)
}

/// `validate_name`, and no other profile of `config` has the same name apart from letter case (`ignoring`: the profile
/// that is being renamed).
pub fn check_new_name(config: &Path, name: &str, ignoring: Option<&str>) -> Result<String, NameError> {
    let name = validate_name(name)?;
    match list(config).into_iter().find(|other| same_folder(other, &name) && Some(other.as_str()) != ignoring) {
        Some(other) => Err(NameError::Exists(other)),
        None => Ok(name),
    }
}

pub fn profile_dir(config: &Path, name: &str) -> PathBuf {
    config.join(DIR).join(name)
}

/// Where the settings, keybindings and snippets of `profile` are: its folder, or `config` for the default profile.
pub fn files_dir(config: &Path, profile: Option<&str>) -> PathBuf {
    profile.map_or_else(|| config.to_path_buf(), |name| profile_dir(config, name))
}

/// The profiles that exist, by name: folders of the profiles folder whose name is a valid profile name.
pub fn list(config: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(config.join(DIR))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| validate_name(name).as_deref() == Ok(name.as_str()))
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

/// The profile in force, and why another one named in the file is not.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Active {
    /// `None`: the default profile.
    pub name: Option<String>,
    /// The profile `active-profile` names could not be used (the folder is gone, is not a folder, cannot be read, or the
    /// name is not a profile name): the default profile is in force instead.
    pub fell_back: Option<String>,
    /// The fall back may be temporary (the folder exists but cannot be read now: permissions, a network drive): the
    /// pointer file must stay, so the profile comes back when the folder can be read again.
    pub transient: bool,
}

/// Reads `active-profile`. A missing file is the default profile; a name that is wrong or whose folder is not usable is the
/// default profile as well, with the name in `fell_back`.
pub fn read_active(config: &Path) -> Active {
    use std::io::Read as _;
    // Only the first kilobyte is read: a name is a few dozen characters, and a huge file must not be read to find that out.
    let mut bytes = Vec::new();
    let read = std::fs::File::open(config.join(POINTER)).and_then(|f| f.take(POINTER_MAX_BYTES + 1).read_to_end(&mut bytes));
    if read.is_err() {
        return Active::default();
    }
    let text = if bytes.len() as u64 <= POINTER_MAX_BYTES { String::from_utf8_lossy(&bytes).trim().to_string() } else { String::from("?") };
    if text.is_empty() {
        return Active::default();
    }
    let shown: String = text.chars().take(MAX_NAME_CHARS).collect();
    let fall_back = |transient: bool| Active { name: None, fell_back: Some(shown.clone()), transient };
    if !validate_name(&text).is_ok_and(|n| n == text) {
        return fall_back(false);
    }
    let dir = profile_dir(config, &text);
    match std::fs::metadata(&dir) {
        Ok(meta) if !meta.is_dir() => fall_back(false),
        Ok(_) if std::fs::read_dir(&dir).is_ok() => Active { name: Some(text), ..Active::default() },
        Ok(_) => fall_back(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => fall_back(false),
        Err(_) => fall_back(true),
    }
}

/// Makes `name` the active profile (`None`: the default one, which removes the pointer file).
pub fn set_active(config: &Path, name: Option<&str>) -> io::Result<()> {
    let path = config.join(POINTER);
    match name {
        Some(name) => crate::fsutil::write_atomic(&path, format!("{name}\n")),
        None => match std::fs::remove_file(path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
    }
}

#[derive(Debug)]
pub enum ProfileError {
    Name(NameError),
    /// There is no such profile.
    Missing(String),
    Io(String),
}

impl ProfileError {
    pub fn message(&self) -> String {
        match self {
            ProfileError::Name(e) => e.message(),
            ProfileError::Missing(name) => crate::i18n::tf("There is no profile named “{}”.", &[name]),
            ProfileError::Io(e) => e.clone(),
        }
    }
}

impl From<NameError> for ProfileError {
    fn from(e: NameError) -> Self {
        ProfileError::Name(e)
    }
}

fn io_error(what: &Path, e: io::Error) -> ProfileError {
    ProfileError::Io(format!("{}: {e}", what.display()))
}

fn make_folder(config: &Path, name: &str) -> Result<PathBuf, ProfileError> {
    let root = config.join(DIR);
    std::fs::create_dir_all(&root).map_err(|e| io_error(&root, e))?;
    let dir = root.join(name);
    // `create_dir` fails when something is there already, so two windows creating one name cannot both succeed.
    std::fs::create_dir(&dir).map_err(|e| io_error(&dir, e))?;
    Ok(dir)
}

/// A new profile with the default settings and keybindings files.
pub fn create(config: &Path, name: &str) -> Result<String, ProfileError> {
    let name = check_new_name(config, name, None)?;
    let dir = make_folder(config, &name)?;
    for (file, template) in [("settings.jsonc", crate::settings::TEMPLATE), ("keybindings.jsonc", crate::settings::KEYBINDINGS_TEMPLATE)] {
        let path = dir.join(file);
        if let Err(e) = std::fs::write(&path, template) {
            // A half-made profile is worse than none.
            let _ = std::fs::remove_dir_all(&dir);
            return Err(io_error(&path, e));
        }
    }
    Ok(name)
}

/// A new profile that starts as a copy of `from` (`None`: the default profile): its settings, keybindings and snippets.
pub fn duplicate(config: &Path, from: Option<&str>, name: &str) -> Result<String, ProfileError> {
    if let Some(from) = from {
        validate_name(from)?;
        if !profile_dir(config, from).is_dir() {
            return Err(ProfileError::Missing(from.to_string()));
        }
    }
    let name = check_new_name(config, name, None)?;
    let source = files_dir(config, from);
    let dir = make_folder(config, &name)?;
    let copied = copy_files(&source, &dir);
    if let Err(e) = copied {
        // A half-made profile is worse than none.
        let _ = std::fs::remove_dir_all(&dir);
        return Err(e);
    }
    Ok(name)
}

fn copy_files(source: &Path, dir: &Path) -> Result<(), ProfileError> {
    for file in FILES {
        let (from, to) = (source.join(file), dir.join(file));
        match std::fs::read(&from) {
            Ok(bytes) => std::fs::write(&to, bytes).map_err(|e| io_error(&to, e))?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_error(&from, e)),
        }
    }
    let (mut files, mut bytes) = (0, 0);
    copy_folder(&source.join(SNIPPETS), &dir.join(SNIPPETS), 0, &mut files, &mut bytes)
}

/// Copies the regular files of a folder tree. Symbolic links are skipped: a link out of the folder must not pull
/// whatever it points to into a profile.
fn copy_folder(from: &Path, to: &Path, depth: usize, files: &mut usize, bytes: &mut u64) -> Result<(), ProfileError> {
    let Ok(entries) = std::fs::read_dir(from) else { return Ok(()) };
    std::fs::create_dir_all(to).map_err(|e| io_error(to, e))?;
    for entry in entries.flatten() {
        let (path, target) = (entry.path(), to.join(entry.file_name()));
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() && depth < 4 {
            copy_folder(&path, &target, depth + 1, files, bytes)?;
        } else if kind.is_file() {
            *files += 1;
            *bytes += entry.metadata().map_or(0, |m| m.len());
            if *files > MAX_COPIED_FILES || *bytes > MAX_COPIED_BYTES {
                return Err(ProfileError::Io(crate::i18n::tf("{} has too many or too large snippet files to copy.", &[&from.display()])));
            }
            std::fs::copy(&path, &target).map_err(|e| io_error(&path, e))?;
        }
    }
    Ok(())
}

/// Gives profile `from` the name `to`. Only the letter case may change when `to` is `from` apart from case.
pub fn rename(config: &Path, from: &str, to: &str) -> Result<String, ProfileError> {
    validate_name(from)?;
    let source = profile_dir(config, from);
    if !source.is_dir() {
        return Err(ProfileError::Missing(from.to_string()));
    }
    let to = check_new_name(config, to, Some(from))?;
    if to == from {
        return Ok(to);
    }
    let target = profile_dir(config, &to);
    std::fs::rename(&source, &target).map_err(|e| io_error(&source, e))?;
    Ok(to)
}

/// Removes profile `name` and everything in it. Links inside it are removed, not followed.
pub fn delete(config: &Path, name: &str) -> Result<(), ProfileError> {
    validate_name(name)?;
    let dir = profile_dir(config, name);
    let meta = std::fs::symlink_metadata(&dir).map_err(|_| ProfileError::Missing(name.to_string()))?;
    let removed = if meta.file_type().is_symlink() { std::fs::remove_file(&dir).or_else(|_| std::fs::remove_dir(&dir)) } else { std::fs::remove_dir_all(&dir) };
    removed.map_err(|e| io_error(&dir, e))
}

#[cfg(test)]
#[path = "profiles_tests.rs"]
mod tests;
