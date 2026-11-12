//! What the gutter and *Rust: Run Runnable at Cursor* offer: rust-analyzer's `experimental/runnables` turned into command
//! lines for the terminal and plans for the debugger, with the project's own test finder (`testrun`) taking over when
//! there is no server. Plain logic: no UI, no I/O.

use serde_json::Value;

use crate::cargo_project::CrateContext;
use crate::lsp::{Runnable, RunnableCommand, RunnableKind};
use crate::shell_quote::word;

/// How the user's settings change what runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// `rust.target`: the triple every Cargo command builds for.
    pub target: Option<String>,
    /// The folder the command runs in. A runnable of a Cargo workspace somewhere else names its manifest.
    pub root: Option<String>,
    /// Tests run through cargo-nextest (`rust.testRunner`, and it is installed).
    pub nextest: bool,
    /// The command goes to cmd or PowerShell, not to a POSIX shell.
    pub windows: bool,
}

/// What debugging a runnable takes: build, then start the program the build reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DebugPlan {
    /// Arguments of the `cargo` build (without `--message-format`).
    pub build_args: Vec<String>,
    /// Arguments of the program under the debugger.
    pub program_args: Vec<String>,
    /// The build makes test executables (the program is picked among those, not among binaries).
    pub test: bool,
    /// The folder the program runs in: the package's, as `cargo test` and `cargo run` do (relative to the opened folder
    /// when it is not absolute). `None`: the opened folder.
    pub cwd: Option<String>,
}

impl DebugPlan {
    /// What debugging does, for the list: the Cargo build, then the program started under the debugger with its arguments.
    pub fn describe(&self, windows: bool) -> String {
        let quote = |words: &[String]| words.iter().map(|w| word_for(w, windows).unwrap_or_else(|| w.clone())).collect::<Vec<_>>().join(" ");
        let build = format!("cargo {}", quote(&self.build_args));
        if self.program_args.is_empty() { build } else { format!("{build} \u{2192} {}", quote(&self.program_args)) }
    }
}

/// One thing the gutter mark on a line can start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// rust-analyzer's name for it (`test tests::adds_up`, `run demo`).
    pub label: String,
    pub kind: RunnableKind,
    /// 0-based line of the mark.
    pub line: u32,
    /// The command line *Run* types into the terminal.
    pub command: String,
    pub debug: Option<DebugPlan>,
}

/// Characters a Windows shell reads as more than text: cmd and PowerShell disagree about them, so a word with one is not run.
const WINDOWS_SPECIAL: &[char] = &['"', '\'', '`', '$', '%', '^', '&', '|', '<', '>', '(', ')', ';', '!', '\n', '\r'];

/// A command word: left alone when plain, quoted when a shell would read it as something else. Flags stay readable.
/// On Windows a word with spaces is double-quoted and one with characters cmd and PowerShell read differently has no
/// spelling that suits both: `None`.
pub(super) fn word_for(s: &str, windows: bool) -> Option<String> {
    let plain_flag = s
        .strip_prefix('-')
        .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '=' | '.' | ',' | '/' | ':' | '+')));
    if plain_flag {
        return Some(s.to_string());
    }
    if !windows {
        return Some(word(s));
    }
    if s.contains(WINDOWS_SPECIAL) || s.ends_with('\\') {
        return None;
    }
    let plain = !s.is_empty() && !s.starts_with('-') && !s.contains(char::is_whitespace) && !s.contains(',');
    Some(if plain { s.to_string() } else { format!("\"{s}\"") })
}

/// All of `words` as command words, or `None` when one cannot be written for the shell.
fn words_for(words: &[String], windows: bool) -> Option<Vec<String>> {
    words.iter().map(|w| word_for(w, windows)).collect()
}

/// Whether Cargo is already told which target to build for.
fn has_target(args: &[String]) -> bool {
    args.iter().any(|a| a == "--target" || a.starts_with("--target="))
}

/// Whether two folder paths are the same folder.
fn same_folder(a: &str, b: &str) -> bool {
    a.trim_end_matches(['/', '\\']) == b.trim_end_matches(['/', '\\'])
}

