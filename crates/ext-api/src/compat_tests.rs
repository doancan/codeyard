//! The compatibility check (docs/plugin-architecture.md §5.2): within a major version the WIT only grows. Every item
//! of the last released version (`released/<version>/`, committed when an API version ships) must still be there with
//! the same shape and the same `@since`; every new item needs an `@since` newer than that release and not newer than
//! `VERSION`. The component model has no optional fields or cases, so adding a field to a record or a case to a
//! variant or enum changes the item, and fails here like a removal does.

use std::collections::BTreeMap;
use std::path::Path;

use wit_parser::{Resolve, SourceMap, Stability, Type, TypeDefKind, TypeOwner};

use super::*;

/// The last released API: its WIT as committed under `released/`.
const RELEASED: &str = "1.0.0";

fn released_files() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("released").join(RELEASED);
    let mut files: Vec<(String, String)> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "wit"))
        .map(|p| (p.file_name().unwrap().to_string_lossy().into_owned(), std::fs::read_to_string(&p).unwrap().replace("\r\n", "\n")))
        .collect();
    files.sort();
    files
}

fn current_files() -> Vec<(String, String)> {
    WIT.iter().map(|(n, t)| (n.to_string(), t.replace("\r\n", "\n"))).collect()
}

/// One package from in-memory files.
fn parse(files: &[(String, String)]) -> (Resolve, wit_parser::PackageId) {
    let mut map = SourceMap::new();
    for (name, text) in files {
        map.push_str(name, text.clone());
    }
    let group = map.parse().map_err(|(_, e)| e).unwrap();
    let mut r = Resolve::new();
    let pkg = r.push_group(group).unwrap();
    (r, pkg)
}

/// An item of the package: its shape (the definition with every named type spelled `interface.name`) and its `@since`.
#[derive(Debug)]
struct Item {
    shape: String,
    since: Option<String>,
    /// A type brought in with `use`: it takes the `@since` of its interface, not one of its own.
    used: bool,
}

/// Every interface, function, type and world item, keyed `interface`, `interface/function`, `interface.type`,
/// `world/import`.
fn items(r: &Resolve, pkg: wit_parser::PackageId) -> BTreeMap<String, Item> {
    let since = |s: &Stability| match s {
        Stability::Stable { since, .. } => Some(since.to_string()),
        _ => None,
    };
    let mut out = BTreeMap::new();
    for id in r.packages[pkg].interfaces.values() {
        let i = &r.interfaces[*id];
        let name = i.name.clone().unwrap();
        out.insert(name.clone(), Item { shape: "interface".into(), since: since(&i.stability), used: false });
        for (tname, tid) in &i.types {
            let td = &r.types[*tid];
            let used = matches!(&td.kind, TypeDefKind::Type(Type::Id(o)) if r.types[*o].owner != td.owner);
            out.insert(format!("{name}.{tname}"), Item { shape: shape(r, &td.kind), since: since(&td.stability), used });
        }
        for (fname, f) in &i.functions {
            let params: Vec<String> = f.params.iter().map(|p| format!("{}: {}", p.name, ty(r, &p.ty))).collect();
            let result = f.result.as_ref().map(|t| format!(" -> {}", ty(r, t))).unwrap_or_default();
            out.insert(format!("{name}/{fname}"), Item { shape: format!("func({}){result}", params.join(", ")), since: since(&f.stability), used: false });
        }
    }
    for wid in r.packages[pkg].worlds.values() {
        let w = &r.worlds[*wid];
        for (dir, map) in [("import", &w.imports), ("export", &w.exports)] {
            for key in map.keys() {
                let n = match key {
                    wit_parser::WorldKey::Interface(id) => r.interfaces[*id].name.clone().unwrap(),
                    wit_parser::WorldKey::Name(n) => n.clone(),
                };
                out.insert(format!("{}/{dir} {n}", w.name), Item { shape: dir.into(), since: None, used: true });
            }
        }
    }
    out
}

