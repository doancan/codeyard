//! Data lens: read JSON, YAML, TOML and .env into one value model (`serde_json::Value`, keys in file order), write it
//! back in any of them, find the path under the cursor and run queries. Pure functions on text: nothing here touches
//! the file system or the network, and nothing runs code from the document.
//!
//! Conversion goes through `Value`, so what a format cannot hold is decided in one place: TOML has no null, .env holds
//! flat text, YAML comments and aliases are gone after reading. A conversion that would have to guess (null into
//! TOML, a nested value into .env) fails and says where; one that only drops comments goes ahead and reports a note.

use std::fmt;

use schema::walk::Seg;
use serde::Serialize as _;
use serde_json::ser::{PrettyFormatter, Serializer};
use serde_json::{Map, Number, Value};

use crate::i18n::{t, tf};

pub use crate::datalens_query::{QueryError, parse as parse_query};

/// Files above this size are not queried (parsing and evaluating them takes seconds and several times their size in memory).
pub const MAX_QUERY_BYTES: usize = 20 << 20;
/// A query result longer than this is cut at a line break and the caller says so.
pub const MAX_RESULT_BYTES: usize = 5 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Json,
    Yaml,
    Toml,
    Env,
}

impl Format {
    pub const ALL: [Format; 4] = [Format::Json, Format::Yaml, Format::Toml, Format::Env];

    pub fn from_path(path: &str) -> Option<Format> {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path).to_ascii_lowercase();
        if name == ".env" || name.starts_with(".env.") || name.ends_with(".env") {
            return Some(Format::Env);
        }
        match name.rsplit_once('.')?.1 {
            "json" | "jsonc" | "geojson" => Some(Format::Json),
            "yaml" | "yml" => Some(Format::Yaml),
            "toml" => Some(Format::Toml),
            _ => None,
        }
    }

    /// Format names are proper names and stay as they are.
    pub fn label(self) -> &'static str {
        match self {
            Format::Json => "JSON",
            Format::Yaml => "YAML",
            Format::Toml => "TOML",
            Format::Env => ".env",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Json => "json",
            Format::Yaml => "yaml",
            Format::Toml => "toml",
            Format::Env => "env",
        }
    }
}

/// Something the conversion changed or dropped without failing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Note {
    CommentsLost,
    AliasesExpanded,
    /// A multi-document YAML file became a JSON-style array of its documents.
    Documents(usize),
    DatesAsText,
    /// Numbers and booleans written to .env become text.
    EnvText,
}

impl Note {
    pub fn text(self) -> String {
        match self {
            Note::CommentsLost => t("Comments are not kept.").to_string(),
            Note::AliasesExpanded => t("Anchors and aliases were expanded.").to_string(),
            Note::Documents(n) => tf("{} YAML documents became one list.", &[&n]),
            Note::DatesAsText => t("TOML dates were turned into text.").to_string(),
            Note::EnvText => t("Numbers and booleans were written as text.").to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DataError {
    pub message: String,
    /// 1-based line and column in the source text, when the error points at one.
    pub at: Option<(usize, usize)>,
}

impl DataError {
    fn plain(message: String) -> Self {
        DataError { message, at: None }
    }

    fn at(text: &str, offset: usize, message: String) -> Self {
        DataError { message, at: Some(line_col(text, offset)) }
    }
}

impl fmt::Display for DataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.at {
            Some((line, col)) => f.write_str(&tf("{0} (line {1}, column {2})", &[&self.message, &line, &col])),
            None => f.write_str(&self.message),
        }
    }
}

/// 1-based line and column (in characters) of a byte offset.
pub fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let mut end = offset.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let before = &text[..end];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, col)
}

pub struct Loaded {
    pub value: Value,
    pub notes: Vec<Note>,
}

// --- Reading ---

/// Read `text` as `format`, or work out the format when it is not known (a selection in a Markdown file, an unsaved scratch).
pub fn parse_any(text: &str, format: Option<Format>) -> Result<Loaded, DataError> {
    match format {
        Some(f) => parse(text, f),
        None => parse(text, sniff(text)),
    }
}

