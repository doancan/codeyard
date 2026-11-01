//! `sdk/typescript/test/rule-cases.json`: globs, paths, request URLs, status items, answer sizes and limits, each
//! judged by the Rust rules here. The TypeScript mock host (sdk/typescript/src/testing) has a copy of these rules, and
//! its test checks the copy against every case, so the two cannot drift. `UPDATE_DOCS=1 cargo test -p ext-manifest
//! reference` writes the file.

use serde_json::{Value, json};

use crate::calls::{UrlProblem, check_status_item, check_url, result_too_large};
use crate::glob::path_matches;
use crate::{globs_allow, limits, relative};

const FILE: &str = "sdk/typescript/test/rule-cases.json";

fn globs() -> Value {
    let cases = [
        ("main.rs", "main.rs"),
        ("main.rs", "src/main.rs"),
        ("**/main.rs", "src/main.rs"),
        ("**/main.rs", "main.rs"),
        ("src/*.rs", "src/lib.rs"),
        ("src/*.rs", "src/a/lib.rs"),
        ("src/**/*.rs", "src/a/b/lib.rs"),
        ("src/**/*.rs", "src/lib.rs"),
        ("src/**", "src"),
        ("src/**", "src/a/b"),
        ("**", "a/b"),
        ("*", ".env"),
        ("?.md", "a.md"),
        ("?.md", "ab.md"),
        ("?.md", "ğ.md"),
        ("pages/[id].tsx", "pages/[id].tsx"),
        ("pages/[id].tsx", "pages/i.tsx"),
        ("./build/out", "build/out"),
        ("*.{rs,md}", "a.rs"),
        ("*a*a*a*a*a*a*a*a*a*a*a*a*a*a*b", &"a".repeat(60)),
        ("**/**/**/**/**/**/**/**/**/**/x", &vec!["d"; 40].join("/")),
        ("**/**/**/**/d", &vec!["d"; 40].join("/")),
    ];
    cases.iter().map(|(g, p)| json!({ "glob": g, "path": p, "matches": path_matches(g, p) })).collect()
}

fn allowed() -> Value {
    let cases: [(&[&str], &str); 10] = [
        (&["**"], "src/main.rs"),
        (&["**"], ".env"),
        (&["**"], "app/.env.local"),
        (&["**"], ".git/config"),
        (&["**"], ".gitignore"),
        (&[".env"], ".env"),
        (&["**/.env*"], "app/.env.local"),
        (&[".git/config"], ".git/config"),
        (&["**/*.git*"], ".git/config"),
        (&["main.rs"], "src/main.rs"),
    ];
    cases
        .iter()
        .map(|(globs, path)| {
            let globs: Vec<String> = globs.iter().map(|g| g.to_string()).collect();
            json!({ "globs": globs, "path": path, "allowed": globs_allow(&globs, path) })
        })
        .collect()
}

fn paths() -> Value {
    let folder = "file:///work";
    let uris = [
        "file:///work/src/main.rs",
        "file:///work",
        "file:///work/",
        "file:///work/src/../../etc/passwd",
        "file:///work/src/%2e%2e/%2e%2e/etc/passwd",
        "file:///work/src/./a/../b.rs",
        "file:///work/a%20b.txt",
        "file:///work/%C4%B0stanbul/%C4%9F.txt",
        "file:///work/İstanbul/ğ.txt",
        "file:///work/a%00b",
        "file:///work/a%zz",
        "file:///workshop/a",
        "file:///etc/passwd",
        "src/main.rs",
        "https://example.com/work/a",
    ];
    uris.iter().map(|u| json!({ "folder": folder, "uri": u, "relative": relative(&[folder.to_string()], u) })).collect()
}

fn urls() -> Value {
    let urls = [
        "https://api.example.com/x",
        "https://API.Example.com./x?y#z",
        "https://api.example.com:8443/",
        "http://api.example.com/",
        "HTTP://api.example.com/",
        "file:///etc/passwd",
        "https://127.0.0.1/",
        "https://[::1]:8443/x",
        "https://10.1/",
        "https://[::1]/",
        "https://2130706433/",
        "https://0x7f.1/",
        "https://user:pw@api.example.com/",
        "https://user@api.example.com/x",
        "https://",
        "https://api.example.com:0/",
        "https://api.example.com:/",
        "https://api.example.com:99999/",
        "https://bad_host.example.com/",
        "https://a.example.com../",
        "https://ıstanbul.example.com/",
        "api.example.com/x",
        "javascript:alert(1)",
    ];
    urls.iter()
        .map(|u| match check_url(u) {
            Ok(t) => json!({ "url": u, "host": t.host, "port": t.port }),
            Err(e) => {
                let problem = match e.problem {
                    UrlProblem::NotHttps => "not-https",
                    UrlProblem::IpLiteral => "ip-literal",
                    UrlProblem::Invalid => "invalid",
                };
                json!({ "url": u, "problem": problem, "message": e.message, "host": e.host, "denied": e.refusal().err() })
            }
        })
        .collect()
}

/// Declared ids (none: no manifest), id, text, tooltip.
type StatusCase<'a> = (Option<&'a [&'a str]>, &'a str, &'a str, Option<&'a str>);

