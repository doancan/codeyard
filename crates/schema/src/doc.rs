//! Turns JSON (with comments and trailing commas) and YAML documents into a `serde_json::Value`, keeping each node's source span.
//! Spans are byte offsets; nodes are addressed by JSON pointer (`/a/0/b`), and a schema error maps from the pointer back to a line.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use serde_json::{Map, Number, Value};

/// Where a node sits in the text: the value's span and, for object members, the key's span.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Span {
    pub value: Range<usize>,
    pub key: Option<Range<usize>>,
}

#[derive(Debug, Default)]
pub struct Parsed {
    pub value: Value,
    /// Pointer -> span ("" is the root).
    pub spans: HashMap<String, Span>,
    /// Custom-tagged (`!Ref`, `!reference`...) nodes: their type can't be checked against the schema, and their subtree is ignored.
    pub opaque: HashSet<String>,
    /// Start of every object key that repeats an earlier key of the same object; the later value is the one in `value`.
    pub duplicates: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    pub message: String,
    pub offset: usize,
}

/// Pointer segment escaping (RFC 6901).
pub fn escape(seg: &str) -> String {
    seg.replace('~', "~0").replace('/', "~1")
}

pub fn child(pointer: &str, seg: &str) -> String {
    format!("{pointer}/{}", escape(seg))
}

const MAX_DEPTH: usize = 128;

// --- JSON ---

pub fn parse_json(text: &str) -> Result<Parsed, SyntaxError> {
    let mut p = JsonParser { s: text.as_bytes(), text, i: 0, out: Parsed::default() };
    p.skip();
    if p.i >= p.s.len() {
        return Err(SyntaxError { message: "Expected a value".into(), offset: 0 });
    }
    let value = p.value("", 0, None)?;
    p.skip();
    if p.i < p.s.len() {
        return Err(p.err("Unexpected content after the value"));
    }
    p.out.value = value;
    Ok(p.out)
}

struct JsonParser<'a> {
    s: &'a [u8],
    text: &'a str,
    i: usize,
    out: Parsed,
}