/// JSON when the text opens a brace or bracket, TOML when it reads as TOML, otherwise YAML (which also accepts most plain text).
pub fn sniff(text: &str) -> Format {
    let t = text.trim_start_matches('\u{feff}').trim_start();
    if t.starts_with(['{', '[']) {
        Format::Json
    } else if t.contains('=') && toml::from_str::<toml::Table>(text).is_ok_and(|v| !v.is_empty()) {
        Format::Toml
    } else {
        Format::Yaml
    }
}

pub fn parse(text: &str, format: Format) -> Result<Loaded, DataError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut notes = Vec::new();
    let value = match format {
        Format::Json => parse_json(text, &mut notes)?,
        Format::Yaml => parse_yaml(text, &mut notes)?,
        Format::Toml => parse_toml(text, &mut notes)?,
        Format::Env => parse_env(text, &mut notes)?,
    };
    Ok(Loaded { value, notes })
}

/// A parsed document that refuses a repeated key, which serde_json would silently resolve to the last one.
struct Strict(Value);

impl<'de> serde::Deserialize<'de> for Strict {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Strict;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON value")
            }

            fn visit_bool<E>(self, v: bool) -> Result<Strict, E> {
                Ok(Strict(Value::Bool(v)))
            }

            fn visit_i64<E>(self, v: i64) -> Result<Strict, E> {
                Ok(Strict(Value::from(v)))
            }

            fn visit_u64<E>(self, v: u64) -> Result<Strict, E> {
                Ok(Strict(Value::from(v)))
            }

            fn visit_f64<E>(self, v: f64) -> Result<Strict, E> {
                Ok(Strict(Number::from_f64(v).map_or(Value::Null, Value::Number)))
            }

            fn visit_str<E>(self, v: &str) -> Result<Strict, E> {
                Ok(Strict(Value::String(v.to_string())))
            }

            fn visit_string<E>(self, v: String) -> Result<Strict, E> {
                Ok(Strict(Value::String(v)))
            }

            fn visit_unit<E>(self) -> Result<Strict, E> {
                Ok(Strict(Value::Null))
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Strict, A::Error> {
                let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(4096));
                while let Some(Strict(v)) = seq.next_element()? {
                    items.push(v);
                }
                Ok(Strict(Value::Array(items)))
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Strict, A::Error> {
                let mut out = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if out.contains_key(&key) {
                        return Err(serde::de::Error::custom(t("Duplicate key")));
                    }
                    let Strict(v) = map.next_value()?;
                    out.insert(key, v);
                }
                Ok(Strict(Value::Object(out)))
            }
        }
        d.deserialize_any(V)
    }
}

fn parse_json(text: &str, notes: &mut Vec<Note>) -> Result<Value, DataError> {
    // The strict pass is the fast one; it also fails on comments, trailing commas and repeated keys, and the second
    // pass then tells those apart.
    if let Ok(Strict(v)) = serde_json::from_str::<Strict>(text) {
        return Ok(v);
    }
    // Comments and trailing commas (settings.jsonc, tsconfig.json) are common in files with a .json name.
    match schema::doc::parse_json(text) {
        Ok(p) => {
            if let Some(&at) = p.duplicates.first() {
                return Err(DataError::at(text, at, t("Duplicate key").to_string()));
            }
            notes.push(Note::CommentsLost);
            Ok(p.value)
        }
        // The tolerant parser knows what a JSONC file may contain, so its complaint is the more useful one.
        Err(lenient) => Err(DataError::at(text, lenient.offset, lenient.message)),
    }
}

