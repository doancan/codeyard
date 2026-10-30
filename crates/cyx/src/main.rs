//! `cyx`: the one command extension authors need.
//!
//! ```text
//! cyx new <theme|snippets|keymap|language|language-server|command|view> <name> [--lang ts] [--publisher NAME] [--sdk PATH]
//! cyx dev [FOLDER] [-- CODEYARD ARGS…]   run Codeyard with the extension loaded from FOLDER; rebuilt and reloaded on save (dev.rs)
//! cyx check [FOLDER] [--fix]             every problem with its file, line and fix; --fix adds missing permissions (fix.rs)
//! cyx package [FOLDER] [-o FILE]         build the code (build.rs), then a reproducible .cyx
//! cyx test [FOLDER]                      the built extension in codeyard-exthost against a scripted app (harness.rs)
//! cyx import-vscode <FOLDER | FILE.vsix> [-o FOLDER]
//! cyx publish [FOLDER] [--dry-run] [--pre-release] [--registry URL]
//! cyx yank | restore VERSION [FOLDER]    hide a version from installs, or bring it back
//! cyx deprecate VERSION [FOLDER] [--replacement ID]
//! cyx yard <keygen|sign|verify> …        the registry operator's commands (yard.rs)
//! ```
//!
//! Everything it checks is checked by `ext-manifest` and `ext-pkg`, the same code the app and the registry run, so a
//! folder `cyx check` accepts installs. Only publishing and status changes use the network (`publish.rs`).

mod build;
mod dev;
mod fix;
mod harness;
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
mod tests;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

const HELP: &str = "cyx: build Codeyard extensions

usage:
  cyx new <kind> <name> [--publisher NAME]   start an extension (kind: theme, snippets, keymap, language,
                                             language-server, command, view; command and view are Rust, or
                                             TypeScript with --lang ts; --sdk PATH takes the SDK from a Codeyard
                                             checkout)
  cyx dev [FOLDER] [-- CODEYARD ARGS…]       run Codeyard with it loaded; it reloads when you save
  cyx check [FOLDER] [--fix]                 list every problem with its file, line and fix; --fix adds the
                                             missing permissions to codeyard-extension.toml
  cyx package [FOLDER] [-o FILE]             build the code, then write a .cyx to install or publish
  cyx test [FOLDER]                          build the code and run each command and view in the extension host
  cyx import-vscode <FOLDER | FILE.vsix> [-o FOLDER]
                                             convert a VS Code theme, snippets, keymap or language
  cyx publish [FOLDER] [--dry-run] [--pre-release] [--registry URL]
                                             sign it (no keys: GitHub Actions or a browser sign-in) and publish
  cyx yank | restore VERSION [FOLDER]        hide a published version from installs, or bring it back
  cyx deprecate VERSION [FOLDER] [--replacement ID]
  cyx yard <keygen|sign|verify> …           the registry operator's commands (cyx yard help)
  cyx --version

FOLDER defaults to the current folder. Docs: docs/extensions/developer-guide.md";

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
            let dir = folder(rest.first())?;
            test(&dir)
        }
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
            Ok(())
        }
        "package" => {
            let (dir, out) = parse_package(rest)?;
            let (path, digest) = package(&dir, out.as_deref())?;
            println!("Wrote {}\ncontent digest {digest}", path.display());
            Ok(())
        }
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
            let (dir, registry, dry_run, pre_release) = parse_publish(rest)?;
            let p = publish::Publish { dir: &dir, registry: &registry, dry_run, pre_release };
            let ep = sigstore::Endpoints::from_env();
            let oidc = ep.oidc.clone();
            println!("{}", publish::publish(&p, &ep, || oidc::token(&oidc), now())?);
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
        println!("Nothing to fix.");
    }
    println!();
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn parse_publish(rest: &[String]) -> Result<(PathBuf, String, bool, bool), String> {
    let usage = "usage: cyx publish [FOLDER] [--dry-run] [--pre-release] [--registry URL]";
    let (mut dir, mut registry, mut dry_run, mut pre_release) = (None, None, false, false);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--dry-run" => dry_run = true,
            "--pre-release" => pre_release = true,
            "--registry" => registry = Some(it.next().ok_or(usage)?.as_str()),
            s if s.starts_with('-') => {
                return Err(format!(
                    "cyx publish: unknown option {s}
{usage}"
                ));
            }
            _ => dir = Some(a.clone()),
        }
    }
    Ok((folder(dir.as_ref())?, publish::registry_url(registry), dry_run, pre_release))
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
        }
    }

    pub fn is_code(self) -> bool {
        matches!(self, Kind::Command | Kind::View)
    }
}

type NewArgs = (Kind, String, String, Option<String>, scaffold::Lang);

fn parse_new(rest: &[String]) -> Result<NewArgs, String> {
    let usage = "usage: cyx new <theme|snippets|keymap|language|language-server|command|view> <name> [--lang rust|ts] [--publisher NAME] [--sdk PATH]";
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
        return Err(format!("error: --lang is for code extensions (command, view); a {} has no code", kind.word()));
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
    // The package rules (files the manifest names, paths, sizes) as a full package build would apply them.
    let files = ext_pkg::collect_dir(dir).map_err(|e| format!("error: {e}"))?;
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
    match ext_pkg::write(check_files) {
        Ok(_) => Ok(CheckReport { id: m.id.to_string(), version: m.version.to_string(), warnings }),
        Err(ext_pkg::Error::Manifest(errors)) => Err(render(&errors, &text)),
        Err(ext_pkg::Error::Component(errors)) => Err(errors.iter().map(|d| d.render(ext_pkg::COMPONENT, "")).collect::<Vec<_>>().join("\n")),
        Err(e) => Err(format!("error: {e}")),
    }
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

fn parse_package(rest: &[String]) -> Result<(PathBuf, Option<PathBuf>), String> {
    let mut dir = None;
    let mut out = None;
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-o" | "--output" => out = Some(PathBuf::from(it.next().ok_or("usage: cyx package [FOLDER] [-o FILE]")?)),
            _ => dir = Some(a.clone()),
        }
    }
    Ok((folder(dir.as_ref())?, out))
}

/// Build the code if there is any, check, then write `<id>-<version>.cyx` (or `out`).
pub fn package(dir: &Path, out: Option<&Path>) -> Result<(PathBuf, ext_pkg::ContentDigest), String> {
    build::build(dir)?;
    let report = check(dir)?;
    let files = ext_pkg::collect_dir(dir).map_err(|e| format!("error: {e}"))?;
    let (bytes, digest) = ext_pkg::write(files).map_err(|e| format!("error: {e}"))?;
    let path = out.map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(format!("{}-{}.cyx", report.id, report.version)));
    std::fs::write(&path, bytes).map_err(|e| format!("error: cannot write {}: {e}", path.display()))?;
    Ok((path, digest))
}

/// `cyx test`: build, check, then run the component in the extension host. Fails on any refused call.
fn test(dir: &Path) -> Result<(), String> {
    if build::build(dir)?.is_none() {
        check_dev(dir)?;
        return Err("error: this extension has no code to test\n   = fix: `cyx check` covers themes, snippets and other no-code extensions".into());
    }
    check_dev(dir)?;
    let exthost = harness::exthost().ok_or(
        "error: codeyard-exthost was not found\n   = fix: install Codeyard (cyx finds the extension host beside itself), or set CODEYARD_EXTHOST to its path",
    )?;
    let manifest = build::manifest(dir)?;
    let report = harness::run(dir, &manifest, &exthost)?;
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
