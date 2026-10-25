//! Structural search: a pattern is code in the target language, matched against the syntax tree instead of the text
//! (ast-grep style). `$X` stands for one node, `$$$X` for zero or more sibling nodes, and a name used twice must match
//! identical text both times. Names are upper case (`$X`, `$ARGS_1`); a name starting with `_` binds nothing.
//!
//! Most grammars reject `$` in an identifier, so the pattern is rewritten with placeholder identifiers before it is
//! parsed. Matching walks the tree of the searched file and compares node kinds, and leaf text exactly. Comments are
//! ignored on both sides unless the pattern itself contains one.
//!
//! This module only needs the grammars the crate already links; it never runs anything from the files it reads.

use std::borrow::Cow;
use std::ops::Range;

use tree_sitter::{Node, Parser, Tree};

use crate::{Language, language_for_path};

/// A language patterns can be written in, with the file extensions it covers. Several grammars may serve one entry
/// (TypeScript and TSX): a pattern compiled for the entry is matched by node kind, which both grammars share.
pub struct Family {
    pub id: &'static str,
    pub extensions: &'static [&'static str],
}

pub const FAMILIES: &[Family] = &[
    Family { id: "Rust", extensions: &["rs"] },
    Family { id: "TypeScript", extensions: &["ts", "mts", "cts", "tsx"] },
    Family { id: "JavaScript", extensions: &["js", "mjs", "cjs", "jsx"] },
    Family { id: "Python", extensions: &["py", "pyi"] },
    Family { id: "Go", extensions: &["go"] },
    Family { id: "Java", extensions: &["java"] },
    Family { id: "C", extensions: &["c", "h"] },
    Family { id: "C++", extensions: &["cc", "cpp", "cxx", "hh", "hpp", "hxx", "ipp"] },
    Family { id: "C#", extensions: &["cs"] },
    Family { id: "Ruby", extensions: &["rb"] },
    Family { id: "PHP", extensions: &["php"] },
    Family { id: "Kotlin", extensions: &["kt", "kts"] },
    Family { id: "Swift", extensions: &["swift"] },
    Family { id: "Lua", extensions: &["lua"] },
    Family { id: "Shell", extensions: &["sh", "bash", "zsh"] },
];

/// The family a file belongs to, by extension.
pub fn family_for_path(path: &str) -> Option<&'static Family> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    FAMILIES.iter().find(|f| f.extensions.contains(&ext.as_str()))
}

pub fn family(id: &str) -> Option<&'static Family> {
    FAMILIES.iter().find(|f| f.id == id)
}

