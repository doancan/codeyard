use super::*;
use std::path::PathBuf;

fn err(kind: ErrorKind) -> io::Error {
    io::Error::from(kind)
}

#[test]
fn every_kind_maps_to_its_cause() {
    let cases = [
        (ErrorKind::PermissionDenied, Cause::Permission),
        (ErrorKind::NotFound, Cause::NotFound),
        (ErrorKind::AlreadyExists, Cause::Exists),
        (ErrorKind::StorageFull, Cause::NoSpace),
        (ErrorKind::QuotaExceeded, Cause::NoSpace),
        (ErrorKind::ReadOnlyFilesystem, Cause::ReadOnly),
        (ErrorKind::CrossesDevices, Cause::CrossDevice),
        (ErrorKind::InvalidFilename, Cause::InvalidName),
        (ErrorKind::DirectoryNotEmpty, Cause::NotEmpty),
        (ErrorKind::ResourceBusy, Cause::Busy),
        (ErrorKind::TimedOut, Cause::Timeout),
        (ErrorKind::Interrupted, Cause::Interrupted),
        (ErrorKind::BrokenPipe, Cause::BrokenPipe),
        (ErrorKind::NotADirectory, Cause::NotADirectory),
        (ErrorKind::IsADirectory, Cause::IsADirectory),
        (ErrorKind::Other, Cause::Other),
    ];
    for (kind, cause) in cases {
        assert_eq!(classify(&err(kind)), cause, "{kind:?}");
    }
}

#[cfg(unix)]
#[test]
fn unix_error_numbers_map_to_their_cause() {
    let cases = [
        (libc::EACCES, Cause::Permission),
        (libc::ENOENT, Cause::NotFound),
        (libc::EEXIST, Cause::Exists),
        (libc::ENOSPC, Cause::NoSpace),
        (libc::EROFS, Cause::ReadOnly),
        (libc::EXDEV, Cause::CrossDevice),
        (libc::ENAMETOOLONG, Cause::NameTooLong),
        (libc::ENOTEMPTY, Cause::NotEmpty),
        (libc::EBUSY, Cause::Busy),
        (libc::ETIMEDOUT, Cause::Timeout),
        (libc::EINTR, Cause::Interrupted),
        (libc::EPIPE, Cause::BrokenPipe),
        (libc::ENOTDIR, Cause::NotADirectory),
        (libc::EISDIR, Cause::IsADirectory),
    ];
    for (code, cause) in cases {
        assert_eq!(classify(&io::Error::from_raw_os_error(code)), cause, "errno {code}");
    }
}

#[test]
fn windows_codes_map_to_their_cause_on_every_platform() {
    let cases = [
        (5, Cause::Permission),
        (32, Cause::Busy),
        (33, Cause::Busy),
        (112, Cause::NoSpace),
        (123, Cause::InvalidName),
        (206, Cause::NameTooLong),
        (145, Cause::NotEmpty),
        (1, Cause::Other),
    ];
    for (code, cause) in cases {
        assert_eq!(classify_windows_code(code), cause, "code {code}");
    }
}

#[test]
fn a_host_error_is_described_by_its_variant_or_by_the_error_number_in_its_text() {
    use host::HostError;
    let p = Path::new("/srv/app/data.db");
    let op = |e| HostError::Op(e);
    let cause = |e: &HostError| describe_host(e, p, Verb::Save);
    assert!(cause(&op(proto::Error::PermissionDenied("/srv".into()))).cause.contains("permission"));
    assert!(cause(&op(proto::Error::NotFound("/srv".into()))).cause.contains("does not exist"));
    assert!(cause(&op(proto::Error::IsDirectory("/srv".into()))).cause.contains("folder"));
    assert!(cause(&op(proto::Error::AlreadyExists("/srv".into()))).cause.contains("already exists"));
    let full = cause(&op(proto::Error::Other("/srv/app/data.db: No space left on device (os error 28)".into())));
    assert!(full.cause.contains("disk is full"), "{full:?}");
    assert_eq!(full.hint, "Free some space and try again.");
    assert!(full.detail.contains("os error 28"), "the raw text stays in the detail");
    assert!(cause(&op(proto::Error::Other("x: Read-only file system (os error 30)".into()))).cause.contains("read-only"));
    assert!(cause(&op(proto::Error::Other("x: File name too long (os error 36)".into()))).cause.contains("too long"));
    assert!(cause(&HostError::Disconnected("ssh exited".into())).cause.contains("connection"));
    let unknown = cause(&op(proto::Error::Other("x: something odd".into())));
    assert!(unknown.cause.contains("something odd"), "{unknown:?}");
}