impl JsonParser<'_> {
    fn err(&self, message: &str) -> SyntaxError {
        SyntaxError { message: message.into(), offset: self.i.min(self.s.len()) }
    }

    /// Whitespace and comments (`//`, `/* */`).
    fn skip(&mut self) {
        loop {
            while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\r' | b'\n' | 0xEF | 0xBB | 0xBF) {
                self.i += 1;
            }
            if self.s[self.i..].starts_with(b"//") {
                while self.i < self.s.len() && self.s[self.i] != b'\n' {
                    self.i += 1;
                }
            } else if self.s[self.i..].starts_with(b"/*") {
                match self.text[self.i + 2..].find("*/") {
                    Some(n) => self.i += n + 4,
                    None => self.i = self.s.len(),
                }
            } else {
                return;
            }
        }
    }

    fn value(&mut self, pointer: &str, depth: usize, key: Option<Range<usize>>) -> Result<Value, SyntaxError> {
        if depth > MAX_DEPTH {
            return Err(self.err("Nested too deeply"));
        }
        self.skip();
        let start = self.i;
        let v = match self.s.get(self.i) {
            None => return Err(self.err("Unexpected end of file")),
            Some(b'{') => self.object(pointer, depth)?,
            Some(b'[') => self.array(pointer, depth)?,
            Some(b'"') => Value::String(self.string()?),
            Some(b't') if self.s[self.i..].starts_with(b"true") => {
                self.i += 4;
                Value::Bool(true)
            }
            Some(b'f') if self.s[self.i..].starts_with(b"false") => {
                self.i += 5;
                Value::Bool(false)
            }
            Some(b'n') if self.s[self.i..].starts_with(b"null") => {
                self.i += 4;
                Value::Null
            }
            Some(b'-' | b'0'..=b'9') => self.number()?,
            Some(_) => return Err(self.err("Unexpected character")),
        };
        self.out.spans.insert(pointer.to_string(), Span { value: start..self.i, key });
        Ok(v)
    }

    fn number(&mut self) -> Result<Value, SyntaxError> {
        let start = self.i;
        while self.i < self.s.len() && matches!(self.s[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
            self.i += 1;
        }
        let raw = &self.text[start..self.i];
        let n = raw
            .parse::<i64>()
            .map(Number::from)
            .ok()
            .or_else(|| raw.parse::<u64>().map(Number::from).ok())
            .or_else(|| raw.parse::<f64>().ok().and_then(Number::from_f64));
        n.map(Value::Number).ok_or(SyntaxError { message: format!("Invalid number “{raw}”"), offset: start })
    }

    fn string(&mut self) -> Result<String, SyntaxError> {
        let start = self.i;
        self.i += 1;
        while self.i < self.s.len() {
            match self.s[self.i] {
                b'\\' => self.i += 2,
                b'"' => {
                    self.i += 1;
                    return serde_json::from_str::<String>(&self.text[start..self.i])
                        .map_err(|e| SyntaxError { message: format!("Invalid string: {e}"), offset: start });
                }
                b'\n' => break,
                _ => self.i += 1,
            }
        }
        Err(SyntaxError { message: "Unterminated string".into(), offset: start })
    }

    fn object(&mut self, pointer: &str, depth: usize) -> Result<Value, SyntaxError> {
        self.i += 1;
        let mut map = Map::new();
        loop {
            self.skip();
            match self.s.get(self.i) {
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Value::Object(map));
                }
                Some(b'"') => {}
                Some(_) => return Err(self.err("Expected a property name in double quotes")),
                None => return Err(self.err("Unexpected end of file in object")),
            }
            let key_start = self.i;
            let key = self.string()?;
            let key_range = key_start..self.i;
            self.skip();
            if self.s.get(self.i) != Some(&b':') {
                return Err(self.err("Expected ':' after the property name"));
            }
            self.i += 1;
            let v = self.value(&child(pointer, &key), depth + 1, Some(key_range))?;
            if map.insert(key, v).is_some() {
                self.out.duplicates.push(key_start);
            }
            self.skip();
            match self.s.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b'}') => {}
                Some(_) => return Err(self.err("Expected ',' or '}'")),
                None => return Err(self.err("Unexpected end of file in object")),
            }
        }
    }

    fn array(&mut self, pointer: &str, depth: usize) -> Result<Value, SyntaxError> {
        self.i += 1;
        let mut items = Vec::new();
        loop {
            self.skip();
            match self.s.get(self.i) {
                Some(b']') => {
                    self.i += 1;
                    return Ok(Value::Array(items));
                }
                None => return Err(self.err("Unexpected end of file in array")),
                _ => {}
            }
            let v = self.value(&child(pointer, &items.len().to_string()), depth + 1, None)?;
            items.push(v);
            self.skip();
            match self.s.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b']') => {}
                Some(_) => return Err(self.err("Expected ',' or ']'")),
                None => return Err(self.err("Unexpected end of file in array")),
            }
        }
    }
}

// --- YAML ---

/// YAML documents, each (start byte offset, parse result). Multi-document files (Kubernetes) are supported.
pub fn parse_yaml(text: &str) -> Vec<(usize, Result<Parsed, SyntaxError>)> {
    use yaml_rust2::parser::Parser;
    let chars: Vec<usize> = if text.is_ascii() { Vec::new() } else { text.char_indices().map(|(b, _)| b).chain(std::iter::once(text.len())).collect() };
    let byte = |marker_index: usize| if chars.is_empty() { marker_index.min(text.len()) } else { chars.get(marker_index).copied().unwrap_or(text.len()) };
    let mut b = YamlBuilder { text, byte: &byte, docs: Vec::new(), cur: None, stack: Vec::new(), anchors: HashMap::new(), budget: ALIAS_BUDGET, fatal: None };
    let mut parser = Parser::new_from_str(text);
    let loaded = parser.load(&mut b, true);
    if let Some(e) = b.fatal.take() {
        return vec![(e.offset, Err(e))];
    }
    match loaded {
        Ok(()) => b.docs,
        Err(e) => {
            let m = e.marker();
            let mut docs = b.docs;
            docs.push((byte(m.index()), Err(SyntaxError { message: e.info().to_string(), offset: byte(m.index()) })));
            docs
        }
    }
}

