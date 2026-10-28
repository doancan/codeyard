use std::io::Write as _;

use crate::tar::{self, BLOCK};
use crate::*;

const MANIFEST: &str = r#"[extension]
id = "you.midnight"
version = "0.1.0"
name = "Midnight"
license = "MIT"

[[themes]]
label = "Midnight"
path = "themes/midnight.json"
"#;

fn file(path: &str, data: &str) -> Entry {
    Entry { path: path.into(), data: data.as_bytes().to_vec(), executable: false }
}

fn theme_files() -> Vec<Entry> {
    vec![file("themes/midnight.json", "{\"name\":\"Midnight\"}"), file(ext_manifest::FILE_NAME, MANIFEST), file("README.md", "# Midnight\n")]
}

fn gzip(tar: &[u8]) -> Vec<u8> {
    let mut gz = flate2::GzBuilder::new().mtime(0).write(Vec::new(), flate2::Compression::fast());
    gz.write_all(tar).unwrap();
    gz.finish().unwrap()
}

/// A tar written with `tar::write` but none of `write`'s checks, to build hostile packages.
fn raw_package(entries: &[Entry]) -> Vec<u8> {
    let mut out = Vec::new();
    tar::write(entries, &mut out).unwrap();
    gzip(&out)
}

/// The tar inside a package, to edit headers by hand.
fn untar(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(bytes), &mut out).unwrap();
    out
}

fn fix_checksum(h: &mut [u8]) {
    h[148..156].copy_from_slice(b"        ");
    let sum: u32 = h[..BLOCK].iter().map(|&b| u32::from(b)).sum();
    let s = format!("{sum:06o}\0 ");
    h[148..156].copy_from_slice(s.as_bytes());
}

fn with_contents(mut entries: Vec<Entry>) -> Vec<Entry> {
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let list = contents_list(&entries);
    entries.push(file(CONTENTS, &list));
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries
}

type Edit = Box<dyn Fn(&mut Vec<u8>)>;

fn err(bytes: &[u8]) -> String {
    read(bytes).expect_err("package should be refused").to_string()
}

#[test]
fn a_package_round_trips() {
    let (bytes, digest) = write(theme_files()).unwrap();
    let p = read(&bytes).unwrap();
    assert_eq!(p.digest, digest);
    assert_eq!(p.manifest.id.as_str(), "you.midnight");
    let paths: Vec<_> = p.files.keys().map(String::as_str).collect();
    assert_eq!(paths, vec!["CONTENTS.sha256", "README.md", "codeyard-extension.toml", "themes/midnight.json"]);
    let contents = String::from_utf8(p.files[CONTENTS].data.clone()).unwrap();
    assert!(contents.lines().all(|l| l.len() > 66 && l.as_bytes()[64] == b' '), "{contents}");
    assert!(!contents.contains(CONTENTS));
}

#[test]
fn packaging_is_reproducible() {
    let (a, da) = write(theme_files()).unwrap();
    let mut shuffled = theme_files();
    shuffled.reverse();
    let (b, db) = write(shuffled).unwrap();
    assert_eq!(a, b, "same files in another order give the same bytes");
    assert_eq!(da, db);
    let mut changed = theme_files();
    changed[2].data.push(b'!');
    assert_ne!(write(changed).unwrap().1, da, "different content, different digest");
}

#[test]
fn the_digest_ignores_how_the_archive_was_compressed() {
    let (bytes, digest) = write(theme_files()).unwrap();
    let recompressed = gzip(&untar(&bytes));
    assert_ne!(recompressed, bytes);
    assert_eq!(read(&recompressed).unwrap().digest, digest);
}

#[test]
fn digests_print_and_parse() {
    let d = write(theme_files()).unwrap().1;
    assert_eq!(ContentDigest::parse(&d.hex()), Some(d));
    assert_eq!(ContentDigest::parse(&d.hex().to_uppercase()), None);
    assert_eq!(ContentDigest::parse("abc"), None);
}

