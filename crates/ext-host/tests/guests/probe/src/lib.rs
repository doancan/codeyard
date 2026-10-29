//! A test extension for codeyard-exthost's tests: each command exercises one part of the host, including the
//! hostile ones (spin, grow, trap, escape).

wit_bindgen::generate!({ path: "../../../../ext-api/wit", world: "extension" });

mod providers;

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
}

struct Probe;

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
            providers::handle(id, request);
            return;
        };
        if providers::command(id, &command).is_some() {
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
            "every" => reply(id, format!("{}", timers::every(30))),
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
        if let Some(id) = WAITING.with(|w| w.borrow_mut().remove(&ticket)) {
            reply(id, describe(&result));
        }
    }

    fn event(_ticket: Ticket, _event: StreamEvent) {}
}

export!(Probe);
