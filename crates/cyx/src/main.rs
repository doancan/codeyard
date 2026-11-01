//! `cyx`: the one command extension authors need.
//!
//! ```text
//! cyx new <theme|snippets|keymap|language|language-server|command|view|pull-requests> <name> [--lang ts] [--publisher NAME] [--sdk PATH]
//! cyx dev [FOLDER] [-- CODEYARD ARGS…]   run Codeyard with the extension loaded from FOLDER; rebuilt and reloaded on save (dev.rs)
//! cyx check [FOLDER] [--fix]             every problem with its file, line and fix; --fix adds missing permissions (fix.rs)
//! cyx explain CODE                       what an error code means and how to fix it (explain.rs)
//! cyx package [FOLDER] [-o FILE] [--dry-run] [--no-build] [--target TRIPLE]  build the code (build.rs), then a reproducible .cyx
//!                                        (one per platform in native/ when the extension ships native programs)
//! cyx test [FOLDER] [--fixtures FILE]    the built extension in codeyard-exthost against a scripted app (harness.rs)
//! cyx import-vscode <FOLDER | FILE.vsix> [-o FOLDER]
//! cyx publish [FOLDER] [--dry-run] [--pre-release] [--registry URL] [--target TRIPLE]
//! cyx yank | restore VERSION [FOLDER] [--registry URL]   hide a version from installs, or bring it back
//! cyx deprecate VERSION [FOLDER] [--replacement ID] [--registry URL]
//! cyx yard <keygen|sign|verify> …        the registry operator's commands (yard.rs)
//! ```
//!
//! Everything it checks is checked by `ext-manifest` and `ext-pkg`, the same code the app and the registry run, so a
//! folder `cyx check` accepts installs. Only publishing and status changes use the network (`publish.rs`).

mod build;
mod dev;
mod explain;
mod fix;
mod fixtures;
mod harness;
mod keys;
mod net;
mod oidc;
mod publish;
mod scaffold;
mod sigstore;
mod sourcemap;
mod typescript;
mod vscode;
mod yard;

#[cfg(test)]
mod code_tests;
#[cfg(test)]
mod doc_tests;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

const HELP: &str = "cyx: build Codeyard extensions

usage:
  cyx new <kind> <name> [--publisher NAME]   start an extension (kind: theme, snippets, keymap, language,
                                             language-server, command, view, pull-requests; the code ones are Rust,
                                             or TypeScript with --lang ts; --sdk PATH takes the SDK from a Codeyard
                                             checkout)
  cyx dev [FOLDER] [-- CODEYARD ARGS…]       run Codeyard with it loaded; it reloads when you save
  cyx check [FOLDER] [--fix]                 list every problem with its file, line and fix; --fix adds the
                                             missing permissions to codeyard-extension.toml
  cyx explain CODE                           what an error code such as CY0201 means and how to fix it
  cyx package [FOLDER] [-o FILE|FOLDER]      build the code, then write a .cyx to install or publish
                                             (--dry-run: list the files it would hold; builds and writes nothing;
                                             --no-build: package the extension.wasm already there;
                                             --target TRIPLE: one platform's package of an extension with
                                             native programs in native/<TRIPLE>/; without it, every platform)
  cyx test [FOLDER] [--fixtures FILE]        build the code and run each command and view in the extension host;
                                             FILE (default test/fixtures.json) answers its HTTP requests
  cyx import-vscode <FOLDER | FILE.vsix> [-o FOLDER]
                                             convert a VS Code theme, snippets, keymap or language
  cyx publish [FOLDER] [--dry-run] [--pre-release] [--registry URL] [--target TRIPLE]
                                             sign it (no keys: GitHub Actions or a browser sign-in) and publish
                                             (native programs: every platform in native/, or the one named)
  cyx yank | restore VERSION [FOLDER] [--registry URL]
                                             hide a published version from installs, or bring it back
  cyx deprecate VERSION [FOLDER] [--replacement ID] [--registry URL]
  cyx yard <keygen|sign|verify> …           the registry operator's commands (cyx yard help)
  cyx --version

FOLDER defaults to the current folder. Docs: docs/extensions/README.md";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("{e}");
            }
            ExitCode::FAILURE
        }
    }
}

