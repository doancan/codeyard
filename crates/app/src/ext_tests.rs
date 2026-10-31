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
                // cmd-shift-alt-z is free; secondary-s is Save on every platform (cmd on macOS, ctrl elsewhere).
                "[[keybindings]]\nkey = \"cmd-shift-alt-z\"\ncommand = \"core:workbench.toggleZenMode\"\nwhen = \"editor && editor.language == 'rust'\"\n[[keybindings]]\nkey = \"secondary-s\"\ncommand = \"core:workbench.toggleZenMode\"\n",
            ),
        )],
    );
    let (wb, cx) = setup(cx, &cfg);
    std::fs::write(cfg.join("keybindings.jsonc"), "[]").unwrap();
    reload(&wb, cx);
    let notice = wb.read_with(cx, |wb, _| wb.notice_for_test()).map(|(_, t)| t).unwrap_or_default();
    assert!(notice.contains("“secondary-s” is a built-in shortcut"), "{notice}");
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
            ("emacs.json", r#"[ { "key": "secondary-s", "command": "workbench.toggleZenMode" } ]"#),
        ],
    );
    let (wb, cx) = setup(cx, &cfg);
    let save = keys_of("secondary-s");
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
    let servers = crate::ext::server_entries(Some(&cfg), &[]);
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

#[test]
fn install_file_checks_like_the_view() {
    let cfg = config_dir("install-file");
    let pkg = |manifest_text: String, extra: Vec<(&str, &str)>| {
        let mut entries = vec![ext_pkg::Entry { path: "codeyard-extension.toml".into(), data: manifest_text.into_bytes(), executable: false }];
        entries.extend(extra.into_iter().map(|(p, t)| ext_pkg::Entry { path: p.into(), data: t.as_bytes().to_vec(), executable: false }));
        ext_pkg::write(entries).unwrap().0
    };
    let store = crate::ext::store(&cfg);
    let good = cfg.join("good.cyx");
    std::fs::write(&good, pkg(manifest("you.night", "[[themes]]\nlabel = \"Night\"\npath = \"t.json\"\n"), vec![("t.json", THEME)])).unwrap();
    let m = crate::ext::install_file(&store, &good).unwrap();
    assert_eq!(m.id.as_str(), "you.night");
    assert!(store.locked().unwrap().contains_key(&m.id));
    let code = cfg.join("code.cyx");
    let mut code_pkg = vec![
        ext_pkg::Entry {
            path: "codeyard-extension.toml".into(),
            data: manifest("you.code", "[code]\nentry = \"main.ts\"\n[[commands]]\nid = \"go\"\ntitle = \"Go\"\n").into_bytes(),
            executable: false,
        },
        ext_pkg::Entry { path: "main.ts".into(), data: vec![], executable: false },
    ];
    code_pkg.push(ext_pkg::Entry { path: ext_pkg::COMPONENT.into(), data: ext_pkg::testing::COMPONENT.to_vec(), executable: false });
    std::fs::write(&code, ext_pkg::write(code_pkg).unwrap().0).unwrap();
    // Code without permissions installs (it runs in codeyard-exthost); with permissions it needs the consent sheet.
    if crate::ext_runtime::code_runs_here(&"tests").is_ok() {
        assert_eq!(crate::ext::install_file(&store, &code).unwrap().id.as_str(), "you.code");
    }
    let asks = cfg.join("asks.cyx");
    let manifest_text =
        manifest("you.asks", "[code]\nentry = \"main.ts\"\n[[commands]]\nid = \"go\"\ntitle = \"Go\"\n[permissions]\nnet = [\"api.example.com\"]\n");
    let asks_pkg = vec![
        ext_pkg::Entry { path: "codeyard-extension.toml".into(), data: manifest_text.into_bytes(), executable: false },
        ext_pkg::Entry { path: "main.ts".into(), data: vec![], executable: false },
        ext_pkg::Entry { path: ext_pkg::COMPONENT.into(), data: ext_pkg::testing::COMPONENT.to_vec(), executable: false },
    ];
    std::fs::write(&asks, ext_pkg::write(asks_pkg).unwrap().0).unwrap();
    assert!(crate::ext::install_file(&store, &asks).unwrap_err().contains("asks for permissions"));
    let junk = cfg.join("junk.cyx");
    std::fs::write(&junk, b"not a package").unwrap();
    assert!(crate::ext::install_file(&store, &junk).unwrap_err().contains("no gzip header"));
}

