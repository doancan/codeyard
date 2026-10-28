//! Random (but repeatable) inputs: a manifest comes from strangers, so no input may panic, every error span must sit on
//! a char boundary inside the text, and whatever parses must re-validate the same way.

use crate::{limits, parse, when};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const SEED: &str = r#"[extension]
id = "dogan.deploy-status"
version = "0.1.0-beta.2"
name = "Deploy Status"
license = "MIT"
categories = ["Deploy"]

[code]
entry = "src/main.ts"

[permissions]
read = ["vercel.json", "**/*.ts"]
net = ["api.vercel.com", "*.example.com"]
run = ["vercel"]
clipboard = "read-write"
why = "Shows deployments."

[activation]
workspace-contains = ["vercel.json"]

[dependencies]
"other.ext" = "1.2"

[[commands]]
id = "refresh"
title = "Deploy: Refresh"
enablement = "workspace.trusted && !terminal"

[[views]]
id = "deployments"
title = "Deployments"
location = "sidebar"
when = "ext.ready || editor.language == 'json'"

[[keybindings]]
key = "cmd-shift-d"
command = "refresh"

[[languages]]
id = "vercel"
filenames = ["vercel.json"]
block-comment = ["/*", "*/"]

[settings.interval]
type = "number"
default = 60
description = "Seconds."

[[language-servers]]
command = "vercel"
languages = ["json"]
"#;

const PIECES: &[&str] = &[
    "[",
    "]",
    "[[",
    "]]",
    "\"",
    "'",
    "=",
    "\n",
    " ",
    ",",
    ".",
    "-",
    "#",
    "\\",
    "\\u0000",
    "..",
    "/",
    "*",
    "**",
    "!",
    "&&",
    "||",
    "(",
    ")",
    "ş",
    "İ",
    "🦀",
    "\u{202e}",
    "\t",
    "true",
    "false",
    "1e309",
    "-0",
    "0x10",
    "{",
    "}",
    "commands",
    "permissions",
    "run",
    "why",
    "core:",
    "ext.",
];

fn check(text: &str) {
    match parse(text) {
        Ok(m) => {
            // Whatever is accepted holds the invariants the rest of the platform relies on.
            assert!(crate::ExtId::parse(m.id.as_str()).is_ok());
            for c in &m.contributions.commands {
                assert!(c.id.starts_with(&format!("{}.", m.id)));
            }
            if m.risk().needs_why() {
                assert!(m.permissions.why.is_some());
            }
            assert!(m.code.is_some() || !m.contributions.needs_code());
        }
        Err(errors) => {
            assert!(!errors.is_empty());
            for e in errors {
                if let Some(span) = &e.span {
                    assert!(span.start <= span.end && span.end <= text.len(), "span {span:?} outside {} bytes", text.len());
                    assert!(text.is_char_boundary(span.start) && text.is_char_boundary(span.end), "span {span:?} splits a character");
                }
                let _ = e.render("m.toml", text);
            }
        }
    }
}

#[test]
fn mutated_manifests_never_panic() {
    let mut rng = Rng(0x5eed_cafe);
    check(SEED);
    for _ in 0..4000 {
        let mut text = SEED.to_string();
        for _ in 0..1 + rng.below(6) {
            let mut at = rng.below(text.len() + 1);
            while !text.is_char_boundary(at) {
                at -= 1;
            }
            match rng.below(3) {
                0 => text.insert_str(at, PIECES[rng.below(PIECES.len())]),
                1 => {
                    let mut end = (at + 1 + rng.below(12)).min(text.len());
                    while !text.is_char_boundary(end) {
                        end -= 1;
                    }
                    text.replace_range(at..end, "");
                }
                _ => {
                    let line = text.lines().nth(rng.below(60)).unwrap_or("").to_string();
                    text.insert_str(at, &format!("\n{line}\n"));
                }
            }
        }
        check(&text);
    }
}

#[test]
fn random_bytes_never_panic() {
    let mut rng = Rng(0xdead_beef);
    for _ in 0..2000 {
        let len = rng.below(300);
        let text: String = (0..len).map(|_| PIECES[rng.below(PIECES.len())]).collect();
        check(&text);
    }
}

#[test]
fn when_never_panics() {
    let mut rng = Rng(0x0bad_f00d);
    for _ in 0..5000 {
        let src: String = (0..rng.below(40)).map(|_| PIECES[rng.below(PIECES.len())]).collect();
        if let Ok(e) = when::parse(&src) {
            let _ = e.eval(&|_| None);
        }
    }
    // Nesting is bounded, so evaluation cannot overflow the stack.
    let deep = "!(".repeat(10_000) + "editor" + &")".repeat(10_000);
    assert!(when::parse(&deep).is_err());
    let long_chain = vec!["editor"; limits::WHEN_LEN].join("&&");
    assert!(when::parse(&long_chain).is_err());
}

#[test]
fn huge_tables_are_bounded() {
    let many: String = (0..limits::CONTRIBUTIONS_PER_KIND + 1).map(|i| format!("[[themes]]\nlabel = \"t{i}\"\npath = \"t{i}.json\"\n")).collect();
    let e = parse(&format!("{SEED}{many}")).unwrap_err();
    assert!(e.iter().any(|d| d.message.contains("more than")), "{e:?}");
}
