//! Tests for `update.rs`: no real network (a signed release is written to a temp folder and fetched through `file://`),
//! a throw-away signing key, no real state folder.

use std::sync::atomic::AtomicU32;

use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};

use super::*;

const DMG: &str = "macos-arm64.dmg";

/// A fresh folder under the temp dir, removed when the guard drops.
pub(crate) fn scratch(name: &str) -> (PathBuf, RemoveOnDrop) {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!("codeyard-update-{name}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&dir).unwrap();
    (dir.clone(), RemoveOnDrop::new(dir))
}

fn keypair() -> Ed25519KeyPair {
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
    Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap()
}

/// A release folder as the release workflow lays it out, signed with a key only this test knows.
struct Fixture {
    dir: PathBuf,
    key: Ed25519KeyPair,
    _guard: RemoveOnDrop,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let (dir, _guard) = scratch(name);
        Fixture { dir, key: keypair(), _guard }
    }

    fn cfg(&self) -> Config {
        Config {
            base: format!("file://{}", self.dir.display()),
            pin: None,
            curl: system_tool("curl"),
            keys: vec![self.key.public_key().as_ref().try_into().unwrap()],
            suffix: Some(DMG),
            local: true,
            installer_max: 1 << 20,
        }
    }

    fn put(&self, name: &str, data: &[u8]) {
        std::fs::write(self.dir.join(name), data).unwrap();
    }

    fn read(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.dir.join(name)).unwrap()
    }

    /// Writes `SHA256SUMS.txt` and its signature from literal text.
    fn sign(&self, sums: &[u8]) {
        self.put("SHA256SUMS.txt", sums);
        self.put("SHA256SUMS.txt.sig", self.key.sign(sums).as_ref());
    }

    /// Writes the files and a signed list of exactly those.
    fn publish_files(&self, files: &[(String, &[u8])]) {
        let mut sums = String::new();
        for (name, data) in files {
            self.put(name, data);
            sums += &format!("{}  {name}\n", sha256_hex(data));
        }
        self.sign(sums.as_bytes());
    }

    fn publish_with_notes(&self, version: &str, installer: &[u8], notes: &[u8]) {
        let f = |suffix: &str| format!("Codeyard-{version}-{suffix}");
        self.publish_files(&[
            (f(DMG), installer),
            (format!("codeyard-{version}-macos-arm64.tar.gz"), b"tar"),
            (f("windows-x64-setup.exe"), b"exe"),
            (f("windows-x64.zip"), b"zip"),
            ("RELEASE_NOTES.md".into(), notes),
        ]);
    }

    fn publish(&self, version: &str, installer: &[u8]) {
        self.publish_with_notes(version, installer, b"# What is new\n\n- Updates\n");
    }
}

fn check(f: &Fixture, current: &str) -> Result<Option<Release>, UpdateError> {
    check_with(&f.cfg(), current)
}

fn download(f: &Fixture, release: &Release) -> Result<Downloaded, UpdateError> {
    download_in(&f.cfg(), &f.dir.join("dl"), "0.1.0", release, &mut |_| {}, &AtomicBool::new(false))
}

fn err<T: std::fmt::Debug>(r: Result<T, UpdateError>) -> UpdateError {
    r.expect_err("expected an error")
}

#[test]
fn update_happy_path_checks_downloads_and_verifies() {
    let f = Fixture::new("happy");
    f.publish("0.2.0", b"installer bytes");
    let release = check(&f, "0.1.0").unwrap().expect("a newer release");
    assert_eq!(release.version, "0.2.0");
    assert_eq!(release.notes, "# What is new\n\n- Updates\n");
    let asset = release.asset.clone().unwrap();
    assert_eq!(asset.name, "Codeyard-0.2.0-macos-arm64.dmg");
    assert_eq!(asset.sha256, sha256_hex(b"installer bytes"));
    assert!(asset.url.ends_with("/Codeyard-0.2.0-macos-arm64.dmg"));

    let mut seen = Vec::new();
    let dl = download_in(&f.cfg(), &f.dir.join("dl"), "0.1.0", &release, &mut |p| seen.push(p), &AtomicBool::new(false)).unwrap();
    assert_eq!(std::fs::read(dl.path()).unwrap(), b"installer bytes");
    assert_eq!(*seen.last().unwrap(), Progress { done: 15, total: Some(15) });
    assert!(seen.windows(2).all(|w| w[0].done <= w[1].done));
    assert_eq!(dl.release, release);
}

#[test]
fn update_equal_and_older_releases_are_not_offered() {
    let f = Fixture::new("notnewer");
    f.publish("0.10.0", b"x");
    assert_eq!(check(&f, "0.10.0").unwrap(), None, "equal is not newer");
    assert_eq!(check(&f, "0.11.0").unwrap(), None, "never a downgrade");
    assert!(check(&f, "0.9.0").unwrap().is_some(), "0.9.0 < 0.10.0 numerically, not as text");
    assert!(Version::parse("0.9.0").unwrap() < Version::parse("0.10.0").unwrap());
    assert!(Version::parse("1.0.0").unwrap() > Version::parse("0.99.99").unwrap());
}

