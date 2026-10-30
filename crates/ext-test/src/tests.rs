//! The SDK and the mock host together: one test extension, each command using one part of the API.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use codeyard_extension::native::{Call, Note};
use codeyard_extension::prelude::*;

use crate::{MockHost, ProcessFixture};

const MANIFEST: &str = r#"
[extension]
id      = "you.probe"
version = "0.1.0"
name    = "Probe"
license = "MIT"

[code]
entry = "Cargo.toml"

[permissions]
read = ["*.json"]
net  = ["api.example.com"]
run  = ["git"]
why  = "Tests."

[[commands]]
id    = "greet"
title = "Greet"

[[views]]
id       = "counter"
title    = "Counter"
location = "sidebar"

[settings.interval]
type        = "number"
default     = 60
description = "Seconds."
"#;

struct Probe;

thread_local! {
    static TICKS: Rc<Cell<u32>> = Rc::new(Cell::new(0));
}

impl Extension for Probe {
    fn activate(ctx: Context) -> Result<()> {
        ctx.command("greet", async |ctx, _args| {
            match ctx.input(InputOptions::prompt("Your name?")).await? {
                Some(name) => ctx.notice(format!("Hello, {name}!")),
                None => ctx.notice("Hello, stranger."),
            }
            Ok(())
        });
        ctx.command("echo", async |_ctx, args| Ok(args));
        ctx.command("pick", async |ctx, _args| ctx.quick_pick(["a", "b", "c"], PickOptions::default()).await);
        ctx.command("fetch", async |ctx, args| {
            let url = args.as_str().unwrap_or_default().to_string();
            let r = ctx.fetch(Request::get(url)).await?.error_for_status()?;
            r.json::<Value>()
        });
        ctx.command("read", async |ctx, args| ctx.read_text(&format!("file:///work/{}", args.as_str().unwrap_or_default())).await);
        ctx.command("git", async |ctx, _args| {
            let mut p = ctx.spawn(Command::new("git").args(["log", "--oneline"]));
            let mut lines = 0;
            while let Some(chunk) = p.next().await {
                if let Chunk::Stdout(b) = chunk {
                    lines += b.iter().filter(|&&c| c == b'\n').count();
                }
            }
            Ok(json!({ "lines": lines, "code": p.wait().await? }))
        });
        ctx.command("abandon", async |ctx, _args| {
            drop(ctx.spawn(Command::new("git").arg("status")));
            Ok(())
        });
        ctx.command("count", async |ctx, _args| {
            let n: u32 = ctx.storage().get_json("n")?.unwrap_or(0) + 1;
            ctx.storage().set_json("n", &n)?;
            Ok(n)
        });
        ctx.command("tally", async |ctx, _args| {
            let n: u32 = ctx.state().get("tally").unwrap_or(0) + 1;
            ctx.state().set("tally", &n)?;
            Ok(json!({ "tally": n, "reason": ctx.activation_reason() }))
        });
        ctx.command("interval", async |ctx, _args| ctx.setting::<u32>("interval"));
        ctx.command("words", async |ctx, _args| {
            let doc = ctx.active_document().await?.ok_or("no document")?;
            ctx.set_diagnostics(&doc.uri, &[Diagnostic::new(Range::line(0, 0, 1), DiagnosticSeverity::Hint, "first")]);
            Ok(doc.text.split_whitespace().count())
        });
        ctx.command("fail", async |_ctx, _args| -> Result<()> { Err(Error::other("on purpose")) });
        ctx.view("counter", async |ctx| Ok(json!({ "type": "text", "text": ctx.storage().get_json::<u32>("n")?.unwrap_or(0).to_string() })));
        ctx.on_view_event("counter", async |ctx, _event| {
            ctx.invalidate("counter");
            Ok(())
        });
        ctx.formatter(async |_ctx, doc, _range, _options| Ok(vec![TextEdit::replace(Range::line(0, 0, doc.text.len() as u32), doc.text.trim())]));
        ctx.hover(async |_ctx, doc, position| {
            let line = doc.text.lines().nth(position.line as usize).unwrap_or_default();
            Ok(line.contains("TODO").then(|| Hover::new(format!("**TODO** on line {}", position.line + 1))))
        });
        ctx.completion(async |_ctx, _doc, _position, trigger| {
            let mut items = vec![CompletionItem::new("todo", CompletionKind::Keyword)];
            if trigger.as_deref() == Some(".") {
                items.push(CompletionItem { insert_text: Some("len()$0".into()), snippet: true, ..CompletionItem::new("len", CompletionKind::Method) });
            }
            Ok(items)
        });
        // A lens on each `fn` line in view that echoes its line, and one resolved later; a hint with each line's length.
        ctx.code_lenses(async |_ctx, doc, range| {
            let lines = doc.text.lines().enumerate().skip(range.start.line as usize).take((range.end.line - range.start.line) as usize);
            let mut out: Vec<CodeLens> =
                lines.filter(|(_, l)| l.starts_with("fn ")).map(|(n, _)| CodeLens::new(n as u32, "Run", "echo", json!({ "line": n }))).collect();
            out.push(CodeLens::unresolved(range.start.line, "later"));
            Ok(out)
        });
        ctx.resolve_code_lens(async |_ctx, _doc, mut lens| {
            lens.command = Some(LensCommand { title: format!("Resolved {}", lens.id), command: "echo".into(), args: json!(lens.id) });
            Ok(lens)
        });
        ctx.inlay_hints(async |_ctx, doc, range| {
            let lines = doc.text.lines().enumerate().skip(range.start.line as usize).take((range.end.line - range.start.line) as usize);
            Ok(lines.map(|(n, l)| InlayHint::new(Position::new(n as u32, l.len() as u32), format!("{} chars", l.len()))).collect())
        });
        let ticks = TICKS.with(Rc::clone);
        ctx.every(60, async move |_ctx| ticks.set(ticks.get() + 1));
        Ok(())
    }
}

