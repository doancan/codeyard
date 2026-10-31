use std::path::Path;

use ext_manifest::{Code, Manifest};
use wit_parser::{ManglingAndAbi, Resolve};

use crate::component::check_component;

/// A component that imports `imports` (`codeyard:api/net@1.0.0`, or an interface of the test package) and exports
/// `codeyard:api/guest` unless `export` is false. `extra` is WIT added to the test package.
fn build(imports: &[&str], export: bool, extra: &str) -> Vec<u8> {
    let mut r = Resolve::new();
    r.push_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../ext-api/wit")).unwrap();
    let mut world = String::from("package test:ext;\n");
    world.push_str(extra);
    world.push_str("world w {\n");
    for i in imports {
        world.push_str(&format!("  import {i};\n"));
    }
    if export {
        world.push_str("  export codeyard:api/guest@1.0.0;\n");
    }
    world.push_str("}\n");
    let pkg = r.push_str("test.wit", &world).unwrap();
    let w = r.select_world(&[pkg], Some("w")).unwrap();
    let mut module = wit_component::dummy_module(&r, w, ManglingAndAbi::Standard32);
    wit_component::embed_component_metadata(&mut module, &r, w, wit_component::StringEncoding::UTF8, false).unwrap();
    wit_component::ComponentEncoder::default().module(&module).unwrap().validate(true).encode().unwrap()
}

fn manifest(permissions: &str) -> Manifest {
    ext_manifest::parse(&format!(
        "[extension]\nid = \"a.b\"\nversion = \"1.0.0\"\nname = \"N\"\nlicense = \"MIT\"\n[code]\nentry = \"Cargo.toml\"\n[[commands]]\nid = \"go\"\ntitle = \"Go\"\n[permissions]\n{permissions}\n"
    ))
    .unwrap()
}

fn codes(r: Result<crate::Component, Vec<ext_manifest::Diagnostic>>) -> Vec<Code> {
    r.unwrap_err().into_iter().map(|d| d.code).collect()
}

#[test]
fn the_fixture_is_current() {
    let want = build(&[], true, "");
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/guest.wasm");
    if std::env::var_os("UPDATE_FIXTURES").is_some() {
        std::fs::write(&path, &want).unwrap();
    }
    assert!(std::fs::read(&path).unwrap_or_default() == want, "testdata/guest.wasm is out of date: run `UPDATE_FIXTURES=1 cargo test -p ext-pkg fixture`");
    let c = check_component(crate::testing::COMPONENT, &manifest("")).unwrap();
    assert_eq!(c.min_api, ext_api::VERSION);
    let (want, _) = build_from(v0_1, &[], "codeyard:api/guest@0.1.0");
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/guest-0.1.wasm");
    if std::env::var_os("UPDATE_FIXTURES").is_some() {
        std::fs::write(&path, &want).unwrap();
    }
    assert!(std::fs::read(&path).unwrap_or_default() == want, "testdata/guest-0.1.wasm is out of date: run `UPDATE_FIXTURES=1 cargo test -p ext-pkg fixture`");
    assert!(check_component(crate::testing::COMPONENT_0_1, &manifest("")).unwrap().in_transition());
    assert!(c.interfaces.iter().all(|i| ext_api::interface(i).is_some_and(|i| i.need == ext_api::Need::Nothing)), "{c:?}");
}

#[test]
fn an_import_needs_its_permission() {
    let wasm = build(&["codeyard:api/net@1.0.0", "codeyard:api/process@1.0.0"], true, "");
    let errs = check_component(&wasm, &manifest("")).unwrap_err();
    assert_eq!(errs.iter().map(|d| d.code).collect::<Vec<_>>(), vec![Code::NotDeclared, Code::NotDeclared]);
    assert_eq!(
        errs[0].to_string(),
        r#"error[CY0201]: the code uses `net`, which needs the net permission, but the manifest does not declare it (fix: add to [permissions]:  net = ["<host>"])"#
    );
    let ok = check_component(&wasm, &manifest("net = [\"api.example.com\"]\nrun = [\"vercel\"]\nwhy = \"Deploys.\"")).unwrap();
    assert!(ok.interfaces.contains(&"net".to_string()) && ok.interfaces.contains(&"process".to_string()));
}