fn shape(r: &Resolve, k: &TypeDefKind) -> String {
    match k {
        TypeDefKind::Record(rec) => {
            format!("record {{ {} }}", rec.fields.iter().map(|f| format!("{}: {}", f.name, ty(r, &f.ty))).collect::<Vec<_>>().join(", "))
        }
        TypeDefKind::Variant(v) => {
            let cases: Vec<String> = v
                .cases
                .iter()
                .map(|c| match &c.ty {
                    Some(t) => format!("{}({})", c.name, ty(r, t)),
                    None => c.name.clone(),
                })
                .collect();
            format!("variant {{ {} }}", cases.join(", "))
        }
        TypeDefKind::Enum(e) => format!("enum {{ {} }}", e.cases.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")),
        TypeDefKind::Flags(f) => format!("flags {{ {} }}", f.flags.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")),
        TypeDefKind::Type(t) => format!("= {}", ty(r, t)),
        TypeDefKind::Resource => "resource".into(),
        other => anonymous(r, other),
    }
}

fn anonymous(r: &Resolve, k: &TypeDefKind) -> String {
    match k {
        TypeDefKind::List(t) => format!("list<{}>", ty(r, t)),
        TypeDefKind::Option(t) => format!("option<{}>", ty(r, t)),
        TypeDefKind::Tuple(tu) => format!("tuple<{}>", tu.types.iter().map(|t| ty(r, t)).collect::<Vec<_>>().join(", ")),
        TypeDefKind::Result(res) => {
            format!("result<{}, {}>", res.ok.as_ref().map_or("_".into(), |t| ty(r, t)), res.err.as_ref().map_or("_".into(), |t| ty(r, t)))
        }
        TypeDefKind::Type(t) => ty(r, t),
        other => format!("{other:?}"),
    }
}

fn ty(r: &Resolve, t: &Type) -> String {
    match t {
        Type::Id(id) => {
            let td = &r.types[*id];
            match (&td.name, td.owner) {
                (Some(n), TypeOwner::Interface(o)) => format!("{}.{n}", r.interfaces[o].name.as_deref().unwrap_or("?")),
                (Some(n), _) => n.clone(),
                (None, _) => anonymous(r, &td.kind),
            }
        }
        other => format!("{other:?}").to_lowercase(),
    }
}

/// What breaks compatibility going from the released WIT to the current one; empty when it only grew.
fn diff(released: &[(String, String)], current: &[(String, String)]) -> Vec<String> {
    let (old_r, old_p) = parse(released);
    let (new_r, new_p) = parse(current);
    let version = |r: &Resolve, p: wit_parser::PackageId| Version::parse(&r.packages[p].name.version.as_ref().unwrap().to_string()).unwrap();
    let (old_v, new_v) = (version(&old_r, old_p), version(&new_r, new_p));
    if new_v < old_v {
        return vec![format!("the package version went back from {old_v} to {new_v}")];
    }
    if new_v.0 != old_v.0 {
        // A new major may change anything; the migration guide says what.
        return vec![];
    }
    let old = items(&old_r, old_p);
    let new = items(&new_r, new_p);
    let mut problems = Vec::new();
    for (key, o) in &old {
        match new.get(key) {
            None => problems.push(format!("{key} was removed")),
            Some(n) if n.shape != o.shape => problems.push(format!("{key} changed: {} became {}", o.shape, n.shape)),
            Some(n) if n.since != o.since => problems.push(format!("{key} changed its @since from {:?} to {:?}", o.since, n.since)),
            Some(_) => {}
        }
    }
    for (key, n) in new.iter().filter(|(k, _)| !old.contains_key(*k)) {
        if n.used {
            continue;
        }
        match n.since.as_deref().map(|s| Version::parse(s).unwrap()) {
            None => problems.push(format!("{key} is new and has no @since")),
            Some(s) if s <= old_v || s > new_v => problems.push(format!("{key} is new, so its @since must be after {old_v} and at most {new_v}, not {s}")),
            Some(_) => {}
        }
    }
    problems
}

#[test]
fn the_api_only_grows_within_a_major() {
    let problems = diff(&released_files(), &current_files());
    assert!(problems.is_empty(), "codeyard:api changed incompatibly since {RELEASED} (docs/plugin-architecture.md §5.2):\n  {}", problems.join("\n  "));
}

#[test]
fn the_released_snapshot_is_complete() {
    // Every file of the package has its released copy, so a new file cannot slip past the check.
    let released: Vec<String> = released_files().into_iter().map(|(n, _)| n).collect();
    for (name, _) in WIT {
        assert!(released.contains(&name.to_string()), "released/{RELEASED}/{name} is missing");
    }
    if VERSION.to_string() == RELEASED {
        assert_eq!(
            released_files(),
            {
                let mut c = current_files();
                c.sort();
                c
            },
            "the released WIT of {RELEASED} must match wit/ until a new version is in the works"
        );
    }
}

/// The current files with `edit` applied to one of them.
fn edited(file: &str, from: &str, to: &str) -> Vec<(String, String)> {
    current_files()
        .into_iter()
        .map(|(n, t)| {
            if n == file {
                assert!(t.contains(from), "{from:?} is not in {file}");
                (n, t.replacen(from, to, 1))
            } else {
                (n, t)
            }
        })
        .collect()
}

/// The current files as a minor release: the package version bumped.
fn as_version(files: Vec<(String, String)>, v: &str) -> Vec<(String, String)> {
    files.into_iter().map(|(n, t)| (n, t.replace(&format!("package codeyard:api@{VERSION};"), &format!("package codeyard:api@{v};")))).collect()
}

#[test]
fn a_removed_function_fails_the_check() {
    let removed = edited("permissions.wit", "  @since(version = 1.0.0)\n  set: func(key: string, value: string) -> ticket;\n", "");
    let problems = diff(&current_files(), &as_version(removed, "1.1.0"));
    assert_eq!(problems, vec!["secrets/set was removed"]);
}

#[test]
fn an_added_function_passes_the_check() {
    let added = edited(
        "permissions.wit",
        "  delete: func(key: string) -> ticket;\n",
        "  delete: func(key: string) -> ticket;\n  /// Completes with `uris`.\n  @since(version = 1.9.0)\n  keys: func() -> ticket;\n",
    );
    assert_eq!(diff(&current_files(), &as_version(added.clone(), "1.9.0")), Vec::<String>::new());
    // Its `@since` must be the version that adds it.
    let problems =
        diff(&current_files(), &as_version(added.clone(), "1.10.0").into_iter().map(|(n, t)| (n, t.replace("1.9.0)", "1.0.0)"))).collect::<Vec<_>>());
    assert_eq!(problems, vec![format!("secrets/keys is new, so its @since must be after {VERSION} and at most 1.10.0, not 1.0.0")]);
    // And a new interface is an addition too.
    let iface = [
        current_files(),
        vec![(
            "extra.wit".into(),
            format!("package codeyard:api@{VERSION};\n\n/// New.\n@since(version = 1.9.0)\ninterface extra {{\n  @since(version = 1.9.0)\n  go: func();\n}}\n"),
        )],
    ]
    .concat();
    assert_eq!(diff(&current_files(), &as_version(iface, "1.9.0")), Vec::<String>::new());
}

#[test]
fn a_changed_signature_or_type_fails_the_check() {
    let changed = edited("permissions.wit", "  get: func(key: string) -> ticket;", "  get: func(key: string, fallback: string) -> ticket;");
    assert_eq!(
        diff(&current_files(), &as_version(changed, "1.1.0")),
        vec!["secrets/get changed: func(key: string) -> secrets.ticket became func(key: string, fallback: string) -> secrets.ticket"]
    );
    // A new case in a released variant breaks components built against the old one.
    let case = edited("types.wit", "    timeout,\n", "    timeout,\n    busy,\n");
    let problems = diff(&current_files(), &as_version(case, "1.1.0"));
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].starts_with("types.error changed: variant"), "{problems:?}");
    // A new field in a released record, too.
    let field = edited("types.wit", "    character: u32,\n", "    character: u32,\n    offset: u32,\n");
    assert!(diff(&current_files(), &as_version(field, "1.1.0")).iter().any(|p| p.starts_with("types.position changed")));
}

