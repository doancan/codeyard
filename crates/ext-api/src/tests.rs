use std::collections::BTreeSet;
use std::path::Path;

use wit_parser::{Resolve, Stability, Type, TypeDefKind, TypeOwner};

use super::*;
use crate::value::{LOCAL, Notify, Start};

fn resolve() -> (Resolve, wit_parser::PackageId) {
    let mut r = Resolve::new();
    let (pkg, _) = r.push_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("wit")).unwrap();
    (r, pkg)
}

fn interfaces(r: &Resolve, pkg: wit_parser::PackageId) -> Vec<&wit_parser::Interface> {
    r.packages[pkg].interfaces.values().map(|id| &r.interfaces[*id]).collect()
}

#[test]
fn the_package_parses_and_matches_the_constants() {
    let (r, pkg) = resolve();
    let p = &r.packages[pkg];
    assert_eq!(format!("{}:{}", p.name.namespace, p.name.name), PACKAGE);
    assert_eq!(p.name.version.as_ref().unwrap().to_string(), VERSION.to_string());
    // The embedded copies are the files.
    for (name, text) in WIT {
        let on_disk = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("wit").join(name)).unwrap();
        assert_eq!(on_disk.replace("\r\n", "\n"), text.replace("\r\n", "\n"), "{name}");
    }
}

#[test]
fn every_interface_is_in_the_table_with_its_since() {
    let (r, pkg) = resolve();
    let wit: BTreeSet<&str> = interfaces(&r, pkg).iter().map(|i| i.name.as_deref().unwrap()).collect();
    let table: BTreeSet<&str> = INTERFACES.iter().map(|i| i.name).chain(EXPORTS).collect();
    assert_eq!(wit, table);
    for i in interfaces(&r, pkg) {
        let name = i.name.as_deref().unwrap();
        let Stability::Stable { since, .. } = &i.stability else { panic!("{name} has no @since") };
        let want = interface(name).map_or(VERSION, |t| t.since);
        assert_eq!(since.to_string(), want.to_string(), "{name}");
        assert!(i.docs.contents.is_some(), "{name} has no doc comment");
    }
    // The world imports every table interface (some only through `use`).
    let world = r.packages[pkg].worlds["extension"];
    let imported: BTreeSet<String> = r.worlds[world]
        .imports
        .keys()
        .filter_map(|k| match k {
            wit_parser::WorldKey::Interface(id) => r.interfaces[*id].name.clone(),
            wit_parser::WorldKey::Name(n) => Some(n.clone()),
        })
        .collect();
    for t in INTERFACES {
        assert!(imported.contains(t.name), "the world does not import {}", t.name);
    }
}

#[test]
fn every_imported_function_has_one_call() {
    let starts = [
        Start::ShowNotice { severity: value::Severity::Info, message: String::new(), actions: vec![] },
        Start::QuickPick { items: vec![], options: Default::default() },
        Start::InputBox { options: Default::default() },
        Start::OpenExternal { url: String::new() },
        Start::Execute { command: String::new(), args_json: String::new() },
        Start::ActiveDocument,
        Start::Selections,
        Start::ReadDocument { uri: String::new() },
        Start::ApplyEdits { uri: String::new(), version: 0, edits: vec![] },
        Start::SetSelections { uri: String::new(), selections: vec![] },
        Start::ReadFile { uri: String::new() },
        Start::ReadMany { uris: vec![] },
        Start::Stat { uri: String::new() },
        Start::FindFiles { glob: String::new(), max: 0 },
        Start::Search { query: value::SearchQuery { pattern: String::new(), regex: false, case_sensitive: false, whole_word: false, include: vec![], max: 0 } },
        Start::WriteFile { uri: String::new(), contents: vec![] },
        Start::Delete { uri: String::new() },
        Start::Rename { from: String::new(), to: String::new() },
        Start::CreateDirectory { uri: String::new() },
        Start::Fetch { request: http() },
        Start::Session { provider: String::new(), scopes: vec![] },
        Start::AuthFetch { provider: String::new(), request: http() },
        Start::SecretGet { key: String::new() },
        Start::SecretSet { key: String::new(), value: String::new() },
        Start::SecretDelete { key: String::new() },
        Start::Spawn { command: value::Command { program: String::new(), args: vec![], cwd: None, env: vec![], stdin: None } },
        Start::ReadClipboard,
        Start::WriteClipboard { text: String::new() },
        Start::SendText { text: String::new() },
    ];
    let notifies = [
        Notify::Log { level: value::LogLevel::Info, message: String::new() },
        Notify::Cancel { ticket: 0 },
        Notify::Respond { id: 0, result: Ok(value::Response::None) },
        Notify::StartProgress { id: 0, title: String::new(), cancellable: false },
        Notify::ReportProgress { id: 0, message: None, percent: None },
        Notify::EndProgress { id: 0 },
        Notify::SetStatusItem { item: value::StatusItem { id: String::new(), text: String::new(), tooltip: None, command: None, tone: value::Tone::Neutral } },
        Notify::RemoveStatusItem { id: String::new() },
        Notify::Every { id: 0, seconds: 30 },
        Notify::ClearTimer { id: 0 },
        Notify::Invalidate { view: String::new() },
        Notify::SetDiagnostics { uri: String::new(), diagnostics: vec![] },
        Notify::ClearDiagnostics { uri: None },
        Notify::SetDecorations { uri: String::new(), set: String::new(), decorations: vec![] },
        Notify::ScmRefresh { provider: String::new() },
    ];
    let mut ours: Vec<&str> = starts.iter().map(Start::wit_name).chain(notifies.iter().map(Notify::wit_name)).collect();
    ours.extend(LOCAL);
    let mut sorted = ours.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), ours.len(), "a WIT function has two calls");

    let (r, pkg) = resolve();
    let mut wit = Vec::new();
    for i in interfaces(&r, pkg) {
        let name = i.name.as_deref().unwrap();
        if EXPORTS.contains(&name) {
            continue;
        }
        for f in i.functions.keys() {
            wit.push(format!("{name}/{f}"));
        }
    }
    wit.sort();
    assert_eq!(wit, sorted);
}

