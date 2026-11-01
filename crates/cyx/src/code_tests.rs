//! Code extensions: the Rust and TypeScript templates, the examples built from them, `cyx package` and `cyx test`. The
//! tests that build a component need a built codeyard-exthost, and the wasm32-wasip2 target (Rust) or Node.js 22 with
//! network access for the first `npm ci` (TypeScript); without them they are skipped with a note, unless
//! CODEYARD_REQUIRE_WASM is set (CI's Linux job), where a skip is a failure.

use std::path::{Path, PathBuf};

use crate::*;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cyx-code-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The exthost to run, or why these tests cannot run here.
fn toolchain() -> Option<PathBuf> {
    let exthost = std::env::var_os("CODEYARD_EXTHOST").map(PathBuf::from).or_else(|| {
        // target/debug/deps/cyx-… → target/debug/codeyard-exthost
        let exe = if cfg!(windows) { "codeyard-exthost.exe" } else { "codeyard-exthost" };
        std::env::current_exe().ok()?.parent()?.parent().map(|d| d.join(exe)).filter(|p| p.is_file())
    });
    let missing = if !build::target_installed() {
        Some("the wasm32-wasip2 target is not installed (rustup target add wasm32-wasip2)")
    } else if exthost.is_none() {
        Some("codeyard-exthost is not built (cargo build -p ext-host, or set CODEYARD_EXTHOST)")
    } else {
        None
    };
    match missing {
        Some(why) if std::env::var_os("CODEYARD_REQUIRE_WASM").is_some() => panic!("CODEYARD_REQUIRE_WASM is set, but {why}"),
        Some(why) => {
            eprintln!("skipped: {why}");
            None
        }
        None => exthost,
    }
}

/// One cargo target folder for every component these tests build, so the SDK is compiled once.
fn target_dir() -> PathBuf {
    repo().join("target/cyx-wasm-tests")
}