fn parse_yaml(text: &str, notes: &mut Vec<Note>) -> Result<Value, DataError> {
    let mut docs = Vec::new();
    for (_, parsed) in schema::doc::parse_yaml(text) {
        let doc = parsed.map_err(|e| DataError::at(text, e.offset, e.message))?;
        if let Some(&at) = doc.duplicates.first() {
            return Err(DataError::at(text, at, t("Duplicate key").to_string()));
        }
        docs.push(doc.value);
    }
    let (comments, aliases) = scan_yaml_markers(text);
    if comments {
        notes.push(Note::CommentsLost);
    }
    if aliases {
        notes.push(Note::AliasesExpanded);
    }
    Ok(match docs.len() {
        0 => Value::Null,
        1 => docs.remove(0),
        n => {
            notes.push(Note::Documents(n));
            Value::Array(docs)
        }
    })
}

/// Whether a text has `#` comments and `*alias` uses. Quoted text is skipped, and a `#` only starts a comment at the
/// start of a line or after a space, as in YAML, TOML and .env. A heuristic for the notes, not a parser.
fn scan_yaml_markers(text: &str) -> (bool, bool) {
    let (mut comments, mut aliases) = (false, false);
    for line in text.lines() {
        let (code, comment) = split_comment(line);
        comments |= comment;
        aliases |= code
            .split([' ', '[', ']', '{', '}', ','])
            .any(|w| w.strip_prefix('*').and_then(|r| r.chars().next()).is_some_and(|c| c.is_alphanumeric() || c == '_'));
    }
    (comments, aliases)
}

/// The part of a line before a comment (quoted text blanked out), and whether there was a comment.
fn split_comment(line: &str) -> (String, bool) {
    let mut code = String::with_capacity(line.len());
    let (mut quote, mut prev) = (None, ' ');
    for c in line.chars() {
        match quote {
            Some(q) => {
                if c == q && prev != '\\' {
                    quote = None;
                }
                code.push(' ');
            }
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                code.push(' ');
            }
            None if c == '#' && (prev == ' ' || prev == '\t' || code.is_empty()) => return (code, true),
            None => code.push(c),
        }
        prev = c;
    }
    (code, false)
}

fn parse_toml(text: &str, notes: &mut Vec<Note>) -> Result<Value, DataError> {
    let table = toml::from_str::<toml::Table>(text).map_err(|e| match e.span() {
        Some(span) => DataError::at(text, span.start, e.message().to_string()),
        None => DataError::plain(e.message().to_string()),
    })?;
    if text.lines().any(|l| split_comment(l).1) {
        notes.push(Note::CommentsLost);
    }
    let mut dates = false;
    let value = from_toml(toml::Value::Table(table), &mut dates)?;
    if dates {
        notes.push(Note::DatesAsText);
    }
    Ok(value)
}

fn from_toml(v: toml::Value, dates: &mut bool) -> Result<Value, DataError> {
    Ok(match v {
        toml::Value::String(s) => Value::String(s),
        toml::Value::Integer(i) => Value::from(i),
        toml::Value::Float(f) => match Number::from_f64(f) {
            Some(n) => Value::Number(n),
            None => return Err(DataError::plain(tf("The number {} cannot be written in JSON", &[&f]))),
        },
        toml::Value::Boolean(b) => Value::Bool(b),
        toml::Value::Datetime(d) => {
            *dates = true;
            Value::String(d.to_string())
        }
        toml::Value::Array(a) => Value::Array(a.into_iter().map(|v| from_toml(v, dates)).collect::<Result<_, _>>()?),
        toml::Value::Table(t) => {
            let mut m = Map::new();
            for (k, v) in t {
                m.insert(k, from_toml(v, dates)?);
            }
            Value::Object(m)
        }
    })
}