#[test]
fn the_line_of_a_parse_error_is_found_in_the_text() {
    assert_eq!(line_in("settings.jsonc: expected value at line 3 column 14 (previous settings kept)"), Some(3));
    assert_eq!(line_in("EOF while parsing an object at line 120 column 1"), Some(120));
    assert_eq!(line_in("expected an object { \u{2026} }"), None);
    assert_eq!(line_in("at line 0 column 0"), None);
    assert_eq!(line_in("x at line  y"), None);
    assert_eq!(line_in(""), None);
}

#[test]
fn the_error_number_is_found_in_the_text() {
    assert_eq!(os_error_number("a: No space left on device (os error 28)"), Some(28));
    assert_eq!(os_error_number("no number here"), None);
    assert_eq!(os_error_number("(os error )"), None);
}

#[test]
fn every_cause_has_a_sentence_and_a_next_step() {
    let path = PathBuf::from("/tmp/a.txt");
    for kind in [
        ErrorKind::PermissionDenied,
        ErrorKind::NotFound,
        ErrorKind::AlreadyExists,
        ErrorKind::StorageFull,
        ErrorKind::ReadOnlyFilesystem,
        ErrorKind::CrossesDevices,
        ErrorKind::InvalidFilename,
        ErrorKind::DirectoryNotEmpty,
        ErrorKind::ResourceBusy,
        ErrorKind::TimedOut,
        ErrorKind::Interrupted,
        ErrorKind::BrokenPipe,
        ErrorKind::NotADirectory,
        ErrorKind::IsADirectory,
        ErrorKind::Other,
    ] {
        let text = describe_io(&err(kind), &path, Verb::Save);
        assert!(text.message.contains("a.txt"), "{kind:?}: {text:?}");
        assert!(text.message.starts_with("Could not save "), "{kind:?}: {text:?}");
        assert!(!text.hint.is_empty(), "{kind:?} has a next step");
        assert!(!text.detail.is_empty(), "{kind:?}: the detail has the system's text");
    }
}

#[test]
fn a_full_disk_says_to_free_space() {
    let text = describe_io(&err(ErrorKind::StorageFull), Path::new("/data/out.bin"), Verb::Write);
    assert!(text.message.contains("disk is full"), "{text:?}");
    assert_eq!(text.hint, "Free some space and try again.");
    assert_eq!(text.full(), format!("{} {}", text.message, text.hint));
}

#[test]
fn the_verb_picks_the_headline() {
    let p = Path::new("x");
    let head = |v| describe_io(&err(ErrorKind::NotFound), p, v).message;
    for (verb, start) in [
        (Verb::Read, "Could not read x."),
        (Verb::Write, "Could not write x."),
        (Verb::Save, "Could not save x."),
        (Verb::Create, "Could not create x."),
        (Verb::Delete, "Could not delete x."),
        (Verb::Rename, "Could not rename x."),
    ] {
        assert!(head(verb).starts_with(start), "{verb:?}");
    }
}

#[test]
fn turkish_and_emoji_file_names_are_kept_whole() {
    for name in ["/ev/Çalışma/İş Planı ğüşöç.md", "/tmp/📁/ünïcödé 🚀.txt"] {
        let text = describe_io(&err(ErrorKind::PermissionDenied), Path::new(name), Verb::Read);
        assert!(text.message.contains(name), "{text:?}");
        assert!(!text.detail.contains(name), "a path that fits is not repeated in the detail: {text:?}");
    }
}

#[test]
fn a_long_path_is_shortened_in_the_sentence_but_whole_in_the_detail() {
    let long = format!("/{}/file-ğ.txt", "klasör/".repeat(80));
    let text = describe_io(&err(ErrorKind::InvalidFilename), Path::new(&long), Verb::Create);
    assert!(text.message.contains('…'), "{text:?}");
    assert!(text.message.contains("file-ğ.txt"), "the file name stays");
    assert!(text.message.chars().count() < 400);
    assert!(text.detail.starts_with(&long), "the detail has the whole path");
    assert_eq!(shorten_path("/short"), "/short");
}

#[test]
fn a_known_cause_keeps_the_raw_text_in_the_detail_and_an_unknown_one_in_the_sentence_too() {
    let known = describe_io(&io::Error::from(ErrorKind::PermissionDenied), Path::new("/x"), Verb::Read);
    assert!(!known.message.contains("permission denied"), "{known:?}");
    assert!(known.detail.contains("permission denied"), "{known:?}");
    let raw = io::Error::other("weird driver failure 0x80070057");
    let text = describe_io(&raw, Path::new("/x"), Verb::Read);
    assert!(text.message.contains("weird driver failure 0x80070057"), "an unnamed cause is never hidden");
    assert!(text.detail.contains("weird driver failure 0x80070057"));
    assert!(text.cause.starts_with("The system reported an error"));
}

