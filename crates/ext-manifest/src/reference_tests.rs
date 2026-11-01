//! `docs/extensions/reference/manifest.md`, `contributions.md` and `limits.md`, generated from `raw.rs` and `limits.rs`
//! so they cannot drift: a field or a limit without a doc comment fails here. `UPDATE_DOCS=1 cargo test -p ext-manifest
//! reference` writes them.

use std::path::Path;

use crate::limits::*;

const RAW: &str = include_str!("raw.rs");
const LIMITS: &str = include_str!("limits.rs");
const UPDATE: &str = "UPDATE_DOCS=1 cargo test -p ext-manifest reference";

struct Field {
    /// The name as written in the manifest.
    key: String,
    ty: String,
    required: bool,
    doc: String,
}

struct Struct {
    name: String,
    fields: Vec<Field>,
    /// Index of the first field after a blank line, if any (`RawManifest`: where contributions start).
    split: Option<usize>,
}

fn kebab(s: &str) -> String {
    s.replace('_', "-")
}

/// The structs of `raw.rs` in order, with their fields' docs. The file is plain `serde` structs, one field per line.
fn structs() -> Vec<Struct> {
    let mut out: Vec<Struct> = Vec::new();
    let (mut doc, mut rename, mut default, mut blank) = (Vec::<String>::new(), None, false, false);
    for line in RAW.lines() {
        let t = line.trim();
        if let Some(name) = t.strip_prefix("pub struct ").and_then(|r| r.strip_suffix(" {")) {
            out.push(Struct { name: name.to_string(), fields: Vec::new(), split: None });
            doc.clear();
            blank = false;
        } else if t.is_empty() {
            blank = true;
        } else if let Some(d) = t.strip_prefix("///") {
            doc.push(d.trim().to_string());
        } else if let Some(attr) = t.strip_prefix("#[serde(").filter(|_| line.starts_with("    ")) {
            default |= attr.split([',', ')']).any(|a| a.trim() == "default");
            rename = attr.split_once("rename = \"").map(|(_, r)| r.split('"').next().unwrap_or_default().to_string());
        } else if let Some((name, ty)) = t.strip_prefix("pub ").and_then(|r| r.split_once(": ")).filter(|_| line.starts_with("    ")) {
            let s = out.last_mut().expect("a field inside a struct");
            assert!(!doc.is_empty(), "raw.rs: {}.{name} has no doc comment; the manifest reference is made from it", s.name);
            if blank && !s.fields.is_empty() && s.split.is_none() {
                s.split = Some(s.fields.len());
            }
            let ty = ty.trim_end_matches(',').to_string();
            let required = !default && !ty.starts_with("Option<");
            s.fields.push(Field { key: rename.take().unwrap_or_else(|| kebab(name)), ty, required, doc: doc.join(" ") });
            default = false;
            doc.clear();
            blank = false;
        } else {
            doc.clear();
        }
    }
    out
}

/// `S<Vec<S<String>>>` → `Vec<String>`: the spans are for error messages only.
fn unspan(ty: &str) -> String {
    let mut t = ty.to_string();
    while let Some(i) = t.find("S<") {
        let mut depth = 0;
        let end = t[i + 1..]
            .char_indices()
            .find_map(|(j, c)| {
                match c {
                    '<' => depth += 1,
                    '>' => depth -= 1,
                    _ => {}
                }
                (depth == 0).then_some(i + 1 + j)
            })
            .expect("balanced");
        t = format!("{}{}{}", &t[..i], &t[i + 2..end], &t[end + 1..]);
    }
    t
}

fn describe(ty: &str) -> String {
    let t = unspan(ty);
    let t = t.strip_prefix("Option<").and_then(|r| r.strip_suffix('>')).unwrap_or(&t);
    match t {
        "String" => "string".into(),
        "bool" => "boolean".into(),
        "Vec<String>" => "array of strings".into(),
        "[String; 2]" => "array of two strings".into(),
        "toml::Value" => "any value".into(),
        "BTreeMap<String, String>" => "table of strings".into(),
        other => panic!("raw.rs: describe the type {other} in reference_tests.rs"),
    }
}

fn cell(s: &str) -> String {
    s.replace('|', "\\|")
}