/// `args` with `--manifest-path` after the subcommand when the runnable belongs to a Cargo workspace other than the
/// folder the command runs in (rust-analyzer's `workspaceRoot`).
fn with_manifest(args: &[String], workspace_root: Option<&String>, options: &Options) -> Vec<String> {
    let mut out = args.to_vec();
    if let (Some(workspace), Some(folder)) = (workspace_root, options.root.as_ref())
        && !same_folder(workspace, folder)
        && !out.iter().any(|a| a == "--manifest-path" || a.starts_with("--manifest-path="))
        && !out.is_empty()
    {
        out.splice(1..1, ["--manifest-path".to_string(), format!("{}/Cargo.toml", workspace.trim_end_matches(['/', '\\']))]);
    }
    out
}

/// `args` with `--target <triple>` after the subcommand's arguments, when the subcommand builds and none is there.
fn with_target(args: &[String], extra: &[String], options: &Options) -> Vec<String> {
    let mut out: Vec<String> = args.iter().chain(extra).cloned().collect();
    let builds = matches!(args.first().map(String::as_str), Some("run" | "test" | "bench" | "check" | "build" | "nextest"));
    if let Some(target) = options.target.as_ref().filter(|_| builds && !has_target(&out)) {
        out.extend(["--target".into(), target.clone()]);
    }
    out
}

/// The command line of a Cargo runnable; `None` for the ones that are not Cargo's.
pub fn command_line(runnable: &Runnable, options: &Options) -> Option<String> {
    let RunnableCommand::Cargo { cargo_args, extra_args, executable_args, workspace_root, .. } = &runnable.command else { return None };
    let mut words = vec!["cargo".to_string()];
    let args = with_target(&with_manifest(cargo_args, workspace_root.as_ref(), options), extra_args, options);
    if options.nextest
        && let Some((_, rest)) = args.split_first().filter(|(first, _)| *first == "test")
        && let Some(nextest) = super::nextest::nextest_words(rest, executable_args, false)
    {
        words.extend(words_for(&nextest, options.windows)?);
        return Some(words.join(" "));
    }
    words.extend(words_for(&args, options.windows)?);
    if !executable_args.is_empty() {
        words.push("--".into());
        words.extend(words_for(executable_args, options.windows)?);
    }
    Some(words.join(" "))
}

/// How to debug a runnable: `cargo run` becomes a build of the same target, `cargo test` a build of its test executable.
pub fn debug_plan(runnable: &Runnable, options: &Options) -> Option<DebugPlan> {
    let RunnableCommand::Cargo { cargo_args, extra_args, executable_args, workspace_root, cwd } = &runnable.command else { return None };
    let cargo_args = &with_manifest(cargo_args, workspace_root.as_ref(), options);
    match cargo_args.first()?.as_str() {
        "run" => {
            let mut args = vec!["build".to_string()];
            args.extend(cargo_args[1..].iter().cloned());
            Some(DebugPlan { build_args: with_target(&args, extra_args, options), program_args: executable_args.clone(), test: false, cwd: cwd.clone() })
        }
        // Documentation tests are compiled and run by rustdoc: there is no executable to debug.
        "test" if !cargo_args.iter().chain(extra_args).any(|a| a == "--doc") => {
            let mut args = cargo_args.clone();
            args.push("--no-run".into());
            let mut program_args = executable_args.clone();
            if !program_args.iter().any(|a| a.starts_with("--test-threads")) {
                program_args.push("--test-threads=1".into());
            }
            Some(DebugPlan { build_args: with_target(&args, extra_args, options), program_args, test: true, cwd: cwd.clone() })
        }
        _ => None,
    }
}

/// The items rust-analyzer's runnables give: those with a place in the file, that Cargo can run.
pub fn items(runnables: &[Runnable], options: &Options) -> Vec<Item> {
    runnables
        .iter()
        .filter_map(|r| {
            let line = r.line()?;
            Some(Item { label: r.label.clone(), kind: r.kind(), line, command: command_line(r, options)?, debug: debug_plan(r, options) })
        })
        .collect()
}