fn files(dir: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if p.is_dir() {
                if name != "target" && name != "node_modules" {
                    stack.push(p);
                }
            } else if !["Cargo.lock", "package-lock.json", ext_pkg::COMPONENT, typescript::SOURCE_MAP].contains(&name.as_str()) {
                let rel = p.strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/");
                out.push((rel, std::fs::read_to_string(&p).unwrap().replace("\r\n", "\n")));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn the_examples_are_the_templates() {
    // examples/extensions/hello-* are `cyx new command|view hello-… [--lang ts] --publisher codeyard --sdk ../../..`.
    let root = tmp("examples");
    let ts = scaffold::Lang::TypeScript;
    for (kind, name, lang) in [
        (Kind::Command, "hello-command", scaffold::Lang::Rust),
        (Kind::View, "hello-view", scaffold::Lang::Rust),
        (Kind::Command, "hello-command-ts", ts),
        (Kind::View, "hello-view-ts", ts),
    ] {
        scaffold::create_in(&root.join(name), kind, name, "codeyard", Some("../../.."), lang).unwrap();
        let example = repo().join("examples/extensions").join(name);
        assert_eq!(files(&root.join(name)), files(&example), "{name}: regenerate it with `cyx new` (see examples/extensions/README.md)");
    }
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn templates_default_to_the_sdk_repository() {
    let root = tmp("git");
    scaffold::create_with(&root.join("tools"), Kind::Command, "my-tools", "you", None).unwrap();
    let cargo = std::fs::read_to_string(root.join("tools/Cargo.toml")).unwrap();
    let pin = scaffold::sdk_git_ref(env!("CARGO_PKG_VERSION"));
    assert!(cargo.contains(&format!("codeyard-extension = {{ git = \"{}\", {pin} }}", scaffold::SDK_REPOSITORY)), "{cargo}");
    // Pinned to the release of this cyx; before the first release with the SDK there is no tag to pin to (e2e trial).
    assert_eq!(scaffold::sdk_git_ref("0.7.0"), "tag = \"v0.7.0\"");
    assert_eq!(scaffold::sdk_git_ref("0.7.3"), "tag = \"v0.7.3\"");
    assert_eq!(scaffold::sdk_git_ref("0.6.0"), "branch = \"main\"");
    assert!(std::fs::read_to_string(root.join("tools/src/lib.rs")).unwrap().contains("struct MyTools;"));
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn the_examples_build_package_and_pass_cyx_test() {
    let Some(exthost) = toolchain() else { return };
    let examples = [
        ("hello-command", ""),
        ("hello-view", ""),
        ("trim-formatter", ""),
        ("todo-actions", ""),
        ("crate-version", "net"),
        ("branch-status", "process"),
        ("jj-scm", "process"),
        ("gitea-pulls", "auth"),
    ];
    for (name, needs) in examples {
        let dir = repo().join("examples/extensions").join(name);
        build::build_in(&dir, Some(&target_dir())).unwrap_or_else(|e| panic!("{name}: {e}"));
        let report = check(&dir).unwrap_or_else(|e| panic!("{name}: {e}"));
        // The forge examples keep a placeholder client id for the reader to replace; `cyx check` says so (#187).
        let placeholder = |w: &String| w.contains("looks like a placeholder");
        assert!(report.warnings.iter().all(placeholder), "{name}: {:?}", report.warnings);
        assert_eq!(report.warnings.is_empty(), !["gitea-pulls", "gitlab"].contains(&name), "{name}: {:?}", report.warnings);
        let files = ext_pkg::collect_dir(&dir).unwrap();
        let (bytes, _) = ext_pkg::write(files).unwrap();
        let pkg = ext_pkg::read(&bytes).unwrap();
        // The component imports only what it uses: the free interfaces and the one its permission grants.
        let interfaces = pkg.component.unwrap().interfaces;
        let with_permission: Vec<&str> =
            interfaces.iter().map(String::as_str).filter(|i| ext_api::interface(i).is_some_and(|i| i.need != ext_api::Need::Nothing)).collect();
        assert_eq!(with_permission.join(","), needs, "{name}: {interfaces:?}");
        let manifest = build::manifest(&dir).unwrap();
        let fixtures = crate::fixtures::Fixtures::load(&dir, None).unwrap();
        let run = harness::run(&dir, &manifest, &exthost, &fixtures).unwrap();
        assert!(run.failures.is_empty(), "{name}: {:#?}", run.lines);
        assert!(run.lines.iter().any(|l| l.contains("activates")), "{:?}", run.lines);
        for v in &manifest.contributions.views {
            let draws = format!("view {} draws", v.id.rsplit('.').next().unwrap_or(&v.id));
            assert!(run.lines.iter().any(|l| l.contains(&draws)), "{name}: {:#?}", run.lines);
        }
        let _ = std::fs::remove_file(dir.join(ext_pkg::COMPONENT));
    }
}

#[test]
fn cyx_test_fails_on_a_denied_call_and_prints_the_fix() {
    let Some(exthost) = toolchain() else { return };
    let root = tmp("denied");
    let dir = root.join("fetcher");
    scaffold::create_with(&dir, Kind::Command, "fetcher", "you", Some(&repo().to_string_lossy())).unwrap();
    let manifest_path = dir.join(ext_manifest::FILE_NAME);
    let manifest = std::fs::read_to_string(&manifest_path).unwrap();
    let more = "\n[[commands]]\nid    = \"boom\"\ntitle = \"Boom\"\n\n[permissions]\nnet = [\"api.example.com\"]\n";
    std::fs::write(&manifest_path, format!("{manifest}{more}")).unwrap();
    let code = r#"use codeyard_extension::prelude::*;

struct Fetcher;

impl Extension for Fetcher {
    fn activate(ctx: Context) -> Result<()> {
        ctx.command("hello", async |ctx, _args| {
            // The extension catches the refusal; cyx test fails anyway.
            let r = ctx.fetch(Request::get("https://other.example.org/x")).await;
            Ok(r.err().map(|e| e.to_string()))
        });
        ctx.command("boom", async |_ctx, _args| -> Result<()> { panic!("boom") });
        Ok(())
    }
}

codeyard_extension::export!(Fetcher);
"#;
    std::fs::write(dir.join("src/lib.rs"), code).unwrap();
    // The example's lock file, so the build needs no registry lookups.
    std::fs::copy(repo().join("examples/extensions/hello-command/Cargo.lock"), dir.join("Cargo.lock")).unwrap();
    build::build_in(&dir, Some(&target_dir())).unwrap();
    let manifest = build::manifest(&dir).unwrap();
    let run = harness::run(&dir, &manifest, &exthost, &Default::default()).unwrap();
    let all = run.failures.join("\n");
    assert!(all.contains(r#"= fix: add to [permissions]:  net = ["other.example.org"]"#), "{all}");
    assert!(all.contains("net/fetch"), "{all}");
    // A panic is a trap, reported with where it happened.
    assert!(all.contains("the extension trapped"), "{all}");
    std::fs::remove_dir_all(&root).unwrap();
}

/// API 1.1 in the real exthost: `cyx test` gives each watch a file of its own (a trap in a handler fails the run), and
/// treats the host in a `net-from-setting` setting's default as the one the user allowed.
#[test]
fn cyx_test_exercises_watches_and_net_from_setting() {
    let Some(exthost) = toolchain() else { return };
    let root = tmp("watches");
    let dir = root.join("todos");
    scaffold::create_with(&dir, Kind::Command, "todos", "you", Some(&repo().to_string_lossy())).unwrap();
    let manifest_path = dir.join(ext_manifest::FILE_NAME);
    let manifest = std::fs::read_to_string(&manifest_path).unwrap();
    let more = "\n[permissions]\nread = [\"src/**/*.rs\"]\nnet-from-setting = \"server\"\n\n[settings.server]\ntype = \"string\"\ndefault = \"ghe.example.com\"\ndescription = \"Your GitHub Enterprise host.\"\n";
    std::fs::write(&manifest_path, format!("{manifest}{more}")).unwrap();
    let code = r#"use codeyard_extension::prelude::*;

struct Todos;

impl Extension for Todos {
    fn activate(ctx: Context) -> Result<()> {
        ctx.on_documents(true, async |ctx, e| {
            ctx.log(LogLevel::Info, format!("{:?} {} {}", e.change, e.uri, e.text.unwrap_or_default().trim()));
        })?;
        ctx.on_files("src/**/*.rs", async |_ctx, events| {
            if events.iter().any(|e| e.change == FileChange::Deleted) {
                panic!("deleted");
            }
        })?;
        ctx.command("hello", async |ctx, _args| {
            let r = ctx.fetch(Request::get("https://ghe.example.com/api/v3/user")).await?;
            Ok(r.status)
        });
        Ok(())
    }
}

codeyard_extension::export!(Todos);
"#;
    std::fs::write(dir.join("src/lib.rs"), code).unwrap();
    std::fs::copy(repo().join("examples/extensions/hello-command/Cargo.lock"), dir.join("Cargo.lock")).unwrap();
    build::build_in(&dir, Some(&target_dir())).unwrap();
    let manifest = build::manifest(&dir).unwrap();
    let fixtures = crate::fixtures::Fixtures::parse(r#"{ "http": [{ "url": "https://ghe.example.com/api/v3/user", "body": {} }] }"#).unwrap();
    let run = harness::run(&dir, &manifest, &exthost, &fixtures).unwrap();
    let lines = run.lines.join("\n");
    assert!(lines.contains("command hello answers"), "{lines}");
    assert!(lines.contains("Opened file://") && lines.contains("src/example.rs TODO: example"), "{lines}");
    assert!(lines.contains("document watch handles src/example.rs opened, changed and saved"), "{lines}");
    let all = run.failures.join("\n");
    assert!(all.contains("the extension trapped"), "{all}");
    assert_eq!(run.failures.len(), 1, "{all}");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn cyx_test_fails_on_a_view_codeyard_would_not_draw() {
    // #182: the `{ "type": … }` shorthand the view template once wrote is refused by Codeyard, so `cyx test` refuses it.
    let Some(exthost) = toolchain() else { return };
    let root = tmp("shorthand");
    let dir = root.join("counter");
    scaffold::create_in(&dir, Kind::View, "counter", "you", Some(&repo().to_string_lossy()), scaffold::Lang::Rust).unwrap();
    let code = r#"use codeyard_extension::prelude::*;

struct Counter;

impl Extension for Counter {
    fn activate(ctx: Context) -> Result<()> {
        ctx.view("main", async |_ctx| Ok(json!({ "type": "column", "children": [{ "type": "text", "text": "2" }] })));
        ctx.command("reset", async |_ctx, _args| Ok(()));
        Ok(())
    }
}

codeyard_extension::export!(Counter);
"#;
    std::fs::write(dir.join("src/lib.rs"), code).unwrap();
    std::fs::copy(repo().join("examples/extensions/hello-view/Cargo.lock"), dir.join("Cargo.lock")).unwrap();
    build::build_in(&dir, Some(&target_dir())).unwrap();
    let manifest = build::manifest(&dir).unwrap();
    let run = harness::run(&dir, &manifest, &exthost, &crate::fixtures::Fixtures::load(&dir, None).unwrap()).unwrap();
    let all = run.failures.join("\n");
    assert!(all.contains("view main is refused by Codeyard: the view is malformed: unknown field"), "{all}");
    assert!(!run.lines.iter().any(|l| l.contains("view main draws")), "{:?}", run.lines);
    std::fs::remove_dir_all(&root).unwrap();
}

/// One run reports every problem, with Codeyard's rules: a trap restarts the exthost and the rest still runs; an error
/// is an answer only after something `cyx test` cannot give offline; files come from the fixtures; the 8 MiB answer
/// limit, the status bar rule and the providers are checked too. Before, the first trap ended the run, a command that
/// returned any error (or had no handler) passed, and a 20 MiB answer timed out.
#[test]
fn cyx_test_reports_every_problem_in_one_run() {
    let Some(exthost) = toolchain() else { return };
    let root = tmp("all");
    let dir = root.join("all-problems");
    scaffold::create_with(&dir, Kind::Command, "all-problems", "you", Some(&repo().to_string_lossy())).unwrap();
    let manifest_path = dir.join(ext_manifest::FILE_NAME);
    let manifest = std::fs::read_to_string(&manifest_path).unwrap();
    let mut more = String::new();
    for id in ["boom", "missing", "bug", "offline", "config", "huge", "status"] {
        more.push_str(&format!("\n[[commands]]\nid    = \"{id}\"\ntitle = \"{id}\"\n"));
    }
    more.push_str(
        "\n[[status-bar]]\nid = \"state\"\n\n[[hover]]\nlanguages = [\"rust\"]\n\n[permissions]\nread = [\".git/config\"]\nnet  = [\"api.example.com\"]\n",
    );
    std::fs::write(&manifest_path, format!("{manifest}{more}")).unwrap();
    let code = r#"use codeyard_extension::prelude::*;

struct AllProblems;

fn explode(n: usize) -> usize {
    let v: Vec<usize> = Vec::new();
    v[n]
}

impl Extension for AllProblems {
    fn activate(ctx: Context) -> Result<()> {
        ctx.command("hello", async |_ctx, _args| Ok("hi"));
        ctx.command("boom", async |_ctx, _args| Ok(explode(3)));
        ctx.command("bug", async |_ctx, _args| -> Result<()> { Err(Error::other("a bug")) });
        ctx.command("offline", async |ctx, _args| Ok(ctx.fetch(Request::get("https://api.example.com/x")).await?.status));
        ctx.command("config", async |ctx, _args| {
            let folder = ctx.folders().first().cloned().unwrap_or_default();
            ctx.read_text(&format!("{folder}/.git/config")).await
        });
        ctx.command("huge", async |_ctx, _args| Ok("x".repeat(20 << 20)));
        ctx.command("status", async |ctx, _args| ctx.set_status_item(StatusItem { id: "other".into(), text: "Hi".into(), ..Default::default() }));
        ctx.hover(async |_ctx, doc, _position| Ok(Some(Hover::new(format!("{} lines", doc.text.lines().count())))));
        Ok(())
    }
}

codeyard_extension::export!(AllProblems);
"#;
    std::fs::write(dir.join("src/lib.rs"), code).unwrap();
    std::fs::create_dir_all(dir.join("test")).unwrap();
    std::fs::write(dir.join("test/fixtures.json"), r#"{ "files": { ".git/config": "[core]\n\tbare = false\n" } }"#).unwrap();
    std::fs::copy(repo().join("examples/extensions/hello-command/Cargo.lock"), dir.join("Cargo.lock")).unwrap();
    build::build_in(&dir, Some(&target_dir())).unwrap();
    let manifest = build::manifest(&dir).unwrap();
    let fixtures = crate::fixtures::Fixtures::load(&dir, None).unwrap();
    let run = harness::run(&dir, &manifest, &exthost, &fixtures).unwrap();
    let lines = run.lines.join("\n");
    let failures = run.failures.join("\n");
    assert!(lines.contains("✓ command hello answers"), "{lines}");
    // The panic: the extension's own frame, demangled, with the runtime's panic handling left out.
    assert!(failures.contains("the extension trapped"), "{failures}");
    assert!(failures.contains("at all_problems::explode"), "{failures}");
    assert!(!failures.contains("_ZN") && !failures.contains("at std::") && !failures.contains("at core::"), "{failures}");
    assert!(lines.contains("(the extension host was started again)"), "{lines}");
    assert!(failures.contains("command missing failed: not found: no command 'missing' is registered"), "{failures}");
    assert!(failures.contains("command bug failed: a bug"), "{failures}");
    assert!(lines.contains("✓ command offline answers (with an error, after no fixture for GET https://api.example.com/x"), "{lines}");
    assert!(lines.contains("✓ command config answers"), "{lines}");
    assert!(failures.contains("command huge failed: invalid: the result is 20 MiB; at most 8 MiB"), "{failures}");
    assert!(failures.contains("status item \"other\" is not declared"), "{failures}");
    assert!(lines.contains("✓ hover answers"), "{lines}");
    std::fs::remove_dir_all(&root).unwrap();
}

/// Diagnostics are judged as Codeyard judges them: a `[[diagnostics]]` declaration listing the file's language, a file in
/// the workspace, messages without hidden characters. Codeyard shows nothing otherwise, so `cyx test` fails.
#[test]
fn cyx_test_fails_diagnostics_codeyard_would_not_show() {
    let Some(exthost) = toolchain() else { return };
    let root = tmp("marks");
    let dir = root.join("marker");
    scaffold::create_with(&dir, Kind::Command, "marker", "you", Some(&repo().to_string_lossy())).unwrap();
    let manifest_path = dir.join(ext_manifest::FILE_NAME);
    let base = std::fs::read_to_string(&manifest_path).unwrap();
    let mut more = String::new();
    for id in ["fine", "other-language", "hidden", "outside", "read-outside", "fetch-ip"] {
        more.push_str(&format!("\n[[commands]]\nid    = \"{id}\"\ntitle = \"{id}\"\n"));
    }
    let declared = format!("{base}{more}\n[permissions]\nread = [\"src/lib.rs\"]\nnet  = [\"api.example.com\"]\n");
    let code = r#"use codeyard_extension::prelude::*;

struct Marker;

fn mark(ctx: &Context, file: &str, message: &str) {
    let folder = ctx.folders().first().cloned().unwrap_or_default();
    let d = Diagnostic::new(Range::line(0, 0, 1), DiagnosticSeverity::Warning, message);
    ctx.set_diagnostics(&format!("{folder}/{file}"), &[d]);
}

impl Extension for Marker {
    fn activate(ctx: Context) -> Result<()> {
        ctx.command("hello", async |_ctx, _args| Ok(()));
        ctx.command("fine", async |ctx, _args| Ok(mark(&ctx, "src/lib.rs", "unused import")));
        ctx.command("other-language", async |ctx, _args| Ok(mark(&ctx, "Cargo.toml", "odd")));
        ctx.command("hidden", async |ctx, _args| Ok(mark(&ctx, "src/lib.rs", "safe\u{202e}evil")));
        ctx.command("outside", async |ctx, _args| {
            ctx.set_diagnostics("file:///etc/hosts.rs", &[Diagnostic::new(Range::line(0, 0, 1), DiagnosticSeverity::Hint, "x")]);
            Ok(())
        });
        // What no manifest entry can allow: a file outside the workspace, a request to an IP address.
        ctx.command("read-outside", async |ctx, _args| Ok(ctx.read_text("file:///etc/passwd").await.is_err()));
        ctx.command("fetch-ip", async |ctx, _args| Ok(ctx.fetch(Request::get("https://127.0.0.1/")).await.is_err()));
        Ok(())
    }
}

codeyard_extension::export!(Marker);
"#;
    std::fs::write(dir.join("src/lib.rs"), code).unwrap();
    std::fs::copy(repo().join("examples/extensions/hello-command/Cargo.lock"), dir.join("Cargo.lock")).unwrap();
    // The components are the same whatever `[[diagnostics]]` says: build once, test with each manifest.
    std::fs::write(&manifest_path, &declared).unwrap();
    build::build_in(&dir, Some(&target_dir())).unwrap();
    let run_with = |text: &str| {
        std::fs::write(&manifest_path, text).unwrap();
        let manifest = build::manifest(&dir).unwrap();
        harness::run(&dir, &manifest, &exthost, &Default::default()).unwrap().failures.join("\n")
    };
    let none = run_with(&declared);
    assert!(none.contains("set-diagnostics on") && none.contains("the manifest declares no [[diagnostics]]"), "{none}");
    assert!(none.contains(r#"= fix: add [[diagnostics]] with languages = ["rust"]"#), "{none}");
    let listed = run_with(&format!("{declared}\n[[diagnostics]]\nid        = \"lint\"\nlanguages = [\"rust\"]\n"));
    assert!(listed.contains("Cargo.toml is refused by Codeyard: [[diagnostics]] does not list its language, toml"), "{listed}");
    assert!(listed.contains(r#"= fix: add "toml" to the languages of [[diagnostics]]"#), "{listed}");
    assert!(listed.contains("Codeyard drops 1 of 1 diagnostics"), "{listed}");
    assert!(listed.contains("file:///etc/hosts.rs is refused by Codeyard: it is neither in the workspace nor open in an editor"), "{listed}");
    assert_eq!(listed.matches("src/lib.rs is refused").count(), 0, "{listed}");
    // No line to add for what no entry allows: the fix says so, as the SDKs do.
    assert!(listed.contains("workspace/read-file is refused, and no permission can allow it"), "{listed}");
    assert!(listed.contains("= fix: none: this cannot be granted, the file is outside the workspace"), "{listed}");
    assert!(listed.contains("net/fetch is refused, and no permission can allow it"), "{listed}");
    assert!(listed.contains("= fix: none: this cannot be granted, it is an IP address, and net lists host names only"), "{listed}");
    assert!(!listed.contains("unused import"), "the fine command is not a failure: {listed}");
    std::fs::remove_dir_all(&root).unwrap();
}

/// An `async activate` that rejects after an `await` fails the activation; a command that throws is a failure, not an
/// answer; the JavaScript engine's own file is named without the folder it was built in.
#[test]
fn cyx_test_fails_an_async_activate_and_a_throwing_command() {
    let Some(exthost) = toolchain() else { return };
    if !node() {
        return;
    }
    let root = tmp("ts-async");
    let dir = root.join("late");
    scaffold::create_in(&dir, Kind::Command, "late", "you", None, scaffold::Lang::TypeScript).unwrap();
    let manifest_path = dir.join(ext_manifest::FILE_NAME);
    let manifest = std::fs::read_to_string(&manifest_path).unwrap();
    std::fs::write(&manifest_path, format!("{manifest}\n[permissions]\nnet = [\"api.example.com\"]\n")).unwrap();
    let code = r#"import type { Context } from "@codeyard/extension";

export async function activate(ctx: Context) {
  ctx.command("hello", () => (undefined as unknown as { name: string }).name);
  const r = await fetch("https://api.example.com/config");
  if (!r.ok) throw new Error(`no config (${r.status})`);
}
"#;
    std::fs::write(dir.join("src/extension.ts"), code).unwrap();
    std::fs::write(dir.join("test/fixtures.json"), r#"{ "http": [{ "url": "https://api.example.com/config", "status": 503 }] }"#).unwrap();
    build::build(&dir).unwrap();
    let manifest = build::manifest(&dir).unwrap();
    let fixtures = crate::fixtures::Fixtures::load(&dir, None).unwrap();
    let run = harness::run(&dir, &manifest, &exthost, &fixtures).unwrap();
    let failures = run.failures.join("\n");
    let lines = run.lines.join("\n");
    assert!(failures.contains("activate failed: no config (503)"), "{lines}");
    assert!(failures.contains("command hello failed"), "{lines}");
    assert!(!lines.contains("/initializer.js"), "{lines}");
    std::fs::remove_dir_all(&root).unwrap();
}

/// `cyx dev` with a stand-in for Codeyard that exits once `extension.wasm` is replaced: saving a source file rebuilds it.
#[cfg(unix)]
#[test]
fn cyx_dev_rebuilds_on_save() {
    use std::os::unix::fs::PermissionsExt;
    if toolchain().is_none() {
        return;
    }
    let root = tmp("dev");
    let dir = root.join("live");
    scaffold::create_with(&dir, Kind::Command, "live", "you", Some(&repo().to_string_lossy())).unwrap();
    std::fs::copy(repo().join("examples/extensions/hello-command/Cargo.lock"), dir.join("Cargo.lock")).unwrap();
    let bin = root.join("codeyard");
    let script = "#!/bin/sh\nw=\"$2/extension.wasm\"\nbefore=$(cksum < \"$w\")\ni=0\nwhile [ $i -lt 1200 ]; do\n  sleep 0.1\n  [ \"$(cksum < \"$w\")\" != \"$before\" ] && exit 0\n  i=$((i+1))\ndone\nexit 3\n";
    std::fs::write(&bin, script).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let src = dir.join("src/lib.rs");
    let edit = {
        let (wasm, src) = (dir.join(ext_pkg::COMPONENT), src.clone());
        std::thread::spawn(move || {
            while !wasm.exists() {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            // The first build is done: cyx noted the sources before it, so this save is seen however long the check
            // and the stand-in's start take.
            let code = std::fs::read_to_string(&src).unwrap().replace("Hello, {name}!", "Hi, {name}!");
            std::fs::write(&src, code).unwrap();
        })
    };
    dev::run_with(&dir, &bin, &[], Some(&target_dir())).unwrap();
    edit.join().unwrap();
    // Nothing half-written is left behind.
    assert!(!dir.join(build::PARTIAL).exists());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn package_builds_the_component_and_reports_a_missing_target() {
    // Without code there is nothing to build.
    let root = tmp("nocode");
    scaffold::create_with(&root.join("night"), Kind::Theme, "night", "you", None).unwrap();
    assert_eq!(build::build(&root.join("night")).unwrap(), None);
    std::fs::remove_dir_all(&root).unwrap();
    if !build::target_installed() {
        let root = tmp("notarget");
        let dir = root.join("x");
        scaffold::create_with(&dir, Kind::Command, "x", "you", None).unwrap();
        let e = build::build(&dir).unwrap_err();
        assert!(e.contains("rustup target add wasm32-wasip2"), "{e}");
        std::fs::remove_dir_all(&root).unwrap();
    }
}

/// Node.js for the TypeScript build, or why these tests cannot run here.
fn node() -> bool {
    let ok = std::process::Command::new("node")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().trim_start_matches('v').split('.').next()?.parse::<u32>().ok())
        .is_some_and(|major| major >= 22);
    match ok {
        false if std::env::var_os("CODEYARD_REQUIRE_WASM").is_some() => panic!("CODEYARD_REQUIRE_WASM is set, but Node.js 22 or newer is not on PATH"),
        false => {
            eprintln!("skipped: Node.js 22 or newer is not on PATH");
            false
        }
        true => true,
    }
}

#[test]
fn the_typescript_examples_build_package_and_pass_cyx_test() {
    let Some(exthost) = toolchain() else { return };
    if !node() {
        return;
    }
    for (name, needs) in [
        ("hello-command-ts", ""),
        ("hello-view-ts", ""),
        ("octokit-repos-ts", "auth"),
        ("crate-version-ts", "net"),
        // ComponentizeJS imports every interface the narrowed world has: `read` brings `watch` (API 1.1) too.
        ("vercel-status", "net,secrets,watch,workspace"),
        ("gitlab", "auth"),
    ] {
        let dir = repo().join("examples/extensions").join(name);
        build::build(&dir).unwrap_or_else(|e| panic!("{name}: {e}"));
        let report = check(&dir).unwrap_or_else(|e| panic!("{name}: {e}"));
        // The forge examples keep a placeholder client id for the reader to replace; `cyx check` says so (#187).
        let placeholder = |w: &String| w.contains("looks like a placeholder");
        assert!(report.warnings.iter().all(placeholder), "{name}: {:?}", report.warnings);
        assert_eq!(report.warnings.is_empty(), !["gitea-pulls", "gitlab"].contains(&name), "{name}: {:?}", report.warnings);
        let (bytes, _) = ext_pkg::write(ext_pkg::collect_dir(&dir).unwrap()).unwrap();
        let pkg = ext_pkg::read(&bytes).unwrap();
        // The component imports the free interfaces and what the manifest grants, nothing more.
        let interfaces = pkg.component.unwrap().interfaces;
        let with_permission: Vec<&str> =
            interfaces.iter().map(String::as_str).filter(|i| ext_api::interface(i).is_some_and(|i| i.need != ext_api::Need::Nothing)).collect();
        assert_eq!(with_permission.join(","), needs, "{name}: {interfaces:?}");
        assert!(pkg.files.contains_key(typescript::SOURCE_MAP), "{name}: the source map is packaged");
        let manifest = build::manifest(&dir).unwrap();
        let fixtures = crate::fixtures::Fixtures::load(&dir, None).unwrap();
        let run = harness::run(&dir, &manifest, &exthost, &fixtures).unwrap();
        assert!(run.failures.is_empty(), "{name}: {:#?}", run.lines);
        assert!(run.lines.iter().any(|l| l.contains("activates")), "{:?}", run.lines);
        if name == "gitlab" {
            // #185: test/fixtures.json answers the provider's signed request, so its list runs in the real exthost.
            assert!(run.lines.iter().any(|l| l.contains("pull requests gitlab lists (1 open)")), "{:#?}", run.lines);
        }
        for v in &manifest.contributions.views {
            let draws = format!("view {} draws", v.id.rsplit('.').next().unwrap_or(&v.id));
            assert!(run.lines.iter().any(|l| l.contains(&draws)), "{name}: {:#?}", run.lines);
        }
        let _ = std::fs::remove_file(dir.join(ext_pkg::COMPONENT));
        let _ = std::fs::remove_file(dir.join(typescript::SOURCE_MAP));
    }
}

#[test]
fn cyx_test_maps_typescript_stacks_and_fails_on_refused_calls() {
    let Some(exthost) = toolchain() else { return };
    if !node() {
        return;
    }
    let root = tmp("ts-denied");
    let dir = root.join("fetcher");
    scaffold::create_in(&dir, Kind::Command, "fetcher", "you", None, scaffold::Lang::TypeScript).unwrap();
    let manifest_path = dir.join(ext_manifest::FILE_NAME);
    let manifest = std::fs::read_to_string(&manifest_path).unwrap();
    let more = "\n[[commands]]\nid    = \"boom\"\ntitle = \"Boom\"\n\n[[commands]]\nid    = \"files\"\ntitle = \"Files\"\n\n[permissions]\nnet = [\"api.example.com\"]\n";
    std::fs::write(&manifest_path, format!("{manifest}{more}")).unwrap();
    let code = r#"import type { Context } from "@codeyard/extension";

function explode(): never {
  throw new Error("it broke");
}

export function activate(ctx: Context) {
  ctx.command("hello", async () => {
    // The extension catches the refusal; cyx test fails anyway.
    return fetch("https://other.example.org/x").then(() => "sent", (e) => String(e));
  });
  ctx.command("boom", async () => explode());
  // `read` is not declared, so the component does not import `workspace` at all.
  ctx.command("files", (ctx) => ctx.readText("file:///work/a.json").catch(() => "refused"));
}
"#;
    std::fs::write(dir.join("src/extension.ts"), code).unwrap();
    build::build(&dir).unwrap();
    let manifest = build::manifest(&dir).unwrap();
    let run = harness::run(&dir, &manifest, &exthost, &Default::default()).unwrap();
    let all = run.failures.join("\n");
    assert!(all.contains(r#"= fix: add to [permissions]:  net = ["other.example.org"]"#), "{all}");
    assert!(all.contains(r#"= fix: add to [permissions]:  read = ["**/a.json"]"#), "{all}");
    // The thrown error names the .ts file and line, not the bundle.
    let lines = run.lines.join("\n");
    assert!(lines.contains("it broke\n    at explode (src/extension.ts:4:9)"), "{lines}");
    assert!(!lines.contains("extension.js:"), "{lines}");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn typescript_builds_refuse_node_built_ins() {
    if toolchain().is_none() || !node() {
        return;
    }
    let root = tmp("ts-fs");
    let dir = root.join("reader");
    scaffold::create_in(&dir, Kind::Command, "reader", "you", None, scaffold::Lang::TypeScript).unwrap();
    let code = "import { readFileSync } from \"node:fs\";\nimport type { Context } from \"@codeyard/extension\";\n\nexport function activate(ctx: Context) {\n  ctx.command(\"hello\", () => readFileSync(\"/etc/hosts\", \"utf8\"));\n}\n";
    std::fs::write(dir.join("src/extension.ts"), code).unwrap();
    let e = build::build(&dir).unwrap_err();
    assert!(e.contains(r#"error: "node:fs" is a Node.js built-in module, which Codeyard extensions do not have"#), "{e}");
    assert!(e.contains("--> src/extension.ts:1:30") && e.contains("= fix: use ctx.readFile"), "{e}");
    std::fs::remove_dir_all(&root).unwrap();
}

/// #187: `cyx new pull-requests`, in Rust and TypeScript: it builds, `cyx check` flags the placeholder client id, its
/// test folder stays out of the package, and `cyx test` lists through the account with `test/fixtures.json` (#185).
#[test]
fn the_pull_request_templates_build_and_list_with_their_fixtures() {
    let Some(exthost) = toolchain() else { return };
    let root = tmp("pulls");
    let sdk = repo().to_string_lossy().into_owned();
    for (name, lang) in [("forge-pulls", scaffold::Lang::Rust), ("forge-pulls-ts", scaffold::Lang::TypeScript)] {
        if lang == scaffold::Lang::TypeScript && !node() {
            continue;
        }
        let dir = root.join(name);
        scaffold::create_in(&dir, Kind::PullRequests, name, "you", Some(&sdk), lang).unwrap();
        assert!(std::fs::read_to_string(dir.join("README.md")).unwrap().contains(&format!("codeyard://ext/you.{name}/callback")));
        match lang {
            scaffold::Lang::Rust => build::build_in(&dir, Some(&target_dir())).map(|_| ()),
            scaffold::Lang::TypeScript => build::build(&dir).map(|_| ()),
        }
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        let report = check(&dir).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.warnings.len(), 1, "{name}: {:?}", report.warnings);
        assert!(report.warnings[0].contains("[[oauth]] forge looks like a placeholder"), "{:?}", report.warnings);
        let (bytes, _) = ext_pkg::write(ext_pkg::collect_dir(&dir).unwrap()).unwrap();
        let pkg = ext_pkg::read(&bytes).unwrap();
        assert!(!pkg.files.keys().any(|f| f.starts_with("test/")), "{name}: {:?}", pkg.files.keys().collect::<Vec<_>>());
        let manifest = build::manifest(&dir).unwrap();
        let fixtures = crate::fixtures::Fixtures::load(&dir, None).unwrap();
        let run = harness::run(&dir, &manifest, &exthost, &fixtures).unwrap();
        assert!(run.failures.is_empty(), "{name}: {:#?}", run.lines);
        assert!(run.lines.iter().any(|l| l.contains("pull requests pulls lists (1 open)")), "{name}: {:#?}", run.lines);
    }
    std::fs::remove_dir_all(&root).unwrap();
}
