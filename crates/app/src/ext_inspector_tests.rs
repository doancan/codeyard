use std::time::{Duration, Instant};

use ext_api::value::{self as v, Completion};
use ext_manifest::Permissions;

use super::*;
use crate::ext_broker::Scope;

fn denied(permission: &str, why: v::Why) -> Completion {
    Completion::Failed(v::Error::Denied(v::Denied { permission: permission.into(), why }))
}

fn denial(permission: &str, why: v::Why) -> Denial {
    Denial { extension: "you.probe".into(), at: 0, call: "workspace/read-file".into(), permission: permission.into(), why }
}

#[test]
fn a_call_is_split_into_io_and_overhead_and_a_request_into_guest_time() {
    let mut r = Record::default();
    r.request_sent("you.probe", 1, "command:fetch");
    let arrived = Instant::now();
    std::thread::sleep(Duration::from_millis(5));
    r.ticket_started("you.probe", 7, "workspace/read-file", arrived);
    std::thread::sleep(Duration::from_millis(3));
    r.ticket_done("you.probe", 7, &Completion::Done, Some(Duration::from_millis(2)));
    let call = r.calls.back().unwrap().clone();
    assert_eq!((call.name.as_str(), call.io, call.guest, call.error.as_deref()), ("workspace/read-file", Duration::from_millis(2), None, None));
    // The time before the work started counts as overhead.
    assert!(call.overhead >= Duration::from_millis(5), "{call:?}");
    std::thread::sleep(Duration::from_millis(10));
    r.request_answered("you.probe", 1, false);
    let req = r.calls.back().unwrap().clone();
    assert_eq!(req.name, "command:fetch");
    let guest = req.guest.unwrap();
    // The request's time minus what its call spent in the app.
    assert!(guest >= Duration::from_millis(10) && guest < Duration::from_secs(5), "{req:?}");
    // Answers to requests it never sent are ignored.
    r.request_answered("you.probe", 99, false);
    assert_eq!(r.calls.len(), 2);
}

#[test]
fn denials_are_kept_in_a_ring() {
    let mut r = Record::default();
    for i in 0..DENIALS + 5 {
        r.ticket_started("you.probe", i as u64, "net/fetch", Instant::now());
        r.ticket_done("you.probe", i as u64, &denied("net = [\"api.example.com\"]", v::Why::NotDeclared), None);
    }
    assert_eq!(r.denials.len(), DENIALS);
    assert_eq!(r.calls.back().unwrap().error.as_deref(), Some("denied"));
    r.denied("you.other", "net/fetch", &v::Denied { permission: "net".into(), why: v::Why::RevokedByUser });
    assert_eq!(r.denials_of("you.other").count(), 1);
    r.exited("you.probe");
}

