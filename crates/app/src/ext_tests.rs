//! Installed declarative extensions reach every reader: themes, snippets, key bindings and keymap presets, languages,
//! schemas, settings and language servers; turned-off extensions and `--no-extensions` reach none (#41).

use std::path::{Path, PathBuf};

use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::ui::ActiveTheme;
use crate::workbench::Workbench;

fn config_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-ext-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Package `files` (path, text) as an extension and install it into the store of `cfg`.
fn install(cfg: &Path, files: &[(&str, &str)]) -> ext_core::Installed {
    let entries = files.iter().map(|(p, t)| ext_pkg::Entry { path: (*p).into(), data: t.as_bytes().to_vec(), executable: false }).collect();
    let (bytes, _) = ext_pkg::write(entries).expect("test package is valid");
    crate::ext::store(cfg).install(&bytes, ext_core::Source::File { path: "test.cyx".into() }).expect("installs")
}

fn manifest(id: &str, rest: &str) -> String {
    format!("[extension]\nid = \"{id}\"\nversion = \"1.0.0\"\nname = \"Test\"\nlicense = \"MIT\"\n{rest}")
}

fn setup<'a>(cx: &'a mut TestAppContext, cfg: &Path) -> (Entity<Workbench>, &'a mut VisualTestContext) {
    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(ActiveTheme(theme::DARK));
        cx.set_global(crate::settings::Settings { lsp_enabled: false, ..Default::default() });
    });
    let dir = cfg.to_path_buf();
    let (wb, cx) = cx.add_window_view(move |window, cx| Workbench::with_state(state::State::in_memory().ok(), Some(dir), window, cx));
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    (wb, cx)
}

fn reload(wb: &Entity<Workbench>, cx: &mut VisualTestContext) {
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
}

/// A binding's keys in GPUI's own spelling, to compare with `keys_of`.
fn keys(b: &gpui::KeyBinding) -> String {
    b.keystrokes().iter().map(|k| k.unparse()).collect::<Vec<_>>().join(" ")
}

fn keys_of(spec: &str) -> String {
    keys(&gpui::KeyBinding::load(spec, Box::new(gpui::NoAction), None, false, None, &gpui::DummyKeyboardMapper).unwrap())
}

const THEME: &str = r##"{"name":"x","colors":{"editor.background":"#101418","editor.foreground":"#d0d6dc"}}"##;

