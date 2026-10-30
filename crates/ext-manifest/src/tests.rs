use crate::risk::{Level, is_wide_glob, is_wide_host};
use crate::when::{self, Value};
use crate::*;

/// The manifest of docs/plugin-architecture.md §4, verbatim. If this stops parsing, the docs are wrong.
const DOC_EXAMPLE: &str = r#"
[extension]
id      = "dogan.deploy-status"         # <publisher>.<name>, lowercase a-z 0-9 -
version = "0.1.0"
name    = "Deploy Status"
license = "MIT"

[code]
entry = "src/main.ts"                   # or "Cargo.toml"; omit for declarative extensions

[permissions]
read = ["vercel.json"]
net  = ["api.vercel.com"]

[activation]
workspace-contains = ["vercel.json"]    # the only event you ever write by hand

[[commands]]
id    = "refresh"                       # becomes dogan.deploy-status.refresh
title = "Deploy: Refresh"

[[views]]
id       = "deployments"
title    = "Deployments"
location = "sidebar"                    # sidebar | bottom | editor
"#;

const THEME: &str = r#"
[extension]
id      = "you.midnight"
version = "0.1.0"
name    = "Midnight"
license = "MIT"

[[themes]]
label = "Midnight"
path  = "themes/midnight.json"

[[snippets]]
language = "rust"
path     = "snippets/rust.json"
"#;

fn errors(text: &str) -> Vec<Diagnostic> {
    parse(text).expect_err("manifest should be rejected")
}

fn codes(text: &str) -> Vec<Code> {
    errors(text).into_iter().map(|d| d.code).collect()
}

/// A minimal valid header followed by `rest`.
fn with(rest: &str) -> String {
    format!("[extension]\nid = \"a.b\"\nversion = \"1.0.0\"\nname = \"N\"\nlicense = \"MIT\"\n{rest}")
}

#[test]
fn the_architecture_example_parses() {
    let m = parse(DOC_EXAMPLE).unwrap();
    assert_eq!(m.id.as_str(), "dogan.deploy-status");
    assert_eq!(m.version.to_string(), "0.1.0");
    assert_eq!(m.code.as_ref().unwrap().language, CodeLanguage::TypeScript);
    assert_eq!(m.contributions.commands[0].id, "dogan.deploy-status.refresh");
    assert_eq!(m.contributions.views[0].location, ViewLocation::Sidebar);
    assert_eq!(
        m.activation,
        vec![
            Activation::Command("dogan.deploy-status.refresh".into()),
            Activation::View("dogan.deploy-status.deployments".into()),
            Activation::WorkspaceContains("vercel.json".into()),
        ]
    );
    assert!(m.untrusted);
    // Reads a named file and sends: Medium, no `why` needed.
    assert_eq!(m.risk().level, Level::Medium);
}

#[test]
fn a_theme_pack_needs_no_code_and_never_activates() {
    let m = parse(THEME).unwrap();
    assert!(m.is_declarative());
    assert!(m.activation.is_empty());
    assert_eq!(m.contributions.themes[0].path, "themes/midnight.json");
    assert_eq!(m.risk().level, Level::Low);
}

#[test]
fn syntax_errors_point_at_the_line() {
    let text = with("[[commands]]\nid = \"x\"\ntitel = \"typo\"\n");
    let e = errors(&text);
    assert_eq!(e[0].code, Code::Syntax);
    assert!(e[0].message.contains("unknown field `titel`"), "{}", e[0].message);
    assert_eq!(e[0].line_col(&text).map(|(l, _)| l), Some(8));
    assert!(e[0].render(FILE_NAME, &text).starts_with("error[CY0101]: unknown field `titel`"));
}

#[test]
fn a_missing_extension_table_says_how_to_start() {
    let e = errors("[code]\nentry = \"Cargo.toml\"\n");
    assert_eq!(e[0].code, Code::Syntax);
    assert!(e[0].fix.as_deref().unwrap().contains("[extension]"));
}

#[test]
fn ids_are_publisher_dot_name_in_lowercase() {
    for good in ["a.b", "dogan.deploy-status", "vercel.v0", "x1.y-2-z"] {
        assert!(ExtId::parse(good).is_ok(), "{good}");
    }
    for bad in ["deploy", "a.b.c", "A.b", "a.B", "a.-b", "a.b-", "a.b--c", "1a.b", "a._b", "a.b c", ".b", "a."] {
        assert!(ExtId::parse(bad).is_err(), "{bad}");
    }
    let text = THEME.replace("you.midnight", "You.Midnight");
    let e = errors(&text);
    assert_eq!(e[0].code, Code::BadId);
    assert_eq!(e[0].line_col(&text).map(|(l, _)| l), Some(3));
    assert!(e[0].fix.is_some());
}

