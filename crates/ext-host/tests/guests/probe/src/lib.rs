//! A test extension for codeyard-exthost's tests: each command exercises one part of the host, including the
//! hostile ones (spin, grow, trap, escape).

wit_bindgen::generate!({ path: "../../../../ext-api/wit", world: "extension" });

mod providers;
#[cfg(feature = "scm")]
mod scm;
mod pulls;

use std::cell::RefCell;
use std::collections::HashMap;

use codeyard::api::types::{Completion, Error, HttpRequest, StreamEvent, Ticket};
use codeyard::api::{documents, env, log, requests, settings, storage, timers, ui, window, workspace};
use exports::codeyard::api::guest::{Activation, Guest};
use codeyard::api::host_requests::Request;
use codeyard::api::responses::Response;

thread_local! {
    /// Which request each outstanding ticket answers.
    static WAITING: RefCell<HashMap<Ticket, u64>> = RefCell::new(HashMap::new());
    /// Streamed bodies (`data` events) per ticket, so an answer can show what arrived.
    static BODIES: RefCell<HashMap<Ticket, Vec<u8>>> = RefCell::new(HashMap::new());
}

struct Probe;

fn get(url: &str) -> HttpRequest {
    HttpRequest { method: "GET".into(), url: url.into(), headers: vec![], body: vec![], timeout_ms: None }
}

fn reply(id: u64, json: String) {
    requests::respond(id, Ok(&Response::Json(json)));
}

fn wait(ticket: Ticket, id: u64) {
    WAITING.with(|w| w.borrow_mut().insert(ticket, id));
}

fn describe(c: &Completion) -> String {
    match c {
        Completion::Done => "done".into(),
        Completion::Failed(Error::Denied(d)) => format!("denied {} {:?}", d.permission, d.why),
        Completion::Failed(e) => format!("failed {e:?}"),
        Completion::Bytes(b) => format!("bytes {}", b.len()),
        Completion::Chosen(c) => format!("chosen {c:?}"),
        Completion::Text(t) => format!("text {t:?}"),
        Completion::Document(Some(d)) => format!("document {} {}", d.language, d.text.len()),
        Completion::Document(None) => "no document".into(),
        Completion::Account(a) => format!("account {} {}", a.provider, a.label),
        other => format!("{other:?}"),
    }
}

impl Guest for Probe {
    fn activate(a: Activation) -> Result<(), String> {
        log::log(log::Level::Info, &format!("activated: {}", a.reason));
        Ok(())
    }

    fn deactivate() {
        let _ = storage::set("deactivated", b"yes");
    }