#[test]
fn a_name_cannot_reorder_hide_or_break_the_text() {
    // A right-to-left override makes "evil\u{202e}txt.exe" read as "evilexe.txt".
    for (name, shown) in [
        ("/tmp/evil\u{202e}txt.exe", "/tmp/evil\\u{202e}txt.exe"),
        ("/tmp/a\nb\rc\u{7}d", "/tmp/a\\u{a}b\\u{d}c\\u{7}d"),
        ("/tmp/\u{2066}x\u{2069}\u{200e}\u{200f}\u{200b}\u{feff}", "/tmp/\\u{2066}x\\u{2069}\\u{200e}\\u{200f}\\u{200b}\\u{feff}"),
    ] {
        let text = describe_io(&err(ErrorKind::PermissionDenied), Path::new(name), Verb::Save);
        assert!(text.message.contains(shown), "{text:?}");
        assert!(text.message.chars().all(|c| !is_unsafe(c)), "{text:?}");
        assert!(text.detail.chars().all(|c| !is_unsafe(c) || c == '\n'), "{text:?}");
    }
    // Text that is only unusual stays as it is.
    for name in ["/ev/\u{130}\u{15f} plan\u{131}.md", "/tmp/\u{1f680}/\u{1f1f9}\u{1f1f7}.txt", "/tmp/\u{5e9}\u{5dc}\u{5d5}\u{5dd}.txt", "/tmp/a b\tc"] {
        let want = if name.contains('\t') { name.replace('\t', "\\u{9}") } else { name.to_string() };
        assert_eq!(shorten_path(name), want);
    }
    // The raw text may have several lines (a remote command's stderr) but no hidden marks.
    assert_eq!(escape_unsafe("first\nsecond\u{202e}", true), "first\nsecond\\u{202e}");
    assert_eq!(escape_unsafe("first\nsecond", false), "first\\u{a}second");
}

#[test]
fn a_long_path_is_escaped_before_it_is_cut() {
    let long = format!("/{}\u{202e}/x.txt", "k".repeat(300));
    let text = describe_io(&err(ErrorKind::PermissionDenied), Path::new(&long), Verb::Save);
    assert!(text.message.contains('…') && text.message.chars().all(|c| !is_unsafe(c)), "{text:?}");
    assert!(text.detail.contains("\\u{202e}"), "the whole path, escaped, is in the detail");
}

#[test]
fn creating_or_renaming_where_the_folder_is_missing_says_so() {
    for verb in [Verb::Create, Verb::Rename] {
        let text = describe_io(&err(ErrorKind::NotFound), Path::new("/no/such/dir/new.txt"), verb);
        assert!(text.cause.contains("folder it belongs in"), "{text:?}");
        assert!(!text.cause.contains("moved or deleted"));
        assert!(text.hint.starts_with("Create the folder first"));
    }
    for verb in [Verb::Read, Verb::Save, Verb::Delete, Verb::Write] {
        let text = describe_io(&err(ErrorKind::NotFound), Path::new("/a/b.txt"), verb);
        assert!(text.cause.contains("moved or deleted"), "{verb:?}: {text:?}");
    }
}

#[cfg(unix)]
#[test]
fn not_permitted_is_not_the_same_as_no_permission() {
    let eperm = describe_io(&io::Error::from_raw_os_error(libc::EPERM), Path::new("/x"), Verb::Write);
    assert_eq!(classify(&io::Error::from_raw_os_error(libc::EPERM)), Cause::NotPermitted);
    assert!(eperm.cause.contains("protected, locked or sandboxed"), "{eperm:?}");
    assert!(!eperm.cause.contains("permission"), "{eperm:?}");
    assert_eq!(classify(&io::Error::from_raw_os_error(libc::EACCES)), Cause::Permission);
}

#[test]
fn remote_error_numbers_for_not_found_exists_and_not_permitted() {
    use host::HostError;
    let describe = |n: &str| describe_host(&HostError::Op(proto::Error::Other(format!("/x: whatever (os error {n})"))), Path::new("/x"), Verb::Save).cause;
    assert!(describe("2").contains("does not exist"));
    assert!(describe("17").contains("already exists"));
    assert!(describe("1").contains("protected, locked or sandboxed"));
    assert!(describe("13").contains("permission"));
}
