use super::*;
use serde_json::json;

fn metadata(kind: &str) -> Value {
    json!({
        "workspace_members": ["member"],
        "packages": [
            {"id":"member", "name":"actual-package", "manifest_path":"/workspace/member path/Cargo.toml", "targets":[
                {"name":"worker", "kind":[kind], "required-features":["cli","format"]}
            ]},
            {"id":"dependency", "name":"dependency", "manifest_path":"/cache/dep/Cargo.toml", "targets":[{"name":"unrelated", "kind":["bin"]}]}
        ]
    })
}

#[test]
fn rust_hardening_metadata_uses_workspace_members_actual_target_names_and_required_features() {
    let targets = parse_metadata(&metadata("bin").to_string()).unwrap();
    assert_eq!(targets.len(), 1);
    let target = &targets[0];
    assert_eq!((target.package.as_str(), target.name.as_str()), ("actual-package", "worker"));
    assert_eq!(
        target.build_args(),
        ["build", "--message-format=json", "--manifest-path", "/workspace/member path/Cargo.toml", "--bin", "worker", "--features", "cli,format"]
    );
    let configs = launch_configs(targets, "/workspace");
    assert_eq!(configs[0].name, "Debug Rust: worker");
    assert!(configs[0].args.get("program").is_none(), "Cargo's actual artifact determines the program later");
}

#[test]
fn rust_hardening_library_and_integration_targets_build_test_harnesses_and_filter_artifacts() {
    for kind in ["lib", "rlib", "proc-macro", "test"] {
        let mut targets = parse_metadata(&metadata(kind).to_string()).unwrap();
        let target = targets.remove(0);
        let args = target.build_args();
        assert_eq!(args[0], "test");
        assert!(args.iter().any(|a| a == "--no-run"));
        let mut artifact =
            json!({"reason":"compiler-artifact", "target":{"name":"worker", "kind":[kind]}, "profile":{"test":true}, "executable":"/custom/tests/worker"});
        assert!(target.accepts_artifact(&artifact), "{kind}");
        artifact["profile"]["test"] = json!(false);
        assert!(!target.accepts_artifact(&artifact), "never debug an ordinary library artifact as a test");
    }
}

#[test]
fn rust_hardening_failed_test_builds_reject_partial_artifacts_and_missing_exit_status() {
    let artifact = json!({"reason":"compiler-artifact","target":{"name":"first","kind":["test"]},"profile":{"test":true},"executable":"/old/test"});
    let diagnostic = json!({"reason":"compiler-message","message":{"rendered":"error[E0308]: wrong type\n --> src/lib.rs:2:5\n"}});
    let output = format!("{artifact}\n{diagnostic}\n{}\n", json!({"reason":"build-finished","success":false}));
    for exit in [Some(101), None, Some(0)] {
        let error = crate::testexp::built_test_executables(&output, exit).unwrap_err();
        assert!(error.contains("error[E0308]") && !error.contains("compiler-artifact"));
    }
    assert!(crate::testexp::built_test_executables(&artifact.to_string(), Some(0)).is_ok());
}

#[test]
fn rust_hardening_cargo_argument_decoding_preserves_shell_quoted_paths_as_literal_argv() {
    for name in ["space name", "quote'name", "$(touch marker)", "a;b", "dollar$HOME", "Türkçe crate"] {
        let command = format!("cargo test --manifest-path {} --test=api a::b -- --exact", crate::shell_quote::word(&format!("{name}/Cargo.toml")));
        assert_eq!(crate::cargo_args::selection(&command).unwrap(), ["--manifest-path", &format!("{name}/Cargo.toml"), "--test", "api"]);
    }
    assert!(crate::cargo_args::selection("cargo test --test 'unterminated").is_err());
    assert!(crate::cargo_args::selection("cargo test --manifest-path").is_err());
    assert!(crate::cargo_args::selection("echo test --test api").is_err());
}

