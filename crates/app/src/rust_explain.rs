//! "Explain error E0xxx": which diagnostics have a rustc error code, and where the explanation comes from: the link the
//! server attached to the diagnostic when there is one, otherwise `rustc --explain` run on the machine that builds.

/// `E0382`: an `E` and four digits.
pub fn error_code(code: &str) -> Option<&str> {
    let code = code.trim();
    (code.len() == 5 && code.starts_with('E') && code[1..].bytes().all(|b| b.is_ascii_digit())).then_some(code)
}

/// The rustc error code of a diagnostic: the server's `code`, or, for what was read from compiler output (a task), the
/// ` [E0382]` the reader put at the end of the message.
pub fn code_of<'a>(code: Option<&'a str>, message: &'a str) -> Option<&'a str> {
    code.and_then(error_code).or_else(|| {
        let first = message.lines().next()?.trim_end();
        let inner = first.strip_suffix(']')?.rsplit_once(" [")?.1;
        error_code(inner)
    })
}

/// How an explanation is shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Explain {
    /// Open the page the server linked (`codeDescription.href`).
    Browse(String),
    /// Run `rustc --explain <code>` and show its Markdown.
    Rustc(String),
}

/// The explanation for a diagnostic with `code` and the server's `href`; `None` when `code` is not a rustc error code
/// (a clippy lint name has its own page behind `href`, but is not explained by rustc).
pub fn plan(code: &str, href: Option<&str>) -> Option<Explain> {
    let code = error_code(code)?;
    match href.filter(|h| h.starts_with("https://") || h.starts_with("http://")) {
        Some(link) => Some(Explain::Browse(link.to_string())),
        None => Some(Explain::Rustc(code.to_string())),
    }
}

/// What `rustc --explain` printed, as a page: the title names the code, the text is already Markdown.
pub fn explanation_page(code: &str, output: &str) -> String {
    super::views::markdown_document(&crate::i18n::tf("Error code {}", &[&code]), output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_rustc_error_codes_are_explained() {
        assert_eq!(error_code("E0382"), Some("E0382"));
        assert_eq!(error_code(" E0308 "), Some("E0308"));
        for no in ["", "E382", "E03821", "e0382", "unused_variables", "clippy::needless_return", "E0382; id", "E03é2"] {
            assert_eq!(error_code(no), None, "{no:?}");
        }
    }

    #[test]
    fn the_code_comes_from_the_server_or_from_the_end_of_a_tasks_message() {
        assert_eq!(code_of(Some("E0308"), "mismatched types"), Some("E0308"));
        assert_eq!(code_of(None, "mismatched types [E0308]"), Some("E0308"), "a problem read from `cargo check` output");
        assert_eq!(code_of(None, "mismatched types [E0308]\nexpected due to this"), Some("E0308"));
        assert_eq!(code_of(Some("clippy::x"), "unused [E0001]"), Some("E0001"));
        for none in ["unused variable [unused_variables]", "array [E0123 is open", "x [E012]", "", "[E0382]"] {
            assert_eq!(code_of(None, none), None, "{none:?}");
        }
    }

    #[test]
    fn the_servers_link_is_used_when_it_has_one() {
        // The shape rust-analyzer 0.3.3113 sent for E0308.
        let href = "https://doc.rust-lang.org/error-index.html#E0308";
        assert_eq!(plan("E0308", Some(href)), Some(Explain::Browse(href.into())));
        assert_eq!(plan("E0308", None), Some(Explain::Rustc("E0308".into())));
        assert_eq!(plan("E0308", Some("javascript:alert(1)")), Some(Explain::Rustc("E0308".into())), "only web links are opened");
        assert_eq!(plan("E0308", Some("file:///etc/passwd")), Some(Explain::Rustc("E0308".into())));
        assert_eq!(plan("clippy::needless_return", Some("https://rust-lang.github.io/rust-clippy/")), None);
    }

    #[test]
    fn the_real_explanation_becomes_a_page_with_its_code_in_the_title() {
        let output = std::fs::read_to_string(format!("{}/testdata/rustc-explain-E0382.md", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let page = explanation_page("E0382", &output);
        assert!(page.starts_with("# Error code E0382\n\nA variable was used after its contents have been moved elsewhere."), "{page}");
        assert!(page.contains("```\nstruct MyStruct"), "the examples keep their fences");
        assert!(page.ends_with("\n") && !page.ends_with("\n\n"));
    }
}
