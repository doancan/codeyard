//! Plain-language text for a failed file operation: what failed, on which path, the likely cause and a next step.
//! Pure: no UI, no I/O. The raw operating-system text is kept as the detail so nothing is lost, but it is never the headline.

use std::io::{self, ErrorKind};
use std::path::Path;

use crate::i18n::{t, tf, tr};

/// A path longer than this (characters) is shortened in the sentence; the whole path stays in the detail.
const MAX_PATH_CHARS: usize = 160;
const KEEP_HEAD: usize = 40;
const KEEP_TAIL: usize = 100;

/// What was being done to the path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    Read,
    Write,
    Save,
    Create,
    Delete,
    Rename,
}

/// Why it failed, as far as the error says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    Permission,
    /// EPERM: not a matter of file permissions (an immutable or protected item, a sandbox).
    NotPermitted,
    NotFound,
    /// Creating or renaming something where the folder it belongs in is not there.
    ParentMissing,
    Exists,
    NoSpace,
    ReadOnly,
    CrossDevice,
    InvalidName,
    NameTooLong,
    NotEmpty,
    Busy,
    Timeout,
    Interrupted,
    BrokenPipe,
    NotADirectory,
    IsADirectory,
    Other,
}

/// The message for a person. `message` has the headline with the path and the cause; `cause` is the cause sentence alone (for a
/// dialog that already says what failed); `hint` is the next step; `detail` is the raw error text and the full path, for the
/// notification's detail (the whole path too when the sentence had to shorten it). A cause the system does not name keeps its raw text in the sentence, so nothing is hidden.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorText {
    pub message: String,
    pub cause: String,
    pub hint: String,
    pub detail: String,
}

impl ErrorText {
    /// Headline, cause and next step as one text, for places that have no room for a separate hint.
    pub fn full(&self) -> String {
        if self.hint.is_empty() { self.message.clone() } else { format!("{} {}", self.message, self.hint) }
    }
}

/// The cause of an I/O error. Kinds first (the standard library maps Windows and Unix codes onto them); the raw code only
/// settles what the kind leaves open.
pub fn classify(err: &io::Error) -> Cause {
    // EPERM is reported as "permission denied" too, but its advice is not the same as for EACCES.
    if let Some(Cause::NotPermitted) = err.raw_os_error().map(classify_code) {
        return Cause::NotPermitted;
    }
    match err.kind() {
        ErrorKind::PermissionDenied => Cause::Permission,
        ErrorKind::NotFound => Cause::NotFound,
        ErrorKind::AlreadyExists => Cause::Exists,
        ErrorKind::StorageFull | ErrorKind::QuotaExceeded => Cause::NoSpace,
        ErrorKind::ReadOnlyFilesystem => Cause::ReadOnly,
        ErrorKind::CrossesDevices => Cause::CrossDevice,
        // The standard library files both a bad name and a name that is too long under this kind; the code tells them apart.
        ErrorKind::InvalidFilename => err.raw_os_error().map(classify_code).filter(|c| *c == Cause::NameTooLong).unwrap_or(Cause::InvalidName),
        ErrorKind::DirectoryNotEmpty => Cause::NotEmpty,
        ErrorKind::ResourceBusy => Cause::Busy,
        ErrorKind::TimedOut => Cause::Timeout,
        ErrorKind::Interrupted => Cause::Interrupted,
        ErrorKind::BrokenPipe => Cause::BrokenPipe,
        ErrorKind::NotADirectory => Cause::NotADirectory,
        ErrorKind::IsADirectory => Cause::IsADirectory,
        _ => err.raw_os_error().map_or(Cause::Other, classify_code),
    }
}

/// Windows error codes that older standard libraries leave uncategorized (sharing violation, disk full, bad name).
/// Plain numbers on every platform, so the table is testable everywhere.
#[cfg(any(windows, test))]
fn classify_windows_code(code: i32) -> Cause {
    match code {
        5 | 19 => Cause::Permission,
        32 | 33 => Cause::Busy,
        39 | 112 => Cause::NoSpace,
        123 | 161 => Cause::InvalidName,
        206 => Cause::NameTooLong,
        145 => Cause::NotEmpty,
        _ => Cause::Other,
    }
}

#[cfg(windows)]
fn classify_code(code: i32) -> Cause {
    classify_windows_code(code)
}

