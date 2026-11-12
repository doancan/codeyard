use super::*;

const HOME: &str = "/Users/dogan";

fn clean(text: &str) -> String {
    redact(text, Some(HOME))
}

#[test]
fn the_home_folder_becomes_a_tilde_every_time() {
    assert_eq!(clean("open /Users/dogan/src/a.rs and /Users/dogan/src/b.rs"), "open ~/src/a.rs and ~/src/b.rs");
    assert_eq!(clean("/Users/dogan"), "~");
    assert_eq!(clean("in '/Users/dogan/x' (/Users/dogan)"), "in '~/x' (~)");
}

#[test]
fn a_longer_name_is_not_cut_in_the_middle() {
    let out = redact("/Users/adam/x", Some("/Users/ad"));
    assert_eq!(out, "~/x", "the generic rule hides the other account: {out}");
    assert!(!out.contains("adam"));
}

#[test]
fn windows_paths_match_with_either_separator_and_any_case() {
    let home = r"C:\Users\Doğan Can";
    assert_eq!(redact(r"C:\Users\Doğan Can\src\a.rs", Some(home)), r"~\src\a.rs");
    assert_eq!(redact("C:/Users/Doğan Can/src/a.rs", Some(home)), "~/src/a.rs");
    assert_eq!(redact(r"c:\users\doğan can\src", Some(home)), r"~\src");
}

#[test]
fn turkish_user_names_are_found() {
    assert_eq!(redact("/Users/İsmail/x /Users/ışık/y", Some("/Users/İsmail")), "~/x ~/y");
    assert_eq!(redact("/home/çağrı/.config/codeyard", Some("/home/çağrı")), "~/.config/codeyard");
}

#[test]
fn other_accounts_are_hidden_too() {
    assert_eq!(clean("/home/ci/work and /Users/shared-build/x and D:\\Users\\bob\\y"), "~/work and ~/x and ~\\y");
}

#[test]
fn a_missing_or_tiny_home_changes_nothing_else() {
    assert_eq!(redact("/usr/bin/git", None), "/usr/bin/git");
    assert_eq!(redact("/ is the root", Some("/")), "/ is the root");
    assert_eq!(redact("", Some(HOME)), "");
}

#[test]
fn url_user_info_is_removed_and_the_host_kept() {
    assert_eq!(clean("clone https://bob:hunter2@github.com/o/r.git"), "clone https://***@github.com/o/r.git");
    assert_eq!(clean("https://ghp_abcdefghijklmnopqrstuvwxyz0123456789@github.com/o/r"), "https://***@github.com/o/r");
}

#[test]
fn authorization_headers_lose_their_value() {
    assert_eq!(clean("Authorization: Bearer abc.def.ghi"), "Authorization: [redacted]");
    assert_eq!(clean("> authorization:Basic dXNlcjpwYXNz\n> Accept: */*"), "> authorization: [redacted]\n> Accept: */*");
    assert_eq!(clean("Cookie: session=abc; theme=dark"), "Cookie: [redacted]");
    assert_eq!(clean("Proxy-Authorization: Basic xxxxxxxx"), "Proxy-Authorization: [redacted]");
}