#[test]
fn free_interfaces_need_nothing() {
    let wasm = build(&["codeyard:api/window@1.0.0", "codeyard:api/storage@1.0.0", "codeyard:api/documents@1.0.0"], true, "");
    check_component(&wasm, &manifest("")).unwrap();
}

#[test]
fn clipboard_read_and_write_are_separate() {
    let wasm = build(&["codeyard:api/clipboard-write@1.0.0"], true, "");
    check_component(&wasm, &manifest("clipboard = \"write\"")).unwrap();
    assert_eq!(codes(check_component(&wasm, &manifest("clipboard = \"read\""))), vec![Code::NotDeclared]);
}

#[test]
fn foreign_imports_and_missing_exports_are_refused() {
    let wasm = build(&["evil"], true, "interface evil { steal: func(); }\n");
    assert_eq!(codes(check_component(&wasm, &manifest(""))), vec![Code::BadComponent]);
    let wasm = build(&[], false, "");
    let errs = check_component(&wasm, &manifest("")).unwrap_err();
    assert!(errs[0].message.contains("does not export codeyard:api/guest"), "{}", errs[0]);
}

/// A component built from the API's WIT as another version: `wit` edits each file's text (the version, items added).
fn build_from(wit: impl Fn(&str) -> String, imports: &[&str], export: &str) -> (Vec<u8>, ext_api::since::Items) {
    let files: Vec<(String, String)> = ext_api::WIT
        .iter()
        // Windows checkouts may turn line ends into CRLF; the edits are written with LF.
        .map(|(n, t)| (n.to_string(), wit(&t.replace("\r\n", "\n"))))
        .collect();
    let mut map = wit_parser::SourceMap::new();
    for (n, t) in &files {
        map.push_str(n, t.clone());
    }
    let mut r = Resolve::new();
    r.push_group(map.parse().map_err(|(_, e)| e).unwrap()).unwrap();
    let mut world = String::from("package test:ext;\nworld w {\n");
    for i in imports {
        world.push_str(&format!("  import {i};\n"));
    }
    world.push_str(&format!("  export {export};\n}}\n"));
    let pkg = r.push_str("test.wit", &world).unwrap();
    let w = r.select_world(&[pkg], Some("w")).unwrap();
    let mut module = wit_component::dummy_module(&r, w, ManglingAndAbi::Standard32);
    wit_component::embed_component_metadata(&mut module, &r, w, wit_component::StringEncoding::UTF8, false).unwrap();
    let wasm = wit_component::ComponentEncoder::default().module(&module).unwrap().encode().unwrap();
    (wasm, ext_api::since::Items::parse(files.iter().map(|(_, t)| t.as_str())))
}

/// API 1.1.0: `net` gains `teleport`.
fn v1_1(text: &str) -> String {
    let text = text.replace("codeyard:api@1.0.0;", "codeyard:api@1.1.0;");
    text.replace(
        "  fetch: func(request: http-request) -> ticket;\n",
        "  fetch: func(request: http-request) -> ticket;\n  @since(version = 1.1.0)\n  teleport: func();\n",
    )
}

#[test]
fn an_item_this_codeyard_lacks_needs_a_newer_codeyard() {
    let (wasm, _) = build_from(v1_1, &["codeyard:api/net@1.1.0"], "codeyard:api/guest@1.1.0");
    let errs = check_component(&wasm, &manifest("net = [\"api.example.com\"]")).unwrap_err();
    assert_eq!(errs.iter().map(|d| d.code).collect::<Vec<_>>(), vec![Code::NeedsNewerCodeyard]);
    assert!(errs[0].to_string().contains("uses `net.teleport`, which this Codeyard (API 1.0.0) does not have"), "{}", errs[0]);
    // A Codeyard with 1.1 runs it, and the extension needs 1.1.
    let (_, items) = build_from(v1_1, &[], "codeyard:api/guest@1.1.0");
    let host = crate::component::Host { version: ext_api::Version(1, 1, 0), items: &items };
    let c = crate::component::check_against(&wasm, &manifest("net = [\"api.example.com\"]"), &host).unwrap();
    assert_eq!((c.min_api, c.built_for), (ext_api::Version(1, 1, 0), ext_api::Version(1, 1, 0)));
}