/// AC1: the fix of a denied read, pasted into the manifest, makes the same call pass.
#[test]
fn a_fix_pasted_into_the_manifest_makes_the_call_pass() {
    let root = std::env::temp_dir().join(format!("inspector-fix-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    let root = crate::ui::canonical_for_test(&root).to_string_lossy().replace('\\', "/");
    let scope = |p: Permissions| Scope {
        extension: "you.probe".into(),
        permissions: p,
        roots: vec![root.clone()],
        trusted: true,
        dependencies: Vec::new(),
        revoked: Permissions::default(),
        programs: None,
    };
    let file = format!("{root}/src/a.rs");
    let d = match scope(Permissions::default()).may_read(&file) {
        Err(v::Error::Denied(d)) => d,
        other => panic!("{other:?}"),
    };
    let entry = denial(&d.permission, d.why);
    let fix = fix_line(&entry, &Permissions::default()).expect("a manifest line");
    let manifest = format!(
        "[extension]\nid = \"you.probe\"\nversion = \"1.0.0\"\nname = \"P\"\nlicense = \"MIT\"\n[code]\nentry = \"Cargo.toml\"\n[permissions]\n{fix}\n\
         [[commands]]\nid = \"go\"\ntitle = \"Go\"\n"
    );
    let m = ext_manifest::parse(&manifest).expect("the fix is a valid manifest line");
    assert!(scope(m.permissions).may_read(&file).is_ok(), "{fix}");
}

#[test]
fn fixes_name_the_switch_or_nothing() {
    let off = Permissions { net: vec!["api.example.com".into(), "cdn.example.com".into()], ..Default::default() };
    // The exthost knows only the kind: the entries the user turned off fill it in.
    assert_eq!(fix_line(&denial("net", v::Why::RevokedByUser), &off).as_deref(), Some(r#"net = ["api.example.com", "cdn.example.com"]"#));
    assert_eq!(fix_line(&denial("net", v::Why::RevokedByUser), &Permissions::default()), None);
    let clip = Permissions { clipboard: ext_manifest::Clipboard::Write, ..Default::default() };
    assert_eq!(fix_line(&denial("clipboard", v::Why::RevokedByUser), &clip).as_deref(), Some(r#"clipboard = "write""#));
    // Trust, not the manifest, fixes an untrusted folder; a path outside the workspace has no fix.
    assert_eq!(fix_line(&denial(r#"write = ["a.txt"]"#, v::Why::UntrustedWorkspace), &off), None);
    assert_eq!(fix_line(&denial("a path inside the workspace (not /etc/passwd)", v::Why::NotDeclared), &off), None);
    // #183: a host outside the account's names the `[[oauth]]` table, not `auth`.
    let host = ext_manifest::oauth_host_line("gitlab", "evil.example.org");
    assert_eq!(
        fix_line(&denial(&host, v::Why::NotDeclared), &off).as_deref(),
        Some(r#"hosts = ["evil.example.org"]  # in the [[oauth]] table with id = "gitlab""#)
    );
    assert_eq!(fix_line(&denial(&ext_manifest::oauth_host_line("github", "example.com"), v::Why::NotDeclared), &off), None);
}

/// AC3: no file contents and no paths outside the extension.
#[test]
fn the_crash_report_names_no_paths_outside_the_extension() {
    let trap = ext_proto::Trap {
        message: "wasm trap: unreachable at /Users/ada/secret-project/probe/src/lib.rs:12".into(),
        frames: vec![
            "probe::handle (/Users/ada/secret-project/probe/src/lib.rs:12)".into(),
            "core::panicking::panic (/rustc/abc123/library/core/src/panicking.rs:75)".into(),
            r"wit (C:\Users\ada\.cargo\registry\wit-bindgen-0.41\src\rt\mod.rs:3)".into(),
            "<unknown>".into(),
            "helper (lib.rs:4)".into(),
        ],
    };
    let r = crash_report("you.probe", "1.0.1", Some(&trap));
    assert!(r.starts_with("you.probe 1.0.1 crashed"), "{r}");
    assert!(r.contains("probe::handle (src/lib.rs:12)"), "{r}");
    assert!(r.contains("(src/panicking.rs:75)") && r.contains("(src/rt/mod.rs:3)") && r.contains("helper (lib.rs:4)"), "{r}");
    for leak in ["ada", "secret-project", "/rustc", "Users", ".cargo"] {
        assert!(!r.contains(leak), "{leak} in {r}");
    }
    assert!(crash_report("you.probe", "1.0.1", None).contains("without a trap"));
}

#[test]
fn report_opens_a_prefilled_issue_only_for_https_repositories() {
    let url = issue_url("https://github.com/you/probe.git", "you.probe", "1.0.1", "trap: x\n").unwrap();
    assert!(url.starts_with("https://github.com/you/probe/issues/new?title=you.probe%201.0.1%20crashes&body="), "{url}");
    assert!(url.contains("trap%3A%20x"), "{url}");
    assert_eq!(issue_url("http://example.com/x", "a.b", "1", ""), None);
    assert_eq!(issue_url("https://example.com/x?y=1", "a.b", "1", ""), None);
}

#[test]
fn durations_read_well() {
    assert_eq!(duration(Duration::from_micros(340)), "340 µs");
    assert_eq!(duration(Duration::from_micros(1250)), "1.2 ms");
    assert_eq!(duration(Duration::from_millis(2500)), "2.5 s");
    assert_eq!(clock(86_400 + 3600 * 14 + 62), "14:01:02");
}
