//! A test extension for codeyard-exthost's tests: each command exercises one part of the host, including the
//! hostile ones (spin, grow, trap).

wit_bindgen::generate!({ path: "../../../../ext-api/wit", world: "extension" });

use std::cell::RefCell;
use std::collections::HashMap;

use codeyard::api::types::{Completion, Error, HttpRequest, StreamEvent, Ticket};
use codeyard::api::{documents, env, log, requests, settings, storage, timers, window, workspace};
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
        let Request::Command((command, args)) = request else {
            // Timers and settings changes are counted, so tests can see them arrive.
            if let Request::Timer(_) | Request::SettingsChanged(_) = &request {
                let n = storage::get("events").map_or(0, |b| b.len()) + 1;
                let _ = storage::set("events", &vec![1u8; n]);
            }
            requests::respond(id, Ok(&Response::None));
            return;
        };
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
            _ => requests::respond(id, Err(&Error::Invalid(format!("unknown command {command}")))),
        }
    }

    fn cancel(_id: u64) {}

    fn complete(ticket: Ticket, result: Completion) {
        let body = BODIES.with(|b| b.borrow_mut().remove(&ticket));
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
