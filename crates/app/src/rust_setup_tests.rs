//! The optional Rust setup: exact JSONC changes, comments kept, reset to the original text.

use super::*;

const USER: &str = "// my settings\n{\n  // keep me\n  \"editor.tabSize\": 2,\n  \"lsp.servers\": {\n    // go stays\n    \"go\": null,\n  },\n  \"rust.features\": [\"cli\"],\n}\n";

#[test]
fn nested_values_are_set_and_removed_without_touching_comments() {
    let text = settings::set_path_in(USER, &["lsp.servers", "rust", "settings", "a"], Some(&serde_json::json!(1)));
    assert!(text.contains("// go stays") && text.contains("// keep me"), "{text}");
    let v: Value = serde_json::from_str(&theme::themes::strip_jsonc(&text)).unwrap();
    assert_eq!(v["lsp.servers"]["rust"]["settings"]["a"], 1);
    assert_eq!(v["lsp.servers"]["go"], Value::Null);
    let text = settings::set_path_in(&text, &["lsp.servers", "rust", "settings", "b"], Some(&serde_json::json!("x")));
    let text = settings::set_path_in(&text, &["lsp.servers", "rust", "settings", "a"], Some(&serde_json::json!(2)));
    let v: Value = serde_json::from_str(&theme::themes::strip_jsonc(&text)).unwrap();
    assert_eq!(v["lsp.servers"]["rust"]["settings"], serde_json::json!({ "a": 2, "b": "x" }));
    let text = settings::set_path_in(&text, &["lsp.servers", "rust", "settings", "a"], None);
    let text = settings::set_path_in(&text, &["lsp.servers", "rust", "settings", "b"], None);
    assert_eq!(text, USER, "removing the last member also removes the objects it emptied");
    assert_eq!(settings::set_path_in(USER, &["missing", "x"], None), USER);
    let replaced = settings::set_path_in("{ \"a\": null }", &["a", "b"], Some(&serde_json::json!(true)));
    assert_eq!(serde_json::from_str::<Value>(&replaced).unwrap(), serde_json::json!({ "a": { "b": true } }));
}

#[test]
fn the_rust_setup_previews_its_exact_change_and_resets_to_the_original() {
    let mut s = Settings::default();
    s.rust_build.features = vec!["cli".into()];
    let (applied, diff) = rust_setup_change(USER, &s, false);
    assert!(applied.contains("// my settings") && applied.contains("// keep me") && applied.contains("// go stays"), "{applied}");
    let v: Value = serde_json::from_str(&theme::themes::strip_jsonc(&applied)).unwrap();
    let ra = &v["lsp.servers"]["rust"]["settings"];
    assert_eq!(ra["rust-analyzer.check.command"], "clippy");
    assert_eq!(ra["rust-analyzer.cargo.features"], serde_json::json!(["cli"]), "checks use the build's features");
    assert_eq!(ra["rust-analyzer.cargo.targetDir"], true);
    assert_eq!((&v["editor.formatters"]["rust"], &v["editor.formatOnSave"]), (&serde_json::json!("lsp"), &serde_json::json!(true)));
    // The preview lists every added line and nothing else.
    let added: Vec<_> = diff.lines().filter(|l| l.starts_with('+') && !l.starts_with("+++")).collect();
    let removed: Vec<_> = diff.lines().filter(|l| l.starts_with('-') && !l.starts_with("---")).collect();
    assert!(added.iter().any(|l| l.contains("rust-analyzer.check.command")), "{diff}");
    assert!(removed.is_empty(), "{diff}");
    assert_eq!(rust_setup_change(&applied, &s, false).1, "", "applying twice changes nothing");
    let (reset, diff) = rust_setup_change(&applied, &s, true);
    assert!(diff.lines().any(|l| l.starts_with('-') && l.contains("clippy")), "{diff}");
    assert_eq!(reset, USER, "reset removes exactly what the setup added");
}

#[gpui::test]
fn the_rust_setup_command_writes_after_confirmation_and_resets(cx: &mut gpui::TestAppContext) {
    let dir = crate::qa_workbench_tests::fresh("rust-setup-preset");
    std::fs::write(dir.join("settings.jsonc"), USER).unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &dir);
    let read = || std::fs::read_to_string(dir.join("settings.jsonc")).unwrap();
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Preferences: Apply Rust Setup…", window, cx));
    cx.run_until_parked();
    crate::question::cancel_keeps_focus(&wb, cx, "Cancel");
    cx.run_until_parked();
    assert_eq!(read(), USER, "nothing is written without confirmation");
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Preferences: Apply Rust Setup…", window, cx));
    cx.run_until_parked();
    cx.simulate_prompt_answer("Apply");
    cx.run_until_parked();
    assert!(read().contains("\"rust-analyzer.check.command\":\"clippy\"") && read().contains("// keep me"), "{}", read());
    assert!(cx.update(|_, cx| cx.global::<Settings>().format_on_save), "applied at once");
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Preferences: Reset Rust Setup…", window, cx));
    cx.run_until_parked();
    cx.simulate_prompt_answer("Reset");
    cx.run_until_parked();
    assert_eq!(read(), USER);
}

#[gpui::test]
fn reset_settings_restores_the_template_and_keeps_a_backup(cx: &mut gpui::TestAppContext) {
    let dir = crate::qa_workbench_tests::fresh("reset-settings");
    let broken = "{ \"editor.tabSize\": 2, this is not json";
    std::fs::write(dir.join("settings.jsonc"), broken).unwrap();
    let (wb, cx) = crate::qa_workbench_tests::setup_with_config(cx, &dir);
    let read = || std::fs::read_to_string(dir.join("settings.jsonc")).unwrap();
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Preferences: Reset Settings to Defaults…", window, cx));
    cx.run_until_parked();
    crate::question::cancel_keeps_focus(&wb, cx, "Cancel");
    cx.run_until_parked();
    assert_eq!(read(), broken, "nothing is written without confirmation");
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Preferences: Reset Settings to Defaults…", window, cx));
    cx.run_until_parked();
    cx.simulate_prompt_answer("Reset");
    cx.run_until_parked();
    assert_eq!(read(), settings::TEMPLATE);
    let backups: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("settings.backup-")).collect();
    assert_eq!(backups.len(), 1, "the old file is kept");
    assert_eq!(std::fs::read_to_string(backups[0].path()).unwrap(), broken);
}

#[test]
fn a_second_reset_in_the_same_second_keeps_the_first_backup() {
    let dir = crate::qa_workbench_tests::fresh("reset-settings-twice");
    let path = dir.join("settings.jsonc");
    std::fs::write(&path, "{ \"mine\": 1 }").unwrap();
    let first = write_default_settings(&path).unwrap().unwrap();
    let second = write_default_settings(&path).unwrap().unwrap();
    assert_ne!(first, second);
    assert_eq!(std::fs::read_to_string(first).unwrap(), "{ \"mine\": 1 }", "the user's own file survives");
}
