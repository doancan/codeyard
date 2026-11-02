//! The incremental tree against a full parse: random edits, reparsed on the spot, over budget in the background,
//! or with more edits made while a background parse runs, must always end where a parse from scratch would. And a
//! background parse that lands asks for a frame only when something shown changes color.
//!
//! There are no injected languages (every file is parsed with one grammar), so the languages whose text nests most
//! (HTML with a script, Markdown with fenced code) stand in for them.

use std::ops::Range;
use std::time::Duration;

use ropey::Rope;

use super::{Edit, ParseJob, Pos, Syntax, language_for_path};

/// xorshift64*: the same sequence on every run and platform.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn point(rope: &Rope, byte: usize) -> Pos {
    let line = rope.byte_to_line(byte);
    (line, byte - rope.line_to_byte(line))
}

/// Replace `bytes` (on character boundaries) with `text` in the rope and in the tree.
fn splice(s: &mut Syntax, rope: &mut Rope, bytes: Range<usize>, text: &str) {
    let (start, old_end) = (point(rope, bytes.start), point(rope, bytes.end));
    let chars = rope.byte_to_char(bytes.start)..rope.byte_to_char(bytes.end);
    rope.remove(chars.clone());
    rope.insert(chars.start, text);
    let new_end_byte = bytes.start + text.len();
    s.edit(&Edit { start_byte: bytes.start, old_end_byte: bytes.end, new_end_byte, start, old_end, new_end: point(rope, new_end_byte) });
}

/// A random edit: an insertion, a deletion or a replacement, at character boundaries.
fn random_edit(rng: &mut Rng, s: &mut Syntax, rope: &mut Rope, pieces: &[&str]) {
    let n = rope.len_chars();
    let a = rng.below(n + 1);
    let b = match rng.below(3) {
        0 => a,
        _ => (a + rng.below(12)).min(n),
    };
    let text = if a < b && rng.below(3) == 0 { "" } else { pieces[rng.below(pieces.len())] };
    let bytes = rope.char_to_byte(a)..rope.char_to_byte(b);
    splice(s, rope, bytes, text);
}

/// Land a background parse, and every follow-up job it asks for, until the tree is settled.
fn settle(s: &mut Syntax, rope: &Rope, mut job: Option<ParseJob>) {
    while let Some(j) = job.take() {
        job = s.finish(j.run(), rope).and_then(|landed| landed.next);
    }
    assert!(!s.parsing());
}

/// The tree and the colors of the whole text equal those of a parse from scratch.
fn assert_full_parse(s: &Syntax, rope: &Rope, path: &str, step: usize) {
    let full = Syntax::new(language_for_path(path).unwrap(), rope).unwrap();
    let all = 0..rope.len_bytes();
    assert_eq!(s.tree.as_ref().unwrap().root_node().to_sexp(), full.tree.as_ref().unwrap().root_node().to_sexp(), "{path} tree after step {step}:\n{rope}");
    assert_eq!(s.highlights(rope, all.clone()), full.highlights(rope, all), "{path} colors after step {step}:\n{rope}");
}

const RUST: &[&str] = &[
    "fn ",
    "x",
    " ",
    "\n",
    "{",
    "}",
    "(",
    ")",
    "\"",
    "'",
    "//",
    "/*",
    "*/",
    "let ",
    "= 1;",
    "ğ",
    "struct S;",
    "#[derive(Debug)]",
    "r#\"",
    "::",
    "<T>",
    "0x1f",
    "\\n",
    "ab ",
    "impl S { fn f(&self) {} }\n",
];
const PYTHON: &[&str] = &["def ", "f", "(", ")", ":", "\n    ", "\"\"\"", "#", "'", "class ", "return ", "ğ", " ", "\n", "x = 1\n"];
const JS: &[&str] = &["function ", "x", "{", "}", "`", "${", "//", "/*", "*/", "'", "const ", "=>", " ", "\n", "ğ", "a(b, c);\n"];
const HTML: &[&str] = &["<div>", "</div>", "<script>", "</script>", "let a = 1;", "<!--", "-->", "\"", "=", " ", "\n", "ğ", "<p class=\"x\">"];
const MARKDOWN: &[&str] = &["# ", "```", "rust\n", "\n", "*", "_", "[a](b)", "`", "- ", "ğ", " ", "> ", "fn main() {}\n"];

const SOURCES: &[(&str, &str, &[&str])] = &[
    (
        "a.rs",
        "/// Doc.\npub fn handler(req: &Request) -> Result<Response, Error> {\n    let id = req.param(\"id\")?;\n    Ok(Response::json(&id))\n}\n\n",
        RUST,
    ),
    ("a.py", "class A:\n    \"\"\"Doc.\"\"\"\n    def f(self, x):\n        return x + 1  # one\n\n", PYTHON),
    ("a.js", "// c\nexport function f(a) {\n  const s = `x${a}`;\n  return s.length;\n}\n\n", JS),
    ("a.html", "<div class=\"a\">\n  <script>let x = 'y';</script>\n  <!-- c -->\n</div>\n", HTML),
    ("a.md", "# Title\n\nText with `code`.\n\n```rust\nfn x() {}\n```\n\n- item\n", MARKDOWN),
];