/// `KEY=value` lines: `export` prefix, `#` comments, single quotes (literal), double quotes (`\n`, `\"`, `\\` and
/// friends, may span lines) and bare values (trailing ` # comment` removed). Everything is text.
fn parse_env(text: &str, notes: &mut Vec<Note>) -> Result<Value, DataError> {
    let mut out = Map::new();
    let mut lines = text.lines().enumerate();
    let mut comments = false;
    while let Some((n, raw)) = lines.next() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            comments = true;
            continue;
        }
        let line = line.strip_prefix("export ").map_or(line, str::trim_start);
        let bad = |message: String| DataError { message, at: Some((n + 1, 1)) };
        let Some((key, rest)) = line.split_once('=') else {
            return Err(bad(t("Expected KEY=value").to_string()));
        };
        let key = key.trim();
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')) {
            return Err(bad(tf("“{}” is not a valid variable name", &[&key])));
        }
        let rest = rest.trim_start();
        let value = match rest.chars().next() {
            Some('"') => {
                let mut body = rest[1..].to_string();
                // A double-quoted value may continue on the following lines until the closing quote.
                loop {
                    match find_closing_quote(&body) {
                        Some(end) => {
                            let after = body[end + 1..].trim();
                            if !(after.is_empty() || after.starts_with('#')) {
                                return Err(bad(t("Unexpected text after the closing quote").to_string()));
                            }
                            comments |= after.starts_with('#');
                            break unescape_env(&body[..end]);
                        }
                        None => match lines.next() {
                            Some((_, more)) => {
                                body.push('\n');
                                body.push_str(more);
                            }
                            None => return Err(bad(t("The quoted value is not closed").to_string())),
                        },
                    }
                }
            }
            Some('\'') => {
                let Some(end) = rest[1..].find('\'') else { return Err(bad(t("The quoted value is not closed").to_string())) };
                let after = rest[end + 2..].trim();
                if !(after.is_empty() || after.starts_with('#')) {
                    return Err(bad(t("Unexpected text after the closing quote").to_string()));
                }
                comments |= after.starts_with('#');
                rest[1..end + 1].to_string()
            }
            _ => {
                let (code, comment) = match rest.find(" #").or_else(|| rest.find("\t#")) {
                    Some(i) => (&rest[..i], true),
                    None => (rest, false),
                };
                comments |= comment;
                code.trim_end().to_string()
            }
        };
        if out.insert(key.to_string(), Value::String(value)).is_some() {
            return Err(bad(tf("Duplicate key “{}”", &[&key])));
        }
    }
    if comments {
        notes.push(Note::CommentsLost);
    }
    Ok(Value::Object(out))
}

fn find_closing_quote(s: &str) -> Option<usize> {
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        match c {
            '\\' if !escaped => escaped = true,
            '"' if !escaped => return Some(i),
            _ => escaped = false,
        }
    }
    None
}

fn unescape_env(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some(other @ ('"' | '\\' | '$' | '\'')) => out.push(other),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

// --- Writing ---

#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Spaces per level in JSON and YAML (TOML and .env have a fixed layout).
    pub indent: usize,
    pub sort_keys: bool,
    /// .env: write nested values as `A__B__0=…` instead of refusing them.
    pub flatten_env: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { indent: 2, sort_keys: false, flatten_env: false }
    }
}

pub struct Emitted {
    pub text: String,
    pub notes: Vec<Note>,
}

pub fn emit(value: &Value, to: Format, opts: &Options) -> Result<Emitted, DataError> {
    let sorted;
    let value = if opts.sort_keys {
        sorted = sort_keys(value);
        &sorted
    } else {
        value
    };
    let mut notes = Vec::new();
    let text = match to {
        Format::Json => emit_json(value, opts.indent)?,
        Format::Yaml => emit_yaml(value, opts.indent),
        Format::Toml => emit_toml(value)?,
        Format::Env => emit_env(value, opts.flatten_env, &mut notes)?,
    };
    Ok(Emitted { text, notes })
}

fn sort_keys(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            Value::Object(keys.into_iter().map(|k| (k.clone(), sort_keys(&m[k]))).collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(sort_keys).collect()),
        other => other.clone(),
    }
}

