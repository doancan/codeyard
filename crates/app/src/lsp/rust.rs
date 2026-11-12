//! rust-analyzer's own protocol extensions, as plain data: `experimental/serverStatus`, flycheck progress,
//! `experimental/runnables`, `experimental/externalDocs` and the request names of the server controls.
//! No UI and no I/O; the workbench decides what to do with the values.

use serde_json::Value;

use super::{Range16, clip_text, parse_range, uri_to_path};

/// Requests and notifications that control the server (Rust: Reload Workspace and the other palette commands).
pub const RELOAD_WORKSPACE: &str = "rust-analyzer/reloadWorkspace";
pub const REBUILD_PROC_MACROS: &str = "rust-analyzer/rebuildProcMacros";
pub const RUN_FLYCHECK: &str = "rust-analyzer/runFlycheck";
pub const CANCEL_FLYCHECK: &str = "rust-analyzer/cancelFlycheck";
pub const CLEAR_FLYCHECK: &str = "rust-analyzer/clearFlycheck";
pub const ANALYZER_STATUS: &str = "rust-analyzer/analyzerStatus";
pub const RUNNABLES: &str = "experimental/runnables";
pub const EXTERNAL_DOCS: &str = "experimental/externalDocs";
pub const VIEW_HIR: &str = "rust-analyzer/viewHir";
pub const VIEW_MIR: &str = "rust-analyzer/viewMir";
pub const VIEW_SYNTAX_TREE: &str = "rust-analyzer/viewSyntaxTree";

/// Semantic token modifiers the editor draws, as flags: `unsafe`, `mutable` and rust-analyzer's `consuming` (a value
/// moved out by this use).
pub const MOD_UNSAFE: u8 = 1;
pub const MOD_MUTABLE: u8 = 2;
pub const MOD_CONSUMED: u8 = 4;

/// The flags of the modifiers in `bits`, given the server's modifier table.
pub fn modifier_flags(bits: u64, legend: &[String]) -> u8 {
    if bits == 0 {
        return 0;
    }
    legend.iter().take(64).enumerate().filter(|(i, _)| bits >> i & 1 == 1).fold(0, |flags, (_, name)| {
        flags
            | match name.as_str() {
                "unsafe" => MOD_UNSAFE,
                "mutable" => MOD_MUTABLE,
                "consuming" | "consumed" => MOD_CONSUMED,
                _ => 0,
            }
    })
}

/// Progress tokens of the check the server runs on save (`rust-analyzer/flycheck/0`, one per workspace).
const FLYCHECK_TOKEN: &str = "rust-analyzer/flycheck";
/// Most characters of a status message or a progress label.
const MAX_TEXT: usize = 2000;

/// How the server rates itself (`experimental/serverStatus`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Health {
    #[default]
    Ok,
    Warning,
    Error,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerStatus {
    pub health: Health,
    /// `false` while the server is loading the workspace or indexing.
    pub quiescent: bool,
    pub message: Option<String>,
}

pub fn parse_server_status(params: &Value) -> Option<ServerStatus> {
    let health = match params["health"].as_str()? {
        "ok" => Health::Ok,
        "warning" => Health::Warning,
        "error" => Health::Error,
        _ => return None,
    };
    let message = params["message"].as_str().map(str::trim).filter(|m| !m.is_empty()).map(|m| clip_text(m, MAX_TEXT));
    Some(ServerStatus { health, quiescent: params["quiescent"].as_bool().unwrap_or(true), message })
}

/// One step of the check run (`cargo check` or `clippy`), from `$/progress`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlycheckStep {
    /// The command's title (`cargo check`).
    Begin(String),
    /// The target being checked (`demo (lib)`).
    Report(String),
    End,
}

/// The token and step of a flycheck progress notification; `None` for any other progress.
pub fn parse_flycheck_progress(params: &Value) -> Option<(String, FlycheckStep)> {
    let token = params["token"].as_str().filter(|t| t.starts_with(FLYCHECK_TOKEN))?;
    let value = &params["value"];
    let text = |key: &str, default: &str| clip_text(value[key].as_str().filter(|s| !s.is_empty()).unwrap_or(default), MAX_TEXT);
    let step = match value["kind"].as_str()? {
        "begin" => FlycheckStep::Begin(text("title", "cargo check")),
        "report" => FlycheckStep::Report(text("message", "")),
        "end" => FlycheckStep::End,
        _ => return None,
    };
    Some((token.to_string(), step))
}

