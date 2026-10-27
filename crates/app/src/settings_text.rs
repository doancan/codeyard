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
    let quoted_key = format!("\"{key}\"");
    let new_value = serde_json::to_string(value).unwrap_or_default();
    // Find the real line, not a key inside a comment line.
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("//")
            && trimmed.starts_with(&quoted_key)
            && let Some(colon) = line[line.len() - trimmed.len() + quoted_key.len()..].find(':').map(|c| c + line.len() - trimmed.len() + quoted_key.len())
        {
            let after = &line[colon + 1..];
            let v_start = offset + colon + 1 + (after.len() - after.trim_start().len());
            let v_end = v_start + value_len(&text[v_start..]);
            return format!("{}{}{}", &text[..v_start], new_value, &text[v_end..]);
        }
        offset += line.len();
    }
    match text.find('{') {
        Some(i) => format!("{}\n  {quoted_key}: {new_value},{}", &text[..=i], &text[i + 1..]),
        None => format!("{{\n  {quoted_key}: {new_value}\n}}\n"),
    }
}

/// Length of a JSON value (bytes): string, nested array/object or bare value.
fn value_len(rest: &str) -> usize {
    let bytes = rest.as_bytes();
    let (mut depth, mut in_str, mut esc) = (0i32, false, false);
    for (i, &b) in bytes.iter().enumerate() {
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
            continue;
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
            b',' | b'\n' | b'/' if depth == 0 => return i,
            _ => {}
        }
    }
    rest.len()
}