struct Frame {
    pointer: String,
    key: Option<Range<usize>>,
    start: usize,
    kind: FrameKind,
    anchor: usize,
    custom_tag: bool,
}

enum FrameKind {
    Map { map: Map<String, Value>, pending_key: Option<(String, Range<usize>)>, merges: Vec<Value> },
    Seq(Vec<Value>),
}

struct YamlBuilder<'a> {
    text: &'a str,
    byte: &'a dyn Fn(usize) -> usize,
    docs: Vec<(usize, Result<Parsed, SyntaxError>)>,
    cur: Option<(usize, Parsed)>,
    stack: Vec<Frame>,
    anchors: HashMap<usize, Value>,
    /// Remaining node budget for alias expansion: stops nested aliases from blowing up a small file's
    /// memory ("billion laughs").
    budget: usize,
    /// Budget or depth exceeded: later events are ignored and the result is an error.
    fatal: Option<SyntaxError>,
}

/// Number of nodes an alias will copy (non-recursive).
fn weight(v: &Value) -> usize {
    let mut n = 0;
    let mut todo = vec![v];
    while let Some(v) = todo.pop() {
        n += 1;
        match v {
            Value::Array(a) => todo.extend(a),
            Value::Object(m) => todo.extend(m.values()),
            _ => {}
        }
    }
    n
}

const ALIAS_BUDGET: usize = 1_000_000;
const MAX_YAML_DEPTH: usize = 200;

/// YAML 1.2 core schema: type of a plain (unquoted) scalar.
pub fn plain_scalar(s: &str) -> Value {
    match s {
        "" | "~" | "null" | "Null" | "NULL" => return Value::Null,
        "true" | "True" | "TRUE" => return Value::Bool(true),
        "false" | "False" | "FALSE" => return Value::Bool(false),
        ".inf" | ".Inf" | ".INF" | "+.inf" | "-.inf" | ".nan" | ".NaN" | ".NAN" => return Value::String(s.into()),
        _ => {}
    }
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    let unsigned = s.strip_prefix(['-', '+']).unwrap_or(s);
    if digits(unsigned) {
        if let Ok(n) = s.trim_start_matches('+').parse::<i64>() {
            return Value::Number(n.into());
        }
        if let Ok(n) = s.trim_start_matches('+').parse::<u64>() {
            return Value::Number(n.into());
        }
    }
    if let Some(h) = s.strip_prefix("0x").filter(|h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit()))
        && let Ok(n) = i64::from_str_radix(h, 16)
    {
        return Value::Number(n.into());
    }
    if let Some(o) = s.strip_prefix("0o").filter(|o| !o.is_empty() && o.bytes().all(|b| (b'0'..=b'7').contains(&b)))
        && let Ok(n) = i64::from_str_radix(o, 8)
    {
        return Value::Number(n.into());
    }
    let float_like = {
        let t = unsigned;
        let mut seen_digit = false;
        let mut ok = !t.is_empty();
        let mut prev = ' ';
        for c in t.chars() {
            match c {
                '0'..='9' => seen_digit = true,
                '.' => {}
                'e' | 'E' if seen_digit => {}
                '+' | '-' if matches!(prev, 'e' | 'E') => {}
                _ => ok = false,
            }
            prev = c;
        }
        ok && seen_digit
    };
    if float_like
        && let Ok(f) = s.parse::<f64>()
        && let Some(n) = Number::from_f64(f)
    {
        return Value::Number(n);
    }
    Value::String(s.into())
}