/// Local and SSH are the same (#35, exit criterion 5): over SSH a file in an extension's language gets that language, and
/// the extension's language server starts on the host, found on PATH there, and is told the language id.
#[gpui::test]
fn extension_language_and_server_work_over_ssh(cx: &mut TestAppContext) {
    let Some(_sshd) = crate::test_sshd::start("ext-ssh") else { return };
    let cfg = config_dir("ssh");
    let Some(fake) = crate::test_lsp::FakeServer::new(&cfg.join("server"), &serde_json::json!({})) else { return };
    let (program, args) = fake.command();
    let args = args.iter().map(|a| format!("{a:?}")).collect::<Vec<_>>().join(", ");
    install(
        &cfg,
        &[(
            "codeyard-extension.toml",
            &manifest(
                "you.foo",
                &format!(
                    "[permissions]\nrun = [\"{program}\"]\nwhy = \"Starts the Foo language server.\"\n[[languages]]\nid = \"foo\"\nextensions = [\"foo\"]\nline-comment = \"#\"\n[[language-servers]]\ncommand = \"{program}\"\nargs = [{args}]\nlanguages = [\"foo\"]\n"
                ),
            ),
        )],
    );
    let root = cfg.join("project");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.foo"), "# hello\n").unwrap();
    let (wb, cx) = setup(cx, &cfg);
    assert!(cx.update(|_, cx| cx.global::<crate::settings::Settings>().lsp_servers.iter().any(|(k, _)| k == "foo")), "the server joins lsp.servers");
    wb.update_in(cx, |wb, window, cx| wb.connect(crate::test_sshd::ALIAS.into(), window, cx));
    crate::qa_integration_tests::wait_secs(cx, 30, "connected", |cx| {
        cx.executor().advance_clock(std::time::Duration::from_secs(1));
        wb.read_with(cx, |wb, _| wb.ports_ready())
    });
    let r = root.to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_path(&r, window, cx));
    cx.run_until_parked();
    crate::qa_integration_tests::ssh_open_file(&wb, cx, &root.join("a.foo"));
    crate::qa_integration_tests::wait_secs(cx, 30, "the extension's server opened a.foo on the host", |cx| {
        cx.executor().advance_clock(std::time::Duration::from_millis(500));
        !fake.received("textDocument/didOpen").is_empty()
    });
    let open = &fake.received("textDocument/didOpen")[0];
    assert_eq!(open["params"]["textDocument"]["languageId"], "foo");
    assert!(open["params"]["textDocument"]["uri"].as_str().unwrap_or_default().ends_with("/a.foo"), "{open}");
    crate::ext::set_for_test(Vec::new());
}

/// `--install-extension FOLDER`: the folder is packaged as it is, without building, with the same checks as a file. The
/// performance scenarios' decoration provider (scripts/perf/run.sh) installs this way: code with no permissions.
#[test]
fn a_built_folder_installs_like_a_package() {
    let cfg = config_dir("folder");
    let dir = cfg.join("src-decorations");
    std::fs::create_dir_all(&dir).unwrap();
    let code = "[code]\nentry = \"Cargo.toml\"\n[[commands]]\nid = \"decorate\"\ntitle = \"Decorate\"\n[[decorations]]\nid = \"lines\"\nlanguages = [\"rust\"]\n[[tasks]]\nid = \"noop\"\n";
    std::fs::write(dir.join(ext_manifest::FILE_NAME), manifest("perf.decorations", code)).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "").unwrap();
    std::fs::write(dir.join(ext_pkg::COMPONENT), include_bytes!("../../ext-host/testdata/decorator.wasm")).unwrap();
    let store = crate::ext::store(&cfg);
    let m = crate::ext::install_file(&store, &dir).expect("installs");
    assert_eq!(m.id.to_string(), "perf.decorations");
    // Code that asks for a permission needs the consent sheet, from a folder as from a file.
    std::fs::write(dir.join(ext_manifest::FILE_NAME), manifest("perf.decorations", &format!("[permissions]\nread = [\"a.txt\"]\n{code}"))).unwrap();
    let e = crate::ext::install_file(&store, &dir).unwrap_err();
    assert!(e.contains("asks for permissions"), "{e}");
}