fn http() -> value::HttpRequest {
    value::HttpRequest { method: "GET".into(), url: String::new(), headers: vec![], body: vec![], timeout_ms: None }
}

#[test]
fn imports_are_classified() {
    assert_eq!(classify("codeyard:api/net@0.1.0"), Import::Api { interface: "net", version: Some(Version(0, 1, 0)) });
    assert_eq!(classify("codeyard:api/net"), Import::Api { interface: "net", version: None });
    assert_eq!(classify("codeyard:api/net@x"), Import::Unknown("codeyard:api/net@x"));
    assert_eq!(classify("wasi:io/streams@0.2.6"), Import::Wasi("wasi:io/streams@0.2.6"));
    assert_eq!(classify("wasi:sockets/tcp@0.2.6"), Import::ForbiddenWasi("wasi:sockets/tcp@0.2.6"));
    assert_eq!(classify("wasi:http/outgoing-handler@0.2.6"), Import::ForbiddenWasi("wasi:http/outgoing-handler@0.2.6"));
    assert_eq!(classify("acme:evil/x"), Import::Unknown("acme:evil/x"));
}

#[test]
fn versions() {
    let v = Version::parse;
    assert_eq!(v("0.1.0"), Some(Version(0, 1, 0)));
    assert_eq!(v("0.1"), None);
    assert_eq!(v("0.1.0.1"), None);
    assert!(Version(0, 1, 0).runs_on(Version(0, 1, 3)));
    assert!(!Version(0, 1, 4).runs_on(Version(0, 1, 3)));
    assert!(!Version(0, 1, 0).runs_on(Version(0, 2, 0)), "a 0.x minor may break");
    assert!(Version(1, 0, 0).runs_on(Version(1, 4, 0)));
    assert!(!Version(1, 0, 0).runs_on(Version(2, 0, 0)));
}

#[test]
fn needs_follow_the_manifest() {
    use ext_manifest::Clipboard;
    let mut p = Permissions::default();
    assert!(Need::Nothing.granted(&p));
    assert!(!Need::Net.granted(&p));
    p.net = vec!["api.example.com".into()];
    p.clipboard = Clipboard::Read;
    assert!(Need::Net.granted(&p));
    assert!(Need::ClipboardRead.granted(&p));
    assert!(!Need::ClipboardWrite.granted(&p));
    assert!(!Need::Run.granted(&p));
}

