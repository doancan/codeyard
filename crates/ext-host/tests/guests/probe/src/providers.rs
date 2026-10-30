//! The probe's providers (#104): a formatter, code actions, diagnostics, decorations, tasks and a status item, each
//! as small as the app's tests need.
//!
//! - Formatting puts spaces around the first `=` of each line and drops trailing spaces. A document containing `slow`
//!   is never answered, until the host cancels the request: then the answer comes anyway (late), and the host must
//!   drop it.
//! - Formatting and code actions also lint the document: each `TODO` becomes a warning and a gutter decoration.
//! - Hover names the position; completion offers `probe-item` (and `probe-dot` after `.`). In a document containing
//!   `slow`, completion is never answered until the host cancels it: then a late `probe-late` comes, to be dropped.

use std::cell::RefCell;
use std::collections::HashSet;

use crate::codeyard::api::host_requests::Request;
use crate::codeyard::api::responses::{CodeAction, CompletionItem, CompletionKind, CompletionList, Hover, Response, Task};
use crate::codeyard::api::types::{Diagnostic, DiagnosticSeverity, Document, Position, Range, TextEdit, Tone};
use crate::codeyard::api::{languages, requests, window};

thread_local! {
    /// Format requests the probe sits on (documents that contain `slow`).
    static SLOW: RefCell<HashSet<u64>> = RefCell::new(HashSet::new());
    /// Completion requests the probe sits on.
    static SLOW_COMPLETION: RefCell<HashSet<u64>> = RefCell::new(HashSet::new());
}

fn completion_item(label: &str, detail: &str) -> CompletionItem {
    CompletionItem {
        label: label.into(),
        kind: CompletionKind::Keyword,
        detail: Some(detail.into()),
        documentation: Some(format!("**{label}** from the probe")),
        insert_text: None,
        snippet: false,
        range: None,
        additional_edits: vec![],
        filter_text: None,
        sort_text: None,
    }
}