#[test]
fn versions_follow_semver_order() {
    let v = |s| Version::parse(s).unwrap();
    assert!(v("1.0.0-alpha") < v("1.0.0-alpha.1"));
    assert!(v("1.0.0-alpha.1") < v("1.0.0-alpha.beta"));
    assert!(v("1.0.0-beta.2") < v("1.0.0-beta.11"));
    assert!(v("1.0.0-rc.1") < v("1.0.0"));
    assert!(v("1.2.3") < v("1.10.0"));
    for bad in ["1.0", "1.0.0.0", "01.0.0", "1.0.0+build", "1.0.0-", "1..0", "v1.0.0", "1.0.0-a..b"] {
        assert!(Version::parse(bad).is_err(), "{bad}");
    }
    assert_eq!(Version::parse_minimum("1.2").unwrap(), v("1.2.0"));
    assert_eq!(codes(&THEME.replace("0.1.0", "0.1")), vec![Code::BadVersion]);
}

#[test]
fn every_problem_is_reported_not_just_the_first() {
    let text = r#"
[extension]
id = "Bad"
version = "x"
name = ""
license = "MIT"
"#;
    assert_eq!(codes(text), vec![Code::BadId, Code::BadVersion, Code::BadText]);
}

#[test]
fn paths_stay_inside_the_package() {
    for bad in ["/etc/passwd", "../x.json", "themes/../../x.json", "themes\\\\x.json", "C:/x.json", "themes//x.json", ""] {
        let text = THEME.replace("themes/midnight.json", bad);
        assert!(codes(&text).contains(&Code::BadPath), "{bad}");
    }
}

#[test]
fn commands_without_code_are_refused_with_the_fix() {
    let e = errors(&with("[[commands]]\nid = \"go\"\ntitle = \"Go\"\n"));
    assert_eq!(e[0].code, Code::NeedsCode);
    assert!(e[0].fix.as_deref().unwrap().contains("[code]"));
}

#[test]
fn code_that_nothing_starts_is_refused() {
    let e = errors(&with("[code]\nentry = \"Cargo.toml\"\n"));
    assert_eq!(e[0].code, Code::NeverActivates);
    assert!(e[0].fix.as_deref().unwrap().contains("workspace-contains"));
    assert!(parse(&with("[code]\nentry = \"Cargo.toml\"\n[activation]\nworkspace-contains = [\"Cargo.toml\"]\n")).is_ok());
}

#[test]
fn the_entry_decides_the_language() {
    let lang = |entry: &str| parse(&with(&format!("[code]\nentry = \"{entry}\"\n[[commands]]\nid = \"x\"\ntitle = \"X\"\n"))).map(|m| m.code.unwrap().language);
    assert_eq!(lang("Cargo.toml").unwrap(), CodeLanguage::Rust);
    assert_eq!(lang("ext/Cargo.toml").unwrap(), CodeLanguage::Rust);
    assert_eq!(lang("src/main.ts").unwrap(), CodeLanguage::TypeScript);
    assert_eq!(lang("index.js").unwrap(), CodeLanguage::JavaScript);
    assert!(lang("main.py").is_err());
}

#[test]
fn duplicate_and_malformed_ids_are_caught() {
    let text = with(
        "[code]\nentry = \"Cargo.toml\"\n[[commands]]\nid = \"go\"\ntitle = \"A\"\n[[commands]]\nid = \"go\"\ntitle = \"B\"\n[[commands]]\nid = \"Go Now\"\ntitle = \"C\"\n",
    );
    assert_eq!(codes(&text), vec![Code::DuplicateId, Code::BadContribution]);
}

#[test]
fn high_risk_permissions_need_a_why() {
    let text = with("[permissions]\nread = [\"**\"]\nnet = [\"api.example.com\"]\n");
    let e = errors(&text);
    assert_eq!(e[0].code, Code::WhyRequired);
    assert!(e[0].message.contains("read") && e[0].message.contains("net"), "{}", e[0].message);
    assert!(e[0].fix.as_deref().unwrap().contains("why ="));
    assert!(e[0].line_col(&text).is_some());
    let ok = parse(&with("[permissions]\nread = [\"**\"]\nnet = [\"api.example.com\"]\nwhy = \"Uploads the files you choose.\"\n")).unwrap();
    assert_eq!(ok.risk().level, Level::High);
}

#[test]
fn risk_levels() {
    let lvl = |p: Permissions| risk(&p).level;
    assert_eq!(lvl(Permissions::default()), Level::Low);
    assert_eq!(lvl(Permissions { read: vec!["vercel.json".into()], ..Default::default() }), Level::Medium);
    assert_eq!(lvl(Permissions { net: vec!["api.x.com".into()], ..Default::default() }), Level::Medium);
    // Named files plus network: the files are the extension's own config, not private data.
    assert_eq!(lvl(Permissions { read: vec!["vercel.json".into()], net: vec!["api.x.com".into()], ..Default::default() }), Level::Medium);
    assert_eq!(lvl(Permissions { read: vec!["**/*.rs".into()], net: vec!["api.x.com".into()], ..Default::default() }), Level::High);
    assert_eq!(lvl(Permissions { auth: vec!["github".into()], net: vec!["api.github.com".into()], ..Default::default() }), Level::High);
    assert_eq!(lvl(Permissions { clipboard: Clipboard::Read, terminal: true, ..Default::default() }), Level::High);
    assert_eq!(lvl(Permissions { run: vec!["taplo".into()], ..Default::default() }), Level::High);
    assert_eq!(lvl(Permissions { secrets: true, ..Default::default() }), Level::Medium);
}