#[test]
fn update_download_refuses_a_release_that_is_not_newer() {
    let f = Fixture::new("downgrade");
    f.publish("0.2.0", b"x");
    let release = check(&f, "0.1.0").unwrap().unwrap();
    for current in ["0.2.0", "0.3.0"] {
        let e = err(download_in(&f.cfg(), &f.dir.join("dl"), current, &release, &mut |_| {}, &AtomicBool::new(false)));
        assert!(matches!(e, UpdateError::NotNewer { .. }), "{e}");
        assert_eq!(e.kind(), ErrorKind::Untrusted);
    }
    assert!(!f.dir.join("dl").exists(), "nothing was fetched");
}

#[test]
fn update_bogus_signature_is_rejected() {
    let f = Fixture::new("badsig");
    f.publish("0.2.0", b"x");
    let other = keypair();
    f.put("SHA256SUMS.txt.sig", other.sign(&f.read("SHA256SUMS.txt")).as_ref());
    assert!(matches!(err(check(&f, "0.1.0")), UpdateError::BadSignature));
    f.put("SHA256SUMS.txt.sig", &[0; 64]);
    assert!(matches!(err(check(&f, "0.1.0")), UpdateError::BadSignature));
    f.put("SHA256SUMS.txt.sig", &f.key.sign(b"something else").as_ref()[..63]);
    assert!(matches!(err(check(&f, "0.1.0")), UpdateError::BadSignature), "a short signature");
    f.put("SHA256SUMS.txt.sig", &[f.key.sign(&f.read("SHA256SUMS.txt")).as_ref(), &[0]].concat());
    assert!(matches!(err(check(&f, "0.1.0")), UpdateError::TooLarge { .. }), "a long signature");
}

#[test]
fn update_a_key_that_is_not_trusted_is_rejected() {
    let f = Fixture::new("untrusted");
    f.publish("0.2.0", b"x");
    let mut cfg = f.cfg();
    cfg.keys = vec![[7; 32]];
    assert!(matches!(err(check_with(&cfg, "0.1.0")), UpdateError::BadSignature));
    cfg.keys.push(f.key.public_key().as_ref().try_into().unwrap());
    assert!(check_with(&cfg, "0.1.0").unwrap().is_some(), "a second key in the list is enough (rotation)");
}

#[test]
fn update_tampered_sums_line_is_a_signature_failure_not_a_parse_failure() {
    let f = Fixture::new("tamper-sums");
    f.publish("0.2.0", b"x");
    let sums = String::from_utf8(f.read("SHA256SUMS.txt")).unwrap();
    let tampered = sums.replacen(&sha256_hex(b"x")[..1], if sha256_hex(b"x").starts_with('0') { "1" } else { "0" }, 1);
    assert_ne!(tampered, sums);
    f.put("SHA256SUMS.txt", tampered.as_bytes());
    assert!(matches!(err(check(&f, "0.1.0")), UpdateError::BadSignature));
    // Unsigned garbage must not reach the parser either.
    f.put("SHA256SUMS.txt", b"not a list\n");
    assert!(matches!(err(check(&f, "0.1.0")), UpdateError::BadSignature));
}

#[test]
fn update_signed_but_malformed_sums_are_rejected_by_the_parser() {
    let f = Fixture::new("badsums");
    let h = "a".repeat(64);
    for (text, why) in [
        (format!("{h} Codeyard-0.2.0-macos-arm64.dmg\n"), "one space"),
        (format!("{}  Codeyard-0.2.0-macos-arm64.dmg\n", "z".repeat(64)), "not hex"),
        (format!("{h}  a/b\n"), "path separator"),
        (format!("{}  Codeyard-0.2.0-macos-arm64.dmg\n", "A".repeat(64)), "upper-case hex: the signing scripts write lower case"),
        (format!("{h}  Codeyard-0.2.0-macos-arm64.dmg\r\n"), "CRLF"),
        (format!("{h} *Codeyard-0.2.0-macos-arm64.dmg\n"), "binary-mode marker"),
        (format!("{h}  Codeyard 0.2.0-macos-arm64.dmg\n"), "space in a name"),
        (format!("{h}   Codeyard-0.2.0-macos-arm64.dmg\n"), "three spaces"),
        (format!("{h}  x\n{h}  x\n"), "duplicate"),
        (format!("{h}  Codeyard-0.2.0-macos-arm64.dmg\n").replace("0.2.0", "0.2.0\u{7}"), "control character"),
        (format!("{h}  something-else.txt\n"), "no release files"),
    ] {
        f.sign(text.as_bytes());
        let e = err(check(&f, "0.1.0"));
        assert!(matches!(e, UpdateError::BadSums(_) | UpdateError::BadVersion(_)), "{why}: {e}");
    }
    f.sign(&[0xff, 0xfe]);
    assert!(matches!(err(check(&f, "0.1.0")), UpdateError::NotText(_)));
}