impl YamlBuilder<'_> {
    /// Hand the finished value to its container (or the document root).
    fn put(&mut self, value: Value, span_end: usize, start: usize, anchor: usize, custom_tag: bool) {
        let (mut pointer, mut key) = (String::new(), None);
        match self.stack.last_mut() {
            None => {}
            Some(Frame { pointer: p, kind: FrameKind::Seq(items), .. }) => {
                pointer = child(p, &items.len().to_string());
            }
            // Scalar in key position: held as the pending key (the value arrives with the next event).
            Some(Frame { kind: FrameKind::Map { pending_key: None, .. }, .. }) => {}
            Some(Frame { pointer: p, kind: FrameKind::Map { pending_key: Some((k, kr)), .. }, .. }) => {
                pointer = child(p, k);
                key = Some(kr.clone());
            }
        }
        // If a key is expected, this is the key.
        if let Some(Frame { kind: FrameKind::Map { pending_key: pending @ None, .. }, .. }) = self.stack.last_mut() {
            let name = match &value {
                Value::String(s) => s.clone(),
                other => other.to_string().trim_matches('"').to_string(),
            };
            *pending = Some((name, start..span_end));
            return;
        }
        if anchor != 0 {
            self.anchors.insert(anchor, value.clone());
        }
        if let Some((_, doc)) = &mut self.cur {
            doc.spans.insert(pointer.clone(), Span { value: start..span_end, key });
            if custom_tag {
                doc.opaque.insert(pointer.clone());
            }
        }
        let mut duplicate = None;
        match self.stack.last_mut() {
            None => {
                if let Some((_, doc)) = &mut self.cur {
                    doc.value = value;
                }
            }
            Some(Frame { kind: FrameKind::Seq(items), .. }) => items.push(value),
            Some(Frame { kind: FrameKind::Map { map, pending_key, merges }, .. }) => {
                if let Some((k, range)) = pending_key.take() {
                    if k == "<<" {
                        merges.push(value);
                    } else if map.insert(k, value).is_some() {
                        duplicate = Some(range.start);
                    }
                }
            }
        }
        if let (Some(at), Some((_, doc))) = (duplicate, &mut self.cur) {
            doc.duplicates.push(at);
        }
    }

    fn scalar_end(&self, start: usize, value_len_chars: usize, quoted: bool) -> usize {
        let rest = &self.text[start.min(self.text.len())..];
        let line_end = rest.find('\n').map_or(self.text.len(), |n| start + n);
        let want = if quoted {
            rest.chars().take(value_len_chars + 2).map(char::len_utf8).sum::<usize>()
        } else {
            rest.chars().take(value_len_chars).map(char::len_utf8).sum::<usize>()
        };
        (start + want.max(1)).min(line_end.max(start))
    }
}