pub fn emit_json(value: &Value, indent: usize) -> Result<String, DataError> {
    let pad = " ".repeat(indent.clamp(1, 8));
    let mut buf = Vec::new();
    let mut ser = Serializer::with_formatter(&mut buf, PrettyFormatter::with_indent(pad.as_bytes()));
    value.serialize(&mut ser).map_err(|e| DataError::plain(e.to_string()))?;
    let mut text = String::from_utf8(buf).map_err(|e| DataError::plain(e.to_string()))?;
    text.push('\n');
    Ok(text)
}

fn emit_yaml(value: &Value, indent: usize) -> String {
    let mut out = String::new();
    let step = indent.clamp(1, 8);
    match value {
        Value::Object(m) if !m.is_empty() => yaml_block(&mut out, value, 0, false, step),
        Value::Array(a) if !a.is_empty() => yaml_block(&mut out, value, 0, false, step),
        other => {
            out.push_str(&yaml_scalar(other));
            out.push('\n');
        }
    }
    out
}

/// A non-empty mapping or sequence as lines starting at column `col`. `inline_first`: the cursor is already past a
/// `- ` and the first line continues it.
fn yaml_block(out: &mut String, v: &Value, col: usize, inline_first: bool, step: usize) {
    let pad = |out: &mut String, n: usize| out.extend(std::iter::repeat_n(' ', n));
    match v {
        Value::Object(m) => {
            for (i, (k, val)) in m.iter().enumerate() {
                if !(inline_first && i == 0) {
                    pad(out, col);
                }
                out.push_str(&yaml_string(k));
                out.push(':');
                yaml_after_key(out, val, col, step);
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                if !(inline_first && i == 0) {
                    pad(out, col);
                }
                out.push('-');
                match item {
                    Value::Object(m) if !m.is_empty() => {
                        out.push(' ');
                        yaml_block(out, item, col + 2, true, step);
                    }
                    Value::Array(a) if !a.is_empty() => {
                        out.push(' ');
                        yaml_block(out, item, col + 2, true, step);
                    }
                    scalar => {
                        out.push(' ');
                        out.push_str(&yaml_scalar(scalar));
                        out.push('\n');
                    }
                }
            }
        }
        _ => {}
    }
}

fn yaml_after_key(out: &mut String, val: &Value, col: usize, step: usize) {
    match val {
        Value::Object(m) if !m.is_empty() => {
            out.push('\n');
            yaml_block(out, val, col + step, false, step);
        }
        Value::Array(a) if !a.is_empty() => {
            out.push('\n');
            yaml_block(out, val, col + step, false, step);
        }
        scalar => {
            out.push(' ');
            out.push_str(&yaml_scalar(scalar));
            out.push('\n');
        }
    }
}

fn yaml_scalar(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => yaml_string(s),
        Value::Array(_) => "[]".into(),
        Value::Object(_) => "{}".into(),
    }
}

fn yaml_string(s: &str) -> String {
    if yaml_plain_ok(s) { s.to_string() } else { yaml_quote(s) }
}

/// Whether a string can be written without quotes and still read back as the same string by YAML 1.1 and 1.2 readers.
fn yaml_plain_ok(s: &str) -> bool {
    let Some(first) = s.chars().next() else { return false };
    if s != s.trim() || s.chars().any(needs_escape) {
        return false;
    }
    if "-?:,[]{}#&*!|>'\"%@`".contains(first) || s.contains(": ") || s.contains(" #") || s.ends_with(':') {
        return false;
    }
    // Numbers, dates and versions look alike to readers that differ on what a number is: quote everything that starts like one.
    let second = s.chars().nth(1);
    if first.is_ascii_digit() || (matches!(first, '+' | '.') && second.is_some_and(|c| c.is_ascii_digit() || c == '.' || first == '.')) {
        return false;
    }
    if s.starts_with("---") || s.starts_with("...") || s == "=" || s == "<<" {
        return false;
    }
    // Words that YAML 1.1 reads as booleans or null, and the ones 1.2 does.
    if matches!(s.to_ascii_lowercase().as_str(), "y" | "n" | "yes" | "no" | "on" | "off" | "true" | "false" | "null" | "~") {
        return false;
    }
    matches!(schema::doc::plain_scalar(s), Value::String(ref x) if x == s)
}