#[test]
fn rust_hardening_metadata_invalid_documents_and_disabled_tests_fail_safely() {
    for source in ["", "null", "[]", "{", r#"{"packages":[]}"#] {
        assert!(parse_metadata(source).is_err(), "{source}");
    }
    let mut meta = metadata("lib");
    meta["packages"][0]["targets"][0]["test"] = json!(false);
    assert!(parse_metadata(&meta.to_string()).unwrap().is_empty());
    assert_eq!(diagnostics(r#"{"reason":"compiler-message","message":{"message":"fallback compiler error"}}"#), "fallback compiler error\n");
}

#[test]
fn rust_hardening_test_plans_use_the_nearest_manifest_instead_of_guessing_the_package_name() {
    let manifests = ["Cargo.toml".into(), "packages/folder name/Cargo.toml".into()].into();
    let rel = "packages/folder name/src/nested/checks.rs";
    let context = context_from_files(rel, &manifests).unwrap();
    let source = "#[test]\nfn works() {}\n";
    let run = crate::testrun::plan_in_crate(rel, source, 1, false, None, Some(&context)).unwrap();
    assert_eq!(run.id, "nested::checks::works");
    assert_eq!(run.command, "cargo test --manifest-path 'packages/folder name/Cargo.toml' nested::checks::works -- --exact");
    let cases = crate::testrun::cases_in_crate(rel, source, None, Some(&context));
    assert_eq!(cases[0].run, run);
}

#[test]
fn rust_hardening_real_virtual_workspace_respects_features_target_names_and_custom_output() {
    let dir = crate::qa_workbench_tests::fresh("rust-hardening-real-cargo-workspace");
    std::fs::create_dir_all(dir.join("member path/src")).unwrap();
    std::fs::create_dir_all(dir.join(".cargo")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"member path\"]\nresolver = \"3\"\n").unwrap();
    std::fs::write(dir.join(".cargo/config.toml"), "[build]\ntarget-dir = \"custom output\"\n").unwrap();
    std::fs::write(dir.join("member path/Cargo.toml"), "[package]\nname = \"actual-package\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[features]\ncli = []\n[[bin]]\nname = \"worker-tool\"\npath = \"src/worker.rs\"\nrequired-features = [\"cli\"]\n").unwrap();
    std::fs::write(dir.join("member path/src/worker.rs"), "fn main() { println!(\"cargo-target-ok\"); }\n").unwrap();
    let host = host::local();
    let root = dir.to_str().unwrap();
    let targets = discover(&host, root).unwrap();
    assert_eq!(targets.len(), 1);
    let target = &targets[0];
    assert_eq!((target.package.as_str(), target.name.as_str()), ("actual-package", "worker-tool"));
    assert_eq!(target.features, ["cli"]);
    let args = target.build_args();
    let output = host.exec("cargo", &args.iter().map(String::as_str).collect::<Vec<_>>(), Some(root), None).unwrap();
    assert!(output.ok(), "{}", output.stderr_str());
    let artifact = output.stdout_str().lines().filter_map(|line| serde_json::from_str::<Value>(line).ok()).find(|m| target.accepts_artifact(m)).unwrap();
    let program = artifact["executable"].as_str().unwrap();
    assert!(program.contains("custom output") && program.contains("worker-tool"), "{program}");
    let ran = host.exec(program, &[], Some(root), None).unwrap();
    assert!(ran.ok());
    assert_eq!(ran.stdout_str().trim(), "cargo-target-ok");
}

#[test]
fn the_chosen_features_profile_and_output_folder_reach_the_build() {
    let mut target = parse_metadata(&metadata("bin").to_string()).unwrap().remove(0);
    assert_eq!(target.build.summary(), None, "Cargo's defaults");
    target.build = BuildOptions {
        features: vec!["format".into(), "tls".into()],
        no_default_features: true,
        profile: Some("release".into()),
        target_dir: Some("out dir".into()),
    };
    let args = target.build_args();
    let tail = &args[args.iter().position(|a| a == "--features").unwrap()..];
    assert_eq!(tail, ["--features", "cli,format,tls", "--no-default-features", "--profile", "release", "--target-dir", "out dir"]);
    assert_eq!(target.build.summary().as_deref(), Some("release · features: format, tls · no default features · output: out dir"));
}
