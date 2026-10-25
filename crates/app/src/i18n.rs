//! UI localization: the key is the English text itself (`t("Save")`); a translation is a `locales/<code>.tsv` file embedded
//! in the build. A missing key stays English. The scope is limited (see docs/user-guide.md): menu bar,
//! command palette labels, welcome screen, status bar, git confirmation dialogs, Source Control panel, Explorer, the frame of the Settings and
//! Keyboard Shortcuts pages, saving and file operations, and git branch/tag prompts.
//!
//! File format: one `English<TAB>Translation` per line; lines starting with `#` and blank lines are ignored; a line whose translation is left empty means "not translated yet" (skipped). Placeholders
//! may be `{}` (next argument) or `{0}`, `{1}`… (a specific argument); a translation may use a subset of the
//! arguments in the key (Turkish has no plural suffix).

use std::collections::HashMap;
use std::fmt::{Display, Write as _};
use std::sync::OnceLock;

/// Plural rule: which plural form a number takes (CLDR). `forms()` is the number of `|`-separated forms expected in a translation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code, reason = "rules are ready for languages not yet shipped; tested with CLDR examples (docs/translating.md)")]
pub enum Plural {
    /// No plural suffix (tr, ja, zh, ko, vi, id): a single form.
    None,
    /// 1 → first, everything else → second (en, de, es, it, nl, pt-PT, sv…).
    OneOther,
    /// 0 and 1 → first, others → second (fr, pt-BR).
    French,
    /// ru, uk, be: 1, 21, 31… → first; 2-4, 22-24… → second; the rest → third.
    EastSlavic,
    /// pl: 1 → first; 2-4, 22-24… (except 12-14) → second; the rest → third.
    Polish,
}

impl Plural {
    /// Number of forms expected in a translation.
    #[allow(dead_code, reason = "read by tests that validate the translation table")]
    pub const fn forms(self) -> usize {
        match self {
            Plural::None => 1,
            Plural::OneOther | Plural::French => 2,
            Plural::EastSlavic | Plural::Polish => 3,
        }
    }

    /// Index of the form for `n` (`0..forms()`).
    pub const fn index(self, n: usize) -> usize {
        match self {
            Plural::None => 0,
            Plural::OneOther => (n != 1) as usize,
            Plural::French => (n > 1) as usize,
            Plural::EastSlavic => {
                if n % 10 == 1 && n % 100 != 11 {
                    0
                } else if matches!(n % 10, 2..=4) && !matches!(n % 100, 12..=14) {
                    1
                } else {
                    2
                }
            }
            Plural::Polish => {
                if n == 1 {
                    0
                } else if matches!(n % 10, 2..=4) && !matches!(n % 100, 12..=14) {
                    1
                } else {
                    2
                }
            }
        }
    }
}

/// How well a language's translation is maintained: determines what the tests enforce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// The language of the keys (English only).
    Source,
    /// **Every** text marked with `t()` in code and found in the palette, menus and tooltips has a translation; tests enforce this,
    /// so a change that adds new text must update this language too.
    Complete,
    /// Partial translation: file format, placeholders and dead keys are checked, but a missing key is not an error (it stays English).
    #[allow(dead_code, reason = "the first community language will be added as partial; tried with a fake language in tests")]
    Partial,
}

/// A UI language. To add a new language see docs/translating.md.
pub struct Language {
    /// Code matching the `ui.language` value and the system language tag (`tr`, `pt-BR`, `zh-Hans`): compared in lowercase.
    pub code: &'static str,
    /// The language's own name (in docs and the translation status report).
    #[allow(dead_code, reason = "read by tests and the docs/translating.md table")]
    pub name: &'static str,
    /// Embedded translation files (none for English); multiple parts are merged, a duplicate key is an error.
    pub tables: &'static [&'static str],
    pub plural: Plural,
    /// Rule used when uppercasing headings (Turkish `i` → `İ`).
    pub case: crate::textcase::CaseLocale,
    #[allow(dead_code, reason = "tests read from here which checks are mandatory")]
    pub status: Status,
}