fn field_table(s: &Struct) -> String {
    let mut out = String::from("| Field | Type | Required | What it does |\n| --- | --- | --- | --- |\n");
    for f in &s.fields {
        let req = if f.required { "yes" } else { "no" };
        out.push_str(&format!("| `{}` | {} | {req} | {} |\n", f.key, describe(&f.ty), cell(&f.doc)));
    }
    out
}

/// The struct a top-level field holds, and how it is written: `[x]`, `[[x]]` or `[x.<key>]`.
fn table_of<'a>(f: &Field, all: &'a [Struct]) -> Option<(String, &'a Struct)> {
    let t = unspan(&f.ty);
    let (form, inner) = if let Some(r) = t.strip_prefix("Vec<").and_then(|r| r.strip_suffix('>')) {
        (format!("[[{}]]", f.key), r.to_string())
    } else if let Some(r) = t.strip_prefix("BTreeMap<String, ").and_then(|r| r.strip_suffix('>')) {
        (format!("[{}.<key>]", f.key), r.to_string())
    } else {
        (format!("[{}]", f.key), t.strip_prefix("Option<").and_then(|r| r.strip_suffix('>')).unwrap_or(&t).to_string())
    };
    all.iter().find(|s| s.name == inner).map(|s| (form, s))
}

fn header(title: &str, source: &str, intro: &str) -> String {
    format!("# {title}\n\n<!-- Generated from crates/ext-manifest/src/{source}: run `{UPDATE}` after changing it. -->\n\n{intro}\n")
}

fn manifest_and_contributions() -> (String, String) {
    let all = structs();
    let top = all.iter().find(|s| s.name == "RawManifest").expect("RawManifest");
    let split = top.split.expect("raw.rs: a blank line in RawManifest separates the tables from the contributions");
    let (tables, contributions) = top.fields.split_at(split);

    let mut manifest = header(
        "Manifest reference",
        "raw.rs",
        "Every field of `codeyard-extension.toml` outside the contributions, which are in [Contributions](contributions.md). \
         A field that is not listed here is an error (`CY0101`), so a typo is reported instead of ignored. Sizes and counts \
         are in [Limits](limits.md), and what each error means is in [Errors](errors.md). The guide is the \
         [developer guide](../developer-guide.md#the-manifest).",
    );
    manifest.push_str("\n| Top level | Required | What it is |\n| --- | --- | --- |\n");
    for f in tables {
        let form = table_of(f, &all).map_or_else(|| format!("`{}`", f.key), |(form, _)| format!("[`{form}`](#{})", f.key));
        let req = if f.required { "yes" } else { "no" };
        manifest.push_str(&format!("| {form} | {req} | {} |\n", cell(&f.doc)));
    }
    for f in tables {
        if let Some((form, s)) = table_of(f, &all) {
            manifest.push_str(&format!("\n## {}\n\n`{form}`: {}\n\n{}", f.key, f.doc, field_table(s)));
        }
    }

    let mut contrib = header(
        "Contributions reference",
        "raw.rs",
        "What an extension adds to Codeyard. Each kind is an array of tables (`[[commands]]`) except `settings`. Ids are \
         local: `refresh` in `you.deploy` becomes `you.deploy.refresh`. Contributions that run code need a `[code]` table \
         ([Manifest](manifest.md#code)); the others work without any. The guides are the \
         [developer guide](../developer-guide.md#contributions-that-run-your-code), [Commands](../commands.md) and \
         [Views](../views.md).",
    );
    contrib.push('\n');
    for f in contributions {
        let (form, _) = table_of(f, &all).unwrap_or_else(|| panic!("raw.rs: contribution {} is not a table", f.key));
        contrib.push_str(&format!("- [`{form}`](#{}): {}\n", f.key, f.doc));
    }
    for f in contributions {
        let (form, s) = table_of(f, &all).expect("checked above");
        contrib.push_str(&format!("\n## {}\n\n`{form}`: {}\n\n{}", f.key, f.doc, field_table(s)));
    }
    contrib.push_str(&format!(
        "\n## Key names\n\nA `key` is one to four keystrokes separated by spaces (`ctrl-k ctrl-r`), each modifiers and one key \
         joined by `-`. Modifiers: {}. `secondary` is `cmd` on macOS and `ctrl` elsewhere. A key is one character \
         (`a`, `1`, `/`; `ctrl--` is the minus key), `f1` to `f35`, or one of {}. A key a built-in shortcut uses is not \
         bound; `cyx check` warns about it.\n",
        crate::KEY_MODIFIERS.iter().map(|m| format!("`{m}`")).collect::<Vec<_>>().join(", "),
        crate::KEY_NAMES.iter().map(|m| format!("`{m}`")).collect::<Vec<_>>().join(", ")
    ));
    contrib.push_str(&format!(
        "\n## Language ids\n\nThe languages Codeyard gives files itself, for `language` and `languages` fields and \
         `editor.language`: {}. An extension's `[[languages]]` adds its own ids. `cyx check` warns about an id that is \
         neither one of these nor one the extension adds (a dependency may add it).\n",
        crate::LANGUAGE_IDS.iter().map(|m| format!("`{m}`")).collect::<Vec<_>>().join(", ")
    ));
    (manifest, contrib)
}

