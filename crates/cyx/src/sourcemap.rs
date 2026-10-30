//! Source maps (version 3) for TypeScript extensions: stack traces name the bundle (`handler (extension.js:12:5)`);
//! `cyx test` rewrites each such frame to the `.ts` file and line it came from (`handler (src/extension.ts:8:7)`).

use std::path::Path;

/// One mapped position: the source file, 1-based line and column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Original {
    pub source: String,
    pub line: u32,
    pub column: u32,
}

/// A parsed source map: per generated line, its segments as (generated column, source index, line, column), 0-based.
pub struct SourceMap {
    sources: Vec<String>,
    lines: Vec<Vec<(u32, u32, u32, u32)>>,
}

impl SourceMap {
    pub fn parse(json: &str) -> Option<SourceMap> {
        let v: serde_json::Value = serde_json::from_str(json).ok()?;
        let sources: Vec<String> = v["sources"].as_array()?.iter().map(|s| s.as_str().unwrap_or("").to_string()).collect();
        let mappings = v["mappings"].as_str()?;
        let (mut source, mut line, mut column) = (0i64, 0i64, 0i64);
        let mut lines = Vec::new();
        for text in mappings.split(';') {
            let mut segments = Vec::new();
            let mut generated = 0i64;
            for seg in text.split(',').filter(|s| !s.is_empty()) {
                let f = vlq(seg)?;
                generated += *f.first()?;
                if f.len() >= 4 {
                    source += f[1];
                    line += f[2];
                    column += f[3];
                    segments.push((generated.try_into().ok()?, source.try_into().ok()?, line.try_into().ok()?, column.try_into().ok()?));
                }
            }
            lines.push(segments);
        }
        Some(SourceMap { sources, lines })
    }

    /// Where 1-based `line`:`column` of the bundle came from.
    pub fn lookup(&self, line: u32, column: u32) -> Option<Original> {
        let segments = self.lines.get(line.checked_sub(1)? as usize)?;
        let col = column.saturating_sub(1);
        let &(_, s, l, c) = segments.iter().rev().find(|seg| seg.0 <= col).or_else(|| segments.first())?;
        Some(Original { source: self.sources.get(s as usize)?.clone(), line: l + 1, column: c + 1 })
    }

    /// `text` with every `<bundle>:line:column` replaced by the original position.
    pub fn apply(&self, bundle: &str, text: &str) -> String {
        let needle = format!("{bundle}:");
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(at) = rest.find(&needle) {
            out.push_str(&rest[..at]);
            let after = &rest[at + needle.len()..];
            match position(after).and_then(|(l, c, used)| self.lookup(l, c).map(|o| (o, used))) {
                Some((o, used)) => {
                    out.push_str(&format!("{}:{}:{}", o.source, o.line, o.column));
                    rest = &after[used..];
                }
                None => {
                    out.push_str(&needle);
                    rest = after;
                }
            }
        }
        out.push_str(rest);
        out
    }
}

/// The source map of the TypeScript extension in `dir`, if it has one.
pub fn load(dir: &Path) -> Option<SourceMap> {
    SourceMap::parse(&std::fs::read_to_string(dir.join(crate::typescript::SOURCE_MAP)).ok()?)
}

/// `12:5` at the start of `s`: line, column and the bytes they take.
fn position(s: &str) -> Option<(u32, u32, usize)> {
    let digits = |s: &str| s.bytes().take_while(u8::is_ascii_digit).count();
    let n = digits(s);
    let line = s[..n].parse().ok()?;
    let rest = s[n..].strip_prefix(':')?;
    let m = digits(rest);
    let column = rest[..m].parse().ok()?;
    Some((line, column, n + 1 + m))
}

/// The base64 VLQ fields of one segment.
fn vlq(seg: &str) -> Option<Vec<i64>> {
    let mut out = Vec::new();
    let (mut value, mut shift) = (0i64, 0u32);
    for b in seg.bytes() {
        let digit = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as i64;
        value += (digit & 31) << shift;
        if digit & 32 != 0 {
            shift += 5;
            if shift > 60 {
                return None;
            }
        } else {
            out.push(if value & 1 == 1 { -(value >> 1) } else { value >> 1 });
            value = 0;
            shift = 0;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Line 1: column 0 → a.ts 1:1; line 2: column 2 → a.ts 3:5, column 10 → b.ts 7:1.
    const MAP: &str = r#"{"version":3,"sources":["src/a.ts","src/b.ts"],"mappings":"AAAA;EAEI,QCIJ"}"#;

    #[test]
    fn decodes_and_looks_up() {
        assert_eq!(vlq("AAAA"), Some(vec![0, 0, 0, 0]));
        assert_eq!(vlq("EAEI"), Some(vec![2, 0, 2, 4]));
        assert_eq!(vlq("2H"), Some(vec![123]));
        let m = SourceMap::parse(MAP).unwrap();
        assert_eq!(m.lookup(1, 1), Some(Original { source: "src/a.ts".into(), line: 1, column: 1 }));
        assert_eq!(m.lookup(2, 5), Some(Original { source: "src/a.ts".into(), line: 3, column: 5 }));
        assert_eq!(m.lookup(2, 11), Some(Original { source: "src/b.ts".into(), line: 7, column: 1 }));
        assert_eq!(m.lookup(9, 1), None);
    }

    #[test]
    fn rewrites_stack_frames() {
        let m = SourceMap::parse(MAP).unwrap();
        let text = "boom\n    at greet (extension.js:2:11)\n    at <anonymous> (extension.js:1:1)\n    at other (lib.js:2:2)";
        assert_eq!(m.apply("extension.js", text), "boom\n    at greet (src/b.ts:7:1)\n    at <anonymous> (src/a.ts:1:1)\n    at other (lib.js:2:2)");
        // Nothing that looks like a position is left alone.
        assert_eq!(m.apply("extension.js", "see extension.js: no"), "see extension.js: no");
    }
}
