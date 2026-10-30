//! Only in probe-scm.wasm (#144): a toy source control provider, `toy`, for folders with a `.toyvcs` folder. It imports
//! `process` (the commit runs `echo`), which every manifest using the plain probe would otherwise have to declare.
//!
//! - `.toyvcs/changes` lists the changes, one per line: `<group> <letter> <path>`, the group `staged` or `changes`, the
//!   letter one of `M A D R U I C`.
//! - `.toyvcs/base/<path>` is a file's original text, for the gutter.
//! - The commit command runs `echo <message>` through `run` and answers `committed <message> <files> <exit>`.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::codeyard::api::host_requests::Request;
use crate::codeyard::api::responses::Response;
use crate::codeyard::api::scm::{self, FileState, ScmAction, ScmGroup, ScmInput, ScmResource, ScmState};
use crate::codeyard::api::types::{Completion, Ticket};
use crate::codeyard::api::{process, requests, workspace};

enum Waiting {
    Status(u64, String),
    Original(u64),
    Commit(u64, String, usize),
}

thread_local! {
    static WAITING: RefCell<HashMap<Ticket, Waiting>> = RefCell::new(HashMap::new());
    /// Folders asked about, so a file's original text can be found under its folder's `.toyvcs/base`.
    static ROOTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn action(command: &str, title: &str, icon: &str) -> ScmAction {
    ScmAction { command: command.into(), title: title.into(), icon: Some(icon.into()) }
}

fn state(root: &str, text: &str) -> ScmState {
    let mut staged = ScmGroup {
        id: "staged".into(),
        label: "Staged Changes".into(),
        resources: vec![],
        resource_actions: vec![action("scm-unstage", "Unstage", "minus")],
        group_actions: vec![action("scm-unstage", "Unstage All", "minus")],
        hide_when_empty: true,
    };
    let mut changes = ScmGroup {
        id: "changes".into(),
        label: "Changes".into(),
        resources: vec![],
        resource_actions: vec![action("scm-discard", "Discard Changes", "undo-2"), action("scm-stage", "Stage", "plus")],
        group_actions: vec![action("scm-stage", "Stage All", "plus")],
        hide_when_empty: false,
    };
    for line in text.lines() {
        let mut parts = line.splitn(3, ' ');
        let (Some(group), Some(letter), Some(path)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let state = match letter {
            "A" => FileState::Added,
            "D" => FileState::Deleted,
            "R" => FileState::Renamed,
            "U" => FileState::Untracked,
            "I" => FileState::Ignored,
            "C" => FileState::Conflicted,
            _ => FileState::Modified,
        };
        let r = ScmResource { uri: format!("{root}/{path}"), state, detail: None };
        if group == "staged" { staged.resources.push(r) } else { changes.resources.push(r) }
    }
    ScmState {
        head: Some("toy main".into()),
        input: ScmInput { placeholder: "Message (toy)".into(), value: None, accept: Some(action("scm-commit", "Commit", "check")) },
        groups: vec![staged, changes],
    }
}

/// `"key":"value"` from the JSON the app sends (no escapes in the tests' values).
fn field(json: &str, key: &str) -> String {
    let pat = format!("\"{key}\":\"");
    json.split_once(&pat).and_then(|(_, rest)| rest.split_once('"')).map(|(v, _)| v.to_string()).unwrap_or_default()
}

/// A provider request: `Some(request)` back when it is not one.
pub fn handle(id: u64, request: Request) -> Option<Request> {
    match request {
        Request::ScmStatus((_, root)) => {
            ROOTS.with(|r| {
                let mut r = r.borrow_mut();
                if !r.contains(&root) {
                    r.push(root.clone());
                }
            });
            let t = workspace::read_file(&format!("{root}/.toyvcs/changes"));
            WAITING.with(|w| w.borrow_mut().insert(t, Waiting::Status(id, root)));
        }
        Request::ScmOriginal((_, uri)) => {
            let root = ROOTS.with(|r| r.borrow().iter().find(|root| uri.starts_with(&format!("{root}/"))).cloned());
            match root {
                Some(root) => {
                    let t = workspace::read_file(&format!("{root}/.toyvcs/base/{}", &uri[root.len() + 1..]));
                    WAITING.with(|w| w.borrow_mut().insert(t, Waiting::Original(id)));
                }
                None => requests::respond(id, Ok(&Response::Text(None))),
            }
        }
        Request::ScmValidate((_, _, value)) => {
            let message = if value.trim().is_empty() {
                Some("Write a message first".to_string())
            } else if value.lines().next().is_some_and(|l| l.chars().count() > 50) {
                Some("Keep the first line under 50 characters".to_string())
            } else {
                None
            };
            requests::respond(id, Ok(&Response::Text(message)));
        }
        other => return Some(other),
    }
    None
}

/// The provider's commands: `None` when `command` is not one of them.
pub fn command(id: u64, command: &str, args: &str) -> Option<()> {
    let files = args.split_once("\"uris\":[").map_or(0, |(_, rest)| rest.split(']').next().unwrap_or_default().matches("\"file://").count());
    match command {
        "scm-commit" => {
            let message = field(args, "message");
            let c = process::Command { program: "echo".into(), args: vec![message.clone()], cwd: None, env: vec![], stdin: None };
            let t = process::spawn(&c);
            WAITING.with(|w| w.borrow_mut().insert(t, Waiting::Commit(id, message, files)));
        }
        "scm-stage" | "scm-unstage" | "scm-discard" => crate::reply(id, format!("{command} {files}")),
        "scm-refresh" => {
            scm::refresh("toy");
            crate::reply(id, "refreshed".into());
        }
        _ => return None,
    }
    Some(())
}

/// A ticket of the provider finished: `false` when it is not one of its tickets.
pub fn complete(ticket: Ticket, result: &Completion) -> bool {
    let Some(waiting) = WAITING.with(|w| w.borrow_mut().remove(&ticket)) else { return false };
    let text = match result {
        Completion::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
        _ => None,
    };
    match waiting {
        Waiting::Status(id, root) => requests::respond(id, Ok(&Response::Scm(state(&root, text.as_deref().unwrap_or_default())))),
        Waiting::Original(id) => requests::respond(id, Ok(&Response::Text(text))),
        Waiting::Commit(id, message, files) => crate::reply(id, format!("committed {message} {files} {}", crate::describe(result))),
    }
    true
}