/// Supported languages; the order is the `current()` index and 0 is always English (the language of the keys).
pub const LANGUAGES: &[Language] = &[
    Language { code: "en", name: "English", tables: &[], plural: Plural::OneOther, case: crate::textcase::CaseLocale::Invariant, status: Status::Source },
    Language {
        code: "tr",
        name: "Türkçe",
        tables: &[include_str!("../locales/tr.tsv")],
        plural: Plural::None,
        case: crate::textcase::CaseLocale::Turkish,
        status: Status::Complete,
    },
];

/// Valid values for `ui.language`: `auto` and the `LANGUAGES` codes (a test verifies it matches `LANGUAGES`).
pub const SETTING_CHOICES: &[&str] = &["auto", "en", "tr"];

#[cfg(not(test))]
static CURRENT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
// Tests run in parallel, each on its own thread: the language is per-thread (default English).
#[cfg(test)]
thread_local! {
    static CURRENT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// `LANGUAGES` index of the current language.
pub fn current() -> usize {
    #[cfg(not(test))]
    return CURRENT.load(std::sync::atomic::Ordering::Relaxed);
    #[cfg(test)]
    return CURRENT.with(|c| c.get());
}

/// Changes the language; `true` if it changed (menus need to be rebuilt).
pub fn set(lang: usize) -> bool {
    let lang = if lang < LANGUAGES.len() { lang } else { 0 };
    let changed = current() != lang;
    #[cfg(not(test))]
    CURRENT.store(lang, std::sync::atomic::Ordering::Relaxed);
    #[cfg(test)]
    CURRENT.with(|c| c.set(lang));
    changed
}

/// Resolves the `ui.language` value and the system language list (`system_locales`) and applies the language; `true` if it changed.
pub fn apply_setting(setting: &str) -> bool {
    set(resolve(setting, &system_locales()))
}

/// `ui.language` → language index. `auto`: the first supported language in the system language list (English if none).
pub fn resolve(setting: &str, locales: &[String]) -> usize {
    resolve_in(&language_codes(), setting, locales)
}

fn language_codes() -> Vec<&'static str> {
    LANGUAGES.iter().map(|l| l.code).collect()
}

/// `resolve` over a given code list (tests try regional matching with fake lists).
pub fn resolve_in(codes: &[&str], setting: &str, locales: &[String]) -> usize {
    if let Some(ix) = codes.iter().position(|c| *c == setting) {
        return ix;
    }
    locales.iter().find_map(|l| language_of_in(codes, l)).unwrap_or(0)
}

/// The most specific match wins: "pt-BR" looks at `pt-br` first, then `pt` ("zh-Hant-TW" → `zh-hant-tw`, `zh-hant`, `zh`).
/// Code comparison ignores case and the `_`/`-` distinction; `trx` does not match `tr`.
pub fn language_of_in(codes: &[&str], locale: &str) -> Option<usize> {
    let tag = locale.trim().split(['.', '@']).next().unwrap_or("").replace('_', "-").to_ascii_lowercase();
    if tag == "c" || tag == "posix" {
        return Some(0);
    }
    let mut candidate = tag.as_str();
    loop {
        if !candidate.is_empty()
            && let Some(ix) = codes.iter().position(|c| c.replace('_', "-").eq_ignore_ascii_case(candidate))
        {
            return Some(ix);
        }
        candidate = candidate.rsplit_once('-')?.0;
    }
}

/// Language list in the system's order of preference. macOS: the system language list (`NSLocale`); Windows: the user locale
/// name; others (and the fallback on macOS): `LC_ALL`, `LC_MESSAGES`, `LANG`, `LANGUAGE`. Empty in tests (hardware independent).
pub fn system_locales() -> Vec<String> {
    if cfg!(test) {
        return Vec::new();
    }
    // Only the macOS and Windows branches below push to it; on other targets it stays empty.
    #[allow(unused_mut)]
    let mut out: Vec<String> = Vec::new();
    #[cfg(target_os = "macos")]
    out.extend(macos_languages());
    #[cfg(windows)]
    out.extend(windows_locale());
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(var)
            && !v.trim().is_empty()
        {
            out.push(v);
            break;
        }
    }
    if let Ok(v) = std::env::var("LANGUAGE") {
        out.extend(v.split(':').filter(|s| !s.is_empty()).map(str::to_string));
    }
    out
}