impl Family {
    /// Grammar for patterns of this family.
    pub fn language(&self) -> &'static Language {
        language_for_path(&format!("pattern.{}", self.extensions[0])).expect("every family has a grammar")
    }

    /// Grammar for a file of this family (`None` if the file belongs to another one).
    pub fn language_of(&self, path: &str) -> Option<&'static Language> {
        family_for_path(path).filter(|f| f.id == self.id)?;
        language_for_path(path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternErrorKind {
    Empty,
    /// The parser found something it could not use.
    Syntax,
    /// The parser would need this token to finish the pattern.
    Missing(String),
    /// Several statements, or an expression and a statement: a pattern is one node.
    NotOneNode,
    /// `$X` alone matches every node.
    OnlyMetavariable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternError {
    pub kind: PatternErrorKind,
    /// Bytes of the pattern the problem is at (never empty unless the pattern is).
    pub span: Range<usize>,
}

/// One metavariable of a pattern.
struct Var {
    /// Without the `$`; empty for the anonymous `$_` / `$$$`, which bind nothing.
    name: String,
    multi: bool,
    /// As typed in the pattern, to put it back where it stood inside a string or comment.
    written: String,
}

pub struct Pattern {
    lang: &'static Language,
    /// Wrapped, placeholder-rewritten source the tree was parsed from.
    src: String,
    tree: Tree,
    /// Child indices from the root of `tree` to the node the pattern stands for.
    path: Vec<usize>,
    vars: Vec<Var>,
    keep_comments: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capture {
    pub name: String,
    /// Bytes of the searched text. Empty for a `$$$X` that matched nothing (its position is not meaningful).
    pub range: Range<usize>,
    pub text: String,
    pub multi: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub range: Range<usize>,
    pub captures: Vec<Capture>,
}

/// The text could not be parsed without errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseFailed;

const PLACEHOLDER: &str = "__cymv";
/// Upper bound on matcher steps for one candidate node: `$$$` can backtrack, and a pattern must never hang the search.
const STEP_LIMIT: u32 = 20_000;

/// Node kinds that only wrap the construct a pattern means: dropped while the pattern has exactly one child.
const WRAPPERS: &[&str] = &["source_file", "program", "module", "translation_unit", "compilation_unit", "chunk", "expression_statement", "statement_list"];

/// Nodes that stand for a whole file or block of statements: a pattern that ends up as one of these has several statements.
const CONTAINERS: &[&str] = &["source_file", "program", "module", "translation_unit", "compilation_unit", "chunk", "statement_list"];

/// Contexts to try when the bare pattern does not parse: a statement needs a function around it in Rust or Go, a type
/// needs a `type` item. (prefix, suffix); the pattern is found again inside by its byte range.
fn contexts(family: &str) -> &'static [(&'static str, &'static str)] {
    match family {
        "Rust" => &[("", ""), ("fn __w() {\n", "\n}"), ("type __w = ", ";"), ("impl __w {\n", "\n}")],
        "TypeScript" => &[("", ""), ("function __w() {\n", "\n}"), ("type __w = ", ";"), ("class __w {\n", "\n}")],
        "JavaScript" => &[("", ""), ("function __w() {\n", "\n}"), ("class __w {\n", "\n}")],
        "Go" => &[("", ""), ("package p\nfunc __w() {\n", "\n}"), ("package p\n", "")],
        "Java" | "C#" => &[("class W {\n", "\n}"), ("class W { void w() {\n", ";\n} }")],
        "C" | "C++" => &[("", ""), ("void __w(void) {\n", ";\n}")],
        "PHP" => &[("<?php\n", ""), ("<?php\n", ";")],
        "Kotlin" => &[("", ""), ("fun w() {\n", "\n}")],
        _ => &[("", "")],
    }
}

fn family_of(lang: &Language) -> &'static str {
    match lang.name {
        "JavaScript JSX" => "JavaScript",
        "TypeScript JSX" => "TypeScript",
        n => FAMILIES.iter().map(|f| f.id).find(|id| *id == n).unwrap_or(""),
    }
}

fn is_comment(n: Node) -> bool {
    n.kind().contains("comment")
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_uppercase() || c == '_'
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'
}

/// Replace metavariables by placeholder identifiers. Returns the new text, the variables, and for every placeholder the
/// (rewritten range, original range) pair so error spans can be mapped back.
fn rewrite(pattern: &str) -> (String, Vec<Var>, Vec<(Range<usize>, Range<usize>)>) {
    let mut out = String::with_capacity(pattern.len() + 16);
    let mut vars: Vec<Var> = Vec::new();
    let mut spans = Vec::new();
    let mut prev: Option<char> = None;
    let mut rest = pattern;
    let mut at = 0;
    while let Some(c) = rest.chars().next() {
        let after_word = prev.is_some_and(|p| p.is_alphanumeric() || p == '_' || p == '$');
        let parsed = if c == '$' && !after_word {
            let dollars = rest.bytes().take_while(|b| *b == b'$').count();
            let multi = dollars == 3;
            let tail = &rest[dollars..];
            let name_len = if tail.starts_with(is_name_start) { tail.find(|c| !is_name_char(c)).unwrap_or(tail.len()) } else { 0 };
            // `$$$` alone is anonymous; `$` alone, `$$` and `$$$$` are plain text (a dollar is a valid character in JavaScript, Bash, PHP).
            match (dollars, name_len) {
                (1, n) if n > 0 => Some((1 + n, false, &tail[..n])),
                (3, n) => Some((3 + n, multi, &tail[..n])),
                _ => None,
            }
        } else {
            None
        };
        match parsed {
            Some((len, multi, name)) => {
                let anonymous = name.is_empty() || name.starts_with('_');
                let name = if anonymous { "" } else { name };
                let existing = (!anonymous).then(|| vars.iter().position(|v| v.name == name && v.multi == multi)).flatten();
                let ix = existing.unwrap_or_else(|| {
                    vars.push(Var { name: name.to_string(), multi, written: rest[..len].to_string() });
                    vars.len() - 1
                });
                let start = out.len();
                out.push_str(&format!("{PLACEHOLDER}{ix}__"));
                spans.push((start..out.len(), at..at + len));
                at += len;
                rest = &rest[len..];
                prev = Some('_');
            }
            None => {
                out.push(c);
                at += c.len_utf8();
                rest = &rest[c.len_utf8()..];
                prev = Some(c);
            }
        }
    }
    (out, vars, spans)
}

/// Position in the rewritten text -> position in what the user typed.
fn to_original(pos: usize, end: bool, spans: &[(Range<usize>, Range<usize>)], original_len: usize) -> usize {
    let mut delta = 0isize;
    for (rw, orig) in spans {
        if pos >= rw.end {
            delta = orig.end as isize - rw.end as isize;
        } else if pos > rw.start || (pos == rw.start && !end) {
            return if end { orig.end } else { orig.start };
        } else {
            break;
        }
    }
    ((pos as isize + delta).max(0) as usize).min(original_len)
}

/// The first problem the parser reported, as bytes of the parsed text.
fn first_error(root: Node, src: &str) -> (PatternErrorKind, Range<usize>) {
    fn walk<'t>(n: Node<'t>) -> Option<Node<'t>> {
        if n.is_missing() || n.is_error() {
            // The innermost first error says more than the ERROR node around it.
            let mut c = n.walk();
            let inner = n.children(&mut c).find(|k| k.has_error() || k.is_missing()).and_then(walk);
            return Some(inner.unwrap_or(n));
        }
        if !n.has_error() {
            return None;
        }
        let mut c = n.walk();
        n.children(&mut c).find_map(walk)
    }
    let Some(n) = walk(root) else { return (PatternErrorKind::Syntax, 0..src.len().min(1)) };
    let r = n.byte_range();
    // One character is enough to point at; an ERROR node can span the whole rest of the pattern.
    let end = if r.is_empty() || n.is_error() { (r.start + src[r.start..].chars().next().map_or(1, char::len_utf8)).min(src.len()) } else { r.end };
    let kind = if n.is_missing() { PatternErrorKind::Missing(n.kind().to_string()) } else { PatternErrorKind::Syntax };
    (kind, r.start..end.max(r.start))
}