#[test]
fn update_malformed_versions_are_rejected() {
    let f = Fixture::new("badversion");
    for v in ["1.2", "1.2.x", "1.2.3-rc1", "01.2.3", "1.2.3.4", "1.2.-3", "+1.2.3", "1..3", "", "99999999999.0.0"] {
        f.publish_files(&[(format!("Codeyard-{v}-{DMG}"), b"x"), ("RELEASE_NOTES.md".into(), b"n")]);
        let e = err(check(&f, "0.1.0"));
        // "+1.2.3" never gets as far as the version: `+` is not a character of a file name.
        assert!(matches!(e, UpdateError::BadVersion(_) | UpdateError::BadSums(_)), "{v:?}: {e}");
    }
    f.publish_files(&[(format!("Codeyard-0.2.0-{DMG}"), b"x"), ("Codeyard-0.3.0-windows-x64.zip".into(), b"y")]);
    assert!(matches!(err(check(&f, "0.1.0")), UpdateError::BadVersion(_)), "assets of two versions in one release");
    assert!(matches!(err(check(&f, "0.1.0-dev")), UpdateError::BadVersion(_)), "our own version must parse too");
}

#[test]
fn update_oversized_files_are_rejected() {
    let f = Fixture::new("oversize");
    f.sign(&vec![b'a'; 64 * 1024 + 1]);
    assert!(matches!(err(check(&f, "0.1.0")), UpdateError::TooLarge { limit: 65536, .. }));

    f.publish_with_notes("0.2.0", b"x", &vec![b'n'; 256 * 1024 + 1]);
    let e = err(check(&f, "0.1.0"));
    assert!(matches!(&e, UpdateError::TooLarge { file, .. } if file == "RELEASE_NOTES.md"), "{e}");

    f.publish("0.2.0", &[0; 100]);
    let mut cfg = f.cfg();
    let release = check_with(&cfg, "0.1.0").unwrap().unwrap();
    cfg.installer_max = 10;
    let e = err(download_in(&cfg, &f.dir.join("dl"), "0.1.0", &release, &mut |_| {}, &AtomicBool::new(false)));
    assert!(matches!(&e, UpdateError::TooLarge { limit: 10, .. }), "{e}");
    assert!(!f.dir.join("dl").exists(), "the partial file is gone");
}

#[test]
fn update_tampered_notes_and_installer_are_rejected() {
    let f = Fixture::new("tamper-files");
    f.publish("0.2.0", b"good");
    f.put("RELEASE_NOTES.md", b"# evil notes");
    let e = err(check(&f, "0.1.0"));
    assert!(matches!(&e, UpdateError::HashMismatch(n) if n == "RELEASE_NOTES.md"), "{e}");

    f.publish("0.2.0", b"good");
    let release = check(&f, "0.1.0").unwrap().unwrap();
    f.put("Codeyard-0.2.0-macos-arm64.dmg", b"evil");
    let e = err(download(&f, &release));
    assert!(matches!(&e, UpdateError::HashMismatch(n) if n.ends_with(".dmg")), "{e}");
    assert_eq!(e.kind(), ErrorKind::Untrusted);
    assert!(!f.dir.join("dl").exists(), "the rejected file is removed");
}

#[test]
fn update_notes_must_be_utf8_and_listed() {
    let f = Fixture::new("notes");
    f.publish_with_notes("0.2.0", b"x", &[0xff, 0xfe, 0x00]);
    assert!(matches!(err(check(&f, "0.1.0")), UpdateError::NotText(_)));

    // Served but not in the signed list: not trusted.
    f.publish_files(&[(format!("Codeyard-0.2.0-{DMG}"), b"x")]);
    f.put("RELEASE_NOTES.md", b"unsigned notes");
    assert!(matches!(err(check(&f, "0.1.0")), UpdateError::MissingEntry(n) if n == "RELEASE_NOTES.md"));
}

#[test]
fn update_a_release_without_an_installer_for_this_system_is_an_update_without_asset() {
    let f = Fixture::new("noasset");
    // A Windows-only release read by a Mac: still a release, the UI points to its page.
    f.publish_files(&[
        ("Codeyard-0.2.0-windows-x64-setup.exe".into(), b"e"),
        ("Codeyard-0.2.0-windows-x64.zip".into(), b"z"),
        ("RELEASE_NOTES.md".into(), b"n"),
    ]);
    let release = check(&f, "0.1.0").unwrap().expect("the release is offered");
    assert_eq!((release.version.as_str(), release.asset.clone(), release.notes.as_str()), ("0.2.0", None, "n"));
    let e = err(download_in(&f.cfg(), &f.dir.join("dl"), "0.1.0", &release, &mut |_| {}, &AtomicBool::new(false)));
    assert!(matches!(e, UpdateError::NoInstaller));
    // The same list on a system that has none at all (Linux).
    let mut cfg = f.cfg();
    cfg.suffix = None;
    assert_eq!(check_with(&cfg, "0.1.0").unwrap().unwrap().asset, None);
    // A Mac-only release read by Windows.
    f.publish_files(&[("Codeyard-0.2.0-macos-arm64.dmg".into(), b"d"), ("RELEASE_NOTES.md".into(), b"n")]);
    cfg.suffix = Some("windows-x64-setup.exe");
    assert_eq!(check_with(&cfg, "0.1.0").unwrap().unwrap().asset, None);
}