#[test]
fn a_newer_minor_that_uses_only_older_items_runs() {
    // Built with a 1.1 SDK, using nothing 1.1 added: it runs here, and needs only 1.0.
    let (wasm, _) = build_from(v1_1, &["codeyard:api/window@1.1.0"], "codeyard:api/guest@1.1.0");
    let c = check_component(&wasm, &manifest("")).unwrap();
    assert_eq!((c.min_api, c.built_for), (ext_api::Version(1, 0, 0), ext_api::Version(1, 1, 0)));
    assert!(!c.in_transition());
}

#[test]
fn a_newer_major_needs_a_newer_codeyard() {
    let v2 = |t: &str| t.replace("codeyard:api@1.0.0;", "codeyard:api@2.0.0;").replace("version = 1.0.0", "version = 2.0.0");
    let (wasm, _) = build_from(v2, &["codeyard:api/log@2.0.0"], "codeyard:api/guest@2.0.0");
    let errs = check_component(&wasm, &manifest("")).unwrap_err();
    assert!(errs.iter().all(|d| d.code == Code::NeedsNewerCodeyard), "{errs:?}");
    assert!(errs[0].to_string().contains("uses API 2.0.0; this Codeyard has 1.0.0"), "{}", errs[0]);
}

/// The API's WIT as it was at 0.1.0: the same shape under the old version.
fn v0_1(text: &str) -> String {
    text.replace("codeyard:api@1.0.0;", "codeyard:api@0.1.0;").replace("version = 1.0.0", "version = 0.1.0")
}

#[test]
fn a_0_1_component_runs_during_the_transition() {
    let (wasm, _) = build_from(v0_1, &["codeyard:api/net@0.1.0"], "codeyard:api/guest@0.1.0");
    let c = check_component(&wasm, &manifest("net = [\"api.example.com\"]")).unwrap();
    assert!(c.in_transition(), "{c:?}");
    assert_eq!(c.built_for, ext_api::Version(0, 1, 0));
    assert!(c.interfaces.contains(&"net".to_string()), "{c:?}");
    // The exthost renames its imports to 1.0.0; the result is a 1.0 component to the check too.
    let upgraded = ext_api::transition::upgrade(&wasm).unwrap();
    let c = check_component(&upgraded, &manifest("net = [\"api.example.com\"]")).unwrap();
    assert_eq!(c.built_for, ext_api::VERSION);
    assert!(!c.in_transition());
    // Another 0.x is refused with the migration guide.
    let v02 = |t: &str| t.replace("codeyard:api@1.0.0;", "codeyard:api@0.2.0;").replace("version = 1.0.0", "version = 0.2.0");
    let (wasm, _) = build_from(v02, &["codeyard:api/log@0.2.0"], "codeyard:api/guest@0.2.0");
    let errs = check_component(&wasm, &manifest("")).unwrap_err();
    assert!(errs[0].to_string().contains("built against API 0.2.0, which this Codeyard (1.0.0) no longer runs"), "{}", errs[0]);
    assert!(errs[0].to_string().contains("migrating-to-1.0.md"), "{}", errs[0]);
}

#[test]
fn not_a_component_is_refused() {
    let core = b"\0asm\x01\0\0\0".to_vec();
    let errs = check_component(&core, &manifest("")).unwrap_err();
    assert!(errs[0].message.contains("core WebAssembly module"), "{}", errs[0]);
    assert_eq!(codes(check_component(b"not wasm", &manifest(""))), vec![Code::BadComponent]);
}

#[test]
fn a_code_package_needs_its_component() {
    use crate::{Entry, Error, read, write};
    let m = "[extension]\nid = \"a.b\"\nversion = \"1.0.0\"\nname = \"N\"\nlicense = \"MIT\"\n[code]\nentry = \"Cargo.toml\"\n[[commands]]\nid = \"go\"\ntitle = \"Go\"\n";
    let entry = |path: &str, data: &[u8]| Entry { path: path.into(), data: data.to_vec(), executable: false };
    assert!(matches!(write(vec![entry(ext_manifest::FILE_NAME, m.as_bytes()), entry("Cargo.toml", b"")]), Err(Error::Component(_))));
    let (bytes, _) =
        write(vec![entry(ext_manifest::FILE_NAME, m.as_bytes()), entry("Cargo.toml", b""), entry(crate::COMPONENT, crate::testing::COMPONENT)]).unwrap();
    let p = read(&bytes).unwrap();
    assert_eq!(p.component.unwrap().min_api, ext_api::VERSION);
}
