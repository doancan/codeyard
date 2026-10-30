use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

use ext_api::value::{self as v, Completion, Start, StreamEvent};
use ext_manifest::Permissions;

use super::*;

fn root(name: &str) -> String {
    let d = std::env::temp_dir().join(format!("broker-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::write(d.join("vercel.json"), b"{}").unwrap();
    std::fs::write(d.join(".env"), b"SECRET=1").unwrap();
    std::fs::write(d.join("src/a.rs"), b"fn main() { needle(); }\n").unwrap();
    crate::ui::canonical_for_test(&d).to_string_lossy().replace('\\', "/")
}

fn scope(root: &str, p: Permissions, trusted: bool) -> Scope {
    let eff = Permissions { ..p };
    Scope {
        extension: "you.ext".into(),
        permissions: eff,
        roots: vec![root.to_string()],
        trusted,
        dependencies: vec!["you.lib".into()],
        revoked: Permissions::default(),
    }
}

fn perms(read: &[&str], write: &[&str], run: &[&str]) -> Permissions {
    let s = |l: &[&str]| l.iter().map(|x| x.to_string()).collect();
    Permissions { read: s(read), write: s(write), run: s(run), ..Default::default() }
}

#[derive(Default)]
struct Collect(Mutex<Vec<StreamEvent>>);

impl Sink for Collect {
    fn event(&self, e: StreamEvent) {
        self.0.lock().unwrap().push(e);
    }
}

fn call(scope: &Scope, c: Start) -> (Completion, Vec<StreamEvent>) {
    let host = host::local();
    let sink = Collect::default();
    let done = run(&host, scope, &c, &sink, &AtomicBool::new(false)).expect("handled here");
    (done, sink.0.into_inner().unwrap())
}

fn denied_why(c: &Completion) -> Option<v::Why> {
    match c {
        Completion::Failed(v::Error::Denied(d)) => Some(d.why),
        _ => None,
    }
}

#[test]
fn a_declared_file_is_read_and_another_is_denied() {
    let r = root("read");
    let s = scope(&r, perms(&["vercel.json"], &[], &[]), true);
    assert_eq!(call(&s, Start::ReadFile { uri: uri(&format!("{r}/vercel.json")) }).0, Completion::Bytes(b"{}".to_vec()));
    let (c, _) = call(&s, Start::ReadFile { uri: uri(&format!("{r}/src/a.rs")) });
    assert_eq!(denied_why(&c), Some(v::Why::NotDeclared));
    match c {
        Completion::Failed(v::Error::Denied(d)) => assert_eq!(d.permission, "read = [\"src/a.rs\"]"),
        _ => unreachable!(),
    }
    // Outside the workspace, and escaping it with `..`, are refused before the host is asked.
    let (c, _) = call(&s, Start::ReadFile { uri: uri(&format!("{r}/../etc/passwd")) });
    assert!(denied_why(&c).is_some(), "{c:?}");
    let (c, _) = call(&s, Start::ReadFile { uri: "file:///etc/passwd".into() });
    assert!(denied_why(&c).is_some(), "{c:?}");
}

/// #108: a file whose `read` entry the user turned off is denied as `revoked-by-user`, naming that entry.
#[test]
fn a_permission_the_user_turned_off_is_revoked_by_user() {
    let r = root("revoked");
    let s = Scope { revoked: perms(&["src/**"], &[], &[]), ..scope(&r, perms(&["vercel.json"], &[], &[]), true) };
    match call(&s, Start::ReadFile { uri: uri(&format!("{r}/src/a.rs")) }).0 {
        Completion::Failed(v::Error::Denied(d)) => assert_eq!((d.permission.as_str(), d.why), ("read = [\"src/**\"]", v::Why::RevokedByUser)),
        other => panic!("{other:?}"),
    }
    assert!(matches!(s.may_run("vercel"), Err(v::Error::Denied(v::Denied { why: v::Why::NotDeclared, .. }))));
}

#[test]
fn credentials_must_be_named() {
    let r = root("env");
    let wide = scope(&r, perms(&["**"], &[], &[]), true);
    assert!(denied_why(&call(&wide, Start::ReadFile { uri: uri(&format!("{r}/.env")) }).0).is_some());
    assert!(matches!(call(&wide, Start::ReadFile { uri: uri(&format!("{r}/src/a.rs")) }).0, Completion::Bytes(_)));
    let named = scope(&r, perms(&[".env"], &[], &[]), true);
    assert!(matches!(call(&named, Start::ReadFile { uri: uri(&format!("{r}/.env")) }).0, Completion::Bytes(_)));
}

/// A code action the user chose edits the document it was asked for; any other file needs `write`, so an action
/// cannot slip a change into `.git/hooks` or a file outside the workspace.
#[test]
fn a_code_action_edits_its_document_and_other_files_need_write() {
    let r = root("edit");
    let doc = format!("{r}/src/a.rs");
    let none = scope(&r, perms(&[], &[], &[]), true);
    assert_eq!(none.may_edit(&doc, &uri(&doc)).unwrap(), doc);
    assert_eq!(none.may_edit(&doc, &format!("{r}/src/./a.rs")).unwrap(), doc, "the same file, spelled differently");
    for other in [format!("{r}/vercel.json"), format!("{r}/.git/hooks/pre-commit"), format!("{r}/src/../../escape.rs"), "/etc/passwd".into()] {
        assert!(matches!(none.may_edit(&doc, &uri(&other)), Err(v::Error::Denied(_))), "{other}");
    }
    let json = scope(&r, perms(&[], &["*.json"], &[]), true);
    assert_eq!(json.may_edit(&doc, &uri(&format!("{r}/vercel.json"))).unwrap(), format!("{r}/vercel.json"));
    // In an untrusted folder (`write` already masked away) the other file is refused for that reason, the document not.
    let untrusted = scope(&r, perms(&[], &[], &[]), false);
    assert!(matches!(untrusted.may_edit(&doc, &uri(&format!("{r}/vercel.json"))), Err(v::Error::Denied(v::Denied { why: v::Why::UntrustedWorkspace, .. }))));
    assert!(untrusted.may_edit(&doc, &uri(&doc)).is_ok());
    // A document that does not normalize grants nothing.
    assert!(none.may_edit("relative/a.rs", "relative/a.rs").is_err());
}

#[test]
fn writes_need_write_and_trust() {
    let r = root("write");
    let target = uri(&format!("{r}/out.txt"));
    let s = scope(&r, perms(&[], &["out.txt"], &[]), true);
    assert_eq!(call(&s, Start::WriteFile { uri: target.clone(), contents: b"x".to_vec() }).0, Completion::Done);
    assert_eq!(std::fs::read(format!("{r}/out.txt")).unwrap(), b"x");
    // The effective permissions of an untrusted folder have no `write` (ext_core::trust); the reason says why.
    let untrusted = scope(&r, perms(&[], &[], &[]), false);
    let (c, _) = call(&untrusted, Start::WriteFile { uri: target, contents: b"y".to_vec() });
    assert_eq!(denied_why(&c), Some(v::Why::UntrustedWorkspace));
}

#[test]
fn find_files_and_search_report_only_readable_files() {
    let r = root("find");
    let s = scope(&r, perms(&["src/**"], &[], &[]), true);
    let (c, _) = call(&s, Start::FindFiles { glob: "**".into(), max: 100 });
    assert_eq!(c, Completion::Uris(vec![uri(&format!("{r}/src/a.rs"))]));
    let q = v::SearchQuery { pattern: "needle".into(), regex: false, case_sensitive: true, whole_word: false, include: vec![], max: 10 };
    let (c, events) = call(&s, Start::Search { query: q });
    assert_eq!(c, Completion::Done);
    assert_eq!(events.len(), 1, "{events:?}");
    let StreamEvent::SearchMatch(m) = &events[0] else { panic!() };
    assert_eq!(m.uri, uri(&format!("{r}/src/a.rs")));
    assert_eq!(m.range.start, v::Position { line: 0, character: 12 });
}

#[cfg(unix)]
#[test]
fn programs_come_from_run_and_never_from_the_workspace() {
    let r = root("run");
    let s = scope(&r, perms(&[], &[], &["echo"]), true);
    let cmd = |program: &str| v::Command { program: program.into(), args: vec!["hi".into()], cwd: None, env: vec![], stdin: None };
    // The user's environment does not reach the program; what the extension passes does.
    let s_env = scope(&r, perms(&[], &[], &["env"]), true);
    let probe = v::Command { program: "env".into(), args: vec![], cwd: None, env: vec![("DEPLOY".into(), "yes".into())], stdin: None };
    let (c, out) = call(&s_env, Start::Spawn { command: probe });
    assert_eq!(c, Completion::Exit(Some(0)));
    let text: Vec<u8> = out.into_iter().flat_map(|e| if let StreamEvent::Stdout(b) = e { b } else { vec![] }).collect();
    let text = String::from_utf8_lossy(&text);
    assert!(text.contains("DEPLOY=yes"), "{text}");
    assert!(!text.contains("CARGO") && !text.contains("RUST"), "the test runner's environment leaked: {text}");
    let (c, out) = call(&s, Start::Spawn { command: cmd("echo") });
    assert_eq!(c, Completion::Exit(Some(0)));
    let text: Vec<u8> = out.into_iter().flat_map(|e| if let StreamEvent::Stdout(b) = e { b } else { vec![] }).collect();
    assert_eq!(String::from_utf8_lossy(&text).trim(), "hi");
    let (c, _) = call(&s, Start::Spawn { command: cmd("ls") });
    assert_eq!(denied_why(&c), Some(v::Why::NotDeclared));
    // A program planted inside the workspace is refused even when its name is declared.
    let planted = format!("{r}/tool");
    std::fs::write(&planted, "#!/bin/sh\necho owned\n").unwrap();
    let s = scope(&r, perms(&[], &[], &[&planted]), true);
    let (c, _) = call(&s, Start::Spawn { command: cmd(&planted) });
    assert!(denied_why(&c).is_some(), "{c:?}");
}

#[test]
fn commands_are_own_dependencies_or_listed_core() {
    let s = Scope {
        extension: "you.ext".into(),
        permissions: Permissions { commands: vec!["core:workbench.toggleZenMode".into()], ..Default::default() },
        roots: vec![],
        trusted: true,
        dependencies: vec!["you.lib".into()],
        revoked: Permissions::default(),
    };
    assert!(s.may_execute("you.ext.refresh").is_ok());
    assert!(s.may_execute("you.lib.go").is_ok());
    assert!(s.may_execute("core:workbench.toggleZenMode").is_ok());
    // Listed for extensions but not in this manifest: the line to add. Not listed for extensions: refused with where
    // the list is.
    assert!(matches!(s.may_execute("core:workbench.saveAll"), Err(v::Error::Denied(d)) if d.permission == "commands = [\"core:workbench.saveAll\"]"));
    assert!(matches!(s.may_execute("core:workbench.quit"), Err(v::Error::Invalid(m)) if m.contains("core-commands.md")));
    assert!(s.may_execute("you.extra.go").is_err(), "a prefix of the id is not the id");
    assert!(s.may_execute("them.ext.go").is_err());
}

#[test]
fn core_commands_need_what_they_imply() {
    use ext_manifest::core_commands::Implies;
    let r = root("imply");
    let s = scope(&r, perms(&["docs/**"], &["src/**"], &["cargo"]), true);
    let file = |rel: &str| format!("{r}/{rel}");
    assert!(s.may_imply(Implies::Nothing, &[file("anything")]).is_ok());
    assert!(s.may_imply(Implies::ReadArgument, &[file("docs/a.md")]).is_ok());
    assert!(denied_why(&Completion::Failed(s.may_imply(Implies::ReadArgument, &[file("src/a.rs")]).unwrap_err())).is_some());
    assert!(s.may_imply(Implies::WriteUnsaved, &[file("src/a.rs"), file("src/b.rs")]).is_ok());
    // One file it may not write is enough to refuse the whole command.
    let e = s.may_imply(Implies::WriteUnsaved, &[file("src/a.rs"), file("Cargo.toml")]).unwrap_err();
    assert!(matches!(&e, v::Error::Denied(d) if d.permission == "write = [\"Cargo.toml\"]"), "{e:?}");
    assert!(s.may_imply(Implies::RunTask, &["cargo".into()]).is_ok());
    assert!(s.may_imply(Implies::RunTask, &["make".into()]).is_err());
    // An untrusted folder's permissions have no `write` or `run` (ext_core::trust), and the denial says why.
    let untrusted = scope(&r, perms(&["docs/**"], &[], &[]), false);
    let e = untrusted.may_imply(Implies::WriteActiveFile, &[file("src/a.rs")]).unwrap_err();
    assert_eq!(denied_why(&Completion::Failed(e)), Some(v::Why::UntrustedWorkspace));
}

#[test]
fn uris_normalize() {
    assert_eq!(normalize("file:///a/b/../c/./d"), Some("/a/c/d".into()));
    assert_eq!(normalize("file:///a%20b"), Some("/a b".into()));
    assert_eq!(normalize("file:///C:/x/y"), Some("C:/x/y".into()));
    assert_eq!(normalize("relative/path"), None);
    assert_eq!(normalize("file:///../.."), None);
    assert_eq!(normalize("file:///a%00b"), None, "a NUL is never a path");
}
