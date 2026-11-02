//! Changing one key of a settings file in place: comments, other keys and their layout are kept.

use serde_json::Value;

/// Change a text setting in the file; comments and other lines are preserved.
/// If the key is missing it is inserted after the `{`.
pub fn set_string_in(text: &str, key: &str, value: &str) -> String {
    set_value_in(text, key, &Value::String(value.to_string()))
}

/// Any JSON value (number, array, object). Even if the existing value spans several
/// lines (an array), the whole of it is replaced by tracking brackets and strings.
pub fn set_value_in(text: &str, key: &str, value: &Value) -> String {
    let quoted_key = serde_json::to_string(key).unwrap();
    let new_value = serde_json::to_string(value).unwrap_or_default();
    let bytes = text.as_bytes();
    let root = skip_trivia(bytes, 0);
    let mut ranges = Vec::new();
    if bytes.get(root) == Some(&b'{') {
        let mut i = skip_trivia(bytes, root + 1);
        while bytes.get(i) == Some(&b'"') {
            let key_end = i + value_len(&text[i..]);
            let colon = skip_trivia(bytes, key_end);
            if bytes.get(colon) != Some(&b':') {
                break;
            }
            let start = skip_trivia(bytes, colon + 1);
            let end = start + value_len(&text[start..]);
            if serde_json::from_str::<String>(&text[i..key_end]).is_ok_and(|candidate| candidate == key) {
                ranges.push(start..end);
            }
            i = skip_trivia(bytes, end);
            if bytes.get(i) != Some(&b',') {
                break;
            }
            i = skip_trivia(bytes, i + 1);
        }
    }
    if !ranges.is_empty() {
        let mut out = text.to_string();
        // Updating every occurrence also repairs a duplicate left by an older writer.
        for range in ranges.into_iter().rev() {
            out.replace_range(range, &new_value);
        }
        return out;
    }
    if bytes.get(root) == Some(&b'{') {
        format!("{}\n  {quoted_key}: {new_value},{}", &text[..=root], &text[root + 1..])
    } else {
        format!("{{\n  {quoted_key}: {new_value}\n}}\n")
    }
}

/// Skip JSONC whitespace and comments without interpreting their contents as syntax.
pub(crate) fn skip_trivia(bytes: &[u8], mut i: usize) -> usize {
    loop {
        while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        if bytes.get(i..i + 2) == Some(b"//") {
            while bytes.get(i).is_some_and(|b| *b != b'\n') {
                i += 1;
            }
        } else if bytes.get(i..i + 2) == Some(b"/*") {
            i += 2;
            while i < bytes.len() && bytes.get(i..i + 2) != Some(b"*/") {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
        } else {
            return i;
        }
    }
}

/// Length of a JSON value (bytes): string, nested array/object or bare value.
pub(crate) fn value_len(rest: &str) -> usize {
    let bytes = rest.as_bytes();
    let (mut depth, mut in_str, mut esc) = (0i32, false, false);
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if in_str {
            match (esc, b) {
                (true, _) => esc = false,
                (false, b'\\') => esc = true,
                (false, b'"') => {
                    in_str = false;
                    if depth == 0 {
                        return i + 1;
                    }
                }
                _ => {}
            }
            i += 1;
            continue;
        }
        if b == b'/' {
            if depth == 0 {
                return i;
            }
            let next = skip_trivia(bytes, i);
            if next != i {
                i = next;
                continue;
            }
        }
        match b {
            b'"' => in_str = true,
            b'[' | b'{' => depth += 1,
            b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
                if depth < 0 {
                    return i; // end of the object: the bare value is over
                }
            }
            b',' if depth == 0 => return i,
            b if b.is_ascii_whitespace() && depth == 0 => return i,
            _ => {}
        }
        i += 1;
    }
    rest.len()
}

/// One step of a path into a JSONC document: an object member or an array element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seg<'a> {
    Key(&'a str),
    Index(usize),
}

/// Byte range of the value at `path`, read the way a JSON parser does (for a repeated key, the last one wins). Comments,
/// trailing commas and a byte order mark are skipped. `None` when the path does not lead to a value.
pub fn value_range(text: &str, path: &[Seg]) -> Option<std::ops::Range<usize>> {
    let bytes = text.as_bytes();
    let first = if text.starts_with('\u{feff}') { 3 } else { 0 };
    let start = skip_trivia(bytes, first);
    if start >= bytes.len() {
        return None;
    }
    let mut range = start..start + value_len(text.get(start..)?);
    for seg in path {
        range = match *seg {
            Seg::Key(key) => members(text, range.clone())?.into_iter().rev().find(|(k, _)| k == key).map(|(_, v)| v)?,
            Seg::Index(n) => array_items(text, range.clone())?.into_iter().nth(n)?,
        };
    }
    Some(range)
}

/// The members (decoded key, value range) of the object at `range`; `None` if it is not an object.
pub fn members(text: &str, range: std::ops::Range<usize>) -> Option<Vec<(String, std::ops::Range<usize>)>> {
    Some(member_spans(text, range)?.into_iter().map(|(k, _, v)| (k, v)).collect())
}