fn utf16_len(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

/// The whole document, as a range.
fn everything(text: &str) -> Range {
    let lines: Vec<&str> = text.split('\n').collect();
    let last = lines.last().copied().unwrap_or("");
    Range { start: Position { line: 0, character: 0 }, end: Position { line: lines.len() as u32 - 1, character: utf16_len(last) } }
}

fn formatted(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            let line = line.trim_end();
            match line.split_once('=') {
                Some((k, v)) => format!("{} = {}", k.trim_end(), v.trim_start()),
                None => line.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `TODO`s become warnings and decorations on the document.
fn lint(doc: &Document) {
    let mut diagnostics = Vec::new();
    let mut decorations = Vec::new();
    for (n, line) in doc.text.split('\n').enumerate() {
        if let Some(at) = line.find("TODO") {
            let col = utf16_len(&line[..at]);
            let range = Range { start: Position { line: n as u32, character: col }, end: Position { line: n as u32, character: col + 4 } };
            diagnostics.push(Diagnostic { range, severity: DiagnosticSeverity::Warning, message: "TODO left here".into(), code: Some("todo".into()) });
            decorations.push(languages::Decoration { range, place: languages::DecorationPlace::Gutter, tone: Tone::Warning, text: None, hover: None });
            decorations.push(languages::Decoration {
                range,
                place: languages::DecorationPlace::After,
                tone: Tone::Muted,
                text: Some("to do".into()),
                hover: Some("Found by the probe".into()),
            });
        }
    }
    languages::set_diagnostics(&doc.uri, &diagnostics);
    languages::set_decorations(&doc.uri, "todos", &decorations);
}

/// Every request that is not a command.
pub fn handle(id: u64, request: Request) {
    match request {
        Request::Format((doc, _range, _options)) => {
            lint(&doc);
            if doc.text.contains("slow") {
                SLOW.with(|s| s.borrow_mut().insert(id));
                return;
            }
            let edits = vec![TextEdit { range: everything(&doc.text), new_text: formatted(&doc.text) }];
            requests::respond(id, Ok(&Response::Edits(edits)));
        }
        Request::CodeActions((doc, _range, _diagnostics)) => {
            lint(&doc);
            let mut actions = vec![CodeAction {
                id: "hello".into(),
                title: "Probe: Say hello".into(),
                edits: vec![],
                command: None,
                preferred: false,
                fixes: vec![],
            }];
            if let Some((n, line)) = doc.text.split('\n').enumerate().find(|(_, l)| l.contains("TODO")) {
                let col = utf16_len(&line[..line.find("TODO").unwrap_or(0)]);
                let range = Range { start: Position { line: n as u32, character: col }, end: Position { line: n as u32, character: col + 4 } };
                let edits = vec![(doc.uri.clone(), vec![TextEdit { range, new_text: "DONE".into() }])];
                actions.insert(0, CodeAction { id: "done".into(), title: "Probe: Mark as done".into(), edits, command: None, preferred: true, fixes: vec![] });
            }
            requests::respond(id, Ok(&Response::CodeActions(actions)));
        }
        Request::RunCodeAction(action) => {
            let _ = window::show_notice(window::Severity::Info, &format!("Code action {action}"), &[]);
            requests::respond(id, Ok(&Response::None));
        }
        Request::ProvideTasks => {
            let task = Task { id: "hello".into(), label: "Probe: Hello task".into(), detail: Some("says hello".into()), group: "build".into() };
            requests::respond(id, Ok(&Response::Tasks(vec![task])));
        }
        Request::RunTask(task) => {
            let _ = window::show_notice(window::Severity::Info, &format!("Task {task} ran"), &[]);
            requests::respond(id, Ok(&Response::None));
        }
        Request::Hover((_doc, position)) => {
            let contents = format!("**Probe** hover at {}:{}", position.line, position.character);
            requests::respond(id, Ok(&Response::Hover(Some(Hover { contents, range: None }))));
        }
        Request::Completion((doc, _position, trigger)) => {
            if doc.text.contains("slow") {
                SLOW_COMPLETION.with(|s| s.borrow_mut().insert(id));
                return;
            }
            let mut items = vec![completion_item("probe-item", "from the probe")];
            if trigger.as_deref() == Some(".") {
                items.push(completion_item("probe-dot", "after a dot"));
            }
            requests::respond(id, Ok(&Response::Completions(CompletionList { items, incomplete: false })));
        }
        _ => requests::respond(id, Ok(&Response::None)),
    }
}

/// Commands that drive the status item and diagnostics: `None` when `command` is not one of them.
pub fn command(id: u64, command: &str) -> Option<()> {
    let item = |id: &str, text: &str| window::StatusItem {
        id: id.into(),
        text: text.into(),
        tooltip: Some("The probe's state".into()),
        command: Some("notice".into()),
        tone: Tone::Success,
    };
    let result = match command {
        "status" => window::set_status_item(&item("state", "Probe ready")).map_err(|e| format!("{e:?}")),
        "status-undeclared" => window::set_status_item(&item("other", "Not declared")).map_err(|e| format!("{e:?}")),
        "unstatus" => {
            window::remove_status_item("state");
            Ok(())
        }
        "clear" => {
            languages::clear_diagnostics(None);
            Ok(())
        }
        _ => return guards(id, command),
    };
    requests::respond(id, Ok(&Response::Json(format!("{result:?}"))));
    Some(())
}

/// The host stopped waiting for `id`. A request the probe sat on is answered now anyway: a late answer.
pub fn cancel(id: u64) {
    if SLOW.with(|s| s.borrow_mut().remove(&id)) {
        let edits = vec![TextEdit { range: everything(""), new_text: "LATE".into() }];
        requests::respond(id, Ok(&Response::Edits(edits)));
    }
    if SLOW_COMPLETION.with(|s| s.borrow_mut().remove(&id)) {
        let items = vec![completion_item("probe-late", "too late")];
        requests::respond(id, Ok(&Response::Completions(CompletionList { items, incomplete: false })));
    }
}

/// Only in probe-guards.wasm, for the app's run-time guards (ext_prompts_ui_tests.rs): `clipboard` and `run` import
/// `clipboard-read` and `process`, which every manifest using the plain probe would otherwise have to declare; `hold`
/// keeps 64 MB more each time, touched, under the guest's limit and over a lowered resident cap.
#[cfg(feature = "guards")]
fn guards(id: u64, command: &str) -> Option<()> {
    thread_local! {
        static HELD: RefCell<Vec<Vec<u8>>> = const { RefCell::new(Vec::new()) };
    }
    match command {
        "clipboard" => crate::wait(crate::codeyard::api::clipboard_read::read_text(), id),
        "run" => {
            let c = crate::codeyard::api::process::Command { program: "echo".into(), args: vec!["hi".into()], cwd: None, env: vec![], stdin: None };
            crate::wait(crate::codeyard::api::process::spawn(&c), id)
        }
        "hold" => {
            let total = HELD.with(|h| {
                let mut h = h.borrow_mut();
                h.push(vec![1u8; 64 << 20]);
                std::hint::black_box(&*h);
                h.len() * 64
            });
            crate::reply(id, format!("{total}"))
        }
        _ => return None,
    }
    Some(())
}

#[cfg(not(feature = "guards"))]
fn guards(_: u64, _: &str) -> Option<()> {
    None
}
