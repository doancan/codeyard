//! Workspace file model: reading, path resolution, edits that keep the rest of the text, and hostile input.

use super::*;

const SAMPLE: &str = r#"// Shared by the whole team.
{
    /* the folders, in order */
    "folders": [
        { "path": "api", "name": "Backend ✨", "x-team": { "owner": "ada" } }, // the service
        { "path": "../web" },
        { "name": "broken entry" },
        { "uri": "file:///srv/my%20app" },
    ],
    "settings": { "editor.tabSize": 2, /* inline */ "files.insertFinalNewline": true },
    "tasks": { "version": "2.0.0", "tasks": [ { "label": "ws build", "command": "make" } ] },
    "launch": { "configurations": [ { "name": "ws run", "type": "python", "request": "launch", "program": "${workspaceFolder}/a.py" } ] },
    "extensions": { "recommendations": ["some.thing"] },
    "remoteAuthority": "ssh-remote+box",
}
"#;

fn doc(text: &str, dir: &str) -> Document {
    Document::parse(text, dir).unwrap_or_else(|e| panic!("{e}\n{text}"))
}

#[test]
fn resolves_relative_absolute_and_uri_folders() {
    let text = r#"{
        // yorum
        "folders": [
            { "path": "api" },
            { "path": "../web", "name": "Web" },
            { "path": "/opt/tools" },
            { "uri": "file:///srv/my%20app" },
            { "uri": "ssh-remote://box/home/x" },
            { "path": "./api/" },
            { "name": "no path" },
            { "path": "gone" }
        ],
        "settings": { "editor.tabSize": 2 }
    }"#;
    let all = |p: &str| !p.ends_with("/gone");
    assert_eq!(folders(text, "/work/mono", all), ["/work/mono/api", "/work/web", "/opt/tools", "/srv/my app"]);
    assert!(folders("{ not json", "/w", all).is_empty());
    assert!(folders("{}", "/w", all).is_empty());
}

#[test]
fn recognises_workspace_files() {
    assert!(is_workspace_file("/a/b.code-workspace"));
    assert!(!is_workspace_file("/a/b.json"));
}

#[test]
fn windows_workspace_folders_resolve_drives_and_backslashes() {
    let text = serde_json::json!({ "folders": [
        { "path": r"src\nested\.." }, { "path": r"..\web" },
        { "path": r"D:\tools\..\sdk" }, { "uri": "file:///E:/my%20app" },
        { "uri": "file://localhost/F:/work" },
        { "path": "C:/../../" }, { "path": r"\\?\G:\long\dir" },
        { "uri": "file:///bad%XX" }, { "uri": "file:///a%00b" }
    ] })
    .to_string();
    assert_eq!(folders(&text, r"C:\work\mono", |_| true), ["C:/work/mono/src", "C:/work/web", "D:/sdk", "E:/my app", "F:/work", "C:/", "G:/long/dir"]);
}

/// Looking at `\\host\share` makes Windows connect and offer the user's NTLM credentials to that host, so a workspace file from a
/// cloned repository must not make us look at a server the user did not choose.
#[test]
fn a_workspace_file_does_not_probe_network_shares_of_other_hosts() {
    let text = serde_json::json!({ "folders": [
        { "path": r"\\evil\share\dir" }, { "uri": "file://evil/share/other" }, { "path": r"\\?\UNC\evil\share\dir" },
        { "path": "//./pipe/x" }, { "path": "docs" }
    ] })
    .to_string();
    let asked = std::cell::RefCell::new(Vec::new());
    let found = folders(&text, r"C:\work", |p| {
        asked.borrow_mut().push(p.to_string());
        true
    });
    assert_eq!(found, ["C:/work/docs"]);
    assert_eq!(asked.into_inner(), ["C:/work/docs"], "no network path was even tested for existence");
    // The model keeps them, marked as not opened, so saving leaves them in the file.
    let d = doc(&text, r"C:\work");
    assert_eq!(d.folders().iter().filter(|f| f.target == Target::Unsupported).count(), 4);
}