/// The items that apply at `position` (line, UTF-16 column): the innermost first, as the cursor is inside them.
pub fn items_at(runnables: &[Runnable], position: (u32, u32), options: &Options) -> Vec<Item> {
    let mut covering: Vec<&Runnable> = runnables.iter().filter(|r| r.covers(position)).collect();
    // The smaller range is the nearer item (a test inside its module).
    covering.sort_by_key(|r| r.location.as_ref().map(|l| (l.range.1.0 - l.range.0.0, l.range.1.1)));
    let owned: Vec<Runnable> = covering.into_iter().cloned().collect();
    items(&owned, options)
}

/// Runnables of a source file when there is no server: the tests the project's test finder knows, and `fn main` where
/// the file is a binary's or an example's entry point. `rel` is relative to the folder, `context` names its manifest.
pub fn fallback_items(rel: &str, text: &str, context: Option<&CrateContext>, options: &Options) -> Vec<Item> {
    if !rel.ends_with(".rs") {
        return Vec::new();
    }
    let mut out: Vec<Item> = crate::testrun::cases_in_crate(rel, text, None, context)
        .into_iter()
        .map(|case| {
            let debug = crate::cargo_args::selection(&case.run.command).ok().map(|selected| {
                let mut build_args = vec!["test".to_string()];
                build_args.extend(selected);
                build_args.push("--no-run".into());
                DebugPlan {
                    build_args: with_target(&build_args, &[], options),
                    program_args: vec![case.run.id.clone(), "--exact".into(), "--nocapture".into(), "--test-threads=1".into()],
                    test: true,
                    cwd: package_dir(context),
                }
            });
            let command = with_target_in_line(&case.run.command, options);
            let command = if options.nextest { super::nextest::convert(&command, options.windows, false).unwrap_or(command) } else { command };
            Item { label: format!("test {}", case.run.id), kind: RunnableKind::Test, line: case.line as u32, command, debug }
        })
        .collect();
    if let Some(line) = main_line(text) {
        let inner = context.map_or(rel, |c| c.relative.as_str());
        if let Some(selection) = binary_selection(inner) {
            let manifest: Vec<String> = context.map(|c| vec!["--manifest-path".into(), c.manifest.clone()]).unwrap_or_default();
            let mut run_args = vec!["run".to_string()];
            run_args.extend(manifest.iter().cloned());
            run_args.extend(selection.iter().cloned());
            let mut build_args = vec!["build".to_string()];
            build_args.extend(manifest);
            build_args.extend(selection);
            let run_args = with_target(&run_args, &[], options);
            let label = format!("run {}", inner.rsplit('/').next().unwrap_or(inner).trim_end_matches(".rs"));
            let Some(quoted) = words_for(&run_args, options.windows) else { return out };
            let command = format!("cargo {}", quoted.join(" "));
            out.push(Item {
                label,
                kind: RunnableKind::Run,
                line,
                command,
                debug: Some(DebugPlan { build_args: with_target(&build_args, &[], options), program_args: Vec::new(), test: false, cwd: package_dir(context) }),
            });
        }
    }
    out.sort_by_key(|i| i.line);
    out
}

/// The folder of the package a file belongs to, relative to the opened folder (`None` for the folder itself).
fn package_dir(context: Option<&CrateContext>) -> Option<String> {
    context.and_then(|c| c.manifest.rsplit_once('/')).map(|(dir, _)| dir.to_string()).filter(|d| !d.is_empty())
}

/// `cwd` as an absolute path under `root`.
fn resolve_cwd(root: &str, cwd: Option<&str>) -> String {
    match cwd {
        Some(c) if c.starts_with('/') || c.as_bytes().get(1) == Some(&b':') || c.starts_with("\\\\") => c.to_string(),
        Some(c) => format!("{}/{c}", root.trim_end_matches('/')),
        None => root.trim_end_matches('/').to_string(),
    }
}

/// A shell command `cargo test … -- …` with `--target` before its `--`.
fn with_target_in_line(command: &str, options: &Options) -> String {
    let Some(target) = options.target.as_ref() else { return command.to_string() };
    if command.contains(" --target ") || command.contains(" --target=") {
        return command.to_string();
    }
    match command.split_once(" -- ") {
        Some((head, tail)) => format!("{head} --target {} -- {tail}", word(target)),
        None => format!("{command} --target {}", word(target)),
    }
}