#[test]
fn update_a_signed_list_with_no_installer_of_any_platform_is_suspicious() {
    let f = Fixture::new("noinstaller");
    // Archives and notes only: not something our release workflow makes.
    f.publish_files(&[("Codeyard-0.2.0-windows-x64.zip".into(), b"z"), ("codeyard-0.2.0-macos-arm64.tar.gz".into(), b"t"), ("RELEASE_NOTES.md".into(), b"n")]);
    let e = err(check(&f, "0.1.0"));
    assert!(matches!(&e, UpdateError::MissingEntry(n) if n.contains("an installer")), "{e}");
    assert_eq!(e.kind(), ErrorKind::Untrusted);
    // The notes must be in the signed list in any case.
    f.publish_files(&[(format!("Codeyard-0.2.0-{DMG}"), b"d")]);
    f.put("RELEASE_NOTES.md", b"unsigned");
    assert!(matches!(err(check(&f, "0.1.0")), UpdateError::MissingEntry(n) if n == "RELEASE_NOTES.md"));
}

#[test]
fn update_a_missing_file_is_a_network_error_the_user_can_retry() {
    let f = Fixture::new("missing");
    let e = err(check(&f, "0.1.0"));
    assert!(matches!(e, UpdateError::Network(_)), "{e}");
    assert_eq!(e.kind(), ErrorKind::Network);
}

#[test]
fn update_cancel_stops_the_download_and_leaves_nothing() {
    let f = Fixture::new("cancel");
    f.publish("0.2.0", b"x");
    let release = check(&f, "0.1.0").unwrap().unwrap();
    let cancel = AtomicBool::new(true);
    let e = err(download_in(&f.cfg(), &f.dir.join("dl"), "0.1.0", &release, &mut |_| {}, &cancel));
    assert!(matches!(e, UpdateError::Cancelled));
    assert_eq!(e.kind(), ErrorKind::Cancelled);
    assert!(!f.dir.join("dl").exists());
}

#[test]
fn update_download_clears_the_previous_attempt() {
    let f = Fixture::new("fresh");
    f.publish("0.2.0", b"x");
    let release = check(&f, "0.1.0").unwrap().unwrap();
    std::fs::create_dir_all(f.dir.join("dl")).unwrap();
    std::fs::write(f.dir.join("dl/stale.dmg"), b"old").unwrap();
    let dl = download(&f, &release).unwrap();
    assert!(!f.dir.join("dl/stale.dmg").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(dl.path().parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
    }
}

#[test]
fn update_default_source_is_the_repository_over_https_and_curl_is_locked_down() {
    let cfg = Config::new(None);
    assert_eq!(cfg.base, "https://github.com/doancan/codeyard/releases/latest/download");
    assert!(!cfg.local);
    assert_eq!(cfg.keys, TRUSTED_KEYS);
    assert_eq!(cfg.url("SHA256SUMS.txt"), "https://github.com/doancan/codeyard/releases/latest/download/SHA256SUMS.txt");
    let args: Vec<String> = curl(&cfg).get_args().map(|a| a.to_string_lossy().into_owned()).collect();
    assert_eq!(args[0], "-q", "ignore ~/.curlrc");
    let pair = |k: &str, v: &str| args.windows(2).any(|w| w[0] == k && w[1] == v);
    assert!(pair("--proto", "=https") && pair("--proto-redir", "=https") && pair("--max-redirs", "5"), "{args:?}");
    assert!(args.contains(&"--tlsv1.2".into()) && args.contains(&"--fail".into()));

    let local = Config::new(Some("http://127.0.0.1:1/x/".into()));
    assert!(local.local);
    assert_eq!(local.base, "http://127.0.0.1:1/x", "trailing slash dropped");
    assert_eq!(local.keys, TRUSTED_KEYS, "the keys cannot be overridden");
}

#[test]
fn update_trusted_key_list_is_not_empty_and_decodes() {
    assert!(!TRUSTED_KEYS.is_empty());
    assert_eq!(TRUSTED_KEYS[0][0], 0x47);
    assert_eq!(TRUSTED_KEYS[0][31], 0x7c);
}

#[test]
fn update_content_length_comes_from_the_last_successful_response() {
    let redirect = "HTTP/2 302\r\nlocation: https://x/y\r\ncontent-length: 0\r\n\r\n";
    assert_eq!(final_content_length(redirect), None, "still redirecting");
    assert_eq!(final_content_length(&format!("{redirect}HTTP/2 200\r\nContent-Length: 1234\r\n\r\n")), Some(1234));
    assert_eq!(final_content_length("HTTP/1.1 200 OK\ncontent-type: x\n\n"), None, "no length sent");
    assert_eq!(final_content_length(""), None);
}

#[test]
fn update_policy_parses_and_prints() {
    for p in [Policy::Ask, Policy::Daily, Policy::Manual] {
        assert_eq!(p.as_str().parse::<Policy>(), Ok(p));
    }
    assert_eq!("Daily".parse::<Policy>(), Err(UnknownPolicy));
    assert_eq!("".parse::<Policy>(), Err(UnknownPolicy));
}

#[test]
fn update_daily_check_is_due_after_a_day_and_handles_a_backwards_clock() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10 * 86_400);
    let ago = |s| Some(now - Duration::from_secs(s));
    assert!(should_check_now(None, now, Policy::Daily));
    assert!(!should_check_now(ago(60), now, Policy::Daily));
    assert!(!should_check_now(ago(86_399), now, Policy::Daily));
    assert!(should_check_now(ago(86_400), now, Policy::Daily));
    assert!(should_check_now(Some(now + Duration::from_secs(5)), now, Policy::Daily), "clock went backwards: check and re-record");
    for p in [Policy::Ask, Policy::Manual] {
        assert!(!should_check_now(None, now, p) && !should_check_now(ago(10 * 86_400), now, p), "{p:?} never checks by itself");
    }
}

