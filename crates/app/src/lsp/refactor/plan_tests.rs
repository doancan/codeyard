//! Refactoring plans: what a server's edit becomes, and every way it is refused.

use std::collections::{HashMap, HashSet};

use super::*;
use crate::lsp::refactor::edit::parse;
use crate::replace_plan::Version;
use serde_json::{Value, json};

const RENAME_FN: &str = include_str!("../../../testdata/refactor/rename_fn.json");
const RENAME_MOD: &str = include_str!("../../../testdata/refactor/rename_mod.json");
const MAIN_RS: &str = include_str!("../../../testdata/refactor/demo/src/main.rs");
const UTIL_RS: &str = include_str!("../../../testdata/refactor/demo/src/util.rs");
const AYAR_RS: &str = include_str!("../../../testdata/refactor/demo/src/ayar.rs");
const RAPOR_RS: &str = include_str!("../../../testdata/refactor/demo/src/rapor.rs");

/// A host in memory. Folders are implied by the files and the `dirs` set.
#[derive(Default)]
struct FakeEnv {
    files: HashMap<String, Vec<u8>>,
    dirs: HashSet<String>,
    /// Link path prefix -> where it really points.
    links: Vec<(String, String)>,
    open: HashMap<String, OpenDoc>,
}

impl FakeEnv {
    fn demo() -> Self {
        let mut env = FakeEnv::default();
        env.dirs.extend(["/ws/demo".to_string(), "/ws/demo/src".to_string()]);
        env.put("/ws/demo/src/main.rs", MAIN_RS);
        env.put("/ws/demo/src/util.rs", UTIL_RS);
        env.put("/ws/demo/src/ayar.rs", AYAR_RS);
        // The captured file has CRLF line ends; the fixtures are LF so Windows checkouts do not change them.
        env.put("/ws/demo/src/rapor.rs", &RAPOR_RS.replace('\n', "\r\n"));
        env
    }

    fn put(&mut self, path: &str, text: &str) {
        self.files.insert(path.to_string(), text.as_bytes().to_vec());
    }

    fn real(&self, path: &str) -> String {
        let path = clean(path);
        for (from, to) in &self.links {
            if let Some(rest) = path.strip_prefix(from.as_str()) {
                return format!("{to}{rest}");
            }
        }
        path
    }
}

impl Env for FakeEnv {
    fn open_doc(&self, path: &str) -> Option<&OpenDoc> {
        self.open.get(path)
    }
    fn kind(&self, path: &str) -> Kind {
        let path = self.real(path);
        if self.files.contains_key(&path) {
            Kind::File
        } else if self.dirs.contains(&path) {
            Kind::Dir
        } else {
            Kind::Missing
        }
    }
    fn read(&self, path: &str) -> Result<(Vec<u8>, u64), String> {
        let bytes = self.files.get(&self.real(path)).ok_or_else(|| "not found".to_string())?;
        Ok((bytes.clone(), proto::content_hash(bytes)))
    }
    fn resolve(&self, path: &str) -> String {
        self.real(path)
    }
}

fn roots() -> Vec<String> {
    vec!["/ws/demo".to_string()]
}

fn plan(edit: &Value, env: &FakeEnv) -> RefactorPlan {
    build(parse(edit), env, &roots(), "local".into(), "/ws/demo".into(), Encoding::Utf8).unwrap()
}

fn text_edit(path: &str, version: Value, edits: Value) -> Value {
    json!({ "textDocument": { "uri": format!("file://{path}"), "version": version }, "edits": edits })
}

fn replace(l0: u32, c0: u32, l1: u32, c1: u32, new: &str) -> Value {
    json!({ "range": { "start": { "line": l0, "character": c0 }, "end": { "line": l1, "character": c1 } }, "newText": new })
}

fn file<'a>(plan: &'a RefactorPlan, path: &str) -> &'a FileItem {
    plan.files().find(|f| f.file.path == path).unwrap_or_else(|| panic!("no {path}"))
}

fn written(plan: &RefactorPlan, path: &str) -> String {
    let f = &file(plan, path).file;
    String::from_utf8(f.output_bytes(&vec![true; f.changes.len()]).unwrap()).unwrap()
}

fn doc(text: &str) -> OpenDoc {
    OpenDoc {
        text: text.into(),
        epoch: 1,
        version: 5,
        encoding: Encoding::Utf8,
        dirty: false,
        synced: true,
        server_version: Some(1),
        edited_since_request: false,
    }
}

