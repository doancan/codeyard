//! Text sizes come from the type scale (`theme::text`, from `crates/theme/design/tokens.json`), so the whole UI moves together
//! when the scale changes. The test fails when a UI text size is an absolute number (`.text_size(px(11.))`, or a number in an `if`
//! branch) or uses gpui's own `rems`-based sizes (`.text_sm()`, `rems(…)`), which bypass the scale.
//! Sizes derived from a setting (`font_size - 1.`, `size * 0.5`) are fine. A size that really must stay a number gets
//! `// scale-keep: reason` on the line.

use std::path::Path;

/// gpui's helpers size text in rems, not in our scale.
const REM_SINKS: &[&str] = &[".text_xs()", ".text_sm()", ".text_base()", ".text_lg()", ".text_xl()", "rems("];

/// The argument of the first `.text_size(px(` on the line, up to its closing bracket.
fn size_argument(line: &str) -> Option<&str> {
    let start = line.find(".text_size(px(")? + ".text_size(px(".len();
    let mut depth = 0usize;
    for (i, ch) in line[start..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' if depth == 0 => return Some(&line[start..start + i]),
            ')' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// Is the size an absolute number: the argument starts with one, or it is an `if` with a number branch (`{ 11. }`)?
fn is_absolute(arg: &str) -> bool {
    let arg = arg.trim();
    let digit = |s: &str| s.starts_with(|c: char| c.is_ascii_digit());
    digit(arg) || (arg.starts_with("if ") && arg.match_indices("{ ").any(|(at, _)| digit(&arg[at + 2..])))
}

/// Lines that break the rule: `(line number, line)`. Test modules at the end of a file are skipped.
fn violations(source: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (n, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("#[cfg(test)]") && source.lines().nth(n + 1).is_some_and(|l| l.trim_start().starts_with("mod ")) {
            break;
        }
        if trimmed.starts_with("//") || line.contains("scale-keep:") {
            continue;
        }
        if size_argument(line).is_some_and(is_absolute) || REM_SINKS.iter().any(|s| line.contains(s)) {
            out.push((n + 1, trimmed.to_string()));
        }
    }
    out
}

#[test]
fn ui_text_sizes_come_from_the_type_scale() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    for path in crate::i18n_tests::rs_files(&dir) {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if !name.ends_with(".rs") || name == "tests.rs" || name.ends_with("_tests.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        found.extend(violations(&text).into_iter().map(|(n, l)| format!("{name}:{n}: {l}")));
    }
    assert!(found.is_empty(), "text sizes outside the type scale (use theme::text::*, or mark with `// scale-keep: reason`):\n{}", found.join("\n"));
}

#[test]
fn the_scanner_tells_absolute_sizes_from_derived_ones() {
    let flagged = |s: &str| !violations(s).is_empty();
    assert!(flagged(".text_size(px(11.))"));
    assert!(flagged("d.text_size(px(if native { 11. } else { text::SM.0 }))"));
    assert!(flagged("div().text_sm().child(x)"));
    assert!(!flagged(".text_size(px(text::XXS.0))"));
    assert!(!flagged(".text_size(px(font_size - 1.))"));
    assert!(!flagged(".text_size(px(size * if long { 0.5 } else { 0.62 }))"), "a factor in a branch is still derived");
    assert!(!flagged(".text_size(px(12.)) // scale-keep: matches the system alert"));
    assert!(!flagged("#[cfg(test)]\nmod tests {\n    fn f() { d.text_size(px(9.)); }\n}"));
}