#[test]
fn update_errors_read_in_plain_english_and_carry_a_kind() {
    use UpdateError::*;
    let io = || io::Error::other("disk full");
    let cases = [
        (Network("timed out".into()), ErrorKind::Network),
        (Http { file: "a".into(), status: 503 }, ErrorKind::Network),
        (Tls("certificate has expired".into()), ErrorKind::Network),
        (NoRelease, ErrorKind::NoRelease),
        (Unsigned, ErrorKind::Untrusted),
        (NoSpace, ErrorKind::Install),
        (BadSignature, ErrorKind::Untrusted),
        (HashMismatch("a.dmg".into()), ErrorKind::Untrusted),
        (TooLarge { file: "a.dmg".into(), limit: 400 << 20 }, ErrorKind::Untrusted),
        (NotNewer { offered: "0.1.0".into(), current: "0.2.0".into() }, ErrorKind::Untrusted),
        (NoWritePermission("/Applications".into()), ErrorKind::Install),
        (BundleMismatch("wrong".into()), ErrorKind::Install),
        (MountFailed("busy".into()), ErrorKind::Install),
        (RollbackFailed { app: "/A.app".into(), backup: "/.old.app".into(), cause: io() }, ErrorKind::Install),
        (Cancelled, ErrorKind::Cancelled),
    ];
    for (e, kind) in cases {
        assert_eq!(e.kind(), kind, "{e:?}");
        let text = e.to_string();
        assert!(text.ends_with('.') || text.contains(": "), "{text}");
        assert!(!text.contains("{") && !text.contains("Error"), "{text}");
    }
    assert!(RollbackFailed { app: "/A.app".into(), backup: "/.old.app".into(), cause: io() }.to_string().contains("/.old.app"));
}

/// The key the app trusts and the key the release workflow verifies with must be the same one: a mismatch would make every
/// installed app reject every update, silently.
#[test]
fn the_embedded_key_is_the_one_the_release_scripts_verify_with() {
    fn base64(text: &str) -> Vec<u8> {
        let mut out = Vec::new();
        let (mut acc, mut bits) = (0u32, 0);
        for c in text.bytes().filter(|c| *c != b'=') {
            let v = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/".iter().position(|x| *x == c).expect("base64 character");
            acc = (acc << 6) | v as u32;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
                acc &= (1 << bits) - 1;
            }
        }
        out
    }
    let pem = include_str!("../../../scripts/update-public-key.pem");
    let body: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
    let der = base64(&body);
    // SubjectPublicKeyInfo of an Ed25519 key: a 12-byte header, then the 32 raw bytes.
    assert_eq!(der.len(), 44, "unexpected key encoding");
    assert_eq!(der[..12], [0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00]);
    assert_eq!(TRUSTED_KEYS[0], der[12..], "scripts/update-public-key.pem and TRUSTED_KEYS differ");
}

// --- Against a local HTTP server and a scripted curl ---

use std::collections::HashMap;
use std::io::Write as _;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

