//! Data lens: conversion round trips, lossy cases and their warnings, the query language against jq's documented
//! behaviour, paths at the cursor, decoding, and random input that must neither panic nor hang.

use serde_json::{Value, json};

use crate::datalens::{DataError, Format, Note, Options, emit, line_col, parse, parse_any, path_at, run_query, sniff};
use crate::datalens_decode::decode;

fn j(text: &str) -> Value {
    serde_json::from_str(text).expect("test JSON")
}

fn convert(text: &str, from: Format, to: Format) -> Result<(String, Vec<Note>), DataError> {
    let loaded = parse(text, from)?;
    let mut notes = loaded.notes;
    let out = emit(&loaded.value, to, &Options::default())?;
    notes.extend(out.notes);
    Ok((out.text, notes))
}

// --- Round trips ---

/// Documents every format can hold: no null, arrays of one kind.
const FIXTURES: &[&str] = &[
    r#"{"name": "codeyard", "version": "0.2.2", "private": true, "workers": 4, "ratio": 0.75}"#,
    r#"{"servers": [{"host": "a.example.com", "port": 8080}, {"host": "b.example.com", "port": 8081}], "tags": ["x", "y"]}"#,
    r#"{"deep": {"er": {"est": {"leaf": "ok", "list": [1, 2, 3]}}}, "empty_map": {}, "empty_list": []}"#,
    r#"{"b": 1, "a": 2, "c": {"z": 1, "y": 2}}"#,
    r#"{"quote": "say \"hi\"", "slash": "C:\\temp", "tab": "a\tb", "nl": "line1\nline2", "uni": "ğüşiöç 😀", "empty": ""}"#,
    r#"{"looks_like": {"bool": "true", "null": "null", "int": "123", "float": "1.5", "date": "2024-01-31", "yes": "yes", "tilde": "~", "colon": "a: b", "hash": "a #b", "dash": "- x", "star": "*x", "version": "1.2.3"}}"#,
    r#"{"big": 9007199254740993, "neg": -9223372036854775808, "max": 9223372036854775807, "float": 1.5e300}"#,
    r#"{"ğ anahtar": {"iç": [{"a-b": 1}, {"c d": 2}]}, "": "empty key", "x": "<<", "a.b": 3}"#,
    r#"{"matrix": [[1, 2], [3, 4]], "objs": [{"k": [true, false]}]}"#,
];

#[test]
fn json_yaml_toml_json_round_trip_keeps_values_and_key_order() {
    for src in FIXTURES {
        let start = parse(src, Format::Json).unwrap().value;
        let (yaml, _) = convert(src, Format::Json, Format::Yaml).unwrap();
        let (toml, _) = convert(&yaml, Format::Yaml, Format::Toml).unwrap_or_else(|e| panic!("{e}\n{yaml}"));
        let (json, _) = convert(&toml, Format::Toml, Format::Json).unwrap();
        let back = parse(&json, Format::Json).unwrap().value;
        assert_eq!(back, start, "{src}\nYAML:\n{yaml}\nTOML:\n{toml}\nJSON:\n{json}");
    }
}

#[test]
fn json_to_yaml_and_back_keeps_key_order() {
    let (yaml, _) = convert(FIXTURES[3], Format::Json, Format::Yaml).unwrap();
    assert!(yaml.starts_with("b: 1\na: 2\nc:\n  z: 1\n  \"y\": 2\n"), "{yaml}");
    let back = parse(&yaml, Format::Yaml).unwrap().value;
    assert_eq!(back.as_object().unwrap().keys().collect::<Vec<_>>(), ["b", "a", "c"]);
}

#[test]
fn integers_beyond_two_to_the_53_stay_exact_in_every_format() {
    let src = r#"{"id": 9007199254740993, "low": -9223372036854775808}"#;
    for to in [Format::Yaml, Format::Toml, Format::Json] {
        let (text, _) = convert(src, Format::Json, to).unwrap();
        assert!(text.contains("9007199254740993") && text.contains("-9223372036854775808"), "{to:?}: {text}");
    }
    let (yaml, _) = convert("id: 18446744073709551615\n", Format::Yaml, Format::Json).unwrap();
    assert!(yaml.contains("18446744073709551615"), "{yaml}");
}