/// Every limit with its value. A constant missing here fails `limits_reference`.
macro_rules! values {
    ($($n:ident),* $(,)?) => { vec![$((stringify!($n), $n as u64)),*] };
}

fn limit_values() -> Vec<(&'static str, u64)> {
    values![
        MANIFEST_BYTES,
        ID_LEN,
        ID_PART_LEN,
        NAME_LEN,
        DESCRIPTION_LEN,
        WHY_LEN,
        CATEGORIES,
        CONTRIBUTIONS_PER_KIND,
        PERMISSION_ENTRIES,
        DEPENDENCIES,
        ITEM_LEN,
        WHEN_LEN,
        WHEN_DEPTH,
        PACKAGE_BYTES,
        TARGET_PACKAGE_BYTES,
        PACKAGE_UNPACKED_BYTES,
        PACKAGE_FILES,
        PACKAGE_FILE_BYTES,
        DATA_DIR_BYTES,
        CALL_BYTES,
        FRAME_BYTES,
        IPC_BYTES_PER_SECOND,
        IPC_BURST_BYTES,
        TIMER_MIN_SECONDS,
        GUEST_MEMORY_BYTES,
        EXTHOST_MEMORY_BYTES,
        GUEST_CALL_MILLIS,
        ACTIVATE_MILLIS,
        PROVIDER_DEADLINE_MILLIS,
        PULL_REQUEST_DEADLINE_MILLIS,
        STATUS_TEXT_LEN,
        MARKS_PER_FILE,
        DIAGNOSTIC_FILES,
        DIAGNOSTIC_MESSAGE_LEN,
        NET_TIMEOUT_SECS,
        NET_REDIRECTS,
        NET_REQUEST_BYTES,
        NET_RESPONSE_BYTES,
        UI_VIEW_BYTES,
        UI_NODES,
        UI_DEPTH,
        UI_ROWS_PER_PAGE,
        UI_LABEL_LEN,
        UI_TEXT_LEN,
        UI_DOCUMENT_BYTES,
        UI_KEY_LEN,
        UI_IMAGE_BYTES,
        UI_IMAGE_URLS,
        UI_CHOICES,
        UI_COLUMNS,
        UI_URL_LEN,
        SCM_MARKERS,
        SCM_RESOURCES,
        SCM_ORIGINAL_BYTES,
        SCM_DEADLINE_MILLIS,
    ]
}

/// The headings of the blank-line separated groups of `limits.rs`, in order.
const LIMIT_GROUPS: [&str; 5] = ["Manifest", "Packages and storage", "Calls and the extension host", "Views", "Source control"];

fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn bytes(n: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * 1024;
    if n >= MIB && n.is_multiple_of(MIB) {
        format!("{} MiB", n / MIB)
    } else if n >= KIB && n.is_multiple_of(KIB) {
        format!("{} KiB", thousands(n / KIB))
    } else {
        format!("{} bytes", thousands(n))
    }
}