#[cfg(target_os = "macos")]
fn macos_languages() -> Vec<String> {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    let mut out = Vec::new();
    // SAFETY: `+[NSLocale preferredLanguages]` returns an autoreleased NSArray of NSString (UTF-8 convertible).
    unsafe {
        let list: *mut AnyObject = msg_send![class!(NSLocale), preferredLanguages];
        if list.is_null() {
            return out;
        }
        let n: usize = msg_send![list, count];
        for i in 0..n.min(16) {
            let s: *mut AnyObject = msg_send![list, objectAtIndex: i];
            if s.is_null() {
                continue;
            }
            let utf8: *const std::os::raw::c_char = msg_send![s, UTF8String];
            if !utf8.is_null() {
                out.push(std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned());
            }
        }
    }
    out
}

#[cfg(windows)]
fn windows_locale() -> Option<String> {
    unsafe extern "system" {
        fn GetUserDefaultLocaleName(name: *mut u16, len: i32) -> i32;
    }
    let mut buf = [0u16; 85]; // LOCALE_NAME_MAX_LENGTH
    // SAFETY: the buffer length is passed; the function writes at most that many UTF-16 units.
    let n = unsafe { GetUserDefaultLocaleName(buf.as_mut_ptr(), buf.len() as i32) };
    (n > 1).then(|| String::from_utf16_lossy(&buf[..(n as usize - 1)]))
}

type Table = HashMap<&'static str, &'static str>;

/// Parses a translation file: `(key, translation)` pairs and error messages for duplicate/malformed lines.
pub fn parse_table(tsv: &'static str) -> (Vec<(&'static str, &'static str)>, Vec<String>) {
    let (mut pairs, mut errors) = (Vec::new(), Vec::new());
    for (n, line) in tsv.lines().enumerate() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        match line.split_once('\t') {
            // `English<TAB>` (empty translation) = not translated yet (this is how `template.tsv` looks): skipped, the key stays English.
            // So a `Complete` language fails the coverage test, a `Partial` one does not.
            Some((k, v)) if !k.is_empty() && v.trim().is_empty() => {}
            Some((k, v)) if !k.is_empty() && !v.contains('\t') => pairs.push((k, v)),
            _ => errors.push(format!("line {}: expected `English<TAB>Translation`", n + 1)),
        }
    }
    let mut seen = std::collections::HashSet::new();
    for (k, _) in &pairs {
        if !seen.insert(*k) {
            errors.push(format!("duplicate key “{k}”"));
        }
    }
    (pairs, errors)
}

/// Parses all translation parts of a language; a duplicate key across parts is also an error.
pub fn parse_tables(parts: &[&'static str]) -> (Vec<(&'static str, &'static str)>, Vec<String>) {
    let (mut pairs, mut errors) = (Vec::new(), Vec::new());
    let mut seen = std::collections::HashSet::new();
    for (i, part) in parts.iter().enumerate() {
        let (p, e) = parse_table(part);
        errors.extend(e.into_iter().map(|e| if parts.len() > 1 { format!("part {}: {e}", i + 1) } else { e }));
        for (k, v) in p {
            if seen.insert(k) {
                pairs.push((k, v));
            } else if parts.len() > 1 {
                errors.push(format!("duplicate key “{k}” across parts"));
            }
        }
    }
    (pairs, errors)
}

fn table(lang: usize) -> Option<&'static Table> {
    static TABLES: [OnceLock<Table>; LANGUAGES.len()] = [const { OnceLock::new() }; LANGUAGES.len()];
    let l = LANGUAGES.get(lang)?;
    if l.tables.is_empty() {
        return None;
    }
    Some(TABLES[lang].get_or_init(|| parse_tables(l.tables).0.into_iter().collect()))
}

