//! API 1.1 in the mock host: watches of documents and files, `open_in_editor` and `net-from-setting`.

use std::cell::RefCell;
use std::rc::Rc;

use codeyard_extension::native::Call;
use codeyard_extension::prelude::*;

use crate::MockHost;

const MANIFEST: &str = r#"
[extension]
id      = "you.todos"
version = "0.1.0"
name    = "Todos"
license = "MIT"

[code]
entry = "Cargo.toml"

[permissions]
read             = ["src/**/*.rs"]
commands         = ["core:editor.open"]
net-from-setting = "server"
secrets          = true
why              = "Tests."

[[commands]]
id    = "watch"
title = "Watch"

[settings.server]
type        = "string"
default     = ""
description = "Your GitHub Enterprise host."
"#;

thread_local! {
    static SEEN: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
}

fn seen() -> Vec<String> {
    SEEN.with(|s| s.borrow_mut().drain(..).collect())
}

struct Todos;

impl Extension for Todos {
    fn activate(ctx: Context) -> Result<()> {
        ctx.command("watch", async |ctx, _| {
            let docs = ctx.on_documents(true, async |_, e| {
                SEEN.with(|s| s.borrow_mut().push(format!("{:?} {} v{} {:?}", e.change, e.uri, e.version, e.text)));
            })?;
            let files = ctx.on_files("src/**/*.rs", async |_, events| {
                for e in events {
                    SEEN.with(|s| s.borrow_mut().push(format!("{:?} {}", e.change, e.uri)));
                }
            })?;
            Ok(json!([format!("{docs:?}"), format!("{files:?}")]))
        });
        ctx.command("open", async |ctx, _| ctx.open_in_editor("file:///work/src/a.rs", Some(Range::line(9, 0, 4))).await.map(|()| Value::Null));
        ctx.command("fetch", async |ctx, args| {
            let r = ctx.fetch(Request::get(args.as_str().unwrap_or_default())).await?;
            Ok(json!(r.status))
        });
        Ok(())
    }
}

#[test]
fn watches_get_what_the_extension_may_read() {
    let host = MockHost::new();
    host.manifest(MANIFEST);
    host.open_document("src/old.rs", "rust", "// TODO");
    host.activate::<Todos>().unwrap();
    host.command("watch", Value::Null).unwrap();
    // A document already open arrives first.
    assert_eq!(seen(), [r#"Opened file:///work/src/old.rs v1 Some("// TODO")"#]);

    host.open_document("src/a.rs", "rust", "fn a() {}");
    host.edit_document("src/a.rs", "fn a() {} // TODO");
    host.save_document("src/a.rs");
    // Not in `read`: nothing.
    host.open_document("notes.md", "markdown", "secret");
    assert_eq!(
        seen(),
        [
            r#"Opened file:///work/src/a.rs v1 Some("fn a() {}")"#,
            r#"Changed file:///work/src/a.rs v2 Some("fn a() {} // TODO")"#,
            r#"Saved file:///work/src/a.rs v2 Some("fn a() {} // TODO")"#,
        ]
    );

    host.create_file("src/b.rs", "x");
    host.change_file("src/b.rs", "y");
    host.delete_file("src/b.rs");
    // Outside the glob, or outside `read`.
    host.create_file("src/b.txt", "x");
    host.create_file(".env", "KEY=1");
    assert_eq!(seen(), ["Created file:///work/src/b.rs", "Changed file:///work/src/b.rs", "Deleted file:///work/src/b.rs"]);
}

#[test]
fn a_stopped_watch_gets_nothing_and_one_without_read_is_refused() {
    struct Once;
    impl Extension for Once {
        fn activate(ctx: Context) -> Result<()> {
            let w = ctx.on_files("*.rs", async |_, events| SEEN.with(|s| s.borrow_mut().push(format!("{}", events.len()))))?;
            w.stop();
            Ok(())
        }
    }
    let host = MockHost::new();
    host.manifest(MANIFEST);
    host.activate::<Once>().unwrap();
    host.create_file("src/c.rs", "x");
    assert!(seen().is_empty());

    let host = MockHost::new();
    host.manifest(&MANIFEST.replace(r#"read             = ["src/**/*.rs"]"#, ""));
    let err = host.activate::<Once>().unwrap_err();
    assert!(err.contains(r#"read = ["*.rs"]"#), "{err}");
}

#[test]
fn open_in_editor_sends_the_selection() {
    let host = MockHost::new();
    host.manifest(MANIFEST);
    host.activate::<Todos>().unwrap();
    host.command("open", Value::Null).unwrap();
    let args = host.calls().into_iter().find_map(|c| match c {
        Call::Execute { command, args_json } if command == "core:editor.open" => Some(args_json),
        _ => None,
    });
    let args: Value = serde_json::from_str(&args.unwrap()).unwrap();
    assert_eq!(args, json!({"uri": "file:///work/src/a.rs", "selection": {"start": {"line": 9, "character": 0}, "end": {"line": 9, "character": 4}}}));
}

#[test]
fn net_from_setting_reaches_only_hosts_the_user_allowed() {
    let host = MockHost::new();
    host.manifest(MANIFEST).http("GET", "https://ghe.example.com/api/v3/user", 200, "{}");
    host.activate::<Todos>().unwrap();
    let err = host.command("fetch", json!("https://ghe.example.com/api/v3/user")).unwrap_err();
    let denied = err.denied().unwrap();
    assert_eq!(denied.permission, r#"net-from-setting host = "ghe.example.com""#);
    assert!(err.to_string().contains("they add it on the extension page with Allow Host…"), "{err}");
    host.allow_host("ghe.example.com");
    assert_eq!(host.command("fetch", json!("https://ghe.example.com/api/v3/user")).unwrap(), json!(200));
}