#[test]
fn indent_and_sorted_keys_apply_to_json_and_yaml() {
    let value = parse(r#"{"b": {"y": 1, "x": [1]}, "a": 2}"#, Format::Json).unwrap().value;
    let four = emit(&value, Format::Json, &Options { indent: 4, ..Options::default() }).unwrap().text;
    assert_eq!(four, "{\n    \"b\": {\n        \"y\": 1,\n        \"x\": [\n            1\n        ]\n    },\n    \"a\": 2\n}\n");
    let sorted = emit(&value, Format::Yaml, &Options { sort_keys: true, ..Options::default() }).unwrap().text;
    // `y` is a boolean to YAML 1.1 readers, so as a key it is quoted.
    assert_eq!(sorted, "a: 2\nb:\n  x:\n    - 1\n  \"y\": 1\n");
}

#[test]
fn yaml_output_reads_back_for_awkward_strings() {
    let strings = [
        "",
        " lead",
        "trail ",
        "a: b",
        "a #b",
        "#x",
        "- x",
        "-",
        "?",
        ":",
        "*x",
        "&x",
        "!x",
        "|",
        ">",
        "%x",
        "@x",
        "`x",
        "[x]",
        "{x}",
        "'q'",
        "\"q\"",
        "null",
        "Null",
        "~",
        "true",
        "False",
        "yes",
        "No",
        "on",
        "OFF",
        "y",
        "n",
        "123",
        "-1",
        "+1",
        "1.5",
        "1e3",
        ".5",
        ".inf",
        ".NaN",
        "0x1F",
        "0o7",
        "012",
        "2024-01-31",
        "1.2.3",
        "a\nb",
        "a\r\nb",
        "tab\there",
        "\u{85}",
        "\u{2028}",
        "\u{7f}",
        "\0",
        "---",
        "...",
        "= ",
        "=",
        "<<",
        "end:",
        "ğüş",
        "😀",
        "a\\b",
        "c:\\x",
        "0",
    ];
    for s in strings {
        // The YAML reader of the schema crate takes any `<<` key, quoted or not, as a merge key.
        let key = if s == "<<" { "x<<" } else { s };
        let doc = json!({ key: s, "list": [s] });
        let yaml = emit(&doc, Format::Yaml, &Options::default()).unwrap().text;
        let back = parse(&yaml, Format::Yaml).unwrap_or_else(|e| panic!("{s:?}: {e}\n{yaml}")).value;
        assert_eq!(back, doc, "{s:?}\n{yaml}");
    }
}

// --- Lossy and refused conversions ---

#[test]
fn yaml_comments_and_anchors_are_reported() {
    let src = "# header\nbase: &b\n  a: 1 # why\nuse: *b\nquoted: \"# not a comment\"\n";
    let (json, notes) = convert(src, Format::Yaml, Format::Json).unwrap();
    assert!(notes.contains(&Note::CommentsLost) && notes.contains(&Note::AliasesExpanded), "{notes:?}");
    assert_eq!(parse(&json, Format::Json).unwrap().value, j(r##"{"base": {"a": 1}, "use": {"a": 1}, "quoted": "# not a comment"}"##));
    assert_eq!(Note::CommentsLost.text(), "Comments are not kept.");
    let (_, notes) = convert("quoted: \"# not a comment\"\nurl: http://x/#frag\n", Format::Yaml, Format::Json).unwrap();
    assert!(notes.is_empty(), "{notes:?}");
}

#[test]
fn yaml_merge_keys_expand() {
    let (json, _) = convert("base: &b {a: 1, b: 2}\nx:\n  <<: *b\n  b: 3\n", Format::Yaml, Format::Json).unwrap();
    assert_eq!(parse(&json, Format::Json).unwrap().value["x"], j(r#"{"b": 3, "a": 1}"#));
}

#[test]
fn multi_document_yaml_becomes_an_array() {
    let (json, notes) = convert("a: 1\n---\nb: 2\n---\n- 3\n", Format::Yaml, Format::Json).unwrap();
    assert_eq!(parse(&json, Format::Json).unwrap().value, j(r#"[{"a": 1}, {"b": 2}, [3]]"#));
    assert!(notes.contains(&Note::Documents(3)));
}

#[test]
fn toml_refuses_null_and_mixed_arrays_with_the_path() {
    let err = convert(r#"{"a": {"b": [1, null]}}"#, Format::Json, Format::Toml).unwrap_err();
    assert!(err.message.contains(".a.b[1]") && err.message.contains("no null"), "{err}");
    let err = convert(r#"{"servers": [{"x": 1}, {"x": null}]}"#, Format::Json, Format::Toml).unwrap_err();
    assert!(err.message.contains(".servers[1].x"), "{err}");
    let err = convert(r#"{"list": [1, "two"]}"#, Format::Json, Format::Toml).unwrap_err();
    assert!(err.message.contains(".list") && err.message.contains("integer") && err.message.contains("string"), "{err}");
    let err = convert(r#"{"list": [1, 2.5]}"#, Format::Json, Format::Toml).unwrap_err();
    assert!(err.message.contains("integer") && err.message.contains("float"), "{err}");
    let err = convert("[1, 2]", Format::Json, Format::Toml).unwrap_err();
    assert!(err.message.contains("top level") && err.message.contains("array"), "{err}");
    let err = convert(r#"{"big": 18446744073709551615}"#, Format::Json, Format::Toml).unwrap_err();
    assert!(err.message.contains(".big") && err.message.contains("64"), "{err}");
}

#[test]
fn toml_dates_become_text_with_a_note() {
    let (json, notes) = convert("when = 2024-05-01T10:00:00Z\nday = 2024-05-01\n", Format::Toml, Format::Json).unwrap();
    assert_eq!(parse(&json, Format::Json).unwrap().value, j(r#"{"when": "2024-05-01T10:00:00Z", "day": "2024-05-01"}"#));
    assert!(notes.contains(&Note::DatesAsText));
}

#[test]
fn toml_tables_come_out_after_plain_values() {
    let (toml, _) = convert(r#"{"a": {"x": 1}, "b": 2, "c": [{"k": 1}, {"k": 2}]}"#, Format::Json, Format::Toml).unwrap();
    assert_eq!(toml, "b = 2\n\n[a]\nx = 1\n\n[[c]]\nk = 1\n\n[[c]]\nk = 2\n");
}

#[test]
fn duplicate_keys_are_refused_with_the_line() {
    let err = parse("{\n  \"a\": 1,\n  \"b\": 2,\n  \"a\": 3\n}", Format::Json).err().unwrap();
    assert_eq!((err.at.map(|a| a.0), err.message.as_str()), (Some(4), "Duplicate key"), "{err}");
    let err = parse("a: 1\nb:\n  c: 1\n  c: 2\n", Format::Yaml).err().unwrap();
    assert_eq!(err.at.map(|a| a.0), Some(4), "{err}");
    let err = parse("a = 1\nb = 2\na = 3\n", Format::Toml).err().unwrap();
    assert_eq!(err.at.map(|a| a.0), Some(3), "{err}");
    let err = parse("A=1\nB=2\nA=3\n", Format::Env).err().unwrap();
    assert_eq!(err.at.map(|a| a.0), Some(3), "{err}");
}

#[test]
fn syntax_errors_carry_a_position() {
    let err = parse("{\n  \"a\": [1, 2\n  \"b\": 3\n}", Format::Json).err().unwrap();
    assert_eq!(err.at.map(|a| a.0), Some(3), "{err}");
    let err = parse("a: [1, 2\nb: 3\n", Format::Yaml).err().unwrap();
    assert!(err.at.is_some(), "{err}");
    let err = parse("a = \nb = 2\n", Format::Toml).err().unwrap();
    assert_eq!(err.at.map(|a| a.0), Some(1), "{err}");
    assert!(err.to_string().contains("line 1"), "{err}");
}

#[test]
fn jsonc_is_read_and_the_loss_is_reported() {
    let (out, notes) = convert("{\n  // why\n  \"a\": 1, /* two */ \"b\": [1, 2,],\n}\n", Format::Json, Format::Yaml).unwrap();
    assert_eq!(out, "a: 1\nb:\n  - 1\n  - 2\n");
    assert_eq!(notes, [Note::CommentsLost]);
}

#[test]
fn env_files_read_and_write() {
    let src =
        "# database\nexport HOST=localhost\nPORT=5432\nNAME='my db' # trailing\nPASS=\"p\\\"w\\nd\"\nEMPTY=\nMULTI=\"one\ntwo\"\nURL=https://x.example/a?b=c\n";
    let loaded = parse(src, Format::Env).unwrap();
    assert_eq!(
        loaded.value,
        j(r#"{"HOST": "localhost", "PORT": "5432", "NAME": "my db", "PASS": "p\"w\nd", "EMPTY": "", "MULTI": "one\ntwo", "URL": "https://x.example/a?b=c"}"#)
    );
    assert!(loaded.notes.contains(&Note::CommentsLost));
    let out = emit(&loaded.value, Format::Env, &Options::default()).unwrap().text;
    assert_eq!(parse(&out, Format::Env).unwrap().value, loaded.value, "{out}");
    assert!(out.contains("NAME='my db'") && out.contains("HOST=localhost") && out.contains("EMPTY=\n"), "{out}");
}

#[test]
fn env_holds_flat_text_only_unless_flattening_is_chosen() {
    let nested = r#"{"db": {"host": "h", "ports": [1, 2]}, "debug": true, "N": 3}"#;
    let err = convert(nested, Format::Json, Format::Env).unwrap_err();
    assert!(err.message.contains(".db") && err.message.contains("__"), "{err}");
    let value = parse(nested, Format::Json).unwrap().value;
    let out = emit(&value, Format::Env, &Options { flatten_env: true, ..Options::default() }).unwrap();
    assert_eq!(out.text, "db__host=h\ndb__ports__0=1\ndb__ports__1=2\ndebug=true\nN=3\n");
    assert_eq!(out.notes, [Note::EnvText]);
    let err = convert(r#"{"a": null}"#, Format::Json, Format::Env).unwrap_err();
    assert!(err.message.contains("no null"), "{err}");
    let err = convert(r#"{"bad key": "x"}"#, Format::Json, Format::Env).unwrap_err();
    assert!(err.message.contains("bad key"), "{err}");
}

#[test]
fn format_follows_the_file_name() {
    for (path, want) in [
        ("/x/a.json", Some(Format::Json)),
        ("tsconfig.JSONC", Some(Format::Json)),
        ("C:\\x\\ci.yml", Some(Format::Yaml)),
        ("a.yaml", Some(Format::Yaml)),
        ("Cargo.toml", Some(Format::Toml)),
        (".env", Some(Format::Env)),
        ("/p/.env.local", Some(Format::Env)),
        ("prod.env", Some(Format::Env)),
        ("notes.md", None),
        ("Makefile", None),
    ] {
        assert_eq!(Format::from_path(path), want, "{path}");
    }
    assert_eq!(sniff("{\"a\": 1}"), Format::Json);
    assert_eq!(sniff("a = 1\n[b]\nc = 2\n"), Format::Toml);
    assert_eq!(sniff("a: 1\n"), Format::Yaml);
}

#[test]
fn positions_count_characters() {
    assert_eq!(line_col("ab\nğüş x", 0), (1, 1));
    assert_eq!(line_col("ab\nğüş x", 3), (2, 1));
    assert_eq!(line_col("ab\nğüş x", "ab\nğüş".len()), (2, 4));
    assert_eq!(line_col("ab", 99), (1, 3));
    assert_eq!(line_col("ğ", 1), (1, 1), "an offset inside a character moves back to its start");
}

// --- Queries ---

fn query(doc: &str, q: &str) -> Result<Vec<Value>, String> {
    let value = j(doc);
    let parsed = crate::datalens::parse_query(q).map_err(|e| format!("syntax at {:?}: {}", e.offset, e.message))?;
    parsed.run(&value, &|| false).map_err(|e| e.message)
}

fn check(doc: &str, q: &str, want: &[&str]) {
    let want: Vec<Value> = want.iter().map(|w| j(w)).collect();
    assert_eq!(query(doc, q), Ok(want), "{q} on {doc}");
}

/// Each row is what jq 1.7 prints for the same program and input (jq manual, "Builtin operators and functions").
#[test]
fn query_conformance_with_jq() {
    let table: &[(&str, &str, &[&str])] = &[
        (r#"{"a":1}"#, ".a", &["1"]),
        (r#"{"a":{"b":2}}"#, ".a.b", &["2"]),
        (r#"{"a":[1,2,3]}"#, ".a[1]", &["2"]),
        (r#"{"a":[1,2,3]}"#, ".a[-1]", &["3"]),
        (r#"{"a":[1,2,3]}"#, ".a[]", &["1", "2", "3"]),
        (r#"{"a":[1,2,3]}"#, ".a[1:]", &["[2,3]"]),
        (r#"{"a":[1,2,3]}"#, ".a[:2]", &["[1,2]"]),
        (r#"{"a":[1,2,3]}"#, ".a[5]", &["null"]),
        (r#"{"a b":1}"#, r#".["a b"]"#, &["1"]),
        (r#"{"a b":1}"#, r#"."a b""#, &["1"]),
        (r#"{"ağ":1}"#, r#"."ağ""#, &["1"]),
        (r#"{"a":1}"#, ".b", &["null"]),
        ("null", ".a.b.c", &["null"]),
        (r#"[[1,2],[3,4]]"#, ".[][0]", &["1", "3"]),
        (r#"{"a":{"b":1},"c":{"b":2}}"#, ".[] | .b", &["1", "2"]),
        (r#"{"a":{"b":1}}"#, "[..] | length", &["3"]),
        ("[1,2,3]", "length", &["3"]),
        (r#""héllo""#, "length", &["5"]),
        ("null", "length", &["0"]),
        ("-5", "length", &["5"]),
        (r#"{"b":1,"a":2}"#, "keys", &[r#"["a","b"]"#]),
        (r#"{"b":1,"a":2}"#, "keys_unsorted", &[r#"["b","a"]"#]),
        ("[10,20]", "keys", &["[0,1]"]),
        ("[3,1,2]", "sort", &["[1,2,3]"]),
        (r#"[null,true,false,"a",1,[0],{"a":1}]"#, "sort", &[r#"[null,false,true,1,"a",[0],{"a":1}]"#]),
        (r#"[{"id":1},{"id":3},{"id":5}]"#, ".[] | select(.id == 3)", &[r#"{"id":3}"#]),
        (r#"{"items":[{"id":1,"name":"x"},{"id":3,"name":"y"}]}"#, ".items[] | select(.id == 3) | .name", &[r#""y""#]),
        ("[1,2,3]", "map(. * 2)", &["[2,4,6]"]),
        ("[1,2,3]", "map(select(. > 1))", &["[2,3]"]),
        (r#"{"a":1,"b":2}"#, "map(. + 1)", &["[2,3]"]),
        (r#"{"a":1}"#, r#"has("a"), has("b")"#, &["true", "false"]),
        ("[1,2]", "has(0), has(2)", &["true", "false"]),
        ("[true,false,null]", "map(not)", &["[false,true,true]"]),
        ("1", ". + 1", &["2"]),
        ("[1,2]", ".[0] + .[1]", &["3"]),
        (r#""a""#, r#". + "b""#, &[r#""ab""#]),
        ("[1]", ". + [2]", &["[1,2]"]),
        (r#"{"a":1}"#, r#". + {"b":2}"#, &[r#"{"a":1,"b":2}"#]),
        (r#"{"a":{"x":1}}"#, r#". * {"a":{"y":2}}"#, &[r#"{"a":{"x":1,"y":2}}"#]),
        ("null", ". + 1", &["1"]),
        ("[1,2,3]", ". - [2]", &["[1,3]"]),
        ("10", ". / 4", &["2.5"]),
        ("10", ". / 5", &["2"]),
        ("7", ". % 3", &["1"]),
        ("-7", ". % 3", &["-1"]),
        ("5", "-.", &["-5"]),
        ("1", "(1, 2) + (10, 20)", &["11", "12", "21", "22"]),
        ("null", r#". // 5"#, &["5"]),
        ("0", ". // 5", &["0"]),
        ("false", ". // 5", &["5"]),
        (r#"{"a":null}"#, r#".a // "d""#, &[r#""d""#]),
        ("[1,null,2]", ".[] | values", &["1", "2"]),
        ("[1,2,3]", ".[] | . == 2", &["false", "true", "false"]),
        ("1", "1 < 2, 2 < 1, 1 <= 1, 1 != 1, 1 >= 2, 3 > 2", &["true", "false", "true", "false", "false", "true"]),
        ("1", r#""a" < "b", [1] < [2], null < false, {} > []"#, &["true", "true", "true", "true"]),
        ("1", "true and false, true or false, (true, false) and true", &["false", "true", "true", "false"]),
        ("1", "null and 1, 0 and 1, false or null", &["false", "true", "false"]),
        ("[1,2,3]", "add", &["6"]),
        (r#"["a","b"]"#, "add", &[r#""ab""#]),
        ("[]", "add", &["null"]),
        ("[[1],[2]]", "add", &["[1,2]"]),
        ("[1,2,3]", "first, last", &["1", "3"]),
        ("[]", "first", &["null"]),
        ("[[1,2],[3,[4]]]", "flatten", &["[1,2,3,4]"]),
        ("[[1,[2]],3]", "flatten(1)", &["[1,[2],3]"]),
        ("[1,2,2,3,1]", "unique", &["[1,2,3]"]),
        (r#"[{"k":2},{"k":1}]"#, "sort_by(.k)", &[r#"[{"k":1},{"k":2}]"#]),
        (r#"[{"k":1,"v":"a"},{"k":2,"v":"b"},{"k":1,"v":"c"}]"#, "group_by(.k) | map(length)", &["[2,1]"]),
        (r#"[{"k":1,"v":"a"},{"k":1,"v":"b"}]"#, "unique_by(.k) | length", &["1"]),
        ("[1,2,3]", "reverse", &["[3,2,1]"]),
        ("[3,1,2]", "min, max", &["1", "3"]),
        (r#"{"a":1,"b":2}"#, "to_entries", &[r#"[{"key":"a","value":1},{"key":"b","value":2}]"#]),
        (r#"[{"key":"a","value":1},{"name":"b","v":2}]"#, "from_entries", &[r#"{"a":1,"b":2}"#]),
        (r#"{"a":1,"b":2}"#, "with_entries({key: .key, value: (.value * 10)})", &[r#"{"a":10,"b":20}"#]),
        (r#"{"a":1,"b":2}"#, "map_values(. + 1)", &[r#"{"a":2,"b":3}"#]),
        ("[1,2,3]", "{a: .[0], b: .[2]}", &[r#"{"a":1,"b":3}"#]),
        (r#"{"a":1,"c":2}"#, "{a, b: .c}", &[r#"{"a":1,"b":2}"#]),
        (r#"{"a":1}"#, r#"{("x" + "y"): .a}"#, &[r#"{"xy":1}"#]),
        ("[1,2]", r#"{a: .[]}"#, &[r#"{"a":1}"#, r#"{"a":2}"#]),
        ("[1,2]", "[.[] | . * 10]", &["[10,20]"]),
        ("[1,2,3]", r#"if length > 2 then "big" else "small" end"#, &[r#""big""#]),
        ("2", r#"if . == 1 then "a" elif . == 2 then "b" else "c" end"#, &[r#""b""#]),
        ("3", r#"if . == 1 then "a" end"#, &["3"]),
        (r#""abc""#, "ascii_upcase", &[r#""ABC""#]),
        (r#""abc""#, r#"startswith("a"), endswith("x")"#, &["true", "false"]),
        (r#""a,b,c""#, r#"split(",")"#, &[r#"["a","b","c"]"#]),
        (r#"["a",1,null]"#, r#"join("-")"#, &[r#""a-1-""#]),
        (r#""abc""#, r#"ltrimstr("a"), rtrimstr("c")"#, &[r#""bc""#, r#""ab""#]),
        ("12", "tostring, (tostring | tonumber)", &[r#""12""#, "12"]),
        (r#""1.5""#, "tonumber", &["1.5"]),
        (r#"[null,true,1,"a",[],{}]"#, ".[] | type", &[r#""null""#, r#""boolean""#, r#""number""#, r#""string""#, r#""array""#, r#""object""#]),
        ("[1,2,3]", "any(. > 2), all(. > 0), any(. > 5), all(. > 1)", &["true", "true", "false", "false"]),
        ("[true,false]", "any, all", &["true", "false"]),
        ("1", ".[]?", &[]),
        ("[1,2]", "(.[] | select(. > 5)) // \"none\"", &[r#""none""#]),
        ("[1,2,3]", "limit(2; .[])", &["1", "2"]),
        ("[1,2,3]", "first(.[] | select(. > 1))", &["2"]),
        ("[1,2,3]", "[.[] | select(. > 1)] | length", &["2"]),
        (r#""{\"a\":[1]}""#, "fromjson | .a[0]", &["1"]),
        ("[1,2]", "tojson", &[r#""[1,2]""#]),
        ("[1,2,3]", ".[1:] | .[0]", &["2"]),
        (r#""héllo""#, ".[1:3]", &[r#""él""#]),
        (r#"{"a":[{"b":1},{"b":2}]}"#, "[.a[].b]", &["[1,2]"]),
        (r#"{"a":{"b":1}}"#, ".a | .. ", &[r#"{"b":1}"#, "1"]),
        ("2.7", "floor", &["2"]),
        ("[1,2]", "map(select(. > 5)) | length", &["0"]),
        ("1", "# a comment\n. + 1", &["2"]),
    ];
    assert!(table.len() >= 40);
    for (doc, q, want) in table {
        check(doc, q, want);
    }
}

#[test]
fn query_runtime_errors_read_like_jq() {
    let err = |doc: &str, q: &str| query(doc, q).unwrap_err();
    assert_eq!(err("1", ".a"), "Cannot index number with “a”");
    assert_eq!(err("null", ".[]"), "Cannot iterate over null (null)");
    assert_eq!(err("[1]", ".a"), "Cannot index array with string");
    assert_eq!(err(r#"{"a":1}"#, ".[0]"), "Cannot index object with number");
    assert_eq!(err("1", "1 / 0"), "number (1) and number (0) cannot be divided because the divisor is zero");
    assert_eq!(err("{}", ". + 1"), "object ({}) and number (1) cannot be added");
    assert_eq!(err(r#""a""#, "keys"), "string (\"a\") has no keys");
    assert_eq!(err(r#""a""#, "tonumber"), "Cannot parse \"a\" as a number");
    assert!(err("true", "length").contains("has no length"));
    assert_eq!(query("1", ".a?"), Ok(vec![]));
    assert_eq!(query("1", "try .a"), Ok(vec![]));
    assert_eq!(query("[1,\"a\"]", ".[] | (. + 1)?"), Ok(vec![json!(2)]));
}

#[test]
fn query_syntax_errors_point_at_the_problem() {
    let at = |q: &str| crate::datalens::parse_query(q).err().map(|e| (e.offset, e.message));
    assert_eq!(at(".a |"), Some((Some(4), "The query ends too early".into())));
    assert_eq!(at(".a )"), Some((Some(3), "Unexpected “)”".into())));
    assert_eq!(at(".[1"), Some((Some(3), "Expected “]”".into())));
    assert_eq!(at("frobnicate"), Some((Some(0), "“frobnicate” is not a function this query language has".into())));
    assert_eq!(at(".a | map(.b"), Some((Some(11), "Expected “)”".into())));
    assert_eq!(at("\"abc"), Some((Some(0), "The string is not closed".into())));
    assert_eq!(at("$x"), Some((Some(0), "Variables are not supported".into())));
    assert_eq!(at("map(.a; .b)"), Some((Some(0), "“map” is called with the wrong number of arguments".into())));
    assert_eq!(at("input"), Some((Some(0), "“input” is not supported".into())));
    assert_eq!(at("env"), Some((Some(0), "“env” is not supported".into())));
    assert_eq!(at("\"a\\(1)\""), Some((Some(2), "String interpolation is not supported".into())));
    assert!(at("[".repeat(100).as_str()).is_some(), "deep nesting is an error, not a stack overflow");
    assert!(at(&format!("{}1", ".|".repeat(1000))).is_some(), "long chains are refused");
    assert!(crate::datalens::parse_query("").is_ok() && crate::datalens::parse_query("  ").is_ok(), "an empty program is `.`");
}

#[test]
fn query_has_no_way_out_of_the_document() {
    for q in [
        "input",
        "inputs",
        "$ENV",
        "env",
        "input_filename",
        "now",
        "debug",
        "stderr",
        "halt",
        "import \"a\" as a; .",
        "include \"a\"; .",
        "@sh",
        "$__loc__",
        "getpath",
        "system(\"id\")",
    ] {
        assert!(crate::datalens::parse_query(q).is_err(), "{q}");
    }
}

#[test]
fn query_is_bounded_in_work_and_size() {
    // Quadratic blow-up: every element paired with every element.
    let big = Value::Array((0..3000).map(Value::from).collect());
    let q = crate::datalens::parse_query("[.[] + .[]] | length").unwrap();
    let err = q.run(&big, &|| false).unwrap_err();
    assert!(err.message.contains("more work than allowed"), "{err:?}");
    let err = crate::datalens::parse_query("\"x\" * 100000000").unwrap().run(&json!(null), &|| false).unwrap_err();
    assert!(err.message.contains("too long"), "{err:?}");
    // A newer query cancels the older one.
    let q = crate::datalens::parse_query("[.[] + .[]] | length").unwrap();
    let err = q.run(&big, &|| true).unwrap_err();
    assert!(err.message.is_empty(), "cancelled runs are silent: {err:?}");
}

#[test]
fn query_results_are_text_cut_at_five_megabytes() {
    let rows = Value::Array((0..400_000).map(|i| json!({"id": i, "name": "xxxxxxxxxxxxxxxx"})).collect());
    let out = run_query(&rows, ".[]", &Options::default(), &|| false).unwrap();
    assert!(out.truncated && out.text.len() <= crate::datalens::MAX_RESULT_BYTES && out.text.ends_with("}\n"), "{} bytes", out.text.len());
    assert_eq!(out.results, 400_000);
    // One huge result is cut at a line break instead.
    let out = run_query(&Value::Array((0..900_000).map(Value::from).collect()), ".", &Options::default(), &|| false).unwrap();
    assert!(out.truncated && out.text.len() <= crate::datalens::MAX_RESULT_BYTES && out.text.ends_with(",\n"), "{} bytes", out.text.len());
    let out = run_query(&j(r#"{"a":[1,2]}"#), ".a[]", &Options::default(), &|| false).unwrap();
    assert_eq!((out.text.as_str(), out.truncated), ("1\n2\n", false));
    let out = run_query(&j(r#"{"a":[1]}"#), ".", &Options { indent: 4, ..Options::default() }, &|| false).unwrap();
    assert_eq!(out.text, "{\n    \"a\": [\n        1\n    ]\n}\n");
}

// --- Paths ---

fn path(marked: &str, format: Format) -> String {
    let at = marked.find('|').expect("cursor marker");
    let text = marked.replacen('|', "", 1);
    path_at(&text, format, at).unwrap_or_else(|e| format!("error: {e}"))
}

#[test]
fn json_path_at_the_cursor() {
    assert_eq!(path(r#"{"servers": [{"host": "a"}, {"host": "b|"}]}"#, Format::Json), ".servers[1].host");
    assert_eq!(path(r#"{"servers": [{"host": "a"}, {"ho|st": "b"}]}"#, Format::Json), ".servers[1].host");
    assert_eq!(path(r#"{"a": [1, 2|, 3]}"#, Format::Json), ".a[1]");
    assert_eq!(path(r#"{"a": {"b": 1}|}"#, Format::Json), ".a");
    assert_eq!(path("{|}", Format::Json), ".");
    assert_eq!(path(r#"{"a b": {"c-d": [true|]}}"#, Format::Json), r#"."a b"."c-d"[0]"#);
    assert_eq!(path("|[1]", Format::Json), ".");
}

#[test]
fn paths_with_non_ascii_keys_and_text_before_the_cursor() {
    assert_eq!(path(r#"{"sunucular": [{"ad": "şğü😀"}, {"ağ": {"şifre": 1|}}]}"#, Format::Json), r#".sunucular[1]."ağ"."şifre""#);
    assert_eq!(path("{\"ğ\": \"ğğğ\", \"ü\": [\"ö\", \"ç|\"]}", Format::Json), r#"."ü"[1]"#);
    assert_eq!(path("sunucu:\n  - ad: şğü\n  - \"ğ ü\":\n      bağlantı: 8|0\n", Format::Yaml), r#".sunucu[1]."ğ ü"."bağlantı""#);
    assert_eq!(path("[servis]\nad = \"ğ\"\n\n[servis.\"ö z\"]\n\"kapı\" = 4|2\n", Format::Toml), r#".servis."ö z"."kapı""#);
}

#[test]
fn json_path_survives_half_typed_text() {
    assert_eq!(path(r#"{"a": {"b": [1, 2], "c": |"#, Format::Json), ".a.c");
    assert_eq!(path(r#"{"a": [{"x": 1}, {"y": |"#, Format::Json), ".a[1].y");
}

#[test]
fn yaml_path_at_the_cursor() {
    let doc = "name: ci\njobs:\n  build:\n    steps:\n      - run: make\n      - uses: act|ions/x@v4\n        with:\n          path: dist\n";
    assert_eq!(path(doc, Format::Yaml), ".jobs.build.steps[1].uses");
    assert_eq!(path("a:\n  b: 1\n  |c: 2\n", Format::Yaml), ".a.c");
    assert_eq!(path("a:\n  b: [1, 2|, 3]\n", Format::Yaml), ".a.b[1]");
    assert_eq!(path("top: 1\n---\nsecond:\n  x: |5\n", Format::Yaml), ".second.x");
    assert_eq!(path("k|ey: 1\n", Format::Yaml), ".key");
    assert!(path("a: [1\nb: |2\n", Format::Yaml).starts_with("error"), "YAML needs a valid document");
}

#[test]
fn toml_path_at_the_cursor() {
    let doc = "title = \"x\"\n\n[servers.alpha]\nip = \"10.0.0.1\"\nports = [80, 4|43]\n\n[[fleet]]\nname = \"a\"\n\n[[fleet]]\nname = \"b\"\n";
    assert_eq!(path(doc, Format::Toml), ".servers.alpha.ports[1]");
    assert_eq!(path("[[fleet]]\nname = \"a\"\n[[fleet]]\nname = \"b|\"\n", Format::Toml), ".fleet[1].name");
    assert_eq!(path("a.b.c = 1|\n", Format::Toml), ".a.b.c");
    assert_eq!(path("tit|le = 1\n", Format::Toml), ".title");
    assert_eq!(path("t = { x = 1, y = [2|] }\n", Format::Toml), ".t.y[0]");
    assert_eq!(path("a = |", Format::Toml).chars().take(5).collect::<String>(), "error");
}

// --- Decoding ---

const JWT: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";

#[test]
fn jwt_is_decoded_and_never_called_verified() {
    let lines = decode(JWT).unwrap();
    assert!(lines[0].starts_with("Signature not verified"), "{lines:?}");
    let text = lines.join("\n");
    for want in ["\"alg\": \"HS256\"", "\"name\": \"John Doe\"", "\"sub\": \"1234567890\"", "iat is a date:", "UTC    2018-01-18 01:30:22"] {
        assert!(text.contains(want), "{want}\n{text}");
    }
    assert!(!text.contains("SflKxw"), "the signature is not echoed");
    let bearer = decode(&format!("  \"Bearer {JWT}\" \n")).unwrap();
    assert_eq!(bearer, lines);
    let unsigned = decode("eyJhbGciOiJub25lIn0.eyJhIjoxfQ.").unwrap();
    assert!(unsigned.join("\n").contains("no signature"));
}

#[test]
fn epoch_values_in_seconds_and_milliseconds() {
    let lines = decode("1700000000").unwrap();
    assert_eq!(lines[0], "1700000000 (seconds)");
    assert_eq!(lines[1], "  UTC    2023-11-14 22:13:20");
    let lines = decode("1700000000123").unwrap();
    assert_eq!(lines[0], "1700000000123 (milliseconds)");
    assert_eq!(lines[1], "  UTC    2023-11-14 22:13:20.123");
    assert!(lines[2].trim_start().starts_with("Local") || lines[2].contains("not available"), "{lines:?}");
    let many = decode("0, 86400\n951782400").unwrap();
    assert_eq!(many.iter().filter(|l| l.contains("UTC")).count(), 3);
    assert!(many.join("\n").contains("2000-02-29 00:00:00"));
    assert_eq!(crate::datalens_decode::civil_for_test(-1), "1969-12-31 23:59:59");
}

#[test]
fn decode_refuses_what_it_does_not_know() {
    for bad in ["", "   ", "hello", "12abc", "1e5", "99999999999999999", "a.b.c"] {
        assert!(decode(bad).is_err(), "{bad:?}");
    }
}

// --- Speed ---

#[test]
fn converting_five_megabytes_is_fast() {
    let rows: Vec<String> = (0..40_000)
        .map(|i| format!(r#"{{"id": {i}, "name": "item number {i}", "tags": ["a", "b", "c"], "price": {i}.25, "active": true, "owner": {{"first": "Ada", "last": "Lovelace"}}}}"#))
        .collect();
    let json = format!("{{\"items\": [{}]}}", rows.join(", "));
    assert!(json.len() > 5_000_000, "{}", json.len());
    // Debug builds are an order of magnitude slower than the 150 ms target; the target is checked with --release.
    let limit = if cfg!(debug_assertions) { 3000 } else { 150 };
    for to in [Format::Yaml, Format::Toml, Format::Json] {
        let start = std::time::Instant::now();
        let (out, _) = convert(&json, Format::Json, to).unwrap();
        let ms = start.elapsed().as_millis();
        eprintln!("JSON -> {to:?}: {} bytes in {ms} ms", out.len());
        assert!(ms < limit, "JSON -> {to:?} took {ms} ms");
    }
}

// --- Random input ---

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn pick<'a>(&mut self, list: &[&'a str]) -> &'a str {
        list[(self.next() % list.len() as u64) as usize]
    }
}

const PIECES: &[&str] = &[
    "{",
    "}",
    "[",
    "]",
    ":",
    ",",
    "\"",
    "'",
    "a",
    "key",
    "1",
    "-",
    "0.5",
    "1e999",
    "null",
    "true",
    " ",
    "  ",
    "\n",
    "\t",
    "#",
    "//",
    "/*",
    "*/",
    "&x",
    "*x",
    "---",
    "- ",
    "? ",
    "= ",
    "=",
    ".",
    "..",
    "|",
    ">",
    "\\",
    "ğ",
    "😀",
    "\u{feff}",
    "\0",
    "<<",
    "[a]",
    "[[b]]",
    "a.b = ",
    "export ",
    "\\u12",
    "!!str",
    "%",
    "@",
    "$",
    ".a",
    ".[]",
    "| select(",
    ")",
    "(",
    "map(",
    "and",
    "or",
    "if",
    "then",
    "end",
    "//",
    ";",
    "\"\\(",
    "[:",
    "1:2]",
    "9223372036854775808",
    "18446744073709551616",
    "2024-01-01T00:00:00Z",
];

fn random_text(rng: &mut Rng) -> String {
    let n = (rng.next() % 40) as usize;
    (0..n).map(|_| rng.pick(PIECES)).collect()
}

#[test]
fn random_text_never_panics_or_hangs() {
    let doc = j(r#"{"a": [1, 2, {"b": null}], "c": "text", "d": {"e": [[1], [2, 3]]}}"#);
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let started = std::time::Instant::now();
    for _ in 0..4000 {
        let text = random_text(&mut rng);
        for format in Format::ALL {
            if let Ok(loaded) = parse(&text, format) {
                for to in Format::ALL {
                    let _ = emit(&loaded.value, to, &Options { flatten_env: true, ..Options::default() });
                }
            }
            let offset = (rng.next() % (text.len() as u64 + 2)) as usize;
            let _ = path_at(&text, format, offset);
        }
        let _ = parse_any(&text, None);
        let _ = decode(&text);
        let _ = run_query(&doc, &text, &Options::default(), &|| false);
    }
    // Structured nesting: deep arrays, many keys, long chains.
    for depth in [10, 100, 1000, 100_000] {
        let text = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        let _ = parse(&text, Format::Json);
        let _ = parse(&text, Format::Yaml);
        let _ = path_at(&text, Format::Json, depth);
        let _ = path_at(&text, Format::Yaml, depth);
        let _ = run_query(&doc, &text, &Options::default(), &|| false);
        let _ = run_query(&doc, &"(".repeat(depth), &Options::default(), &|| false);
        let _ = run_query(&doc, &"-".repeat(depth), &Options::default(), &|| false);
        let nested = format!("{}1{}", "a:\n ".repeat(depth.min(500)), "");
        let _ = parse(&nested, Format::Yaml);
    }
    assert!(started.elapsed().as_secs() < 120, "random input took {:?}", started.elapsed());
}