/// What `experimental/runnables` offers. Cargo runnables keep their arguments apart so the command line is built
/// (and quoted) in one place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Runnable {
    pub label: String,
    pub location: Option<RunnableLocation>,
    pub command: RunnableCommand,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunnableLocation {
    pub path: String,
    /// The whole item (`fn`, `mod`), and the name inside it.
    pub range: Range16,
    pub name: Range16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunnableCommand {
    /// `cargo <cargo_args> <extra_args> -- <executable_args>`.
    Cargo { cargo_args: Vec<String>, extra_args: Vec<String>, executable_args: Vec<String>, cwd: Option<String>, workspace_root: Option<String> },
    /// A program that is not Cargo (rust-analyzer's `kind: "shell"`).
    Shell { program: String, args: Vec<String>, cwd: Option<String> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunnableKind {
    Run,
    Test,
    /// `test-mod`: every test of a module.
    TestModule,
    Bench,
    Check,
    Other,
}

impl Runnable {
    pub fn kind(&self) -> RunnableKind {
        let RunnableCommand::Cargo { cargo_args, .. } = &self.command else { return RunnableKind::Other };
        match cargo_args.first().map(String::as_str) {
            Some("run") => RunnableKind::Run,
            Some("test") if self.label.starts_with("test-mod") => RunnableKind::TestModule,
            Some("test") => RunnableKind::Test,
            Some("bench") => RunnableKind::Bench,
            Some("check") => RunnableKind::Check,
            _ => RunnableKind::Other,
        }
    }

    /// Whether the item covers the (line, column) position.
    pub fn covers(&self, position: (u32, u32)) -> bool {
        self.location.as_ref().is_some_and(|l| l.range.0 <= position && position <= l.range.1)
    }

    /// 0-based line where the gutter mark belongs: the item's name.
    pub fn line(&self) -> Option<u32> {
        self.location.as_ref().map(|l| l.name.0.0)
    }
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

/// The answer to `experimental/runnables`. Entries the client cannot run are dropped.
pub fn parse_runnables(v: &Value) -> Vec<Runnable> {
    v.as_array().map(|a| a.iter().filter_map(parse_runnable).collect()).unwrap_or_default()
}

fn parse_runnable(v: &Value) -> Option<Runnable> {
    let label = v["label"].as_str().filter(|l| !l.is_empty())?.to_string();
    let args = &v["args"];
    let cwd = args["cwd"].as_str().filter(|c| !c.is_empty()).map(str::to_string);
    let command = match v["kind"].as_str()? {
        "cargo" => {
            let cargo_args = strings(&args["cargoArgs"]);
            if cargo_args.is_empty() {
                return None;
            }
            let workspace_root = args["workspaceRoot"].as_str().filter(|c| !c.is_empty()).map(str::to_string);
            RunnableCommand::Cargo {
                cargo_args,
                extra_args: strings(&args["cargoExtraArgs"]),
                executable_args: strings(&args["executableArgs"]),
                cwd,
                workspace_root,
            }
        }
        "shell" => RunnableCommand::Shell { program: args["program"].as_str().filter(|p| !p.is_empty())?.to_string(), args: strings(&args["args"]), cwd },
        _ => return None,
    };
    let location = parse_runnable_location(&v["location"]);
    Some(Runnable { label, location, command })
}

fn parse_runnable_location(v: &Value) -> Option<RunnableLocation> {
    let path = uri_to_path(v["targetUri"].as_str().or(v["uri"].as_str())?)?;
    let (range, name) = match v.get("targetRange") {
        Some(range) => (parse_range(range), parse_range(&v["targetSelectionRange"])),
        None => (parse_range(&v["range"]), parse_range(&v["range"])),
    };
    Some(RunnableLocation { path, range, name })
}

/// Where the documentation of a symbol is: a web page (docs.rs or the crate's own site) and, when built, a local HTML file.
/// Only these two kinds are kept: a link of any other scheme from the server is never opened.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DocsLinks {
    /// `http://` or `https://`.
    pub web: Option<String>,
    /// A `file:` URI of an `.html` file.
    pub local: Option<String>,
    /// The server sent a link that is neither, which was dropped.
    pub rejected: bool,
}

/// The path of a local documentation link: a `file:` URI whose path ends in `.html`.
pub fn local_docs_path(url: &str) -> Option<String> {
    // `file:///path` or `file://localhost/path`. A host that is a server (`file://server/share/x.html`) names a network
    // share, and opening it would send the user's credentials there.
    let rest = url.strip_prefix("file://")?;
    if !matches!(rest.split('/').next(), Some("" | "localhost")) {
        return None;
    }
    let path = uri_to_path(url)?;
    if path.starts_with("//") {
        return None; // an empty host with a UNC path
    }
    (path.ends_with(".html") && !path.split('/').any(|part| part == "..")).then_some(path)
}

/// The answer to `experimental/externalDocs`: a URL, or `{ web, local }` when the client announced `localDocs`.
pub fn parse_external_docs(v: &Value) -> DocsLinks {
    let mut links = DocsLinks::default();
    let mut take = |link: &Value, local_allowed: bool, web_allowed: bool| {
        let Some(url) = link.as_str().filter(|u| !u.is_empty()) else { return };
        if web_allowed && (url.starts_with("https://") || url.starts_with("http://")) {
            links.web = Some(url.to_string());
        } else if local_allowed && local_docs_path(url).is_some() {
            links.local = Some(url.to_string());
        } else {
            links.rejected = true;
        }
    };
    match v {
        Value::String(_) => take(v, true, true),
        Value::Object(_) => {
            take(&v["web"], false, true);
            take(&v["local"], true, false);
        }
        _ => {}
    }
    links
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Real answers and notifications of rust-analyzer 0.3.3113 for a small package (see `testdata/rust-analyzer`).
    fn fixture(name: &str) -> Value {
        let text = std::fs::read_to_string(format!("{}/testdata/rust-analyzer/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn the_real_notification_stream_gives_status_and_flycheck_steps() {
        let text = std::fs::read_to_string(format!("{}/testdata/rust-analyzer/notifications.jsonl", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let mut statuses = Vec::new();
        let mut steps = Vec::new();
        for line in text.lines() {
            let msg: Value = serde_json::from_str(line).unwrap();
            match msg["method"].as_str().unwrap() {
                "experimental/serverStatus" => statuses.push(parse_server_status(&msg["params"]).unwrap()),
                "$/progress" => steps.push(parse_flycheck_progress(&msg["params"]).unwrap().1),
                other => panic!("unexpected {other}"),
            }
        }
        assert_eq!(statuses[0], ServerStatus { health: Health::Ok, quiescent: false, message: None });
        assert_eq!(statuses[1], ServerStatus { health: Health::Ok, quiescent: true, message: None });
        assert_eq!(steps[0], FlycheckStep::Begin("cargo check".into()));
        assert_eq!(steps[1], FlycheckStep::Report("demo (lib)".into()));
        assert_eq!(steps.last(), Some(&FlycheckStep::End));
    }

    #[test]
    fn status_with_a_warning_and_a_message_and_bad_values() {
        let s = parse_server_status(&json!({ "health": "warning", "quiescent": true, "message": "  Failed to load workspace.\n" })).unwrap();
        assert_eq!((s.health, s.message.as_deref()), (Health::Warning, Some("Failed to load workspace.")));
        assert_eq!(parse_server_status(&json!({ "health": "error", "quiescent": false, "message": "" })).unwrap().message, None);
        assert_eq!(parse_server_status(&json!({ "health": "weird", "quiescent": true })), None);
        assert_eq!(parse_server_status(&json!({ "quiescent": true })), None);
        assert!(parse_server_status(&json!({ "health": "ok" })).unwrap().quiescent, "older servers send no flag: not busy");
        let long = parse_server_status(&json!({ "health": "error", "message": "x".repeat(10_000) })).unwrap();
        assert!(long.message.unwrap().len() <= MAX_TEXT + 4);
    }

    #[test]
    fn only_flycheck_tokens_are_flycheck_progress() {
        let begin = json!({ "token": "rust-analyzer/flycheck/1", "value": { "kind": "begin", "title": "clippy" } });
        assert_eq!(parse_flycheck_progress(&begin), Some(("rust-analyzer/flycheck/1".into(), FlycheckStep::Begin("clippy".into()))));
        let indexing = json!({ "token": "rustAnalyzer/Roots Scanned", "value": { "kind": "report", "message": "1/15" } });
        assert_eq!(parse_flycheck_progress(&indexing), None);
        assert_eq!(parse_flycheck_progress(&json!({ "token": 7, "value": { "kind": "end" } })), None, "numeric tokens are other servers' work");
        assert_eq!(parse_flycheck_progress(&json!({ "token": "rust-analyzer/flycheck/0", "value": { "kind": "mystery" } })), None);
        let untitled = json!({ "token": "rust-analyzer/flycheck/0", "value": { "kind": "begin" } });
        assert_eq!(parse_flycheck_progress(&untitled).unwrap().1, FlycheckStep::Begin("cargo check".into()));
    }

    #[test]
    fn runnables_of_a_library_file() {
        let list = parse_runnables(&fixture("runnables-lib.json"));
        let kinds: Vec<(&str, RunnableKind, Option<u32>)> = list.iter().map(|r| (r.label.as_str(), r.kind(), r.line())).collect();
        assert_eq!(
            kinds,
            [
                ("test-mod tests", RunnableKind::TestModule, Some(10)),
                ("test tests::adds_up", RunnableKind::Test, Some(14)),
                ("test tests::fails_on_purpose", RunnableKind::Test, Some(19)),
                ("test tests::ignored_one", RunnableKind::Test, Some(25)),
                ("cargo check -p demo --all-targets", RunnableKind::Check, None),
                ("cargo test -p demo --all-targets", RunnableKind::Test, None),
            ]
        );
        let RunnableCommand::Cargo { cargo_args, executable_args, cwd, workspace_root, .. } = &list[1].command else { panic!("cargo") };
        assert_eq!(workspace_root.as_deref(), Some("/work/demo"));
        assert_eq!(cargo_args, &["test", "--package", "demo", "--lib"]);
        assert_eq!(executable_args, &["tests::adds_up", "--exact", "--nocapture", "--include-ignored"]);
        assert_eq!(cwd.as_deref(), Some("/work/demo"));
        assert_eq!(list[1].location.as_ref().unwrap().path, "/work/demo/src/lib.rs");
    }

    #[test]
    fn runnables_of_a_main_file_and_the_ones_at_a_position() {
        let list = parse_runnables(&fixture("runnables-main.json"));
        assert_eq!(list[0].kind(), RunnableKind::Run);
        assert_eq!((list[0].label.as_str(), list[0].line()), ("run demo", Some(0)));
        assert!(list[0].covers((1, 4)) && !list[0].covers((5, 0)));
        assert!(!list[1].covers((0, 0)), "a runnable without a location covers nothing");
    }

    #[test]
    fn runnables_that_cannot_be_run_are_dropped() {
        let v = json!([
            { "label": "", "kind": "cargo", "args": { "cargoArgs": ["run"] } },
            { "label": "a", "kind": "cargo", "args": { "cargoArgs": [] } },
            { "label": "b", "kind": "teleport", "args": {} },
            { "label": "c", "kind": "shell", "args": { "program": "" } },
            { "label": "d", "kind": "shell", "args": { "program": "make", "args": ["test"], "cwd": "/w" },
              "location": { "uri": "file:///w/a.rs", "range": { "start": { "line": 2, "character": 0 }, "end": { "line": 3, "character": 1 } } } },
        ]);
        let list = parse_runnables(&v);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].command, RunnableCommand::Shell { program: "make".into(), args: vec!["test".into()], cwd: Some("/w".into()) });
        assert_eq!(list[0].kind(), RunnableKind::Other);
        assert_eq!(list[0].line(), Some(2), "a plain Location marks the start of its range");
        assert!(parse_runnables(&Value::Null).is_empty());
    }

    #[test]
    fn external_docs_in_both_answer_shapes() {
        let web = "https://docs.rs/demo/0.1.0/demo/fn.add.html";
        assert_eq!(parse_external_docs(&json!(web)), DocsLinks { web: Some(web.into()), ..Default::default() });
        assert_eq!(parse_external_docs(&json!("http://localhost:8000/x.html")).web.as_deref(), Some("http://localhost:8000/x.html"));
        let local = "file:///w/target/doc/demo/fn.add.html";
        assert_eq!(parse_external_docs(&json!(local)).local.as_deref(), Some(local));
        let both = parse_external_docs(&json!({ "web": web, "local": local }));
        assert_eq!((both.web.as_deref(), both.local.as_deref(), both.rejected), (Some(web), Some(local), false));
        assert_eq!(parse_external_docs(&Value::Null), DocsLinks::default());
        assert_eq!(parse_external_docs(&json!({ "web": "" })), DocsLinks::default());
    }

    /// A link from the server is a web page or a local HTML file, nothing else: no other scheme reaches the system.
    #[test]
    fn external_docs_of_any_other_kind_are_dropped() {
        for bad in [
            "javascript:alert(1)",
            "vscode://file/etc/passwd",
            "ssh://evil.example/x",
            "file:///etc/passwd",
            "file:///w/doc/index.txt",
            "file:///w/../etc/x.html",
            "ftp://x/y.html",
            "HTTPS://upper.example/",
            "//host/x.html",
            "/just/a/path.html",
        ] {
            let plain = parse_external_docs(&json!(bad));
            assert_eq!((plain.web, plain.local, plain.rejected), (None, None, true), "{bad}");
            let object = parse_external_docs(&json!({ "web": bad, "local": bad }));
            assert_eq!((object.web, object.local, object.rejected), (None, None, true), "{bad}");
        }
        // A web link in `local`, and a file in `web`, are the wrong kind there.
        let swapped = parse_external_docs(&json!({ "web": "file:///w/a.html", "local": "https://docs.rs/x" }));
        assert_eq!((swapped.web, swapped.local, swapped.rejected), (None, None, true));
        // One good link keeps working next to a bad one.
        let mixed = parse_external_docs(&json!({ "web": "https://docs.rs/x", "local": "ssh://x" }));
        assert_eq!((mixed.web.as_deref(), mixed.local, mixed.rejected), (Some("https://docs.rs/x"), None, true));
        assert_eq!(local_docs_path("file:///w/a%20b/index.html").as_deref(), Some("/w/a b/index.html"));
        assert_eq!(local_docs_path("file://localhost/w/index.html").as_deref(), Some("/w/index.html"));
        for share in
            ["file://server/share/x.html", "file://192.168.0.1/c$/x.html", "file:////server/share/x.html", "file:/w/x.html", "file:x.html", "FILE:///w/x.html"]
        {
            assert_eq!(local_docs_path(share), None, "{share}");
        }
        let both = parse_external_docs(&json!({ "local": "file://server/share/x.html" }));
        assert_eq!((both.local, both.rejected), (None, true), "a network share is not a local copy");
    }

    /// The first 40 tokens rust-analyzer sent for the demo library, with its two tables.
    #[test]
    fn real_semantic_tokens_carry_the_unsafe_modifier_of_an_unsafe_block() {
        let sample = fixture("semantic-tokens.json");
        let names = |key: &str| sample[key].as_array().unwrap().iter().map(|n| n.as_str().unwrap().to_string()).collect::<Vec<_>>();
        let tokens = crate::lsp::decode_semantic_tokens(&sample["data"], &names("types"), &names("mods"));
        let flagged: Vec<(usize, u32, &str, u8)> = tokens.iter().filter(|t| t.mods != 0).map(|t| (t.line, t.start, t.kind.as_str(), t.mods)).collect();
        assert_eq!(flagged, [(6, 4, "keyword", MOD_UNSAFE), (6, 13, "operator", MOD_UNSAFE)], "`unsafe` and the dereference inside it");
        assert!(tokens.iter().any(|t| t.kind == "function" && t.line == 1 && t.start == 7 && t.mods == 0), "other modifiers are not drawn");
        // Without the server's table the same data has no flags.
        assert!(crate::lsp::decode_semantic_tokens(&sample["data"], &names("types"), &[]).iter().all(|t| t.mods == 0));
    }

    #[test]
    fn the_real_hir_and_mir_answers_are_plain_text() {
        assert_eq!(fixture("view-hir.json").as_str(), Some("fn add(i32, i32) -> i32 {...}\n"));
        assert!(fixture("view-mir.json").as_str().unwrap().starts_with("fn add() {"));
    }
}
