//! The workspace calls the Rust SDK shares with WIT and TypeScript: reading many files at once, searching, renaming
//! and creating folders, on the mock host.

use codeyard_extension::prelude::*;

use crate::MockHost;

const MANIFEST: &str = r#"
[extension]
id      = "you.files"
version = "0.1.0"
name    = "Files"
license = "MIT"

[code]
entry = "Cargo.toml"

[permissions]
read  = ["*.md", "notes/**"]
write = ["notes/**"]
commands = ["core:editor.open"]

[[commands]]
id    = "go"
title = "Go"
"#;

struct Files;

fn uri(path: &str) -> String {
    format!("file:///work/{path}")
}

impl Extension for Files {
    fn activate(ctx: Context) -> Result<()> {
        ctx.command("read-many", async |ctx, args| {
            let uris: Vec<String> = args.as_array().into_iter().flatten().filter_map(|p| p.as_str()).map(uri).collect();
            let uris: Vec<&str> = uris.iter().map(String::as_str).collect();
            let files = ctx.read_many(&uris).await?;
            Ok(files.into_iter().map(|(u, r)| json!([u, r.map(|b| String::from_utf8_lossy(&b).into_owned()).map_err(|e| e.to_string())])).collect::<Vec<_>>())
        });
        ctx.command("search", async |ctx, args| {
            let mut found = Vec::new();
            ctx.search(&SearchQuery::new(args.as_str().unwrap_or_default()), |m| found.push(json!([m.uri, m.range.start.line, m.line]))).await?;
            Ok(found)
        });
        ctx.command("move", async |ctx, _args| {
            ctx.create_directory(&uri("notes/old")).await?;
            ctx.rename(&uri("notes/a.md"), &uri("notes/old/a.md")).await
        });
        ctx.command("open", async |ctx, args| ctx.execute("core:editor.open", args).await);
        ctx.command("read-uri", async |ctx, args| ctx.read_text(args.as_str().unwrap_or_default()).await);
        ctx.command("escape", async |ctx, _args| ctx.rename(&uri("notes/a.md"), &uri("a.md")).await);
        Ok(())
    }
}

fn host() -> MockHost {
    let h = MockHost::allow_all();
    h.manifest(MANIFEST);
    h.activate::<Files>().unwrap();
    h.file("README.md", "# Files\nTODO: write\n").file("notes/a.md", "todo: more").file("secret.txt", "todo: hidden");
    h
}

#[test]
fn read_many_gives_each_file_its_own_result() {
    let h = host();
    let out = h.command("read-many", json!(["README.md", "notes/a.md", "missing.md"])).unwrap();
    assert_eq!(out[0], json!([uri("README.md"), { "Ok": "# Files\nTODO: write\n" }]));
    assert_eq!(out[1][1], json!({ "Ok": "todo: more" }));
    assert!(out[2][1]["Err"].as_str().is_some(), "{out}");
    // A file outside `read` refuses the call with the line to add.
    let e = h.command("read-many", json!(["README.md", "secret.txt"])).unwrap_err();
    assert!(e.to_string().contains(r#"read = ["secret.txt"]"#), "{e}");
}

#[test]
fn search_streams_its_matches() {
    let h = host();
    let out = h.command("search", json!("todo")).unwrap();
    // Only files the extension may read: not secret.txt.
    assert_eq!(out, json!([[uri("README.md"), 1, "TODO: write"], [uri("notes/a.md"), 0, "todo: more"]]));
}

#[test]
fn rename_and_create_directory_need_write() {
    let h = host();
    h.command("move", Value::Null).unwrap();
    assert_eq!(h.read("notes/old/a.md").as_deref(), Some(&b"todo: more"[..]));
    assert_eq!(h.read("notes/a.md"), None);
    h.file("notes/a.md", "again");
    let e = h.command("escape", Value::Null).unwrap_err();
    assert!(e.to_string().contains(r#"write = ["a.md"]"#), "{e}");
}

/// `core:editor.open` is judged as the app judges it: its argument, then `read` on the file.
#[test]
fn editor_open_checks_its_argument_and_the_file() {
    let h = host();
    for bad in [json!([]), json!(7), json!(null), json!({ "uri": 1 })] {
        let e = h.command("open", bad.clone()).unwrap_err().to_string();
        assert!(e.contains("core:editor.open takes a `file://` URI"), "{bad}: {e}");
    }
    // Never a file inside the workspace: refused, and no entry can lift it.
    for outside in ["https://example.com/a.md", "file:///etc/passwd"] {
        let e = h.command("open", json!(outside)).unwrap_err().to_string();
        assert!(e.contains("cannot be granted") || e.contains("denied"), "{outside}: {e}");
    }
    let e = h.command("open", json!(uri("secret.txt"))).unwrap_err().to_string();
    assert!(e.contains(r#"read = ["secret.txt"]"#), "{e}");
    h.command("open", json!(uri("README.md"))).unwrap();
    let range = json!({ "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } });
    h.command("open", json!({ "uri": uri("notes/a.md"), "selection": range })).unwrap();
}

/// A fake file is found by the path Codeyard reads: `..`, `//` and `%6d` are resolved first.
#[test]
fn fake_files_are_found_by_their_resolved_path() {
    let h = host();
    for path in ["./README.md", "notes/../README.md", "/README.md", "%52EADME.md", "notes/%2e%2e/README.md"] {
        let out = h.command("read-uri", json!(format!("file:///work/{path}"))).unwrap();
        assert_eq!(out, json!("# Files\nTODO: write\n"), "{path}");
    }
}
