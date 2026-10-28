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
