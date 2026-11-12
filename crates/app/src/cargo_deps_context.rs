//! What the cursor is on in a `Cargo.toml` that is being typed: a crate name, a version string or a feature name. The
//! document is often not valid TOML at that moment (an open quote, an unfinished array), so this is a small lexer over the text
//! before the cursor, not the parser.

use std::ops::Range;

const DEPS: [&str; 5] = ["dependencies", "dev-dependencies", "build-dependencies", "dev_dependencies", "build_dependencies"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ctx {
    /// A new entry's key in a dependency table: `ser|`.
    CrateName { prefix: String, replace: Range<usize> },
    /// Inside the quotes of a version requirement: `serde = "1.|"`.
    Version { package: String, prefix: String, replace: Range<usize>, closed: bool },
    /// Inside the quotes of one entry of a `features` array.
    Feature { package: String, prefix: String, replace: Range<usize>, closed: bool },
}

/// How a table header places its entries: dependencies as keys, or one dependency as the table itself.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Table {
    Dependencies,
    Dependency(String),
    Other,
}

fn table_of(header: &[String]) -> Table {
    let deps = |s: &String| DEPS.contains(&s.as_str());
    match header {
        [d] if deps(d) => Table::Dependencies,
        [w, d] if w == "workspace" && d == "dependencies" => Table::Dependencies,
        [t, _, d] if t == "target" && deps(d) => Table::Dependencies,
        [d, name] if deps(d) => Table::Dependency(name.clone()),
        [w, d, name] if w == "workspace" && d == "dependencies" => Table::Dependency(name.clone()),
        [t, _, d, name] if t == "target" && deps(d) => Table::Dependency(name.clone()),
        _ => Table::Other,
    }
}

/// A dotted key (`a."b.c".d`) split into its parts.
fn split_key(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let (mut cur, mut quote) = (String::new(), None::<char>);
    for c in text.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => quote = Some(c),
            (None, '.') => parts.push(std::mem::take(&mut cur).trim().to_string()),
            (None, c) => cur.push(c),
        }
    }
    parts.push(cur.trim().to_string());
    parts
}

enum Frame {
    Table { key: String, in_value: bool },
    Array { owner: String },
}

#[derive(PartialEq)]
enum Mode {
    Key,
    Header,
    Value,
}

struct Lexer {
    header: Vec<String>,
    mode: Mode,
    /// The key (or header) being read, and the top-level key of the current statement.
    buf: String,
    top_key: String,
    frames: Vec<Frame>,
    /// Offset of the opening quote of the string the lexer is in, and its quote.
    string: Option<(usize, char)>,
}

impl Lexer {
    fn end_statement(&mut self) {
        self.mode = Mode::Key;
        self.buf.clear();
        self.top_key.clear();
        self.frames.clear();
    }

    fn open_array(&mut self) {
        let owner = match self.frames.last() {
            Some(Frame::Table { key, in_value: true }) => key.clone(),
            // `serde.features = [` : the field is the last part of the dotted key.
            _ => split_key(&self.top_key).pop().unwrap_or_default(),
        };
        self.frames.push(Frame::Array { owner });
    }

    fn step(&mut self, text: &str, i: usize, c: char) {
        if let Some((start, quote)) = self.string {
            let escaped = quote == '"' && text[..i].chars().rev().take_while(|&b| b == '\\').count() % 2 == 1;
            if c == quote && !escaped {
                self.string = None;
                let content = text[start + 1..i].to_string();
                match (&self.mode, self.frames.last_mut()) {
                    (Mode::Key, _) => self.buf.push_str(&format!("{quote}{content}{quote}")),
                    (Mode::Header, _) => self.buf.push_str(&format!("{quote}{content}{quote}")),
                    (Mode::Value, Some(Frame::Table { key, in_value: false })) => *key = format!("{quote}{content}{quote}"),
                    _ => {}
                }
            } else if c == '\n' {
                self.string = None; // an unterminated one-line string ends with its line
            }
            return;
        }
        match (&self.mode, c) {
            (_, '\n') => {
                if self.mode == Mode::Header || self.frames.is_empty() {
                    self.end_statement();
                }
            }
            (Mode::Key, '[') if self.buf.trim().is_empty() => {
                self.mode = Mode::Header;
                self.buf.clear();
            }
            (Mode::Header, '[') if self.buf.is_empty() => {} // `[[array.of.tables]]`
            (Mode::Header, ']') => {
                let name = self.buf.trim().to_string();
                self.header = split_key(&name);
                self.mode = Mode::Key;
                self.buf.clear();
            }
            (Mode::Key, '=') => {
                self.top_key = self.buf.trim().to_string();
                self.mode = Mode::Value;
            }
            (Mode::Key | Mode::Header, '"' | '\'') => self.string = Some((i, c)),
            (Mode::Key | Mode::Header, c) => self.buf.push(c),
            (Mode::Value, '"' | '\'') => self.string = Some((i, c)),
            (Mode::Value, '{') => self.frames.push(Frame::Table { key: String::new(), in_value: false }),
            (Mode::Value, '[') => self.open_array(),
            (Mode::Value, '}' | ']') => {
                self.frames.pop();
            }
            (Mode::Value, '=') => {
                if let Some(Frame::Table { in_value, .. }) = self.frames.last_mut() {
                    *in_value = true;
                }
            }
            (Mode::Value, ',') => {
                if let Some(Frame::Table { key, in_value }) = self.frames.last_mut() {
                    key.clear();
                    *in_value = false;
                }
            }
            (Mode::Value, c) => {
                if let Some(Frame::Table { key, in_value: false }) = self.frames.last_mut()
                    && !c.is_whitespace()
                {
                    key.push(c);
                }
            }
        }
    }
}

