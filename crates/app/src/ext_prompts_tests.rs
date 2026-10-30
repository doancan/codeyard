use ext_api::value::{self as v, Start};
use ext_manifest::{Clipboard, Permissions};

use super::*;

fn scope(p: Permissions) -> Scope {
    Scope { extension: "you.ext".into(), permissions: p, roots: vec!["/work".into()], trusted: true, dependencies: vec![], revoked: Permissions::default() }
}

fn spawn(program: &str) -> Start {
    Start::Spawn { command: v::Command { program: program.into(), args: vec![], cwd: None, env: vec![], stdin: None } }
}

#[test]
fn only_declared_clipboard_reads_and_remote_processes_ask() {
    let reads = scope(Permissions { clipboard: Clipboard::Read, run: vec!["echo".into()], ..Default::default() });
    let none = scope(Permissions::default());
    assert_eq!(kind_of(&Start::ReadClipboard, &reads, None), Some(Kind::ClipboardRead));
    // Undeclared: denied as before, never asked.
    assert_eq!(kind_of(&Start::ReadClipboard, &none, None), None);
    assert_eq!(kind_of(&spawn("echo"), &reads, Some("ssh://dev")), Some(Kind::RemoteProcess { host: "ssh://dev".into() }));
    assert_eq!(kind_of(&spawn("echo"), &reads, None), None, "a local process does not ask");
    assert_eq!(kind_of(&spawn("rm"), &reads, Some("ssh://dev")), None, "a program outside `run` is denied, not asked");
    assert_eq!(kind_of(&Start::WriteClipboard { text: "x".into() }, &reads, None), None);
}

#[test]
fn the_first_call_asks_and_the_answer_holds_for_the_session() {
    let mut p: Prompts<u64> = Prompts::default();
    let k = Kind::ClipboardRead;
    assert_eq!(p.check("you.a", &k, 1), Step::Ask);
    assert_eq!(p.check("you.a", &k, 2), Step::Wait, "a second call waits for the open dialog");
    assert_eq!(p.answered("you.a", &k), None);
    assert_eq!(p.answer("you.a", &k, true), vec![1, 2]);
    assert_eq!(p.check("you.a", &k, 3), Step::Go(3));
    // Another extension, or another host, asks on its own.
    assert_eq!(p.check("you.b", &k, 4), Step::Ask);
    let remote = Kind::RemoteProcess { host: "ssh://dev".into() };
    assert_eq!(p.check("you.a", &remote, 5), Step::Ask);
    assert_eq!(p.answer("you.a", &remote, false), vec![5]);
    assert_eq!(p.check("you.a", &remote, 6), Step::Deny(6));
    assert_eq!(p.check("you.a", &Kind::RemoteProcess { host: "ssh://other".into() }, 7), Step::Ask);
}

#[test]
fn a_denial_says_revoked_by_user_and_names_no_manifest_line() {
    let v::Completion::Failed(v::Error::Denied(d)) = Kind::ClipboardRead.denied() else { panic!() };
    assert_eq!((d.permission.as_str(), d.why), ("clipboard", v::Why::RevokedByUser));
    let denial =
        crate::ext_inspector::Denial { extension: "you.a".into(), at: 0, call: "clipboard-read/read-text".into(), permission: d.permission, why: d.why };
    assert_eq!(crate::ext_inspector::fix_line(&denial, &Permissions::default()), None);
}

#[test]
fn the_dialog_names_the_extension_publisher_and_what_it_wants() {
    let (title, text) = dialog(&Kind::RemoteProcess { host: "ssh://dev".into() }, "Deploy", "you.deploy", "you, from the yard registry", Some("vercel"));
    assert!(title.contains("Deploy") && title.contains("ssh://dev"), "{title}");
    assert!(text.contains("you.deploy") && text.contains("you, from the yard registry") && text.contains("vercel"), "{text}");
    let (title, _) = dialog(&Kind::ClipboardRead, "Deploy", "you.deploy", "you", None);
    assert!(title.contains("clipboard"), "{title}");
}
