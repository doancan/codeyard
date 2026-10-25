//! JSON / JSONC formatting: re-indents while preserving key order and comments
//! (no server needed). Works on the text; doesn't parse and rewrite.

const MAX_DEPTH: usize = 512;

/// Indent without breaking the structure. `indent`: one level's indentation ("  ", "\t"). Reports
/// definite errors such as unbalanced brackets (`Err`); the content is unchanged.
pub fn format(text: &str, indent: &str) -> Result<String, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + text.len() / 4);
    let mut depth = 0usize;
    let mut stack: Vec<char> = Vec::new();
    let mut i = 0;
    let newline = |out: &mut String, depth: usize| {
        while out.ends_with(' ') || out.ends_with('\t') {
            out.pop();
        }
        out.push('\n');
        for _ in 0..depth {
            out.push_str(indent);
        }
    };
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' => {
                out.push(c);
                i += 1;
                while i < chars.len() {
                    out.push(chars[i]);
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        i += 1;
                        out.push(chars[i]);
                    } else if chars[i] == '"' {
                        break;
                    } else if chars[i] == '\n' {
                        return Err("unterminated string".into());
                    }
                    i += 1;
                }
                i += 1;
            }
            '/' if chars.get(i + 1) == Some(&'/') => {
                // Line comment: stays on its own line (with one space if it started on the same line as earlier content).
                if !out.is_empty() && !out.ends_with('\n') && !out.ends_with(' ') && !out.trim_end_matches([' ', '\t']).ends_with('\n') {
                    out.push(' ');
                }
                while i < chars.len() && chars[i] != '\n' {
                    out.push(chars[i]);
                    i += 1;
                }
                newline(&mut out, depth);
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                let end = (i + 2..chars.len().saturating_sub(1)).find(|&k| chars[k] == '*' && chars[k + 1] == '/').ok_or("unterminated comment")?;
                out.extend(&chars[i..end + 2]);
                i = end + 2;
            }
            '{' | '[' => {
                // Empty container: "{}" / "[]" stay on one line.
                let close = if c == '{' { '}' } else { ']' };
                let mut k = i + 1;
                while k < chars.len() && chars[k].is_whitespace() {
                    k += 1;
                }
                out.push(c);
                if chars.get(k) == Some(&close) {
                    out.push(close);
                    i = k + 1;
                } else {
                    // The output grows with depth × lines: refuse what no real file has, before it eats the memory.
                    if depth >= MAX_DEPTH {
                        return Err(format!("nested deeper than {MAX_DEPTH} levels"));
                    }
                    stack.push(c);
                    depth += 1;
                    newline(&mut out, depth);
                    i += 1;
                }
            }
            '}' | ']' => {
                let want = if c == '}' { '{' } else { '[' };
                if stack.pop() != Some(want) {
                    return Err(format!("unexpected '{c}'"));
                }
                depth = depth.saturating_sub(1);
                newline(&mut out, depth);
                out.push(c);
                i += 1;
            }
            ',' => {
                out.push(',');
                newline(&mut out, depth);
                i += 1;
                // Trailing comma (JSONC): if a closer follows, don't leave an empty line (the closer opens a line anyway).
            }
            ':' => {
                out.push_str(": ");
                i += 1;
            }
            c if c.is_whitespace() => {
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    if !stack.is_empty() {
        return Err("unclosed bracket".into());
    }
    // Blank lines (including those opened after a trailing comma) and trailing whitespace are cleaned up.
    let mut cleaned = String::with_capacity(out.len());
    for line in out.lines().filter(|l| !l.trim().is_empty()) {
        cleaned.push_str(line.trim_end());
        cleaned.push('\n');
    }
    Ok(cleaned.trim_start_matches('\n').to_string())
}