#[test]
fn wide_globs_and_hosts() {
    for g in ["*", "**", "**/*", "**/*.rs", "./**", "**/src/**"] {
        assert!(is_wide_glob(g), "{g}");
    }
    for g in ["vercel.json", "src/*.rs", ".github/workflows/*.yml"] {
        assert!(!is_wide_glob(g), "{g}");
    }
    assert!(is_wide_host("*.com") && is_wide_host("*.co.uk") && is_wide_host("*.vercel.app"));
    assert!(!is_wide_host("*.example.com") && !is_wide_host("api.example.com"));
}

#[test]
fn malformed_permissions_are_named() {
    let bad = [
        "net = [\"https://api.x.com\"]",
        "net = [\"*\"]",
        "net = [\"10.0.0.1\"]",
        "net = [\"localhost\"]",
        "net = [\"API.x.com\"]",
        "net = [\"api.x.com:8080\"]",
        "run = [\"/usr/bin/curl\"]",
        "run = [\"git status\"]",
        "run = [\"../bin/x\"]",
        "read = [\"/etc/*\"]",
        "read = [\"../**\"]",
        "commands = [\"workbench.open\"]",
        "clipboard = \"all\"",
        "auth = [\"Git Hub\"]",
    ];
    for b in bad {
        let text = with(&format!("[permissions]\n{b}\nwhy = \"x\"\n"));
        assert!(codes(&text).contains(&Code::BadPermission), "{b}");
    }
    let ok = with(
        "[permissions]\nnet = [\"*.example.com\", \"api.github.com\"]\nrun = [\"taplo\", \"bin/server\"]\nclipboard = \"write\"\ncommands = [\"core:workbench.openSettings\", \"core:debug.start\"]\nwhy = \"x\"\n",
    );
    parse(&ok).unwrap();
}

#[test]
fn why_is_short_and_has_no_links() {
    let long = "x".repeat(limits::WHY_LEN + 1);
    assert_eq!(codes(&with(&format!("[permissions]\nwhy = \"{long}\"\n"))), vec![Code::BadText]);
    assert_eq!(codes(&with("[permissions]\nwhy = \"see https://evil.example\"\n")), vec![Code::BadText]);
}

#[test]
fn a_language_server_must_declare_its_program() {
    let text = with("[[language-servers]]\ncommand = \"taplo\"\nargs = [\"lsp\", \"stdio\"]\nlanguages = [\"toml\"]\n");
    let e = errors(&text);
    assert_eq!(e[0].code, Code::NotDeclared);
    assert_eq!(e[0].message, "run access to \"taplo\" is not declared");
    assert_eq!(e[0].fix.as_deref(), Some("add to [permissions]:  run = [\"taplo\"]"));
    assert!(e[0].render(FILE_NAME, &text).contains("codeyard-extension.toml:7:"));
    let ok = parse(&format!("{text}[permissions]\nrun = [\"taplo\"]\nwhy = \"Starts the taplo language server for TOML files.\"\n")).unwrap();
    assert!(ok.is_declarative());
    assert_eq!(ok.contributions.language_servers[0].args, vec!["lsp", "stdio"]);
}

#[test]
fn interpreters_are_flagged_for_the_package_check() {
    let p = Permissions { run: vec!["node".into(), "taplo".into(), "/x/python3.12".into(), "bash".into()], ..Default::default() };
    assert_eq!(p.interpreters().collect::<Vec<_>>(), vec!["node", "/x/python3.12", "bash"]);
}

#[test]
fn keybindings_resolve_their_command() {
    let base = with("[code]\nentry = \"Cargo.toml\"\n[[commands]]\nid = \"go\"\ntitle = \"Go\"\n[dependencies]\n\"other.ext\" = \"1.2\"\n");
    let m = parse(&format!("{base}[[keybindings]]\nkey = \"cmd-shift-g\"\ncommand = \"go\"\nwhen = \"editor.language == 'rust'\"\n[[keybindings]]\nkey = \"ctrl-k ctrl-o\"\ncommand = \"other.ext.open\"\n[[keybindings]]\nkey = \"f5\"\ncommand = \"core:debug.start\"\n")).unwrap();
    let cmds: Vec<_> = m.contributions.keybindings.iter().map(|k| k.command.as_str()).collect();
    assert_eq!(cmds, vec!["a.b.go", "other.ext.open", "core:debug.start"]);
    assert_eq!(m.dependencies, vec![(ExtId::parse("other.ext").unwrap(), Version::parse("1.2.0").unwrap())]);
    assert_eq!(codes(&format!("{base}[[keybindings]]\nkey = \"cmd-x\"\ncommand = \"stranger.ext.x\"\n")), vec![Code::BadContribution]);
    assert_eq!(codes(&format!("{base}[[keybindings]]\nkey = \"cmd--\"\ncommand = \"go\"\n")), vec![Code::BadContribution]);
}

#[test]
fn dependencies_are_checked() {
    assert_eq!(codes(&with("[dependencies]\n\"a.b\" = \"1\"\n")), vec![Code::BadDependency]);
    assert_eq!(codes(&with("[dependencies]\n\"other\" = \"1\"\n")), vec![Code::BadDependency]);
    assert_eq!(codes(&with("[dependencies]\n\"o.x\" = \"^1\"\n")), vec![Code::BadDependency]);
}