#[test]
fn a_new_major_may_break() {
    let removed = edited("permissions.wit", "  @since(version = 1.0.0)\n  set: func(key: string, value: string) -> ticket;\n", "");
    assert_eq!(diff(&current_files(), &as_version(removed, "2.0.0")), Vec::<String>::new());
}

#[test]
fn the_since_table_matches_the_wit() {
    let (r, pkg) = parse(&current_files());
    let table = since::items();
    let mut seen = 0;
    for (key, item) in items(&r, pkg) {
        if item.used {
            // A type brought in with `use` has the `@since` of the type it names.
            if let Some((i, t)) = key.split_once('.') {
                assert!(table.item(i, t).is_some(), "{key}");
            }
            continue;
        }
        let want = item.since.as_deref().and_then(Version::parse);
        let have = if let Some((i, f)) = key.split_once('/') {
            table.item(i, f)
        } else if let Some((i, t)) = key.split_once('.') {
            table.item(i, t)
        } else {
            table.interface(&key)
        };
        assert_eq!(have, want, "{key}");
        assert!(want.is_some(), "{key} has no @since");
        seen += 1;
    }
    assert_eq!(table.iter().count(), seen, "the table has items the WIT does not");
    // The interface table agrees.
    for i in INTERFACES {
        assert_eq!(table.interface(i.name), Some(i.since), "{}", i.name);
    }
}