fn limit_value(name: &str, n: u64) -> String {
    if name.ends_with("_BYTES_PER_SECOND") {
        format!("{}/s", bytes(n))
    } else if name.ends_with("_BYTES") {
        bytes(n)
    } else if name.ends_with("_MILLIS") {
        if n.is_multiple_of(1000) { format!("{} s", n / 1000) } else { format!("{n} ms") }
    } else if name.ends_with("_SECS") || name.ends_with("_SECONDS") {
        format!("{n} s")
    } else if name.ends_with("_LEN") {
        format!("{} characters", thousands(n))
    } else {
        thousands(n)
    }
}

fn limits_page() -> String {
    let values = limit_values();
    let mut out = header(
        "Limits",
        "limits.rs",
        "Every size, count and time limit of the extension platform. `cyx check` and the registry refuse a manifest or \
         package over a limit with the error that names it; at run time a call over a limit fails, and an extension host \
         over a memory, time or traffic limit is stopped. The names are the constants in `crates/ext-manifest/src/limits.rs`, \
         which other docs also use.",
    );
    let mut groups: Vec<Vec<(String, String)>> = vec![Vec::new()];
    let mut doc = Vec::<String>::new();
    for line in LIMITS.lines().skip_while(|l| l.starts_with("//!")) {
        let t = line.trim();
        if t.is_empty() {
            if groups.last().is_some_and(|g| !g.is_empty()) {
                groups.push(Vec::new());
            }
        } else if let Some(d) = t.strip_prefix("///") {
            doc.push(d.trim().to_string());
        } else if let Some(rest) = t.strip_prefix("pub const ") {
            let name = rest.split(':').next().unwrap_or_default().to_string();
            assert!(!doc.is_empty(), "limits.rs: {name} has no doc comment; the limits reference is made from it");
            groups.last_mut().expect("a group").push((name, doc.join(" ")));
            doc.clear();
        }
    }
    groups.retain(|g| !g.is_empty());
    assert_eq!(groups.len(), LIMIT_GROUPS.len(), "limits.rs: name the new group of limits in LIMIT_GROUPS");
    let listed: Vec<&str> = groups.iter().flatten().map(|(n, _)| n.as_str()).collect();
    let known: Vec<&str> = values.iter().map(|(n, _)| *n).collect();
    assert_eq!(listed, known, "limits.rs and limit_values() in reference_tests.rs list different constants");
    for (title, group) in LIMIT_GROUPS.iter().zip(&groups) {
        out.push_str(&format!("\n## {title}\n\n| Limit | Value | What it covers |\n| --- | --- | --- |\n"));
        for (name, doc) in group {
            let n = values.iter().find(|(v, _)| v == name).map(|(_, n)| *n).expect("checked above");
            out.push_str(&format!("| `{name}` | {} | {} |\n", limit_value(name, n), cell(doc)));
        }
    }
    out
}

fn check(file: &str, want: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/extensions/reference").join(file);
    if std::env::var_os("UPDATE_DOCS").is_some() {
        std::fs::write(&path, want).unwrap();
    }
    // Windows checkouts may turn line ends into CRLF.
    let have = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
    assert!(have == want, "docs/extensions/reference/{file} is out of date: run `{UPDATE}`");
}

#[test]
fn manifest_reference() {
    let (manifest, contributions) = manifest_and_contributions();
    check("manifest.md", &manifest);
    check("contributions.md", &contributions);
}

#[test]
fn limits_reference() {
    check("limits.md", &limits_page());
}

#[test]
fn limit_values_read_well() {
    assert_eq!(limit_value("MANIFEST_BYTES", 64 * 1024), "64 KiB");
    assert_eq!(limit_value("FRAME_BYTES", 8 * 1024 * 1024 + 64 * 1024), "8,256 KiB");
    assert_eq!(limit_value("IPC_BYTES_PER_SECOND", 32 * 1024 * 1024), "32 MiB/s");
    assert_eq!(limit_value("GUEST_CALL_MILLIS", 2_000), "2 s");
    assert_eq!(limit_value("X_MILLIS", 1_500), "1500 ms");
    assert_eq!(limit_value("PACKAGE_FILES", 10_000), "10,000");
    assert_eq!(limit_value("WHY_LEN", 200), "200 characters");
}