fn needs_escape(c: char) -> bool {
    c.is_control() || matches!(c, '\u{2028}' | '\u{2029}' | '\u{feff}')
}

fn yaml_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if needs_escape(c) => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn kind_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "integer",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "table",
    }
}

fn emit_toml(value: &Value) -> Result<String, DataError> {
    let Value::Object(_) = value else {
        return Err(DataError::plain(tf("TOML needs a table at the top level, but this document is a {}", &[&kind_name(value)])));
    };
    let mut path = Vec::new();
    let table = to_toml(value, &mut path)?;
    toml::to_string(&table).map_err(|e| DataError::plain(e.to_string()))
}

fn to_toml(v: &Value, path: &mut Vec<Seg>) -> Result<toml::Value, DataError> {
    Ok(match v {
        Value::Null => return Err(DataError::plain(tf("{}: TOML has no null; give it a value or remove it", &[&format_path(path)]))),
        Value::Bool(b) => toml::Value::Boolean(*b),
        Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => toml::Value::Integer(i),
            (None, _) if n.is_u64() => {
                return Err(DataError::plain(tf("{0}: {1} does not fit in a TOML integer (64 bits, signed)", &[&format_path(path), &n])));
            }
            (None, Some(f)) => toml::Value::Float(f),
            (None, None) => return Err(DataError::plain(tf("{0}: {1} cannot be written in TOML", &[&format_path(path), &n]))),
        },
        Value::String(s) => toml::Value::String(s.clone()),
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                path.push(Seg::Index(i));
                out.push(to_toml(item, path)?);
                path.pop();
            }
            // A TOML array holds one kind of value; mixing is refused instead of being turned into text.
            if let Some(first) = items.first()
                && let Some(other) = items.iter().find(|x| kind_name(x) != kind_name(first))
            {
                return Err(DataError::plain(tf(
                    "{0}: a TOML array holds one kind of value, but this one has {1} and {2}",
                    &[&format_path(path), &kind_name(first), &kind_name(other)],
                )));
            }
            toml::Value::Array(out)
        }
        Value::Object(m) => {
            let mut out = toml::Table::new();
            for (k, item) in m {
                path.push(Seg::Key(k.clone()));
                out.insert(k.clone(), to_toml(item, path)?);
                path.pop();
            }
            toml::Value::Table(out)
        }
    })
}

fn emit_env(value: &Value, flatten: bool, notes: &mut Vec<Note>) -> Result<String, DataError> {
    let Value::Object(m) = value else {
        return Err(DataError::plain(tf(".env needs a flat set of keys and values, but this document is a {}", &[&kind_name(value)])));
    };
    let mut lines = Vec::new();
    let mut converted = false;
    let mut path = Vec::new();
    for (k, v) in m {
        path.push(Seg::Key(k.clone()));
        env_entries(k.clone(), v, flatten, &mut path, &mut lines, &mut converted)?;
        path.pop();
    }
    if converted {
        notes.push(Note::EnvText);
    }
    let mut text = lines.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    Ok(text)
}

fn env_entries(name: String, v: &Value, flatten: bool, path: &mut Vec<Seg>, lines: &mut Vec<String>, converted: &mut bool) -> Result<(), DataError> {
    let text = match v {
        Value::String(s) => s.clone(),
        Value::Bool(_) | Value::Number(_) => {
            *converted = true;
            v.to_string()
        }
        Value::Null => return Err(DataError::plain(tf("{}: .env has no null; give it a value or remove it", &[&format_path(path)]))),
        Value::Array(_) | Value::Object(_) if !flatten => {
            return Err(DataError::plain(tf("{}: .env holds only text; nested values need the “joined with __” choice", &[&format_path(path)])));
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                path.push(Seg::Index(i));
                env_entries(format!("{name}__{i}"), item, flatten, path, lines, converted)?;
                path.pop();
            }
            return Ok(());
        }
        Value::Object(m) => {
            for (k, item) in m {
                path.push(Seg::Key(k.clone()));
                env_entries(format!("{name}__{k}"), item, flatten, path, lines, converted)?;
                path.pop();
            }
            return Ok(());
        }
    };
    let mut chars = name.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !valid {
        return Err(DataError::plain(tf("{0}: “{1}” is not a name .env files can hold (letters, digits and _)", &[&format_path(path), &name])));
    }
    lines.push(format!("{name}={}", env_quote(&text)));
    Ok(())
}