#[test]
fn key_value_secrets_lose_the_value_but_keep_the_key() {
    assert_eq!(clean("GITHUB_TOKEN=abc123 other=1"), "GITHUB_TOKEN=[redacted] other=1");
    assert_eq!(clean(r#"{"password": "p4ss w0rd", "user": "x"}"#), r#"{"password": [redacted], "user": "x"}"#);
    assert_eq!(clean("api_key: 'k-1'"), "api_key: [redacted]");
    assert_eq!(clean("client_secret = s3cr3t,next"), "client_secret = [redacted],next");
}

#[test]
fn tokens_with_a_known_prefix_are_removed_anywhere() {
    for token in [
        "ghp_0123456789abcdefghijABCDEFGHIJ012345",
        "github_pat_11ABCDEFG0123456789_abcdefghijklmnop",
        "glpat-abcdefghijklmnopqrst",
        "xoxb-1234567890-abcdef",
        "sk-ant-api03-abcdefghijklmnopqrstuvwx",
        "AKIAIOSFODNN7EXAMPLE",
        "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefghij",
    ] {
        let out = clean(&format!("got {token} back"));
        assert_eq!(out, "got [redacted] back", "{token}");
    }
}

#[test]
fn bearer_outside_a_header_is_removed() {
    assert_eq!(clean("sent Bearer abcdefgh12345678 to the server"), "sent Bearer [redacted] to the server");
}

#[test]
fn long_mixed_runs_go_but_commit_ids_and_plain_words_stay() {
    assert_eq!(clean("key Zm9vYmFyMTIzNDU2Nzg5MGFiY2RlZmdoaWprbG1ubw end"), "key [redacted] end");
    let sha1 = "a94a8fe5ccb19ba61c4c0873d391e987982fbbd3";
    let sha256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    assert_eq!(clean(&format!("commit {sha1} {sha256}")), format!("commit {sha1} {sha256}"));
    assert_eq!(clean("a_test_name_that_is_rather_long_indeed_yes"), "a_test_name_that_is_rather_long_indeed_yes");
}

#[test]
fn ssh_logins_and_emails_lose_the_user_and_keep_the_host() {
    assert_eq!(clean("ssh deploy@build.example.com failed"), "ssh ***@build.example.com failed");
    assert_eq!(clean("mail doğan.can+x@example.org."), "mail ***@example.org.");
    assert_eq!(clean("root@192.168.1.5"), "***@192.168.1.5");
    assert_eq!(clean("@mention and a@"), "@mention and a@");
}

#[test]
fn doubled_backslashes_from_debug_or_json_text_are_matched() {
    let home = r"C:\Users\dogan";
    assert_eq!(redact(r"C:\\Users\\dogan\\proj\\a.rs", Some(home)), r"~\\proj\\a.rs");
    assert_eq!(redact(r#"{"path":"C:\\Users\\dogan\\proj"}"#, Some(home)), r#"{"path":"~\\proj"}"#);
    assert_eq!(redact(r"C:\\\\Users\\\\dogan\\\\x", Some(home)), r"~\\\\x");
    // Another account, no home given.
    let other = redact(r"opened C:\\Users\\bob\\proj", None);
    assert!(!other.contains("bob"), "{other}");
}

#[test]
fn windows_forward_slashes_the_extended_prefix_and_encoded_forms_are_matched() {
    let home = r"C:\Users\dogan";
    assert_eq!(redact("C:/Users/dogan/proj", Some(home)), "~/proj");
    let extended = redact(r"\\?\C:\Users\dogan\proj", Some(home));
    assert!(!extended.contains("dogan") && extended.contains("proj"), "{extended}");
    let extended_other = redact(r"at \\?\C:\Users\bob\proj", None);
    assert!(!extended_other.contains("bob"), "{extended_other}");
    assert_eq!(redact("file:///C:/Users/dogan/proj", Some(home)), "file:///~/proj");
    let encoded = redact("open?path=%2FUsers%2Fdogan%2Fproj&x=1", Some("/Users/dogan"));
    assert!(!encoded.contains("dogan") && encoded.contains("proj"), "{encoded}");
    let encoded_win = redact("C%3A%5CUsers%5Cdogan%5Cproj", Some(home));
    assert!(!encoded_win.contains("dogan"), "{encoded_win}");
    let lower = redact("%5cUsers%5cdogan%5cproj", Some("/Users/dogan"));
    assert!(!lower.contains("dogan"), "{lower}");
    let other = redact("see %2Fhome%2Fbob%2Fx", None);
    assert!(!other.contains("bob"), "{other}");
}

#[test]
fn composed_and_decomposed_turkish_names_match_each_other() {
    let nfc = "/Users/do\u{11f}an \u{15f}\u{131}k"; // doğan şık as single letters
    let nfd = "/Users/dog\u{306}an s\u{327}\u{131}k"; // g + breve, s + cedilla
    for (home, text) in [(nfc, nfd), (nfd, nfc), (nfc, nfc), (nfd, nfd)] {
        let out = redact(&format!("in {text}/proj"), Some(home));
        assert_eq!(out, "in ~/proj", "home {home:?} text {text:?}");
    }
    let dotted = redact("/Users/\u{130}smail/x", Some("/Users/I\u{307}smail"));
    assert_eq!(dotted, "~/x");
}

#[test]
fn project_paths_with_users_or_home_in_them_are_left_alone() {
    for path in [
        "src/users/list.rs:12",
        "crates/app/src/home/view.rs",
        "   at crates/app/src/users/mod.rs:5:9",
        "error in ./home/page.tsx",
        "tests/Users/fixtures/a.json",
        r"src\users\list.rs",
        "my/home/dir is a relative path",
    ] {
        assert_eq!(clean(path), path);
    }
    // A real backtrace: the toolchain and registry paths stay, the home folder goes.
    let frame = "  14: run_command_label\n             at /Users/dogan/work/codeyard/crates/app/src/workbench.rs:1766:13\n   2: call_once\n             at /opt/homebrew/Cellar/rust/1.99.1/lib/rustlib/src/rust/library/core/src/ops/function.rs:250:5\n";
    let out = clean(frame);
    assert!(out.contains("at ~/work/codeyard/crates/app/src/workbench.rs:1766:13"), "{out}");
    assert!(out.contains("/opt/homebrew/Cellar/rust/1.99.1/lib/rustlib/src/rust/library/core/src/ops/function.rs:250:5"), "{out}");
}

#[test]
fn another_accounts_home_is_still_hidden_where_a_path_starts() {
    for text in ["cwd=/home/ci/work", "(\"/Users/bob/x\")", "at '/home/bob/x'", "/home/bob/x is it", "D:\\Users\\bob\\y", "[/Users/bob]"] {
        let out = clean(text);
        assert!(!out.contains("bob") && !out.contains("/ci"), "{text:?} -> {out:?}");
    }
}

#[test]
fn prose_with_basic_or_bearer_is_not_a_credential() {
    for text in ["open the basic settings page", "a Basic overview of things", "bearer authentication is supported", "Basic Settings are here"] {
        assert_eq!(clean(text), text);
    }
    assert_eq!(clean("sent Basic dXNlcjpwYXNzd29yZA== now"), "sent Basic [redacted] now");
    assert_eq!(clean("sent Bearer abc123def456 now"), "sent Bearer [redacted] now");
    assert_eq!(clean("Bearer AbCdEfGhIjKlMn"), "Bearer [redacted]");
}

#[test]
fn json_headers_and_more_secret_shapes_are_removed() {
    assert_eq!(clean(r#"{"Cookie": "sid=abc", "Accept": "x"}"#), r#"{"Cookie": [redacted], "Accept": "x"}"#);
    for secret in [
        r#"{"Set-Cookie": "sid=hunter2; Path=/"}"#,
        r#"{"Authorization": "Basic dXNlcjpwYXNz"}"#,
        r#"{"X-Api-Key": "k-hunter2"}"#,
        r#"{'cookie': 'hunter2'}"#,
        "session_id=hunter2",
        "api-key: hunter2",
        "x-api-key: hunter2",
        "passwd=hunter2",
        "Set-Cookie: sid=hunter2",
    ] {
        let out = clean(secret);
        assert!(!out.contains("hunter2") && !out.contains("dXNlcjpwYXNz"), "{secret:?} -> {out:?}");
    }
    // Neighbours survive.
    assert!(clean(r#"{"Cookie": "a=hunter2", "Accept": "text/html"}"#).contains(r#""Accept": "text/html""#));
    let aws = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";
    assert_eq!(aws.len(), 40);
    assert_eq!(clean(&format!("aws_secret {aws} end")), "aws_secret [redacted] end");
    assert_eq!(clean("a settings line terminal.persistentSessionTimeout = 5"), "a settings line terminal.persistentSessionTimeout = 5");
}

#[test]
fn a_long_text_is_redacted_before_it_is_cut() {
    // The token starts 10 characters before the limit: cutting first would keep its first half.
    let token = "ghp_0123456789abcdefghijABCDEFGHIJ012345";
    let text = format!("{}{token} tail", "x ".repeat(995));
    let out = redact_and_cut(&text, None, 2_000);
    assert!(!out.contains("ghp_"), "{out}");
    assert!(out.chars().count() <= 2_001);
    assert!(out.contains("[redacted]"));
    // A huge input is bounded and does not end in a half token.
    let huge = format!("{} {token}", "word ".repeat(2_000_000));
    let started = std::time::Instant::now();
    let out = redact_and_cut(&huge, None, 2_000);
    assert!(out.chars().count() <= 2_001 && started.elapsed().as_secs() < 5);
}

#[test]
fn a_source_location_is_not_an_assignment_but_a_real_one_still_is() {
    for text in ["tokenizer.rs:12", "src/tokenizer.rs:12:5: error", "at crates/app/src/secrets.rs:300", "password_check.rs:7"] {
        assert_eq!(clean(text), text);
    }
    for secret in ["token: 12345", "token=12", "password=1234", "tokenizer.rs: 12345"] {
        assert!(clean(secret).contains("[redacted]"), "{secret}");
    }
}

#[test]
fn a_drive_letter_goes_with_the_home_folder() {
    assert_eq!(redact(r"C:\\Users\\dogancan\\proj", Some("/Users/dogancan")), r"~\\proj");
    assert_eq!(redact(r"opened D:\Users\dogancan\proj", Some("/Users/dogancan")), r"opened ~\proj");
    assert_eq!(redact("in abc:/Users/dogancan/x", Some("/Users/dogancan")), "in abc:~/x", "only a single letter is a drive");
}

#[test]
fn long_runs_of_separators_are_handled_in_linear_time() {
    let started = std::time::Instant::now();
    for unit in ["/", "\\", "%5C", "%2F", "/\\"] {
        let run = unit.repeat(100_000 / unit.len());
        for home in ["/Users/dogan", r"C:\Users\dogan"] {
            let out = redact(&format!("a {run} b"), Some(home));
            assert!(out.len() >= run.len() / 2 && !out.contains("dogan"));
        }
    }
    assert!(started.elapsed().as_secs() < 5, "took {:?}", started.elapsed());
}

#[test]
fn a_bare_key_with_a_40_character_secret_is_redacted() {
    let secret = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";
    for text in [format!("key={secret}"), format!("secret_key: {secret}"), format!("\"key\": \"{secret}\""), format!("AWS_KEY={secret} and more")] {
        let out = clean(&text);
        assert!(!out.contains("wJalr") && !out.contains("EXAMPLEKEY"), "{text} -> {out}");
    }
    assert_eq!(clean("key=short/value"), "key=short/value");
}

#[test]
fn redaction_is_idempotent() {
    let text = "Authorization: Bearer abc\nssh a@b.c /Users/dogan/x token=zzz https://u:p@h/";
    let once = clean(text);
    assert_eq!(clean(&once), once);
}

#[test]
fn ordinary_log_lines_pass_through() {
    let line = "2026-11-12T10:00:00Z INFO update: checked, nothing new (3 extensions)";
    assert_eq!(clean(line), line);
}

#[test]
fn a_huge_line_is_handled_in_linear_time() {
    let big = "a1".repeat(500_000);
    let started = std::time::Instant::now();
    let out = clean(&big);
    assert_eq!(out, "[redacted]");
    assert!(started.elapsed().as_secs() < 5);
}