/// Edits reparsed on the spot, edits whose reparse goes to the background, and edits made while it runs: after every
/// settled step, the tree and the colors equal those of a full parse.
#[test]
fn random_edits_end_where_a_full_parse_does() {
    for (k, (path, item, pieces)) in SOURCES.iter().enumerate() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ (k as u64 + 1));
        let mut rope = Rope::from_str(&item.repeat(40));
        let mut s = Syntax::new(language_for_path(path).unwrap(), &rope).unwrap();
        let mut job: Option<ParseJob> = None;
        let (mut jobs, mut edits_during_jobs) = (0, 0);
        for step in 0..300 {
            // One to three edits (several cursors, a paste over a selection, an undo of a group).
            for _ in 0..1 + rng.below(3) {
                random_edit(&mut rng, &mut s, &mut rope, pieces);
                edits_during_jobs += usize::from(job.is_some());
            }
            match rng.below(4) {
                // A reparse that fits its budget.
                0 => assert!(s.reparse_within(&rope, Duration::from_secs(60)).is_none()),
                // One that does not, while no other runs: it waits; more edits may come before it lands.
                1 if job.is_none() => {
                    job = s.reparse_within(&rope, Duration::ZERO);
                    jobs += usize::from(job.is_some());
                }
                // A background parse lands; its follow-up may wait for the next turn.
                2 => {
                    if let Some(j) = job.take() {
                        job = s.finish(j.run(), &rope).and_then(|landed| landed.next);
                    }
                }
                _ => {}
            }
            if step % 25 == 24 {
                if job.is_none() {
                    job = s.reparse_within(&rope, Duration::from_secs(60));
                }
                settle(&mut s, &rope, job.take());
                assert_full_parse(&s, &rope, path, step);
            }
        }
        settle(&mut s, &rope, job.take());
        assert_full_parse(&s, &rope, path, 300);
        assert!(jobs > 5 && edits_during_jobs > 5, "{path}: {jobs} background parses, {edits_during_jobs} edits while one ran");
    }
}

/// 2000 Rust functions.
fn big_rust() -> Rope {
    Rope::from_str(&SOURCES[0].1.repeat(2000))
}

/// Over budget in the background, with the given text inserted at byte `at`.
fn over_budget(s: &mut Syntax, rope: &mut Rope, at: usize, text: &str) -> ParseJob {
    splice(s, rope, at..at, text);
    s.reparse_within(rope, Duration::ZERO).expect("over budget")
}

/// The last 60 lines.
fn bottom(rope: &Rope) -> Range<usize> {
    rope.line_to_byte(rope.len_lines() - 61)..rope.len_bytes()
}

/// A parse that lands with the same colors for what was shown asks for no frame, and what was shown stays valid for
/// the new tree; nor does one when nothing was drawn since the edit.
#[test]
fn a_parse_that_changes_nothing_shown_asks_for_no_frame() {
    let mut rope = big_rust();
    let mut s = Syntax::new(language_for_path("a.rs").unwrap(), &rope).unwrap();
    s.highlights_shown(&rope, 0..100);
    // `fn` typed before the first item: the reparse recolors the top, not the end of the file, which is in view.
    let job = over_budget(&mut s, &mut rope, 0, "fn ");
    assert!(!s.is_shown(), "an edit leaves nothing shown with this tree");
    s.highlights_shown(&rope, bottom(&rope));
    assert!(s.is_shown());
    let landed = s.finish(job.run(), &rope).expect("its own job");
    assert!(!landed.repaint, "an edit at the top of the file changed the colors of its end");
    assert!(landed.next.is_none());
    assert!(s.is_shown(), "the end is still shown as this tree colors it");
    // Stray words at the top: the whole file is parsed again, and nothing changes color.
    let job = over_budget(&mut s, &mut rope, 0, "ab ab ");
    s.highlights_shown(&rope, 0..rope.line_to_byte(60));
    assert!(!s.finish(job.run(), &rope).expect("its own job").repaint);
    // Nothing drawn since the edit: nothing to compare.
    let job = over_budget(&mut s, &mut rope, 0, "fn ");
    assert!(!s.finish(job.run(), &rope).expect("its own job").repaint);
    s.reparse(&rope);
    assert_full_parse(&s, &rope, "a.rs", 0);
}

/// A parse that lands with other colors for something shown asks for a frame, and ends as a full parse.
#[test]
fn a_parse_that_recolors_what_is_shown_asks_for_a_frame() {
    let mut rope = big_rust();
    let mut s = Syntax::new(language_for_path("a.rs").unwrap(), &rope).unwrap();
    let job = over_budget(&mut s, &mut rope, 0, "fn ");
    let top = 0..rope.line_to_byte(60);
    s.highlights_shown(&rope, top.clone());
    // Typed while the parse runs: the follow-up job reparses it.
    splice(&mut s, &mut rope, 3..3, "f");
    s.highlights_shown(&rope, bottom(&rope));
    s.highlights_shown(&rope, top);
    let landed = s.finish(job.run(), &rope).expect("its own job");
    assert!(landed.repaint, "the item after `fn` is colored otherwise");
    assert!(!s.is_shown(), "what was shown has other colors");
    assert!(s.parsing(), "the key typed meanwhile is still to reparse");
    settle(&mut s, &rope, landed.next);
    assert_full_parse(&s, &rope, "a.rs", 0);
}

/// Shown ranges are kept apart for a few views, then merged into one that covers them all.
#[test]
fn shown_ranges_stay_few() {
    let rope = big_rust();
    let s = Syntax::new(language_for_path("a.rs").unwrap(), &rope).unwrap();
    let mut drawn = Vec::new();
    for i in 0..20 {
        let a = rope.line_to_byte(i * 100);
        s.highlights_shown(&rope, a..a + 10);
        drawn.push(a..a + 10);
        let shown = s.shown.borrow();
        assert!(shown.len() <= super::SHOWN_MAX, "{shown:?}");
        assert!(drawn.iter().all(|d| shown.iter().any(|r| r.start <= d.start && d.end <= r.end)), "{shown:?}");
    }
}
