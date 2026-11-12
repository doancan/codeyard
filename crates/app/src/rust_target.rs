//! Cross-compilation: the `rust.target` triple, the targets installed for the toolchain, and the choices *Rust: Select
//! Target…* offers. Reading the tools' output only; running them is the workbench's job.

/// Longest triple accepted (`x86_64-unknown-linux-gnu` is 24; custom target files have short names).
const MAX_TRIPLE: usize = 64;

/// Whether `text` can be a target triple (or the name of a custom target): letters, digits and `_ - .` only. A value that
/// is not goes nowhere near a command line.
pub fn is_valid_triple(text: &str) -> bool {
    !text.is_empty() && text.len() <= MAX_TRIPLE && !text.starts_with('-') && text.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// The triples `rustup target list --installed` printed, one per line, in order and without repeats.
pub fn parse_installed(output: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in output.lines().map(str::trim).filter(|l| is_valid_triple(l)) {
        if !out.iter().any(|t| t == line) {
            out.push(line.to_string());
        }
    }
    out
}

/// The `host:` triple of `rustc -vV`.
pub fn parse_host(rustc_version: &str) -> Option<String> {
    rustc_version.lines().find_map(|l| l.strip_prefix("host:")).map(str::trim).filter(|t| is_valid_triple(t)).map(str::to_string)
}

/// One row of the target list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// `None`: no `--target`, Cargo builds for the machine it runs on.
    pub triple: Option<String>,
    pub current: bool,
}

/// The rows: the default first, then every installed target; a configured target that is not installed stays in the list
/// (it is what is set) so it can be seen and replaced.
pub fn choices(installed: &[String], current: Option<&str>) -> Vec<Choice> {
    let mut rows = vec![Choice { triple: None, current: current.is_none() }];
    let mut seen: Vec<&str> = installed.iter().map(String::as_str).collect();
    if let Some(current) = current.filter(|c| !installed.iter().any(|t| t == c)) {
        seen.insert(0, current);
    }
    rows.extend(seen.into_iter().map(|t| Choice { triple: Some(t.to_string()), current: current == Some(t) }));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triples_are_checked_before_they_reach_a_command_line() {
        for ok in ["aarch64-apple-darwin", "x86_64-unknown-linux-musl", "wasm32-wasip1", "thumbv7em-none-eabihf", "my_target.json"] {
            assert!(is_valid_triple(ok), "{ok}");
        }
        for bad in ["", "-C", "--target", "a b", "a;b", "$(id)", "a/b", "ğ", &"x".repeat(65)] {
            assert!(!is_valid_triple(bad), "{bad:?}");
        }
    }

    #[test]
    fn rustup_output_is_one_triple_per_line() {
        let output = "aarch64-apple-darwin\nx86_64-unknown-linux-musl\n\nwasm32-wasip1 (installed)\n  aarch64-apple-darwin  \ninfo: oops\n";
        assert_eq!(parse_installed(output), ["aarch64-apple-darwin", "x86_64-unknown-linux-musl"]);
        assert!(parse_installed("").is_empty());
    }

    #[test]
    fn the_host_comes_from_rustc_version_output() {
        let real = "rustc 1.99.1 (abcdef 2026-11-01) (Homebrew)\nbinary: rustc\ncommit-hash: abcdef\nhost: aarch64-apple-darwin\nrelease: 1.99.1\nLLVM version: 21.1.0\n";
        assert_eq!(parse_host(real).as_deref(), Some("aarch64-apple-darwin"));
        assert_eq!(parse_host("rustc 1.0.0\n"), None);
    }

    #[test]
    fn the_list_starts_with_the_default_and_marks_the_current_target() {
        let installed = vec!["aarch64-apple-darwin".to_string(), "wasm32-wasip1".to_string()];
        let rows = choices(&installed, Some("wasm32-wasip1"));
        assert_eq!(
            rows,
            [
                Choice { triple: None, current: false },
                Choice { triple: Some("aarch64-apple-darwin".into()), current: false },
                Choice { triple: Some("wasm32-wasip1".into()), current: true },
            ]
        );
        assert!(choices(&installed, None)[0].current);
        let stale = choices(&installed, Some("riscv64gc-unknown-linux-gnu"));
        assert_eq!(
            stale[1],
            Choice { triple: Some("riscv64gc-unknown-linux-gnu".into()), current: true },
            "a set target that is not installed is still listed"
        );
    }
}
