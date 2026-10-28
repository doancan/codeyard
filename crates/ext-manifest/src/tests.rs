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
        "[permissions]\nnet = [\"*.example.com\", \"api.github.com\"]\nrun = [\"taplo\", \"bin/server\"]\nclipboard = \"write\"\ncommands = [\"core:workbench.open-settings\"]\nwhy = \"x\"\n",
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
        "[[languages]]\nid = \"just\"\nfilenames = [\"Justfile\"]\nextensions = [\".just\"]\ngrammar = \"make\"\nqueries = \"queries/just\"\nline-comment = \"#\"\n[[json-schemas]]\nfile-match = [\"vercel.json\"]\npath = \"schemas/vercel.json\"\n",
    ))
    .unwrap();
    assert_eq!(m.contributions.languages[0].extensions, vec!["just"]);
    assert_eq!(m.contributions.json_schemas[0].file_match, vec!["vercel.json"]);
    assert_eq!(codes(&with("[[languages]]\nid = \"x\"\n")), vec![Code::BadContribution]);
    assert_eq!(codes(&with("[[languages]]\nid = \"x\"\nextensions = [\"x\"]\nqueries = \"q\"\n")), vec![Code::BadContribution]);
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
