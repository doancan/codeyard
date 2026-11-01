//! The mock refuses what Codeyard refuses, with the same rules (`ext_manifest`): found by building extensions against
//! the mock and then running them in the app, where they failed.

use codeyard_extension::prelude::*;

use crate::MockHost;

const MANIFEST: &str = r#"
[extension]
id      = "you.parity"
version = "0.1.0"
name    = "Parity"
license = "MIT"

[code]
entry = "Cargo.toml"

[permissions]
read = ["main.rs", "docs/**"]
net  = ["api.example.com"]
why  = "Tests."

[[commands]]
id    = "read"
title = "Read"

[[status-bar]]
id = "deploys"

[[hover]]
languages = ["rust"]

[[decorations]]
id        = "marks"
languages = ["rust"]

[settings.interval]
type        = "number"
default     = 60
description = "Seconds."
"#;

struct Parity;

impl Extension for Parity {
    fn activate(ctx: Context) -> Result<()> {
        ctx.command("read", async |ctx, args| ctx.read_text(args.as_str().unwrap_or_default()).await);
        ctx.command("find", async |ctx, args| ctx.find_files(args.as_str().unwrap_or_default(), 100).await);
        ctx.command("fetch", async |ctx, args| Ok(ctx.fetch(Request::get(args.as_str().unwrap_or_default())).await?.status));
        ctx.command("status", async |ctx, args| {
            let item =
                StatusItem { id: args["id"].as_str().unwrap_or_default().into(), text: args["text"].as_str().unwrap_or_default().into(), ..Default::default() };
            ctx.set_status_item(item)
        });
        ctx.command("huge", async |_ctx, _args| Ok("x".repeat(20 << 20)));
        ctx.command("open", async |ctx, _args| ctx.open_external("https://example.com/").await);
        ctx.command("mark", async |ctx, _args| {
            let mark = Decoration { range: Range::line(0, 0, 1), place: DecorationPlace::After, tone: Tone::Muted, text: Some("here".into()), hover: None };
            ctx.set_decorations("file:///work/main.rs", "marks", &[mark]);
            Ok(())
        });
        // A provider is not a user action, so it may not open links.
        ctx.hover(async |ctx, _doc, _position| ctx.open_external("https://example.com/").await.map(|()| None));
        // `?` in the handlers nobody waits for; the error goes to the log.
        ctx.on_settings_changed(async |ctx, _keys| {
            let interval: u32 = ctx.setting("interval")?.ok_or("no interval")?;
            ctx.log(LogLevel::Info, format!("interval {interval}"));
            Ok(())
        });
        Ok(())
    }
}

fn host() -> MockHost {
    let h = MockHost::allow_all();
    h.manifest(MANIFEST);
    h.activate::<Parity>().unwrap();
    h
}

#[test]
fn a_file_name_glob_is_the_file_at_the_top_only() {
    let h = host();
    h.file("main.rs", "fn main() {}").file("src/main.rs", "fn main() {}").file("docs/a/b.md", "# b");
    assert_eq!(h.command("read", json!("file:///work/main.rs")).unwrap(), json!("fn main() {}"));
    let e = h.command("read", json!("file:///work/src/main.rs")).unwrap_err();
    assert!(e.to_string().contains(r#"read = ["src/main.rs"]"#), "{e}");
    assert_eq!(h.command("read", json!("file:///work/docs/a/b.md")).unwrap(), json!("# b"));
    // `find-files` matches the whole path too, and returns only what `read` allows.
    assert_eq!(h.command("find", json!("*.rs")).unwrap(), json!(["file:///work/main.rs"]));
    assert_eq!(h.command("find", json!("**/*.rs")).unwrap(), json!(["file:///work/main.rs"]));
}

#[test]
fn a_path_out_of_the_folder_is_refused_however_it_is_spelled() {
    let h = host();
    for uri in ["file:///work/docs/../../etc/passwd", "file:///work/docs/%2e%2e/%2e%2e/etc/passwd", "file:///etc/passwd"] {
        let e = h.command("read", json!(uri)).unwrap_err();
        // As in Codeyard: no line to add, since no entry reaches outside the workspace.
        assert!(e.to_string().contains("none: this cannot be granted, the file is outside the workspace"), "{uri}: {e}");
    }
}

#[test]
fn urls_codeyard_never_fetches_are_invalid_even_with_a_fixture() {
    let h = host();
    h.http("GET", "http://api.example.com/zen", 200, "plain").http("GET", "https://me:pw@api.example.com/zen", 200, "creds");
    for (url, why) in [("http://api.example.com/zen", "only https://"), ("https://me:pw@api.example.com/zen", "user name or password")] {
        let e = h.command("fetch", json!(url)).unwrap_err();
        assert!(matches!(&e, Error::Invalid(m) if m.contains(why)), "{url}: {e:?}");
    }
    // An IP address is a denial no `net` entry can lift, as in Codeyard.
    let e = h.command("fetch", json!("https://127.0.0.1/")).unwrap_err();
    assert!(e.to_string().contains("none: this cannot be granted, it is an IP address"), "{e}");
    assert!(h.fetched().is_empty(), "nothing was sent");
}

#[test]
fn status_items_follow_codeyards_rule() {
    let h = host();
    h.command("status", json!({ "id": "deploys", "text": " Ready " })).unwrap();
    assert_eq!(h.status_items()["deploys"].text, "Ready");
    let e = h.command("status", json!({ "id": "other", "text": "Ready" })).unwrap_err();
    assert!(e.to_string().contains("status item \"other\" is not declared"), "{e}");
    let e = h.command("status", json!({ "id": "deploys", "text": "y".repeat(65) })).unwrap_err();
    assert!(e.to_string().contains("1 to 64 visible characters"), "{e}");
}

#[test]
fn an_answer_over_the_call_limit_fails_its_request() {
    let h = host();
    let e = h.command("huge", Value::Null).unwrap_err();
    assert_eq!(e, Error::Invalid("the result is 20 MiB; at most 8 MiB".into()));
}

#[test]
fn links_open_only_from_a_user_action() {
    let h = host();
    h.command("open", Value::Null).unwrap();
    let doc = Document { uri: h.uri("main.rs"), language: "rust".into(), version: 1, text: "fn main() {}".into() };
    let e = h.hover(doc, Position::new(0, 0)).unwrap_err();
    assert!(e.denied().is_some_and(|d| d.permission.contains("a user action")), "{e}");
}

#[test]
fn decorations_can_be_read_back() {
    let h = host();
    h.command("mark", Value::Null).unwrap();
    let marks = h.decorations("main.rs");
    assert_eq!(marks["marks"].len(), 1);
    assert_eq!(marks["marks"][0].text.as_deref(), Some("here"));
}

#[test]
fn a_settings_handler_may_use_the_question_mark() {
    let h = host();
    h.change_setting("interval", Some("90"));
    assert!(h.logs().iter().any(|(_, m)| m == "interval 90"), "{:?}", h.logs());
    h.change_setting("interval", None);
    assert!(h.logs().iter().any(|(l, m)| *l == LogLevel::Error && m.contains("the settings handler failed: no interval")), "{:?}", h.logs());
}