/// Answers GET requests from a table of `path -> (status, body)` (anything else is 404) and writes down every path asked for.
struct Server {
    base: String,
    asked: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    fn start(routes: HashMap<String, (u16, Vec<u8>)>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (asked, stop) = (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicBool::new(false)));
        let (log, halt) = (asked.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if halt.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(mut stream) = stream else { continue };
                let mut head = [0u8; 2048];
                let n = io::Read::read(&mut stream, &mut head).unwrap_or(0);
                let request = String::from_utf8_lossy(&head[..n]).into_owned();
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                log.lock().unwrap().push(path.clone());
                let (status, body) = routes.get(&path).cloned().unwrap_or((404, b"not found".to_vec()));
                let _ = write!(stream, "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(&body);
            }
        });
        Server { base, asked, stop, thread: Some(thread) }
    }

    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wakes the accept loop so that it can see the flag.
        let _ = std::net::TcpStream::connect(self.base.trim_start_matches("http://"));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A signed release as routes: the list and signature under `latest/`, every other file under `<pin>/v<version>/`.
fn routes_for(f: &Fixture, version: &str, files: &[(&str, &[u8])]) -> HashMap<String, (u16, Vec<u8>)> {
    let mut routes = HashMap::new();
    let mut sums = String::new();
    for (name, data) in files {
        sums += &format!("{}  {name}\n", sha256_hex(data));
        routes.insert(format!("/dl/v{version}/{name}"), (200, data.to_vec()));
    }
    routes.insert("/latest/SHA256SUMS.txt".into(), (200, sums.clone().into_bytes()));
    routes.insert("/latest/SHA256SUMS.txt.sig".into(), (200, f.key.sign(sums.as_bytes()).as_ref().to_vec()));
    routes
}

fn served(f: &Fixture, server: &Server) -> Config {
    let mut cfg = f.cfg();
    cfg.base = format!("{}/latest", server.base);
    cfg.pin = Some(format!("{}/dl", server.base));
    cfg
}

#[test]
fn update_notes_and_installer_come_from_the_tag_of_the_verified_list() {
    let f = Fixture::new("pinned");
    let files: [(&str, &[u8]); 2] = [("Codeyard-0.2.0-macos-arm64.dmg", b"installer"), ("RELEASE_NOTES.md", b"notes")];
    let server = Server::start(routes_for(&f, "0.2.0", &files));
    let cfg = served(&f, &server);
    let release = check_with(&cfg, "0.1.0").unwrap().expect("an update");
    // `latest` has no notes and no installer: they exist only under the tag, so a check that asked `latest` for them would fail.
    assert_eq!(release.notes, "notes");
    let asset = release.asset.clone().unwrap();
    assert_eq!(asset.url, format!("{}/dl/v0.2.0/Codeyard-0.2.0-macos-arm64.dmg", server.base));
    let dl = download_in(&cfg, &f.dir.join("dl"), "0.1.0", &release, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(std::fs::read(dl.path()).unwrap(), b"installer");
    assert_eq!(
        server.asked(),
        ["/latest/SHA256SUMS.txt", "/latest/SHA256SUMS.txt.sig", "/dl/v0.2.0/RELEASE_NOTES.md", "/dl/v0.2.0/Codeyard-0.2.0-macos-arm64.dmg"]
    );
}

#[test]
fn update_the_real_configuration_pins_to_the_tag_and_a_local_one_does_not() {
    let cfg = Config::new(None);
    let v = Version::parse("0.2.0").unwrap();
    assert_eq!(cfg.release_url(v, "RELEASE_NOTES.md"), "https://github.com/doancan/codeyard/releases/download/v0.2.0/RELEASE_NOTES.md");
    assert_eq!(cfg.url("SHA256SUMS.txt"), "https://github.com/doancan/codeyard/releases/latest/download/SHA256SUMS.txt");
    let local = Config::new(Some("http://127.0.0.1:1/x".into()));
    assert_eq!(local.release_url(v, "RELEASE_NOTES.md"), "http://127.0.0.1:1/x/RELEASE_NOTES.md", "a mirror serves one flat folder");
}

#[test]
fn update_a_release_without_a_signature_is_unsigned_not_a_network_problem() {
    let f = Fixture::new("unsigned");
    let mut routes = routes_for(&f, "0.2.0", &[("Codeyard-0.2.0-macos-arm64.dmg", b"i"), ("RELEASE_NOTES.md", b"n")]);
    routes.remove("/latest/SHA256SUMS.txt.sig"); // what v0.1.0 looks like on GitHub today
    let server = Server::start(routes);
    let e = err(check_with(&served(&f, &server), "0.1.0"));
    assert!(matches!(e, UpdateError::Unsigned), "{e}");
    assert_eq!(e.kind(), ErrorKind::Untrusted);
    assert!(e.to_string().contains("isn't signed"), "{e}");
    // 410 Gone says the same; a server error does not: that one is worth another try.
    let mut gone = routes_for(&f, "0.2.0", &[("Codeyard-0.2.0-macos-arm64.dmg", b"i"), ("RELEASE_NOTES.md", b"n")]);
    gone.insert("/latest/SHA256SUMS.txt.sig".into(), (410, vec![]));
    assert!(matches!(err(check_with(&served(&f, &Server::start(gone)), "0.1.0")), UpdateError::Unsigned));
    let mut broken = routes_for(&f, "0.2.0", &[("Codeyard-0.2.0-macos-arm64.dmg", b"i"), ("RELEASE_NOTES.md", b"n")]);
    broken.insert("/latest/SHA256SUMS.txt.sig".into(), (503, vec![]));
    let e = err(check_with(&served(&f, &Server::start(broken)), "0.1.0"));
    assert!(matches!(&e, UpdateError::Http { status: 503, .. }), "{e}");
    assert_eq!(e.kind(), ErrorKind::Network);
}

#[test]
fn update_a_repository_without_a_checksum_list_has_no_release_to_look_at() {
    let f = Fixture::new("norelease");
    let server = Server::start(HashMap::new());
    let e = err(check_with(&served(&f, &server), "0.1.0"));
    assert!(matches!(e, UpdateError::NoRelease), "{e}");
    assert_eq!(e.kind(), ErrorKind::NoRelease);
}

#[test]
fn update_a_server_error_for_the_list_is_a_network_error() {
    let f = Fixture::new("http500");
    let server = Server::start(HashMap::from([("/latest/SHA256SUMS.txt".to_string(), (500, vec![]))]));
    let e = err(check_with(&served(&f, &server), "0.1.0"));
    assert!(matches!(&e, UpdateError::Http { file, status: 500 } if file == "SHA256SUMS.txt"), "{e}");
    assert_eq!(e.kind(), ErrorKind::Network);
}

#[test]
fn update_the_signature_limit_is_named_in_bytes_not_megabytes() {
    let f = Fixture::new("siglimit");
    f.publish("0.2.0", b"x");
    f.put("SHA256SUMS.txt.sig", &[0; 65]);
    let e = err(check(&f, "0.1.0"));
    assert!(matches!(&e, UpdateError::TooLarge { limit: 64, .. }), "{e}");
    assert!(e.to_string().contains("64 bytes") && !e.to_string().contains("MB"), "{e}");
    assert!(UpdateError::TooLarge { file: "a".into(), limit: 64 << 10 }.to_string().contains("64 KB"));
    assert!(UpdateError::TooLarge { file: "a".into(), limit: 400 << 20 }.to_string().contains("400 MB"));
}

/// A `curl` that says `stderr`, exits with `code` and writes its arguments to `<dir>/args`.
#[cfg(unix)]
fn fake_curl(dir: &Path, code: i32, stderr: &str) -> PathBuf {
    let path = dir.join(format!("fake-curl-{code}"));
    let script = format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}/args'\nprintf '%s' '{stderr}' >&2\nexit {code}\n", dir.display());
    std::fs::write(&path, script).unwrap();
    crate::test_files::make_executable(&path);
    path
}