/// `docs/extensions/reference/api.md`, generated from the WIT so it cannot drift.
fn render_reference(r: &Resolve, pkg: wit_parser::PackageId) -> String {
    let mut out = format!(
        "# API reference\n\n<!-- Generated from crates/ext-api/wit: run `UPDATE_DOCS=1 cargo test -p ext-api reference` after \
         changing it. -->\n\n`{PACKAGE}@{VERSION}`. Authors use an SDK, which wraps these calls; this page says what each one does \
         and which permission it needs. Calls that return a `ticket` finish later: the SDK turns them into async functions \
         (Rust) or Promises (TypeScript).\n"
    );
    for i in interfaces(r, pkg) {
        let name = i.name.as_deref().unwrap_or_default();
        out.push_str(&format!("\n## `{name}`\n\n"));
        if let Some(d) = &i.docs.contents {
            out.push_str(&one_paragraph(d));
            out.push_str("\n\n");
        }
        let since = match &i.stability {
            Stability::Stable { since, .. } => since.to_string(),
            _ => String::new(),
        };
        match interface(name) {
            Some(t) if t.need != Need::Nothing => out.push_str(&format!("Needs `{}` in `[permissions]`. Since {since}.\n", t.need.manifest_line())),
            Some(_) => out.push_str(&format!("Needs no permission. Since {since}.\n")),
            None => out.push_str(&format!("Exported by the SDK; authors do not call it. Since {since}.\n")),
        }
        for (tname, id) in &i.types {
            let td = &r.types[*id];
            if !matches!(td.owner, TypeOwner::Interface(o) if r.interfaces[o].name.as_deref() == Some(name)) {
                continue;
            }
            if matches!(td.kind, TypeDefKind::Type(_)) && td.docs.contents.is_none() {
                continue;
            }
            out.push_str(&format!("\n- type `{tname}`: {}", type_shape(r, &td.kind)));
            if let Some(d) = &td.docs.contents {
                out.push_str(&format!(". {}", one_paragraph(d)));
            }
        }
        for (fname, f) in &i.functions {
            let params: Vec<String> = f.params.iter().map(|p| format!("{}: {}", p.name, ty(r, &p.ty))).collect();
            let result = f.result.as_ref().map(|t| format!(" -> {}", ty(r, t))).unwrap_or_default();
            out.push_str(&format!("\n- `{fname}({}){result}`", params.join(", ")));
            if let Some(d) = &f.docs.contents {
                out.push_str(&format!(": {}", one_paragraph(d)));
            }
        }
        out.push('\n');
    }
    out
}

fn one_paragraph(d: &str) -> String {
    d.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn type_shape(r: &Resolve, k: &TypeDefKind) -> String {
    match k {
        TypeDefKind::Record(rec) => {
            let f: Vec<String> = rec.fields.iter().map(|f| format!("{}: {}", f.name, ty(r, &f.ty))).collect();
            format!("record `{{ {} }}`", f.join(", "))
        }
        TypeDefKind::Enum(e) => {
            let c: Vec<&str> = e.cases.iter().map(|c| c.name.as_str()).collect();
            format!("enum `{}`", c.join(" | "))
        }
        TypeDefKind::Variant(v) => {
            let c: Vec<String> = v
                .cases
                .iter()
                .map(|c| match &c.ty {
                    Some(t) => format!("{}({})", c.name, ty(r, t)),
                    None => c.name.clone(),
                })
                .collect();
            format!("variant `{}`", c.join(" | "))
        }
        TypeDefKind::Type(t) => format!("`{}`", ty(r, t)),
        other => format!("`{}`", other.as_str()),
    }
}

fn ty(r: &Resolve, t: &Type) -> String {
    match t {
        Type::Bool => "bool".into(),
        Type::U8 => "u8".into(),
        Type::U16 => "u16".into(),
        Type::U32 => "u32".into(),
        Type::U64 => "u64".into(),
        Type::S8 => "s8".into(),
        Type::S16 => "s16".into(),
        Type::S32 => "s32".into(),
        Type::S64 => "s64".into(),
        Type::F32 => "f32".into(),
        Type::F64 => "f64".into(),
        Type::Char => "char".into(),
        Type::String => "string".into(),
        Type::ErrorContext => "error-context".into(),
        Type::Id(id) => {
            let td = &r.types[*id];
            if let Some(n) = &td.name {
                return n.clone();
            }
            match &td.kind {
                TypeDefKind::List(t) => format!("list<{}>", ty(r, t)),
                TypeDefKind::Option(t) => format!("option<{}>", ty(r, t)),
                TypeDefKind::Tuple(tu) => {
                    format!("tuple<{}>", tu.types.iter().map(|t| ty(r, t)).collect::<Vec<_>>().join(", "))
                }
                TypeDefKind::Result(res) => {
                    format!("result<{}, {}>", res.ok.as_ref().map_or("_".into(), |t| ty(r, t)), res.err.as_ref().map_or("_".into(), |t| ty(r, t)))
                }
                TypeDefKind::Type(t) => ty(r, t),
                other => other.as_str().to_string(),
            }
        }
    }
}

#[test]
fn reference() {
    let (r, pkg) = resolve();
    let want = render_reference(&r, pkg);
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/extensions/reference/api.md");
    if std::env::var_os("UPDATE_DOCS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &want).unwrap();
    }
    // Windows checkouts may turn line ends into CRLF.
    let have = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
    assert!(have == want, "docs/extensions/reference/api.md is out of date: run `UPDATE_DOCS=1 cargo test -p ext-api reference`");
}
