//! Rust values in the debugger: a Rust configuration starts lldb-dap with the toolchain's formatters (and the standard
//! library's source map) in `initCommands`. With the scripted adapter, against this machine's toolchain and against a fake
//! one chosen by the project's `rust-toolchain.toml` (`path = …`, so `rustc` is never looked up on a changed `PATH`);
//! then with a real lldb-dap.
//!
//! The fake toolchain is used over the test SSH server: `cargo test` gives its children `RUSTUP_TOOLCHAIN`, which wins over
//! `rust-toolchain.toml`, and the server starts the shell with a clean environment.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{Entity, TestAppContext, VisualTestContext};
use serde_json::{Value, json};

use crate::qa_integration_tests::wait_secs;
use crate::qa_workbench_tests::{open, project, setup_persistent};
use crate::workbench::Workbench;

const HASH: &str = "abcdef1234567890";

/// What the fake toolchain has.
#[derive(Clone, Copy, PartialEq)]
enum Toolchain {
    /// Formatters, the old `lldb_commands` and `rust-src`.
    Full,
    /// A toolchain without the formatter files.
    NoFormatters,
    /// `rustc` cannot run (the toolchain path does not exist).
    NoRustc,
}

/// rustup is what makes `rust-toolchain.toml` pick the fake toolchain; without it the test cannot choose `rustc`.
fn have_tools() -> bool {
    let ok = |c: &str, a: &str| std::process::Command::new(c).arg(a).output().is_ok_and(|o| o.status.success());
    if ok("python3", "-V") && ok("rustup", "--version") {
        return true;
    }
    crate::test_sshd::skip_missing("python3,rustup");
    false
}

/// A Cargo folder with the scripted adapter as an `type` configuration and a fake toolchain under a path with a space
/// and Turkish letters; returns (folder, adapter log, sysroot).
fn rust_project(name: &str, ty: &str, extra: &str, toolchain: Toolchain) -> (PathBuf, PathBuf, String) {
    let dir = project(name);
    let log = dir.join("dap.log");
    let script = crate::dap::tests::write_fake_adapter(&dir);
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n").unwrap();
    std::fs::write(dir.join("main.c"), (1..=12).map(|i| format!("int line{i};\n")).collect::<String>()).unwrap();
    let sysroot = dir.join("araç zinciri");
    std::fs::create_dir_all(sysroot.join("bin")).unwrap();
    let sysroot = sysroot.to_string_lossy().into_owned();
    let rustc = format!(
        "#!/bin/sh\ncase \"$1\" in\n  --print) echo '{sysroot}' ;;\n  -vV) printf 'rustc 1.80.0\\nbinary: rustc\\ncommit-hash: {HASH}\\nhost: x86_64-unknown-linux-gnu\\n' ;;\nesac\n"
    );
    let bin = Path::new(&sysroot).join("bin/rustc");
    std::fs::write(&bin, rustc).unwrap();
    std::process::Command::new("chmod").arg("+x").arg(&bin).status().unwrap();
    if toolchain == Toolchain::Full {
        std::fs::create_dir_all(Path::new(&sysroot).join("lib/rustlib/etc")).unwrap();
        std::fs::create_dir_all(Path::new(&sysroot).join("lib/rustlib/src/rust/library")).unwrap();
        std::fs::write(Path::new(&sysroot).join("lib/rustlib/etc/lldb_lookup.py"), "").unwrap();
        std::fs::write(Path::new(&sysroot).join("lib/rustlib/etc/lldb_commands"), "").unwrap();
    }
    let path = if toolchain == Toolchain::NoRustc { "/nonexistent/toolchain".to_string() } else { sysroot.clone() };
    std::fs::write(dir.join("rust-toolchain.toml"), format!("[toolchain]\npath = \"{path}\"\n")).unwrap();
    std::fs::create_dir_all(dir.join(".codeyard")).unwrap();
    std::fs::write(
        dir.join(".codeyard/launch.jsonc"),
        format!(
            r#"{{ "configurations": [ {{ "name": "Fake", "type": "{ty}", "request": "launch", "program": "/bin/true", "source": "${{workspaceFolder}}/main.c",
                "debugAdapter": {{ "command": "{}", "args": ["{}"] }} {extra} }} ] }}"#,
            script.display(),
            log.display()
        ),
    )
    .unwrap();
    (dir, log, sysroot)
}

/// Starts the launch file's configuration (the folder also offers its automatic Cargo one).
fn start(wb: &Entity<Workbench>, cx: &mut VisualTestContext) {
    start_named(wb, cx, "Fake");
}