fn status_items() -> Value {
    let long = "y".repeat(limits::STATUS_TEXT_LEN + 1);
    let full = "ğ".repeat(limits::STATUS_TEXT_LEN);
    let tooltip = "t".repeat(limits::ITEM_LEN + 1);
    let cases: [StatusCase; 9] = [
        (Some(&["deploys"]), "deploys", "  Ready ✓ ", None),
        (Some(&["deploys"]), "other", "Ready", None),
        (None, "other", "Ready", None),
        (Some(&["deploys"]), "", "Ready", None),
        (Some(&["deploys"]), "deploys", "   ", None),
        (Some(&["deploys"]), "deploys", &long, None),
        (Some(&["deploys"]), "deploys", &full, Some("Vercel")),
        (Some(&["deploys"]), "deploys", "a\u{202e}b", None),
        (Some(&["deploys"]), "deploys", "ok", Some(&tooltip)),
    ];
    cases
        .iter()
        .map(|(declared, id, text, tooltip)| {
            let is_declared = |i: &str| declared.is_some_and(|d| d.contains(&i));
            let check: Option<&dyn Fn(&str) -> bool> = declared.map(|_| &is_declared as &dyn Fn(&str) -> bool);
            let verdict = match check_status_item(check, id, text, *tooltip) {
                Ok(shown) => json!({ "shown": shown }),
                Err(e) => json!({ "error": e }),
            };
            json!({ "declared": declared, "id": id, "text": text, "tooltip": tooltip, "expect": verdict })
        })
        .collect()
}

/// Declared `languages` of each `[[diagnostics]]` (none: no manifest), language, reachable, other files.
type TargetCase = (Option<Vec<Vec<String>>>, &'static str, bool, usize);

fn diagnostics() -> Value {
    let long = "ğ".repeat(limits::DIAGNOSTIC_MESSAGE_LEN + 1);
    let exact = "x".repeat(limits::DIAGNOSTIC_MESSAGE_LEN);
    let messages = ["  unused import ", "", "   ", "a\u{202e}b", "a\u{200b}b", "a\u{feff}b", "line one\n\tline two", "a\u{7}b", &long, &exact];
    let messages: Vec<Value> = messages
        .iter()
        .map(|m| match crate::calls::shown_diagnostic_message(m) {
            Some((shown, cut)) => json!({ "message": m, "shown": shown, "cut": cut }),
            None => json!({ "message": m, "shown": null }),
        })
        .collect();
    let rust = vec!["rust".to_string()];
    let any: Vec<String> = Vec::new();
    let targets: [TargetCase; 9] = [
        (Some(vec![rust.clone()]), "rust", true, 0),
        (Some(vec![any]), "toml", true, 0),
        (None, "toml", true, 0),
        (Some(vec![]), "rust", true, 0),
        (Some(vec![rust.clone()]), "toml", true, 0),
        (Some(vec![rust.clone()]), "rust", false, 0),
        (Some(vec![rust.clone()]), "rust", true, limits::DIAGNOSTIC_FILES),
        (Some(vec![rust.clone()]), "rust", true, limits::DIAGNOSTIC_FILES - 1),
        (Some(vec![rust.clone(), vec!["markdown".into()]]), "markdown", true, 3),
    ];
    let targets: Vec<Value> = targets
        .iter()
        .map(|(declared, language, reachable, others)| {
            let error = crate::calls::check_diagnostics_target(declared.as_deref(), language, *reachable, *others).err();
            json!({ "declared": declared, "language": language, "reachable": reachable, "otherFiles": others, "error": error })
        })
        .collect();
    let files =
        ["a.rs", "b/c.PY", "x.hxx", "Rakefile", "dir/Gemfile", "a.gemspec", "a.phtml", "a.yml", "a.tsx", "a.zzz", "noext", "dir.d/noext", ".toml", "a.TOML"];
    let languages: Vec<Value> = files.iter().map(|f| json!({ "path": f, "language": crate::builtin_language(f) })).collect();
    json!({ "messages": messages, "targets": targets, "languages": languages })
}

fn results() -> Value {
    [0, limits::CALL_BYTES, limits::CALL_BYTES + 1, 20 << 20].iter().map(|b| json!({ "bytes": b, "error": result_too_large(*b) })).collect()
}

fn cases() -> Value {
    json!({
        "limits": {
            "CALL_BYTES": limits::CALL_BYTES,
            "DATA_DIR_BYTES": limits::DATA_DIR_BYTES,
            "ITEM_LEN": limits::ITEM_LEN,
            "STATUS_TEXT_LEN": limits::STATUS_TEXT_LEN,
            "DIAGNOSTIC_FILES": limits::DIAGNOSTIC_FILES,
            "DIAGNOSTIC_MESSAGE_LEN": limits::DIAGNOSTIC_MESSAGE_LEN,
            "MARKS_PER_FILE": limits::MARKS_PER_FILE,
            "TIMER_MIN_SECONDS": limits::TIMER_MIN_SECONDS,
        },
        "globs": globs(),
        "allowed": allowed(),
        "paths": paths(),
        "urls": urls(),
        "statusItems": status_items(),
        "results": results(),
        "diagnostics": diagnostics(),
        "outside": [
            { "kind": "read", "path": "file:///etc/passwd", "line": crate::outside_line("read", "file:///etc/passwd") },
            { "kind": "write", "path": "file:///work/../x", "line": crate::outside_line("write", "file:///work/../x") },
        ],
    })
}

/// Keys sorted, whether or not another crate in the build turned on serde_json's `preserve_order`.
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

#[test]
fn rule_cases_reference() {
    let want = serde_json::to_string_pretty(&canonical(&cases())).unwrap() + "\n";
    let path = format!("{}/../../{FILE}", env!("CARGO_MANIFEST_DIR"));
    if std::env::var_os("UPDATE_DOCS").is_some() {
        std::fs::write(&path, &want).unwrap();
    }
    let have = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
    assert!(have == want, "{FILE} is out of date: run `UPDATE_DOCS=1 cargo test -p ext-manifest reference`");
}