fn parse(lang: &Language, text: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    parser.set_language(&lang.ts).ok()?;
    parser.parse(text, None)
}

/// Walk down from `root` to the node covering exactly `target`, recording child indices.
fn locate(root: Node, target: &Range<usize>) -> Option<Vec<usize>> {
    let mut node = root;
    let mut path = Vec::new();
    loop {
        if node.byte_range() == *target {
            return Some(path);
        }
        let ix =
            (0..node.child_count() as usize).find(|&i| node.child(i as u32).is_some_and(|k| k.start_byte() <= target.start && target.end <= k.end_byte()))?;
        node = node.child(ix as u32)?;
        path.push(ix);
    }
}

/// Drop wrapper nodes that have a single real child (`program` > `expression_statement` > the call).
fn unwrap_path(root: Node, path: &mut Vec<usize>, keep_comments: bool) {
    loop {
        let mut node = root;
        for &i in path.iter() {
            node = node.child(i as u32).expect("path comes from this tree");
        }
        if !WRAPPERS.contains(&node.kind()) {
            return;
        }
        let kids: Vec<usize> = (0..node.child_count() as usize).filter(|&i| node.child(i as u32).is_some_and(|k| keep_comments || !is_comment(k))).collect();
        match kids.as_slice() {
            [only] if node.child(*only as u32).is_some_and(|k| k.is_named()) => path.push(*only),
            _ => return,
        }
    }
}

fn contains_comment(n: Node) -> bool {
    is_comment(n) || (0..n.child_count() as usize).any(|i| n.child(i as u32).is_some_and(contains_comment))
}