/// Bare when that reads back unchanged, single quotes when possible (nothing is expanded or unescaped in them),
/// double quotes with escapes otherwise.
fn env_quote(s: &str) -> String {
    let plain = s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '/' | ':' | '@' | ',' | '+' | '%'));
    if plain {
        return s.to_string();
    }
    if !s.contains(['\'', '\n', '\r']) {
        return format!("'{s}'");
    }
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '$' => out.push_str("\\$"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// --- Paths ---

/// `.servers[2].host`: keys that are plain identifiers after a dot, anything else as `."name"`.
pub fn format_path(path: &[Seg]) -> String {
    if path.is_empty() {
        return ".".into();
    }
    let mut out = String::new();
    for seg in path {
        match seg {
            Seg::Index(i) => out.push_str(&format!("[{i}]")),
            Seg::Key(k) => {
                let ident = k.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                if ident {
                    out.push('.');
                    out.push_str(k);
                } else {
                    out.push('.');
                    out.push_str(&serde_json::to_string(k).unwrap_or_default());
                }
            }
        }
    }
    out
}

/// Path of the value or key under byte `offset`, for a JSON, YAML or TOML text.
pub fn path_at(text: &str, format: Format, offset: usize) -> Result<String, DataError> {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let segs = match format {
        Format::Json => match schema::doc::parse_json(text) {
            Ok(p) => segs_from_spans(&p, offset),
            // Half-typed text: the cursor scanner still knows where it is.
            Err(e) => match schema::cursor::json_ctx(text, offset) {
                Some(ctx) => ctx.path,
                None => return Err(DataError::at(text, e.offset, e.message)),
            },
        },
        Format::Yaml => {
            let mut docs = Vec::new();
            for (start, parsed) in schema::doc::parse_yaml(text) {
                match parsed {
                    Ok(p) => docs.push((start, p)),
                    Err(e) => return Err(DataError::at(text, e.offset, e.message)),
                }
            }
            // Documents of a multi-document file are told apart by where they start.
            let pick = docs.iter().rposition(|(start, _)| *start <= offset).unwrap_or(0);
            match docs.get(pick) {
                Some((_, p)) => segs_from_spans(p, offset),
                None => Vec::new(),
            }
        }
        Format::Toml => toml_segs(text, offset)?,
        Format::Env => return Err(DataError::plain(t("Paths are shown for JSON, YAML and TOML files").to_string())),
    };
    Ok(format_path(&segs))
}

/// The innermost node whose key or value covers `offset`, as path segments (array positions by walking the value).
fn segs_from_spans(p: &schema::doc::Parsed, offset: usize) -> Vec<Seg> {
    let covers = |r: &std::ops::Range<usize>| r.start <= offset && offset <= r.end;
    let best = p
        .spans
        .iter()
        .filter_map(|(ptr, span)| {
            let mut size = None;
            if covers(&span.value) {
                size = Some(span.value.len());
            }
            if let Some(k) = span.key.as_ref().filter(|k| covers(k)) {
                size = Some(size.map_or(k.len(), |s: usize| s.min(k.len())));
            }
            size.map(|s| (s, std::cmp::Reverse(ptr.len()), ptr))
        })
        .min();
    let Some((_, _, ptr)) = best else { return Vec::new() };
    let mut segs = Vec::new();
    let mut node = &p.value;
    for raw in ptr.split('/').skip(1) {
        let name = raw.replace("~1", "/").replace("~0", "~");
        match node {
            Value::Array(items) => {
                let Ok(i) = name.parse::<usize>() else { break };
                segs.push(Seg::Index(i));
                node = items.get(i).unwrap_or(&Value::Null);
            }
            Value::Object(m) => {
                node = m.get(&name).unwrap_or(&Value::Null);
                segs.push(Seg::Key(name));
            }
            _ => break,
        }
    }
    segs
}

/// TOML: walk the spanned document and keep the innermost key or value that covers the offset.
fn toml_segs(text: &str, offset: usize) -> Result<Vec<Seg>, DataError> {
    use toml::de::{DeTable, DeValue};
    let doc = DeTable::parse(text).map_err(|e| match e.span() {
        Some(s) => DataError::at(text, s.start, e.message().to_string()),
        None => DataError::plain(e.message().to_string()),
    })?;
    fn walk(table: &DeTable<'_>, offset: usize, path: &mut Vec<Seg>, best: &mut Option<(usize, Vec<Seg>)>) {
        for (key, value) in table.iter() {
            path.push(Seg::Key(key.get_ref().to_string()));
            let span = key.span();
            if span.start <= offset && offset <= span.end {
                consider(best, span.len(), path);
            }
            visit(value.get_ref(), value.span(), offset, path, best);
            path.pop();
        }
    }

    fn visit(value: &DeValue<'_>, span: std::ops::Range<usize>, offset: usize, path: &mut Vec<Seg>, best: &mut Option<(usize, Vec<Seg>)>) {
        match value {
            DeValue::Table(t) => walk(t, offset, path, best),
            DeValue::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    path.push(Seg::Index(i));
                    visit(item.get_ref(), item.span(), offset, path, best);
                    path.pop();
                }
            }
            _ => {}
        }
        if span.start <= offset && offset <= span.end {
            consider(best, span.len(), path);
        }
    }

    fn consider(best: &mut Option<(usize, Vec<Seg>)>, size: usize, path: &[Seg]) {
        // Smaller span wins; between equal spans the deeper path.
        let better = match best {
            None => true,
            Some((s, p)) => size < *s || (size == *s && path.len() > p.len()),
        };
        if better {
            *best = Some((size, path.to_vec()));
        }
    }

    let mut best = None;
    walk(doc.get_ref(), offset, &mut Vec::new(), &mut best);
    Ok(best.map(|(_, p)| p).unwrap_or_default())
}