impl yaml_rust2::parser::MarkedEventReceiver for YamlBuilder<'_> {
    fn on_event(&mut self, ev: yaml_rust2::parser::Event, mark: yaml_rust2::scanner::Marker) {
        use yaml_rust2::parser::Event::*;
        use yaml_rust2::scanner::TScalarStyle;
        let at = (self.byte)(mark.index());
        if self.fatal.is_some() {
            return;
        }
        match ev {
            DocumentStart => {
                self.cur = Some((at, Parsed::default()));
                self.stack.clear();
            }
            DocumentEnd => {
                if let Some((start, doc)) = self.cur.take() {
                    self.docs.push((start, Ok(doc)));
                }
                self.stack.clear();
                self.anchors.clear();
            }
            Scalar(s, style, anchor, tag) => {
                let quoted = !matches!(style, TScalarStyle::Plain);
                let custom = tag.as_ref().is_some_and(|t| t.handle != "!!" && !(t.handle.is_empty() && t.suffix == "!"));
                let secondary = tag.as_ref().filter(|t| t.handle == "tag:yaml.org,2002:" || t.handle == "!!").map(|t| t.suffix.as_str());
                let value = match (secondary, quoted) {
                    (Some("str"), _) | (None, true) => Value::String(s.clone()),
                    (Some("int" | "float" | "bool" | "null"), _) | (None, false) => plain_scalar(&s),
                    _ => Value::String(s.clone()),
                };
                let end = self.scalar_end(at, s.chars().count(), quoted);
                self.put(value, end, at, anchor, custom);
            }
            Alias(id) => {
                let v = self.anchors.get(&id).cloned().unwrap_or(Value::Null);
                let w = weight(&v);
                if w > self.budget {
                    self.fatal = Some(SyntaxError { message: "The aliases (*name) expand to far too much data; the file is not checked".into(), offset: at });
                    return;
                }
                self.budget -= w;
                self.put(v, at + 1, at, 0, false);
            }
            MappingStart(..) | SequenceStart(..) if self.stack.len() >= MAX_YAML_DEPTH => {
                self.fatal = Some(SyntaxError { message: "The document is nested too deeply; it is not checked".into(), offset: at });
            }
            MappingStart(anchor, tag) => {
                let (pointer, key) = self.next_pointer();
                self.stack.push(Frame {
                    pointer,
                    key,
                    start: at,
                    kind: FrameKind::Map { map: Map::new(), pending_key: None, merges: Vec::new() },
                    anchor,
                    custom_tag: tag.is_some_and(|t| t.handle != "!!"),
                });
            }
            SequenceStart(anchor, tag) => {
                let (pointer, key) = self.next_pointer();
                self.stack.push(Frame { pointer, key, start: at, kind: FrameKind::Seq(Vec::new()), anchor, custom_tag: tag.is_some_and(|t| t.handle != "!!") });
            }
            MappingEnd | SequenceEnd => {
                let Some(frame) = self.stack.pop() else { return };
                let value = match frame.kind {
                    FrameKind::Seq(items) => Value::Array(items),
                    FrameKind::Map { mut map, merges, .. } => {
                        // `<<: *base` or `<<: [*a, *b]`: explicitly written keys win.
                        for m in merges {
                            let objs = match m {
                                Value::Array(a) => a,
                                o => vec![o],
                            };
                            for o in objs {
                                if let Value::Object(o) = o {
                                    for (k, v) in o {
                                        map.entry(k).or_insert(v);
                                    }
                                }
                            }
                        }
                        Value::Object(map)
                    }
                };
                // Frame popped: hand the finished node to its container; the node's own pointer/key is in the frame.
                let end = at.max(frame.start + 1);
                if frame.anchor != 0 {
                    self.anchors.insert(frame.anchor, value.clone());
                }
                if let Some((_, doc)) = &mut self.cur {
                    doc.spans.insert(frame.pointer.clone(), Span { value: frame.start..end, key: frame.key.clone() });
                    if frame.custom_tag {
                        doc.opaque.insert(frame.pointer.clone());
                    }
                }
                self.attach(value);
            }
            Nothing | StreamStart | StreamEnd => {}
        }
    }
}

impl YamlBuilder<'_> {
    /// Pointer and (for object members) key span of a new container node; a container in key position is ignored.
    fn next_pointer(&mut self) -> (String, Option<Range<usize>>) {
        match self.stack.last() {
            None => (String::new(), None),
            Some(Frame { pointer, kind: FrameKind::Seq(items), .. }) => (child(pointer, &items.len().to_string()), None),
            Some(Frame { pointer, kind: FrameKind::Map { pending_key: Some((k, kr)), .. }, .. }) => (child(pointer, k), Some(kr.clone())),
            Some(Frame { pointer, kind: FrameKind::Map { pending_key: None, .. }, .. }) => (child(pointer, "<complex key>"), None),
        }
    }

    /// Place the finished container value into the parent frame (or the root).
    fn attach(&mut self, value: Value) {
        let mut duplicate = None;
        match self.stack.last_mut() {
            None => {
                if let Some((_, doc)) = &mut self.cur {
                    doc.value = value;
                }
            }
            Some(Frame { kind: FrameKind::Seq(items), .. }) => items.push(value),
            Some(Frame { kind: FrameKind::Map { map, pending_key, merges }, .. }) => {
                if let Some((k, range)) = pending_key.take() {
                    if k == "<<" {
                        merges.push(value);
                    } else if map.insert(k, value).is_some() {
                        duplicate = Some(range.start);
                    }
                }
            }
        }
        if let (Some(at), Some((_, doc))) = (duplicate, &mut self.cur) {
            doc.duplicates.push(at);
        }
    }
}