/// The kind of dependency table the cursor is in (`[dev-dependencies]`, `[target.'cfg(unix)'.build-dependencies]`, …).
pub fn kind_at(text: &str, offset: usize) -> Option<super::manifest::Kind> {
    use super::manifest::Kind;
    let (lx, _) = lex(text, offset)?;
    let name = lx.header.iter().find(|s| DEPS.contains(&s.as_str()))?;
    Some(if name.starts_with("dev") {
        Kind::Dev
    } else if name.starts_with("build") {
        Kind::Build
    } else {
        Kind::Normal
    })
}

fn lex(text: &str, offset: usize) -> Option<(Lexer, usize)> {
    let offset = offset.min(text.len());
    if !text.is_char_boundary(offset) {
        return None;
    }
    let mut lx = Lexer { header: Vec::new(), mode: Mode::Key, buf: String::new(), top_key: String::new(), frames: Vec::new(), string: None };
    let mut comment = false;
    for (i, c) in text[..offset].char_indices() {
        if comment {
            comment = c != '\n';
            if comment {
                continue;
            }
        }
        if c == '#' && lx.string.is_none() {
            comment = true;
            continue;
        }
        lx.step(text, i, c);
    }
    (!comment).then_some((lx, offset))
}

/// The context at byte `offset` of `text`.
pub fn context_at(text: &str, offset: usize) -> Option<Ctx> {
    let (lx, offset) = lex(text, offset)?;
    let table = table_of(&lx.header);
    let line_end = text[offset..].find('\n').map_or(text.len(), |n| offset + n);
    if let Some((start, quote)) = lx.string {
        let (prefix, closed) = (text[start + 1..offset].to_string(), text[offset..line_end].contains(quote));
        let replace = start + 1..offset;
        if lx.mode != Mode::Value || prefix.contains('\n') {
            return None;
        }
        let key = split_key(&lx.top_key);
        let version = |package: &str| Some(Ctx::Version { package: package.to_string(), prefix: prefix.clone(), replace: replace.clone(), closed });
        let feature = |package: &str| Some(Ctx::Feature { package: package.to_string(), prefix: prefix.clone(), replace: replace.clone(), closed });
        return match (&table, lx.frames.as_slice(), key.as_slice()) {
            (Table::Dependencies, [], [name]) => version(name),
            (Table::Dependencies, [], [name, field]) if field == "version" => version(name),
            (Table::Dependencies, [Frame::Table { key: field, in_value: true }], [name]) if field == "version" => version(name),
            (Table::Dependencies, [Frame::Table { .. }, Frame::Array { owner }], [name]) if owner == "features" => feature(name),
            (Table::Dependency(name), [], [field]) if field == "version" => version(name),
            (Table::Dependency(name), [Frame::Array { owner }], [_]) if owner == "features" => feature(name),
            (Table::Dependencies, [Frame::Array { owner }], [name, _]) if owner == "features" => feature(name),
            _ => None,
        };
    }
    // Not in a string: a new key in a dependency table.
    let word = lx.buf.trim_start();
    if lx.mode == Mode::Key && lx.frames.is_empty() && table == Table::Dependencies && !word.is_empty() {
        let word = word.trim_end();
        if word.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') && text[line_start(text, offset)..offset].trim_start() == word {
            return Some(Ctx::CrateName { prefix: word.to_string(), replace: offset - word.len()..offset });
        }
    }
    None
}

fn line_start(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map_or(0, |n| n + 1)
}