/// Compile a pattern written in `lang`. Fast enough to run on every keystroke.
pub fn compile(lang: &'static Language, pattern: &str) -> Result<Pattern, PatternError> {
    let trimmed = pattern.trim();
    if trimmed.is_empty() {
        return Err(PatternError { kind: PatternErrorKind::Empty, span: 0..pattern.len().min(1) });
    }
    let (rewritten, vars, spans) = rewrite(pattern);
    let lead = rewritten.len() - rewritten.trim_start().len();
    let body = lead..rewritten.trim_end().len();
    let mut first_failure = None;
    let mut not_one_node = false;
    for (prefix, suffix) in contexts(family_of(lang)) {
        let src = format!("{prefix}{rewritten}{suffix}");
        let Some(tree) = parse(lang, &src) else { continue };
        let root = tree.root_node();
        let target = prefix.len() + body.start..prefix.len() + body.end;
        if root.has_error() {
            if first_failure.is_none() && prefix.is_empty() {
                let (kind, span) = first_error(root, &src);
                first_failure = Some((kind, span));
            }
            continue;
        }
        let Some(mut path) = locate(root, &target) else {
            not_one_node = true;
            continue;
        };
        let keep_comments = contains_comment(root_at(root, &path));
        unwrap_path(root, &mut path, keep_comments);
        let node = root_at(root, &path);
        // A file-level node that did not unwrap holds several statements.
        if CONTAINERS.contains(&node.kind()) {
            not_one_node = true;
            continue;
        }
        let src_of = |n: Node| &src[n.byte_range()];
        let only_var = src_of(node).strip_prefix(PLACEHOLDER).and_then(|t| t.strip_suffix("__")).is_some_and(|d| d.parse::<usize>().is_ok());
        if only_var {
            return Err(PatternError { kind: PatternErrorKind::OnlyMetavariable, span: 0..pattern.len() });
        }
        return Ok(Pattern { lang, src, tree, path, vars, keep_comments });
    }
    let (kind, span) =
        if not_one_node { (PatternErrorKind::NotOneNode, body.clone()) } else { first_failure.unwrap_or((PatternErrorKind::Syntax, body.clone())) };
    let map = |p: usize, end| match kind {
        PatternErrorKind::NotOneNode => p,
        _ => to_original(p, end, &spans, pattern.len()),
    };
    let (a, b) = (map(span.start, false), map(span.end, true));
    let span = if kind == PatternErrorKind::NotOneNode { 0..pattern.len() } else { a.min(pattern.len())..b.max(a).min(pattern.len()) };
    Err(PatternError { kind, span })
}

fn root_at<'t>(root: Node<'t>, path: &[usize]) -> Node<'t> {
    path.iter().fold(root, |n, &i| n.child(i as u32).expect("path comes from this tree"))
}

impl Pattern {
    pub fn language(&self) -> &'static Language {
        self.lang
    }

    /// Names of the metavariables that bind text (what a replacement template may use), in order of first use.
    pub fn variables(&self) -> Vec<&str> {
        self.vars.iter().filter(|v| !v.name.is_empty()).map(|v| v.name.as_str()).collect()
    }

    fn root(&self) -> Node<'_> {
        root_at(self.tree.root_node(), &self.path)
    }

    /// If `n` is just a metavariable, its index.
    fn var_of(&self, n: Node) -> Option<usize> {
        let r = n.byte_range();
        if r.len() > 24 {
            return None;
        }
        self.src[r].strip_prefix(PLACEHOLDER)?.strip_suffix("__")?.parse().ok()
    }

    /// Pattern text of a leaf with the metavariables written the way the user typed them.
    fn leaf_text(&self, n: Node) -> Cow<'_, str> {
        let t = &self.src[n.byte_range()];
        if !t.contains(PLACEHOLDER) {
            return Cow::Borrowed(t);
        }
        let mut out = String::new();
        let mut rest = t;
        while let Some(at) = rest.find(PLACEHOLDER) {
            out.push_str(&rest[..at]);
            let tail = &rest[at + PLACEHOLDER.len()..];
            let digits = tail.bytes().take_while(u8::is_ascii_digit).count();
            match (tail[..digits].parse::<usize>(), tail[digits..].starts_with("__")) {
                (Ok(i), true) if i < self.vars.len() => {
                    out.push_str(&self.vars[i].written);
                    rest = &tail[digits + 2..];
                }
                _ => {
                    out.push_str(PLACEHOLDER);
                    rest = tail;
                }
            }
        }
        out.push_str(rest);
        Cow::Owned(out)
    }

    /// A piece of text every match has to contain, for narrowing a file search before parsing: the longest
    /// identifier-like token of the pattern (or, failing that, the longest token). Sound because all tokens must
    /// appear in a match, so the longest one alone already rules files out.
    pub fn literal_hint(&self) -> Option<String> {
        fn leaves<'t>(n: Node<'t>, keep_comments: bool, out: &mut Vec<Node<'t>>) {
            if !keep_comments && is_comment(n) {
                return;
            }
            if n.child_count() == 0 {
                out.push(n);
            }
            for i in 0..n.child_count() as usize {
                if let Some(k) = n.child(i as u32) {
                    leaves(k, keep_comments, out);
                }
            }
        }
        let mut all = Vec::new();
        leaves(self.root(), self.keep_comments, &mut all);
        let usable: Vec<(bool, String)> = all
            .into_iter()
            .filter(|n| !n.is_missing())
            .map(|n| (n, &self.src[n.byte_range()]))
            .filter(|(_, t)| !t.is_empty() && !t.contains(PLACEHOLDER) && !t.contains(['\n', '\r']))
            .map(|(n, t)| (n.kind().contains("identifier") || t.chars().all(|c| c.is_alphanumeric() || c == '_'), t.to_string()))
            .collect();
        let best = |words_only: bool| usable.iter().filter(|(w, _)| *w || !words_only).max_by_key(|(_, t)| t.chars().count()).map(|(_, t)| t.clone());
        best(true).or_else(|| best(false))
    }
}