    fn handle(id: u64, request: Request) {
        let request = match request {
            Request::PullRequests(query) => return pulls::handle(id, query),
            other => other,
        };
        // A view (`counter`): a button and how often it was clicked; a click redraws it through `ui.invalidate`.
        if let Request::Render(view) = &request {
            let clicks = storage::get("clicks").map_or(0, |b| b.len());
            let tree = format!(
                r#"{{"element": {{"column": {{"gap": 2}}}}, "children": [
                    {{"element": {{"heading": {{"text": "Counter", "level": 2}}}}}},
                    {{"key": "add", "element": {{"button": {{"label": "Add one", "tone": "accent"}}}}}},
                    {{"key": "count", "element": {{"text": {{"text": "{view}: {clicks} clicks"}}}}}}
                ]}}"#
            );
            requests::respond(id, Ok(&Response::View(tree)));
            return;
        }
        if let Request::UiEvent((view, event)) = &request {
            if event.contains(r#""click""#) && event.contains(r#""add""#) {
                let n = storage::get("clicks").map_or(0, |b| b.len()) + 1;
                let _ = storage::set("clicks", &vec![1u8; n]);
                ui::invalidate(view);
            }
            requests::respond(id, Ok(&Response::None));
            return;
        }
        #[cfg(feature = "scm")]
        let Some(request) = scm::handle(id, request) else { return };
        let Request::Command((command, args)) = request else {
            // Timers and settings changes are counted, so tests can see them arrive.
            if let Request::Timer(_) | Request::SettingsChanged(_) = &request {
                let n = storage::get("events").map_or(0, |b| b.len()) + 1;
                let _ = storage::set("events", &vec![1u8; n]);
            }
            providers::handle(id, request);
            return;
        };
        if providers::command(id, &command).is_some() {
            return;
        }
        #[cfg(feature = "scm")]
        if scm::command(id, &command, &args).is_some() {
            return;
        }
        match command.as_str() {
            "echo" => reply(id, args),
            "env" => reply(id, env::info().api_version),
            "settings" => reply(id, format!("{:?}", settings::get("interval"))),
            "notice" => wait(window::show_notice(window::Severity::Info, "Pick", &["A".into(), "B".into()]), id),
            "read" => wait(workspace::read_file("file:///work/a.txt"), id),
            "fetch" => {
                let r = HttpRequest { method: "GET".into(), url: args, headers: vec![], body: vec![], timeout_ms: None };
                wait(codeyard::api::net::fetch(&r), id)
            }
            "store" => {
                let r = storage::set("k", args.as_bytes());
                reply(id, format!("{r:?} {:?}", storage::keys()))
            }
            "load" => reply(id, format!("{:?}", storage::get("k").map(String::from_utf8))),
            "events" => reply(id, format!("{}", storage::get("events").map_or(0, |b| b.len()))),
            "print" => {
                println!("hello from stdout");
                eprintln!("hello from stderr");
                requests::respond(id, Ok(&Response::None));
            }
            "spin" => {
                let mut x: u64 = 0;
                loop {
                    x = std::hint::black_box(x.wrapping_add(1));
                }
            }
            "grow" => {
                let mut keep: Vec<Vec<u8>> = Vec::new();
                loop {
                    keep.push(vec![1u8; 16 << 20]);
                    std::hint::black_box(&keep);
                }
            }
            "trap" => panic!("asked to trap"),
            "pick" => {
                let items = [window::PickItem { label: "One".into(), detail: None, picked: false }, window::PickItem { label: "Two".into(), detail: None, picked: false }];
                let options = window::PickOptions { title: None, placeholder: Some("Pick one".into()), many: false };
                wait(window::quick_pick(&items, &options), id)
            }
            "ask" => {
                let options = window::InputOptions { title: None, prompt: Some("Name?".into()), value: Some("x".into()), placeholder: None, password: false };
                wait(window::input_box(&options), id)
            }
            "doc" => wait(documents::active(), id),
            "auth" => wait(codeyard::api::auth::session("github", &[]), id),
            "auth-acme" => wait(codeyard::api::auth::session("acme", &[]), id),
            "authfetch" => wait(codeyard::api::auth::fetch("github", &get("https://api.github.com/user/repos")), id),
            "authfetch-other" => wait(codeyard::api::auth::fetch("github", &get("https://api.example.com/steal")), id),
            "authfetch-acme" => wait(codeyard::api::auth::fetch("acme", &get("https://api.acme.example.com/deployments")), id),
            "netfetch" => wait(codeyard::api::net::fetch(&get("https://api.example.com/status")), id),
            "secret-set" => wait(codeyard::api::secrets::set("api-key", "s3cr3t"), id),
            "secret-get" => wait(codeyard::api::secrets::get("api-key"), id),
            "every" => reply(id, format!("{}", timers::every(30))),
            // `commands.execute`: the argument is "<command> <argument>"; the argument goes as a JSON string, or null.
            "exec" => {
                let line = args.trim_matches('"');
                let (command, arg) = line.split_once(' ').map_or((line, "null".to_string()), |(c, a)| (c, format!("\"{a}\"")));
                wait(codeyard::api::commands::execute(command, &arg), id)
            }
            // What an extension can try from inside Wasm: WASI files, sockets and processes. Every line must fail.
            // Only in probe-escape.wasm: its socket imports would fail the package import check, by design.
            #[cfg(feature = "escape")]
            "escape" => {
                let tries = [
                    ("read", std::fs::read(&args).map(drop)),
                    ("list", std::fs::read_dir("/").map(drop)),
                    ("write", std::fs::write("/tmp/codeyard-escape", b"x")),
                    ("connect", std::net::TcpStream::connect("1.1.1.1:443").map(drop)),
                    ("bind", std::net::TcpListener::bind("127.0.0.1:0").map(drop)),
                    ("exec", std::process::Command::new("/bin/sh").status().map(drop)),
                ];
                let lines: Vec<String> = tries.iter().map(|(what, r)| format!("{what}: {}", if r.is_ok() { "ok" } else { "refused" })).collect();
                reply(id, lines.join("\n"))
            }
            _ => requests::respond(id, Err(&Error::Invalid(format!("unknown command {command}")))),
        }
    }

    fn cancel(id: u64) {
        providers::cancel(id);
    }

    fn complete(ticket: Ticket, result: Completion) {
        #[cfg(feature = "scm")]
        if scm::complete(ticket, &result) {
            return;
        }
        let body = BODIES.with(|b| b.borrow_mut().remove(&ticket));
        if pulls::complete(ticket, &result, body.as_deref()).is_some() {
            return;
        }
        if let Some(id) = WAITING.with(|w| w.borrow_mut().remove(&ticket)) {
            match body {
                Some(b) => reply(id, format!("{} body {}", describe(&result), String::from_utf8_lossy(&b))),
                None => reply(id, describe(&result)),
            }
        }
    }

    fn event(ticket: Ticket, event: StreamEvent) {
        if let StreamEvent::Data(d) = event {
            BODIES.with(|b| b.borrow_mut().entry(ticket).or_default().extend_from_slice(&d));
        }
    }
}

export!(Probe);