/// Squash onto a single line (comments are dropped).
pub fn minify(text: &str) -> Result<String, String> {
    let chars: Vec<char> = text.chars().collect();
    let (mut out, mut i, mut depth) = (String::new(), 0, 0i32);
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' => {
                out.push(c);
                i += 1;
                while i < chars.len() {
                    out.push(chars[i]);
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        i += 1;
                        out.push(chars[i]);
                    } else if chars[i] == '"' {
                        break;
                    }
                    i += 1;
                }
                i += 1;
            }
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                let end = (i + 2..chars.len().saturating_sub(1)).find(|&k| chars[k] == '*' && chars[k + 1] == '/').ok_or("unterminated comment")?;
                i = end + 2;
            }
            c if c.is_whitespace() => i += 1,
            '{' | '[' => {
                depth += 1;
                out.push(c);
                i += 1;
            }
            '}' | ']' => {
                depth -= 1;
                if depth < 0 {
                    return Err(format!("unexpected '{c}'"));
                }
                // Drop the trailing comma.
                if out.ends_with(',') {
                    out.pop();
                }
                out.push(c);
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    if depth != 0 {
        return Err("unclosed bracket".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absurd_nesting_is_refused_not_expanded() {
        let src = format!("{}{}", "[".repeat(20_000), "]".repeat(20_000));
        assert!(format(&src, "  ").unwrap_err().contains("nested deeper"));
        let fine = format!("{}1{}", "[".repeat(100), "]".repeat(100));
        assert!(format(&fine, "  ").is_ok());
    }

    #[test]
    fn formats_nested_values_and_keeps_key_order() {
        let src = r#"{"b":1,"a":[1,2,{"x":null}],"c":{}, "d":[]}"#;
        let want = "{\n  \"b\": 1,\n  \"a\": [\n    1,\n    2,\n    {\n      \"x\": null\n    }\n  ],\n  \"c\": {},\n  \"d\": []\n}\n";
        assert_eq!(format(src, "  ").unwrap(), want);
        assert_eq!(format(want, "  ").unwrap(), want, "a second pass gives the same result");
        assert_eq!(format(r#"{"a":1}"#, "\t").unwrap(), "{\n\t\"a\": 1\n}\n");
    }

    #[test]
    fn strings_and_comments_survive() {
        let src = "{ // başlık\n \"url\": \"http://x.io/a,b:{c}\", /* not */ \"k\": \"q\\\"uote\" }";
        let out = format(src, "  ").unwrap();
        assert!(out.contains("\"http://x.io/a,b:{c}\""), "{out}");
        assert!(out.contains("// başlık") && out.contains("/* not */"), "{out}");
        assert!(out.contains("\"q\\\"uote\""));
        assert!(serde_json::from_str::<serde_json::Value>(&theme::themes::strip_jsonc(&out)).is_ok(), "{out}");
    }

    #[test]
    fn trailing_commas_and_errors() {
        let out = format("[1,2,]", "  ").unwrap();
        assert_eq!(out, "[\n  1,\n  2,\n]\n");
        assert!(format("{\"a\":1", "  ").is_err());
        assert!(format("{\"a\":1]", "  ").is_err());
        assert!(format("{\"a\":\"x\n}", "  ").is_err());
        assert_eq!(format("", "  ").unwrap(), "");
    }

    #[test]
    fn minify_drops_whitespace_comments_and_trailing_commas() {
        assert_eq!(minify("{\n  \"a\": [1, 2, ], // x\n  \"b\": \"s p\"\n}").unwrap(), "{\"a\":[1,2],\"b\":\"s p\"}");
        assert!(minify("{").is_err());
    }
}

#[cfg(test)]
mod hardening {
    use super::*;

    #[test]
    fn random_input_never_panics_and_valid_json_stays_valid() {
        let alphabet = ["{", "}", "[", "]", ",", ":", "\"", "\\", "/", "//", "/*", "*/", " ", "\n", "1", "true", "null", "ş", "\"k\":", "\"v\""];
        let mut x = 99u64;
        for _ in 0..2000 {
            let n = 1 + (x % 40) as usize;
            let s: String = (0..n)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    alphabet[(x % alphabet.len() as u64) as usize]
                })
                .collect();
            let _ = format(&s, "  ");
            let _ = minify(&s);
        }
        let valid = r#"{"a":[1,2,{"b":"c\n\"d\""}],"e":null,"f":{"g":[]}}"#;
        let pretty = format(valid, "  ").unwrap();
        let (a, b): (serde_json::Value, serde_json::Value) = (serde_json::from_str(valid).unwrap(), serde_json::from_str(&pretty).unwrap());
        assert_eq!(a, b);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&minify(&pretty).unwrap()).unwrap(), a);
    }
}
