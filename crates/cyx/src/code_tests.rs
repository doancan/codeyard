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
    assert!(cargo.contains(&format!("codeyard-extension = {{ git = \"{}\" }}", scaffold::SDK_REPOSITORY)), "{cargo}");
    assert!(std::fs::read_to_string(root.join("tools/src/lib.rs")).unwrap().contains("struct MyTools;"));
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn the_examples_build_package_and_pass_cyx_test() {
    let Some(exthost) = toolchain() else { return };
    let examples =
        [("hello-command", ""), ("hello-view", ""), ("trim-formatter", ""), ("todo-actions", ""), ("crate-version", "net"), ("branch-status", "process")];
    for (name, needs) in examples {
        let dir = repo().join("examples/extensions").join(name);
        build::build_in(&dir, Some(&target_dir())).unwrap_or_else(|e| panic!("{name}: {e}"));
        let report = check(&dir).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(report.warnings.is_empty(), "{name}: {:?}", report.warnings);
        let files = ext_pkg::collect_dir(&dir).unwrap();
        let (bytes, _) = ext_pkg::write(files).unwrap();
        let pkg = ext_pkg::read(&bytes).unwrap();
        // The component imports only what it uses: the free interfaces and the one its permission grants.
        let interfaces = pkg.component.unwrap().interfaces;
        let with_permission: Vec<&str> =
            interfaces.iter().map(String::as_str).filter(|i| ext_api::interface(i).is_some_and(|i| i.need != ext_api::Need::Nothing)).collect();
        assert_eq!(with_permission.join(","), needs, "{name}: {interfaces:?}");
        let manifest = build::manifest(&dir).unwrap();
        let run = harness::run(&dir, &manifest, &exthost).unwrap();
        assert!(run.failures.is_empty(), "{name}: {:#?}", run.lines);
        assert!(run.lines.iter().any(|l| l.contains("activates")), "{:?}", run.lines);
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
    let run = harness::run(&dir, &manifest, &exthost).unwrap();
    let all = run.failures.join("\n");
    assert!(all.contains(r#"= fix: add to [permissions]:  net = ["other.example.org"]"#), "{all}");
    assert!(all.contains("net/fetch"), "{all}");
    // A panic is a trap, reported with where it happened.
    assert!(all.contains("the extension trapped"), "{all}");
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
            // After cyx has started the stand-in and looked at the sources once.
            std::thread::sleep(std::time::Duration::from_millis(800));
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
    for (name, needs) in [("hello-command-ts", ""), ("hello-view-ts", ""), ("octokit-repos-ts", "auth"), ("crate-version-ts", "net")] {
        let dir = repo().join("examples/extensions").join(name);
        build::build(&dir).unwrap_or_else(|e| panic!("{name}: {e}"));
        let report = check(&dir).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(report.warnings.is_empty(), "{name}: {:?}", report.warnings);
        let (bytes, _) = ext_pkg::write(ext_pkg::collect_dir(&dir).unwrap()).unwrap();
        let pkg = ext_pkg::read(&bytes).unwrap();
        // The component imports the free interfaces and what the manifest grants, nothing more.
        let interfaces = pkg.component.unwrap().interfaces;
        let with_permission: Vec<&str> =
            interfaces.iter().map(String::as_str).filter(|i| ext_api::interface(i).is_some_and(|i| i.need != ext_api::Need::Nothing)).collect();
        assert_eq!(with_permission.join(","), needs, "{name}: {interfaces:?}");
        assert!(pkg.files.contains_key(typescript::SOURCE_MAP), "{name}: the source map is packaged");
        let manifest = build::manifest(&dir).unwrap();
        let run = harness::run(&dir, &manifest, &exthost).unwrap();
        assert!(run.failures.is_empty(), "{name}: {:#?}", run.lines);
        assert!(run.lines.iter().any(|l| l.contains("activates")), "{:?}", run.lines);
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
    let run = harness::run(&dir, &manifest, &exthost).unwrap();
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
