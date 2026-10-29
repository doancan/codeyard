//! Code extensions: the Rust templates, the examples built from them, `cyx package` and `cyx test`. The tests that build
//! a component need the wasm32-wasip2 target and a built codeyard-exthost; without them they are skipped with a note,
//! unless CODEYARD_REQUIRE_WASM is set (CI's Linux job), where a skip is a failure.

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
                if name != "target" {
                    stack.push(p);
                }
            } else if name != "Cargo.lock" && name != ext_pkg::COMPONENT {
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
    // examples/extensions/hello-* are `cyx new command|view hello-… --publisher codeyard --sdk ../../..`.
    let root = tmp("examples");
    for (kind, name) in [(Kind::Command, "hello-command"), (Kind::View, "hello-view")] {
        scaffold::create_with(&root.join(name), kind, name, "codeyard", Some("../../..")).unwrap();
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
    for name in ["hello-command", "hello-view"] {
        let dir = repo().join("examples/extensions").join(name);
        build::build_in(&dir, Some(&target_dir())).unwrap_or_else(|e| panic!("{name}: {e}"));
        let report = check(&dir).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(report.warnings.is_empty(), "{name}: {:?}", report.warnings);
        let files = ext_pkg::collect_dir(&dir).unwrap();
        let (bytes, _) = ext_pkg::write(files).unwrap();
        let pkg = ext_pkg::read(&bytes).unwrap();
        // The component imports only what it uses: no permission is needed.
        assert!(pkg.component.is_some_and(|c| c.interfaces.iter().all(|i| ext_api::interface(i).is_some_and(|i| i.need == ext_api::Need::Nothing))));
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