/// Run one command; the error is the full text to print (already formatted), empty when it was printed already.
fn run(args: &[String]) -> Result<(), String> {
    let (cmd, rest) = args.split_first().ok_or_else(|| HELP.to_string())?;
    match cmd.as_str() {
        "-h" | "--help" | "help" => {
            println!("{HELP}");
            Ok(())
        }
        "-V" | "--version" => {
            println!("cyx {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "new" => {
            let (kind, name, publisher, sdk, lang) = parse_new(rest)?;
            let dir = PathBuf::from(&name);
            scaffold::create_in(&dir, kind, &name, &publisher, sdk.as_deref(), lang)?;
            let (word, next) = match (kind.is_code(), lang) {
                (false, _) => (kind.word().to_string(), "cyx dev"),
                (true, scaffold::Lang::Rust) => (format!("Rust {}", kind.word()), "cargo test\n  cyx test\n  cyx dev"),
                (true, scaffold::Lang::TypeScript) => (format!("TypeScript {}", kind.word()), "npm install\n  npm test\n  cyx test\n  cyx dev"),
            };
            println!("Created {}/ ({word} extension, id {publisher}.{name}).\n\nNext:\n  cd {name}\n  {next}", name);
            if lang == scaffold::Lang::TypeScript && sdk.is_none() {
                println!(
                    "\n@codeyard/extension is pre-release and not on npm yet: until it is, `npm install` fails. Pass --sdk with a\nCodeyard checkout to use its sdk/typescript; `cyx test` and `cyx package` work either way."
                );
            }
            Ok(())
        }
        "test" => {
            let mut it = rest.iter();
            let (mut dir, mut fixtures) = (None, None);
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--fixtures" => fixtures = Some(PathBuf::from(it.next().ok_or("usage: cyx test [FOLDER] [--fixtures FILE]")?)),
                    _ => dir = Some(a),
                }
            }
            test(&folder(dir)?, fixtures.as_deref())
        }
        "explain" => match rest {
            [code] => {
                println!("{}", explain::explain(code)?);
                Ok(())
            }
            _ => Err("usage: cyx explain CODE (such as CY0201)".into()),
        },
        "check" => {
            let fix = rest.iter().any(|a| a == "--fix");
            let dir = folder(rest.iter().find(|a| *a != "--fix"))?;
            if fix {
                print_fixes(&fix::fix(&dir)?);
            }
            let report = check(&dir)?;
            for w in &report.warnings {
                eprintln!("{w}");
            }
            println!("{} {} is ready to package.", report.id, report.version);
            if let Some(r) = &report.requires {
                println!("{r}");
            }
            Ok(())
        }
        "package" => run_package(&parse_package(rest)?),
        "dev" => {
            let (folder_arg, extra) = match rest.iter().position(|a| a == "--") {
                Some(i) => (rest[..i].first(), &rest[i + 1..]),
                None => (rest.first(), &rest[rest.len()..]),
            };
            let dir = folder(folder_arg)?;
            dev::run(&dir, extra)
        }
        "import-vscode" => {
            let (src, out) = parse_import(rest)?;
            let report = vscode::import(&src, out.as_deref())?;
            println!("Converted into {}/", report.out.display());
            for c in &report.converted {
                println!("  ✓ {c}");
            }
            for s in &report.skipped {
                println!("  – {s}");
            }
            println!("\nNext:\n  cd {}\n  cyx dev", report.out.display());
            Ok(())
        }
        "publish" => {
            let (dir, registry, dry_run, pre_release, target) = parse_publish(rest)?;
            let ep = sigstore::Endpoints::from_env();
            let oidc = ep.oidc.clone();
            // An extension with native programs is published once per platform; each package is signed on its own.
            for t in publish_targets(&dir, target)? {
                let p = publish::Publish { dir: &dir, registry: &registry, dry_run, pre_release, target: t.as_deref() };
                println!("{}", publish::publish(&p, &ep, || oidc::token(&oidc), now())?);
            }
            Ok(())
        }
        "yank" | "restore" | "deprecate" => {
            let (version, dir, registry, replacement) = parse_status(cmd, rest)?;
            let change = match cmd.as_str() {
                "yank" => ext_core::yard::StatusChange::Yank,
                "restore" => ext_core::yard::StatusChange::Restore,
                _ => ext_core::yard::StatusChange::Deprecate { replacement },
            };
            let ep = sigstore::Endpoints::from_env();
            let oidc = ep.oidc.clone();
            println!("{}", publish::change_status(&dir, &registry, &version, change, &ep, || oidc::token(&oidc), now())?);
            Ok(())
        }
        "yard" => match rest.first().map(String::as_str) {
            None | Some("help" | "-h" | "--help") => {
                println!("{}", yard::HELP);
                Ok(())
            }
            _ => yard::run(rest),
        },
        other => Err(format!("cyx: unknown command '{other}'\n\n{HELP}")),
    }
}