struct Run<'a> {
    pat: &'a Pattern,
    text: &'a str,
    binds: Vec<Option<Range<usize>>>,
    trail: Vec<usize>,
    steps: u32,
}

/// A `;` that may or may not be written (JavaScript's automatic semicolons, Rust's final expression).
fn is_semicolon(n: Node) -> bool {
    !n.is_named() && n.kind() == ";"
}

impl<'a> Run<'a> {
    fn undo(&mut self, mark: usize) {
        while self.trail.len() > mark {
            let v = self.trail.pop().expect("length checked");
            self.binds[v] = None;
        }
    }

    fn bind(&mut self, v: usize, range: Range<usize>) -> bool {
        if self.pat.vars[v].name.is_empty() {
            return true;
        }
        match &self.binds[v] {
            Some(prev) => self.text[prev.clone()] == self.text[range],
            None => {
                self.binds[v] = Some(range);
                self.trail.push(v);
                true
            }
        }
    }

    fn kids<'t>(&self, n: Node<'t>) -> Vec<Node<'t>> {
        let mut c = n.walk();
        n.children(&mut c).filter(|k| self.pat.keep_comments || !is_comment(*k)).collect()
    }

    fn node(&mut self, p: Node, t: Node) -> bool {
        self.steps += 1;
        if self.steps > STEP_LIMIT {
            return false;
        }
        if let Some(v) = self.pat.var_of(p) {
            return t.is_named() && self.bind(v, t.byte_range());
        }
        if p.kind() != t.kind() {
            return false;
        }
        if p.child_count() == 0 || t.child_count() == 0 {
            return p.child_count() == t.child_count() && self.pat.leaf_text(p) == self.text[t.byte_range()];
        }
        let (pk, tk) = (self.kids(p), self.kids(t));
        self.seq(&pk, &tk)
    }

    /// Match the children of a pattern node against the children of a text node; `$$$X` takes as many siblings as it needs.
    fn seq(&mut self, mut pc: &[Node], mut tc: &[Node]) -> bool {
        loop {
            let Some((p, prest)) = pc.split_first() else { return tc.iter().all(|t| is_semicolon(*t)) };
            if let Some(v) = self.pat.var_of(*p).filter(|v| self.pat.vars[*v].multi) {
                // Fewest nodes first, so `f($$$A, $B)` binds the last argument to `$B`.
                for k in 0..=tc.len() {
                    if self.steps > STEP_LIMIT {
                        return false;
                    }
                    self.steps += 1;
                    let mark = self.trail.len();
                    let range = match (tc.first(), k) {
                        (Some(first), k) if k > 0 => first.start_byte()..tc[k - 1].end_byte(),
                        (Some(first), _) => first.start_byte()..first.start_byte(),
                        (None, _) => 0..0,
                    };
                    if self.bind(v, range) && self.seq(prest, &tc[k..]) {
                        return true;
                    }
                    self.undo(mark);
                }
                return false;
            }
            let Some((t, trest)) = tc.split_first() else { return pc.iter().all(|p| is_semicolon(*p)) };
            if !self.node(*p, *t) {
                return false;
            }
            (pc, tc) = (prest, trest);
        }
    }
}

