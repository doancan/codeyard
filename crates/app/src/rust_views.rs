//! Read-only views of what rust-analyzer returns as text (HIR, MIR, the syntax tree, the server report, `rustc --explain`):
//! each becomes a Markdown document that opens in the preview, so nothing can be typed into it.

use crate::i18n::tf;

/// Most characters shown from one answer. A syntax tree of a big file is megabytes of JSON; the view keeps the start.
pub const MAX_VIEW_CHARS: usize = 400_000;

/// `text` as a fenced block that the text itself cannot close: the fence is longer than any run of backticks inside it.
pub fn fenced(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let fence = "`".repeat((longest + 1).max(3));
    format!("{fence}text\n{}\n{fence}\n", text.trim_end_matches('\n'))
}

/// The text cut to `MAX_VIEW_CHARS`, with a last line saying so.
fn clipped(text: &str) -> String {
    match text.char_indices().nth(MAX_VIEW_CHARS) {
        Some((end, _)) => format!("{}\n{}", &text[..end], tf("… (cut after {} characters)", &[&MAX_VIEW_CHARS])),
        None => text.to_string(),
    }
}

/// A document with a title and the text of one answer.
pub fn text_document(title: &str, text: &str) -> String {
    format!("# {}\n\n{}", title.replace('\n', " "), fenced(&clipped(text)))
}

/// A document with a title and Markdown the caller already wrote (the compiler's explanation of an error code).
pub fn markdown_document(title: &str, markdown: &str) -> String {
    format!("# {}\n\n{}\n", title.replace('\n', " "), clipped(markdown).trim_end())
}

/// rust-analyzer's syntax tree (a JSON tree of nodes and tokens with offsets) as the indented listing people read:
/// `KIND@start..end` per node, with the token's text after it. `None` when `answer` is not that JSON (an older server sends
/// the listing itself, which is shown as it is).
pub fn syntax_tree_listing(answer: &str, source: &str) -> Option<String> {
    let tree: serde_json::Value = serde_json::from_str(answer).ok()?;
    tree["kind"].as_str()?;
    let mut out = String::new();
    list_node(&tree, source, 0, &mut out);
    Some(out)
}

fn list_node(node: &serde_json::Value, source: &str, depth: usize, out: &mut String) {
    // Past the cap the listing is cut anyway (`MAX_VIEW_CHARS`); stop reading the tree.
    if out.len() > MAX_VIEW_CHARS * 4 {
        return;
    }
    let offset = |key: &str| node[key][0].as_u64().map(|n| n as usize);
    let (Some(kind), Some(start), Some(end)) = (node["kind"].as_str(), offset("start"), offset("end")) else { return };
    out.push_str(&format!("{}{kind}@{start}..{end}", "  ".repeat(depth)));
    if node["type"] == "Token"
        && let Some(text) = source.get(start..end)
    {
        out.push_str(&format!(" {text:?}"));
    }
    out.push('\n');
    for child in node["children"].as_array().map(Vec::as_slice).unwrap_or_default() {
        list_node(child, source, depth + 1, out);
    }
}

/// The scratch file name for a view: `HIR of lib.rs.md`; the file name keeps a path out of it.
pub fn view_file_name(what: &str, file: &str) -> String {
    let file = file.rsplit(['/', '\\']).next().unwrap_or(file);
    format!("{what} of {file}.md")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fence_cannot_be_closed_by_the_text() {
        assert_eq!(fenced("fn a() {}\n"), "```text\nfn a() {}\n```\n");
        let tricky = "a\n```\nb\n`````\nc";
        let out = fenced(tricky);
        assert!(out.starts_with("``````text\n") && out.ends_with("\n``````\n"), "{out}");
        assert_eq!(fenced(""), "```text\n\n```\n");
    }

    #[test]
    fn a_huge_answer_is_cut_on_a_character_boundary_and_says_so() {
        let text = "ğ".repeat(MAX_VIEW_CHARS + 10);
        let doc = text_document("HIR of lib.rs", &text);
        assert!(doc.starts_with("# HIR of lib.rs\n\n```text\n"));
        assert!(doc.contains("… (cut after 400000 characters)"));
        assert!(doc.chars().count() < MAX_VIEW_CHARS + 200);
        let whole = text_document("T", "short");
        assert!(!whole.contains("cut after"));
    }

    #[test]
    fn file_names_of_views_have_no_path() {
        assert_eq!(view_file_name("HIR", "/w/crates/a/src/lib.rs"), "HIR of lib.rs.md");
        assert_eq!(view_file_name("MIR", "C:\\w\\src\\main.rs"), "MIR of main.rs.md");
    }

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("{}/testdata/rust-analyzer/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    #[test]
    fn the_real_syntax_tree_is_listed_with_token_text() {
        let answer: String = serde_json::from_str(&fixture("syntax-tree.json")).unwrap();
        let listing = syntax_tree_listing(&answer, &fixture("syntax-tree-source.rs")).unwrap();
        let head: Vec<&str> = listing.lines().take(9).collect();
        assert_eq!(
            head,
            [
                "SOURCE_FILE@0..116",
                "  FN@0..50",
                "    FN_KW@0..2 \"fn\"",
                "    WHITESPACE@2..3 \" \"",
                "    NAME@3..7",
                "      IDENT@3..7 \"main\"",
                "    PARAM_LIST@7..9",
                "      L_PAREN@7..8 \"(\"",
                "      R_PAREN@8..9 \")\"",
            ]
        );
        assert!(listing.contains("WHITESPACE@11..16 \"\\n    \""), "line breaks are escaped: {listing}");
        assert!(listing.contains("ATTR@") || listing.contains("MODULE@"), "the test module is in it");
    }

    #[test]
    fn a_server_that_answers_with_text_or_a_hostile_tree_gets_no_listing() {
        assert_eq!(syntax_tree_listing("SOURCE_FILE@0..3\n  FN@0..3\n", ""), None);
        assert_eq!(syntax_tree_listing("{\"nope\": 1}", ""), None);
        // Offsets that do not fit the text or a character boundary lose the token text, not the tree.
        let odd = r#"{"type":"Node","kind":"A","start":[0,0,0],"end":[9,0,9],"children":[{"type":"Token","kind":"T","start":[0,0,0],"end":[1,0,1]},{"type":"Token","kind":"U","start":[0,0,0],"end":[99,0,99]}]}"#;
        assert_eq!(syntax_tree_listing(odd, "ğ").unwrap(), "A@0..9\n  T@0..1\n  U@0..99\n");
        let nested =
            |levels: usize| format!("{}{}", r#"{"type":"Node","kind":"N","start":[0,0,0],"end":[0,0,0],"children":["#.repeat(levels), "]}".repeat(levels));
        assert_eq!(syntax_tree_listing(&nested(30), "").unwrap().lines().count(), 30);
        // JSON nested beyond the parser's limit is refused instead of overflowing the stack.
        assert_eq!(syntax_tree_listing(&nested(100_000), ""), None);
    }

    #[test]
    fn a_title_cannot_start_a_second_block() {
        assert_eq!(markdown_document("a\nb", "body\n\n"), "# a b\n\nbody\n");
    }
}