#[test]
fn executable_bits_survive() {
    let manifest = MANIFEST.replace("[[themes]]", "[permissions]\nrun = [\"bin/server\"]\nwhy = \"Starts its own server.\"\n\n[[themes]]");
    let mut files = theme_files();
    files[1] = file(ext_manifest::FILE_NAME, &manifest);
    files.push(Entry { path: "bin/server".into(), data: b"\x7fELF".to_vec(), executable: true });
    let p = read(&write(files.clone()).unwrap().0).unwrap();
    assert!(p.files["bin/server"].executable);
    assert!(!p.files["README.md"].executable);
    files.last_mut().unwrap().executable = false;
    assert!(write(files).unwrap_err().to_string().contains("not executable"));
}

#[test]
fn hostile_paths_are_refused() {
    for bad in ["../evil", "/etc/passwd", "themes/../../evil", "a\\b", "C:/x", "con.json", "a/b.", "x:y", "tab\there", "caf\u{e9}.json", "./x"] {
        let mut files = theme_files();
        files.push(file(bad, "x"));
        let e = write(files).expect_err(bad).to_string();
        assert!(e.contains(bad.trim_start_matches("./")) || e.contains("ASCII") || e.contains('\''), "{bad}: {e}");
        // And the reader refuses the same file when a hostile writer puts it there.
        let mut entries = with_contents(theme_files());
        entries.push(file(bad, "x"));
        assert!(read(&raw_package(&entries)).is_err(), "{bad}");
    }
}

#[test]
fn case_collisions_and_file_folder_clashes_are_refused() {
    let mut files = theme_files();
    files.push(file("Readme.md", "other"));
    assert!(write(files).unwrap_err().to_string().contains("same file on macOS and Windows"));
    let mut files = theme_files();
    files.push(file("README.md/inner", "x"));
    assert!(write(files).unwrap_err().to_string().contains("which is a file"));
    let mut entries = with_contents(theme_files());
    entries.push(file("THEMES/midnight.json", "x"));
    assert!(err(&raw_package(&entries)).contains("same file"));
}

#[test]
fn duplicate_entries_are_refused() {
    let mut entries = with_contents(theme_files());
    entries.push(file("README.md", "second"));
    assert!(err(&raw_package(&entries)).contains("same file"));
}

#[test]
fn links_folders_and_devices_are_refused() {
    let entries = with_contents(theme_files());
    for (flag, word) in [
        (b'2', "link"),
        (b'1', "link"),
        (b'5', "folder"),
        (b'3', "entry type"),
        (b'4', "entry type"),
        (b'6', "entry type"),
        (b'x', "extension"),
        (b'L', "extension"),
    ] {
        let mut t = Vec::new();
        tar::write(&entries, &mut t).unwrap();
        t[156] = flag;
        fix_checksum(&mut t[..BLOCK]);
        assert!(err(&gzip(&t)).contains(word), "{}", flag as char);
    }
}

#[test]
fn headers_are_checked_strictly() {
    let entries = with_contents(theme_files());
    let mut good = Vec::new();
    tar::write(&entries, &mut good).unwrap();
    let edits: [(&str, Edit); 6] = [
        ("checksum", Box::new(|t| t[0] ^= 1)),
        (
            "mode",
            Box::new(|t| {
                t[100..108].copy_from_slice(b"0004755\0");
                fix_checksum(&mut t[..BLOCK]);
            }),
        ),
        (
            "ustar",
            Box::new(|t| {
                t[257] = b'X';
                fix_checksum(&mut t[..BLOCK]);
            }),
        ),
        (
            "link name",
            Box::new(|t| {
                t[157] = b'a';
                fix_checksum(&mut t[..BLOCK]);
            }),
        ),
        (
            "octal",
            Box::new(|t| {
                t[124] = b'9';
                fix_checksum(&mut t[..BLOCK]);
            }),
        ),
        (
            "after the end",
            Box::new(|t| {
                let n = t.len();
                t[n - 1] = 1;
            }),
        ),
    ];
    for (what, edit) in edits {
        let mut t = good.clone();
        edit(&mut t);
        assert!(read(&gzip(&t)).is_err(), "{what}");
    }
}