#[cfg(unix)]
#[test]
fn update_curl_failures_are_sorted_by_what_the_user_can_do_about_them() {
    let f = Fixture::new("curl-codes");
    let message = |status: u16| format!("curl: (22) The requested URL returned error: {status}");
    #[derive(Debug, PartialEq)]
    enum Want {
        Network,
        Tls,
        NoSpace,
        Http(u16),
    }
    let table = [
        (6, "curl: (6) Could not resolve host: github.com".to_string(), Want::Network),
        (7, "curl: (7) Failed to connect to github.com port 443".into(), Want::Network),
        (28, "curl: (28) Operation timed out".into(), Want::Network),
        (1, "curl: (1) Protocol \"http\" disabled".into(), Want::Network),
        (22, message(503), Want::Http(503)),
        // curl 8 on macOS reports an HTTP error over HTTP/2 as 56; the message still says which one.
        (56, message(404), Want::Http(404)),
        (56, "curl: (56) Recv failure: Connection reset by peer".into(), Want::Network),
        (23, "curl: (23) Failure writing output to destination".into(), Want::NoSpace),
        (35, "curl: (35) TLS connect error".into(), Want::Tls),
        (51, "curl: (51) SSL: no alternative certificate subject name matches".into(), Want::Tls),
        (60, "curl: (60) SSL certificate problem: certificate has expired".into(), Want::Tls),
        (58, "curl: (58) could not load PEM client certificate".into(), Want::Tls),
        (77, "curl: (77) error setting certificate file".into(), Want::Tls),
    ];
    for (code, stderr, want) in table {
        let mut cfg = f.cfg();
        cfg.curl = fake_curl(&f.dir, code, &stderr);
        let e = err(fetch(&cfg, "http://x/SHA256SUMS.txt", "SHA256SUMS.txt", 100));
        let got = match &e {
            UpdateError::Network(_) => Want::Network,
            UpdateError::Tls(_) => Want::Tls,
            UpdateError::NoSpace => Want::NoSpace,
            UpdateError::Http { status, .. } => Want::Http(*status),
            other => panic!("{code}: {other:?}"),
        };
        assert_eq!(got, want, "exit {code}: {stderr}");
        let kind = if want == Want::NoSpace { ErrorKind::Install } else { ErrorKind::Network };
        assert_eq!(e.kind(), kind, "exit {code}");
    }
    // Nothing on stderr: the exit status still says something.
    let mut cfg = f.cfg();
    cfg.curl = fake_curl(&f.dir, 7, "");
    assert!(matches!(err(fetch(&cfg, "http://x/y", "y", 100)), UpdateError::Network(m) if m.contains("status 7")));
    assert_eq!(err(fetch(&cfg, "http://x/y", "y", 100)).to_string(), "Could not reach the update server: curl exited with status 7");
}

