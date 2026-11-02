//! The schema (schema.rs) against serde and `validate`, and the TypeScript SDK's copies of it: its types and schema
//! (sdk/typescript/src/view-schema.ts) and the cases its mock host must judge as Codeyard does
//! (sdk/typescript/test/view-cases.json). `CODEYARD_UPDATE_VIEW_SCHEMA=1 cargo test -p ext-ui` rewrites both.

use serde_json::{Map, Value, json};

use crate::schema::{Property, Type, properties, schema};
use crate::*;

/// `v` as JSON text with every object's keys sorted, whether or not serde_json's `preserve_order` feature is on (a
/// workspace build may turn it on), so the generated files are the same on every build.
fn json_text(v: &Value) -> String {
    canonical(v).to_string()
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(&canonical(v)).unwrap()
}

fn canonical(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            Value::Object(keys.into_iter().map(|k| (k.clone(), canonical(&m[k]))).collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

fn sample(ty: Type) -> Value {
    match ty {
        Type::Space => json!(1),
        Type::Tone => json!("accent"),
        Type::Selection => json!("one"),
        Type::Bool => json!(true),
        Type::Label | Type::Body | Type::Document | Type::Alt => json!("x"),
        Type::Level => json!(2),
        Type::Percent => json!(50),
        Type::Icon => json!("refresh"),
        Type::Url => json!("https://example.com/a"),
        Type::Page => json!("p"),
        Type::OptionKey => json!("a"),
        Type::Choices => json!([{"key": "a", "label": "A"}]),
        Type::Pairs => json!([{"name": "n", "value": "v"}]),
        Type::Columns => json!(["A"]),
        Type::Image => json!({"url": "https://example.com/a.png"}),
    }
}

/// Every property of `kind` set to a good value.
fn full(kind: Kind) -> Map<String, Value> {
    properties(kind).iter().map(|p| (p.name.to_string(), sample(p.ty))).collect()
}

fn element(kind: Kind, props: Map<String, Value>) -> Value {
    if kind == Kind::Divider { json!("divider") } else { json!({ kind.name(): props }) }
}

fn parses(e: &Value) -> Result<Element, String> {
    serde_json::from_value::<Element>(e.clone()).map_err(|e| e.to_string())
}

#[test]
fn the_schema_is_what_serde_accepts() {
    let schema = schema();
    let kinds: Vec<&str> = schema["elements"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, Kind::ALL.map(Kind::name));
    for kind in Kind::ALL {
        let props = full(kind);
        let parsed = parses(&element(kind, props.clone())).unwrap_or_else(|e| panic!("{kind} with every property: {e}"));
        assert_eq!(parsed.kind(), kind);
        // Serialized, an element names every property it has: the schema lists exactly those.
        let written = serde_json::to_value(&parsed).unwrap();
        let mut names: Vec<&str> = written.get(kind.name()).and_then(Value::as_object).map(|o| o.keys().map(String::as_str).collect()).unwrap_or_default();
        let mut listed: Vec<&str> = properties(kind).iter().map(|p| p.name).collect();
        names.sort_unstable();
        listed.sort_unstable();
        assert_eq!(names, listed, "the properties of {kind}");
        for p in properties(kind) {
            let mut without = props.clone();
            without.remove(p.name);
            assert_eq!(parses(&element(kind, without)).is_err(), p.required, "{kind}.{} required", p.name);
            let mut nulled = props.clone();
            nulled.insert(p.name.into(), Value::Null);
            assert_eq!(parses(&element(kind, nulled)).is_ok(), p.nullable, "{kind}.{} nullable", p.name);
        }
        if kind != Kind::Divider {
            let mut extra = props.clone();
            extra.insert("colour".into(), json!("red"));
            assert!(parses(&element(kind, extra)).is_err(), "{kind} refuses an unknown property");
        }
    }
    assert_eq!(schema["tones"], json!([Tone::Neutral, Tone::Accent, Tone::Success, Tone::Warning, Tone::Danger, Tone::Muted]));
    assert_eq!(schema["selections"], json!([Selection::None, Selection::One, Selection::Many]));
}

/// Values of `ty` that break a rule or do not parse.
fn bad(ty: Type) -> Vec<Value> {
    let s = |n: usize| "a".repeat(n);
    match ty {
        Type::Space => vec![json!(5), json!(256), json!(-1), json!("1")],
        Type::Tone => vec![json!("red")],
        Type::Selection => vec![json!("two")],
        Type::Bool => vec![json!("yes")],
        Type::Label => vec![json!(""), json!("  "), json!(s(257)), json!("é".repeat(256)), json!("a\u{200B}b"), json!("a\nb"), json!(42)],
        Type::Body => vec![json!(s(4097)), json!("é".repeat(4096)), json!("a\u{7}b"), json!("a\n\tb"), json!("a\u{200B}b")],
        // The size cap (64 KB) is checked by the TypeScript tests themselves, to keep the cases small.
        Type::Document => vec![json!("a\u{7}b"), json!("a\n\tb")],
        Type::Alt => vec![json!(""), json!(s(257)), json!("a\u{202E}b")],
        Type::Level => vec![json!(0), json!(4), json!(1.5)],
        Type::Percent => vec![json!(100), json!(101), json!(255), json!(256)],
        Type::Icon => vec![json!("Refresh"), json!(""), json!("a--b"), json!("a-"), json!("1a"), json!(s(41)), json!("ré")],
        Type::Url => vec![
            json!("http://example.com"),
            json!("https://"),
            json!("https://user@example.com"),
            json!("https://exa mple.com"),
            json!("https://example.com:8443/x?y#z"),
            json!("https://example.com:ab"),
            json!(format!("https://example.com/{}", s(2048))),
        ],
        Type::Page => vec![json!(""), json!(s(129))],
        Type::OptionKey => vec![json!("zzz"), json!("")],
        Type::Choices => vec![
            json!([]),
            json!([{"key": "a", "label": "A"}, {"key": "a", "label": "B"}]),
            json!([{"key": "", "label": "A"}]),
            json!([{"key": "a\u{1}", "label": "A"}]),
            json!([{"key": s(129), "label": "A"}]),
            json!([{"key": "a", "label": ""}]),
            json!([{"key": "a", "label": "A", "icon": "x"}]),
            Value::Array((0..257).map(|i| json!({"key": format!("k{i}"), "label": "L"})).collect()),
        ],
        Type::Pairs => vec![
            json!([{"name": "", "value": ""}]),
            json!([{"name": "n", "value": s(4097)}]),
            json!([{"name": "n"}]),
            Value::Array((0..257).map(|_| json!({"name": "n", "value": "v"})).collect()),
        ],
        Type::Columns => vec![json!([]), json!([""]), Value::Array((0..33).map(|i| json!(format!("c{i}"))).collect())],
        Type::Image => vec![
            json!({"bytes": ""}),
            json!({"bytes": "iVBORw=="}),
            json!({"bytes": "!!"}),
            json!({"url": "http://example.com/a.png"}),
            json!({"file": "a.png"}),
            json!({}),
            json!({"url": "https://example.com/a.png", "bytes": "iVBORw=="}),
        ],
    }
}

fn node(key: Option<&str>, element: Value, children: Vec<Value>) -> Value {
    let mut n = Map::new();
    if let Some(k) = key {
        n.insert("key".into(), json!(k));
    }
    n.insert("element".into(), element);
    if !children.is_empty() {
        n.insert("children".into(), Value::Array(children));
    }
    Value::Object(n)
}

fn text(t: &str) -> Value {
    node(None, json!({"text": {"text": t}}), vec![])
}

fn button(key: Option<&str>) -> Value {
    node(key, json!({"button": {"label": "Go"}}), vec![])
}

fn column(key: Option<&str>, children: Vec<Value>) -> Value {
    node(key, json!({"column": {}}), children)
}

fn nested(depth: usize) -> Value {
    (1..depth).fold(text("deep"), |inner, _| column(None, vec![inner]))
}

/// Views that are not one element with bad properties: the tree's own rules and the shapes around it. The row, element
/// and size caps are left to the TypeScript tests, to keep the cases small.
fn structure() -> Vec<(&'static str, String)> {
    let v = |name, value: Value| (name, json_text(&value));
    let raw = |name, text: &str| (name, text.to_string());
    let table = |rows: Vec<Value>| node(Some("t"), json!({"table": {"label": "T", "columns": ["A", "B"]}}), rows);
    let row = |key: Option<&str>, cells: usize| node(key, json!({"row": {}}), (0..cells).map(|_| text("c")).collect());
    let tree = |key: Option<&str>, children: Vec<Value>| node(key, json!({"tree": {"label": "T"}}), children);
    vec![
        // The shorthand of #182, as the old templates wrote it.
        raw("the type shorthand", r#"{"type": "column", "children": [{"type": "text", "text": "2"}, {"type": "button", "key": "add", "label": "Add one"}]}"#),
        v("the reference form of the template", column(None, vec![text("2"), button(Some("add"))])),
        raw("not JSON", "{\"element\": "),
        raw("not an object", "[]"),
        raw("a string", "\"divider\""),
        raw("trailing text", "{\"element\": \"divider\"} x"),
        v("no element", json!({"key": "a"})),
        v("an unknown node field", json!({"element": "divider", "kids": []})),
        v("an unknown element", json!({"element": {"buton": {"label": "Go"}}})),
        v("two kinds in one element", json!({"element": {"text": {"text": "a"}, "badge": {"text": "b"}}})),
        v("a kind without properties", json!({"element": "text"})),
        v("properties that are not an object", json!({"element": {"column": 1}})),
        v("divider as null", json!({"element": {"divider": null}})),
        v("divider with properties", json!({"element": {"divider": {}}})),
        v("children that are not a list", json!({"element": {"column": {}}, "children": {}})),
        v("a key that is not text", json!({"key": 1, "element": "divider"})),
        v("a null key", json!({"key": null, "element": "divider"})),
        v("null children", json!({"element": {"column": {}}, "children": null})),
        v("an interactive element without a key", column(None, vec![button(None)])),
        v("a key with a control character", button(Some("a\u{1}b"))),
        v("an empty key", button(Some(""))),
        v("a long key", button(Some(&"k".repeat(129)))),
        v("a key of 128 bytes", button(Some(&"é".repeat(64)))),
        v("keys repeated among siblings", column(None, vec![button(Some("a")), button(Some("a"))])),
        v("keys repeated through an unkeyed column", column(None, vec![button(Some("a")), column(None, vec![button(Some("a"))])])),
        v(
            "the same key under different keyed parents",
            column(None, vec![column(Some("x"), vec![button(Some("a"))]), column(Some("y"), vec![button(Some("a"))])]),
        ),
        v("children on a text", node(None, json!({"text": {"text": "a"}}), vec![text("b")])),
        v("rows without keys", node(Some("l"), json!({"list": {"label": "L"}}), vec![text("a"), text("b")])),
        v("a table row with the wrong cells", table(vec![row(Some("r1"), 2), row(Some("r2"), 1)])),
        v("a table holding a text", table(vec![node(Some("r"), json!({"text": {"text": "a"}}), vec![])])),
        v("a tree holding a text", tree(Some("t"), vec![node(Some("a"), json!({"text": {"text": "a"}}), vec![])])),
        v("tree items without keys", tree(Some("t"), vec![tree(None, vec![])])),
        v("tree item keys repeated in the tree", tree(Some("t"), vec![tree(Some("a"), vec![tree(Some("b"), vec![])]), tree(Some("b"), vec![])])),
        v("an item tree needs no key of its own rows", tree(Some("t"), vec![tree(Some("a"), vec![tree(Some("b"), vec![])])])),
        v("an empty-state holding a text", node(None, json!({"empty-state": {"title": "T", "text": ""}}), vec![text("a"), button(Some("b"))])),
        v(
            "an empty-state with a link",
            node(None, json!({"empty-state": {"title": "T", "text": ""}}), vec![node(Some("l"), json!({"link": {"label": "L"}}), vec![])]),
        ),
        v("32 levels", nested(32)),
        v("33 levels", nested(33)),
        v("more than 100 problems", column(None, (0..120).map(|i| node(Some(&format!("b{i}")), json!({"button": {"label": ""}}), vec![])).collect())),
        v("problems named down the tree", column(None, vec![column(Some("a\"b"), vec![node(None, json!({"heading": {"text": "h", "level": 9}}), vec![])])])),
    ]
}

fn expect(json: &str) -> Value {
    match decode_json(json) {
        Ok(_) => json!("ok"),
        Err(DecodeError::TooLarge(n)) => json!({ "tooLarge": n }),
        Err(DecodeError::Malformed(m)) => json!({ "malformed": m }),
        Err(DecodeError::Invalid(problems)) => json!({ "problems": problems.iter().map(ToString::to_string).collect::<Vec<_>>() }),
    }
}

fn cases() -> Value {
    let mut views = Vec::new();
    let mut push = |name: String, json: String| {
        let expect = expect(&json);
        views.push(json!({"name": name, "json": json, "expect": expect}));
    };
    for kind in Kind::ALL {
        let props = full(kind);
        push(format!("{kind} with every property"), json_text(&node(Some("k"), element(kind, props.clone()), vec![])));
        if kind == Kind::Divider {
            continue;
        }
        let required: Map<String, Value> = properties(kind).iter().filter(|p| p.required).map(|p| (p.name.to_string(), sample(p.ty))).collect();
        push(format!("{kind} with only what it needs"), json_text(&node(Some("k"), element(kind, required), vec![])));
        let mut extra = props.clone();
        extra.insert("colour".into(), json!("red"));
        push(format!("{kind} with an unknown property"), json_text(&node(Some("k"), element(kind, extra), vec![])));
        for p in properties(kind) {
            let Property { name, required, .. } = *p;
            let mut variants: Vec<(String, Option<Value>)> = bad(p.ty).into_iter().map(|b| (json_text(&b), Some(b))).collect();
            variants.push(("null".into(), Some(Value::Null)));
            if required {
                variants.push(("left out".into(), None));
            }
            for (label, value) in variants {
                let mut with = props.clone();
                match value {
                    Some(v) => with.insert(name.into(), v),
                    None => with.remove(name),
                };
                let mut label = label;
                if label.len() > 60 {
                    label = format!("{}… ({} bytes)", &label[..label.char_indices().nth(40).map_or(label.len(), |(i, _)| i)], label.len());
                }
                push(format!("{kind}.{name} = {label}"), json_text(&node(Some("k"), element(kind, with), vec![])));
            }
        }
    }
    for (name, json) in structure() {
        push(name.to_string(), json);
    }
    let events: Vec<Value> = [
        r#"{"click": {"path": ["add"]}}"#,
        r#"{"click": "add"}"#,
        r#"{"click": {"path": []}}"#,
        r#"{"click": {}}"#,
        r#"{"click": {"path": ["a"], "x": 1}}"#,
        r#"{"change": {"path": ["n"], "value": {"text": "web"}}}"#,
        r#"{"change": {"path": ["n"], "value": {"checked": true}}}"#,
        r#"{"change": {"path": ["n"], "value": {"choice": "a"}}}"#,
        r#"{"change": {"path": ["n"], "value": "web"}}"#,
        r#"{"change": {"path": ["n"], "value": {"checked": "yes"}}}"#,
        r#"{"submit": {"path": ["s"], "text": "deploy"}}"#,
        r#"{"select": {"path": ["d"], "rows": ["d1", "d2"]}}"#,
        r#"{"select": {"path": ["d"], "rows": "d1"}}"#,
        r#"{"expand": {"path": ["p"], "item": "web", "page": ""}}"#,
        r#"{"expand": {"path": ["p"], "item": null, "page": "2"}}"#,
        r#"{"expand": {"path": ["p"], "page": "2"}}"#,
        r#"{"link": {"path": ["help"]}}"#,
        r#"{"type": "click", "key": "add"}"#,
        r#"{"click": {"path": ["a"]}, "link": {"path": ["b"]}}"#,
        r#""click""#,
    ]
    .iter()
    .map(|e| json!({"json": e, "ok": Event::from_json(e).is_ok()}))
    .collect();
    json!({"views": views, "events": events})
}

/// TypeScript for a property's value.
fn ts_type(ty: Type) -> &'static str {
    match ty {
        Type::Space => "Space",
        Type::Tone => "Tone",
        Type::Selection => "Selection",
        Type::Bool => "boolean",
        Type::Level => "1 | 2 | 3",
        Type::Percent => "number",
        Type::Label | Type::Body | Type::Document | Type::Alt | Type::Icon | Type::Url | Type::Page | Type::OptionKey => "string",
        Type::Choices => "{ key: string; label: string }[]",
        Type::Pairs => "{ name: string; value: string }[]",
        Type::Columns => "string[]",
        Type::Image => "{ bytes: string } | { url: string }",
    }
}

fn typescript() -> String {
    let mut out = String::from(
        "// Generated from crates/ext-ui (schema.rs) by `cargo test -p ext-ui`; do not edit. After a change to the view format,\n\
         // `CODEYARD_UPDATE_VIEW_SCHEMA=1 cargo test -p ext-ui` rewrites it. The format is docs/extensions/reference/ui.md.\n\n\
         import type { Tone } from \"./wit.ts\";\n\n\
         /** A spacing step, from 0 (none) to 4 (widest). */\n\
         export type Space = 0 | 1 | 2 | 3 | 4;\n\n\
         /** How many rows of a list, table or tree the user can select. */\n\
         export type Selection = \"none\" | \"one\" | \"many\";\n\n\
         /** The properties of each element (docs/extensions/reference/ui.md). */\n\
         export interface ElementProps {\n",
    );
    for kind in Kind::ALL {
        let name = kind.name();
        let key = if name.contains('-') { format!("\"{name}\"") } else { name.to_string() };
        if kind == Kind::Divider {
            out.push_str(&format!("  {key}: Record<string, never>;\n"));
            continue;
        }
        let props: Vec<String> = properties(kind)
            .iter()
            .map(|p| {
                let mut t = ts_type(p.ty).to_string();
                if p.nullable {
                    t = format!("{t} | null");
                }
                format!("{}{}: {t}", p.name, if p.required { "" } else { "?" })
            })
            .collect();
        if props.is_empty() {
            out.push_str(&format!("  {key}: Record<string, never>;\n"));
        } else {
            out.push_str(&format!("  {key}: {{ {} }};\n", props.join("; ")));
        }
    }
    out.push_str(
        "}\n\n\
         /** The kinds of element. */\n\
         export type Kind = keyof ElementProps;\n\n\
         /** An element: `{ <kind>: { properties } }`, or `\"divider\"`. */\n\
         export type Element = \"divider\" | { [K in Exclude<Kind, \"divider\">]: { [P in K]: ElementProps[K] } }[Exclude<Kind, \"divider\">];\n\n\
         /**\n * One node of a view's tree, as Codeyard reads it from `render`: the element, its key (needed on interactive elements\n\
         \x20* and rows, unique among siblings) and its children (on column, row, list, tree, table and empty-state).\n */\n\
         export interface ViewNode {\n  key?: string;\n  element: Element;\n  children?: ViewNode[];\n}\n\n\
         /** The format as data, for the mock host's check (testing/view-check.ts). */\n\
         export interface ViewSchema {\n\
         \x20 elements: { kind: Kind; children: boolean; rows: boolean; interactive: boolean; properties: { name: string; type: string; required: boolean; nullable: boolean; what: string }[] }[];\n\
         \x20 tones: string[];\n\
         \x20 selections: string[];\n\
         \x20 events: Record<string, string[]>;\n\
         \x20 limits: Record<string, number>;\n\
         }\n\n\
         export const VIEW_SCHEMA: ViewSchema = ",
    );
    out.push_str(&pretty(&schema()));
    out.push_str(";\n");
    out
}

/// `path` (from the repository root) holds `want`, or is rewritten to it under CODEYARD_UPDATE_VIEW_SCHEMA.
fn current(path: &str, want: &str) {
    let full = format!("{}/../../{path}", env!("CARGO_MANIFEST_DIR"));
    if std::env::var_os("CODEYARD_UPDATE_VIEW_SCHEMA").is_some() {
        std::fs::write(&full, want).unwrap();
        return;
    }
    let have = std::fs::read_to_string(&full).unwrap_or_default().replace("\r\n", "\n");
    assert!(have == want, "{path} is out of date: run `CODEYARD_UPDATE_VIEW_SCHEMA=1 cargo test -p ext-ui` and commit it");
}

#[test]
fn the_typescript_sdk_has_the_current_schema_and_cases() {
    current("sdk/typescript/src/view-schema.ts", &typescript());
    current("sdk/typescript/test/view-cases.json", &(pretty(&cases()) + "\n"));
}

#[test]
fn the_cases_cover_every_outcome() {
    let cases = cases();
    let outcome = |want: &str| cases["views"].as_array().unwrap().iter().filter(|c| c["expect"] == want || c["expect"].get(want).is_some()).count();
    assert!(
        outcome("ok") > 40 && outcome("malformed") > 100 && outcome("problems") > 100,
        "{} {} {}",
        outcome("ok"),
        outcome("malformed"),
        outcome("problems")
    );
    // The template's old tree is refused for the reason #182 reported; the reference form is drawn.
    let shorthand = cases["views"].as_array().unwrap().iter().find(|c| c["name"] == "the type shorthand").unwrap();
    assert!(shorthand["expect"]["malformed"].as_str().unwrap().contains("unknown field `type`"), "{shorthand}");
}