#[test]
fn a_workspace_file_on_a_share_may_name_folders_on_that_share() {
    let text = serde_json::json!({ "folders": [
        { "path": "../api" }, { "path": r"\\SERVER\other\x" }, { "uri": "file://server/share/uri" }, { "path": r"\\evil\share" }
    ] })
    .to_string();
    assert_eq!(folders(&text, r"\\server\share\work", |_| true), ["//server/share/api", "//SERVER/other/x", "//server/share/uri"]);
}

/// AC1: names, unknown fields, shared sections and unsupported entries are read; the text is kept byte for byte.
#[test]
fn the_model_keeps_names_sections_and_the_exact_text() {
    let d = doc(SAMPLE, "/work/mono");
    assert_eq!(d.version(), MODEL_VERSION);
    assert_eq!(d.text(), SAMPLE, "byte-exact");
    let f = d.folders();
    assert_eq!(f.len(), 3, "the entry without a path is not a folder");
    assert_eq!((f[0].item, f[0].label(), f[0].path()), (0, "Backend ✨".to_string(), Some("/work/mono/api")));
    assert_eq!((f[1].item, f[1].label(), f[1].path()), (1, "web".to_string(), Some("/work/web")));
    assert_eq!((f[2].item, f[2].path()), (3, Some("/srv/my app")));
    assert_eq!(serde_json::from_str::<Value>(&d.settings_json().unwrap()).unwrap()["editor.tabSize"], 2);
    assert!(d.defines_commands());
    assert!(!doc("{ \"folders\": [] }", "/w").defines_commands());
    let tasks = crate::workbench::tasks::parse_task_file(&d.tasks_json().unwrap(), "workspace");
    assert_eq!(tasks[0].label, "ws build");
    let launch = crate::debug_config::parse_launch_json(&d.launch_json().unwrap(), "/work/mono/api", None, &tasks);
    assert_eq!(launch[0].name, "ws run");
    // Saving without a change writes nothing new.
    let same = d.with_folders(&f.iter().map(|f| Edit::Keep(f.item)).collect::<Vec<_>>()).unwrap();
    assert_eq!(same.text(), SAMPLE);
    assert_eq!(d.rebased("/work/mono").unwrap().text(), SAMPLE, "Save As into the same folder changes nothing");
}