fn host() -> MockHost {
    let h = MockHost::allow_all();
    h.manifest(MANIFEST);
    h.activate::<Probe>().unwrap();
    h
}

#[test]
fn a_command_waits_for_input_and_shows_a_notice() {
    let h = host();
    h.answer_input("Ada");
    h.command("greet", Value::Null).unwrap();
    h.dismiss_input();
    h.command("greet", Value::Null).unwrap();
    assert_eq!(h.notices(), ["Hello, Ada!", "Hello, stranger."]);
    assert_eq!(h.command("echo", json!([1, 2])).unwrap(), json!([1, 2]));
    h.answer_pick(&[2]);
    assert_eq!(h.command("pick", Value::Null).unwrap(), json!(2));
    assert_eq!(h.command("pick", Value::Null).unwrap(), Value::Null);
}

#[test]
fn fetch_uses_fixtures_and_undeclared_hosts_are_denied_with_the_fix() {
    let h = host();
    h.http("GET", "https://api.example.com/v1", 200, r#"{"ok":true}"#);
    assert_eq!(h.command("fetch", json!("https://api.example.com/v1")).unwrap(), json!({"ok": true}));
    assert_eq!(h.fetched()[0].url, "https://api.example.com/v1");
    // No fixture: a clear error, not a hang.
    let e = h.command("fetch", json!("https://api.example.com/missing")).unwrap_err();
    assert!(e.to_string().contains("host.http"), "{e}");
    let e = h.command("fetch", json!("https://evil.example.org/x")).unwrap_err();
    let d = e.denied().expect("denied");
    assert_eq!(d.why, Why::NotDeclared);
    assert!(e.to_string().contains(r#"= fix: add to [permissions]:  net = ["evil.example.org"]"#), "{e}");
    assert_eq!(h.denied().len(), 1);
}

#[test]
fn reads_follow_the_read_globs() {
    let h = host();
    h.file("package.json", "{}").file("secret.txt", "x");
    assert_eq!(h.command("read", json!("package.json")).unwrap(), json!("{}"));
    let e = h.command("read", json!("secret.txt")).unwrap_err();
    assert!(e.to_string().contains(r#"read = ["secret.txt"]"#), "{e}");
}

#[test]
fn processes_stream_and_dropping_one_cancels_it() {
    let h = host();
    h.process("git", &["log", "--oneline"], ProcessFixture::ok("a\nb\nc\n"));
    assert_eq!(h.command("git", Value::Null).unwrap(), json!({"lines": 3, "code": 0}));
    assert_eq!(h.ran()[0].program, "git");
    h.process("git", &["status"], ProcessFixture::ok(""));
    h.command("abandon", Value::Null).unwrap();
    assert!(h.notes().iter().any(|n| matches!(n, Note::CancelTicket(_))), "{:?}", h.notes());
}

#[test]
fn storage_settings_and_documents() {
    let h = host();
    assert_eq!(h.command("count", Value::Null).unwrap(), json!(1));
    assert_eq!(h.command("count", Value::Null).unwrap(), json!(2));
    assert_eq!(h.storage()["n"], b"2");
    assert_eq!(h.command("interval", Value::Null).unwrap(), json!(60));
    h.setting("interval", Some("90"));
    assert_eq!(h.command("interval", Value::Null).unwrap(), json!(90));
    h.active_document("notes.md", "markdown", "one two three");
    assert_eq!(h.command("words", Value::Null).unwrap(), json!(3));
    assert_eq!(h.diagnostics("notes.md")[0].message, "first");
    assert!(matches!(h.calls().last(), Some(Call::ActiveDocument)));
}

#[test]
fn state_survives_a_hot_reload_only() {
    let h = host();
    h.command("tally", Value::Null).unwrap();
    assert_eq!(h.command("tally", Value::Null).unwrap(), json!({ "tally": 2, "reason": "command" }));
    h.reload::<Probe>().unwrap();
    assert_eq!(h.command("tally", Value::Null).unwrap(), json!({ "tally": 3, "reason": "reload" }));
    // The snapshot is read once: it does not linger in storage.
    assert!(!h.storage().contains_key(codeyard_extension::RELOAD_STATE_KEY));
    // A start for any other reason begins empty.
    h.deactivate::<Probe>();
    assert!(h.storage().contains_key(codeyard_extension::RELOAD_STATE_KEY));
    codeyard_extension::native::forget();
    h.activate::<Probe>().unwrap();
    assert_eq!(h.command("tally", Value::Null).unwrap()["tally"], json!(1));
    assert!(!h.storage().contains_key(codeyard_extension::RELOAD_STATE_KEY));
}

#[test]
fn errors_views_formatters_and_timers() {
    let h = host();
    assert_eq!(h.command("fail", Value::Null).unwrap_err(), Error::Other("on purpose".into()));
    let e = h.command("nope", Value::Null).unwrap_err();
    assert!(e.to_string().contains("no command 'nope'"), "{e}");
    h.command("count", Value::Null).unwrap();
    assert_eq!(h.render("counter").unwrap(), json!({"type": "text", "text": "1"}));
    h.view_event("counter", json!({"click": "inc"})).unwrap();
    assert_eq!(h.invalidated(), ["counter"]);
    let doc = Document { uri: h.uri("a.txt"), language: "text".into(), version: 1, text: "  x  ".into() };
    let edits = h.format(doc, None, FormatOptions { tab_size: 4, insert_spaces: true }).unwrap();
    assert_eq!(edits[0].new_text, "x");
    // The virtual clock fires the 60 s timer twice in 150 s.
    h.advance(Duration::from_secs(150));
    assert_eq!(TICKS.with(|t| t.get()), 2);
}

#[test]
fn code_lenses_resolve_and_run_their_command_and_hints_cover_the_range() {
    let h = host();
    let doc = Document { uri: h.uri("a.rs"), language: "rust".into(), version: 1, text: "fn a() {}\nlet x = 1;\nfn b() {}\n".into() };
    let lenses = h.code_lenses(doc.clone(), Range::new(Position::new(1, 0), Position::new(3, 0))).unwrap();
    assert_eq!(lenses.len(), 2, "only the lines asked for: {lenses:?}");
    assert_eq!(lenses[0].range.start.line, 2);
    assert_eq!(h.click_code_lens(&lenses[0]).unwrap(), json!({ "line": 2 }), "the command runs with the lens's arguments");
    assert!(h.click_code_lens(&lenses[1]).is_err(), "an unresolved lens has nothing to run");
    let resolved = h.resolve_code_lens(doc.clone(), lenses[1].clone()).unwrap();
    assert_eq!(resolved.command.as_ref().map(|c| c.title.as_str()), Some("Resolved later"));
    assert_eq!(h.click_code_lens(&resolved).unwrap(), json!("later"));
    let hints = h.inlay_hints(doc, Range::new(Position::new(0, 0), Position::new(2, 0))).unwrap();
    assert_eq!(hints.iter().map(|h| h.label.as_str()).collect::<Vec<_>>(), ["9 chars", "10 chars"]);
    assert_eq!(hints[1].position, Position::new(1, 10));
}

#[test]
fn an_untrusted_folder_refuses_sending() {
    let h = host();
    h.untrusted();
    let e = h.command("fetch", json!("https://api.example.com/v1")).unwrap_err();
    assert_eq!(e.denied().map(|d| d.why), Some(Why::UntrustedWorkspace));
}

#[test]
fn hover_and_completion_answer_for_a_position() {
    let h = host();
    let doc = Document { uri: h.uri("a.rs"), language: "rust".into(), version: 1, text: "fn main() {}\n// TODO\n".into() };
    assert_eq!(h.hover(doc.clone(), Position::new(1, 3)).unwrap(), Some(Hover::new("**TODO** on line 2")));
    assert_eq!(h.hover(doc.clone(), Position::new(0, 3)).unwrap(), None);
    let words = h.completion(doc.clone(), Position::new(0, 0), None).unwrap();
    assert_eq!(words.items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(), ["todo"]);
    assert!(!words.incomplete);
    let dot = h.completion(doc, Position::new(0, 0), Some(".")).unwrap();
    assert_eq!(dot.items[1].insert_text.as_deref(), Some("len()$0"));
}