#[test]
fn contents_must_match() {
    let mut entries = with_contents(theme_files());
    entries.iter_mut().find(|e| e.path == "README.md").unwrap().data = b"changed".to_vec();
    assert!(err(&raw_package(&entries)).contains("does not match"));
    let entries: Vec<_> = with_contents(theme_files()).into_iter().filter(|e| e.path != CONTENTS).collect();
    assert!(err(&raw_package(&entries)).contains("no CONTENTS.sha256"));
}

#[test]
fn the_manifest_must_be_valid_and_its_files_present() {
    let files: Vec<_> = theme_files().into_iter().filter(|e| e.path != "themes/midnight.json").collect();
    let e = write(files).unwrap_err();
    assert_eq!(e.to_string(), "themes.path points to 'themes/midnight.json', which is not in the package");
    let files: Vec<_> = theme_files().into_iter().filter(|e| e.path != ext_manifest::FILE_NAME).collect();
    assert!(matches!(write(files), Err(Error::Manifest(_))));
    let mut files = theme_files();
    files[1] = file(ext_manifest::FILE_NAME, &MANIFEST.replace("you.midnight", "Bad"));
    assert!(write(files).unwrap_err().to_string().contains("CY0103"));
}

#[test]
fn trailing_data_and_second_members_are_refused() {
    let (mut bytes, _) = write(theme_files()).unwrap();
    let mut second = bytes.clone();
    second.extend(gzip(b"hidden"));
    assert!(err(&second).contains("after the gzip stream"));
    bytes.push(0);
    assert!(err(&bytes).contains("after the gzip stream"));
    assert!(err(b"PK\x03\x04").contains("no gzip header"));
    assert!(read(&[0x1f, 0x8b, 8, 0]).is_err());
}

#[test]
fn bombs_stop_at_the_limit() {
    // 300 MiB of zeros compresses to about 300 KiB; reading must stop at the unpacked limit, not allocate it all.
    let mut gz = flate2::GzBuilder::new().write(Vec::new(), flate2::Compression::fast());
    let zeros = vec![0u8; 1 << 20];
    for _ in 0..300 {
        gz.write_all(&zeros).unwrap();
    }
    let bomb = gz.finish().unwrap();
    assert!(bomb.len() < 2 << 20);
    assert!(err(&bomb).contains("unpacks to more than"));
}

#[test]
fn long_paths_use_the_prefix_field() {
    let deep = format!("{}/{}.json", "d".repeat(120), "f".repeat(90));
    let mut files = theme_files();
    files.push(file(&deep, "{}"));
    let p = read(&write(files).unwrap().0).unwrap();
    assert!(p.files.contains_key(&deep));
    let mut files = theme_files();
    files.push(file(&"x".repeat(101), "{}"));
    assert!(write(files).unwrap_err().to_string().contains("too long"));
}

#[test]
fn folders_are_collected_without_litter_or_links() {
    let dir = std::env::temp_dir().join(format!("ext-pkg-collect-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("themes")).unwrap();
    std::fs::create_dir_all(dir.join(".git/objects")).unwrap();
    std::fs::create_dir_all(dir.join("node_modules/x")).unwrap();
    std::fs::write(dir.join(ext_manifest::FILE_NAME), MANIFEST).unwrap();
    std::fs::write(dir.join("themes/midnight.json"), "{}").unwrap();
    std::fs::write(dir.join(".git/objects/a"), "x").unwrap();
    std::fs::write(dir.join("node_modules/x/i.js"), "x").unwrap();
    std::fs::write(dir.join(".DS_Store"), "x").unwrap();
    let files = collect_dir(&dir).unwrap();
    let paths: Vec<_> = files.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, vec![ext_manifest::FILE_NAME, "themes/midnight.json"]);
    assert!(read(&write(files).unwrap().0).is_ok());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/etc/passwd", dir.join("themes/link.json")).unwrap();
        assert!(collect_dir(&dir).unwrap_err().to_string().contains("symbolic link"));
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
