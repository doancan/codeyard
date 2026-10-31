//! `cyx explain CODE` and the generated author references made from cyx's own tables: the error catalogue
//! (`docs/extensions/reference/errors.md`) and the command line (`docs/extensions/reference/cli.md`).

use ext_manifest::{Code, ErrorDoc};

/// Where the error reference lives in the repository; each code is an anchor in it.
pub const ERRORS_DOC: &str = "docs/extensions/reference/errors.md";

/// Every `CY` code, in order: the manifest and component checks, then the registry index and publisher signatures.
pub fn catalogue() -> Vec<ErrorDoc> {
    let mut all: Vec<ErrorDoc> = Code::ALL.iter().map(|c| c.doc()).collect();
    all.extend(ext_core::yard::CODES);
    all.extend(ext_core::publisher::CODES);
    all
}

/// The anchor of a code in the error reference: the code in lowercase, as GitHub writes heading anchors.
pub fn anchor(code: &str) -> String {
    code.to_ascii_lowercase()
}

/// What `cyx explain` prints for `code` (any case).
pub fn explain(code: &str) -> Result<String, String> {
    let want = code.trim().to_ascii_uppercase();
    let doc = catalogue()
        .into_iter()
        .find(|d| d.code == want)
        .ok_or_else(|| format!("error: there is no error code '{code}'\n   = fix: codes look like CY0201; {ERRORS_DOC} lists them all"))?;
    Ok(format!("{}: {}\n   = fix: {}\n   = docs: {ERRORS_DOC}#{}", doc.code, doc.summary, doc.fix, anchor(doc.code)))
}

/// Ranges of the catalogue with a heading each, by the first two digits of the code.
#[cfg(test)]
const RANGES: [(&str, &str); 9] = [
    ("CY01", "The manifest file"),
    ("CY02", "Permissions"),
    ("CY03", "Contributions"),
    ("CY04", "Activation"),
    ("CY05", "`when` expressions"),
    ("CY06", "Dependencies"),
    ("CY07", "The component (`extension.wasm`)"),
    ("CY080", "The registry index"),
    ("CY081", "Publisher signatures"),
];

#[cfg(test)]
const UPDATE: &str = "UPDATE_DOCS=1 cargo test -p cyx reference";

/// `docs/extensions/reference/errors.md`.
#[cfg(test)]
pub fn errors_reference() -> String {
    let mut out = format!(
        "# Errors\n\n<!-- Generated from ext-manifest's `Code` and ext-core's registry and publisher codes: run `{UPDATE}` after \
         changing them. -->\n\n\
         Every problem `cyx`, the Extensions view and the registry report has a stable code. The number never changes \
         meaning, and a retired code is not reused. Each error says what is wrong, where, and what to change:\n\n\
         ```text\nerror[CY0201]: net access to \"api.vercel.com\" is not declared\n  --> codeyard-extension.toml:7:1\n   = fix: add to \
         [permissions]:  net = [\"api.vercel.com\"]\n```\n\n\
         The fix line is the exact change when there is one; otherwise it is the general fix below. `cyx explain CY0201` \
         prints a code's entry. Refused calls at run time are not errors of the manifest: they are `denied` with their own fix \
         ([Commands](../commands.md#errors-and-refused-calls)).\n"
    );
    for (prefix, title) in RANGES {
        let codes: Vec<ErrorDoc> = catalogue().into_iter().filter(|d| d.code.starts_with(prefix)).collect();
        out.push_str(&format!("\n## {title}\n"));
        for d in codes {
            out.push_str(&format!("\n### {}\n\n{}\n\n**Fix:** {}\n", d.code, d.summary, d.fix));
        }
    }
    out
}

/// Environment variables cyx reads, for the CLI reference. A test checks each one against the sources.
#[cfg(test)]
pub const ENVIRONMENT: [(&str, &str); 10] = [
    ("CODEYARD_REGISTRY", "The registry `cyx publish`, `yank`, `restore` and `deprecate` use when there is no `--registry`."),
    ("CODEYARD_BIN", "The Codeyard `cyx dev` starts, instead of `codeyard` on `PATH` or the installed app."),
    ("CODEYARD_EXTHOST", "The `codeyard-exthost` `cyx test` runs, instead of the one beside `cyx` or on `PATH`."),
    ("CODEYARD_CYX_CACHE", "Where cyx keeps the TypeScript build tools it installs, instead of the platform's cache folder."),
    ("SIGSTORE_ID_TOKEN", "An OIDC token for signing, instead of GitHub Actions' token or a browser sign-in."),
    ("CODEYARD_FULCIO_URL", "Another Sigstore certificate authority (a private Sigstore, or tests)."),
    ("CODEYARD_REKOR_URL", "Another Sigstore transparency log."),
    ("CODEYARD_OIDC_URL", "Another OIDC issuer for the browser sign-in."),
    ("CARGO", "The cargo that builds Rust extensions."),
    ("RUSTC", "The rustc cyx asks for its targets."),
];