#[cfg(unix)]
#[test]
fn update_the_download_maps_a_full_disk_and_a_too_large_file() {
    let f = Fixture::new("download-codes");
    f.publish("0.2.0", b"x");
    let release = check(&f, "0.1.0").unwrap().unwrap();
    for (code, stderr) in [(23, "curl: (23) Failure writing output to destination"), (63, "curl: (63) Maximum file size exceeded")] {
        let mut cfg = f.cfg();
        cfg.curl = fake_curl(&f.dir, code, stderr);
        let e = err(download_in(&cfg, &f.dir.join("dl"), "0.1.0", &release, &mut |_| {}, &AtomicBool::new(false)));
        match code {
            23 => assert!(matches!(e, UpdateError::NoSpace), "{e}"),
            _ => assert!(matches!(e, UpdateError::TooLarge { .. }), "{e}"),
        }
        assert!(!f.dir.join("dl").exists(), "nothing is left behind");
    }
    assert_eq!(UpdateError::NoSpace.kind(), ErrorKind::Install);
    assert_eq!(UpdateError::NoSpace.to_string(), "There is not enough disk space to download the update.");
}

#[cfg(unix)]
#[test]
fn update_curl_gets_stall_detection_and_the_small_files_a_shorter_one() {
    let f = Fixture::new("speed");
    let args = |f: &Fixture| -> Vec<String> { String::from_utf8(f.read("args")).unwrap().lines().map(str::to_string).collect() };
    let pair = |a: &[String], k: &str, v: &str| a.windows(2).any(|w| w[0] == k && w[1] == v);
    let mut cfg = f.cfg();
    cfg.curl = fake_curl(&f.dir, 0, "");
    fetch(&cfg, "http://x/SHA256SUMS.txt", "SHA256SUMS.txt", 100).unwrap();
    let small = args(&f);
    assert!(pair(&small, "--speed-limit", "1024") && pair(&small, "--speed-time", "30") && pair(&small, "--max-time", "30"), "{small:?}");
    f.publish("0.2.0", b"x");
    let release = check(&f, "0.1.0").unwrap().unwrap();
    // This one ends in a hash mismatch (curl wrote nothing); only the arguments matter.
    let _ = download_in(&cfg, &f.dir.join("dl"), "0.1.0", &release, &mut |_| {}, &AtomicBool::new(false));
    let big = args(&f);
    assert!(pair(&big, "--speed-limit", "1024") && pair(&big, "--speed-time", "60") && pair(&big, "--max-time", "1800"), "{big:?}");
}

#[test]
fn update_failed_checks_are_retried_at_most_three_times_a_day() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100 * 86_400);
    let ago = |hours: u64| now - Duration::from_secs(hours * 3600);
    assert!(retry_allowed(&[], now));
    assert!(retry_allowed(&[ago(1), ago(4)], now), "two failures: one more try");
    assert!(!retry_allowed(&[ago(1), ago(4), ago(7)], now), "three today: wait");
    assert!(retry_allowed(&[ago(1), ago(4), ago(25)], now), "a failure from yesterday does not count");
    assert!(!retry_allowed(&[ago(1), ago(4), now + Duration::from_secs(60)], now), "a clock that went back counts as recent");
}

#[test]
fn update_an_older_list_than_one_seen_before_is_a_replay() {
    assert!(is_rollback("0.2.0", "0.4.0"));
    assert!(is_rollback("0.9.0", "0.10.0"), "numerically, not as text");
    assert!(!is_rollback("0.4.0", "0.4.0") && !is_rollback("0.5.0", "0.4.0"));
    assert!(!is_rollback("junk", "0.4.0") && !is_rollback("0.2.0", "junk"), "what does not parse is not judged here");
    assert_eq!(higher_version("0.5.0", Some("0.4.0")), "0.5.0");
    assert_eq!(higher_version("0.2.0", Some("0.4.0")), "0.4.0");
    assert_eq!(higher_version("0.2.0", None), "0.2.0");
    assert_eq!(higher_version("0.2.0", Some("junk")), "0.2.0");
}

#[cfg(target_os = "macos")]
#[test]
fn update_helper_programs_are_run_by_absolute_path_on_macos() {
    for tool in ["curl", "hdiutil", "ditto", "plutil", "codesign"] {
        let path = system_tool(tool);
        assert_eq!(path, Path::new("/usr/bin").join(tool));
        assert!(path.is_file(), "{}", path.display());
    }
    assert_eq!(Config::new(None).curl, Path::new("/usr/bin/curl"));
}
