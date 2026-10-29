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
    let (kind, name, publisher) = parse_new(&s(&["theme", "zzz-not-here", "--publisher", "acme"])).unwrap();
    assert_eq!((kind, name.as_str(), publisher.as_str()), (Kind::Theme, "zzz-not-here", "acme"));
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
