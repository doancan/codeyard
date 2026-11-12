use super::*;
use crate::lsp::parse_runnables;

fn fixture(name: &str) -> Vec<Runnable> {
    let text = std::fs::read_to_string(format!("{}/testdata/rust-analyzer/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap();
    parse_runnables(&serde_json::from_str(&text).unwrap())
}

#[test]
fn real_runnables_of_a_library_become_commands_and_debug_plans() {
    let all = items(&fixture("runnables-lib.json"), &Options::default());
    // Entries without a place in the file (`cargo check -p demo --all-targets`) have no gutter mark.
    let shown: Vec<(&str, u32)> = all.iter().map(|i| (i.label.as_str(), i.line)).collect();
    assert_eq!(shown, [("test-mod tests", 10), ("test tests::adds_up", 14), ("test tests::fails_on_purpose", 19), ("test tests::ignored_one", 25)]);
    assert_eq!(all[1].command, "cargo test --package demo --lib -- tests::adds_up --exact --nocapture --include-ignored");
    assert_eq!(all[0].command, "cargo test --package demo --lib -- tests --nocapture");
    let plan = all[1].debug.as_ref().unwrap();
    assert_eq!(plan.build_args, ["test", "--package", "demo", "--lib", "--no-run"]);
    assert_eq!(plan.program_args, ["tests::adds_up", "--exact", "--nocapture", "--include-ignored", "--test-threads=1"]);
    assert!(plan.test);
}

#[test]
fn a_binary_runs_with_cargo_run_and_is_debugged_through_a_build() {
    let all = items(&fixture("runnables-main.json"), &Options::default());
    assert_eq!(all.len(), 1);
    assert_eq!((all[0].label.as_str(), all[0].kind, all[0].line), ("run demo", RunnableKind::Run, 0));
    assert_eq!(all[0].command, "cargo run --package demo --bin demo");
    let plan = all[0].debug.as_ref().unwrap();
    assert_eq!(
        (plan.build_args.as_slice(), plan.test, plan.program_args.is_empty()),
        (["build", "--package", "demo", "--bin", "demo"].map(String::from).as_slice(), false, true)
    );
}

#[test]
fn the_target_triple_goes_before_the_program_arguments() {
    let options = Options { target: Some("aarch64-unknown-linux-gnu".into()), ..Default::default() };
    let lib = items(&fixture("runnables-lib.json"), &options);
    assert_eq!(lib[1].command, "cargo test --package demo --lib --target aarch64-unknown-linux-gnu -- tests::adds_up --exact --nocapture --include-ignored");
    assert_eq!(lib[1].debug.as_ref().unwrap().build_args, ["test", "--package", "demo", "--lib", "--no-run", "--target", "aarch64-unknown-linux-gnu"]);
    let main = items(&fixture("runnables-main.json"), &options);
    assert_eq!(main[0].command, "cargo run --package demo --bin demo --target aarch64-unknown-linux-gnu");
    // A target the runnable already names is the runnable's own.
    let own = Runnable {
        label: "run x".into(),
        location: fixture("runnables-main.json")[0].location.clone(),
        command: RunnableCommand::Cargo {
            cargo_args: ["run", "--target", "wasm32-wasip1"].map(String::from).to_vec(),
            extra_args: vec![],
            executable_args: vec![],
            cwd: None,
            workspace_root: None,
        },
    };
    assert_eq!(command_line(&own, &options).unwrap(), "cargo run --target wasm32-wasip1");
}

#[test]
fn names_with_spaces_and_shell_characters_stay_one_word() {
    let runnable = Runnable {
        label: "test".into(),
        location: None,
        command: RunnableCommand::Cargo {
            cargo_args: ["test", "--package", "my pkg"].map(String::from).to_vec(),
            extra_args: ["--features", "a,b"].map(String::from).to_vec(),
            executable_args: ["tests::it's; $(id)", "--exact"].map(String::from).to_vec(),
            cwd: None,
            workspace_root: None,
        },
    };
    let line = command_line(&runnable, &Options::default()).unwrap();
    assert!(line.starts_with("cargo test --package 'my pkg' --features a,b -- "), "{line}");
    assert!(line.contains(r#"'tests::it'"'"'s; $(id)'"#), "{line}");
    assert!(line.ends_with(" --exact"));
}

#[test]
fn shell_runnables_and_doc_tests_cannot_be_debugged_or_run_as_cargo() {
    let shell = Runnable { label: "x".into(), location: None, command: RunnableCommand::Shell { program: "make".into(), args: vec![], cwd: None } };
    assert_eq!(command_line(&shell, &Options::default()), None);
    assert_eq!(debug_plan(&shell, &Options::default()), None);
    let doc = Runnable {
        label: "doctest".into(),
        location: None,
        command: RunnableCommand::Cargo {
            cargo_args: ["test", "--doc"].map(String::from).to_vec(),
            extra_args: vec![],
            executable_args: vec![],
            cwd: None,
            workspace_root: None,
        },
    };
    assert!(command_line(&doc, &Options::default()).is_some());
    assert_eq!(debug_plan(&doc, &Options::default()), None);
    let bench = Runnable {
        label: "bench".into(),
        location: None,
        command: RunnableCommand::Cargo { cargo_args: vec!["bench".into()], extra_args: vec![], executable_args: vec![], cwd: None, workspace_root: None },
    };
    assert_eq!(debug_plan(&bench, &Options::default()), None);
}

#[test]
fn the_runnables_at_a_position_are_the_innermost_first() {
    let list = fixture("runnables-lib.json");
    // Line 14 (0-based) is inside `tests::adds_up`, which is inside `mod tests`.
    let at = items_at(&list, (14, 8), &Options::default());
    assert_eq!(at.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(), ["test tests::adds_up", "test-mod tests"]);
    assert!(items_at(&list, (3, 0), &Options::default()).is_empty(), "outside every item");
}

const SOURCE: &str = "fn main() {\n    println!(\"hi\");\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn works() {}\n}\n";

#[test]
fn without_a_server_the_project_test_finder_and_the_entry_point_give_the_items() {
    let context = CrateContext { manifest: "crates/tool/Cargo.toml".into(), relative: "src/main.rs".into() };
    let got = fallback_items("crates/tool/src/main.rs", SOURCE, Some(&context), &Options::default());
    let shown: Vec<(&str, u32, &str)> = got.iter().map(|i| (i.label.as_str(), i.line, i.command.as_str())).collect();
    assert_eq!(
        shown,
        [
            ("run main", 0, "cargo run --manifest-path crates/tool/Cargo.toml"),
            ("test tests::works", 7, "cargo test --manifest-path crates/tool/Cargo.toml tests::works -- --exact"),
        ]
    );
    let run = got[0].debug.as_ref().unwrap();
    assert_eq!(run.build_args, ["build", "--manifest-path", "crates/tool/Cargo.toml"]);
    let test = got[1].debug.as_ref().unwrap();
    assert_eq!(test.build_args, ["test", "--manifest-path", "crates/tool/Cargo.toml", "--no-run"]);
    assert_eq!(test.program_args, ["tests::works", "--exact", "--nocapture", "--test-threads=1"]);
}

#[test]
fn fallback_entry_points_by_file_and_a_target_triple() {
    let options = Options { target: Some("x86_64-unknown-linux-musl".into()), ..Default::default() };
    let ctx = |relative: &str| CrateContext { manifest: "Cargo.toml".into(), relative: relative.into() };
    let one = |rel: &str| fallback_items(rel, "fn main() {}\n", Some(&ctx(rel)), &options).into_iter().next().map(|i| i.command);
    assert_eq!(one("src/bin/tool.rs").as_deref(), Some("cargo run --manifest-path Cargo.toml --bin tool --target x86_64-unknown-linux-musl"));
    assert_eq!(one("examples/demo/main.rs").as_deref(), Some("cargo run --manifest-path Cargo.toml --example demo --target x86_64-unknown-linux-musl"));
    assert_eq!(one("src/lib.rs"), None, "a function called main in a library is not an entry point");
    assert_eq!(one("build.rs"), None);
    let tests = fallback_items("tests/api.rs", "#[test]\nfn t() {}\n", None, &options);
    assert_eq!(tests[0].command, "cargo test --test api t --target x86_64-unknown-linux-musl -- --exact");
}

#[test]
fn gutter_lines_without_a_server_are_main_and_the_tests() {
    assert_eq!(mark_lines(SOURCE), [0, 7]);
    assert_eq!(mark_lines("pub fn main_helper() {}\nfn domain() {}\n"), Vec::<u32>::new());
    assert_eq!(mark_lines("async fn main() {}\n"), [0]);
    assert_eq!(mark_lines("// fn main() {}\n"), Vec::<u32>::new());
}

#[test]
fn built_executables_are_split_into_programs_and_test_binaries() {
    let output = [
        r#"{"reason":"compiler-artifact","target":{"name":"dep","kind":["lib"]},"profile":{"test":false},"executable":null}"#,
        r#"{"reason":"compiler-artifact","target":{"name":"demo","kind":["lib"]},"profile":{"test":true},"executable":"/t/deps/demo-1"}"#,
        r#"{"reason":"compiler-artifact","target":{"name":"demo","kind":["bin"]},"profile":{"test":false},"executable":"/t/demo"}"#,
        r#"{"reason":"compiler-artifact","target":{"name":"demo","kind":["bin"]},"profile":{"test":true},"executable":"/t/deps/demo-2"}"#,
        r#"{"reason":"compiler-artifact","target":{"name":"demo","kind":["bin"]},"profile":{"test":false},"executable":"/t/demo"}"#,
        "not json at all",
    ]
    .join("\n");
    let programs: Vec<String> = built_executables(&output, false).into_iter().map(|b| b.path).collect();
    assert_eq!(programs, ["/t/demo"], "duplicates are one program");
    let tests: Vec<(String, String)> = built_executables(&output, true).into_iter().map(|b| (b.name, b.path)).collect();
    assert_eq!(tests, [("demo".to_string(), "/t/deps/demo-1".to_string()), ("demo".to_string(), "/t/deps/demo-2".to_string())]);
}

#[test]
fn launch_configs_carry_the_program_its_arguments_and_the_folder() {
    let plan = DebugPlan { build_args: vec![], program_args: vec!["t".into(), "--exact".into()], test: true, cwd: None };
    let built = [Built { name: "a".into(), kind: "lib".into(), path: "/t/a".into() }, Built { name: "b".into(), kind: "bin".into(), path: "/t/b".into() }];
    let configs = launch_configs("Debug test t", &plan, "/w/", &built);
    assert_eq!(configs.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Debug test t (a, lib)", "Debug test t (b, bin)"]);
    assert_eq!(configs[0].args, serde_json::json!({ "program": "/t/a", "args": ["t", "--exact"], "cwd": "/w" }));
    assert_eq!(launch_configs("Debug run", &plan, "/w", &built[..1])[0].name, "Debug run");
}

#[test]
fn a_workspace_outside_the_folder_is_named_by_its_manifest() {
    let list = fixture("runnables-lib.json");
    let inside = Options { root: Some("/work/demo/".into()), ..Default::default() };
    assert_eq!(items(&list, &inside)[1].command, "cargo test --package demo --lib -- tests::adds_up --exact --nocapture --include-ignored");
    let nested = Options { root: Some("/work".into()), target: Some("wasm32-wasip1".into()), ..Default::default() };
    let got = items(&list, &nested);
    assert_eq!(
        got[1].command,
        "cargo test --manifest-path /work/demo/Cargo.toml --package demo --lib --target wasm32-wasip1 -- tests::adds_up --exact --nocapture --include-ignored"
    );
    assert_eq!(
        got[1].debug.as_ref().unwrap().build_args,
        ["test", "--manifest-path", "/work/demo/Cargo.toml", "--package", "demo", "--lib", "--no-run", "--target", "wasm32-wasip1"]
    );
}

#[test]
fn tests_run_through_nextest_when_asked_and_programs_do_not() {
    let options = Options { nextest: true, target: Some("wasm32-wasip1".into()), ..Default::default() };
    let lib = items(&fixture("runnables-lib.json"), &options);
    assert_eq!(
        lib[1].command,
        "cargo nextest run --no-fail-fast --package demo --lib --target wasm32-wasip1 --no-capture -- tests::adds_up --exact --include-ignored"
    );
    assert_eq!(lib[0].command, "cargo nextest run --no-fail-fast --package demo --lib --target wasm32-wasip1 --no-capture -- tests");
    let main = items(&fixture("runnables-main.json"), &options);
    assert_eq!(main[0].command, "cargo run --package demo --bin demo --target wasm32-wasip1", "a program is not a test");
    // The debugger still builds and starts the test executable itself.
    assert_eq!(lib[1].debug.as_ref().unwrap().build_args[0], "test");
    // Documentation tests are rustdoc's.
    let doc = Runnable {
        label: "doctest".into(),
        location: None,
        command: RunnableCommand::Cargo {
            cargo_args: ["test", "--doc"].map(String::from).to_vec(),
            extra_args: vec![],
            executable_args: vec![],
            cwd: None,
            workspace_root: None,
        },
    };
    assert_eq!(command_line(&doc, &options).as_deref(), Some("cargo test --doc --target wasm32-wasip1"));
    let fallback = fallback_items("tests/api.rs", "#[test]\nfn t() {}\n", None, &Options { nextest: true, ..Default::default() });
    assert_eq!(fallback[0].command, "cargo nextest run --no-fail-fast --test api t -- --exact");
}

/// A test of a workspace member runs in the member's folder, as `cargo test` runs it, not in the workspace root.
#[test]
fn the_debugged_program_runs_in_its_packages_folder() {
    let lib = items(&fixture("runnables-lib.json"), &Options::default());
    assert_eq!(lib[1].debug.as_ref().unwrap().cwd.as_deref(), Some("/work/demo"), "rust-analyzer's cwd is the package");
    let configs = launch_configs("t", lib[1].debug.as_ref().unwrap(), "/work", &[Built { name: "a".into(), kind: "lib".into(), path: "/t/a".into() }]);
    assert_eq!(configs[0].args["cwd"], "/work/demo");

    let context = CrateContext { manifest: "crates/tool/Cargo.toml".into(), relative: "src/lib.rs".into() };
    let got = fallback_items("crates/tool/src/lib.rs", "#[test]\nfn t() {}\n", Some(&context), &Options::default());
    let plan = got[0].debug.as_ref().unwrap();
    assert_eq!(plan.cwd.as_deref(), Some("crates/tool"));
    assert_eq!(launch_configs("t", plan, "/w/", &[Built { name: "a".into(), kind: "lib".into(), path: "/t/a".into() }])[0].args["cwd"], "/w/crates/tool");
    let root_package = CrateContext { manifest: "Cargo.toml".into(), relative: "src/lib.rs".into() };
    let top = fallback_items("src/lib.rs", "#[test]\nfn t() {}\n", Some(&root_package), &Options::default());
    assert_eq!(top[0].debug.as_ref().unwrap().cwd, None);
    assert_eq!(resolve_cwd("/w", None), "/w");
    assert_eq!(resolve_cwd("C:/w", Some("C:/w/pkg")), "C:/w/pkg");
}

/// For cmd and PowerShell a word with spaces is double-quoted, and a word the two read differently is not run at all.
#[test]
fn windows_command_lines_quote_for_cmd_and_powershell_or_refuse() {
    let windows = Options { windows: true, ..Default::default() };
    let make = |args: &[&str]| Runnable {
        label: "t".into(),
        location: None,
        command: RunnableCommand::Cargo {
            cargo_args: ["test", "--package", "demo"].map(String::from).to_vec(),
            extra_args: vec![],
            executable_args: args.iter().map(|s| s.to_string()).collect(),
            cwd: None,
            workspace_root: Some("C:/my work/demo".into()),
        },
    };
    let line = command_line(&make(&["tests::adds_up", "--exact"]), &Options { root: Some("C:/other".into()), ..windows.clone() }).unwrap();
    assert_eq!(line, "cargo test --manifest-path \"C:/my work/demo/Cargo.toml\" --package demo -- tests::adds_up --exact");
    for hostile in ["a\"b", "$(id)", "100%", "a&b", "a|b", "it's", "a^b", "trail\\", "x;y"] {
        assert_eq!(command_line(&make(&[hostile]), &windows), None, "{hostile}");
    }
    // The same names are fine, quoted the POSIX way, in a POSIX shell.
    assert!(command_line(&make(&["it's"]), &Options::default()).is_some());
    assert_eq!(word_for("plain", true).as_deref(), Some("plain"));
    assert_eq!(word_for("a b", true).as_deref(), Some("\"a b\""));
    assert_eq!(word_for("--flag=1", true).as_deref(), Some("--flag=1"));
}

/// The Debug row says what debugging does (the build and the program's arguments), not the command Run would type.
#[test]
fn a_debug_plan_describes_the_build_and_the_program() {
    let with_nextest = Options { nextest: true, ..Default::default() };
    let lib = items(&fixture("runnables-lib.json"), &with_nextest);
    assert!(lib[1].command.starts_with("cargo nextest run"), "{}", lib[1].command);
    let plan = lib[1].debug.as_ref().unwrap();
    assert_eq!(plan.describe(false), "cargo test --package demo --lib --no-run \u{2192} tests::adds_up --exact --nocapture --include-ignored --test-threads=1");
    let main = items(&fixture("runnables-main.json"), &Options::default());
    assert_eq!(main[0].debug.as_ref().unwrap().describe(false), "cargo build --package demo --bin demo");
}
