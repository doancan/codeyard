use std::path::{Path, PathBuf};

use crate::*;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cyx-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn every_template_passes_check_and_packages() {
    let root = tmp("templates");
    for (kind, name) in [(Kind::Theme, "night"), (Kind::Snippets, "snips"), (Kind::Keymap, "emacs"), (Kind::Language, "just"), (Kind::LanguageServer, "taplo")]
    {
        let dir = root.join(name);
        scaffold::create(&dir, kind, name, "you").unwrap();
        let report = check(&dir).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(report.id, format!("you.{name}"));
        assert!(report.warnings.is_empty(), "{name}: {:?}", report.warnings);
        let (file, _) = package(&dir, Some(&root.join(format!("{name}.cyx")))).unwrap();
        let pkg = ext_pkg::read(&std::fs::read(file).unwrap()).unwrap();
        assert_eq!(pkg.manifest.id.as_str(), format!("you.{name}"));
        // The publish workflow is written but never packaged.
        assert!(dir.join(".github/workflows/publish.yml").is_file());
        assert!(pkg.files.keys().all(|p| !p.starts_with(".github")), "{:?}", pkg.files.keys());
    }
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn packaging_twice_gives_the_same_bytes() {
    let root = tmp("repro");
    let dir = root.join("night");
    scaffold::create(&dir, Kind::Theme, "night", "you").unwrap();
    let (a, da) = package(&dir, Some(&root.join("a.cyx"))).unwrap();
    let (b, db) = package(&dir, Some(&root.join("b.cyx"))).unwrap();
    assert_eq!(std::fs::read(a).unwrap(), std::fs::read(b).unwrap());
    assert_eq!(da, db);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn check_runs_the_import_check_on_the_component() {
    let dir = tmp("component");
    let manifest = "[extension]\nid = \"you.code\"\nversion = \"0.1.0\"\nname = \"Code\"\nlicense = \"MIT\"\n[code]\nentry = \"Cargo.toml\"\n[[commands]]\nid = \"go\"\ntitle = \"Go\"\n";
    std::fs::write(dir.join(ext_manifest::FILE_NAME), manifest).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "").unwrap();
    let e = check(&dir).unwrap_err();
    assert!(e.contains("error[CY0700]: the manifest has [code] but there is no extension.wasm"), "{e}");
    assert!(e.contains("= fix: run `cyx package` or `cyx test` first"), "{e}");
    std::fs::write(dir.join(ext_pkg::COMPONENT), ext_pkg::testing::COMPONENT).unwrap();
    let report = check(&dir).unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    // The minimum Codeyard comes from the API items the component uses (#147).
    let c = ext_pkg::check_component(ext_pkg::testing::COMPONENT, &build::manifest(&dir).unwrap()).unwrap();
    assert_eq!(report.requires, Some(requires(&c)));
    assert_eq!(requires_on(&c, "0.7.0"), "It needs Codeyard 0.7.0 or newer (API 1.0.0, from the API items its code uses).");
    // A build before that release (the main branch while the workspace is still 0.6.0) runs it: it must not say
    // otherwise (e2e trial).
    assert_eq!(
        requires_on(&c, "0.6.0"),
        "It needs Codeyard 0.7.0 or newer (API 1.0.0, from the API items its code uses). 0.7.0 is not released yet; this preview build (0.6.0) runs it."
    );
    // A 0.1 build still passes, with a warning that says to rebuild.
    std::fs::write(dir.join(ext_pkg::COMPONENT), ext_pkg::testing::COMPONENT_0_1).unwrap();
    let report = check(&dir).unwrap();
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(report.warnings[0].starts_with("warning: extension.wasm was built against API 0.1.0;"), "{}", report.warnings[0]);
    assert!(report.warnings[0].contains("migrating-to-1.0.md"), "{}", report.warnings[0]);
    let moving = format!("Codeyard still runs it while extensions move to API {}", ext_api::VERSION);
    assert!(report.warnings[0].contains(&moving), "{}", report.warnings[0]);
    std::fs::write(dir.join(ext_pkg::COMPONENT), b"\0asm\x01\0\0\0").unwrap();
    let e = check(&dir).unwrap_err();
    assert!(e.contains("error[CY0700]: extension.wasm is a core WebAssembly module, not a component"), "{e}");
    assert!(e.contains("= fix: build for the wasm32-wasip2 target"), "{e}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// `--no-build` packages the component already in the folder: no toolchain runs (the perf scenario's code extensions).
#[test]
fn package_no_build_takes_the_component_that_is_there() {
    let root = tmp("no-build");
    let dir = root.join("code");
    std::fs::create_dir_all(&dir).unwrap();
    let manifest = "[extension]\nid = \"you.code\"\nversion = \"0.1.0\"\nname = \"Code\"\nlicense = \"MIT\"\n[code]\nentry = \"Cargo.toml\"\n[[commands]]\nid = \"go\"\ntitle = \"Go\"\n";
    std::fs::write(dir.join(ext_manifest::FILE_NAME), manifest).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "not a crate: nothing builds it").unwrap();
    std::fs::write(dir.join(ext_pkg::COMPONENT), ext_pkg::testing::COMPONENT).unwrap();
    let out = root.join("code.cyx");
    let args: Vec<String> = ["package", dir.to_str().unwrap(), "--no-build", "-o", out.to_str().unwrap()].iter().map(|s| s.to_string()).collect();
    run(&args).unwrap();
    let pkg = ext_pkg::read(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!(pkg.manifest.id.as_str(), "you.code");
    assert!(pkg.files.contains_key(ext_pkg::COMPONENT));
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn check_errors_name_the_line_and_the_fix() {
    let root = tmp("errors");
    let dir = root.join("lsp");
    scaffold::create(&dir, Kind::LanguageServer, "lsp", "you").unwrap();
    let path = dir.join(ext_manifest::FILE_NAME);
    let text = std::fs::read_to_string(&path).unwrap().replace("run = [\"taplo\"]\n", "");
    std::fs::write(&path, text).unwrap();
    let e = check(&dir).unwrap_err();
    assert!(e.contains("error[CY0201]: run access to \"taplo\" is not declared"), "{e}");
    assert!(e.contains("--> codeyard-extension.toml:"), "{e}");
    assert!(e.contains("= fix: add to [permissions]:  run = [\"taplo\"]"), "{e}");
    assert!(e.ends_with("1 problem found"), "{e}");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_missing_why_is_a_warning_in_dev_and_an_error_to_package() {
    let root = tmp("why");
    let dir = root.join("lsp");
    scaffold::create(&dir, Kind::LanguageServer, "lsp", "you").unwrap();
    let path = dir.join(ext_manifest::FILE_NAME);
    let text = std::fs::read_to_string(&path).unwrap().lines().filter(|l| !l.starts_with("why")).collect::<Vec<_>>().join("\n");
    std::fs::write(&path, text).unwrap();
    let dev = check_dev(&dir).unwrap();
    assert!(dev.warnings[0].starts_with("warning: these permissions are high risk"), "{:?}", dev.warnings);
    assert!(check(&dir).unwrap_err().contains("CY0202"));
    assert!(package(&dir, Some(&root.join("x.cyx"))).is_err());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn an_unknown_core_command_is_a_warning() {
    let root = tmp("core");
    let dir = root.join("lsp");
    scaffold::create(&dir, Kind::LanguageServer, "lsp", "you").unwrap();
    let path = dir.join(ext_manifest::FILE_NAME);
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("run = [\"taplo\"]\n", "run = [\"taplo\"]\ncommands = [\"core:workbench.saveAll\", \"core:workbench.quit\"]\n");
    std::fs::write(&path, text).unwrap();
    let report = check(&dir).unwrap();
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(report.warnings[0].starts_with("warning: core:workbench.quit is not a core command"), "{:?}", report.warnings);
    assert!(report.warnings[0].contains("reference/core-commands.md"), "{:?}", report.warnings);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_missing_file_is_named() {
    let root = tmp("missing");
    let dir = root.join("night");
    scaffold::create(&dir, Kind::Theme, "night", "you").unwrap();
    std::fs::remove_file(dir.join("themes/night.json")).unwrap();
    let e = check(&dir).unwrap_err();
    assert_eq!(e, "error: themes.path points to 'themes/night.json', which is not in the package");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn new_refuses_bad_names_and_existing_folders() {
    let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert!(parse_new(&s(&["theme", "My Theme"])).unwrap_err().contains("lowercase"));
    assert!(parse_new(&s(&["widget", "x"])).unwrap_err().contains("unknown kind"));
    assert!(parse_new(&s(&["theme", "x", "--publisher", "Bad Name"])).unwrap_err().contains("publisher"));
    let (kind, name, publisher, _, _) = parse_new(&s(&["theme", "zzz-not-here", "--publisher", "acme"])).unwrap();
    assert_eq!((kind, name.as_str(), publisher.as_str()), (Kind::Theme, "zzz-not-here", "acme"));
    let (kind, .., lang) = parse_new(&s(&["view", "zzz-not-here", "--lang", "ts"])).unwrap();
    assert_eq!((kind, lang), (Kind::View, scaffold::Lang::TypeScript));
    assert!(parse_new(&s(&["theme", "x", "--lang", "ts"])).unwrap_err().contains("code extensions"));
    assert!(parse_new(&s(&["command", "x", "--lang", "go"])).unwrap_err().contains("unknown language"));
}

#[test]
fn slugs_are_valid_ids() {
    assert_eq!(slug("Doğancan Kalıpçılar").as_deref(), Some("dogancan-kalipcilar"));
    assert_eq!(slug("  Acme, Inc. ").as_deref(), Some("acme-inc"));
    assert_eq!(slug("42 things").as_deref(), Some("things"));
    assert_eq!(slug("***"), None);
}

fn vscode_extension(root: &Path) -> PathBuf {
    let src = root.join("vs");
    std::fs::create_dir_all(src.join("themes")).unwrap();
    std::fs::create_dir_all(src.join("snippets")).unwrap();
    std::fs::create_dir_all(src.join("schemas")).unwrap();
    std::fs::write(
        src.join("package.json"),
        r#"{
          // comments and trailing commas, as VS Code allows
          "name": "night-owl", "displayName": "Night Owl", "publisher": "sdras", "version": "2.0.1", "license": "MIT",
          "contributes": {
            "themes": [{ "label": "Night Owl", "uiTheme": "vs-dark", "path": "./themes/Night Owl-color-theme.json" }],
            "snippets": [{ "language": "javascript", "path": "./snippets/js.json" }],
            "languages": [{ "id": "owl", "extensions": [".owl"], "configuration": "./owl-language.json" }],
            "grammars": [{ "language": "owl", "scopeName": "source.owl", "path": "./owl.tmLanguage.json" }],
            "jsonValidation": [
              { "fileMatch": "owl.json", "url": "./schemas/owl.json" },
              { "fileMatch": "x.json", "url": "https://example.com/x.json" },
            ],
            "keybindings": [{ "key": "ctrl+k", "command": "owl.go" }],
          },
          "main": "./out/extension.js",
        }"#,
    )
    .unwrap();
    std::fs::write(
        src.join("themes/Night Owl-color-theme.json"),
        r##"{"name":"Night Owl","colors":{"editor.background":"#011627","editor.foreground":"#d6deeb"}}"##,
    )
    .unwrap();
    std::fs::write(src.join("snippets/js.json"), r#"{"Log":{"prefix":"log","body":"console.log($1)"}}"#).unwrap();
    std::fs::write(src.join("owl-language.json"), r#"{ "comments": { "lineComment": "//", "blockComment": ["/*", "*/"] } }"#).unwrap();
    std::fs::write(src.join("schemas/owl.json"), "{}").unwrap();
    src
}

#[test]
fn a_vscode_extension_converts_what_it_can_and_says_what_not() {
    let root = tmp("import");
    let src = vscode_extension(&root);
    let out = root.join("owl-codeyard");
    let r = vscode::import(&src, Some(&out)).unwrap();
    assert_eq!(r.converted, ["theme “Night Owl”", "javascript snippets", "language “owl”", "JSON schema for owl.json"]);
    let skipped = r.skipped.join("\n");
    for word in ["TextMate grammars", "remote schemas", "key bindings", "extension code"] {
        assert!(skipped.contains(word), "{word}: {skipped}");
    }
    assert!(!skipped.contains("needs a fix"), "{skipped}");
    let report = check(&out).unwrap();
    assert_eq!((report.id.as_str(), report.version.as_str()), ("sdras.night-owl", "2.0.1"));
    let m = ext_manifest::parse(&std::fs::read_to_string(out.join(ext_manifest::FILE_NAME)).unwrap()).unwrap();
    assert_eq!(m.contributions.languages[0].block_comment, Some(("/*".into(), "*/".into())));
    assert_eq!(m.contributions.themes[0].path, "themes/Night Owl-color-theme.json");
    // A second import does not overwrite the first.
    assert!(vscode::import(&src, Some(&out)).unwrap_err().contains("already exists"));
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_vsix_is_unpacked() {
    let root = tmp("vsix");
    let src = vscode_extension(&root);
    // A .vsix is a zip whose extension/ folder holds package.json.
    let stage = root.join("stage");
    std::fs::create_dir_all(&stage).unwrap();
    std::fs::rename(&src, stage.join("extension")).unwrap();
    let vsix = root.join("owl.vsix");
    let zipped = std::process::Command::new("zip").arg("-qr").arg(&vsix).arg("extension").current_dir(&stage).status();
    if !zipped.is_ok_and(|s| s.success()) {
        eprintln!("skipping: no zip tool");
        return;
    }
    let r = vscode::import(&vsix, Some(&root.join("out"))).unwrap();
    assert!(r.converted.contains(&"theme “Night Owl”".to_string()));
    std::fs::remove_dir_all(&root).unwrap();
}

/// A test registry from the command line: keys, a root signed by two of three holders, an index signed online.
#[test]
fn yard_keygen_sign_and_verify() {
    let d = tmp("yard");
    let s = |p: PathBuf| p.to_string_lossy().into_owned();
    let run_yard = |args: &[String]| crate::yard::run(args);
    let mut roots = Vec::new();
    for name in ["a", "b", "c"] {
        roots.push(crate::yard::keygen(&d.join(format!("{name}.key"))).unwrap());
    }
    let online = crate::yard::keygen(&d.join("online.key")).unwrap();
    assert!(crate::yard::keygen(&d.join("a.key")).unwrap_err().contains("cannot create"), "never overwrites a key");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(d.join("a.key")).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let root = serde_json::json!({ "format": 1, "version": 1, "registry": "yard.test", "online": { "key": online.hex(), "expires": now + 86400 },
                                   "fulcio": "pem", "rekor": "pem" });
    std::fs::write(d.join("root.json"), serde_json::to_vec_pretty(&root).unwrap()).unwrap();
    let index = serde_json::json!({ "format": 1, "registry": "yard.test", "version": 1, "expires": now + 3600, "extensions": {} });
    std::fs::write(d.join("index.json"), serde_json::to_vec_pretty(&index).unwrap()).unwrap();
    for (role, file, key) in [("root", "root.json", "a.key"), ("root", "root.json", "c.key"), ("index", "index.json", "online.key")] {
        run_yard(&["sign".into(), role.into(), s(d.join(file)), "--key".into(), s(d.join(key))]).unwrap();
    }
    let trust = |threshold: &str| -> Vec<String> {
        let mut a = vec!["verify".to_string(), s(d.clone())];
        for k in &roots {
            a.extend(["--root-key".into(), k.hex()]);
        }
        a.extend(["--threshold".into(), threshold.into()]);
        a
    };
    run_yard(&trust("2")).unwrap();
    let err = run_yard(&trust("3")).unwrap_err();
    assert!(err.starts_with("error[CY0804]: root.json has 2 valid signature(s) from trusted keys; 3 needed"), "{err}");
    // Signing again with the same key replaces its signature; then the third holder makes it 3 of 3.
    run_yard(&["sign".into(), "root".into(), s(d.join("root.json")), "--key".into(), s(d.join("a.key"))]).unwrap();
    run_yard(&["sign".into(), "root".into(), s(d.join("root.json")), "--key".into(), s(d.join("b.key"))]).unwrap();
    run_yard(&trust("3")).unwrap();
    assert!(run_yard(&["sign".into(), "nope".into(), s(d.join("root.json")), "--key".into(), s(d.join("a.key"))]).is_err());
    std::fs::remove_dir_all(&d).unwrap();
}

/// `cyx yard fixture`: a signed index the printed registry entry trusts, the same extensions every time.
#[test]
fn yard_fixture_is_a_signed_index_of_made_up_extensions() {
    let d = tmp("fixture");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let setting = crate::yard::fixture(&d, 300, "http://127.0.0.1:1", now).unwrap();
    let reg = ext_core::registry::Registry::from_setting(&serde_json::from_str(&setting).unwrap()).unwrap();
    assert_eq!((reg.name.as_str(), reg.url.as_str()), ("fixture", "http://127.0.0.1:1"));
    let v = crate::yard::verify(&d, &reg.keys, now).unwrap();
    assert_eq!(v.index.extensions.len(), 300);
    assert!(!v.stale);
    let names: Vec<String> = v.index.extensions.values().map(|e| e.name.clone()).collect();
    crate::yard::fixture(&d, 300, "http://127.0.0.1:1", now).unwrap();
    assert!(crate::yard::verify(&d, &reg.keys, now).is_err(), "new keys every run: the old entry no longer trusts it");
    let read: ext_core::yard::Index = serde_json::from_slice(&std::fs::read(d.join("index.json")).unwrap()).unwrap();
    assert_eq!(read.extensions.values().map(|e| e.name.clone()).collect::<Vec<_>>(), names, "the same extensions");
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn the_publish_workflow_pins_every_action_to_a_commit() {
    let uses: Vec<&str> = scaffold::PUBLISH_WORKFLOW.lines().filter_map(|l| l.trim().strip_prefix("- uses: ")).collect();
    assert_eq!(uses.len(), 2, "{uses:?}");
    for line in uses {
        let rev = line.split_once('@').map(|(_, r)| r.split_whitespace().next().unwrap_or("")).unwrap_or("");
        assert!(rev.len() == 40 && rev.bytes().all(|b| b.is_ascii_hexdigit()), "not pinned to a commit: {line}");
        assert!(line.contains(" # "), "a pin says what it is: {line}");
    }
}

/// `cyx yard check`, what the monitor and the canary run: a healthy registry passes, an index or online key about to
/// expire fails with the fix, and `--revoked` passes only once the version is revoked in the signed index.
#[test]
fn yard_check_reports_what_needs_the_operator() {
    use ext_core::testing::{TestRegistry, github};
    let d = tmp("check");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let mut r = TestRegistry::new(&d, "check", now);
    let files = ["root.json", "root.json.sig", "index.json", "index.json.sig"].map(|n| std::fs::read(r.dir.join(n)).unwrap());
    let keys = r.registry().keys;
    assert!(crate::yard::check(&files, &keys, now, 14, None).unwrap().contains("index expires in 7"), "72 h left");
    assert!(crate::yard::check(&files, &keys, now + 50 * 3600, 14, None).unwrap_err().contains("index expires in 22 h"));
    assert!(crate::yard::check(&files, &keys, now, 100, None).unwrap_err().contains("rotate it"));
    let other = ext_core::yard::RootKeys { keys: vec![ext_core::yard::SigningKey::generate().unwrap().0.public()], threshold: 1 };
    assert!(crate::yard::check(&files, &other, now, 14, None).unwrap_err().contains("CY0804"), "not this registry's keys");

    let manifest = "[extension]\nid = \"codeyard.canary\"\nversion = \"0.0.1\"\nname = \"Canary\"\nlicense = \"MIT\"\n";
    let pkg = ext_pkg::write(vec![ext_pkg::Entry { path: ext_manifest::FILE_NAME.into(), data: manifest.into(), executable: false }]).unwrap().0;
    r.publish(&pkg, &github(1, 2), now);
    let canary = (ext_manifest::ExtId::parse("codeyard.canary").unwrap(), ext_manifest::Version::parse("0.0.1").unwrap());
    let dir = r.dir.clone();
    let read = || ["root.json", "root.json.sig", "index.json", "index.json.sig"].map(|n| std::fs::read(dir.join(n)).unwrap());
    assert!(crate::yard::check(&read(), &keys, now, 14, Some(&canary)).unwrap_err().contains("not revoked in the index yet"));
    r.set_status("codeyard.canary", "0.0.1", ext_core::yard::Status::Revoked { reason: "Canary.".into() }, now);
    assert!(crate::yard::check(&read(), &keys, now, 14, Some(&canary)).unwrap().contains("codeyard.canary 0.0.1 is revoked"));
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn dev_watches_sources_but_not_build_output() {
    let d = std::env::temp_dir().join(format!("cyx-dev-sources-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::create_dir_all(d.join("target/release")).unwrap();
    for f in ["src/lib.rs", "Cargo.toml", "extension.wasm", ".extension.wasm.tmp", "extension.js.map", "Cargo.lock", "target/release/x.wasm", "a-1.0.0.cyx"] {
        std::fs::write(d.join(f), "x").unwrap();
    }
    let names: Vec<String> = dev::sources(&d).iter().map(|(p, _, _)| p.strip_prefix(&d).unwrap().to_string_lossy().replace('\\', "/")).collect();
    assert_eq!(names, ["Cargo.toml", "src/lib.rs"]);
    // A build that did not change the code leaves extension.wasm alone, so Codeyard does not reload.
    let before = std::fs::metadata(d.join("extension.wasm")).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    build::install(&d, b"x").unwrap();
    assert_eq!(std::fs::metadata(d.join("extension.wasm")).unwrap().modified().unwrap(), before);
    assert!(!d.join(build::PARTIAL).exists());
    build::install(&d, b"y").unwrap();
    assert_eq!(std::fs::read(d.join("extension.wasm")).unwrap(), b"y");
    std::fs::remove_dir_all(&d).unwrap();
}

/// #187: a client id the template or a README left in place is flagged; a real one is not.
#[test]
fn placeholder_client_ids_are_flagged() {
    let with = |id: &str| {
        ext_manifest::parse(&format!(
            "[extension]\nid = \"you.forge\"\nversion = \"0.1.0\"\nname = \"F\"\nlicense = \"MIT\"\n[code]\nentry = \"Cargo.toml\"\n[permissions]\nauth = [\"forge\"]\nwhy = \"x\"\n\
             [[pull-requests]]\nid = \"forge\"\nlabel = \"F\"\nhosts = [\"forge.example.com\"]\nauth = \"forge\"\n\
             [[oauth]]\nid = \"forge\"\nname = \"Forge\"\nauthorize-url = \"https://forge.example.com/a\"\ntoken-url = \"https://forge.example.com/t\"\nclient-id = \"{id}\"\nhosts = [\"forge.example.com\"]\n"
        ))
        .unwrap()
    };
    for id in ["your-client-id", "your-gitlab-application-id", "YOUR_CLIENT_ID", "<client-id>", "example-id", "changeme", "xxxxxxxx"] {
        assert_eq!(placeholder_client_ids(&with(id)).len(), 1, "{id}");
    }
    for id in ["3f2a9c1b7d", "Iv1.8a61f9b3a7aba766", "cy-123", "0oa1b2c3d4"] {
        assert!(placeholder_client_ids(&with(id)).is_empty(), "{id}");
    }
}

/// #146: a program inside the package is built per platform: `cyx check` checks each one in `native/`, and `cyx
/// package` writes `<id>-<version>-<triple>.cyx` for each (or the one `--target` names).
#[test]
fn native_programs_are_packaged_per_platform() {
    let root = tmp("native");
    let dir = root.join("hello");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(ext_manifest::FILE_NAME), ext_core::testing::native_manifest("you.hello", "1.0.0")).unwrap();
    let e = check(&dir).unwrap_err();
    assert!(e.contains("native/<triple>/"), "{e}");
    let (linux, mac) = ("x86_64-unknown-linux-musl", "aarch64-apple-darwin");
    for t in [linux, mac] {
        let bin = dir.join("native").join(t).join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("hello"), "#!/bin/sh\necho hi\n").unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(bin.join("hello"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    }
    if !cfg!(unix) {
        return std::fs::remove_dir_all(&root).unwrap(); // A folder on Windows has no executable bit for Unix programs.
    }
    check(&dir).unwrap();
    let out = root.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let written = package_platforms(&dir, Some(&out), None).unwrap();
    let names: Vec<String> = written.iter().map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, [format!("you.hello-1.0.0-{mac}.cyx"), format!("you.hello-1.0.0-{linux}.cyx")]);
    let pkg = ext_pkg::read(&std::fs::read(&written[1].0).unwrap()).unwrap();
    assert_eq!(pkg.target.as_deref(), Some(linux));
    assert!(pkg.files.contains_key("bin/hello") && !pkg.files.keys().any(|p| p.starts_with("native")));
    let one = package_platforms(&dir, Some(&root.join("one.cyx")), Some(linux)).unwrap();
    assert_eq!(one[0].1, written[1].1, "the same platform packages to the same digest");
    assert!(package_platforms(&dir, Some(&root.join("x.cyx")), None).unwrap_err().contains("--target"));
    assert!(package_platforms(&dir, Some(&out), Some("x86_64-pc-windows-msvc")).is_err(), "no folder for it");
    std::fs::remove_dir_all(&root).unwrap();
}