/// Like `members`, with where each key starts.
fn member_spans(text: &str, range: std::ops::Range<usize>) -> Option<Vec<(String, usize, std::ops::Range<usize>)>> {
    let bytes = text.as_bytes();
    if bytes.get(range.start) != Some(&b'{') {
        return None;
    }
    let mut out = Vec::new();
    let mut i = skip_trivia(bytes, range.start + 1);
    while i < range.end && bytes.get(i) == Some(&b'"') {
        let key_end = i + value_len(text.get(i..)?);
        let key = serde_json::from_str::<String>(text.get(i..key_end)?).ok()?;
        let colon = skip_trivia(bytes, key_end);
        if bytes.get(colon) != Some(&b':') {
            return None;
        }
        let start = skip_trivia(bytes, colon + 1);
        let end = start + value_len(text.get(start..)?);
        out.push((key, i, start..end));
        i = skip_trivia(bytes, end);
        if bytes.get(i) != Some(&b',') {
            break;
        }
        i = skip_trivia(bytes, i + 1);
    }
    Some(out)
}

/// Set the value at an object path (e.g. `["lsp.servers", "rust", "settings", "rust-analyzer.check.command"]`), creating
/// the objects that are missing; `None` removes it and the objects that are left empty. Comments and other members stay.
pub fn set_path_in(text: &str, path: &[&str], value: Option<&Value>) -> String {
    let Some((last, parents)) = path.split_last() else { return text.to_string() };
    let segs: Vec<Seg> = path.iter().map(|k| Seg::Key(k)).collect();
    let is_object = |r: &std::ops::Range<usize>| text.as_bytes().get(r.start) == Some(&b'{');
    let Some(value) = value else {
        let Some(parent) = value_range(text, &segs[..parents.len()]) else { return text.to_string() };
        let Some((_, key, val)) = member_spans(text, parent).and_then(|m| m.into_iter().rev().find(|(k, ..)| k == last)) else { return text.to_string() };
        // The whole member: its own line(s) and its comma, when nothing else shares them.
        let line_start = text[..key].rfind('\n').map_or(0, |i| i + 1);
        let start = if text[line_start..key].trim().is_empty() { line_start } else { key };
        let mut end = val.end + (text[val.end..].len() - text[val.end..].trim_start_matches([' ', '\t']).len());
        if text[end..].starts_with(',') {
            end += 1;
        }
        let line_end = text[end..].find('\n').map_or(text.len(), |i| end + i + 1);
        if start == line_start && text[end..line_end].trim().is_empty() {
            end = line_end;
        }
        let out = format!("{}{}", &text[..start], &text[end..]);
        let emptied = !parents.is_empty() && value_range(&out, &segs[..parents.len()]).is_some_and(|r| out[r].split_whitespace().collect::<String>() == "{}");
        return if emptied { set_path_in(&out, parents, None) } else { out };
    };
    // `path[..depth]` are objects; `path[depth]` is missing, not an object, or the value itself.
    let mut depth = 0;
    while depth < path.len() && value_range(text, &segs[..=depth]).is_some_and(|r| depth < parents.len() && is_object(&r)) {
        depth += 1;
    }
    let nested = path[depth + 1..].iter().rev().fold(value.clone(), |inner, key| serde_json::json!({ *key: inner }));
    let json = serde_json::to_string(&nested).unwrap_or_default();
    if let Some(r) = value_range(text, &segs[..=depth]) {
        format!("{}{json}{}", &text[..r.start], &text[r.end..])
    } else if depth == 0 {
        set_value_in(text, path[0], &nested)
    } else {
        let Some(obj) = value_range(text, &segs[..depth]) else { return text.to_string() };
        let indent = "  ".repeat(depth + 1);
        let key = serde_json::to_string(path[depth]).unwrap_or_default();
        format!("{}\n{indent}{key}: {json},{}", &text[..=obj.start], &text[obj.start + 1..])
    }
}

/// The element ranges of the array at `range`; `None` if it is not an array.
pub fn array_items(text: &str, range: std::ops::Range<usize>) -> Option<Vec<std::ops::Range<usize>>> {
    let bytes = text.as_bytes();
    if bytes.get(range.start) != Some(&b'[') {
        return None;
    }
    let mut out = Vec::new();
    let mut i = skip_trivia(bytes, range.start + 1);
    while i < range.end && !matches!(bytes.get(i), Some(b']') | None) {
        let end = i + value_len(text.get(i..)?);
        if end == i {
            return None;
        }
        out.push(i..end);
        i = skip_trivia(bytes, end);
        if bytes.get(i) != Some(&b',') {
            break;
        }
        i = skip_trivia(bytes, i + 1);
    }
    Some(out)
}

#[cfg(test)]
mod span_tests {
    use super::*;

    #[test]
    fn value_ranges_follow_keys_and_indices_through_comments() {
        let text = "\u{feff}{ // c\n \"a\": [ 1, /* x */ { \"b\": \"ş\" }, ], \"a\": [ { \"b\": 2 } ] }";
        let r = value_range(text, &[Seg::Key("a"), Seg::Index(0), Seg::Key("b")]).unwrap();
        assert_eq!(&text[r], "2", "the last repeated key wins, like a JSON parser");
        assert_eq!(value_range(text, &[Seg::Key("missing")]), None);
        assert_eq!(value_range("", &[]), None);
        let list = "[ 1, \"x,y\" , {\"k\":[1,2]}, ]";
        let items = array_items(list, 0..list.len()).unwrap();
        assert_eq!(items.iter().map(|r| &list[r.clone()]).collect::<Vec<_>>(), ["1", "\"x,y\"", "{\"k\":[1,2]}"]);
        assert_eq!(array_items("[]", 0..2).unwrap(), Vec::<std::ops::Range<usize>>::new());
        assert_eq!(array_items("{}", 0..2), None);
        assert_eq!(members("[]", 0..2), None);
    }
}
