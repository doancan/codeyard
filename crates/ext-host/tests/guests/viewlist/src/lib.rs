//! A view for the performance scenarios (scripts/perf/run.sh): the sidebar view `rows` is one list of 10 000 rows, the
//! most a list takes, so the keys measured (PageDown, which the host handles) move through the largest list a view can
//! hold.

wit_bindgen::generate!({ path: "../../../../ext-api/wit", world: "extension" });

use codeyard::api::host_requests::Request;
use codeyard::api::requests;
use codeyard::api::responses::Response;
use codeyard::api::types::{Completion, StreamEvent, Ticket};
use exports::codeyard::api::guest::{Activation, Guest};

/// Rows in the list: the host's limit for one list.
const ROWS: usize = 10_000;

struct ViewList;

fn tree() -> String {
    let mut rows = String::with_capacity(ROWS * 64);
    for i in 0..ROWS {
        if i > 0 {
            rows.push(',');
        }
        rows.push_str(&format!(r#"{{"key":"r{i}","element":{{"text":{{"text":"Row {i}"}}}}}}"#));
    }
    format!(r#"{{"element":{{"column":{{"gap":2}}}},"children":[{{"key":"rows","element":{{"list":{{"label":"Rows","selection":"one"}}}},"children":[{rows}]}}]}}"#)
}

impl Guest for ViewList {
    fn activate(_: Activation) -> Result<(), String> {
        Ok(())
    }

    fn deactivate() {}

    fn handle(id: u64, request: Request) {
        match request {
            Request::Render(view) if view == "rows" => requests::respond(id, Ok(&Response::View(tree()))),
            _ => requests::respond(id, Ok(&Response::None)),
        }
    }

    fn cancel(_: u64) {}

    fn complete(_: Ticket, _: Completion) {}

    fn event(_: Ticket, _: StreamEvent) {}
}

// Watches nothing; API 1.1 components export `events` all the same.
impl exports::codeyard::api::events::Guest for ViewList {
    fn documents(_: u64, _: Vec<exports::codeyard::api::events::DocumentEvent>) {}

    fn files(_: u64, _: Vec<exports::codeyard::api::events::FileEvent>) {}
}

export!(ViewList);