#[gpui::test]
fn an_installed_theme_is_offered_and_applies(cx: &mut TestAppContext) {
    let cfg = config_dir("theme");
    install(
        &cfg,
        &[
            ("codeyard-extension.toml", &manifest("you.night", "[[themes]]\nlabel = \"Night Owl\"\npath = \"themes/night.json\"\n")),
            ("themes/night.json", THEME),
        ],
    );
    let (wb, cx) = setup(cx, &cfg);
    let themes = wb.read_with(cx, |wb, _| wb.themes_for_test());
    assert!(themes.contains(&("ext:you.night/night".into(), "Night Owl".into())), "{themes:?}");
    std::fs::write(cfg.join("settings.jsonc"), r#"{ "theme": "ext:you.night/night" }"#).unwrap();
    reload(&wb, cx);
    assert_eq!(cx.read_global(|t: &ActiveTheme, _| t.0).bg, theme::themes::parse_color("#101418", theme::DARK.bg).unwrap());
}

#[gpui::test]
fn turning_an_extension_off_removes_its_contributions(cx: &mut TestAppContext) {
    let cfg = config_dir("off");
    let i = install(&cfg, &[("codeyard-extension.toml", &manifest("you.night", "[[themes]]\nlabel = \"Night\"\npath = \"t.json\"\n")), ("t.json", THEME)]);
    let (wb, cx) = setup(cx, &cfg);
    assert!(wb.read_with(cx, |wb, _| wb.themes_for_test()).iter().any(|(id, _)| id == "ext:you.night/t"));
    crate::ext::store(&cfg).set_disabled(&i.manifest.id, true).unwrap();
    reload(&wb, cx);
    assert!(!wb.read_with(cx, |wb, _| wb.themes_for_test()).iter().any(|(id, _)| id.starts_with("ext:")));
}

#[test]
fn safe_mode_loads_nothing() {
    let cfg = config_dir("safe");
    install(&cfg, &[("codeyard-extension.toml", &manifest("you.night", "[[themes]]\nlabel = \"Night\"\npath = \"t.json\"\n")), ("t.json", THEME)]);
    crate::ext::reload(Some(&cfg));
    assert_eq!(crate::ext::current().active.len(), 1);
    crate::ext::turn_off();
    crate::ext::reload(Some(&cfg));
    assert!(crate::ext::current().active.is_empty());
}

#[gpui::test]
fn extension_snippets_follow_the_users_own(cx: &mut TestAppContext) {
    let _guard = crate::snippets::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let cfg = config_dir("snippets");
    install(
        &cfg,
        &[
            ("codeyard-extension.toml", &manifest("you.snips", "[[snippets]]\nlanguage = \"rust\"\npath = \"s/rust.json\"\n")),
            ("s/rust.json", r##"{ "Test": { "prefix": "tst", "body": "#[test]\nfn $1() {}", "description": "A test" } }"##),
        ],
    );
    let (wb, cx) = setup(cx, &cfg);
    reload(&wb, cx);
    let found = crate::snippets::for_path("/p/a.rs");
    let s = found.iter().find(|s| s.prefix == "tst").expect("extension snippet");
    assert_eq!(s.origin.as_deref(), Some("you.snips"));
}

#[gpui::test]
fn extension_key_bindings_never_take_a_built_in_key(cx: &mut TestAppContext) {
    let cfg = config_dir("keys");
    install(
        &cfg,
        &[(
            "codeyard-extension.toml",
            &manifest(
                "you.keys",
                // cmd-shift-alt-z is free; cmd-s is Save.
                "[[keybindings]]\nkey = \"cmd-shift-alt-z\"\ncommand = \"core:workbench.toggleZenMode\"\nwhen = \"editor && editor.language == 'rust'\"\n[[keybindings]]\nkey = \"cmd-s\"\ncommand = \"core:workbench.toggleZenMode\"\n",
            ),
        )],
    );
    let (wb, cx) = setup(cx, &cfg);
    std::fs::write(cfg.join("keybindings.jsonc"), "[]").unwrap();
    reload(&wb, cx);
    let notice = wb.read_with(cx, |wb, _| wb.notice_for_test()).map(|(_, t)| t).unwrap_or_default();
    assert!(notice.contains("“cmd-s” is a built-in shortcut"), "{notice}");
    let want = keys_of("cmd-shift-alt-z");
    let bound = cx.update(|_, cx| cx.key_bindings().borrow().bindings().any(|b| keys(b) == want));
    assert!(bound, "the free key is bound");
}

#[test]
fn when_expressions_become_gpui_predicates() {
    for (src, want) in [
        ("editor", "Editor"),
        ("!terminal", "!(Terminal)"),
        ("editor && editor.language == 'rust'", "(Editor && language == rust)"),
        ("file-tree || scm", "(FileTree || ScmPanel)"),
        ("editor.language != \"go\"", "language != go"),
    ] {
        let got = crate::ext::gpui_when(src).unwrap();
        assert_eq!(got, want);
        gpui::KeyBindingContextPredicate::parse(&got).unwrap_or_else(|e| panic!("{got}: {e}"));
    }
    for bad in ["workspace.trusted", "ext.ready", "panel", "true", "editor.language == 'objective-c'", "editor == 'x'"] {
        assert!(crate::ext::gpui_when(bad).is_err(), "{bad}");
    }
}

#[gpui::test]
fn a_chosen_keymap_preset_may_change_built_in_keys(cx: &mut TestAppContext) {
    let cfg = config_dir("preset");
    install(
        &cfg,
        &[
            ("codeyard-extension.toml", &manifest("you.emacs", "[[keymap-preset]]\nlabel = \"Emacs\"\npath = \"emacs.json\"\n")),
            ("emacs.json", r#"[ { "key": "cmd-s", "command": "workbench.toggleZenMode" } ]"#),
        ],
    );
    let (wb, cx) = setup(cx, &cfg);
    let save = keys_of("cmd-s");
    let zen_on_cmd_s = |cx: &mut VisualTestContext| {
        cx.update(|_, cx| cx.key_bindings().borrow().bindings().any(|b| keys(b) == save && b.action().name().ends_with("ToggleZenMode")))
    };
    assert!(!zen_on_cmd_s(cx), "not chosen: no effect");
    std::fs::write(cfg.join("settings.jsonc"), r#"{ "keymap.preset": "you.emacs/Emacs" }"#).unwrap();
    std::fs::write(cfg.join("keybindings.jsonc"), "[]").unwrap();
    reload(&wb, cx);
    assert!(zen_on_cmd_s(cx));
    std::fs::write(cfg.join("settings.jsonc"), r#"{ "keymap.preset": "you.vim/Vim" }"#).unwrap();
    reload(&wb, cx);
    let notice = wb.read_with(cx, |wb, _| wb.notice_for_test()).map(|(_, t)| t).unwrap_or_default();
    assert!(notice.contains("“you.vim/Vim” is not installed"), "{notice}");
}

#[test]
fn extension_languages_fill_gaps_only() {
    let cfg = config_dir("lang");
    install(
        &cfg,
        &[(
            "codeyard-extension.toml",
            &manifest(
                "you.just",
                "[[languages]]\nid = \"just\"\nfilenames = [\"Justfile\"]\nextensions = [\"just\"]\ngrammar = \"makefile\"\nline-comment = \"#\"\n[[languages]]\nid = \"myrust\"\nextensions = [\"rs\"]\nline-comment = \"%%\"\n",
            ),
        )],
    );
    crate::ext::reload(Some(&cfg));
    assert_eq!(crate::snippets::lang_of_path("/p/Justfile"), "just");
    assert_eq!(crate::snippets::lang_of_path("/p/x.JUST"), "just");
    assert_eq!(crate::lsp::language_of("/p/x.just"), Some(("just", "just")));
    assert_eq!(crate::editor::comment_token_for_test("/p/Justfile"), Some("#"));
    assert!(syntax::language_for_path(&crate::editor::syntax_path("/p/Justfile")).is_some(), "highlighted with the make grammar");
    assert!(crate::editor::grammar_known("makefile") && !crate::editor::grammar_known("cobol-85"));
    // A built-in language is never replaced.
    assert_eq!(crate::snippets::lang_of_path("/p/a.rs"), "rust");
    assert_eq!(crate::editor::comment_token_for_test("/p/a.rs"), Some("//"));
    assert_eq!(crate::lsp::language_of("/p/a.rs"), Some(("rust", "rust")));
    crate::ext::set_for_test(Vec::new());
}

#[test]
fn language_servers_and_schemas_join_the_users_settings() {
    let cfg = config_dir("lsp");
    install(
        &cfg,
        &[
            (
                "codeyard-extension.toml",
                &manifest(
                    "you.taplo",
                    "[permissions]\nrun = [\"taplo\"]\nwhy = \"Starts taplo.\"\n[[language-servers]]\ncommand = \"taplo\"\nargs = [\"lsp\", \"stdio\"]\nlanguages = [\"toml\", \"typescriptreact\"]\n[[json-schemas]]\nfile-match = [\"vercel.json\"]\npath = \"schemas/vercel.json\"\n",
                ),
            ),
            ("schemas/vercel.json", "{}"),
        ],
    );
    crate::ext::reload(Some(&cfg));
    let (servers, warnings) = crate::ext::server_entries();
    assert!(warnings.is_empty(), "{warnings:?}");
    let keys: Vec<&str> = servers.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, ["toml", "typescript"], "a language id maps to its server key");
    // The user's own entry for the same key comes later and wins.
    let mut all = servers.clone();
    all.push(("toml".into(), Some(crate::lsp::ServerSpec::from(("my-taplo".to_string(), vec![])))));
    assert_eq!(crate::lsp::server_for("toml", &all).unwrap().command, "my-taplo");
    assert_eq!(crate::lsp::server_for("toml", &servers).unwrap().command, "taplo");
    let schemas = crate::ext::schema_entries();
    assert_eq!(schemas.len(), 1);
    assert!(Path::new(&schemas[0].1).is_absolute() && schemas[0].1.ends_with("vercel.json"));
    assert!(crate::ext::package_dirs().iter().any(|d| Path::new(&schemas[0].1).starts_with(d)));
    crate::ext::set_for_test(Vec::new());
}

#[test]
fn declared_settings_are_checked_and_others_warned() {
    let cfg = config_dir("settings");
    install(
        &cfg,
        &[(
            "codeyard-extension.toml",
            &manifest(
                "you.deploy",
                "[settings.interval]\ntype = \"number\"\ndefault = 60\ndescription = \"Seconds.\"\n[settings.mode]\ntype = \"string\"\nenum = [\"fast\", \"safe\"]\ndescription = \"Mode.\"\n",
            ),
        )],
    );
    crate::ext::reload(Some(&cfg));
    let (s, w) = crate::settings::parse_layers(r#"{ "ext.you.deploy.interval": 30, "ext.you.deploy.mode": "safe" }"#, &[]).unwrap();
    assert!(w.is_empty(), "{w:?}");
    assert_eq!(s.ext_settings["ext.you.deploy.interval"], serde_json::json!(30));
    let (s, w) = crate::settings::parse_layers(r#"{ "ext.you.deploy.interval": "30", "ext.you.deploy.mode": "slow", "ext.other.x.y": 1 }"#, &[]).unwrap();
    assert!(s.ext_settings.is_empty());
    assert_eq!(w.len(), 3, "{w:?}");
    crate::ext::set_for_test(Vec::new());
}

#[gpui::test]
fn a_dev_folder_loads_reloads_and_reports_its_errors(cx: &mut TestAppContext) {
    let cfg = config_dir("dev");
    let dev = cfg.join("my-theme");
    std::fs::create_dir_all(dev.join("themes")).unwrap();
    std::fs::write(dev.join("codeyard-extension.toml"), manifest("you.mine", "[[themes]]\nlabel = \"Mine\"\npath = \"themes/mine.json\"\n")).unwrap();
    std::fs::write(dev.join("themes/mine.json"), THEME).unwrap();
    crate::ext::set_dev(Some(dev.clone()));
    let (wb, cx) = setup(cx, &cfg);
    assert!(wb.read_with(cx, |wb, _| wb.themes_for_test()).iter().any(|(id, name)| id == "ext:you.mine/mine" && name == "Mine"));
    // Saving a file changes the fingerprint the settings poll watches.
    let before = crate::ext::dev_stamp();
    std::fs::write(dev.join("codeyard-extension.toml"), manifest("you.mine", "[[themes]]\nlabel = \"Mine Too\"\npath = \"themes/mine.json\"\n") + "\n")
        .unwrap();
    assert_ne!(crate::ext::dev_stamp(), before);
    reload(&wb, cx);
    assert!(wb.read_with(cx, |wb, _| wb.themes_for_test()).iter().any(|(_, name)| name == "Mine Too"));
    // A mistake shows the author's error instead of the extension.
    std::fs::remove_file(dev.join("themes/mine.json")).unwrap();
    reload(&wb, cx);
    let notice = wb.read_with(cx, |wb, _| wb.notice_for_test()).map(|(_, t)| t).unwrap_or_default();
    assert!(notice.contains("themes.path points to 'themes/mine.json'"), "{notice}");
    crate::ext::set_dev(None);
}

#[cfg(unix)]
#[test]
fn cyx_is_linked_into_the_first_writable_folder() {
    let root = config_dir("cyx-link");
    let target = root.join("Codeyard.app-cyx");
    std::fs::write(&target, "#!/bin/sh\n").unwrap();
    let blocked = root.join("blocked");
    std::fs::write(&blocked, "a file, not a folder").unwrap();
    let bin = root.join("bin");
    let dir = Workbench::install_cyx_link_for_test(&target, &[blocked.clone(), bin.clone()]).unwrap();
    assert_eq!(dir, bin);
    assert_eq!(std::fs::read_link(bin.join("cyx")).unwrap(), target);
    // Again: the old link is replaced.
    Workbench::install_cyx_link_for_test(&target, std::slice::from_ref(&bin)).unwrap();
    // A real file of that name is never replaced.
    std::fs::remove_file(bin.join("cyx")).unwrap();
    std::fs::write(bin.join("cyx"), "someone else's").unwrap();
    assert!(Workbench::install_cyx_link_for_test(&target, std::slice::from_ref(&bin)).unwrap_err().contains("not a link"));
}

#[cfg(unix)]
#[gpui::test]
fn a_new_extension_is_loaded_in_development_and_opened(cx: &mut TestAppContext) {
    let cfg = config_dir("new-ext");
    let dir = cfg.join("glow");
    std::fs::create_dir_all(dir.join("themes")).unwrap();
    std::fs::write(dir.join("codeyard-extension.toml"), manifest("you.glow", "[[themes]]\nlabel = \"Glow\"\npath = \"themes/glow.json\"\n")).unwrap();
    std::fs::write(dir.join("themes/glow.json"), THEME).unwrap();
    let (wb, cx) = setup(cx, &cfg);
    wb.update_in(cx, |wb, window, cx| wb.finish_create_extension_for_test(true, "", dir.clone(), window, cx));
    cx.run_until_parked();
    assert_eq!(crate::ext::dev_dir().as_deref(), Some(dir.as_path()));
    assert!(wb.read_with(cx, |wb, _| wb.themes_for_test()).iter().any(|(id, _)| id == "ext:you.glow/glow"));
    assert!(wb.read_with(cx, |wb, cx| wb.active_editor_entity().map(|e| e.read(cx).path.ends_with("codeyard-extension.toml"))).unwrap_or(false));
    wb.update_in(cx, |wb, window, cx| wb.finish_create_extension_for_test(false, "error: glow/ already exists", dir.clone(), window, cx));
    let notice = wb.read_with(cx, |wb, _| wb.notice_for_test()).map(|(_, t)| t).unwrap_or_default();
    assert!(notice.contains("already exists"), "{notice}");
    crate::ext::set_dev(None);
}
