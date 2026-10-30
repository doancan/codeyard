//! A decoration provider for the performance scenarios (scripts/perf/run.sh): `decorate` marks the active document's
//! lines (a gutter mark on each, a muted note on every tenth), and its task provider answers at once with one task, so
//! the time measured is the platform's, not the extension's. Asked for code lenses and inlay hints (#143), it answers
//! at once too: a lens on every fifth line in view and a hint on every line.

wit_bindgen::generate!({ path: "../../../../ext-api/wit", world: "extension" });

use std::cell::RefCell;
use std::collections::HashMap;

use codeyard::api::host_requests::Request;
use codeyard::api::responses::{CodeLens, InlayHint, LensCommand, Response, Task};
use codeyard::api::types::{Completion, Position, Range, StreamEvent, Ticket, Tone};
use codeyard::api::{documents, languages, requests};
use exports::codeyard::api::guest::{Activation, Guest};

/// Lines decorated: the host keeps at most this many marks per file and set.
const LINES: u32 = 1000;

thread_local! {
    /// Which `decorate` request each `documents.active` ticket answers.
    static WAITING: RefCell<HashMap<Ticket, u64>> = RefCell::new(HashMap::new());
}

struct Decorator;

fn decorate(uri: &str, lines: u32) {
    let mut marks = Vec::new();
    for line in 0..lines.min(LINES) {
        let range = Range { start: Position { line, character: 0 }, end: Position { line, character: 0 } };
        let note = line % 10 == 0;
        marks.push(languages::Decoration {
            range,
            place: if note { languages::DecorationPlace::After } else { languages::DecorationPlace::Gutter },
            tone: if note { Tone::Muted } else { Tone::Success },
            text: note.then(|| "covered".to_string()),
            hover: None,
        });
    }
    languages::set_decorations(uri, "lines", &marks);
}

impl Guest for Decorator {
    fn activate(_: Activation) -> Result<(), String> {
        Ok(())
    }

    fn deactivate() {}

    fn handle(id: u64, request: Request) {
        match request {
            Request::Command((command, _)) if command == "decorate" => {
                let ticket = documents::active();
                WAITING.with(|w| w.borrow_mut().insert(ticket, id));
            }
            Request::CodeLens((_, range)) => {
                let lenses = (range.start.line..range.end.line)
                    .filter(|l| l % 5 == 0)
                    .map(|line| CodeLens {
                        id: format!("{line}"),
                        range: Range { start: Position { line, character: 0 }, end: Position { line, character: 0 } },
                        command: Some(LensCommand { title: format!("Line {}", line + 1), command: "decorate".into(), args_json: "null".into() }),
                    })
                    .collect();
                requests::respond(id, Ok(&Response::CodeLenses(lenses)));
            }
            Request::InlayHints((_, range)) => {
                let hints = (range.start.line..range.end.line)
                    .map(|line| InlayHint { position: Position { line, character: 0 }, label: ": i32".into(), tooltip: None })
                    .collect();
                requests::respond(id, Ok(&Response::InlayHints(hints)));
            }
            Request::ProvideTasks => {
                let task = Task { id: "noop".into(), label: "Decorator: No-op".into(), detail: None, group: "build".into() };
                requests::respond(id, Ok(&Response::Tasks(vec![task])));
            }
            _ => requests::respond(id, Ok(&Response::None)),
        }
    }

    fn cancel(_: u64) {}

    fn complete(ticket: Ticket, result: Completion) {
        let Some(id) = WAITING.with(|w| w.borrow_mut().remove(&ticket)) else { return };
        if let Completion::Document(Some(doc)) = &result {
            decorate(&doc.uri, doc.text.split('\n').count() as u32);
        }
        requests::respond(id, Ok(&Response::Json("null".into())));
    }

    fn event(_: Ticket, _: StreamEvent) {}
}

export!(Decorator);