#[test]
fn languages_and_schemas() {
    let m = parse(&with(
        "[[languages]]\nid = \"just\"\nfilenames = [\"Justfile\"]\nextensions = [\".just\"]\ngrammar = \"make\"\nline-comment = \"#\"\n[[json-schemas]]\nfile-match = [\"vercel.json\"]\npath = \"schemas/vercel.json\"\n",
    ))
    .unwrap();
    assert_eq!(m.contributions.languages[0].extensions, vec!["just"]);
    assert_eq!(m.contributions.json_schemas[0].file_match, vec!["vercel.json"]);
    assert_eq!(codes(&with("[[languages]]\nid = \"x\"\n")), vec![Code::BadContribution]);
    // Not in this version: language queries, icon themes, debug adapters (docs/plugin-architecture.md, Later).
    assert_eq!(codes(&with("[[languages]]\nid = \"x\"\nextensions = [\"x\"]\nqueries = \"q\"\n")), vec![Code::Syntax]);
    assert_eq!(codes(&with("[[icon-themes]]\nlabel = \"x\"\npath = \"x.json\"\n")), vec![Code::Syntax]);
    assert_eq!(codes(&with("[[debug-adapters]]\ntype = \"x\"\ncommand = \"x\"\n")), vec![Code::Syntax]);
    assert_eq!(codes(&with("[[yaml-schemas]]\nfile-match = []\npath = \"s.json\"\n")), vec![Code::BadContribution]);
}

#[test]
fn settings_check_their_defaults() {
    let m = parse(&with("[settings.interval]\ntype = \"number\"\ndefault = 60\ndescription = \"Seconds between refreshes.\"\n")).unwrap();
    assert_eq!(m.contributions.settings[0].key, "interval");
    assert_eq!(m.contributions.settings[0].kind, SettingKind::Number);
    assert_eq!(codes(&with("[settings.interval]\ntype = \"number\"\ndefault = \"60\"\ndescription = \"d\"\n")), vec![Code::BadContribution]);
    assert_eq!(codes(&with("[settings.mode]\ntype = \"string\"\nenum = [\"a\", \"b\"]\ndefault = \"c\"\ndescription = \"d\"\n")), vec![Code::BadContribution]);
    assert_eq!(codes(&with("[settings.n]\ntype = \"int\"\ndescription = \"d\"\n")), vec![Code::BadContribution]);
}

#[test]
fn view_locations_and_when() {
    let base = with("[code]\nentry = \"Cargo.toml\"\n");
    assert_eq!(codes(&format!("{base}[[views]]\nid = \"v\"\ntitle = \"V\"\nlocation = \"left\"\n")), vec![Code::BadContribution]);
    assert_eq!(codes(&format!("{base}[[views]]\nid = \"v\"\ntitle = \"V\"\nlocation = \"bottom\"\nwhen = \"secret.key\"\n")), vec![Code::BadWhen]);
    let m = parse(&format!("{base}[[views]]\nid = \"v\"\ntitle = \"V\"\nlocation = \"editor\"\nwhen = \"!terminal && ext.ready\"\n")).unwrap();
    assert_eq!(m.contributions.views[0].when.as_deref(), Some("!terminal && ext.ready"));
}

#[test]
fn untrusted_defaults_to_running() {
    assert!(parse(THEME).unwrap().untrusted);
    assert!(!parse(&format!("untrusted = false\n{THEME}")).unwrap().untrusted);
}

#[test]
fn oversized_manifests_are_refused_before_parsing() {
    let text = format!("{THEME}\n#{}", "x".repeat(limits::MANIFEST_BYTES));
    assert_eq!(codes(&text), vec![Code::TooLarge]);
}

#[test]
fn every_error_code_is_distinct() {
    let mut seen = std::collections::HashSet::new();
    for c in Code::ALL {
        assert!(seen.insert(c.as_str()), "{} is used twice", c.as_str());
        assert!(c.as_str().starts_with("CY") && c.as_str().len() == 6);
    }
}

#[test]
fn when_expressions() {
    let ctx = |k: &str| match k {
        "editor.language" => Some(Value::Str("rust".into())),
        "terminal" => Some(Value::Bool(false)),
        "ext.count" => Some(Value::Num(3.0)),
        _ => None,
    };
    let t = |src: &str| when::parse(src).unwrap().eval(&ctx);
    assert!(t("editor.language == 'rust' && !terminal"));
    assert!(t("editor.language != \"go\""));
    assert!(t("terminal || editor.language == 'rust'"));
    assert!(!t("terminal && editor.language == 'rust'"));
    assert!(t("!(terminal || scm)"));
    assert!(t("ext.count == 3"));
    assert!(!t("scm"), "unset keys are false");
    assert!(t("true") && !t("false"));
    assert_eq!(when::parse("editor && ext.x").unwrap().keys(), vec!["editor", "ext.x"]);
    for bad in ["", "editor &&", "editor & scm", "editor = 'x'", "(editor", "editor)", "'open", "workbench.internal", "ext.", "editor scm", "#"] {
        assert!(when::parse(bad).is_err(), "{bad:?}");
    }
    let deep = format!("{}editor{}", "(".repeat(limits::WHEN_DEPTH + 1), ")".repeat(limits::WHEN_DEPTH + 1));
    assert!(when::parse(&deep).unwrap_err().contains("nested"));
    assert!(when::parse(&"!".repeat(limits::WHEN_DEPTH + 1)).is_err());
}