// --- Queries ---

pub struct QueryOutput {
    pub text: String,
    pub results: usize,
    /// The text stops early at `MAX_RESULT_BYTES`.
    pub truncated: bool,
}

/// Run `src` on `input` and render the results like `jq` does: each one pretty-printed, one after the other.
pub fn run_query(input: &Value, src: &str, opts: &Options, cancel: &dyn Fn() -> bool) -> Result<QueryOutput, QueryError> {
    let query = parse_query(src)?;
    let results = query.run(input, cancel)?;
    let mut text = String::new();
    let mut truncated = false;
    for r in &results {
        let one = if opts.sort_keys { emit_json(&sort_keys(r), opts.indent) } else { emit_json(r, opts.indent) };
        let one = one.map_err(|e| QueryError { message: e.message, offset: None })?;
        if text.len() + one.len() > MAX_RESULT_BYTES {
            truncated = true;
            // Whole results where there are several; a single huge one is cut at a line break.
            if text.is_empty() {
                let mut cut = MAX_RESULT_BYTES.min(one.len());
                while !one.is_char_boundary(cut) {
                    cut -= 1;
                }
                text.push_str(&one[..one[..cut].rfind('\n').map_or(0, |i| i + 1)]);
            }
            break;
        }
        text.push_str(&one);
    }
    Ok(QueryOutput { text, results: results.len(), truncated })
}