fn start_named(wb: &Entity<Workbench>, cx: &mut VisualTestContext, name: &str) {
    wb.update_in(cx, |wb, window, cx| wb.start_named_for_test(name, window, cx));
}

/// The arguments of the adapter's `launch` request, once it came.
fn launch_args(cx: &mut VisualTestContext, log: &Path) -> Value {
    let line = || std::fs::read_to_string(log).unwrap_or_default().lines().find(|l| l.starts_with("launch ")).map(str::to_string);
    wait_secs(cx, 30, "the launch request", |cx| {
        cx.executor().advance_clock(Duration::from_millis(300));
        line().is_some()
    });
    serde_json::from_str(&line().unwrap()["launch ".len()..]).unwrap()
}

fn console(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<String> {
    wb.read_with(cx, |wb, _| wb.debug_snapshot()).map(|s| s.3).unwrap_or_default()
}

fn expected(sysroot: &str) -> Vec<String> {
    vec![
        format!(r#"command script import "{sysroot}/lib/rustlib/etc/lldb_lookup.py""#),
        format!(r#"command source "{sysroot}/lib/rustlib/etc/lldb_commands""#),
        format!(r#"settings set target.source-map /rustc/{HASH} "{sysroot}/lib/rustlib/src/rust""#),
    ]
}

/// The commands this machine's own toolchain gives, before `user`.
fn this_toolchain(dir: &Path, user: &[String]) -> Vec<String> {
    let out = std::process::Command::new("sh").args(["-c", "rustc --print sysroot && rustc -vV"]).current_dir(dir).output().unwrap();
    let tc = crate::debug_config::parse_rustc_info(&String::from_utf8_lossy(&out.stdout)).expect("rustc runs here");
    let has = |p: &str| Path::new(&tc.sysroot).join("lib/rustlib").join(p).exists();
    crate::debug_config::rust_init_commands(&tc, has("etc/lldb_commands"), has("src/rust/library"), user)
}

/// AC1, AC2: the formatter commands (and the source map, with `rust-src`) come first, the configuration's own commands
/// follow unchanged.
#[gpui::test]
#[cfg_attr(windows, ignore = "the fake toolchain is a POSIX shell script")]
fn rust_debugging_starts_with_the_toolchain_formatters_before_the_user_s_commands(cx: &mut TestAppContext) {
    if !have_tools() {
        return;
    }
    let home = project("dbg-rust-full-home");
    let (dir, log, _) = rust_project("dbg-rust-full", "lldb", r#", "initCommands": ["settings set target.x 1", "breakpoint set -n main"]"#, Toolchain::Full);
    std::fs::remove_file(dir.join("rust-toolchain.toml")).unwrap();
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir, cx);
    start(&wb, cx);
    let user = ["settings set target.x 1".to_string(), "breakpoint set -n main".to_string()];
    let want = this_toolchain(&dir, &user);
    assert!(want[0].starts_with("command script import ") && want[0].ends_with("/lib/rustlib/etc/lldb_lookup.py\""), "{want:?}");
    assert_eq!(launch_args(cx, &log)["initCommands"], json!(want));
    assert!(console(&wb, cx).iter().all(|l| !l.contains("Rust formatters")), "nothing to report");
}

/// AC4 and "already imported": another adapter's arguments are exactly as before, and a configuration that loads the
/// formatters itself is left alone.
#[gpui::test]
#[cfg_attr(windows, ignore = "the fake toolchain is a POSIX shell script")]
fn other_adapters_and_own_formatter_imports_are_left_alone(cx: &mut TestAppContext) {
    if !have_tools() {
        return;
    }
    let home = project("dbg-rust-python-home");
    let (dir, log, _) = rust_project("dbg-rust-python", "python", r#", "initCommands": ["x"]"#, Toolchain::Full);
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir, cx);
    start(&wb, cx);
    let args = launch_args(cx, &log);
    assert_eq!(args["initCommands"], json!(["x"]));
    assert_eq!(args["program"], "/bin/true");

    let (dir, log, _) = rust_project("dbg-rust-own", "lldb", r#", "initCommands": ["command script import /opt/rust/lldb_lookup.py"]"#, Toolchain::Full);
    open(&wb, &dir, cx);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Debug: Stop", window, cx));
    start(&wb, cx);
    assert_eq!(launch_args(cx, &log)["initCommands"], json!(["command script import /opt/rust/lldb_lookup.py"]));
}

/// The fake toolchain's project, open over SSH.
fn over_ssh<'a>(
    cx: &'a mut TestAppContext,
    name: &str,
    extra: &str,
    toolchain: Toolchain,
) -> Option<(Entity<Workbench>, &'a mut VisualTestContext, PathBuf, String, crate::test_sshd::Sshd)> {
    let sshd = crate::test_sshd::start(name)?;
    // The server's non-login shell must find rustup's `rustc` (CI runners keep `~/.cargo/bin` out of that PATH).
    if sshd.run("command -v rustup").is_none() {
        crate::test_sshd::skip_missing("rustup on the SSH server's PATH");
        return None;
    }
    let (dir, log, sysroot) = rust_project(name, "lldb", extra, toolchain);
    let (wb, cx) = crate::qa_integration_tests::ssh_workbench(cx, &dir);
    start(&wb, cx);
    Some((wb, cx, log, sysroot, sshd))
}

/// AC5 and AC1 with an awkward path: over SSH `rustc` runs on the server, in the project (its `rust-toolchain.toml`
/// picks the toolchain), and that sysroot is used, quoted for lldb.
#[gpui::test]
fn rust_debugging_over_ssh_uses_the_server_s_toolchain(cx: &mut TestAppContext) {
    if !have_tools() {
        return;
    }
    let Some((_wb, cx, log, sysroot, _sshd)) = over_ssh(cx, "dbg-rust-ssh", r#", "initCommands": ["mine"]"#, Toolchain::Full) else { return };
    let mut want = expected(&sysroot);
    want.push("mine".into());
    assert!(sysroot.contains("araç zinciri"));
    assert_eq!(launch_args(cx, &log)["initCommands"], json!(want));
}

/// AC3: without `rustc` the session starts as before and the console says why, once.
#[gpui::test]
fn rust_debugging_without_rustc_starts_and_says_why(cx: &mut TestAppContext) {
    if !have_tools() {
        return;
    }
    let _en = crate::i18n::LangGuard::set("en");
    let Some((wb, cx, log, _, _sshd)) = over_ssh(cx, "dbg-rust-norustc", r#", "initCommands": ["mine"]"#, Toolchain::NoRustc) else { return };
    assert_eq!(launch_args(cx, &log)["initCommands"], json!(["mine"]), "the configuration's own commands, nothing added");
    let lines: Vec<String> = console(&wb, cx).into_iter().filter(|l| l.contains("Rust formatters")).collect();
    assert_eq!(lines, ["Rust formatters not loaded: rustc not found"]);
}

/// A toolchain without the formatter files: same, with its own reason, in Turkish too.
#[gpui::test]
fn rust_debugging_with_a_toolchain_without_formatters_says_so_in_turkish(cx: &mut TestAppContext) {
    if !have_tools() {
        return;
    }
    let _tr = crate::i18n::LangGuard::set("tr");
    let Some((wb, cx, log, _, _sshd)) = over_ssh(cx, "dbg-rust-nofiles", "", Toolchain::NoFormatters) else { return };
    assert!(launch_args(cx, &log).get("initCommands").is_none());
    let lines: Vec<String> = console(&wb, cx).into_iter().filter(|l| l.contains("Rust")).collect();
    assert_eq!(lines, ["Rust biçimlendiricileri yüklenmedi: dosyalar araç zincirinde bulunamadı"]);
}

// --- Real lldb-dap ---

fn have_real_tools() -> bool {
    let have = |cmd: &str, args: &[&str]| std::process::Command::new(cmd).args(args).output().map(|o| o.status.success()).unwrap_or(false);
    if have("cargo", &["--version"]) && (have("lldb-dap", &["--help"]) || have("xcrun", &["-f", "lldb-dap"])) {
        return true;
    }
    crate::test_sshd::skip_missing("cargo,lldb-dap,xcrun");
    false
}

const VALUES: &str = "use std::collections::HashMap;\n\nfn main() {\n    let v = vec![1, 2, 3];\n    let s = String::from(\"merhaba\");\n    let m = HashMap::from([(1, \"a\")]);\n    let n = s.len();\n    println!(\"{v:?} {s} {m:?} {n}\");\n}\n";

/// A Cargo project stopped on line 7 (`let n = s.len();`), its variables open.
/// `launch`: a configuration named "Raw" in the launch file (the program is built first); otherwise the automatic Cargo one.
fn stopped_in_values<'a>(cx: &'a mut TestAppContext, name: &str, launch: Option<&str>) -> (Entity<Workbench>, &'a mut VisualTestContext, PathBuf) {
    let dir = project(name);
    std::fs::write(dir.join("Cargo.toml"), format!("[package]\nname = \"{}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n", name.replace('-', "_"))).unwrap();
    std::fs::write(dir.join("src/main.rs"), VALUES).unwrap();
    if let Some(launch) = launch {
        std::fs::create_dir_all(dir.join(".codeyard")).unwrap();
        std::fs::write(dir.join(".codeyard/launch.jsonc"), launch).unwrap();
        assert!(std::process::Command::new("cargo").arg("build").current_dir(&dir).status().unwrap().success());
    }
    let home = project(&format!("{name}-home"));
    let (wb, cx) = setup_persistent(cx, &home);
    open(&wb, &dir, cx);
    let main = std::fs::canonicalize(dir.join("src/main.rs")).unwrap().to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_file_in_root(main.clone(), window, cx));
    cx.run_until_parked();
    let ed = wb.read_with(cx, |wb, _| wb.active_editor_entity()).unwrap();
    ed.update(cx, |e, cx| e.toggle_breakpoint_at(6, cx));
    if launch.is_some() {
        start_named(&wb, cx, "Raw");
    } else {
        wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Debug: Start Debugging", window, cx));
    }
    wait_secs(cx, 240, "stopped after the values", |cx| {
        wb.read_with(cx, |wb, _| wb.debug_snapshot()).is_some_and(|s| s.0.starts_with("stopped") && s.2.as_ref().is_some_and(|(_, l)| *l == 6))
    });
    wait_secs(cx, 30, "the locals", |cx| wb.read_with(cx, |wb, _| wb.debug_variables()).iter().any(|(n, _)| n == "s"));
    (wb, cx, dir)
}

fn value_of(wb: &Entity<Workbench>, cx: &mut VisualTestContext, name: &str) -> String {
    wb.read_with(cx, |wb, _| wb.debug_variables()).into_iter().find(|(n, _)| n == name).map(|(_, v)| v).unwrap_or_default()
}

/// AC6: `String`, `Vec` and `HashMap` show as values; with the formatters left out (a configuration that already
/// "imports" a file that does nothing), the same program shows raw fields, which proves the test measures the change.
#[gpui::test]
fn real_lldb_dap_shows_rust_values(cx: &mut TestAppContext) {
    if !have_real_tools() {
        return;
    }
    let (wb, cx, _) = stopped_in_values(cx, "dbg-rust-values", None);
    assert_eq!(value_of(&wb, cx, "s"), "\"merhaba\"");
    wb.update(cx, |wb, cx| wb.toggle_variable_for_test("v", cx));
    wait_secs(cx, 30, "the elements of v", |cx| {
        let vars = wb.read_with(cx, |wb, _| wb.debug_variables_deep_for_test());
        vars.iter().any(|(n, v)| n == "[0]" && v == "1") && vars.iter().any(|(n, v)| n == "[2]" && v == "3")
    });
    assert!(value_of(&wb, cx, "m").contains("size=1"), "{}", value_of(&wb, cx, "m"));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Debug: Stop", window, cx));
}

#[gpui::test]
fn real_lldb_dap_without_the_formatters_shows_raw_fields(cx: &mut TestAppContext) {
    if !have_real_tools() {
        return;
    }
    // `lldb_lookup` in the configuration's own commands: nothing is added, and this one only prints.
    let launch = r#"{ "configurations": [ { "name": "Raw", "type": "lldb", "request": "launch", "program": "${workspaceFolder}/target/debug/dbg_rust_raw",
        "initCommands": ["script print('lldb_lookup left out')"] } ] }"#;
    let (wb, cx, _) = stopped_in_values(cx, "dbg-rust-raw", Some(launch));
    assert_ne!(value_of(&wb, cx, "s"), "\"merhaba\"", "without the formatters a String is not shown as text");
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Debug: Stop", window, cx));
}

/// AC7: with `rust-src` installed, stepping into `s.len()` opens the standard library's source.
#[gpui::test]
fn real_lldb_dap_steps_into_the_standard_library_source(cx: &mut TestAppContext) {
    if !have_real_tools() {
        return;
    }
    let sysroot = std::process::Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if !Path::new(&sysroot).join("lib/rustlib/src/rust/library").is_dir() {
        crate::test_sshd::skip_missing("rust-src");
        return;
    }
    let (wb, cx, _) = stopped_in_values(cx, "dbg-rust-step", None);
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Debug: Step Into", window, cx));
    wait_secs(cx, 60, "a frame in library/alloc/src", |cx| {
        wb.read_with(cx, |wb, _| wb.debug_snapshot()).is_some_and(|s| s.2.as_ref().is_some_and(|(p, _)| p.contains("library/alloc/src/")))
    });
    let path = wb.read_with(cx, |wb, _| wb.debug_snapshot()).unwrap().2.unwrap().0;
    assert!(Path::new(&path).is_file(), "the source exists: {path}");
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Debug: Stop", window, cx));
}