/// All matches of `pattern` in `text`, parsed with the pattern's own grammar; `Err` if the text has syntax errors.
pub fn find_all_in(pattern: &Pattern, lang: &Language, text: &str) -> Result<Vec<Match>, ParseFailed> {
    let tree = parse(lang, text).ok_or(ParseFailed)?;
    if tree.root_node().has_error() {
        return Err(ParseFailed);
    }
    let root = pattern.root();
    let mut run = Run { pat: pattern, text, binds: vec![None; pattern.vars.len()], trail: Vec::new(), steps: 0 };
    let mut out = Vec::new();
    let mut cursor = tree.walk();
    // Pre-order over the whole tree: an outer match comes before the matches inside it.
    'walk: loop {
        let t = cursor.node();
        if t.is_named() && t.kind() == root.kind() && (pattern.keep_comments || !is_comment(t)) {
            run.binds.iter_mut().for_each(|b| *b = None);
            run.trail.clear();
            run.steps = 0;
            if run.node(root, t) {
                let captures = pattern
                    .vars
                    .iter()
                    .zip(&run.binds)
                    .filter(|(v, _)| !v.name.is_empty())
                    .filter_map(|(v, b)| {
                        let range = b.clone()?;
                        Some(Capture { name: v.name.clone(), text: text[range.clone()].to_string(), range, multi: v.multi })
                    })
                    .collect();
                out.push(Match { range: t.byte_range(), captures });
            }
        }
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                continue 'walk;
            }
            if !cursor.goto_parent() {
                break 'walk;
            }
        }
    }
    Ok(out)
}

/// All matches in `text`, parsed with the pattern's grammar. Text with syntax errors yields nothing.
pub fn find_all(pattern: &Pattern, text: &str) -> Vec<Match> {
    find_all_in(pattern, pattern.lang, text).unwrap_or_default()
}

/// Matches that do not lie inside an earlier one, in text order. Replacing a match changes its inner matches along
/// with it, so only the outermost are safe to apply together; the rest are found again on the next run.
pub fn outermost(matches: &[Match]) -> Vec<&Match> {
    let mut out: Vec<&Match> = Vec::new();
    for m in matches {
        if out.last().is_none_or(|last| m.range.start >= last.range.end) {
            out.push(m);
        }
    }
    out
}

/// Fill a replacement template: `$X` and `$$$X` become the captured text. A `$name` that was not captured stays as it
/// is, so code with dollars in it (template strings, shell variables) can be written without escaping.
pub fn expand(template: &str, captures: &[Capture]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let dollars = tail.bytes().take_while(|b| *b == b'$').count();
        let after = &tail[dollars..];
        let name_len = if after.starts_with(is_name_start) { after.find(|c| !is_name_char(c)).unwrap_or(after.len()) } else { 0 };
        // `$X` also fills in for a `$$$X` (and the other way round) when the name is unambiguous: people mix them up.
        let found = match dollars {
            1 | 3 if name_len > 0 => {
                let name = &after[..name_len];
                captures.iter().find(|c| c.name == name && c.multi == (dollars == 3)).or_else(|| captures.iter().find(|c| c.name == name))
            }
            _ => None,
        };
        match found {
            Some(c) => {
                out.push_str(&c.text);
                rest = &after[name_len..];
            }
            None => {
                // Keep the dollars; the name after them is looked at again only when they are a single `$`.
                out.push_str(&tail[..dollars]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Does the text parse without errors? Used to warn when a replacement would leave a file broken.
pub fn parses_cleanly(lang: &Language, text: &str) -> bool {
    parse(lang, text).is_some_and(|t| !t.root_node().has_error())
}

#[cfg(test)]
mod tests;
