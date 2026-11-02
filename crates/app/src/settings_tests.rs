use super::*;
use crate::globs::{is_excluded, name_matches, path_matches};
use serde_json::json;

/// A repository could fill the clipboard through the terminal and switch off the checks on what is pasted back, so a
/// later paste would run its command: these keys come only from the user's own settings, even in a trusted folder.
#[test]
fn a_project_cannot_loosen_the_terminal_clipboard_and_paste_checks() {
    use crate::terminal_paste::{Sanitize, WarnMultiline};
    let project = [(
        ".codeyard/settings.jsonc",
        r#"{ "terminal.allowClipboardWrite": true, "terminal.sanitizePaste": "off", "terminal.warnMultilinePaste": "never" }"#,
        true,
    )];
    for trusted in [false, true] {
        let (s, w) = parse_layers_trusted("{}", &project, trusted).unwrap();
        assert!(!s.terminal_allow_clipboard_write, "trusted={trusted}");
        assert_eq!(s.terminal_sanitize_paste, Sanitize::Auto, "trusted={trusted}");
        assert_eq!(s.terminal_warn_multiline, WarnMultiline::Unbracketed, "trusted={trusted}");
        assert_eq!(w.iter().filter(|m| m.contains("can only be set in your own settings.jsonc")).count(), 3, "{w:?}");
    }
    let (s, _) = parse_layers(r#"{ "terminal.allowClipboardWrite": true, "terminal.sanitizePaste": "off" }"#, &[]).unwrap();
    assert!(s.terminal_allow_clipboard_write && s.terminal_sanitize_paste == Sanitize::Off, "the user can still choose");
}

#[test]
fn globs_are_linear_even_for_hostile_patterns() {
    let t = std::time::Instant::now();
    assert!(!name_matches("*a*a*a*a*a*a*a*a*a*a*a*a*a*a*a*b", &"a".repeat(60)));
    assert!(name_matches("*a*a*a*a*a*a*a*a*a*a*a*a*a*a*a*", &"a".repeat(60)));
    assert!(!path_matches("**/**/**/**/**/**/**/**/**/**/**/**/x", &vec!["d"; 60].join("/")));
    assert!(path_matches("**/**/**/**/**/**/**/d", &vec!["d"; 60].join("/")));
    assert!(!name_matches(&format!("{}b", "?*".repeat(200)), &"a".repeat(2000)));
    assert!(t.elapsed().as_millis() < 1000 * u128::from(crate::test_sshd::timeout_scale()), "took {:?}", t.elapsed());
}

#[test]
fn globs_keep_their_meaning() {
    for (p, n, want) in [
        ("*.log", "a.log", true),
        ("*.log", "a.logx", false),
        ("a?c", "abc", true),
        ("a?c", "ac", false),
        ("*", "", true),
        ("a*", "a", true),
        ("*a", "ba", true),
        ("*a", "bab", false),
        ("", "", true),
        ("", "x", false),
        ("build/", "build", true),
        ("ç*ş", "çaşış", true),
    ] {
        assert_eq!(name_matches(p, n), want, "{p} vs {n}");
    }
    for (p, n, want) in [
        ("**/node_modules", "node_modules", true),
        ("**/node_modules", "a/b/node_modules", true),
        ("**/node_modules", "a/node_modules/x", false),
        ("src/**/*.rs", "src/a/b/c.rs", true),
        ("src/**/*.rs", "src/c.rs", true),
        ("src/**/*.rs", "lib/c.rs", false),
        ("a/*/c", "a/b/c", true),
        ("a/*/c", "a/b/x/c", false),
        ("./build/out", "build/out", true),
    ] {
        assert_eq!(path_matches(p, n), want, "{p} vs {n}");
    }
}

#[test]
fn color_customizations_parse_globally_and_per_theme_and_report_mistakes() {
    let text = r##"{ "workbench.colorCustomizations": { "accent": "#ff8800", "[Quarry]": { "syntax-string": "#a6e22e" }, "nope": "#fff", "text": "red" } }"##;
    let (s, w) = parse_layers(text, &[]).unwrap();
    assert_eq!(s.color_overrides.len(), 2);
    assert!(s.color_overrides.contains(&(None, "accent".into(), 0xff8800)));
    assert!(s.color_overrides.contains(&(Some("Quarry".into()), "syntax-string".into(), 0xa6e22e)));
    assert_eq!(w.len(), 2, "{w:?}");
    assert!(w.iter().any(|x| x.contains("unknown color role")) && w.iter().any(|x| x.contains("expected a color")));
}

#[test]
fn customization_settings_are_read() {
    let (s, w) = parse_layers(r#"{ "workbench.tabs.closeButton": "off", "editor.cursorWidth": 4, "workbench.fontFamily": "Inter", "files.associations": { "*.tpl": "html" }, "search.exclude": ["dist"], "editor.hover.enabled": false }"#, &[]).unwrap();
    assert!(w.is_empty(), "{w:?}");
    assert_eq!((s.tab_close.as_str(), s.cursor_width, s.ui_font.as_deref()), ("off", 4., Some("Inter")));
    assert_eq!(s.file_associations, [("*.tpl".to_string(), "html".to_string())]);
    assert_eq!((s.search_exclude.as_slice(), s.hover_enabled), (&["dist".to_string()][..], false));
    let (_, w) = parse_layers(r#"{ "workbench.tabs.closeButton": "sometimes", "editor.cursorWidth": 99 }"#, &[]).unwrap();
    assert_eq!(w.len(), 2);
}

/// A repository must not be able to switch the request confirmation off (or pick its own value): user settings only.
#[test]
fn http_confirm_requests_parses_and_cannot_come_from_a_project() {
    assert_eq!(Settings::default().http_confirm, "remote", "the default asks for the first request to a new remote host");
    for v in ["never", "remote", "always"] {
        let (s, w) = parse_layers(&format!(r#"{{ "http.confirmRequests": "{v}" }}"#), &[]).unwrap();
        assert_eq!((s.http_confirm.as_str(), w.len()), (v, 0), "{w:?}");
    }
    let (s, w) = parse_layers(r#"{ "http.confirmRequests": "sometimes" }"#, &[]).unwrap();
    assert_eq!(s.http_confirm, "remote", "an invalid value keeps the default");
    assert_eq!(w.len(), 1, "{w:?}");
    let project = [(".codeyard/settings.jsonc", r#"{ "http.confirmRequests": "never" }"#, true)];
    for trusted in [false, true] {
        let (s, w) = parse_layers_trusted("{}", &project, trusted).unwrap();
        assert_eq!(s.http_confirm, "remote", "trusted={trusted}: a folder cannot turn the question off");
        assert!(w.iter().any(|m| m.contains("only be set in your own")), "{w:?}");
    }
    let (s, _) = parse_layers_trusted(r#"{ "http.confirmRequests": "always" }"#, &project, true).unwrap();
    assert_eq!(s.http_confirm, "always", "the user's own value stands");
}

/// `update.check` decides whether the app contacts github.com by itself: a repository must never choose it, trusted or not.
#[test]
fn update_check_parses_and_cannot_come_from_a_project() {
    use crate::update::Policy;
    assert_eq!(Settings::default().update_check, Policy::Ask, "undecided until the user answers");
    for (v, want) in [("ask", Policy::Ask), ("daily", Policy::Daily), ("manual", Policy::Manual)] {
        let (s, w) = parse_layers(&format!(r#"{{ "update.check": "{v}" }}"#), &[]).unwrap();
        assert_eq!((s.update_check, w.len()), (want, 0), "{w:?}");
    }
    let (s, w) = parse_layers(r#"{ "update.check": "weekly" }"#, &[]).unwrap();
    assert_eq!(s.update_check, Policy::Ask, "an invalid value keeps the default");
    assert_eq!(w.len(), 1, "{w:?}");
    let project = [(".codeyard/settings.jsonc", r#"{ "update.check": "daily" }"#, true), (".vscode/settings.json", r#"{ "update.check": "daily" }"#, false)];
    for trusted in [false, true] {
        let (s, w) = parse_layers_trusted(r#"{ "update.check": "manual" }"#, &project, trusted).unwrap();
        assert_eq!(s.update_check, Policy::Manual, "trusted={trusted}: the user's own value wins over every folder file");
        assert!(w.iter().any(|m| m.contains("only be set in your own")), "{w:?}");
        let (s, _) = parse_layers_trusted("{}", &project, trusted).unwrap();
        assert_eq!(s.update_check, Policy::Ask, "trusted={trusted}: a folder cannot turn network checks on");
    }
}

#[test]
fn untrusted_project_layer_drops_restricted_keys_and_says_so() {
    let project = [(
        ".codeyard/settings.jsonc",
        r#"{ "terminal.shell": "/x", "lsp.servers": { "c": { "command": "/y" } }, "security.workspaceTrust.enabled": false, "editor.tabSize": 2 }"#,
        true,
    )];
    let (s, w) = parse_layers_trusted("{}", &project, false).unwrap();
    assert_eq!((s.terminal_shell, s.lsp_servers.len(), s.tab_size), (None, 0, Some(2)));
    assert_eq!(w.iter().filter(|m| m.contains("ignored until you trust")).count(), 2, "{w:?}");
    assert_eq!(w.iter().filter(|m| m.contains("only be set in your own")).count(), 1, "{w:?}");
    // `parse_layers` (no trust given) is the safe default.
    assert_eq!(parse_layers("{}", &project).unwrap().0.terminal_shell, None);
    let (s, w) = parse_layers_trusted("{}", &project, true).unwrap();
    assert_eq!((s.terminal_shell.as_deref(), s.lsp_servers.len()), (Some("/x"), 1));
    assert!(s.workspace_trust == Settings::default().workspace_trust, "a trusted project still cannot flip the gate");
    assert_eq!(w.len(), 1, "{w:?}");
    // The user's own file is never restricted.
    let (s, _) = parse_layers_trusted(r#"{ "terminal.shell": "/mine" }"#, &[], false).unwrap();
    assert_eq!(s.terminal_shell.as_deref(), Some("/mine"));
}

#[test]
fn project_layers_override_user_settings() {
    let user = r#"{ "editor.tabSize": 4, "theme": "ember" }"#;
    let (s, w) = parse_layers(
        user,
        &[
            (".vscode/settings.json", r#"{ "editor.tabSize": 2, "python.linting": true }"#, false),
            (".codeyard/settings.jsonc", r#"{ "editor.fontSize": 15 }"#, true),
        ],
    )
    .unwrap();
    assert_eq!((s.tab_size, s.theme.as_str(), s.editor_font_size), (Some(2), "ember", 15.));
    assert!(w.is_empty(), "extension settings do not warn: {w:?}");
    let (_, w) = parse_layers(user, &[(".codeyard/settings.jsonc", "{ bozuk", true)]).unwrap();
    assert_eq!(w.len(), 1, "a broken project file only warns");
}

#[test]
fn exclude_globs_like_vscode() {
    let ex = |p: &[&str], path: &str| is_excluded(&p.iter().map(|s| s.to_string()).collect::<Vec<_>>(), path);
    assert!(ex(&["**/node_modules"], "node_modules"));
    assert!(ex(&["**/node_modules"], "web/node_modules/react/index.js"));
    assert!(ex(&["node_modules"], "a/node_modules"), "adla her seviyede");
    assert!(ex(&["build/out"], "build/out/x.o"));
    assert!(!ex(&["build/out"], "src/build/out"), "relative to the root");
    assert!(ex(&["**/*.log"], "logs/a.log"));
    assert!(!ex(&["**/node_modules"], "src/node_modules_x/a"));
}

#[test]
fn template_parses_cleanly() {
    let (s, w) = parse(TEMPLATE);
    assert!(w.is_empty(), "{w:?}");
    // The template shows the shipped defaults; unit tests run with language servers off.
    assert_eq!(s, Settings { lsp_enabled: true, ..Settings::default() });
    assert!(parse_keybindings(KEYBINDINGS_TEMPLATE).1.is_empty());
}

#[test]
fn bad_values_fall_back_and_warn() {
    let (s, w) = parse(r#"{ "editor.fontSize": "büyük", "editor.lineHeight": 500, "files.autoSave": "sometimes", "bilinmeyen": 1, "editor.tabSize": 2 }"#);
    assert_eq!(s.editor_font_size, Settings::default().editor_font_size);
    assert_eq!(s.editor_line_height, Settings::default().editor_line_height);
    assert_eq!(s.auto_save, AutoSave::Off);
    assert_eq!(s.tab_size, Some(2), "valid ones are applied");
    assert_eq!(w.len(), 4, "{w:?}");
}

#[test]
fn auto_save_with_delay() {
    let (s, _) = parse(r#"{ "files.autoSave": "afterDelay", "files.autoSaveDelay": 500 }"#);
    assert_eq!(s.auto_save, AutoSave::AfterDelay(500));
}

#[test]
fn set_string_keeps_comments_and_other_lines() {
    let text = "{\n  // \"theme\": \"eski yorum\"\n  \"theme\": \"auto\", // seçim\n  \"x\": 1,\n}\n";
    let out = set_string_in(text, "theme", "fjord");
    assert_eq!(out, "{\n  // \"theme\": \"eski yorum\"\n  \"theme\": \"fjord\", // seçim\n  \"x\": 1,\n}\n");
    let added = set_string_in("{\n  \"x\": 1\n}\n", "theme", "paper");
    assert_eq!(parse(&added).0.theme, "paper");
}

#[test]
fn set_value_updates_inline_root_keys_without_shadowing_or_touching_nested_keys() {
    let text = "/* { \"theme\": \"comment\" } */\n{\"other\": {\"theme\": \"nested\"}, \"theme\": \"codeyard-light\", \"editor.fontSize\": 13}";
    let out = set_string_in(text, "theme", "codeyard-dark");
    assert_eq!(out, text.replace("codeyard-light", "codeyard-dark"));
    assert_eq!(parse(&out).0.theme, "codeyard-dark");
    let out = set_value_in(&out, "editor.fontSize", &serde_json::json!(15));
    assert_eq!(parse(&out).0.editor_font_size, 15.);
    assert!(out.contains("\"other\": {\"theme\": \"nested\"}"));
}

#[test]
fn set_value_preserves_nested_comments_and_updates_duplicate_or_escaped_keys() {
    let text = "{\"files.exclude\": [\n // } ] must not end the value\n \"target\" /* [ */\n], \"theme\": \"old\", \"th\\u0065me\": \"last\"}";
    let out = set_value_in(text, "files.exclude", &serde_json::json!([".git"]));
    assert_eq!(out, "{\"files.exclude\": [\".git\"], \"theme\": \"old\", \"th\\u0065me\": \"last\"}");
    let out = set_string_in(&out, "theme", "fjord");
    assert_eq!(parse(&out).0.theme, "fjord");
    assert_eq!(out.matches("\"fjord\"").count(), 2);
}

#[test]
fn keybindings_and_unbinding() {
    let (b, w) = parse_keybindings(r#"[{"key":"cmd-1","command":"editor.save","when":"Editor"},{"key":"ctrl-g","command":null},{"key":"x"}]"#);
    assert_eq!(b.len(), 2);
    assert_eq!(b[1].command, None);
    assert_eq!(w.len(), 1);
}

#[test]
fn excludes() {
    let ex = vec!["node_modules".to_string(), "*.log".to_string(), ".git".to_string()];
    assert!(is_excluded(&ex, "web/node_modules/x/i.js"));
    assert!(is_excluded(&ex, "logs/build.log"));
    assert!(!is_excluded(&ex, "src/main.rs"));
    assert!(name_matches("*.min.js", "app.min.js"));
}

#[test]
fn set_value_replaces_multiline_values() {
    let text = "{\n  // yorum: \"files.exclude\": []\n  \"files.exclude\": [\n    \".git\",\n    \"target\"\n  ],\n  \"editor.fontSize\": 13,\n}\n";
    let out = set_value_in(text, "files.exclude", &serde_json::json!(["**/.git"]));
    assert!(out.contains("\"files.exclude\": [\"**/.git\"],\n  \"editor.fontSize\": 13"), "{out}");
    assert!(out.contains("// yorum: \"files.exclude\": []"), "comment is preserved");
    let out = set_value_in(&out, "editor.fontSize", &serde_json::json!(15));
    let (s, w) = parse(&out);
    assert!(w.is_empty(), "{w:?}");
    assert_eq!(s.editor_font_size, 15.);
    assert_eq!(s.files_exclude, vec!["**/.git".to_string()]);
    let out = set_value_in(&out, "editor.formatOnSave", &serde_json::json!(true));
    assert!(parse(&out).0.format_on_save, "added when missing");
}

mod code_actions_on_save {
    use super::super::*;

    #[test]
    fn reads_vscode_object_and_plain_list() {
        let (s, w) = parse(
            r#"{ "editor.codeActionsOnSave": { "source.organizeImports": "explicit", "source.fixAll": true, "source.sortMembers": false, "source.x": "never", "quickfix": true } }"#,
        );
        assert!(w.is_empty(), "{w:?}");
        let mut got = s.code_actions_on_save.clone();
        got.sort();
        assert_eq!(got, ["source.fixAll", "source.organizeImports"], "disabled and non-source entries are skipped");
        let (s, _) = parse(r#"{ "editor.codeActionsOnSave": ["source.organizeImports"] }"#);
        assert_eq!(s.code_actions_on_save, ["source.organizeImports"]);
        let (s, w) = parse(r#"{ "editor.codeActionsOnSave": 3 }"#);
        assert!(s.code_actions_on_save.is_empty() && w.len() == 1, "{w:?}");
    }

    #[test]
    fn formatters_are_chosen_per_language() {
        let (s, w) = parse(r#"{ "editor.formatters": { "toml": "you.taplo", "rust": "lsp", "go": 3 } }"#);
        assert_eq!(s.formatters, [("toml".to_string(), "you.taplo".to_string()), ("rust".to_string(), "lsp".to_string())]);
        assert_eq!(w.len(), 1, "the wrong entry is named: {w:?}");
        let (s, w) = parse(r#"{ "editor.formatters": "you.taplo" }"#);
        assert!(s.formatters.is_empty() && w.len() == 1, "{w:?}");
    }
}

mod hardening {
    use super::super::*;

    /// Every known key given a value of every type must neither crash parsing nor go unreported: it warns.
    #[test]
    fn wrong_types_for_every_key_only_warn() {
        let keys: Vec<&str> = {
            let mut v: Vec<&str> = crate::workbench::settings_ui::CATALOG.iter().map(|e| e.key).collect();
            v.extend([
                "theme",
                "workbench.colorCustomizations",
                "files.associations",
                "ports.rules",
                "lsp.servers",
                "json.schemas",
                "yaml.schemas",
                "spelling.languages",
                "files.exclude",
                "editor.rulers",
                "search.exclude",
                "editor.codeActionsOnSave",
            ]);
            v
        };
        let values = ["null", "true", "-5", "1e308", "\"\"", "\"zzz\"", "[]", "[1,\"a\",null]", "{}", "{\"a\":1}", "{\"[x]\":3,\"accent\":4}", "99999999999"];
        for key in keys {
            for v in values {
                let text = format!("{{ \"{key}\": {v} }}");
                let parsed = parse_layers(&text, &[]);
                assert!(parsed.is_ok(), "{text}");
            }
        }
        assert!(parse_layers("{ \"a\": ", &[]).is_err());
        assert!(parse_layers("[1]", &[]).is_err());
    }

    #[test]
    fn default_settings_match_the_template() {
        // Values in the template (without comments) produce no warnings and agree with the defaults.
        let (s, w) = parse_layers(TEMPLATE, &[]).unwrap();
        assert!(w.is_empty(), "{w:?}");
        let d = Settings::default();
        assert_eq!((s.graph_limit, s.timeline_limit, s.tree_indent, s.cursor_width), (d.graph_limit, d.timeline_limit, d.tree_indent, d.cursor_width));
        assert_eq!((s.startup, s.inline_blame, s.tab_close.as_str()), (d.startup, d.inline_blame, d.tab_close.as_str()));
    }
}

fn server<'a>(s: &'a Settings, key: &str) -> Option<&'a Option<crate::lsp::ServerSpec>> {
    s.lsp_servers.iter().rev().find(|(k, _)| k == key).map(|(_, o)| o)
}

/// `lsp.servers` entries carry `settings` (dotted keys expanded) and `initializationOptions`, with or without `command`.
#[test]
fn lsp_server_entries_carry_settings_and_initialization_options() {
    let (s, w) = parse(
        r#"{ "lsp.servers": {
            "rust": { "settings": { "rust-analyzer": { "check": { "command": "check" } }, "rust-analyzer.check.command": "clippy" },
                      "initializationOptions": { "cargo": { "features": ["foo"] } } },
            "go": { "command": "gopls", "args": ["serve"], "settings": { "gopls.buildFlags": ["-tags=integration"] } },
            "python": { "initializationOptions": "text" },
            "c": { "command": "clangd", "initializationOptions": [1, 2] },
            "java": null
        } }"#,
    );
    assert!(w.is_empty(), "{w:?}");
    let rust = server(&s, "rust").unwrap().as_ref().unwrap();
    assert_eq!(rust.command, None, "no command: the default server");
    assert_eq!(*rust.settings, json!({ "rust-analyzer": { "check": { "command": "clippy" } } }), "dotted after nested wins");
    assert_eq!(rust.init_options, Some(json!({ "cargo": { "features": ["foo"] } })));
    let go = server(&s, "go").unwrap().as_ref().unwrap();
    assert_eq!((go.command.as_deref(), go.args.as_slice()), (Some("gopls"), &["serve".to_string()][..]));
    assert_eq!(*go.settings, json!({ "gopls": { "buildFlags": ["-tags=integration"] } }));
    let python = server(&s, "python").unwrap().as_ref().unwrap();
    assert_eq!((&*python.settings, &python.init_options), (&Value::Null, &Some(json!("text"))), "any JSON is kept unchanged");
    assert_eq!(server(&s, "c").unwrap().as_ref().unwrap().init_options, Some(json!([1, 2])));
    assert_eq!(server(&s, "java"), Some(&None), "null still turns the language off");
    // A plain command entry, as before.
    let (s, _) = parse(r#"{ "lsp.servers": { "python": { "command": "pylsp" } } }"#);
    let p = server(&s, "python").unwrap().as_ref().unwrap();
    assert_eq!((p.command.as_deref(), &*p.settings, &p.init_options), (Some("pylsp"), &Value::Null, &None));
}

/// The project layer replaces the user's entry for a key (later wins), key by key.
#[test]
fn a_later_lsp_server_entry_wins() {
    let project = [(".codeyard/settings.jsonc", r#"{ "lsp.servers": { "rust": { "settings": { "b": 2 } } } }"#, true)];
    let (s, w) = parse_layers_trusted(r#"{ "lsp.servers": { "rust": { "settings": { "a": 1 } } } }"#, &project, true).unwrap();
    assert!(w.is_empty(), "{w:?}");
    assert_eq!(*server(&s, "rust").unwrap().as_ref().unwrap().settings, json!({ "b": 2 }));
}

/// AC7: an invalid `settings` value is named in the interface language; the rest of the entry still applies. An entry with
/// nothing to apply is rejected with the shape it should have.
#[test]
fn invalid_lsp_server_settings_are_warned_about_in_the_interface_language() {
    let text = r#"{ "lsp.servers": { "rust": { "command": "ra", "settings": 5 }, "go": { "args": ["x"] }, "c": { "command": 3 }, "lua": 7 } }"#;
    let (s, w) = parse(text);
    let rust = server(&s, "rust").unwrap().as_ref().unwrap();
    assert_eq!((rust.command.as_deref(), &*rust.settings), (Some("ra"), &Value::Null));
    assert!(server(&s, "go").is_none() && server(&s, "c").is_none() && server(&s, "lua").is_none());
    let shape = r#"{ "command": "…", "args": [], "settings": {} }"#;
    assert_eq!(
        w,
        vec![
            "settings.jsonc: lsp.servers.rust.settings: expected an object".to_string(),
            format!("settings.jsonc: lsp.servers.go: expected {shape} or null"),
            format!("settings.jsonc: lsp.servers.c: expected {shape} or null"),
            format!("settings.jsonc: lsp.servers.lua: expected {shape} or null"),
        ]
    );
    let _tr = crate::i18n::LangGuard::set("tr");
    let (_, w) = parse(text);
    assert_eq!(w[0], "settings.jsonc: lsp.servers.rust.settings: bir nesne bekleniyordu");
    assert_eq!(w[1], format!("settings.jsonc: lsp.servers.go: {shape} ya da null bekleniyordu"));
}