#[test]
fn removing_and_adding_folders_edits_only_the_folder_list() {
    let d = doc(SAMPLE, "/work/mono");
    let next = d.with_folders(&[Edit::Keep(0), Edit::Keep(3), Edit::Add { path: "/work/mono/docs".into(), name: Some("Docs \"ü\"".into()) }]).unwrap();
    let t = next.text();
    assert!(t.starts_with("// Shared by the whole team.\n{\n    /* the folders, in order */\n    \"folders\": ["), "{t}");
    assert!(
        t.contains(r#"{ "path": "api", "name": "Backend ✨", "x-team": { "owner": "ada" } }, // the service"#),
        "kept entries keep their text and comment: {t}"
    );
    assert!(!t.contains("../web"));
    assert!(t.contains(r#"{ "name": "Docs \"ü\"", "path": "docs" }"#), "{t}");
    assert!(t.contains(r#"{ "name": "broken entry" }"#), "entries that are not folders stay: {t}");
    let tail = &SAMPLE[SAMPLE.find("    \"settings\"").unwrap()..];
    assert!(t.ends_with(tail), "everything after the list is untouched: {t}");
    let labels: Vec<String> = next.folders().iter().map(Folder::label).collect();
    assert_eq!(labels, ["Backend ✨", "my app", "Docs \"ü\""]);
    // Folders on another root are written absolute.
    let far = d.with_folders(&[Edit::Add { path: "/opt/tools".into(), name: None }]).unwrap();
    assert_eq!(far.folders()[0].source, Source::Path("/opt/tools".into()));
    // Everything removed: an empty list, the unknown entry kept.
    let none = d.with_folders(&[]).unwrap();
    assert!(none.folders().is_empty());
    assert!(none.text().contains("broken entry"));
}

#[test]
fn comments_travel_with_their_entries() {
    let text =
        "{\n  \"folders\": [ /* first */ { \"path\": \"a\" }, // after a\n    // about b\n    { \"path\": \"b\" } /* x */, // end\n    /* closing */\n  ]\n}\n";
    let d = doc(text, "/w");
    let without_a = d.with_folders(&[Edit::Keep(1)]).unwrap();
    let t = without_a.text();
    assert!(!t.contains("first") && !t.contains("after a"), "comments of the removed entry go with it: {t}");
    for kept in ["// about b", "/* x */", "// end", "/* closing */"] {
        assert!(t.contains(kept), "{kept} is kept: {t}");
    }
    let swapped = d.with_folders(&[Edit::Keep(1), Edit::Keep(0)]).unwrap();
    let t = swapped.text();
    assert!(t.find("// about b").unwrap() < t.find("/* first */").unwrap(), "{t}");
    assert!(t.contains("{ \"path\": \"a\" } // after a"), "{t}");
    assert_eq!(swapped.folders().iter().map(|f| f.path().unwrap()).collect::<Vec<_>>(), ["/w/b", "/w/a"]);
}

#[test]
fn a_file_without_a_folder_list_gets_one_and_keeps_the_rest() {
    for (text, dir) in [("{ /* x */ \"settings\": {} }", "/w"), ("\u{feff}{}", "/w"), ("{\r\n  \"a\": 1\r\n}\r\n", "/w")] {
        let d = doc(text, dir);
        let next = d.with_folders(&[Edit::Add { path: "/w/app".into(), name: None }]).unwrap();
        assert_eq!(next.folders()[0].path(), Some("/w/app"), "{}", next.text());
        assert_eq!(next.text().starts_with('\u{feff}'), text.starts_with('\u{feff}'), "a byte order mark stays");
        if text.contains("\r\n") {
            assert!(!next.text().replace("\r\n", "").contains('\n'), "CRLF files stay CRLF: {:?}", next.text());
        }
        assert!(next.text().contains("/* x */") || !text.contains("/* x */"));
    }
}

/// AC4: Save As into another folder rewrites relative paths so they reach the same folders.
#[test]
fn save_as_rebases_relative_paths() {
    let d = doc(SAMPLE, "/work/mono");
    let moved = d.rebased("/home/ada/elsewhere/deep").unwrap();
    let t = moved.text();
    assert!(t.contains(r#""path": "../../../../work/mono/api""#), "{t}");
    assert!(t.contains(r#""path": "../../../../work/web""#), "{t}");
    assert!(t.contains("file:///srv/my%20app") && t.contains("// the service") && t.contains("remoteAuthority"), "{t}");
    assert_eq!(
        moved.folders().iter().map(|f| f.path().map(str::to_string)).collect::<Vec<_>>(),
        d.folders().iter().map(|f| f.path().map(str::to_string)).collect::<Vec<_>>()
    );
    // Into a subfolder and back: the original text again.
    let inner = d.rebased("/work/mono/api").unwrap();
    assert!(inner.text().contains(r#""path": ".""#), "{}", inner.text());
    assert_eq!(inner.rebased("/work/mono").unwrap().text(), SAMPLE);
}

#[test]
fn save_as_on_windows_keeps_backslashes_and_switches_drives_to_absolute() {
    let text = r#"{ "folders": [ { "path": "src\\app" }, { "path": "..\\lib" }, { "path": "D:\\abs" } ] }"#;
    let d = doc(text, r"C:\work\mono");
    let moved = d.rebased(r"C:\work\other").unwrap();
    assert!(moved.text().contains(r#""path": "..\\mono\\src\\app""#), "{}", moved.text());
    assert!(moved.text().contains(r#""path": "..\\lib""#), "unchanged: {}", moved.text());
    assert!(moved.text().contains(r#""path": "D:\\abs""#));
    let other_drive = d.rebased(r"E:\save").unwrap();
    assert!(other_drive.text().contains(r#""path": "C:/work/mono/src/app""#), "a path on another drive is written absolute: {}", other_drive.text());
    assert_eq!(other_drive.folders()[0].path(), Some("C:/work/mono/src/app"));
    // Drive letters compare without case.
    assert_eq!(relative_to("c:/Work", "C:/work/x"), Some("x".into()));
    // A share is another root.
    assert_eq!(relative_to("//srv/share/a", "//srv/other/a"), None);
    assert_eq!(relative_to("//srv/share/a", "//SRV/share/b"), Some("../b".into()));
}

#[test]
fn relative_paths_cover_parents_siblings_and_the_folder_itself() {
    assert_eq!(relative_to("/a/b", "/a/b"), Some(".".into()));
    assert_eq!(relative_to("/a/b", "/a/b/c/d"), Some("c/d".into()));
    assert_eq!(relative_to("/a/b", "/a/x"), Some("../x".into()));
    assert_eq!(relative_to("/", "/x"), Some("x".into()));
    assert_eq!(relative_to("/a", "C:/x"), None);
    // A letter followed by a multi-byte character is not a drive (and must not split the character).
    assert_eq!(relative_to("a日日/x", "a日日/y"), Some("../y".into()));
    let d = doc(r#"{ "folders": [ { "path": "a日/b" }, { "path": "é:" } ] }"#, "/w");
    assert_eq!(d.folders()[0].path(), Some("/w/a日/b"));
    assert!(d.rebased("/w/a日").unwrap().text().contains(r#""path": "b""#));
}

#[test]
fn traversal_stays_inside_the_root() {
    let d = doc(r#"{ "folders": [ { "path": "../../../../../../etc" }, { "path": "a/../../b/./c" } ] }"#, "/home/ada/ws");
    assert_eq!(d.folders()[0].path(), Some("/etc"));
    assert_eq!(d.folders()[1].path(), Some("/home/ada/b/c"));
    let w = doc(r#"{ "folders": [ { "path": "..\\..\\..\\..\\x" } ] }"#, r"\\srv\share\ws");
    assert_eq!(w.folders()[0].path(), Some("//srv/share/x"), "`..` cannot climb out of the share");
}

/// AC6: whatever is unreadable is refused with a reason, never read as an empty workspace.
#[test]
fn malformed_and_truncated_input_is_refused() {
    let bad = [
        "",
        "   ",
        "{",
        "[1, 2]",
        "\"just a string\"",
        "{ \"folders\": { \"path\": \"x\" } }",
        "{ \"folders\": [ { \"path\": \"x\" } ] /* never closed",
        "{ \"folders\": [ { \"path\": \"x } ] }",
        "{ \"folders\": [ ] } trailing",
        "\u{feff}",
        "{ \"a\": 1 } }",
        "nul\0l",
    ];
    for text in bad {
        assert!(Document::parse(text, "/w").is_err(), "{text:?} must not read as a workspace");
    }
    // Every cut of a real file either reads or is refused; none panics, and a cut that reads keeps its exact text.
    for (i, _) in SAMPLE.char_indices() {
        if let Ok(d) = Document::parse(&SAMPLE[..i], "/w") {
            assert_eq!(d.text(), &SAMPLE[..i]);
        }
    }
    assert!(Document::parse(SAMPLE, "/w").is_ok());
}

#[test]
fn deep_nesting_huge_files_and_duplicate_keys_are_handled() {
    let deep = format!("{{ \"settings\": {}{} }}", "[".repeat(100_000), "]".repeat(100_000));
    assert!(Document::parse(&deep, "/w").is_err(), "the parser's depth limit refuses it without overflowing the stack");
    let huge = format!("{{ \"folders\": [], \"pad\": \"{}\" }}", "x".repeat(MAX_BYTES));
    assert_eq!(Document::parse(&huge, "/w").unwrap_err(), "the file is too large");
    let many = format!("{{ \"folders\": [{}] }}", vec!["{\"path\":\"x\"}"; MAX_FOLDERS + 1].join(","));
    assert!(Document::parse(&many, "/w").is_err());
    // A repeated key: the last one counts (as for every JSON reader) and the edit lands there.
    let dup = r#"{ "folders": [ { "path": "old" } ], "folders": [ { "path": "new" }, { "path": "two" } ] }"#;
    let d = doc(dup, "/w");
    assert_eq!(d.folders().iter().map(|f| f.path().unwrap()).collect::<Vec<_>>(), ["/w/new", "/w/two"]);
    let next = d.with_folders(&[Edit::Keep(1)]).unwrap();
    assert!(next.text().contains(r#""folders": [ { "path": "old" } ]"#), "the ignored copy stays as written: {}", next.text());
    assert_eq!(next.folders().iter().map(|f| f.path().unwrap()).collect::<Vec<_>>(), ["/w/two"]);
    let dup_path = doc(r#"{ "folders": [ { "path": "a", "path": "b" } ] }"#, "/w");
    assert_eq!(dup_path.folders()[0].path(), Some("/w/b"));
    assert!(dup_path.rebased("/x").unwrap().text().contains(r#""path": "a", "path": "../w/b""#));
}

#[test]
fn unicode_long_paths_and_crlf_round_trip() {
    let long = format!("{}/son", "çok-uzun-klasör-adı-🚀".repeat(40));
    let text = format!("\u{feff}{{\r\n  // ünicode\r\n  \"folders\": [\r\n    {{ \"path\": \"{long}\", \"name\": \"日本語\" }}\r\n  ]\r\n}}\r\n");
    let d = doc(&text, "/ev/ğ");
    assert_eq!(d.text(), text);
    assert_eq!(d.folders()[0].label(), "日本語");
    assert_eq!(d.folders()[0].path(), Some(format!("/ev/ğ/{long}").as_str()));
    let moved = d.rebased("/ev").unwrap();
    assert!(moved.text().starts_with('\u{feff}'));
    assert!(moved.text().contains(&format!("\"path\": \"ğ/{long}\"")));
    let added = d.with_folders(&[Edit::Keep(0), Edit::Add { path: "/ev/ğ/ç".into(), name: None }]).unwrap();
    assert!(!added.text().replace("\r\n", "").contains('\n'), "line endings stay CRLF");
    assert_eq!(added.folders()[1].path(), Some("/ev/ğ/ç"));
}

#[test]
fn stale_or_unknown_edits_are_refused() {
    let d = doc(SAMPLE, "/w");
    assert!(d.with_folders(&[Edit::Keep(2)]).is_err(), "item 2 is not a folder");
    assert!(d.with_folders(&[Edit::Keep(99)]).is_err());
}

#[test]
fn hostile_text_never_panics() {
    let seeds = [SAMPLE, "{\"folders\":[{\"path\":\"\\u0000\"}]}", "{\"folders\":[{\"uri\":\"file://%\"}]}", "/*/{}*/{}", "{\"\\\"\":1}"];
    for seed in seeds {
        for cut in 0..seed.len() {
            let Some(head) = seed.get(..cut) else { continue };
            for tail in ["", "\"", "/*", "//", "}", "]", ",", "\\"] {
                let t = format!("{head}{tail}");
                if let Ok(d) = Document::parse(&t, "/w") {
                    let _ = d.rebased("/elsewhere");
                    let _ = d.with_folders(&[Edit::Add { path: "/w/n".into(), name: None }]);
                }
            }
        }
    }
}