#[test]
fn consent_lines_use_plain_words() {
    let p = Permissions {
        read: vec!["vercel.json".into()],
        net: vec!["api.vercel.com".into()],
        auth: vec!["github".into()],
        run: vec!["vercel".into()],
        ..Default::default()
    };
    assert_eq!(p.describe(), vec!["Read vercel.json", "Connect to api.vercel.com", "Act as your GitHub account", "Run vercel with any arguments"]);
    let wide = Permissions { read: vec!["**".into()], net: vec!["*.com".into()], ..Default::default() };
    assert_eq!(wide.describe(), vec!["Read all files in the workspace", "Connect to any website"]);
}

// Regressions from the hardening review of #50: each input once rated too low or was accepted.

#[test]
fn every_way_to_say_all_files_is_wide() {
    for g in ["*/**", ".//**", "**/?*", "**/[!.]*", "**/**/*", "?**", "[a-z]*/**", "src/**", "*/x.json", "*", "?", "*.*", "x*"] {
        assert!(is_wide_glob(g), "{g}");
        let p = Permissions { read: vec![g.into()], net: vec!["evil.example.com".into()], ..Default::default() };
        assert_eq!(risk(&p).level, Level::High, "{g}");
    }
    for g in ["vercel.json", "src/*.rs", ".github/workflows/*.yml", "deploy-*.json", "docs/index.md"] {
        assert!(!is_wide_glob(g), "{g}");
    }
}

#[test]
fn secret_files_count_as_private_data() {
    for g in [".env", ".env.local", ".git/config", "config/.npmrc", "certs/server.pem", "deploy.key", ".ssh/id_ed25519", ".e*", ".aws/credentials"] {
        assert!(crate::risk::is_sensitive_glob(g), "{g}");
        let p = Permissions { read: vec![g.into()], net: vec!["api.example.com".into()], ..Default::default() };
        assert_eq!(risk(&p).level, Level::High, "{g}");
    }
    assert!(!crate::risk::is_sensitive_glob("vercel.json"));
    let p = Permissions { read: vec![".env".into()], ..Default::default() };
    assert!(p.describe().contains(&"Read files that can hold passwords and keys".to_string()));
}

#[test]
fn terminals_and_code_files_run_code() {
    let lvl = |p: Permissions| {
        let r = risk(&p);
        assert!(r.runs_code, "{p:?}");
        r.level
    };
    assert_eq!(lvl(Permissions { terminal: true, ..Default::default() }), Level::High);
    assert_eq!(lvl(Permissions { write: vec!["**".into()], ..Default::default() }), Level::High);
    for g in [".git/hooks/pre-commit", ".vscode/tasks.json", ".envrc", "Makefile", "build.rs", "package.json", ".github/workflows/ci.yml"] {
        assert_eq!(lvl(Permissions { write: vec![g.into()], ..Default::default() }), Level::High, "{g}");
    }
    assert!(!risk(&Permissions { write: vec!["CHANGELOG.md".into()], ..Default::default() }).runs_code);
}

#[test]
fn invisible_characters_cannot_disguise_the_sheet() {
    for bad in ["\u{202e}", "\u{200b}", "\u{2066}", "\u{feff}", "\u{00ad}"] {
        assert_eq!(codes(&with(&format!("[permissions]\nwhy = \"ok{bad}ok\"\n"))), vec![Code::BadText], "{bad:?}");
        assert_eq!(codes(&THEME.replace("name    = \"Midnight\"", &format!("name    = \"Mid{bad}night\""))), vec![Code::BadText], "{bad:?}");
    }
    assert_eq!(codes(&with("[permissions]\nread = [\"vercel.json\\nConnect to nothing\"]\n")), vec![Code::BadPermission]);
    assert_eq!(codes(&with("[permissions]\ncommands = [\"core:a\\u202eb c\"]\n")), vec![Code::BadPermission]);
}

#[test]
fn interpreters_are_recognised_in_any_spelling() {
    for p in [
        "python.exe",
        "NODE.EXE",
        "bash.exe",
        "py",
        "python2",
        "pypy3",
        "pythonw",
        "node22",
        "wsl",
        "busybox",
        "xargs",
        "npx",
        "Rscript",
        "wscript",
        "pwsh.exe",
        "cmd.bat",
    ] {
        assert!(crate::risk::is_interpreter(p), "{p}");
    }
    for p in ["taplo", "rust-analyzer", "vercel", "nodemon", "pythonista-lsp"] {
        assert!(!crate::risk::is_interpreter(p), "{p}");
    }
}

#[test]
fn keybindings_to_core_commands_are_checked_by_grammar() {
    let base = with("");
    let m = parse(&format!("{base}[[keybindings]]\nkey = \"f5\"\ncommand = \"core:debug.start\"\n")).unwrap();
    assert_eq!(m.contributions.keybindings[0].command, "core:debug.start");
    assert!(m.permissions.is_empty(), "a key binding needs no permission");
    for bad in ["core:", "core:.x", "core:a..b", "core:a b", "core:1x"] {
        assert_eq!(codes(&format!("{base}[[keybindings]]\nkey = \"f5\"\ncommand = \"{bad}\"\n")), vec![Code::BadContribution], "{bad}");
    }
}