/// From a byte offset to (line, UTF-16 column), an LSP position.
pub fn position(text: &str, offset: usize) -> (u32, u32) {
    let offset = offset.min(text.len());
    let before = &text[..offset];
    let line = before.matches('\n').count() as u32;
    let line_start = before.rfind('\n').map_or(0, |n| n + 1);
    let col = text[line_start..offset].encode_utf16().count() as u32;
    (line, col)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn json_with_comments_and_trailing_commas() {
        let src = "{\n  // c\n  \"a\": 1, /* x */ \"b\": [true, null, \"ğ\\n\",],\n}\n";
        let p = parse_json(src).unwrap();
        assert_eq!(p.value, json!({"a": 1, "b": [true, null, "ğ\n"]}));
        let a = &p.spans["/a"];
        assert_eq!(&src[a.value.clone()], "1");
        assert_eq!(&src[a.key.clone().unwrap()], "\"a\"");
        assert_eq!(&src[p.spans["/b/2"].value.clone()], "\"ğ\\n\"");
        assert_eq!(&src[p.spans[""].value.clone()][..1], "{");
    }

    #[test]
    fn json_errors_point_at_the_problem() {
        let e = parse_json("{\"a\": }").unwrap_err();
        assert_eq!(e.offset, 6);
        assert!(parse_json("{\"a\" 1}").unwrap_err().message.contains("':'"));
        assert!(parse_json("[1, 2").unwrap_err().message.contains("end of file"));
        assert!(parse_json("{} x").is_err());
        assert!(parse_json("").is_err());
        assert!(parse_json(&"[".repeat(500)).unwrap_err().message.contains("deeply"));
        assert!(parse_json("\"abc").unwrap_err().message.contains("Unterminated"));
    }

    #[test]
    fn pointers_escape_slashes_and_tildes() {
        let p = parse_json("{\"a/b\": {\"c~d\": 1}}").unwrap();
        assert!(p.spans.contains_key("/a~1b/c~0d"));
    }

    fn yaml(src: &str) -> Parsed {
        let mut docs = parse_yaml(src);
        assert_eq!(docs.len(), 1, "{docs:?}");
        docs.remove(0).1.unwrap()
    }

    #[test]
    fn yaml_scalars_follow_the_core_schema() {
        let p = yaml("a: 1\nb: 1.5\nc: true\nd: yes\ne: ~\nf: \"1\"\ng: 0x1F\nh: on\ni: 2026-01-01\nj: 1e3\nk: -7\nl: '1.0'\nm: v1.2\n");
        assert_eq!(p.value["a"], json!(1));
        assert_eq!(p.value["b"], json!(1.5));
        assert_eq!(p.value["c"], json!(true));
        assert_eq!(p.value["d"], json!("yes"), "YAML 1.2: yes is a string");
        assert_eq!(p.value["e"], Value::Null);
        assert_eq!(p.value["f"], json!("1"));
        assert_eq!(p.value["g"], json!(31));
        assert_eq!(p.value["h"], json!("on"), "GitHub Actions `on:`");
        assert_eq!(p.value["i"], json!("2026-01-01"));
        assert_eq!(p.value["j"], json!(1000.0));
        assert_eq!(p.value["k"], json!(-7));
        assert_eq!(p.value["l"], json!("1.0"));
        assert_eq!(p.value["m"], json!("v1.2"));
    }

    #[test]
    fn yaml_structure_spans_anchors_and_merges() {
        let src = "base: &b\n  x: 1\n  y: 2\nitem:\n  <<: *b\n  y: 3\nlist:\n  - a\n  - b: 1\n";
        let p = yaml(src);
        assert_eq!(p.value["item"], json!({"x": 1, "y": 3}), "explicit keys beat merged ones");
        assert_eq!(p.value["list"], json!(["a", {"b": 1}]));
        assert_eq!(&src[p.spans["/list/0"].value.clone()], "a");
        assert_eq!(&src[p.spans["/item/y"].key.clone().unwrap()], "y");
        assert_eq!(&src[p.spans["/item/y"].value.clone()], "3");
        assert!(p.spans.contains_key("/list/1/b"));
    }

    #[test]
    fn yaml_multi_document_and_errors() {
        let docs = parse_yaml("a: 1\n---\nb: 2\n");
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[1].0, docs[1].0);
        assert_eq!(docs[0].1.as_ref().unwrap().value, json!({"a": 1}));
        assert_eq!(docs[1].1.as_ref().unwrap().value, json!({"b": 2}));
        let bad = parse_yaml("a: [1, 2\nb: 3\n");
        assert!(bad.last().unwrap().1.is_err());
    }

    #[test]
    fn yaml_custom_tags_are_opaque_and_unicode_offsets_are_bytes() {
        let src = "Name: !Ref Param\nçay: demli\n";
        let p = yaml(src);
        assert!(p.opaque.contains("/Name"), "{:?}", p.opaque);
        assert_eq!(&src[p.spans["/çay"].value.clone()], "demli");
    }

    #[test]
    fn alias_bombs_and_extreme_nesting_are_stopped_cheaply() {
        // "Billion laughs": each level refers to the previous one ten times.
        let mut doc = String::from("a: &a [\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\"]\n");
        let mut prev = 'a';
        for next in 'b'..='i' {
            doc.push_str(&format!("{next}: &{next} [{}]\n", vec![format!("*{prev}"); 9].join(",")));
            prev = next;
        }
        let started = std::time::Instant::now();
        let docs = parse_yaml(&doc);
        assert!(started.elapsed().as_secs() < 5, "took {:?}", started.elapsed());
        let err = docs.last().unwrap().1.as_ref().unwrap_err();
        assert!(err.message.contains("expand"), "{err:?}");
        // A harmless alias still works.
        let ok = parse_yaml("base: &b {x: 1}\ncopy: *b\n");
        assert_eq!(ok[0].1.as_ref().unwrap().value["copy"], json!({"x": 1}));
        // Deep nesting is an error, not a stack overflow.
        let deep = format!("{}1{}", "[".repeat(5000), "]".repeat(5000));
        let docs = parse_yaml(&deep);
        assert!(docs.last().unwrap().1.is_err());
        let deep_json = "[".repeat(100_000);
        assert!(parse_json(&deep_json).is_err());
    }

    #[test]
    fn repeated_keys_are_recorded_where_they_start() {
        let src = "{\"a\": 1, \"b\": {\"x\": 1, \"x\": 2}, \"a\": 3}";
        let p = parse_json(src).unwrap();
        let at: Vec<&str> = p.duplicates.iter().map(|i| &src[*i..*i + 3]).collect();
        assert_eq!((at, &p.value["a"]), (vec!["\"x\"", "\"a\""], &json!(3)));
        let y = "a: 1\nb:\n  c: 1\n  c: 2\na: 3\n";
        let p = yaml(y);
        assert_eq!(p.duplicates.iter().map(|i| &y[*i..*i + 1]).collect::<Vec<_>>(), ["c", "a"]);
    }

    #[test]
    fn positions_use_utf16_columns() {
        let t = "ab\nçağ 😀x";
        assert_eq!(position(t, 0), (0, 0));
        assert_eq!(position(t, 3), (1, 0));
        let emoji = t.find('😀').unwrap();
        assert_eq!(position(t, emoji), (1, 4));
        assert_eq!(position(t, emoji + 4), (1, 6), "an emoji is two UTF-16 units");
    }
}
