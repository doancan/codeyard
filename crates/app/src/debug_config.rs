//! Debug configurations: `.vscode/launch.json` / `.codeyard/launch.jsonc` and
//! the ones auto-detected from the project (Cargo, Go, open Python file). Pure text handling.
//!
//! `type` -> adapter: `lldb`, `codelldb`, `cppdbg`, `lldb-dap` -> `lldb-dap` (C, C++, Rust, Swift);
//! `go` -> `dlv dap`; `python`, `debugpy` -> `python3 -m debugpy.adapter`. Other types (node ...)
//! get an explicit "not supported" message; the user can also give their own adapter via the `debugAdapter`
//! field: `{ "command": "...", "args": [] }`.

use serde_json::{Value, json};

use crate::workbench::tasks::{Task, expand_variables};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adapter {
    /// DAP `adapterID`.
    pub id: String,
    pub command: String,
    pub args: Vec<String>,
    /// No process spawn: connect directly over TCP to an already running DAP server (`debugpy --listen`, `dlv --headless`...).
    pub connect: Option<(String, u16)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LaunchConfig {
    pub name: String,
    pub adapter: Result<Adapter, String>,
    /// `launch` or `attach`.
    pub request: String,
    /// Arguments sent to the adapter (`program`, `args`, `cwd`... variables expanded).
    pub args: Value,
    /// Shell command to run before launch (automatic build, or a `preLaunchTask` without a task file).
    pub pre_launch: Option<String>,
    /// `preLaunchTask` from a task file: the task runs as a planned task (dependencies, inputs, background readiness)
    /// and the adapter starts only after it succeeds or becomes ready.
    pub pre_launch_task: Option<String>,
    /// Automatically detected Cargo target; its executable is resolved from build artifacts.
    pub cargo_target: Option<crate::cargo_project::CargoTarget>,
    /// Task to run after the session ends (`postDebugTask`): (label, command).
    pub post_debug: Option<(String, String)>,
    /// "Run Without Debugging": the program runs, breakpoints are not sent.
    pub no_debug: bool,
    /// The launch file's `inputs` this configuration uses (`${input:id}`), asked for when it starts.
    pub inputs: Vec<InputDef>,
}

/// One of the launch file's `inputs`.
#[derive(Debug, Clone, PartialEq)]
pub struct InputDef {
    pub id: String,
    pub kind: InputKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputKind {
    /// Typed text.
    Prompt { description: String, default: String },
    /// One of a list.
    Pick { description: String, options: Vec<String>, default: Option<String> },
    /// `command` inputs would run a command from the file: refused.
    Refused(String),
}

/// A `compounds` entry: several configurations started together.
#[derive(Debug, Clone, PartialEq)]
pub struct Compound {
    pub name: String,
    pub configs: Vec<String>,
    /// Stopping one session stops the others.
    pub stop_all: bool,
    pub pre_launch: Option<String>,
}

/// The `inputs` of a launch file.
pub fn parse_inputs(v: &Value) -> Vec<InputDef> {
    v["inputs"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|i| {
                    let id = i["id"].as_str()?.to_string();
                    let description = i["description"].as_str().unwrap_or(&id).to_string();
                    let kind = match i["type"].as_str() {
                        Some("promptString") => InputKind::Prompt { description, default: i["default"].as_str().unwrap_or_default().to_string() },
                        Some("pickString") => InputKind::Pick {
                            description,
                            options: i["options"]
                                .as_array()
                                .map(|o| o.iter().filter_map(|x| x.as_str().or(x["value"].as_str()).map(str::to_string)).collect())
                                .unwrap_or_default(),
                            default: i["default"].as_str().map(str::to_string),
                        },
                        other => InputKind::Refused(other.unwrap_or("?").to_string()),
                    };
                    Some(InputDef { id, kind })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `compounds` of a launch file.
pub fn parse_compounds(text: &str, root: &str, file: Option<&str>, tasks: &[Task]) -> Vec<Compound> {
    let Ok(v) = serde_json::from_str::<Value>(&theme::themes::strip_jsonc(text)) else { return Vec::new() };
    v["compounds"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    let configs: Vec<String> =
                        c["configurations"].as_array()?.iter().filter_map(|x| x.as_str().or(x["name"].as_str()).map(str::to_string)).collect();
                    Some(Compound {
                        name: c["name"].as_str()?.to_string(),
                        configs,
                        stop_all: c["stopAll"].as_bool().unwrap_or(false),
                        pre_launch: task_command(&c["preLaunchTask"], tasks, root, file).map(|(_, cmd)| cmd),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A task named by `preLaunchTask` / `postDebugTask`: (label, command with variables expanded).
fn task_command(label: &Value, tasks: &[Task], root: &str, file: Option<&str>) -> Option<(String, String)> {
    let label = label.as_str()?;
    tasks.iter().find(|t| t.label == label).map(|t| (t.label.clone(), expand_variables(&t.command, root, file)))
}

/// Adds a configuration to a launch file's text: into `configurations` (created if missing), keeping comments and
/// the rest of the file as they are. An empty or missing file becomes a new one.
pub fn insert_configuration(text: &str, snippet: &Value) -> Result<String, String> {
    let body = serde_json::to_string_pretty(snippet).map_err(|e| e.to_string())?;
    let indent = |s: &str, n: usize| s.lines().map(|l| format!("{}{l}", " ".repeat(n))).collect::<Vec<_>>().join("\n");
    if text.trim().is_empty() {
        return Ok(format!("{{\n  \"version\": \"0.2.0\",\n  \"configurations\": [\n{}\n  ]\n}}\n", indent(&body, 4)));
    }
    // The file must be valid before it is edited.
    let parsed: Value = serde_json::from_str(&theme::themes::strip_jsonc(text)).map_err(|e| crate::i18n::tf("the launch file is not valid JSON: {}", &[&e]))?;
    let code = code_positions(text);
    let find = |needle: &str, from: usize| code.iter().map(|&(i, _)| i).filter(|&i| i >= from).find(|&i| text[i..].starts_with(needle));
    match (&parsed, parsed.get("configurations")) {
        (Value::Object(_), Some(Value::Array(_))) => {
            let key = find("\"configurations\"", 0).ok_or("configurations")?;
            let open = find("[", key).ok_or("[")?;
            let close = matching(text, &code, open).ok_or("]")?;
            // Last code character before `]`: empty array, or an element that needs a comma (a trailing one is kept).
            let last = code.iter().rev().find(|&&(i, c)| i > open && i < close && !c.is_whitespace()).map(|&(_, c)| c);
            let sep = if matches!(last, None | Some(',')) { "" } else { "," };
            let head = text[..close].trim_end();
            let insert_at = head.len();
            Ok(format!("{}{sep}\n{}\n  {}", &text[..insert_at], indent(&body, 4), &text[close..]))
        }
        (Value::Array(_), _) => {
            let close = text.rfind(']').ok_or("]")?;
            let last = code.iter().rev().find(|&&(i, c)| i < close && !c.is_whitespace()).map(|&(_, c)| c);
            let sep = if matches!(last, Some('[' | ',')) { "" } else { "," };
            Ok(format!("{}{sep}\n{}\n{}", text[..close].trim_end(), indent(&body, 2), &text[close..]))
        }
        (Value::Object(_), _) => {
            let open = find("{", 0).ok_or("{")?;
            Ok(format!("{}\n  \"configurations\": [\n{}\n  ],{}", &text[..=open], indent(&body, 4), &text[open + 1..]))
        }
        _ => Err(crate::i18n::t("the launch file is not an object").into()),
    }
}

/// Byte positions of characters outside strings and comments, with the character.
fn code_positions(text: &str) -> Vec<(usize, char)> {
    let mut out = Vec::new();
    let mut it = text.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        match c {
            '"' => {
                out.push((i, c));
                let mut escaped = false;
                for (_, c) in it.by_ref() {
                    match c {
                        '\\' if !escaped => escaped = true,
                        '"' if !escaped => break,
                        _ => escaped = false,
                    }
                }
            }
            '/' if it.peek().is_some_and(|&(_, n)| n == '/') => {
                for (_, c) in it.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '/' if it.peek().is_some_and(|&(_, n)| n == '*') => {
                it.next();
                let mut star = false;
                for (_, c) in it.by_ref() {
                    if star && c == '/' {
                        break;
                    }
                    star = c == '*';
                }
            }
            _ => out.push((i, c)),
        }
    }
    out
}

/// The `]` that closes the `[` at `open`.
fn matching(_text: &str, code: &[(usize, char)], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for &(i, c) in code.iter().filter(|(i, _)| *i >= open) {
        match c {
            '[' | '{' => depth += 1,
            ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// The `${input:id}` ids a configuration uses, in order of first use.
pub fn input_ids(config: &LaunchConfig) -> Vec<String> {
    let mut text = config.args.to_string();
    if let Some(p) = &config.pre_launch {
        text.push_str(p);
    }
    let mut out: Vec<String> = Vec::new();
    let mut rest = text.as_str();
    while let Some(i) = rest.find("${input:") {
        rest = &rest[i + 8..];
        let Some(end) = rest.find('}') else { break };
        let id = rest[..end].to_string();
        if !out.contains(&id) {
            out.push(id);
        }
        rest = &rest[end..];
    }
    out
}

/// Replace `${input:id}` with the answers (in the arguments and the pre-launch command).
pub fn substitute_inputs(config: &mut LaunchConfig, answers: &[(String, String)]) {
    fn walk(v: &mut Value, answers: &[(String, String)]) {
        match v {
            Value::String(s) => {
                for (id, value) in answers {
                    *s = s.replace(&format!("${{input:{id}}}"), value);
                }
            }
            Value::Array(a) => a.iter_mut().for_each(|x| walk(x, answers)),
            Value::Object(o) => o.values_mut().for_each(|x| walk(x, answers)),
            _ => {}
        }
    }
    walk(&mut config.args, answers);
    if let Some(p) = &mut config.pre_launch {
        for (id, value) in answers {
            *p = p.replace(&format!("${{input:{id}}}"), value);
        }
    }
}

/// If `lldb-dap` isn't on PATH, on macOS finds it via Xcode (`xcrun`): a shell, the adapter inherits stdin/stdout.
const LLDB_DAP: &str = "if command -v lldb-dap >/dev/null 2>&1; then exec lldb-dap; elif command -v lldb-vscode >/dev/null 2>&1; then exec lldb-vscode; else exec xcrun lldb-dap; fi";

/// Where js-debug may be installed (`src/dapDebugServer.js`): `CODEYARD_JS_DEBUG`, the folder we
/// download to, VS Code (user extension and the one inside the app), Mason (neovim).
pub fn find_js_debug() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(std::path::PathBuf::from);
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os("CODEYARD_JS_DEBUG").map(std::path::PathBuf::from) {
        candidates.push(if p.extension().is_some() { p } else { p.join("js-debug/src/dapDebugServer.js") });
    }
    candidates.push(adapters_dir().join("js-debug/src/dapDebugServer.js"));
    if let Some(h) = &home {
        for dir in [h.join(".vscode/extensions"), h.join(".vscode-insiders/extensions")] {
            for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                if e.file_name().to_string_lossy().starts_with("ms-vscode.js-debug-") {
                    candidates.push(e.path().join("src/dapDebugServer.js"));
                }
            }
        }
        candidates.push(h.join(".local/share/nvim/mason/packages/js-debug-adapter/js-debug/src/dapDebugServer.js"));
    }
    candidates.push("/Applications/Visual Studio Code.app/Contents/Resources/app/extensions/ms-vscode.js-debug/src/dapDebugServer.js".into());
    if let Some(local) = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from) {
        candidates.push(local.join("Programs/Microsoft VS Code/resources/app/extensions/ms-vscode.js-debug/src/dapDebugServer.js"));
    }
    candidates.push("/usr/share/code/resources/app/extensions/ms-vscode.js-debug/src/dapDebugServer.js".into());
    candidates.into_iter().find(|p| p.is_file())
}

/// Folder for adapters we download ourselves: `<settings dir>/adapters`.
pub fn adapters_dir() -> std::path::PathBuf {
    crate::settings::config_dir().unwrap_or_else(|| std::path::PathBuf::from(".codeyard")).join("adapters")
}

/// `Debug: Install JavaScript Debugger` downloads this (Microsoft js-debug, MIT).
pub const JS_DEBUG_URL: &str = "https://github.com/microsoft/vscode-js-debug/releases/download/v1.140.0/js-debug-dap-v1.140.0.tar.gz";

/// SHA-256 of the downloaded archive (v1.140.0 release file): a partial, corrupt or tampered download is rejected without being run.
/// Updated together with `JS_DEBUG_URL` when the version is bumped.
pub const JS_DEBUG_SHA256: &str = "27dab92937ec1ab35821ae955aac867544fe06a1b6307229049f2d789af10968";

/// Verify the js-debug archive and install under `dir/js-debug`: SHA-256 first, then extract to a temp folder, look there for the expected file,
/// finally a single `rename`. An interrupted install is not counted as "installed" by `find_js_debug()`.
pub fn install_js_debug_archive(dir: &std::path::Path, archive: &std::path::Path, sha256: &str) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let data = std::fs::read(archive).map_err(|e| format!("{}: {e}", archive.display()))?;
    let got: String = Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect();
    if !got.eq_ignore_ascii_case(sha256) {
        return Err(crate::i18n::t("the checksum does not match: the download is incomplete or was altered").into());
    }
    let tmp = dir.join(format!(".js-debug-install-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let result = (|| {
        let out = std::process::Command::new("tar").arg("xzf").arg(archive).arg("-C").arg(&tmp).output().map_err(|e| format!("tar: {e}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        let src = tmp.join("js-debug");
        if !src.join("src/dapDebugServer.js").is_file() {
            return Err(crate::i18n::t("the archive does not contain js-debug/src/dapDebugServer.js").into());
        }
        let dest = dir.join("js-debug");
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::rename(&src, &dest).map_err(|e| format!("{}: {e}", dest.display()))
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

fn node_adapter(server: Option<std::path::PathBuf>) -> Result<Adapter, String> {
    match server {
        Some(server) => Ok(Adapter { id: "pwa-node".into(), command: "node".into(), args: vec![server.to_string_lossy().into_owned(), "${port}".into(), "127.0.0.1".into()], connect: None }),
        None => Err(crate::i18n::t("JavaScript debugging needs Microsoft's js-debug adapter: run “Debug: Install JavaScript Debugger” (a one-time download, MIT licensed), or install VS Code (its copy is used).").into()),
    }
}

pub fn adapter_for(ty: &str) -> Result<Adapter, String> {
    let adapter = |id: &str, command: &str, args: &[&str]| Adapter {
        id: id.into(),
        command: command.into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        connect: None,
    };
    match ty {
        // No shell script on Windows: the adapter comes straight from PATH.
        "lldb" | "codelldb" | "cppdbg" | "lldb-dap" | "cppvsdbg" if cfg!(windows) => Ok(adapter("lldb-dap", "lldb-dap", &[])),
        "lldb" | "codelldb" | "cppdbg" | "lldb-dap" | "cppvsdbg" => Ok(adapter("lldb-dap", "sh", &["-c", LLDB_DAP])),
        // Delve only speaks TCP: it connects to us (`${clientAddr}` is filled in dap.rs).
        "go" | "delve" => Ok(adapter("go", "dlv", &["dap", "--client-addr", "${clientAddr}"])),
        "python" | "debugpy" => Ok(adapter("debugpy", "python3", &["-m", "debugpy.adapter"])),
        // js-debug speaks TCP: `${port}` is filled in dap.rs.
        "node" | "pwa-node" | "node-terminal" => node_adapter(find_js_debug()),
        other => Err(crate::i18n::tf(
            "No debug adapter for type “{0}”. Supported: lldb (C, C++, Rust, Swift), go, python; or add {1} to the configuration.",
            &[&other, &"\"debugAdapter\": { \"command\": …, \"args\": […] }"],
        )),
    }
}

/// From `launch.json`: variables (`${workspaceFolder}`, `${file}`...) are expanded, `preLaunchTask`
/// is looked up by label in `tasks` (if not found the label isn't treated as a command, ignored).
pub fn parse_launch_json(text: &str, root: &str, file: Option<&str>, tasks: &[Task]) -> Vec<LaunchConfig> {
    let Ok(v) = serde_json::from_str::<Value>(&theme::themes::strip_jsonc(text)) else { return Vec::new() };
    let items = match &v {
        Value::Array(a) => a.clone(),
        Value::Object(o) => o.get("configurations").and_then(Value::as_array).cloned().unwrap_or_default(),
        _ => Vec::new(),
    };
    let all_inputs = parse_inputs(&v);
    items
        .iter()
        .filter_map(|c| {
            let name = c["name"].as_str()?.to_string();
            let ty = c["type"].as_str().unwrap_or_default();
            // `connect: { host, port }` (debugpy `--listen`, VS Code Python form) or `debugAdapter: { host, port }` for your own
            // adapter: connect straight to that server over TCP, no process spawn.
            let direct =
                |v: &Value| Some((v["host"].as_str().unwrap_or("127.0.0.1").to_string(), v["port"].as_u64().filter(|p| (1..=65535).contains(p))? as u16));
            let is_attach = c["request"].as_str() == Some("attach");
            let adapter = if let Some(addr) = direct(&c["debugAdapter"]).filter(|_| c["debugAdapter"]["command"].is_null()) {
                Ok(Adapter { id: ty.to_string(), command: String::new(), args: Vec::new(), connect: Some(addr) })
            } else if let (true, true, Some(addr)) = (is_attach, matches!(ty, "python" | "debugpy"), direct(&c["connect"])) {
                Ok(Adapter { id: "debugpy".into(), command: String::new(), args: Vec::new(), connect: Some(addr) })
            } else {
                match c["debugAdapter"]["command"].as_str() {
                    Some(cmd) => Ok(Adapter {
                        id: ty.to_string(),
                        command: cmd.to_string(),
                        args: c["debugAdapter"]["args"]
                            .as_array()
                            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                            .unwrap_or_default(),
                        connect: None,
                    }),
                    None => adapter_for(ty),
                }
            };
            let mut args = c.clone();
            let is_node = matches!(ty, "node" | "pwa-node" | "node-terminal");
            if let Some(o) = args.as_object_mut() {
                for k in ["name", "type", "request", "preLaunchTask", "postDebugTask", "debugAdapter"] {
                    o.remove(k);
                }
                // For debugpy connected directly, the address is the connection, not an `attach` argument.
                if matches!(adapter, Ok(Adapter { connect: Some(_), .. })) {
                    o.remove("connect");
                }
                // js-debug wants type and name inside the options; let output reach the DAP console.
                if is_node {
                    o.insert("type".into(), json!("pwa-node"));
                    o.insert("name".into(), json!(name));
                    o.entry("console").or_insert_with(|| json!("internalConsole"));
                }
                // Project root if no working directory is given.
                o.entry("cwd").or_insert_with(|| json!("${workspaceFolder}"));
            }
            let args = expand_value(&args, root, file);
            let pre = c["preLaunchTask"].as_str().and_then(|label| tasks.iter().find(|t| t.label == label));
            let pre_launch_task = pre.filter(|t| t.file.is_some()).map(|t| t.label.clone());
            let pre_launch = pre.filter(|t| t.file.is_none()).map(|t| expand_variables(&t.command, root, file));
            let post_debug = task_command(&c["postDebugTask"], tasks, root, file);
            let mut config = LaunchConfig {
                name,
                adapter,
                request: c["request"].as_str().unwrap_or("launch").to_string(),
                args,
                pre_launch,
                pre_launch_task,
                cargo_target: None,
                post_debug,
                no_debug: false,
                inputs: Vec::new(),
            };
            let used = input_ids(&config);
            config.inputs = all_inputs.iter().filter(|i| used.contains(&i.id)).cloned().collect();
            Some(config)
        })
        .collect()
}

fn expand_value(v: &Value, root: &str, file: Option<&str>) -> Value {
    match v {
        Value::String(s) => Value::String(expand_variables(s, root, file)),
        Value::Array(a) => Value::Array(a.iter().map(|x| expand_value(x, root, file)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), expand_value(x, root, file))).collect()),
        other => other.clone(),
    }
}

/// Auto from the project: `Cargo.toml` (`[package] name`) -> build and debug the Cargo binary;
/// `go.mod` -> run the package with `dlv dap`; open `.py` file -> debugpy.
pub fn detect(cargo_toml: Option<&str>, has_go_mod: bool, file: Option<&str>, root: &str) -> Vec<LaunchConfig> {
    let mut out = Vec::new();
    if let Some(name) = cargo_toml.and_then(package_name) {
        out.push(LaunchConfig {
            name: crate::i18n::tf("Debug Rust: {}", &[&name]),
            adapter: adapter_for("lldb"),
            request: "launch".into(),
            args: json!({ "args": [], "cwd": root }),
            pre_launch: None,
            pre_launch_task: None,
            cargo_target: Some(crate::cargo_project::CargoTarget::binary(name)),
            post_debug: None,
            no_debug: false,
            inputs: Vec::new(),
        });
    }
    if has_go_mod {
        out.push(LaunchConfig {
            name: crate::i18n::t("Debug Go package").into(),
            adapter: adapter_for("go"),
            request: "launch".into(),
            // outputMode remote: program output reaches the console as a DAP event (default: delve's stdout).
            args: json!({ "mode": "debug", "program": root, "cwd": root, "outputMode": "remote" }),
            pre_launch: None,
            pre_launch_task: None,
            cargo_target: None,
            post_debug: None,
            no_debug: false,
            inputs: Vec::new(),
        });
    }
    if let Some(f) = file.filter(|f| [".js", ".mjs", ".cjs"].iter().any(|e| f.ends_with(e))) {
        let base = f.rsplit('/').next().unwrap_or(f);
        out.push(LaunchConfig {
            name: crate::i18n::tf("Debug Node: {}", &[&base]),
            adapter: adapter_for("node"),
            request: "launch".into(),
            args: json!({ "type": "pwa-node", "name": format!("Debug Node: {base}"), "program": f, "cwd": root, "console": "internalConsole" }),
            pre_launch: None,
            pre_launch_task: None,
            cargo_target: None,
            post_debug: None,
            no_debug: false,
            inputs: Vec::new(),
        });
    }
    if let Some(f) = file.filter(|f| f.ends_with(".py")) {
        let base = f.rsplit('/').next().unwrap_or(f);
        out.push(LaunchConfig {
            name: crate::i18n::tf("Debug Python: {}", &[&base]),
            adapter: adapter_for("python"),
            request: "launch".into(),
            args: json!({ "program": f, "cwd": root, "console": "internalConsole" }),
            pre_launch: None,
            pre_launch_task: None,
            cargo_target: None,
            post_debug: None,
            no_debug: false,
            inputs: Vec::new(),
        });
    }
    out
}

/// `name = "..."` in the `[package]` section (lines outside the table are ignored).
fn package_name(cargo: &str) -> Option<String> {
    let mut in_package = false;
    for line in cargo.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_package = l == "[package]";
        } else if in_package
            && let Some((k, v)) = l.split_once('=')
            && k.trim() == "name"
        {
            return Some(v.trim().trim_matches('"').trim_matches('\'').to_string());
        }
    }
    None
}

// --- Rust values in lldb-dap ---

/// The toolchain that builds the program being debugged, as `rustc --print sysroot && rustc -vV` reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct RustToolchain {
    pub sysroot: String,
    /// `None` for a locally built toolchain (`commit-hash: unknown`) or anything that is not a hash.
    pub commit_hash: Option<String>,
    /// Target triple of the toolchain's host (`aarch64-apple-darwin`).
    pub host: Option<String>,
}

/// Reads the output of `rustc --print sysroot && rustc -vV`: the sysroot is the first line, the rest is `key: value`.
pub fn parse_rustc_info(out: &str) -> Option<RustToolchain> {
    let mut lines = out.lines().map(str::trim).filter(|l| !l.is_empty());
    let sysroot = lines.next()?.to_string();
    if sysroot.contains(": ") || !(sysroot.starts_with('/') || sysroot.as_bytes().get(1) == Some(&b':')) {
        return None;
    }
    let (mut commit_hash, mut host) = (None, None);
    for line in lines {
        match line.split_once(": ") {
            Some(("commit-hash", h)) if (7..=64).contains(&h.len()) && h.bytes().all(|b| b.is_ascii_hexdigit()) => commit_hash = Some(h.to_string()),
            Some(("host", h)) if !h.is_empty() => host = Some(h.to_string()),
            _ => {}
        }
    }
    Some(RustToolchain { sysroot: sysroot.trim_end_matches(['/', '\\']).to_string(), commit_hash, host })
}

/// Whether a configuration debugs Rust: lldb-dap (or an `lldb` entry with its own adapter command) in a folder with a
/// `Cargo.toml`. A C program in a folder without one, and every other adapter, is left as it is.
pub fn is_rust_config(config: &LaunchConfig, has_cargo_toml: bool) -> bool {
    has_cargo_toml && config.adapter.as_ref().is_ok_and(|a| matches!(a.id.as_str(), "lldb-dap" | "lldb"))
}

/// A path as one argument of an lldb command line.
fn lldb_quote(path: &str) -> String {
    format!("\"{}\"", path.replace('\\', "\\\\").replace('"', "\\\""))
}

/// `initCommands` that load the toolchain's formatters (`String`, `Vec`, `HashMap` as values) and, with `rust-src`, map the
/// standard library's build paths to its sources; the configuration's own commands follow, in their order. Toolchains
/// before 1.89 also ship `lldb_commands`, which is sourced when present. A configuration that imports `lldb_lookup`
/// itself is left alone.
pub fn rust_init_commands(tc: &RustToolchain, has_commands_file: bool, has_src: bool, existing: &[String]) -> Vec<String> {
    if existing.iter().any(|c| c.contains("lldb_lookup")) {
        return existing.to_vec();
    }
    let etc = format!("{}/lib/rustlib/etc", tc.sysroot);
    let mut out = vec![format!("command script import {}", lldb_quote(&format!("{etc}/lldb_lookup.py")))];
    if has_commands_file {
        out.push(format!("command source {}", lldb_quote(&format!("{etc}/lldb_commands"))));
    }
    if let (true, Some(hash)) = (has_src, &tc.commit_hash) {
        out.push(format!("settings set target.source-map /rustc/{hash} {}", lldb_quote(&format!("{}/lib/rustlib/src/rust", tc.sysroot))));
    }
    out.extend(existing.iter().cloned());
    out
}

/// The `initCommands` a Rust configuration starts with, found on the machine that runs the program (`rustc` runs in the
/// project root, so `rust-toolchain.toml` picks the toolchain). `Ok(None)`: nothing to add (Windows MSVC, whose debug
/// info lldb's Rust formatters are not made for). `Err`: why the formatters are not loaded.
pub fn rust_formatters(host: &dyn host::Host, root: &str, existing: &[String]) -> Result<Option<Vec<String>>, String> {
    // rustup would download a toolchain the project names but nobody installed: asking about it must not start a download.
    let (shell, flag, probe) = if cfg!(windows) && !host.is_remote() {
        ("cmd", "/C", "set RUSTUP_AUTO_INSTALL=0&& rustc --print sysroot && rustc -vV")
    } else {
        ("sh", "-c", "RUSTUP_AUTO_INSTALL=0 rustc --print sysroot && RUSTUP_AUTO_INSTALL=0 rustc -vV")
    };
    let info = host
        .exec(shell, &[flag, probe], Some(root), None)
        .ok()
        .filter(|o| o.ok())
        .and_then(|o| parse_rustc_info(&o.stdout_str()))
        .ok_or_else(|| crate::i18n::t("rustc not found").to_string())?;
    if info.host.as_deref().is_some_and(|h| h.ends_with("windows-msvc")) {
        return Ok(None);
    }
    let exists = |p: &str| host.stat(&format!("{}/lib/rustlib/{p}", info.sysroot)).is_ok();
    if !exists("etc/lldb_lookup.py") {
        return Err(crate::i18n::t("files not found in the toolchain").to_string());
    }
    Ok(Some(rust_init_commands(&info, exists("etc/lldb_commands"), exists("src/rust/library"), existing)))
}

#[cfg(test)]
mod tests {
    #[test]
    fn compounds_inputs_and_the_post_debug_task_are_read() {
        let tasks =
            vec![Task::shell("build".into(), "make".into(), "tasks.jsonc"), Task::shell("clean".into(), "rm -rf ${workspaceFolder}/out".into(), "tasks.jsonc")];
        let text = r#"{
          // comment
          "configurations": [
            { "name": "Server", "type": "lldb", "request": "launch", "program": "${input:binary}", "args": ["--port", "${input:port}"], "postDebugTask": "clean" },
            { "name": "Client", "type": "lldb", "request": "launch", "program": "/bin/client" }
          ],
          "compounds": [ { "name": "Both", "configurations": ["Server", "Client"], "stopAll": true, "preLaunchTask": "build" }, { "configurations": [] } ],
          "inputs": [
            { "id": "port", "type": "pickString", "description": "Port", "options": ["8080", { "label": "alt", "value": "9090" }], "default": "9090" },
            { "id": "binary", "type": "promptString", "description": "Binary", "default": "/bin/server" },
            { "id": "danger", "type": "command", "command": "shell.run" }
          ]
        }"#;
        let configs = parse_launch_json(text, "/p", None, &tasks);
        assert_eq!(configs[0].post_debug, Some(("clean".into(), "rm -rf /p/out".into())));
        assert!(configs[0].args.get("postDebugTask").is_none(), "not passed to the adapter");
        assert_eq!(input_ids(&configs[0]), vec!["binary", "port"]);
        assert_eq!(configs[0].inputs.len(), 2, "only the inputs it uses");
        assert!(
            matches!(&configs[0].inputs.iter().find(|i| i.id == "port").unwrap().kind, InputKind::Pick { options, default: Some(d), .. } if options == &["8080", "9090"] && d == "9090")
        );
        assert!(configs[1].inputs.is_empty() && configs[1].post_debug.is_none());
        let mut c = configs[0].clone();
        substitute_inputs(&mut c, &[("binary".into(), "/srv/app".into()), ("port".into(), "8080".into())]);
        assert_eq!(c.args["program"], "/srv/app");
        assert_eq!(c.args["args"], serde_json::json!(["--port", "8080"]));
        assert!(input_ids(&c).is_empty());
        let compounds = parse_compounds(text, "/p", None, &tasks);
        assert_eq!(
            compounds,
            vec![Compound { name: "Both".into(), configs: vec!["Server".into(), "Client".into()], stop_all: true, pre_launch: Some("make".into()) }]
        );
        let v: Value = serde_json::from_str(r#"{ "inputs": [ { "id": "x", "type": "command" } ] }"#).unwrap();
        assert!(matches!(&parse_inputs(&v)[0].kind, InputKind::Refused(k) if k == "command"), "command inputs are never run");
    }

    #[test]
    fn a_configuration_is_added_without_touching_the_rest_of_the_file() {
        let snippet = serde_json::json!({ "name": "New", "type": "go" });
        let names = |text: &str| parse_launch_json(text, "/p", None, &[]).into_iter().map(|c| c.name).collect::<Vec<_>>();
        // A file with comments and a trailing comma.
        let text = "{\n  // keep me\n  \"configurations\": [\n    { \"name\": \"Old\", \"type\": \"go\" }, // and me\n  ],\n}\n";
        let out = insert_configuration(text, &snippet).unwrap();
        assert!(out.contains("// keep me") && out.contains("// and me"), "{out}");
        assert_eq!(names(&out), vec!["Old", "New"]);
        // No trailing comma, a `]` inside a string and a comment.
        let text = r#"{ "configurations": [ { "name": "A ]", "type": "go" } /* ] */ ] }"#;
        assert_eq!(names(&insert_configuration(text, &snippet).unwrap()), vec!["A ]", "New"]);
        // Empty array, no key, an array file, an empty file.
        assert_eq!(names(&insert_configuration(r#"{ "configurations": [] }"#, &snippet).unwrap()), vec!["New"]);
        assert_eq!(names(&insert_configuration(r#"{ "version": "0.2.0" }"#, &snippet).unwrap()), vec!["New"]);
        assert_eq!(names(&insert_configuration(r#"[ { "name": "A", "type": "go" } ]"#, &snippet).unwrap()), vec!["A", "New"]);
        assert_eq!(names(&insert_configuration("  ", &snippet).unwrap()), vec!["New"]);
        assert!(insert_configuration("{ not json", &snippet).is_err(), "a broken file is not edited");
    }

    use super::*;

    fn sha256_of(path: &std::path::Path) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(std::fs::read(path).unwrap()).iter().map(|b| format!("{b:02x}")).collect()
    }

    fn archive_with(files: &[(&str, &str)], name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("codeyard-jsdbg-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let content = root.join("content");
        for (rel, text) in files {
            let p = content.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        let archive = root.join("a.tar.gz");
        let ok = std::process::Command::new("tar").arg("czf").arg(&archive).arg("-C").arg(&content).arg(".").status().map(|s| s.success()).unwrap_or(false);
        assert!(ok, "tar is available");
        let dest = root.join("adapters");
        std::fs::create_dir_all(&dest).unwrap();
        (archive, dest)
    }

    #[test]
    fn the_js_debug_archive_is_verified_and_installed_whole_or_not_at_all() {
        let (archive, dest) = archive_with(&[("js-debug/src/dapDebugServer.js", "// server")], "ok");
        let sha = sha256_of(&archive);
        // Wrong checksum (truncated or altered download): nothing is installed.
        let e = install_js_debug_archive(&dest, &archive, &"0".repeat(64)).unwrap_err();
        assert!(e.contains("checksum"), "{e}");
        assert!(!dest.join("js-debug").exists());
        // Right checksum: installed, no temporary folder left.
        install_js_debug_archive(&dest, &archive, &sha).unwrap();
        assert!(dest.join("js-debug/src/dapDebugServer.js").is_file());
        assert_eq!(std::fs::read_dir(&dest).unwrap().flatten().count(), 1, "the install folder was cleaned up");
        // Installing again over an existing copy works.
        install_js_debug_archive(&dest, &archive, &sha.to_uppercase()).unwrap();
        // An archive without the expected file is refused even with a matching checksum.
        let (other, dest2) = archive_with(&[("readme.txt", "x")], "wrong");
        let e = install_js_debug_archive(&dest2, &other, &sha256_of(&other)).unwrap_err();
        assert!(e.contains("does not contain"), "{e}");
        assert!(!dest2.join("js-debug").exists(), "a half install is never left as 'installed'");
    }

    fn task(label: &str, command: &str) -> Task {
        Task::shell(label.into(), command.into(), "tasks.json")
    }

    #[test]
    fn launch_json_expands_variables_and_finds_the_pre_launch_task() {
        let text = r#"{ "version": "0.2.0", "configurations": [
            // VS Code form
            { "name": "Run app", "type": "lldb", "request": "launch", "program": "${workspaceFolder}/build/app",
              "args": ["--port", "${workspaceFolderBasename}"], "preLaunchTask": "build" },
            { "name": "Py", "type": "python", "request": "launch", "program": "${file}", "cwd": "${workspaceFolder}/src" },
            { "name": "Ruby", "type": "ruby", "program": "x.rb" },
            { "name": "Mine", "type": "custom", "debugAdapter": { "command": "my-dap", "args": ["--stdio"] }, "program": "x" },
            { "type": "lldb" }
        ] }"#;
        let c = parse_launch_json(text, "/p/demo", Some("/p/demo/src/m.py"), &[task("build", "make -C ${workspaceFolder}")]);
        assert_eq!(c.len(), 4, "unnamed configuration is skipped");
        assert_eq!(c[0].name, "Run app");
        assert_eq!(c[0].args["program"], "/p/demo/build/app");
        assert_eq!(c[0].args["args"], json!(["--port", "demo"]));
        assert_eq!(c[0].args["cwd"], "/p/demo", "project root when cwd is not given");
        assert!(c[0].args.get("preLaunchTask").is_none() && c[0].args.get("type").is_none(), "meta fields do not go to the adapter");
        assert_eq!(c[0].pre_launch.as_deref(), Some("make -C /p/demo"));
        assert_eq!(c[0].adapter.as_ref().unwrap().id, "lldb-dap");
        assert_eq!((c[1].args["program"].as_str(), c[1].args["cwd"].as_str()), (Some("/p/demo/src/m.py"), Some("/p/demo/src")));
        assert_eq!(c[1].adapter.as_ref().unwrap().args, ["-m", "debugpy.adapter"]);
        assert!(c[2].adapter.as_ref().unwrap_err().contains("ruby"), "unsupported type gives an explicit message");
        assert_eq!(c[3].adapter.as_ref().unwrap().command, "my-dap");
        assert_eq!(c[3].request, "launch");
    }

    #[test]
    fn attach_by_socket_connects_directly_instead_of_spawning_an_adapter() {
        let text = r#"{ "configurations": [
            { "name": "Py", "type": "python", "request": "attach", "connect": { "host": "localhost", "port": 5678 }, "justMyCode": false },
            { "name": "PyDefaultHost", "type": "debugpy", "request": "attach", "connect": { "port": 5679 } },
            { "name": "Headless", "type": "go", "request": "attach", "mode": "remote", "debugAdapter": { "host": "127.0.0.1", "port": 2345 } },
            { "name": "NotAttach", "type": "python", "request": "launch", "program": "x.py", "connect": { "port": 1 } },
            { "name": "BadPort", "type": "python", "request": "attach", "connect": { "port": 0 } }
        ] }"#;
        let c = parse_launch_json(text, "/p", None, &[]);
        let a = |i: usize| c[i].adapter.clone().unwrap();
        assert_eq!(a(0).connect, Some(("localhost".to_string(), 5678)));
        assert_eq!(a(0).id, "debugpy");
        assert_eq!(a(1).connect, Some(("127.0.0.1".to_string(), 5679)));
        assert_eq!(a(2).connect, Some(("127.0.0.1".to_string(), 2345)));
        assert!(c[0].args.get("connect").is_none() && c[0].args["justMyCode"] == false, "address is not an attach argument; other options pass through");
        assert_eq!(a(3).connect, None, "connect is not an address for launch");
        assert_eq!(a(4).connect, None, "invalid port is ignored (falls back to debugpy.adapter)");
    }

    #[test]
    fn bad_json_gives_no_configurations() {
        assert!(parse_launch_json("{ not json", "/p", None, &[]).is_empty());
        assert!(parse_launch_json("{}", "/p", None, &[]).is_empty());
    }

    #[test]
    fn detects_cargo_go_and_python() {
        let cargo = "[workspace]\nmembers = []\n\n[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[dependencies]\nname = \"not-this\"\n";
        let c = detect(Some(cargo), true, Some("/p/a.py"), "/p/");
        assert_eq!(c.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Debug Rust: demo", "Debug Go package", "Debug Python: a.py"]);
        assert!(c[0].args.get("program").is_none());
        assert_eq!(c[0].cargo_target.as_ref().map(|t| t.name.as_str()), Some("demo"));
        assert!(c[0].pre_launch.is_none());
        assert!(detect(Some("[workspace]\nmembers = []\n"), false, Some("/p/a.rs"), "/p").is_empty(), "workspace without a package and a non-.py file");
    }

    #[test]
    fn node_uses_js_debug_over_tcp_and_explains_when_it_is_missing() {
        let a = node_adapter(Some("/x/js-debug/src/dapDebugServer.js".into())).unwrap();
        assert_eq!((a.id.as_str(), a.command.as_str()), ("pwa-node", "node"));
        assert_eq!(a.args, ["/x/js-debug/src/dapDebugServer.js", "${port}", "127.0.0.1"]);
        assert_eq!(crate::dap::transport_of(&a.args), crate::dap::Transport::Connect);
        assert!(node_adapter(None).unwrap_err().contains("Install JavaScript Debugger"));
        // Go: delve connects to us.
        assert_eq!(crate::dap::transport_of(&adapter_for("go").unwrap().args), crate::dap::Transport::Accept);
        assert_eq!(crate::dap::transport_of(&adapter_for("python").unwrap().args), crate::dap::Transport::Stdio);
    }

    #[test]
    fn node_launch_arguments_carry_type_name_and_console() {
        let c = parse_launch_json(
            r#"{ "configurations": [ { "name": "Server", "type": "node", "request": "launch", "program": "${workspaceFolder}/server.js" } ] }"#,
            "/p",
            None,
            &[],
        );
        assert_eq!(c[0].args["type"], "pwa-node");
        assert_eq!(c[0].args["name"], "Server");
        assert_eq!(c[0].args["console"], "internalConsole");
        assert_eq!(c[0].args["program"], "/p/server.js");
        let d = detect(None, false, Some("/p/app.mjs"), "/p");
        assert_eq!(d[0].name, "Debug Node: app.mjs");
        assert_eq!(d[0].args["program"], "/p/app.mjs");
    }

    #[test]
    fn unknown_type_message_lists_what_is_supported() {
        let e = adapter_for("ruby").unwrap_err();
        assert!(e.contains("lldb") && e.contains("go") && e.contains("python") && e.contains("debugAdapter"), "{e}");
    }

    fn toolchain(sysroot: &str, hash: Option<&str>) -> RustToolchain {
        RustToolchain { sysroot: sysroot.into(), commit_hash: hash.map(str::to_string), host: Some("aarch64-apple-darwin".into()) }
    }

    #[test]
    fn rust_init_commands_come_first_and_keep_the_user_s() {
        let tc = toolchain("/home/u/.rustup/toolchains/stable", Some("58cf8f9c35b355199727627d07ae34a4e2846d67"));
        let user = vec!["settings set target.x 1".to_string(), "breakpoint set -n main".to_string()];
        assert_eq!(
            rust_init_commands(&tc, true, true, &user),
            [
                r#"command script import "/home/u/.rustup/toolchains/stable/lib/rustlib/etc/lldb_lookup.py""#,
                r#"command source "/home/u/.rustup/toolchains/stable/lib/rustlib/etc/lldb_commands""#,
                r#"settings set target.source-map /rustc/58cf8f9c35b355199727627d07ae34a4e2846d67 "/home/u/.rustup/toolchains/stable/lib/rustlib/src/rust""#,
                "settings set target.x 1",
                "breakpoint set -n main",
            ]
        );
        // Newer toolchains have no lldb_commands; without rust-src or a hash there is no source map.
        assert_eq!(rust_init_commands(&tc, false, false, &[]).len(), 1);
        assert_eq!(rust_init_commands(&toolchain("/t", None), false, true, &[]).len(), 1, "unknown hash: no map");
        // The user already imports the formatters: nothing is added.
        let own = vec!["command script import /x/lldb_lookup.py".to_string()];
        assert_eq!(rust_init_commands(&tc, true, true, &own), own);
    }

    #[test]
    fn sysroot_paths_are_quoted_for_lldb() {
        let tc = toolchain(r#"/Users/Ayşe Yılmaz/my "rust"\tools"#, None);
        assert_eq!(
            rust_init_commands(&tc, false, false, &[])[0],
            r#"command script import "/Users/Ayşe Yılmaz/my \"rust\"\\tools/lib/rustlib/etc/lldb_lookup.py""#
        );
    }

    #[test]
    fn rustc_info_is_read_and_garbage_rejected() {
        let out = "/home/u/.rustup/toolchains/stable\nrustc 1.99.1 (58cf8f9c3 2026-10-13)\nbinary: rustc\ncommit-hash: 58cf8f9c35b355199727627d07ae34a4e2846d67\nhost: x86_64-unknown-linux-gnu\nrelease: 1.99.1\n";
        assert_eq!(
            parse_rustc_info(out),
            Some(RustToolchain {
                sysroot: "/home/u/.rustup/toolchains/stable".into(),
                commit_hash: Some("58cf8f9c35b355199727627d07ae34a4e2846d67".into()),
                host: Some("x86_64-unknown-linux-gnu".into())
            })
        );
        let local = parse_rustc_info("/opt/rust/\ncommit-hash: unknown\nhost: aarch64-apple-darwin\n").unwrap();
        assert_eq!((local.sysroot.as_str(), local.commit_hash), ("/opt/rust", None));
        assert_eq!(parse_rustc_info(r"C:\Users\u\.rustup\toolchains\stable-x86_64-pc-windows-msvc").unwrap().commit_hash, None);
        for garbage in ["", "\n\n", "error: no such command\n", "rustc 1.99.1\n", "relative/path\n", "commit-hash: abc\n"] {
            assert_eq!(parse_rustc_info(garbage), None, "{garbage:?}");
        }
        assert_eq!(parse_rustc_info("/t\ncommit-hash: not-hex-at-all\n").unwrap().commit_hash, None);
    }

    #[test]
    fn rust_configurations_are_lldb_in_a_cargo_folder() {
        let root = "/p";
        let cargo = detect(Some("[package]\nname = \"demo\"\n"), false, None, root);
        assert!(is_rust_config(&cargo[0], true), "the automatic Cargo configuration");
        let launch = parse_launch_json(
            r#"{ "configurations": [
                { "name": "Native", "type": "lldb", "request": "launch", "program": "x" },
                { "name": "Attach", "type": "lldb", "request": "attach", "pid": 1 },
                { "name": "Own lldb-dap", "type": "lldb", "debugAdapter": { "command": "/opt/lldb-dap" }, "program": "x" },
                { "name": "Go", "type": "go", "request": "launch", "program": "." },
                { "name": "Py", "type": "python", "request": "launch", "program": "a.py" },
                { "name": "Other", "type": "custom", "debugAdapter": { "command": "x" } }
            ] }"#,
            root,
            None,
            &[],
        );
        let rust: Vec<bool> = launch.iter().map(|c| is_rust_config(c, true)).collect();
        assert_eq!(rust, [true, true, true, false, false, false]);
        assert!(!is_rust_config(&launch[0], false), "a C program in a folder without Cargo.toml");
        let py = detect(None, false, Some("/p/a.py"), root);
        assert!(!is_rust_config(&py[0], true));
    }
}
