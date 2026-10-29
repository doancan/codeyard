use std::path::Path;

use ext_manifest::{Code, Manifest};
use wit_parser::{ManglingAndAbi, Resolve};

use crate::component::check_component;

/// A component that imports `imports` (`codeyard:api/net@0.1.0`, or an interface of the test package) and exports
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
        world.push_str("  export codeyard:api/guest@0.1.0;\n");
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
    assert!(c.interfaces.iter().all(|i| ext_api::interface(i).is_some_and(|i| i.need == ext_api::Need::Nothing)), "{c:?}");
}

#[test]
fn an_import_needs_its_permission() {
    let wasm = build(&["codeyard:api/net@0.1.0", "codeyard:api/process@0.1.0"], true, "");
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
    let wasm = build(&["codeyard:api/window@0.1.0", "codeyard:api/storage@0.1.0", "codeyard:api/documents@0.1.0"], true, "");
    check_component(&wasm, &manifest("")).unwrap();
}

#[test]
fn clipboard_read_and_write_are_separate() {
    let wasm = build(&["codeyard:api/clipboard-write@0.1.0"], true, "");
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

#[test]
fn a_newer_api_needs_a_newer_codeyard() {
    let mut r = Resolve::new();
    r.push_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../ext-api/wit")).unwrap();
    let future = "package codeyard:api@0.2.0;\n@since(version = 0.2.0)\ninterface teleport { go: func(); }\n";
    r.push_str("future.wit", future).unwrap();
    let pkg = r.push_str("test.wit", "package test:ext;\nworld w { import codeyard:api/teleport@0.2.0; export codeyard:api/guest@0.1.0; }\n").unwrap();
    let w = r.select_world(&[pkg], Some("w")).unwrap();
    let mut module = wit_component::dummy_module(&r, w, ManglingAndAbi::Standard32);
    wit_component::embed_component_metadata(&mut module, &r, w, wit_component::StringEncoding::UTF8, false).unwrap();
    let wasm = wit_component::ComponentEncoder::default().module(&module).unwrap().encode().unwrap();
    let errs = check_component(&wasm, &manifest("")).unwrap_err();
    assert_eq!(errs.iter().map(|d| d.code).collect::<Vec<_>>(), vec![Code::NeedsNewerCodeyard]);
    assert!(errs[0].to_string().contains("uses API 0.2.0; this Codeyard has 0.1.0"), "{}", errs[0]);
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