#[test]
fn activation_without_code_is_refused() {
    let e = errors(&format!("{THEME}[activation]\nworkspace-contains = [\"x\"]\n"));
    assert_eq!(e[0].code, Code::BadContribution);
    assert!(e[0].message.contains("[activation]"));
}

#[test]
fn cyx_dev_can_warn_about_a_missing_why() {
    let text = with("[permissions]\nrun = [\"taplo\"]\n");
    assert_eq!(codes(&text), vec![Code::WhyRequired]);
    let m = parse_with(&text, Options { require_why: false }).unwrap();
    assert!(m.missing_why());
}

#[test]
fn numeric_pre_releases_have_no_leading_zeros() {
    assert!(Version::parse("1.0.0-01").is_err());
    assert!(Version::parse("1.0.0-0").is_ok() && Version::parse("1.0.0-0a").is_ok());
}

#[test]
fn file_names_are_portable() {
    for bad in
        ["themes/a.json:evil.json", "con.json", "themes/NUL", "aux/x.json", "themes/x.json.", "themes/x.json ", ".. /x", "~/x.json", "t*.json", "./x.json"]
    {
        assert!(check_package_path(bad).is_err(), "{bad}");
    }
    for g in ["~/.ssh/id_rsa", "{..,x}/secret", "C:/x", "a//b", "a/./b"] {
        assert!(codes(&with(&format!("[permissions]\nread = [\"{g}\"]\nwhy = \"x\"\n"))).contains(&Code::BadPermission), "{g}");
    }
}

#[test]
fn more_shared_suffixes_are_any_host() {
    for h in ["*.s3.amazonaws.com", "*.ngrok.io", "*.workers.dev", "*.com.br"] {
        assert!(is_wide_host(h), "{h}");
    }
}

#[test]
fn net_allows_exact_hosts_and_names_under_a_wildcard() {
    let net: Vec<String> = vec!["api.vercel.com".into(), "*.example.com".into(), "*.co.uk".into()];
    for h in ["api.vercel.com", "a.example.com", "a.b.example.com", "anything.co.uk"] {
        assert!(net_allows(&net, h), "{h}");
    }
    for h in [
        "vercel.com",
        "x.api.vercel.com",
        "api.vercel.com.evil.test",
        "example.com",
        "badexample.com",
        ".example.com",
        "a..example.com",
        "a.example.com.",
        "",
        "co.uk",
    ] {
        assert!(!net_allows(&net, h), "{h}");
    }
    // `*` is refused by validation; even if it got through, it matches nothing.
    assert!(!net_allows(&["*".into()], "api.x.com"));
    assert!(!net_allows(&["*.".into()], "a."));
    let p = Permissions { net: vec!["*.example.com".into()], ..Default::default() };
    assert!(p.allows_host("api.example.com") && !p.allows_host("api.example.org"));
}

#[test]
fn an_update_asks_only_for_what_is_new() {
    let old = Permissions {
        read: vec!["*.toml".into()],
        net: vec!["api.x.com".into()],
        clipboard: Clipboard::Write,
        why: Some("Old reason.".into()),
        ..Default::default()
    };
    // Fewer, or the same with another reason: nothing to ask.
    assert!(Permissions { read: vec!["*.toml".into()], ..Default::default() }.added_since(&old).is_empty());
    assert!(Permissions { why: Some("New reason.".into()), ..old.clone() }.added_since(&old).is_empty());
    let new = Permissions {
        read: vec!["*.toml".into(), "*.lock".into()],
        net: vec!["api.x.com".into()],
        clipboard: Clipboard::ReadWrite,
        terminal: true,
        ..Default::default()
    };
    let added = new.added_since(&old);
    assert_eq!(added.describe(), ["Read *.lock", "Read the clipboard", "Type into terminals"]);
    // A narrower entry than before is still a new one: entries compare as written.
    let narrower = Permissions { net: vec!["v2.api.x.com".into()], ..Default::default() };
    assert_eq!(narrower.added_since(&old).describe(), ["Connect to v2.api.x.com"]);
}

const ACME: &str = "[[oauth]]\nid = \"acme\"\nname = \"Acme\"\nauthorize-url = \"https://acme.example.com/oauth/authorize\"\ntoken-url = \"https://acme.example.com/oauth/token\"\nclient-id = \"cy-123\"\nscopes = [\"read\", \"deploy:write\"]\nhosts = [\"api.acme.example.com\"]\naccount-url = \"https://api.acme.example.com/me\"\n";