fn print_fixes(f: &fix::Fixed) {
    if !f.applied.is_empty() {
        println!("Fixed {} [permissions]:", ext_manifest::FILE_NAME);
        for l in &f.applied {
            println!("  {l}");
        }
    }
    if !f.left.is_empty() {
        println!("Not fixed, these need you:");
        for (l, why) in &f.left {
            println!("  {l}   ({why})");
        }
    }
    if f.applied.is_empty() && f.left.is_empty() {
        println!("No permissions to add.");
    }
    if let Some(note) = &f.note {
        println!("note: {note}");
    }
    println!();
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

type PublishArgs = (PathBuf, String, bool, bool, Option<String>);

fn parse_publish(rest: &[String]) -> Result<PublishArgs, String> {
    let usage = "usage: cyx publish [FOLDER] [--dry-run] [--pre-release] [--registry URL] [--target TRIPLE]";
    let (mut dir, mut registry, mut dry_run, mut pre_release, mut target) = (None, None, false, false, None);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--dry-run" => dry_run = true,
            "--pre-release" => pre_release = true,
            "--registry" => registry = Some(it.next().ok_or(usage)?.as_str()),
            "--target" => target = Some(it.next().ok_or(usage)?.clone()),
            s if s.starts_with('-') => {
                return Err(format!(
                    "cyx publish: unknown option {s}
{usage}"
                ));
            }
            _ => dir = Some(a.clone()),
        }
    }
    Ok((folder(dir.as_ref())?, publish::registry_url(registry), dry_run, pre_release, target))
}

/// The platforms to publish: none (one package) without native programs, else `target` or every one in `native/`.
fn publish_targets(dir: &Path, target: Option<String>) -> Result<Vec<Option<String>>, String> {
    if !build::manifest(dir).is_ok_and(|m| m.has_native_programs()) {
        return match target {
            Some(_) => Err("error: --target is for extensions with native programs (a `run` entry inside the package)".into()),
            None => Ok(vec![None]),
        };
    }
    Ok(match target {
        Some(t) => vec![Some(t)],
        None => native_platforms(dir)?.into_iter().map(Some).collect(),
    })
}

fn parse_status(cmd: &str, rest: &[String]) -> Result<(ext_manifest::Version, PathBuf, String, Option<ext_manifest::ExtId>), String> {
    let usage = format!("usage: cyx {cmd} VERSION [FOLDER] [--registry URL]{}", if cmd == "deprecate" { " [--replacement ID]" } else { "" });
    let (mut version, mut dir, mut registry, mut replacement) = (None, None, None, None);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--registry" => registry = Some(it.next().ok_or(&usage)?.as_str()),
            "--replacement" if cmd == "deprecate" => {
                let r = it.next().ok_or(&usage)?;
                replacement = Some(ext_manifest::ExtId::parse(r).map_err(|e| format!("cyx {cmd}: replacement {e}"))?);
            }
            s if s.starts_with('-') => {
                return Err(format!(
                    "cyx {cmd}: unknown option {s}
{usage}"
                ));
            }
            _ if version.is_none() => version = Some(ext_manifest::Version::parse(a).map_err(|e| format!("cyx {cmd}: version {e}"))?),
            _ => dir = Some(a.clone()),
        }
    }
    Ok((version.ok_or(usage)?, folder(dir.as_ref())?, publish::registry_url(registry), replacement))
}

fn folder(arg: Option<&String>) -> Result<PathBuf, String> {
    let dir = arg.map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    if !dir.join(ext_manifest::FILE_NAME).is_file() {
        return Err(format!(
            "error: {} has no {}\n   = fix: run this in your extension's folder, or start one with `cyx new theme my-theme`",
            dir.display(),
            ext_manifest::FILE_NAME
        ));
    }
    Ok(dir)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Theme,
    Snippets,
    Keymap,
    Language,
    LanguageServer,
    /// Code extensions, in Rust or TypeScript.
    Command,
    View,
    /// A `[[pull-requests]]` provider with its `[[oauth]]` account.
    PullRequests,
}

impl Kind {
    fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "theme" => Kind::Theme,
            "snippets" => Kind::Snippets,
            "keymap" => Kind::Keymap,
            "language" => Kind::Language,
            "language-server" | "lsp" => Kind::LanguageServer,
            "command" => Kind::Command,
            "view" => Kind::View,
            "pull-requests" => Kind::PullRequests,
            _ => return None,
        })
    }

    fn word(self) -> &'static str {
        match self {
            Kind::Theme => "theme",
            Kind::Snippets => "snippets",
            Kind::Keymap => "keymap",
            Kind::Language => "language",
            Kind::LanguageServer => "language server",
            Kind::Command => "command",
            Kind::View => "view",
            Kind::PullRequests => "pull request provider",
        }
    }

    pub fn is_code(self) -> bool {
        matches!(self, Kind::Command | Kind::View | Kind::PullRequests)
    }
}