/// `docs/extensions/reference/cli.md`.
#[cfg(test)]
pub fn cli_reference() -> String {
    let mut out = format!(
        "# cyx\n\n<!-- Generated from crates/cyx/src/main.rs and yard.rs: run `{UPDATE}` after changing them. -->\n\n\
         `cyx` is the command-line tool for extension authors. It ships with Codeyard: **Extensions: Install cyx Command in \
         PATH** in the palette adds it. Everything it checks is checked by the same code the app and the registry run, \
         so a folder `cyx check` accepts installs. It exits with 0 on success and 1 on any error, and prints errors in the \
         [format of the error reference](errors.md).\n\n\
         ## cyx --help\n\n```text\n{}\n```\n\n\
         ## cyx yard help\n\nThe registry operator's commands. Authors do not need them.\n\n```text\n{}\n```\n\n\
         ## Environment\n\n| Variable | What it changes |\n| --- | --- |\n",
        crate::HELP,
        crate::yard::HELP,
    );
    for (name, what) in ENVIRONMENT {
        out.push_str(&format!("| `{name}` | {what} |\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    fn repo() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn check(file: &str, want: &str) {
        let path = repo().join("docs/extensions/reference").join(file);
        if std::env::var_os("UPDATE_DOCS").is_some() {
            std::fs::write(&path, want).unwrap();
        }
        // Windows checkouts may turn line ends into CRLF.
        let have = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
        assert!(have == want, "docs/extensions/reference/{file} is out of date: run `{UPDATE}`");
    }

    #[test]
    fn errors_reference_is_current() {
        check("errors.md", &errors_reference());
    }

    #[test]
    fn cli_reference_is_current() {
        check("cli.md", &cli_reference());
    }

    #[test]
    fn every_code_is_unique_has_a_fix_and_a_range() {
        let all = catalogue();
        for (i, d) in all.iter().enumerate() {
            assert!(d.code.len() == 6 && d.code.starts_with("CY") && d.code[2..].bytes().all(|b| b.is_ascii_digit()), "{}", d.code);
            assert!(all[..i].iter().all(|o| o.code != d.code), "{} is listed twice", d.code);
            assert!(i == 0 || all[i - 1].code < d.code, "{} is out of order", d.code);
            assert!(d.summary.ends_with('.') && !d.fix.is_empty(), "{}: a summary sentence and a fix line", d.code);
            assert!(RANGES.iter().any(|(p, _)| d.code.starts_with(p)), "{}: add its range to RANGES", d.code);
        }
    }

    /// Each code has a heading, so an anchor, in the page (`errors_reference_is_current` keeps the file equal to it).
    #[test]
    fn every_code_has_an_anchor_in_the_error_reference() {
        let page = errors_reference();
        for d in catalogue() {
            // GitHub's anchor for "### CY0201" is #cy0201, the one `cyx explain` prints.
            assert!(page.lines().any(|l| l == format!("### {}", d.code)), "{} has no heading in {ERRORS_DOC}", d.code);
            assert!(explain(d.code).unwrap().ends_with(&format!("{ERRORS_DOC}#{}", anchor(d.code))));
        }
    }

    /// A code written anywhere in the sources (an error, a test, a doc) is one the catalogue explains.
    #[test]
    fn every_code_in_the_sources_is_in_the_catalogue() {
        let known: Vec<&str> = catalogue().iter().map(|d| d.code).collect();
        let mut stack = vec![repo().join("crates"), repo().join("docs"), repo().join("sdk/typescript/src")];
        while let Some(p) = stack.pop() {
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n == "target" || n == "node_modules") {
                    continue;
                }
                stack.extend(std::fs::read_dir(&p).unwrap().flatten().map(|e| e.path()));
                continue;
            }
            if !p.extension().is_some_and(|e| e == "rs" || e == "md" || e == "ts") {
                continue;
            }
            let text = std::fs::read_to_string(&p).unwrap_or_default();
            for (i, _) in text.match_indices("CY0") {
                let Some(code) = text.get(i..i + 6) else { continue };
                if code[2..].bytes().all(|b| b.is_ascii_digit()) {
                    assert!(known.contains(&code), "{}: {code} is not in the error catalogue (explain.rs)", p.display());
                }
            }
        }
    }

    #[test]
    fn explain_prints_the_entry_and_refuses_unknown_codes() {
        let e = explain("cy0201").unwrap();
        assert!(e.starts_with("CY0201: "), "{e}");
        assert!(e.contains("= fix: add the entry to [permissions]"), "{e}");
        assert!(e.ends_with("= docs: docs/extensions/reference/errors.md#cy0201"), "{e}");
        assert!(explain("CY0814").unwrap().contains("not the one the publisher signed"));
        let unknown = explain("CY9999").unwrap_err();
        assert!(unknown.contains("no error code 'CY9999'") && unknown.contains("= fix:"), "{unknown}");
    }

    /// Every variable cyx reads is documented, and every documented one is read.
    #[test]
    fn environment_matches_the_sources() {
        let mut sources = String::new();
        for e in std::fs::read_dir(repo().join("crates/cyx/src")).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.ends_with(".rs") && !name.contains("tests") && name != "explain.rs" {
                sources.push_str(&std::fs::read_to_string(e.path()).unwrap());
            }
        }
        for (name, _) in ENVIRONMENT {
            assert!(sources.contains(&format!("\"{name}\"")), "{name} is documented but cyx does not read it");
        }
        let generic = ["PATH", "HOME", "LOCALAPPDATA", "XDG_CACHE_HOME", "GITHUB_ACTIONS", "ACTIONS_ID_TOKEN_REQUEST_URL", "ACTIONS_ID_TOKEN_REQUEST_TOKEN"];
        for (i, _) in sources.match_indices("var(\"").chain(sources.match_indices("var_os(\"")).chain(sources.match_indices("var(name")) {
            let rest = &sources[i..];
            let Some(name) = rest.split('"').nth(1) else { continue };
            if rest.starts_with("var(name") || generic.contains(&name) {
                continue;
            }
            assert!(ENVIRONMENT.iter().any(|(n, _)| *n == name), "cyx reads {name}: document it in ENVIRONMENT");
        }
    }
}