#[test]
fn an_oauth_provider_is_declared_and_named_in_auth() {
    let m = parse(&with(&format!("[permissions]\nauth = [\"github\", \"acme\"]\nwhy = \"x\"\n{ACME}"))).unwrap();
    let p = m.oauth_provider("acme").expect("declared");
    assert_eq!(p.name, "Acme");
    assert_eq!(p.token_url, "https://acme.example.com/oauth/token");
    assert_eq!(p.scopes, ["read", "deploy:write"]);
    assert_eq!(p.hosts, ["api.acme.example.com"]);
    assert_eq!(p.account_url.as_deref(), Some("https://api.acme.example.com/me"));
    assert!(m.oauth_provider("github").is_none(), "GitHub is built in");
    // An `auth` entry that is neither built in nor declared.
    let e = errors(&with("[permissions]\nauth = [\"acme\"]\n"));
    assert_eq!(e[0].code, Code::NotDeclared);
    assert!(e[0].fix.as_deref().is_some_and(|f| f.contains("[[oauth]]")), "{:?}", e[0].fix);
    // A provider that `auth` does not name.
    assert_eq!(codes(&with(&format!("[permissions]\nauth = [\"github\"]\n{ACME}"))), vec![Code::NotDeclared]);
    // GitHub cannot be redeclared, and an id is used once.
    let gh = ACME.replace("id = \"acme\"", "id = \"github\"");
    assert_eq!(codes(&with(&format!("[permissions]\nauth = [\"github\"]\n{gh}"))), vec![Code::BadContribution]);
    assert_eq!(codes(&with(&format!("[permissions]\nauth = [\"acme\"]\n{ACME}{ACME}"))), vec![Code::DuplicateId]);
}

#[test]
fn oauth_urls_are_https_with_a_host_name() {
    let bad = [
        ("token-url = \"https://acme.example.com/oauth/token\"", "token-url = \"http://acme.example.com/oauth/token\""),
        ("token-url = \"https://acme.example.com/oauth/token\"", "token-url = \"https://10.0.0.1/token\""),
        ("token-url = \"https://acme.example.com/oauth/token\"", "token-url = \"https://user:pw@acme.example.com/token\""),
        ("token-url = \"https://acme.example.com/oauth/token\"", "token-url = \"https://localhost/token\""),
        ("authorize-url = \"https://acme.example.com/oauth/authorize\"", "authorize-url = \"javascript:alert(1)\""),
        ("authorize-url = \"https://acme.example.com/oauth/authorize\"", "authorize-url = \"https://acme.example.com/a#frag\""),
        ("account-url = \"https://api.acme.example.com/me\"", "account-url = \"file:///etc/passwd\""),
        ("hosts = [\"api.acme.example.com\"]", "hosts = [\"*\"]"),
        ("hosts = [\"api.acme.example.com\"]", "hosts = []"),
        ("client-id = \"cy-123\"", "client-id = \"a&redirect_uri=x\""),
        ("scopes = [\"read\", \"deploy:write\"]", "scopes = [\"read write\"]"),
    ];
    for (from, to) in bad {
        let text = with(&format!("[permissions]\nauth = [\"acme\"]\n{}", ACME.replace(from, to)));
        assert!(parse(&text).is_err(), "{to}");
    }
    assert_eq!(check_https_url("https://auth.example.com:8443/token?x=1").as_deref(), Ok("auth.example.com"));
    assert!(check_https_url("https://auth.example.com:99999/").is_err());
    // An unknown field is an error, as everywhere in the manifest.
    let typo = ACME.replace("client-id", "client_id");
    assert_eq!(codes(&with(&format!("[permissions]\nauth = [\"acme\"]\n{typo}"))), vec![Code::Syntax]);
}

const CODE: &str = "[code]\nentry = \"Cargo.toml\"\n[[commands]]\nid = \"go\"\ntitle = \"Go\"\n[[views]]\nid = \"list\"\ntitle = \"List\"\nlocation = \"sidebar\"\n[dependencies]\n\"other.ext\" = \"1\"\n";

#[test]
fn menus_name_a_command_a_place_and_for_view_titles_a_view() {
    let base = with(CODE);
    let m = parse(&format!(
        "{base}[[menus]]\ncommand = \"go\"\nlocation = \"editor-context\"\nwhen = \"editor.language == 'rust'\"\n\
         [[menus]]\ncommand = \"other.ext.open\"\nlocation = \"file-tree\"\n\
         [[menus]]\ncommand = \"go\"\nlocation = \"view-title\"\nview = \"list\"\n"
    ))
    .unwrap();
    let menus = &m.contributions.menus;
    assert_eq!(
        menus.iter().map(|m| (m.command.as_str(), m.location)).collect::<Vec<_>>(),
        [("a.b.go", MenuLocation::EditorContext), ("other.ext.open", MenuLocation::FileTree), ("a.b.go", MenuLocation::ViewTitle)]
    );
    assert_eq!(menus[2].view.as_deref(), Some("a.b.list"));
    assert_eq!(menus[0].when.as_deref(), Some("editor.language == 'rust'"));
    for bad in [
        "[[menus]]\ncommand = \"stranger.ext.x\"\nlocation = \"file-tree\"\n",
        "[[menus]]\ncommand = \"go\"\nlocation = \"toolbar\"\n",
        "[[menus]]\ncommand = \"go\"\nlocation = \"view-title\"\n",
        "[[menus]]\ncommand = \"go\"\nlocation = \"view-title\"\nview = \"nope\"\n",
        "[[menus]]\ncommand = \"go\"\nlocation = \"file-tree\"\nview = \"list\"\n",
    ] {
        assert_eq!(codes(&format!("{base}{bad}")), vec![Code::BadContribution], "{bad}");
    }
    assert_eq!(codes(&format!("{base}[[menus]]\ncommand = \"go\"\nlocation = \"file-tree\"\nwhen = \"editor &&\"\n")), vec![Code::BadWhen]);
}

