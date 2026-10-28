//! Decode the generated test command back to argv without evaluating shell syntax.

pub fn selection(command: &str) -> Result<Vec<String>, String> {
    let words = words(command).ok_or_else(|| crate::i18n::t("Unterminated quote in the Cargo test command").to_string())?;
    if words.first().map(String::as_str) != Some("cargo") || words.get(1).map(String::as_str) != Some("test") {
        return Err(crate::i18n::t("Expected a cargo test command").to_string());
    }
    let mut words = words.into_iter().skip(2);
    let mut args = Vec::new();
    while let Some(word) = words.next() {
        if word == "--" {
            break;
        }
        let (flag, inline) = word.split_once('=').map_or((word.as_str(), None), |(f, v)| (f, Some(v)));
        if matches!(
            flag,
            "-p" | "--package"
                | "--test"
                | "--bin"
                | "--example"
                | "--bench"
                | "--manifest-path"
                | "--features"
                | "-F"
                | "--target"
                | "--target-dir"
                | "--profile"
        ) {
            let value = inline.map(str::to_string).or_else(|| words.next()).ok_or_else(|| crate::i18n::tf("Missing value for Cargo option {}", &[&flag]))?;
            args.extend([flag.to_string(), value]);
        } else if matches!(
            flag,
            "--lib"
                | "--bins"
                | "--tests"
                | "--examples"
                | "--benches"
                | "--workspace"
                | "--all-features"
                | "--no-default-features"
                | "--release"
                | "--locked"
                | "--offline"
                | "--frozen"
        ) {
            args.push(flag.to_string());
        }
    }
    Ok(args)
}

fn words(text: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut started = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), c) => word.push(c),
            (_, '\\') => {
                let next = chars.next()?;
                if quote == Some('"') && !matches!(next, '$' | '`' | '"' | '\\' | '\n') {
                    word.push('\\');
                }
                if next != '\n' {
                    word.push(next);
                }
                started = true;
            }
            (None, '\'' | '"') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (_, c) => {
                word.push(c);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    if started {
        out.push(word);
    }
    Some(out)
}