type NewArgs = (Kind, String, String, Option<String>, scaffold::Lang);

fn parse_new(rest: &[String]) -> Result<NewArgs, String> {
    let usage =
        "usage: cyx new <theme|snippets|keymap|language|language-server|command|view|pull-requests> <name> [--lang rust|ts] [--publisher NAME] [--sdk PATH]";
    let mut positional = Vec::new();
    let (mut publisher, mut sdk, mut lang) = (None, None, scaffold::Lang::Rust);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--publisher" => publisher = Some(it.next().ok_or(usage)?.clone()),
            "--sdk" => sdk = Some(it.next().ok_or(usage)?.clone()),
            "--lang" => {
                let l = it.next().ok_or(usage)?;
                lang = scaffold::Lang::parse(l).ok_or_else(|| format!("cyx: unknown language '{l}' (rust or ts)\n{usage}"))?;
            }
            _ => positional.push(a.clone()),
        }
    }
    let [kind, name] = positional.as_slice() else { return Err(usage.into()) };
    let kind = Kind::parse(kind).ok_or_else(|| format!("cyx: unknown kind '{kind}'\n{usage}"))?;
    if lang != scaffold::Lang::Rust && !kind.is_code() {
        return Err(format!("error: --lang is for code extensions (command, view, pull-requests); a {} has no code", kind.word()));
    }
    ext_manifest::check_id_part(name).map_err(|e| format!("error: the name '{name}' {e}\n   = fix: use lowercase letters, digits and -, like my-theme"))?;
    let publisher = publisher.unwrap_or_else(default_publisher);
    ext_manifest::check_id_part(&publisher)
        .map_err(|e| format!("error: the publisher '{publisher}' {e}\n   = fix: pass --publisher with lowercase letters, digits and -"))?;
    if Path::new(name).exists() {
        return Err(format!("error: {name}/ already exists\n   = fix: pick another name or remove the folder"));
    }
    Ok((kind, name.clone(), publisher, sdk, lang))
}

