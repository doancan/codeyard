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

[[diagnostics]]
id        = "lint"
languages = ["rust", "toml"]

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
        ctx.command("diagnose", async |ctx, args| {
            let uri = args["uri"].as_str().unwrap_or_default().to_string();
            let messages: Vec<String> = args["messages"].as_array().into_iter().flatten().filter_map(|m| m.as_str()).map(str::to_string).collect();
            let list: Vec<Diagnostic> = messages.iter().map(|m| Diagnostic::new(Range::line(0, 0, 1), DiagnosticSeverity::Warning, m.as_str())).collect();
            ctx.set_diagnostics(&uri, &list);
            Ok(())
        });
        ctx.command("clear", async |ctx, args| {
            ctx.clear_diagnostics(args.as_str());
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

fn diagnose(h: &MockHost, uri: &str, messages: &[&str]) {
    h.command("diagnose", json!({ "uri": uri, "messages": messages })).unwrap();
}

fn warned(h: &MockHost, what: &str) -> bool {
    h.logs().iter().any(|(l, m)| *l == LogLevel::Warn && m.contains(what))
}

#[test]
fn diagnostics_follow_codeyards_rules() {
    let h = host();
    // A declared language, a file in the workspace: kept, trimmed.
    diagnose(&h, "file:///work/src/lib.rs", &["  unused import ", "", "   "]);
    let kept = h.diagnostics("src/lib.rs");
    assert_eq!(kept.iter().map(|d| d.message.as_str()).collect::<Vec<_>>(), ["unused import"]);
    assert!(warned(&h, "2 with an empty message or hidden characters dropped"), "{:?}", h.logs());
    // Bidirectional and zero-width characters drop the message; line breaks and tabs do not.
    diagnose(&h, "file:///work/src/lib.rs", &["a\u{202e}b", "c\u{200b}d", "line one\n\tline two"]);
    assert_eq!(h.diagnostics("src/lib.rs").iter().map(|d| d.message.as_str()).collect::<Vec<_>>(), ["line one\n\tline two"]);
    // Cut at 4096 characters, with a `…`.
    diagnose(&h, "file:///work/src/lib.rs", &[&"ğ".repeat(5000)]);
    let long = &h.diagnostics("src/lib.rs")[0].message;
    assert_eq!((long.chars().count(), long.ends_with('…')), (4097, true));
    // A language the manifest does not list, and a file outside the workspace that is not open.
    diagnose(&h, "file:///work/main.go", &["no"]);
    assert!(h.diagnostics("main.go").is_empty() && warned(&h, "does not list its language, go"), "{:?}", h.logs());
    diagnose(&h, "file:///work/notes.md", &["no"]);
    assert!(warned(&h, "does not list its language, plaintext"), "Codeyard has no markdown language id: {:?}", h.logs());
    diagnose(&h, "file:///etc/hosts.rs", &["no"]);
    assert!(h.diagnostics("file:///etc/hosts.rs").is_empty() && warned(&h, "neither in the workspace nor open in an editor"), "{:?}", h.logs());
    // Open in the editor, it is allowed wherever it is.
    h.active_document("file:///elsewhere/lib.rs", "rust", "fn main() {}");
    diagnose(&h, "file:///elsewhere/lib.rs", &["fine"]);
    assert_eq!(h.diagnostics("file:///elsewhere/lib.rs").len(), 1);
}

#[test]
fn diagnostics_need_a_declaration_and_stop_at_a_thousand_files() {
    let h = MockHost::allow_all();
    h.manifest(&MANIFEST.replace("[[diagnostics]]\nid        = \"lint\"\nlanguages = [\"rust\", \"toml\"]\n", ""));
    h.activate::<Parity>().unwrap();
    diagnose(&h, "file:///work/a.rs", &["x"]);
    assert!(h.diagnostics("a.rs").is_empty() && warned(&h, "declares no [[diagnostics]]"), "{:?}", h.logs());

    let h = host();
    for i in 0..1000 {
        diagnose(&h, &format!("file:///work/f{i}.rs"), &["x"]);
    }
    diagnose(&h, "file:///work/one-too-many.rs", &["x"]);
    assert!(h.diagnostics("one-too-many.rs").is_empty() && warned(&h, "1000 files already"));
    // A file that already has some may change, and clearing makes room.
    diagnose(&h, "file:///work/f0.rs", &["y"]);
    assert_eq!(h.diagnostics("f0.rs")[0].message, "y");
    h.command("clear", json!("file:///work/f1.rs")).unwrap();
    diagnose(&h, "file:///work/one-too-many.rs", &["x"]);
    assert_eq!(h.diagnostics("one-too-many.rs").len(), 1);
}

#[test]
fn an_extensions_own_language_counts_for_diagnostics() {
    let h = MockHost::allow_all();
    h.manifest(
        &MANIFEST
            .replace("languages = [\"rust\", \"toml\"]", "languages = [\"acme\"]")
            .replace("[[hover]]", "[[languages]]\nid = \"acme\"\nextensions = [\"acme\"]\n\n[[hover]]"),
    );
    h.activate::<Parity>().unwrap();
    diagnose(&h, "file:///work/a.acme", &["fine"]);
    assert_eq!(h.diagnostics("a.acme").len(), 1, "{:?}", h.logs());
    diagnose(&h, "file:///work/a.rs", &["no"]);
    assert!(h.diagnostics("a.rs").is_empty() && warned(&h, "does not list its language, rust"));
}