/// The translation of the key in language `lang`; `None` if absent.
pub fn lookup(lang: usize, key: &str) -> Option<&'static str> {
    table(lang)?.get(key).copied()
}

/// Translates the English text into the current language; if there is no translation the text itself is returned.
pub fn t(key: &str) -> &str {
    if current() == 0 {
        return key;
    }
    lookup(current(), key).unwrap_or(key)
}

/// Marks a translation key and returns the text as is: when the text will be translated at render time (e.g. a field's
/// placeholder, updated when the language changes), having the key appear in the source as `tr("…")` lets the tests find it.
pub const fn tr(key: &'static str) -> &'static str {
    key
}

/// Of the `|`-separated forms in a translation, the one matching `n`; `None` if the form is missing.
pub fn pick_form(plural: Plural, translated: &str, n: usize) -> Option<&str> {
    translated.split('|').nth(plural.index(n))
}

/// Plural: `tn("{} file", "{} files", n, &[])`. In the translation table the key is the two English forms joined with `|`
/// (`{} file|{} files`), and the translation is the language's plural forms joined with `|` (Russian 3, Turkish 1: `{} dosya`). The first
/// placeholder (`{0}`, or the first `{}`) is `n`, and `args` come after it. If there is no translation, the English rule (`n == 1` is singular) is used.
pub fn tn(one: &str, other: &str, n: usize, args: &[&dyn Display]) -> String {
    let mut all: Vec<&dyn Display> = Vec::with_capacity(args.len() + 1);
    all.push(&n);
    all.extend_from_slice(args);
    if current() != 0
        && let Some(translated) = lookup(current(), &format!("{one}|{other}"))
    {
        // The form count is checked in the table (test); on a malformed row it falls back to English instead of the last form.
        if let Some(form) = pick_form(LANGUAGES[current()].plural, translated, n) {
            return fill(form, &all);
        }
    }
    fill(if Plural::OneOther.index(n) == 0 { one } else { other }, &all)
}

/// `t` + placeholders: `tf("Delete branch {}?", &[&name])`. If no translation is found the English text is filled in.
pub fn tf(key: &str, args: &[&dyn Display]) -> String {
    fill(t(key), args)
}

/// Fills the `{}` and `{N}` placeholders; extra/missing arguments are silently ignored.
pub fn fill(template: &str, args: &[&dyn Display]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut next = 0;
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) if after[..close].chars().all(|c| c.is_ascii_digit()) => {
                let spec = &after[..close];
                let ix = if spec.is_empty() {
                    next += 1;
                    next - 1
                } else {
                    spec.parse().unwrap_or(usize::MAX)
                };
                if let Some(a) = args.get(ix) {
                    let _ = write!(out, "{a}");
                }
                rest = &after[close + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Uppercases headings with the language's rule; in Turkish `i` → `İ`, `ı` → `I`.
pub fn upper(s: &str) -> String {
    match LANGUAGES.get(current()).map(|l| l.case) {
        Some(case @ crate::textcase::CaseLocale::Turkish) => crate::textcase::upper(s, case),
        _ => s.to_uppercase(),
    }
}

/// Argument indices of the placeholders in the text (`{}` counted in order; for tests).
#[cfg(test)]
pub fn placeholders(s: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut next = 0;
    let mut rest = s;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) if after[..close].chars().all(|c| c.is_ascii_digit()) => {
                let spec = &after[..close];
                out.push(if spec.is_empty() {
                    next += 1;
                    next - 1
                } else {
                    spec.parse().unwrap_or(usize::MAX)
                });
                rest = &after[close + 1..];
            }
            _ => rest = after,
        }
    }
    out
}

/// Temporarily changes the language in tests; reverts to English on drop (thread-local, so it does not affect parallel tests).
#[cfg(test)]
pub struct LangGuard;

#[cfg(test)]
impl LangGuard {
    pub fn set(code: &str) -> Self {
        set(resolve(code, &[]));
        LangGuard
    }
}

#[cfg(test)]
impl Drop for LangGuard {
    fn drop(&mut self) {
        set(0);
    }
}