#[cfg(not(windows))]
fn classify_code(code: i32) -> Cause {
    match code {
        libc::EPERM => Cause::NotPermitted,
        libc::ENOENT => Cause::NotFound,
        libc::EEXIST => Cause::Exists,
        libc::ENOSPC | libc::EDQUOT => Cause::NoSpace,
        libc::EROFS => Cause::ReadOnly,
        libc::EXDEV => Cause::CrossDevice,
        libc::ENAMETOOLONG => Cause::NameTooLong,
        libc::ENOTEMPTY => Cause::NotEmpty,
        libc::EBUSY | libc::ETXTBSY => Cause::Busy,
        libc::ETIMEDOUT => Cause::Timeout,
        libc::EPIPE => Cause::BrokenPipe,
        libc::ENOTDIR => Cause::NotADirectory,
        libc::EISDIR => Cause::IsADirectory,
        libc::EACCES => Cause::Permission,
        _ => Cause::Other,
    }
}

/// Characters that reorder, hide or break text (control characters, zero-width and bidirectional marks and overrides).
fn is_unsafe(c: char) -> bool {
    c.is_control() || matches!(c, '\u{61c}' | '\u{180e}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}')
}

/// `text` with each unsafe character written out as `\u{202e}` so a file name cannot spoof what is shown (`evil\u{202e}txt.exe`).
/// `keep_lines` leaves newlines and tabs alone, for text that is allowed to have several lines.
pub fn escape_unsafe(text: &str, keep_lines: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if is_unsafe(c) && !(keep_lines && matches!(c, '\n' | '\t')) {
            out.push_str(&format!("\\u{{{:x}}}", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

/// `…` in the middle of a path that is too long to read in a sentence; its start and its file name stay.
pub fn shorten_path(path: &str) -> String {
    let path = escape_unsafe(path, false);
    let count = path.chars().count();
    if count <= MAX_PATH_CHARS {
        return path.to_string();
    }
    let head: String = path.chars().take(KEEP_HEAD).collect();
    let tail: String = path.chars().skip(count - KEEP_TAIL).collect();
    format!("{head}…{tail}")
}

fn headline(verb: Verb, path: &str) -> String {
    let key = match verb {
        Verb::Read => tr("Could not read {}."),
        Verb::Write => tr("Could not write {}."),
        Verb::Save => tr("Could not save {}."),
        Verb::Create => tr("Could not create {}."),
        Verb::Delete => tr("Could not delete {}."),
        Verb::Rename => tr("Could not rename {}."),
    };
    tf(key, &[&path])
}

fn cause_text(cause: Cause) -> &'static str {
    match cause {
        Cause::Permission => tr("You do not have permission to do that."),
        Cause::NotPermitted => tr("The system does not allow this: the item may be protected, locked or sandboxed."),
        Cause::NotFound => tr("It does not exist (it may have been moved or deleted)."),
        Cause::ParentMissing => tr("The folder it belongs in does not exist."),
        Cause::Exists => tr("Something with that name already exists."),
        Cause::NoSpace => tr("The disk is full (or your quota is used up)."),
        Cause::ReadOnly => tr("The disk or folder is read-only."),
        Cause::CrossDevice => tr("The source and the target are on different disks."),
        Cause::InvalidName => tr("The name has characters this system does not allow."),
        Cause::NameTooLong => tr("The name or the path is too long."),
        Cause::NotEmpty => tr("The folder is not empty."),
        Cause::Busy => tr("Another program is using it."),
        Cause::Timeout => tr("The operation took too long and was stopped."),
        Cause::Interrupted => tr("The operation was interrupted."),
        Cause::BrokenPipe => tr("The connection closed before it finished."),
        Cause::NotADirectory => tr("A part of the path is a file, not a folder."),
        Cause::IsADirectory => tr("It is a folder, not a file."),
        Cause::Other => tr("The system reported an error: {}."),
    }
}

fn hint_text(cause: Cause) -> &'static str {
    match cause {
        Cause::Permission => tr("Check the permissions of the file and its folder, then try again."),
        Cause::NotPermitted => tr("Check whether the item is locked or protected, or choose another location."),
        Cause::NotFound => tr("Check that the path is right and that the drive or network share is connected."),
        Cause::ParentMissing => tr("Create the folder first, or check that the path is right."),
        Cause::Exists => tr("Choose another name or remove the existing one first."),
        Cause::NoSpace => tr("Free some space and try again."),
        Cause::ReadOnly => tr("Make the location writable, or save somewhere else."),
        Cause::CrossDevice => tr("Copy it to the other disk, then delete the original."),
        Cause::InvalidName => tr("Use a name without special characters."),
        Cause::NameTooLong => tr("Use a shorter name or a shallower folder."),
        Cause::NotEmpty => tr("Empty the folder first, or delete its contents too."),
        Cause::Busy => tr("Close the other program, then try again."),
        Cause::Timeout => tr("Check the connection or the disk, then try again."),
        Cause::Interrupted => tr("Try again."),
        Cause::BrokenPipe => tr("Check the connection, then try again."),
        Cause::NotADirectory => tr("Check the path."),
        Cause::IsADirectory => tr("Choose a file instead."),
        Cause::Other => tr("Try again; copy the details if it keeps happening."),
    }
}

/// Describe `err`, which happened while doing `verb` to `path`.
pub fn describe_io(err: &io::Error, path: &Path, verb: Verb) -> ErrorText {
    build(classify(err), &err.to_string(), path, verb)
}

/// The same for an error that came back from a host (a remote agent keeps the operating system's words in its message, with the
/// error number in parentheses, so the cause is still found).
pub fn describe_host(err: &host::HostError, path: &Path, verb: Verb) -> ErrorText {
    use host::HostError;
    let text = err.to_string();
    let cause = match err {
        HostError::Op(proto::Error::PermissionDenied(_)) => Cause::Permission,
        HostError::Op(proto::Error::NotFound(_)) => Cause::NotFound,
        HostError::Op(proto::Error::IsDirectory(_)) => Cause::IsADirectory,
        HostError::Op(proto::Error::AlreadyExists(_)) => Cause::Exists,
        HostError::Op(proto::Error::Cancelled) => Cause::Interrupted,
        HostError::Disconnected(_) => Cause::BrokenPipe,
        _ => os_error_number(&text).map_or(Cause::Other, classify_linux_code),
    };
    build(cause, &text, path, verb)
}

/// Error numbers of the remote agents, which are Linux programs whatever this machine runs.
fn classify_linux_code(code: i32) -> Cause {
    match code {
        1 => Cause::NotPermitted,
        2 => Cause::NotFound,
        13 => Cause::Permission,
        16 | 26 => Cause::Busy,
        17 => Cause::Exists,
        18 => Cause::CrossDevice,
        20 => Cause::NotADirectory,
        21 => Cause::IsADirectory,
        28 | 122 => Cause::NoSpace,
        30 => Cause::ReadOnly,
        32 => Cause::BrokenPipe,
        36 => Cause::NameTooLong,
        39 => Cause::NotEmpty,
        110 => Cause::Timeout,
        _ => Cause::Other,
    }
}

/// The line a parse error points at ("... at line 3 column 14"), for a button that opens the file there.
pub fn line_in(message: &str) -> Option<u32> {
    let after = message.split(" at line ").nth(1)?;
    after.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok().filter(|line| *line > 0)
}

/// `N` of "(os error N)" in the text of an I/O error.
fn os_error_number(text: &str) -> Option<i32> {
    let after = text.split("(os error ").nth(1)?;
    after.split(')').next()?.trim().parse().ok()
}

fn build(cause: Cause, raw: &str, path: &Path, verb: Verb) -> ErrorText {
    // Making something where its folder is missing is a different story from a file that went away.
    let cause = if cause == Cause::NotFound && matches!(verb, Verb::Create | Verb::Rename) { Cause::ParentMissing } else { cause };
    let full_path = escape_unsafe(&path.display().to_string(), false);
    let raw = escape_unsafe(raw, true);
    let raw = raw.as_str();
    let shown_path = shorten_path(&full_path);
    let cause_sentence = match cause {
        Cause::Other => tf(cause_text(cause), &[&raw]),
        known => t(cause_text(known)).to_string(),
    };
    ErrorText {
        message: format!("{} {cause_sentence}", headline(verb, &shown_path)),
        cause: cause_sentence,
        hint: t(hint_text(cause)).to_string(),
        detail: if shown_path == full_path { raw.to_string() } else { format!("{full_path}\n{raw}") },
    }
}

#[cfg(test)]
#[path = "errors_tests.rs"]
mod tests;