/// The publisher part of new ids: `git config user.name` made into an id, or `me`.
fn default_publisher() -> String {
    let name = std::process::Command::new("git")
        .args(["config", "user.name"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    slug(&name).filter(|s| ext_manifest::check_id_part(s).is_ok()).unwrap_or_else(|| "me".into())
}

/// Lowercase ASCII letters and digits joined by single `-`; Turkish and other Latin letters lose their marks.
pub fn slug(s: &str) -> Option<String> {
    let mut out = String::new();
    for c in s.chars() {
        let c = match c {
            'ç' | 'Ç' => 'c',
            'ğ' | 'Ğ' => 'g',
            'ı' | 'İ' | 'í' | 'î' | 'ï' | 'ì' => 'i',
            'ö' | 'Ö' | 'ó' | 'ô' | 'ò' => 'o',
            'ş' | 'Ş' => 's',
            'ü' | 'Ü' | 'ú' | 'û' | 'ù' => 'u',
            'á' | 'à' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            c => c,
        };
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    // An id part starts with a letter.
    let out = out.trim_end_matches('-').trim_start_matches(|c: char| c.is_ascii_digit() || c == '-').to_string();
    (!out.is_empty()).then(|| out.chars().take(ext_manifest::limits::ID_PART_LEN).collect::<String>().trim_end_matches('-').to_string())
}

#[derive(Debug)]
pub struct CheckReport {
    pub id: String,
    pub version: String,
    pub warnings: Vec<String>,
    /// For a built code extension: the oldest Codeyard it runs on, worked out from the API items its component uses.
    pub requires: Option<String>,
}

/// Everything install and publish check, printed the way the guide shows: code, file:line, fix.
pub fn check(dir: &Path) -> Result<CheckReport, String> {
    check_with(dir, ext_manifest::Options::default())
}

/// `cyx dev` lets an author try high-risk permissions before writing `why`: a warning, not an error.
pub(crate) fn check_dev(dir: &Path) -> Result<CheckReport, String> {
    check_with(dir, ext_manifest::Options { require_why: false })
}

fn check_with(dir: &Path, options: ext_manifest::Options) -> Result<CheckReport, String> {
    let manifest_path = dir.join(ext_manifest::FILE_NAME);
    let text = std::fs::read_to_string(&manifest_path).map_err(|e| format!("error: cannot read {}: {e}", manifest_path.display()))?;
    let m = ext_manifest::parse_with(&text, options).map_err(|errors| render(&errors, &text))?;
    let mut warnings = Vec::new();
    if m.missing_why() {
        warnings.push(format!(
            "warning: these permissions are high risk, so installing and publishing need a `why`\n  --> {}\n   = fix: add to [permissions]:  why = \"One honest sentence about what you do with them.\"",
            ext_manifest::FILE_NAME
        ));
    }
    // A core command this version does not list is refused when the extension runs it (a newer Codeyard may list it).
    for c in m.permissions.commands.iter().filter(|c| ext_manifest::core_command(c).is_none()) {
        warnings.push(format!(
            "warning: {c} is not a core command extensions may run, so commands.execute refuses it\n  --> {} [permissions] commands\n   = fix: pick one from docs/extensions/reference/core-commands.md",
            ext_manifest::FILE_NAME
        ));
    }
    // The app does not bind a key a built-in shortcut uses.
    for k in m.contributions.keybindings.iter().filter(|k| keys::is_builtin_key(&k.key)) {
        warnings.push(format!(
            "warning: key '{}' is a built-in shortcut, so Codeyard will not bind it to {}\n  --> {} [[keybindings]] key\n   = fix: pick a key no built-in uses, such as one with cmd-alt-shift or ctrl-alt-shift",
            k.key,
            k.command,
            ext_manifest::FILE_NAME
        ));
    }
    warnings.extend(unknown_languages(&m));
    // A client id left as the template wrote it passes every check and fails at the first sign-in (#187).
    for o in placeholder_client_ids(&m) {
        warnings.push(format!(
            "warning: the client id of [[oauth]] {} looks like a placeholder: {:?}\n  --> {} [[oauth]] client-id\n   = fix: register an OAuth application with the provider and put its client id here; `cyx publish` refuses a placeholder",
            o.id,
            o.client_id,
            ext_manifest::FILE_NAME
        ));
    }
    // The package rules (files the manifest names, paths, sizes) as a full package build would apply them: for each
    // platform of an extension with native programs.
    // An error does not hide the warnings found so far: they come after it, so the author fixes both in one pass.
    let mut secrets = Vec::new();
    let with_warnings = |error: String| if warnings.is_empty() { error } else { format!("{error}\n\n{}", warnings.join("\n")) };
    if m.has_native_programs() {
        for t in native_platforms(dir).map_err(with_warnings)? {
            secrets.extend(check_files(dir, Some(&t), &m, &text).map_err(with_warnings)?);
        }
    } else {
        secrets.extend(check_files(dir, None, &m, &text).map_err(with_warnings)?);
    }
    secrets.sort();
    secrets.dedup();
    if !secrets.is_empty() {
        warnings.push(format!(
            "warning: left out of the package because they look like secrets: {}\n   = fix: keep keys and tokens outside the extension folder; a file that only looks like a secret can be renamed. docs/extensions/developer-guide.md (\"What goes into a package\") lists the rules",
            secrets.join(", ")
        ));
    }
    let requires = match (&m.code, std::fs::read(dir.join(ext_pkg::COMPONENT))) {
        (Some(_), Ok(wasm)) => ext_pkg::check_component(&wasm, &m).ok().map(|c| {
            if c.in_transition() {
                warnings.push(transition_warning(&c));
            }
            requires(&c)
        }),
        _ => None,
    };
    Ok(CheckReport { id: m.id.to_string(), version: m.version.to_string(), warnings, requires })
}

/// "It needs Codeyard 0.7.0 or newer (API 1.0.0, …)." for a component that passed the import check. When this build
/// is a preview of that release (the workspace version is still the one before it), it says this build runs it.
pub(crate) fn requires(c: &ext_pkg::Component) -> String {
    requires_on(c, env!("CARGO_PKG_VERSION"))
}

fn requires_on(c: &ext_pkg::Component, build: &str) -> String {
    let api = c.min_api;
    match ext_api::codeyard_for(api) {
        None => format!("It needs a Codeyard newer than any release (API {api}, from the API items its code uses)."),
        Some(release) if ext_api::is_preview_of(build, release) => format!(
            "It needs Codeyard {release} or newer (API {api}, from the API items its code uses). {release} is not released yet; this preview build ({build}) runs it."
        ),
        Some(release) => format!("It needs Codeyard {release} or newer (API {api}, from the API items its code uses)."),
    }
}

/// A component built against a 0.x API, which Codeyard still runs during the move to 1.0.
pub(crate) fn transition_warning(c: &ext_pkg::Component) -> String {
    format!(
        "warning: {} was built against API {}; Codeyard still runs it while extensions move to API {}.x, but registries no longer take it and a later Codeyard will refuse it\n  --> {}\n   = fix: rebuild with the current SDK (`cyx package`); docs/extensions/migrating-to-1.0.md lists what changed",
        ext_pkg::COMPONENT,
        c.built_for,
        ext_api::VERSION.0,
        ext_pkg::COMPONENT
    )
}

/// The platforms an extension with native programs has in `native/`; none is an error that says what to do.
fn native_platforms(dir: &Path) -> Result<Vec<String>, String> {
    let targets = ext_pkg::native_targets(dir);
    if targets.is_empty() {
        return Err(format!(
            "error: permissions.run names a program inside the package, but {}/ has no platform folder\n   = fix: build the program for each platform and put it in {}/<triple>/ (one of {})",
            ext_pkg::NATIVE_DIR,
            ext_pkg::NATIVE_DIR,
            ext_manifest::TARGETS.join(", ")
        ));
    }
    Ok(targets)
}

/// Build the package in memory to apply every package rule; the paths left out as secrets.
fn check_files(dir: &Path, target: Option<&str>, m: &ext_manifest::Manifest, text: &str) -> Result<Vec<String>, String> {
    let ext_pkg::Collected { files, secrets } = ext_pkg::collect(dir, target).map_err(|e| format!("error: {e}"))?;
    let check_files: Vec<ext_pkg::Entry> = if m.missing_why() {
        // Validate the rest as if `why` were there, so the author sees every other problem now.
        files
            .into_iter()
            .map(|mut e| {
                if e.path == ext_manifest::FILE_NAME {
                    e.data = with_placeholder_why(&String::from_utf8_lossy(&e.data)).into_bytes();
                }
                e
            })
            .collect()
    } else {
        files
    };
    let platform = target.map(|t| format!(" (for {t})")).unwrap_or_default();
    match ext_pkg::write(check_files) {
        Ok(_) => Ok(secrets),
        Err(ext_pkg::Error::Manifest(errors)) => Err(render(&errors, text)),
        Err(ext_pkg::Error::Component(errors)) => Err(errors.iter().map(|d| d.render(ext_pkg::COMPONENT, "")).collect::<Vec<_>>().join("\n")),
        Err(e) => Err(format!("error{platform}: {e}")),
    }
}

/// A warning per language id that is neither built in nor added by this extension: a typo, or a language a dependency
/// adds.
fn unknown_languages(m: &ext_manifest::Manifest) -> Vec<String> {
    let c = &m.contributions;
    let mut used: Vec<String> = c.snippets.iter().map(|s| s.language.clone()).collect();
    used.extend(c.language_servers.iter().flat_map(|s| s.languages.iter().cloned()));
    used.extend(c.provider_languages());
    let mut seen = Vec::new();
    used.into_iter()
        .filter(|id| !ext_manifest::LANGUAGE_IDS.contains(&id.as_str()) && !c.languages.iter().any(|l| &l.id == id))
        .filter(|id| {
            let first = !seen.contains(id);
            seen.push(id.clone());
            first
        })
        .map(|id| {
            let fix = match keys::closest_language(&id) {
                Some(close) => format!("did you mean '{close}'?"),
                None => "use a language id from reference/contributions.md (Language ids), or add the language with [[languages]]".into(),
            };
            format!(
                "warning: '{id}' is not a language Codeyard knows, and this extension does not add it: nothing uses it unless a dependency adds that language\n  --> {}\n   = fix: {fix}",
                ext_manifest::FILE_NAME
            )
        })
        .collect()
}

/// `[[oauth]]` tables whose `client-id` is still a placeholder: `your-client-id`, `<client id>`, `example-…`.
pub(crate) fn placeholder_client_ids(m: &ext_manifest::Manifest) -> Vec<&ext_manifest::OAuthProvider> {
    m.oauth
        .iter()
        .filter(|o| {
            let id = o.client_id.to_ascii_lowercase();
            id.starts_with("your")
                || id.contains('<')
                || ["example", "placeholder", "changeme", "change-me", "replace", "todo", "xxx"].iter().any(|w| id.contains(w))
        })
        .collect()
}

pub(crate) fn with_placeholder_why(manifest: &str) -> String {
    match manifest.find("[permissions]") {
        Some(i) => {
            let at = i + "[permissions]".len();
            format!("{}\nwhy = \"(dev)\"{}", &manifest[..at], &manifest[at..])
        }
        None => manifest.to_string(),
    }
}

fn render(errors: &[ext_manifest::Diagnostic], text: &str) -> String {
    let mut out: Vec<String> = errors.iter().map(|d| d.render(ext_manifest::FILE_NAME, text)).collect();
    out.push(format!("\n{} problem{} found", errors.len(), if errors.len() == 1 { "" } else { "s" }));
    out.join("\n\n")
}

/// `cyx package`. `--dry-run` writes nothing, so it does not build: the build would rewrite `extension.wasm`.
fn run_package(p: &PackageArgs) -> Result<(), String> {
    // A wrong -o fails now, not after a build that can take a while.
    if let Some(o) = &p.out {
        check_output(o)?;
    }
    if p.build && !p.dry_run {
        build::build(&p.dir)?;
    }
    for w in check(&p.dir)?.warnings {
        eprintln!("{w}");
    }
    if p.dry_run {
        for (name, files) in package_contents(&p.dir, p.target.as_deref())? {
            println!("{name} would hold {} files:", files.len());
            for (path, size) in files {
                println!("  {path}  ({size} bytes)");
            }
        }
        if build::manifest(&p.dir)?.code.is_some() {
            println!("{} is the one in the folder now: --dry-run does not build.", ext_pkg::COMPONENT);
        }
        println!("Nothing was built or written. docs/extensions/developer-guide.md (\"What goes into a package\") says what is left out.");
        return Ok(());
    }
    for (path, digest) in package_checked(&p.dir, p.out.as_deref(), p.target.as_deref())? {
        println!("Wrote {}\ncontent digest {digest}", path.display());
    }
    Ok(())
}

/// `cyx package`'s arguments.
struct PackageArgs {
    dir: PathBuf,
    out: Option<PathBuf>,
    /// Build first (not with `--no-build`).
    build: bool,
    /// List the files, write nothing (`--dry-run`).
    dry_run: bool,
    target: Option<String>,
}

fn parse_package(rest: &[String]) -> Result<PackageArgs, String> {
    let usage = "usage: cyx package [FOLDER] [-o FILE] [--dry-run] [--no-build] [--target TRIPLE]";
    let (mut dir, mut out, mut build, mut dry_run, mut target) = (None, None, true, false, None);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-o" | "--output" => out = Some(PathBuf::from(it.next().ok_or(usage)?)),
            "--no-build" => build = false,
            "--dry-run" => dry_run = true,
            "--target" => target = Some(it.next().ok_or(usage)?.clone()),
            s if s.starts_with('-') => return Err(format!("cyx package: unknown option {s}\n{usage}")),
            _ => dir = Some(a.clone()),
        }
    }
    Ok(PackageArgs { dir: folder(dir.as_ref())?, out, build, dry_run, target })
}

/// `-o` names an existing folder, or a file in one.
fn check_output(out: &Path) -> Result<(), String> {
    if out.is_dir() {
        return Ok(());
    }
    let text = out.to_string_lossy();
    let folder = if text.ends_with('/') || text.ends_with(std::path::MAIN_SEPARATOR) { Some(out) } else { out.parent().filter(|p| !p.as_os_str().is_empty()) };
    match folder {
        Some(f) if !f.is_dir() => {
            Err(format!("error: -o {}: the folder {} does not exist\n   = fix: create it first, or pick another -o", out.display(), f.display()))
        }
        _ => Ok(()),
    }
}

/// A file in a package and its size in bytes.
type FileSize = (String, usize);

/// What `cyx package --dry-run` lists for a checked folder: per package (`<id>-<version>[-<triple>].cyx`), its files
/// and sizes.
fn package_contents(dir: &Path, target: Option<&str>) -> Result<Vec<(String, Vec<FileSize>)>, String> {
    let m = build::manifest(dir)?;
    package_targets(dir, target)?
        .iter()
        .map(|t| {
            let files = ext_pkg::collect_dir_for(dir, t.as_deref()).map_err(|e| format!("error: {e}"))?;
            let (bytes, _) = ext_pkg::write(files).map_err(|e| format!("error: {e}"))?;
            let package = ext_pkg::read(&bytes).map_err(|e| format!("error: {e}"))?;
            let list = package.files.values().map(|e| (e.path.clone(), e.data.len())).collect();
            Ok((ext_pkg::file_name(&m, t.as_deref()), list))
        })
        .collect()
}

/// `cyx package` after the build: one package, or for an extension with native programs one per platform
/// (`<id>-<version>-<triple>.cyx`), only `target`'s when it is given.
pub fn package_platforms(dir: &Path, out: Option<&Path>, target: Option<&str>) -> Result<Vec<(PathBuf, ext_pkg::ContentDigest)>, String> {
    check(dir)?;
    package_checked(dir, out, target)
}

/// [`package_platforms`] for a folder `check` accepted.
fn package_checked(dir: &Path, out: Option<&Path>, target: Option<&str>) -> Result<Vec<(PathBuf, ext_pkg::ContentDigest)>, String> {
    let targets = package_targets(dir, target)?;
    if out.is_some_and(|o| !o.is_dir()) && targets.len() > 1 {
        return Err("error: -o names one file, but this extension has a package per platform\n   = fix: add --target TRIPLE, or give -o a folder".into());
    }
    targets.iter().map(|t| package_one(dir, out, t.as_deref())).collect()
}

/// The platforms to package: none (one package) without native programs, else `target` or every one in `native/`.
fn package_targets(dir: &Path, target: Option<&str>) -> Result<Vec<Option<String>>, String> {
    Ok(match (build::manifest(dir)?.has_native_programs(), target) {
        (false, None) => vec![None],
        (false, Some(_)) => return Err("error: --target is for extensions with native programs (a `run` entry inside the package)".into()),
        (true, Some(t)) => vec![Some(t.to_string())],
        (true, None) => native_platforms(dir)?.into_iter().map(Some).collect(),
    })
}

fn package_one(dir: &Path, out: Option<&Path>, target: Option<&str>) -> Result<(PathBuf, ext_pkg::ContentDigest), String> {
    let files = ext_pkg::collect_dir_for(dir, target).map_err(|e| format!("error: {e}"))?;
    let (bytes, digest) = ext_pkg::write(files).map_err(|e| format!("error: {e}"))?;
    let m = ext_pkg::read(&bytes).map_err(|e| format!("error: {e}"))?.manifest;
    // `-o` names the file, or the folder the usual name goes in.
    let path = match out {
        Some(o) if o.is_dir() => o.join(ext_pkg::file_name(&m, target)),
        Some(o) => o.to_path_buf(),
        None => PathBuf::from(ext_pkg::file_name(&m, target)),
    };
    std::fs::write(&path, bytes).map_err(|e| format!("error: cannot write {}: {e}", path.display()))?;
    Ok((path, digest))
}

/// Build the code if there is any, check, then write `<id>-<version>.cyx` (or `out`).
pub fn package(dir: &Path, out: Option<&Path>) -> Result<(PathBuf, ext_pkg::ContentDigest), String> {
    build::build(dir)?;
    package_built(dir, out)
}

/// `cyx package --no-build`: the `extension.wasm` already in the folder (built by another tool, or a fixture), checked
/// and packaged like a built one.
pub fn package_built(dir: &Path, out: Option<&Path>) -> Result<(PathBuf, ext_pkg::ContentDigest), String> {
    check(dir)?;
    package_one(dir, out, None)
}

/// `cyx test`: build, check, then run the component in the extension host. Fails on any refused call.
fn test(dir: &Path, fixtures: Option<&Path>) -> Result<(), String> {
    let fixtures = fixtures::Fixtures::load(dir, fixtures)?;
    if build::build(dir)?.is_none() {
        check_dev(dir)?;
        return Err("error: this extension has no code to test\n   = fix: `cyx check` covers themes, snippets and other no-code extensions".into());
    }
    check_dev(dir)?;
    let exthost = harness::exthost().ok_or(
        "error: codeyard-exthost was not found\n   = fix: install Codeyard (cyx finds the extension host beside itself), or set CODEYARD_EXTHOST to its path",
    )?;
    let manifest = build::manifest(dir)?;
    let report = harness::run(dir, &manifest, &exthost, &fixtures)?;
    println!("{} {}", manifest.id, manifest.version);
    for line in &report.lines {
        println!("{line}");
    }
    if report.failures.is_empty() {
        println!("ok");
        Ok(())
    } else {
        Err(format!("\n{} problem{} found", report.failures.len(), if report.failures.len() == 1 { "" } else { "s" }))
    }
}

fn parse_import(rest: &[String]) -> Result<(PathBuf, Option<PathBuf>), String> {
    let usage = "usage: cyx import-vscode <FOLDER | FILE.vsix> [-o FOLDER]";
    let mut src = None;
    let mut out = None;
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-o" | "--output" => out = Some(PathBuf::from(it.next().ok_or(usage)?)),
            _ => src = Some(PathBuf::from(a)),
        }
    }
    Ok((src.ok_or(usage)?, out))
}

/// The Codeyard to start: `CODEYARD_BIN`, then `codeyard` on PATH, then the usual install locations.
pub(crate) fn codeyard_binary() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CODEYARD_BIN") {
        return Some(PathBuf::from(p));
    }
    let exe = if cfg!(windows) { "codeyard.exe" } else { "codeyard" };
    let on_path = std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|d| d.join(exe)).find(|p| p.is_file()));
    // Shipped next to the app (the bundle's MacOS folder, the Windows install folder).
    let beside = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(exe))).filter(|p| p.is_file());
    let known = Some(PathBuf::from("/Applications/Codeyard.app/Contents/MacOS/codeyard")).filter(|p| cfg!(target_os = "macos") && p.is_file());
    on_path.or(beside).or(known)
}