fn problem(plan: &RefactorPlan, path: &str) -> String {
    file(plan, path).file.problem.as_ref().unwrap_or_else(|| panic!("{path} is not refused")).message()
}

// --- A real rust-analyzer answer ---

#[test]
fn a_real_rename_becomes_the_four_files_with_exact_new_text() {
    let env = FakeEnv::demo();
    let p = plan(&serde_json::from_str(RENAME_FN).unwrap(), &env);
    assert_eq!(p.files().count(), 4);
    assert_eq!(p.change_count(), 9);
    // Every file is selected from the start and none is refused.
    assert!(p.files().all(|f| f.file.problem.is_none()));
    // Independent check: the same text with the call sites renamed by hand. The comment and the string keep the old name.
    let util = written(&p, "/ws/demo/src/util.rs");
    assert_eq!(util, UTIL_RS.replace("fn hesapla", "fn topla").replace("    hesapla(x, x)", "    topla(x, x)"));
    assert!(util.contains("// 🦀🦀 hesapla çağrısı"), "{util}");
    // Turkish letters and an emoji before the symbol on the same line: the UTF-16 columns land on the right bytes.
    let ayar = written(&p, "/ws/demo/src/ayar.rs");
    assert!(ayar.contains(r#"let ş = "şğüıöç 😀"; let _ = topla(1, 2); let _ = ş;"#), "{ayar}");
    assert_eq!(ayar.matches("topla").count(), 3);
    assert_eq!(ayar.matches("hesapla").count(), 0);
    let main = written(&p, "/ws/demo/src/main.rs");
    assert!(main.contains("let sonuç = util::topla(2, 3);"));
    assert!(main.contains("println!(\"🦀 sonuç = {}\", sonuç);"));
}

#[test]
fn crlf_files_keep_their_line_endings_and_their_lines_untouched() {
    let env = FakeEnv::demo();
    let p = plan(&serde_json::from_str(RENAME_FN).unwrap(), &env);
    let rapor = written(&p, "/ws/demo/src/rapor.rs");
    assert_eq!(rapor, "use crate::util::topla;\r\n\r\npub fn özet() -> i32 {\r\n    topla(7, 8)\r\n}\r\n");
}

#[test]
fn a_utf8_byte_order_mark_and_a_latin1_file_survive() {
    let mut env = FakeEnv::demo();
    env.files.insert("/ws/demo/src/bom.rs".into(), b"\xEF\xBB\xBFfn \xC3\xA7() {}\n".to_vec());
    let edit = json!({ "documentChanges": [text_edit("/ws/demo/src/bom.rs", Value::Null, json!([replace(0, 3, 0, 4, "ş")]))] });
    let p = plan(&edit, &env);
    assert_eq!(file(&p, "/ws/demo/src/bom.rs").file.output_bytes(&[true]).unwrap(), b"\xEF\xBB\xBFfn \xC5\x9F() {}\n");
    // Latin-1 bytes decode with the fallback encoding, and a replacement it cannot store refuses the file.
    env.files.insert("/ws/demo/src/l1.txt".into(), b"caf\xE9 = 1\n".to_vec());
    let edit = json!({ "documentChanges": [text_edit("/ws/demo/src/l1.txt", Value::Null, json!([replace(0, 0, 0, 3, "ba\u{20ac}")]))] });
    let p = build(parse(&edit), &env, &roots(), "local".into(), "/ws/demo".into(), Encoding::Iso8859_1).unwrap();
    assert!(file(&p, "/ws/demo/src/l1.txt").file.problem.is_some());
    let edit = json!({ "documentChanges": [text_edit("/ws/demo/src/l1.txt", Value::Null, json!([replace(0, 0, 0, 3, "bar")]))] });
    let p = build(parse(&edit), &env, &roots(), "local".into(), "/ws/demo".into(), Encoding::Iso8859_1).unwrap();
    assert_eq!(file(&p, "/ws/demo/src/l1.txt").file.output_bytes(&[true]).unwrap(), b"bar\xE9 = 1\n");
}

#[test]
fn a_real_module_rename_is_a_row_of_its_own_and_starts_unselected() {
    let env = FakeEnv::demo();
    let p = plan(&serde_json::from_str(RENAME_MOD).unwrap(), &env);
    let renames: Vec<_> = p.resources().collect();
    assert_eq!(renames.len(), 1);
    assert_eq!(renames[0].op, Resource::Rename { from: "/ws/demo/src/util.rs".into(), to: "/ws/demo/src/araçlar.rs".into() });
    assert!(renames[0].problem.is_none());
    let chosen = Chosen::initial(&p);
    let rename_at = p.items.iter().position(|i| matches!(i, Item::Resource(_))).unwrap();
    assert_eq!(chosen.item(rename_at), [false], "a file operation is chosen on purpose");
    assert!(p.files().all(|f| f.needs.is_none()));
    assert_eq!(chosen.counts().0, p.change_count());
    assert!(needs_review(&p, ReviewMode::Never), "file operations are always reviewed");
}

// --- Positions ---

#[test]
fn columns_count_utf16_units_so_an_emoji_is_two() {
    let mut env = FakeEnv::demo();
    env.put("/ws/demo/src/e.rs", "let a = \"😀ş\"; x\n");
    // 😀 is two units: "let a = \"" is 9, the emoji 9..11, ş 11..12, the quote 12.
    let ok = json!({ "documentChanges": [text_edit("/ws/demo/src/e.rs", Value::Null, json!([replace(0, 11, 0, 12, "s")]))] });
    assert_eq!(written(&plan(&ok, &env), "/ws/demo/src/e.rs"), "let a = \"😀s\"; x\n");
    // Column 10 is between the two halves of the emoji.
    let bad = json!({ "documentChanges": [text_edit("/ws/demo/src/e.rs", Value::Null, json!([replace(0, 10, 0, 11, "s")]))] });
    assert!(problem(&plan(&bad, &env), "/ws/demo/src/e.rs").contains("inside a character"));
}

#[test]
fn a_column_past_the_line_is_the_end_of_the_line_and_a_line_past_the_text_is_refused() {
    let mut env = FakeEnv::demo();
    env.put("/ws/demo/src/e.rs", "ab\ncd\n");
    let end_of_line = json!({ "documentChanges": [text_edit("/ws/demo/src/e.rs", Value::Null, json!([replace(0, 1, 0, 4_000_000_000, "X")]))] });
    assert_eq!(written(&plan(&end_of_line, &env), "/ws/demo/src/e.rs"), "aX\ncd\n");
    // Line 2 is the empty line after the last line break: the end of the text.
    let end_of_text = json!({ "documentChanges": [text_edit("/ws/demo/src/e.rs", Value::Null, json!([replace(2, 0, 2, 0, "tail")]))] });
    assert_eq!(written(&plan(&end_of_text, &env), "/ws/demo/src/e.rs"), "ab\ncd\ntail");
    let past = json!({ "documentChanges": [text_edit("/ws/demo/src/e.rs", Value::Null, json!([replace(4, 0, 4, 0, "tail")]))] });
    assert!(problem(&plan(&past, &env), "/ws/demo/src/e.rs").contains("does not exist"));
    let backwards = json!({ "documentChanges": [text_edit("/ws/demo/src/e.rs", Value::Null, json!([replace(1, 1, 0, 1, "x")]))] });
    assert!(problem(&plan(&backwards, &env), "/ws/demo/src/e.rs").contains("ends before it starts"));
}

#[test]
fn all_three_line_endings_are_lines() {
    let mut env = FakeEnv::demo();
    env.put("/ws/demo/src/m.rs", "a\r\nb\rc\nd");
    let edit = json!({ "documentChanges": [text_edit("/ws/demo/src/m.rs", Value::Null, json!([
        replace(0, 0, 0, 1, "A"), replace(1, 0, 1, 1, "B"), replace(2, 0, 2, 1, "C"), replace(3, 0, 3, 1, "D"),
    ]))] });
    assert_eq!(written(&plan(&edit, &env), "/ws/demo/src/m.rs"), "A\r\nB\rC\nD");
}

#[test]
fn a_multi_line_replacement_takes_the_files_line_ending() {
    let env = FakeEnv::demo();
    let edit = json!({ "documentChanges": [text_edit("/ws/demo/src/rapor.rs", Value::Null, json!([replace(3, 4, 3, 4, "let x = 1;\n    ")]))] });
    assert_eq!(written(&plan(&edit, &env), "/ws/demo/src/rapor.rs"), RAPOR_RS.replace('\n', "\r\n").replace("    hesapla", "    let x = 1;\r\n    hesapla"));
}

#[test]
fn overlapping_edits_refuse_the_file_but_inserts_next_to_a_replacement_do_not() {
    let mut env = FakeEnv::demo();
    env.put("/ws/demo/src/o.rs", "0123456789\n");
    let overlap = json!({ "documentChanges": [text_edit("/ws/demo/src/o.rs", Value::Null, json!([replace(0, 2, 0, 6, "x"), replace(0, 5, 0, 8, "y")]))] });
    assert!(problem(&plan(&overlap, &env), "/ws/demo/src/o.rs").contains("overlap"));
    let inside = json!({ "documentChanges": [text_edit("/ws/demo/src/o.rs", Value::Null, json!([replace(0, 2, 0, 6, "x"), replace(0, 4, 0, 4, "y")]))] });
    assert!(problem(&plan(&inside, &env), "/ws/demo/src/o.rs").contains("overlap"));
    let same = json!({ "documentChanges": [text_edit("/ws/demo/src/o.rs", Value::Null, json!([replace(0, 2, 0, 6, "x"), replace(0, 2, 0, 6, "x")]))] });
    assert!(problem(&plan(&same, &env), "/ws/demo/src/o.rs").contains("overlap"));
    // Inserts at the start and end of a replaced range, and two inserts in one place (their order is the server's).
    let fine = json!({ "documentChanges": [text_edit("/ws/demo/src/o.rs", Value::Null, json!([
        replace(0, 6, 0, 6, "<end>"), replace(0, 2, 0, 6, "-"), replace(0, 2, 0, 2, "<a>"), replace(0, 2, 0, 2, "<b>"),
    ]))] });
    assert_eq!(written(&plan(&fine, &env), "/ws/demo/src/o.rs"), "01<a><b>-<end>6789\n");
}

#[test]
fn edits_are_sorted_into_text_order_whatever_order_the_server_sent() {
    let mut env = FakeEnv::demo();
    env.put("/ws/demo/src/o.rs", "abcdef\n");
    let edit = json!({ "documentChanges": [text_edit("/ws/demo/src/o.rs", Value::Null, json!([replace(0, 4, 0, 5, "E"), replace(0, 0, 0, 1, "A")]))] });
    let p = plan(&edit, &env);
    assert_eq!(written(&p, "/ws/demo/src/o.rs"), "AbcdEf\n");
    assert_eq!(file(&p, "/ws/demo/src/o.rs").file.changes.iter().map(|c| c.line).collect::<Vec<_>>(), [1, 1]);
}

#[test]
fn two_entries_for_one_file_are_one_row_and_their_versions_must_agree() {
    let mut env = FakeEnv::demo();
    env.put("/ws/demo/src/o.rs", "abc\n");
    let edit = json!({ "documentChanges": [
        text_edit("/ws/demo/src/o.rs", Value::Null, json!([replace(0, 0, 0, 1, "A")])),
        text_edit("/ws/demo/src/o.rs", Value::Null, json!([replace(0, 2, 0, 3, "C")])),
    ] });
    let p = plan(&edit, &env);
    assert_eq!(p.files().count(), 1);
    assert_eq!(written(&p, "/ws/demo/src/o.rs"), "AbC\n");
    let edit = json!({ "documentChanges": [
        text_edit("/ws/demo/src/o.rs", json!(3), json!([replace(0, 0, 0, 1, "A")])),
        text_edit("/ws/demo/src/o.rs", json!(4), json!([replace(0, 2, 0, 3, "C")])),
    ] });
    assert!(problem(&plan(&edit, &env), "/ws/demo/src/o.rs").contains("two different document versions"));
}

// --- Paths a server must not reach ---

#[test]
fn a_file_outside_the_open_folders_is_refused_with_its_real_location() {
    let mut env = FakeEnv::demo();
    env.put("/etc/passwd", "root\n");
    env.put("/home/ada/.ssh/config", "Host x\n");
    let r = replace(0, 0, 0, 1, "X");
    for (uri_path, shown) in
        [("/etc/passwd", "/etc/passwd"), ("/ws/demo/../../etc/passwd", "/etc/passwd"), ("/ws/demo/src/../../../home/ada/.ssh/config", "/home/ada/.ssh/config")]
    {
        let edit = json!({ "documentChanges": [text_edit(uri_path, Value::Null, json!([r.clone()]))] });
        let p = plan(&edit, &env);
        let why = problem(&p, uri_path);
        assert!(why.starts_with("Outside the open folders") && why.contains(shown), "{why}");
        assert!(file(&p, uri_path).file.output_bytes(&[true]).is_err());
        assert_eq!(Chosen::initial(&p).counts().0, 0, "a refused file cannot be selected");
    }
}

#[test]
fn a_symbolic_link_inside_the_folder_that_points_out_is_refused_and_its_target_is_shown() {
    let mut env = FakeEnv::demo();
    env.put("/secret/keys.txt", "k\n");
    env.links.push(("/ws/demo/link".into(), "/secret".into()));
    let edit = json!({ "documentChanges": [text_edit("/ws/demo/link/keys.txt", Value::Null, json!([replace(0, 0, 0, 1, "X")]))] });
    let p = plan(&edit, &env);
    let why = problem(&p, "/ws/demo/link/keys.txt");
    assert!(why.contains("/secret/keys.txt"), "{why}");
}

#[test]
fn a_symbolic_link_to_another_open_folder_is_allowed() {
    let mut env = FakeEnv::demo();
    env.put("/other/lib.rs", "abc\n");
    env.links.push(("/ws/demo/lib".into(), "/other".into()));
    let edit = json!({ "documentChanges": [text_edit("/ws/demo/lib/lib.rs", Value::Null, json!([replace(0, 0, 0, 1, "X")]))] });
    let p = build(parse(&edit), &env, &["/ws/demo".into(), "/other".into()], "local".into(), "/ws/demo".into(), Encoding::Utf8).unwrap();
    assert!(file(&p, "/ws/demo/lib/lib.rs").file.problem.is_none());
}

#[test]
fn a_document_open_in_an_editor_is_edited_there_even_outside_the_folders() {
    let mut env = FakeEnv::demo();
    env.open.insert("/home/ada/.cargo/registry/dep/src/lib.rs".into(), doc("abc\n"));
    let edit = json!({ "documentChanges": [text_edit("/home/ada/.cargo/registry/dep/src/lib.rs", json!(1), json!([replace(0, 0, 0, 1, "X")]))] });
    let p = plan(&edit, &env);
    assert!(matches!(file(&p, "/home/ada/.cargo/registry/dep/src/lib.rs").file.version, Version::Buffer { .. }));
    assert!(file(&p, "/home/ada/.cargo/registry/dep/src/lib.rs").file.problem.is_none());
}

#[test]
fn a_missing_file_or_a_folder_is_shown_as_unreadable() {
    let env = FakeEnv::demo();
    let edit = json!({ "documentChanges": [text_edit("/ws/demo/src/nope.rs", Value::Null, json!([replace(0, 0, 0, 1, "X")]))] });
    let p = plan(&edit, &env);
    assert!(matches!(file(&p, "/ws/demo/src/nope.rs").file.problem, Some(Problem::Unreadable(_))));
}

#[test]
fn a_malformed_edit_refuses_only_its_own_file() {
    let env = FakeEnv::demo();
    let edit = json!({ "documentChanges": [
        text_edit("/ws/demo/src/util.rs", Value::Null, json!([{ "newText": "x" }])),
        text_edit("/ws/demo/src/main.rs", Value::Null, json!([replace(0, 0, 0, 3, "pub mod")])),
    ] });
    let p = plan(&edit, &env);
    assert!(problem(&p, "/ws/demo/src/util.rs").contains("malformed"));
    assert!(file(&p, "/ws/demo/src/main.rs").file.problem.is_none());
}

// --- Open documents ---

#[test]
fn an_open_document_is_the_source_and_a_stale_one_is_refused() {
    let mut env = FakeEnv::demo();
    let edit = |v: Value| json!({ "documentChanges": [text_edit("/ws/demo/src/util.rs", v, json!([replace(0, 7, 0, 14, "topla")]))] });
    // In sync: the editor's text (not the disk's) is what the plan is made from.
    env.open.insert("/ws/demo/src/util.rs".into(), doc("pub fn hesapla() {}\n"));
    let p = plan(&edit(json!(1)), &env);
    assert!(matches!(file(&p, "/ws/demo/src/util.rs").file.version, Version::Buffer { epoch: 1, version: 5, .. }));
    assert_eq!(written(&p, "/ws/demo/src/util.rs"), "pub fn topla() {}\n");
    // The server computed against another version.
    let p = plan(&edit(json!(2)), &env);
    assert!(problem(&p, "/ws/demo/src/util.rs").contains("version 2"), "{}", problem(&p, "/ws/demo/src/util.rs"));
    // An unversioned edit does not claim a version.
    assert!(plan(&edit(Value::Null), &env).files().all(|f| f.file.problem.is_none()));
    // Typing continued while the server was answering.
    env.open.get_mut("/ws/demo/src/util.rs").unwrap().edited_since_request = true;
    assert!(problem(&plan(&edit(json!(1)), &env), "/ws/demo/src/util.rs").contains("edited while the language server"));
}

#[test]
fn unsaved_text_the_server_never_saw_refuses_the_file_but_a_synced_dirty_buffer_is_fine() {
    let mut env = FakeEnv::demo();
    let edit = json!({ "documentChanges": [text_edit("/ws/demo/src/util.rs", Value::Null, json!([replace(0, 7, 0, 14, "topla")]))] });
    env.open.insert("/ws/demo/src/util.rs".into(), OpenDoc { dirty: true, synced: false, ..doc("pub fn hesapla() {}\n") });
    assert!(problem(&plan(&edit, &env), "/ws/demo/src/util.rs").contains("unsaved changes"));
    env.open.insert("/ws/demo/src/util.rs".into(), OpenDoc { dirty: true, synced: true, ..doc("pub fn hesapla() {}\n") });
    assert!(plan(&edit, &env).files().all(|f| f.file.problem.is_none()));
    // Clean and not attached to the server: the buffer equals the disk, so it is simply used.
    env.open.insert("/ws/demo/src/util.rs".into(), OpenDoc { dirty: false, synced: false, ..doc("pub fn hesapla() {}\n") });
    assert!(plan(&edit, &env).files().all(|f| f.file.problem.is_none()));
}

// --- File operations ---

fn ops(entries: Value) -> Value {
    json!({ "documentChanges": entries })
}

#[test]
fn a_created_file_takes_edits_that_depend_on_it() {
    let env = FakeEnv::demo();
    let edit = ops(json!([
        { "kind": "create", "uri": "file:///ws/demo/src/new.rs" },
        text_edit("/ws/demo/src/new.rs", Value::Null, json!([replace(0, 0, 0, 0, "pub fn f() {}\n")])),
    ]));
    let p = plan(&edit, &env);
    let new = file(&p, "/ws/demo/src/new.rs");
    assert_eq!(new.needs, Some(0));
    assert_eq!(written(&p, "/ws/demo/src/new.rs"), "pub fn f() {}\n");
    let mut chosen = Chosen::initial(&p);
    assert_eq!(chosen.counts().0, 0, "the edit waits for the create");
    chosen.toggle_part(&p, 1, 0);
    assert_eq!(chosen.blocker(&p), Some(Blocker::NeedsOperation("/ws/demo/src/new.rs".into())));
    chosen.toggle_item(&p, 0);
    assert_eq!(chosen.blocker(&p), None);
    // Dropping the create drops the edit with it.
    chosen.toggle_item(&p, 0);
    assert_eq!(chosen.counts().0, 0);
    // Choosing it brings the edit back.
    chosen.toggle_item(&p, 0);
    assert_eq!(chosen.item(1), [true]);
}

#[test]
fn edits_to_a_file_moved_by_the_edit_are_read_from_where_it_was() {
    let env = FakeEnv::demo();
    let edit = ops(json!([
        { "kind": "rename", "oldUri": "file:///ws/demo/src/util.rs", "newUri": "file:///ws/demo/src/tools.rs" },
        text_edit("/ws/demo/src/tools.rs", Value::Null, json!([replace(0, 7, 0, 14, "topla")])),
    ]));
    let p = plan(&edit, &env);
    assert_eq!(file(&p, "/ws/demo/src/tools.rs").needs, Some(0));
    assert_eq!(written(&p, "/ws/demo/src/tools.rs"), UTIL_RS.replace("fn hesapla", "fn topla"));
    // The write is checked against the hash of the file at its old place.
    assert!(matches!(file(&p, "/ws/demo/src/tools.rs").file.version, Version::Disk { hash } if hash == proto::content_hash(UTIL_RS.as_bytes())));
}

#[test]
fn a_folder_rename_carries_the_edits_below_it() {
    let mut env = FakeEnv::demo();
    env.dirs.insert("/ws/demo/a".into());
    env.put("/ws/demo/a/x.rs", "abc\n");
    let edit = ops(json!([
        { "kind": "rename", "oldUri": "file:///ws/demo/a", "newUri": "file:///ws/demo/b" },
        text_edit("/ws/demo/b/x.rs", Value::Null, json!([replace(0, 0, 0, 1, "X")])),
    ]));
    let p = plan(&edit, &env);
    assert_eq!(file(&p, "/ws/demo/b/x.rs").needs, Some(0));
    assert_eq!(written(&p, "/ws/demo/b/x.rs"), "Xbc\n");
}

#[test]
fn operations_that_would_overwrite_are_refused_unless_the_server_said_to_ignore() {
    let env = FakeEnv::demo();
    let create = |opts: Value| ops(json!([{ "kind": "create", "uri": "file:///ws/demo/src/util.rs", "options": opts }]));
    for opts in [json!({}), json!({ "overwrite": true })] {
        let p = plan(&create(opts), &env);
        let r = p.resources().next().unwrap();
        assert!(r.problem.as_deref().is_some_and(|m| m.contains("never overwrites")), "{:?}", r.problem);
    }
    let p = plan(&create(json!({ "ignoreIfExists": true })), &env);
    let r = p.resources().next().unwrap();
    assert!(r.problem.is_none() && r.noop);
    let rename =
        |opts: Value| ops(json!([{ "kind": "rename", "oldUri": "file:///ws/demo/src/util.rs", "newUri": "file:///ws/demo/src/ayar.rs", "options": opts }]));
    assert!(plan(&rename(json!({ "overwrite": true })), &env).resources().next().unwrap().problem.is_some());
    assert!(plan(&rename(json!({ "ignoreIfExists": true })), &env).resources().next().unwrap().noop);
}

#[test]
fn rename_and_delete_check_their_sources_and_destinations() {
    let mut env = FakeEnv::demo();
    env.dirs.insert("/ws/demo/a".into());
    let one = |entry: Value| plan(&ops(json!([entry])), &env).resources().next().map(|r| (r.problem.clone(), r.noop)).unwrap();
    let rename = |from: &str, to: &str| json!({ "kind": "rename", "oldUri": format!("file://{from}"), "newUri": format!("file://{to}") });
    assert!(one(rename("/ws/demo/src/missing.rs", "/ws/demo/src/m2.rs")).0.unwrap().contains("not there"));
    assert!(one(rename("/ws/demo/a", "/ws/demo/a/b")).0.unwrap().contains("into itself"));
    assert!(one(rename("/ws/demo/src/util.rs", "/elsewhere/util.rs")).0.unwrap().contains("Outside"));
    assert_eq!(one(rename("/ws/demo/src/util.rs", "/ws/demo/src/util.rs")), (None, true));
    let delete = |path: &str, opts: Value| json!({ "kind": "delete", "uri": format!("file://{path}"), "options": opts });
    assert!(one(delete("/ws/demo/src/missing.rs", json!({}))).0.unwrap().contains("not there"));
    assert_eq!(one(delete("/ws/demo/src/missing.rs", json!({ "ignoreIfNotExists": true }))), (None, true));
    assert!(one(delete("/etc/hosts", json!({}))).0.unwrap().contains("Outside"));
    assert!(one(delete("/ws/demo", json!({ "recursive": true }))).0.unwrap().contains("open folder"));
    assert!(one(delete("/ws", json!({ "recursive": true }))).0.is_some(), "a folder above the open folder is not deletable either");
    assert_eq!(one(delete("/ws/demo/src/util.rs", json!({}))), (None, false));
}

#[test]
fn deleting_a_folder_and_a_file_with_unsaved_text_are_distinct_rows() {
    let mut env = FakeEnv::demo();
    env.open.insert("/ws/demo/src/util.rs".into(), OpenDoc { dirty: true, ..doc("x") });
    let p = plan(
        &ops(json!([
            { "kind": "delete", "uri": "file:///ws/demo/src/util.rs" },
            { "kind": "delete", "uri": "file:///ws/demo/src", "options": { "recursive": true } },
        ])),
        &env,
    );
    let rows: Vec<_> = p.resources().collect();
    assert!(rows[0].problem.as_deref().unwrap().contains("unsaved"));
    assert_eq!(rows[1].op, Resource::Delete { path: "/ws/demo/src".into(), dir: true });
}

#[test]
fn a_deleted_file_is_gone_for_the_operations_after_it() {
    let env = FakeEnv::demo();
    let p = plan(
        &ops(json!([
            { "kind": "delete", "uri": "file:///ws/demo/src/util.rs" },
            { "kind": "create", "uri": "file:///ws/demo/src/util.rs" },
        ])),
        &env,
    );
    assert!(p.resources().all(|r| r.problem.is_none()), "deleting then creating the same path works");
}

// --- Limits, summaries ---

#[test]
fn a_huge_edit_is_refused_with_its_counts() {
    let env = FakeEnv::demo();
    let many: Vec<Value> = (0..LIMITS.changes + 1).map(|_| replace(0, 0, 0, 0, "x")).collect();
    let edit = ops(json!([text_edit("/ws/demo/src/util.rs", Value::Null, Value::Array(many))]));
    let err = build(parse(&edit), &env, &roots(), "local".into(), "/ws/demo".into(), Encoding::Utf8).err().unwrap();
    assert_eq!(err, PlanError::TooLarge { files: 1, changes: LIMITS.changes + 1 });
    assert!(err.message().contains("too large"));
    let files: Vec<Value> = (0..LIMITS.files + 1).map(|i| text_edit(&format!("/ws/demo/f{i}.rs"), Value::Null, json!([]))).collect();
    let err = build(parse(&ops(Value::Array(files))), &env, &roots(), "local".into(), "/ws/demo".into(), Encoding::Utf8).err().unwrap();
    assert!(matches!(err, PlanError::TooLarge { files, .. } if files == LIMITS.files + 1));
}

#[test]
fn the_review_is_decided_by_the_setting_and_what_the_edit_touches() {
    let mut env = FakeEnv::demo();
    let one_open_file = ops(json!([text_edit("/ws/demo/src/util.rs", Value::Null, json!([replace(0, 7, 0, 14, "topla"), replace(5, 4, 5, 11, "topla")]))]));
    env.open.insert("/ws/demo/src/util.rs".into(), doc(UTIL_RS));
    let p = plan(&one_open_file, &env);
    assert!(!needs_review(&p, ReviewMode::Auto), "a rename inside one open file needs no extra click");
    assert!(!needs_review(&p, ReviewMode::Never));
    assert!(needs_review(&p, ReviewMode::Always));
    // One file that is not open has no editor undo.
    env.open.clear();
    assert!(needs_review(&plan(&one_open_file, &env), ReviewMode::Auto));
    // Several files.
    let many = plan(&serde_json::from_str(RENAME_FN).unwrap(), &env);
    assert!(needs_review(&many, ReviewMode::Auto));
    assert!(!needs_review(&many, ReviewMode::Never));
    // A refused file is shown whatever the setting says: nothing is dropped silently.
    let mut refused_env = FakeEnv::demo();
    refused_env.put("/etc/hosts", "x\n");
    let refused = plan(&ops(json!([text_edit("/etc/hosts", Value::Null, json!([replace(0, 0, 0, 1, "y")]))])), &refused_env);
    assert!(needs_review(&refused, ReviewMode::Never));
    // Nothing to change.
    let nothing = plan(&ops(json!([])), &env);
    assert!(!needs_review(&nothing, ReviewMode::Auto));
    assert_eq!(nothing.change_count(), 0);
}

#[test]
fn review_modes_parse_and_print_the_setting_values() {
    for (text, mode) in [("auto", ReviewMode::Auto), ("always", ReviewMode::Always), ("never", ReviewMode::Never)] {
        assert_eq!(ReviewMode::parse(text), Some(mode));
        assert_eq!(mode.as_str(), text);
    }
    assert_eq!(ReviewMode::parse("Always"), None);
    assert_eq!(ReviewMode::default(), ReviewMode::Auto);
    assert_eq!(ReviewMode::CHOICES.len(), 3);
}

#[test]
fn selection_toggles_changes_files_and_counts() {
    let env = FakeEnv::demo();
    let p = plan(&serde_json::from_str(RENAME_FN).unwrap(), &env);
    let mut chosen = Chosen::initial(&p);
    assert_eq!(chosen.counts(), (9, 4));
    assert_eq!(chosen.state(0), Tri::All);
    chosen.toggle_part(&p, 0, 1);
    assert_eq!(chosen.state(0), Tri::Some);
    assert_eq!(chosen.counts(), (8, 4));
    chosen.toggle_item(&p, 1);
    assert_eq!(chosen.state(1), Tri::None);
    chosen.toggle_item(&p, 1);
    assert_eq!(chosen.state(1), Tri::All);
    for i in 0..p.items.len() {
        chosen.0[i].iter_mut().for_each(|x| *x = false);
    }
    assert_eq!(chosen.blocker(&p), Some(Blocker::Nothing));
}

#[test]
fn path_helpers_resolve_dots_and_compare_whole_components() {
    assert_eq!(clean("/a/b/../c/./d"), "/a/c/d");
    assert_eq!(clean(r"C:\a\..\b"), "C:/b");
    assert_eq!(clean("a/../../b"), "b");
    assert!(inside("/ws/demo/src", "/ws/demo"));
    assert!(inside("/ws/demo", "/ws/demo/"));
    assert!(!inside("/ws/demo2/src", "/ws/demo"), "a longer name is not inside");
    assert!(!inside("/ws", "/ws/demo"));
    assert!(inside("/anything", "/"));
}