#[test]
fn menus_may_name_listed_core_commands_that_need_no_argument() {
    let base = with(CODE);
    let m = parse(&format!("{base}[[menus]]\ncommand = \"core:workbench.saveAll\"\nlocation = \"view-title\"\nview = \"list\"\n")).unwrap();
    assert_eq!(m.contributions.menus[0].command, "core:workbench.saveAll");
    for bad in ["core:workbench.quit", "core:editor.open"] {
        let text = format!("{base}[[menus]]\ncommand = \"{bad}\"\nlocation = \"file-tree\"\n");
        assert_eq!(codes(&text), vec![Code::BadContribution], "{bad}");
    }
}

#[test]
fn status_items_have_local_ids_and_their_own_commands() {
    let base = with(CODE);
    let m = parse(&format!("{base}[[status-bar]]\nid = \"deploy\"\ncommand = \"go\"\n[[status-bar]]\nid = \"count\"\n")).unwrap();
    let items: Vec<_> = m.contributions.status_items.iter().map(|s| (s.id.as_str(), s.command.as_deref())).collect();
    assert_eq!(items, [("a.b.deploy", Some("a.b.go")), ("a.b.count", None)]);
    assert_eq!(codes(&format!("{base}[[status-bar]]\nid = \"x\"\n[[status-bar]]\nid = \"x\"\n")), vec![Code::DuplicateId]);
    assert_eq!(codes(&format!("{base}[[status-bar]]\nid = \"x\"\ncommand = \"nope\"\n")), vec![Code::BadContribution]);
}

#[test]
fn providers_declare_languages_and_start_the_extension_for_them() {
    let m = parse(&with(
        "[code]\nentry = \"Cargo.toml\"\n[[formatters]]\nlanguages = [\"toml\"]\n[[code-actions]]\nlanguages = [\"toml\", \"rust\"]\n\
         [[diagnostics]]\nid = \"todos\"\nlanguages = [\"rust\"]\n[[decorations]]\nid = \"coverage\"\nlanguages = [\"rust\", \"rust\"]\n[[tasks]]\nid = \"deploys\"\n",
    ))
    .unwrap();
    let c = &m.contributions;
    assert!(c.formatters[0].handles("toml") && !c.formatters[0].handles("rust"));
    assert_eq!(c.diagnostics[0].id, "a.b.todos");
    assert_eq!(c.decorations[0].languages, ["rust"], "a language named twice counts once");
    assert_eq!(c.tasks, ["a.b.deploys"]);
    assert_eq!(m.activation, [Activation::Language("toml".into()), Activation::Language("rust".into()), Activation::Tasks]);
    // Providers run code, so they need [code]; and they need a language that looks like one.
    assert_eq!(codes(&with("[[formatters]]\nlanguages = [\"toml\"]\n")), vec![Code::NeedsCode]);
    assert_eq!(codes(&with("[code]\nentry = \"Cargo.toml\"\n[[formatters]]\nlanguages = []\n")), vec![Code::BadContribution, Code::NeverActivates]);
    assert_eq!(codes(&with("[code]\nentry = \"Cargo.toml\"\n[[code-actions]]\nlanguages = [\"rust\", \"C++\"]\n")), vec![Code::BadContribution]);
    assert_eq!(codes(&with("[code]\nentry = \"Cargo.toml\"\n[[diagnostics]]\nid = \"Bad\"\nlanguages = [\"rust\"]\n")), vec![Code::BadContribution]);
    assert_eq!(codes(&with("[code]\nentry = \"Cargo.toml\"\n[[formatters]]\nlanguage = \"toml\"\n")), vec![Code::Syntax]);
}

#[test]
fn code_lenses_and_inlay_hints_declare_languages_and_start_the_extension_for_them() {
    let m = parse(&with("[code]\nentry = \"Cargo.toml\"\n[[code-lens]]\nlanguages = [\"rust\"]\n[[inlay-hints]]\nlanguages = [\"toml\", \"rust\"]\n")).unwrap();
    let c = &m.contributions;
    assert!(c.code_lenses[0].handles("rust") && !c.code_lenses[0].handles("toml"));
    assert_eq!(c.inlay_hints[0].languages, ["toml", "rust"]);
    assert!(c.needs_code());
    assert_eq!(m.activation, [Activation::Language("rust".into()), Activation::Language("toml".into())]);
    assert_eq!(codes(&with("[[code-lens]]\nlanguages = [\"rust\"]\n")), vec![Code::NeedsCode]);
    assert_eq!(codes(&with("[code]\nentry = \"Cargo.toml\"\n[[inlay-hints]]\nlanguages = []\n")), vec![Code::BadContribution, Code::NeverActivates]);
    assert_eq!(codes(&with("[code]\nentry = \"Cargo.toml\"\n[[code-lens]]\nlanguages = [\"rust\", \"C++\"]\n")), vec![Code::BadContribution]);
    assert_eq!(codes(&with("[code]\nentry = \"Cargo.toml\"\n[[code-lens]]\nlanguage = \"rust\"\n")), vec![Code::Syntax]);
}