/// Where `fn main` is declared at the top of a file (0-based line), if it is.
fn main_line(text: &str) -> Option<u32> {
    text.lines()
        .position(|l| {
            let l = l.strip_prefix("pub ").unwrap_or(l);
            let l = l.strip_prefix("async ").unwrap_or(l);
            l.strip_prefix("fn main").is_some_and(|rest| rest.trim_start().starts_with('(') && !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
        })
        .map(|i| i as u32)
}

static FN_LINE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:(?:async|unsafe|const)\s+)*fn\s+(?:r#)?\w+").unwrap());

/// Of the fallback `items`, those of the function the cursor (0-based `line`) is in; none when it is in other code.
pub fn fallback_at(items: Vec<Item>, text: &str, line: u32) -> Vec<Item> {
    let enclosing = text.lines().take(line as usize + 1).enumerate().filter(|(_, l)| FN_LINE.is_match(l)).map(|(i, _)| i as u32).last();
    items.into_iter().filter(|i| Some(i.line) == enclosing).collect()
}

/// Which Cargo target a source file is the entry point of: the package's binary, a named one or an example.
fn binary_selection(inner: &str) -> Option<Vec<String>> {
    let parts: Vec<&str> = inner.split('/').collect();
    match parts.as_slice() {
        ["src", "main.rs"] => Some(Vec::new()),
        ["src", "bin", file] => Some(vec!["--bin".into(), file.strip_suffix(".rs")?.into()]),
        ["src", "bin", dir, "main.rs"] => Some(vec!["--bin".into(), (*dir).into()]),
        ["examples", file] => Some(vec!["--example".into(), file.strip_suffix(".rs")?.into()]),
        ["examples", dir, "main.rs"] => Some(vec!["--example".into(), (*dir).into()]),
        _ => None,
    }
}

/// Lines that carry a gutter mark when no server has told: the entry point and the tests.
pub fn mark_lines(text: &str) -> Vec<u32> {
    let mut lines: Vec<u32> = crate::testrun::cases("a.rs", text, None).into_iter().map(|c| c.line as u32).collect();
    lines.extend(main_line(text));
    lines.sort_unstable();
    lines.dedup();
    lines
}

/// An executable a build reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Built {
    pub name: String,
    pub kind: String,
    pub path: String,
}

/// The executables of `cargo build --message-format=json` (or of `cargo test --no-run`): test executables when `test`,
/// otherwise binaries and examples.
pub fn built_executables(output: &str, test: bool) -> Vec<Built> {
    let mut seen = std::collections::HashSet::new();
    output
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v["reason"] == "compiler-artifact" && (v["profile"]["test"] == true) == test)
        .filter_map(|v| {
            let path = v["executable"].as_str().filter(|p| !p.is_empty())?.to_string();
            let kind = v["target"]["kind"].get(0).and_then(|k| k.as_str()).unwrap_or("bin").to_string();
            Some(Built { name: v["target"]["name"].as_str().unwrap_or_default().to_string(), kind, path })
        })
        .filter(|b| seen.insert(b.path.clone()))
        .collect()
}

/// Debug configurations for the executables a plan built: one each, named after the runnable.
pub fn launch_configs(title: &str, plan: &DebugPlan, root: &str, built: &[Built]) -> Vec<crate::debug_config::LaunchConfig> {
    use crate::debug_config::{LaunchConfig, adapter_for};
    let root = root.trim_end_matches('/');
    let many = built.len() > 1;
    built
        .iter()
        .map(|b| LaunchConfig {
            name: if many { format!("{title} ({}, {})", b.name, b.kind) } else { title.to_string() },
            adapter: adapter_for("lldb"),
            request: "launch".into(),
            args: serde_json::json!({ "program": b.path, "args": plan.program_args, "cwd": resolve_cwd(root, plan.cwd.as_deref()) }),
            pre_launch: None,
            pre_launch_task: None,
            cargo_target: None,
            post_debug: None,
            no_debug: false,
            inputs: Vec::new(),
        })
        .collect()
}

#[cfg(test)]
#[path = "rust_runnables_tests.rs"]
mod tests;
