//! Searching files: a parallel walk honoring .gitignore + `regex::bytes`.
//! Results are collected per file and sent in chunks; cancellable.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, mpsc};

use proto::{Error, FileMatches, LineMatch, SearchQuery};
use regex::bytes::{Regex, RegexBuilder};

/// Larger files are not searched (minified bundles, logs, data files).
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// The preview is limited to this many characters; the match is centered.
const PREVIEW_CHARS: usize = 240;

pub struct Summary {
    pub files: u32,
    pub matches: u32,
    pub truncated: bool,
    pub cancelled: bool,
}

/// Literal pattern. When case-insensitive, i/ı/İ/I are equivalent: Unicode's simple folding
/// doesn't map "İ" to "i", so in Turkish text a search for "iğne" would miss "İĞNE".
fn literal(pattern: &str, case_sensitive: bool) -> String {
    if case_sensitive {
        return regex::escape(pattern);
    }
    pattern
        .chars()
        .map(|c| match c {
            'i' | 'I' | 'ı' | 'İ' => "[iIıİ]".to_string(),
            c => regex::escape(c.encode_utf8(&mut [0; 4])),
        })
        .collect()
}

pub fn compile(q: &SearchQuery) -> Result<Regex, Error> {
    let mut pattern = if q.regex { q.pattern.clone() } else { literal(&q.pattern, q.case_sensitive) };
    if q.whole_word {
        // In plain text a boundary is added only on an edge that ends in a word character (VS Code: "foo(" can also
        // be searched as a whole word; \b would never match after the trailing "(").
        let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        let (head, tail) = if q.regex { (true, true) } else { (word(q.pattern.chars().next()), word(q.pattern.chars().next_back())) };
        pattern = format!("{}(?:{pattern}){}", if head { r"\b" } else { "" }, if tail { r"\b" } else { "" });
    }
    RegexBuilder::new(&pattern)
        .case_insensitive(!q.case_sensitive)
        .multi_line(true)
        .size_limit(10 * 1024 * 1024)
        .build()
        .map_err(|e| Error::InvalidPattern(e.to_string()))
}

/// Search; each file's results are sent via `emit`. `cancel` is checked per file.
pub fn run(q: &SearchQuery, root: &Path, cancel: &AtomicBool, mut emit: impl FnMut(FileMatches)) -> Result<Summary, Error> {
    if q.pattern.is_empty() {
        return Ok(Summary { files: 0, matches: 0, truncated: false, cancelled: false });
    }
    let re = Arc::new(compile(q)?);
    // Single file (find in the big-file view): no size limit, read line by line; a 500 MB
    // log is not loaded into memory (memory is bounded by the longest line).
    if root.is_file() {
        return search_one_file(&re, root, q.max_results, cancel, emit);
    }
    let mut overrides = ignore::overrides::OverrideBuilder::new(root);
    for g in q.include.iter().filter(|g| !g.trim().is_empty()) {
        overrides.add(g.trim()).map_err(|e| Error::InvalidPattern(e.to_string()))?;
    }
    for g in q.exclude.iter().filter(|g| !g.trim().is_empty()) {
        overrides.add(&format!("!{}", g.trim())).map_err(|e| Error::InvalidPattern(e.to_string()))?;
    }
    let overrides = overrides.build().map_err(|e| Error::InvalidPattern(e.to_string()))?;

    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .overrides(overrides)
        .threads(threads)
        .filter_entry(|e| !matches!(e.file_name().to_str(), Some(".git" | "node_modules")))
        .build_parallel();

    let max = q.max_results;
    let total = AtomicU32::new(0);
    let files = AtomicU32::new(0);
    let (tx, rx) = mpsc::channel::<FileMatches>();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            walker.run(|| {
                let tx = tx.clone();
                let re = re.clone();
                let (total, files) = (&total, &files);
                Box::new(move |entry| {
                    use ignore::WalkState;
                    if cancel.load(Ordering::Relaxed) || total.load(Ordering::Relaxed) >= max {
                        return WalkState::Quit;
                    }
                    let Ok(entry) = entry else { return WalkState::Continue };
                    if !entry.file_type().is_some_and(|t| t.is_file()) {
                        return WalkState::Continue;
                    }
                    if entry.metadata().is_ok_and(|m| m.len() > MAX_FILE_BYTES) {
                        return WalkState::Continue;
                    }
                    let Ok(data) = std::fs::read(entry.path()) else { return WalkState::Continue };
                    files.fetch_add(1, Ordering::Relaxed);
                    if data[..data.len().min(8192)].contains(&0) {
                        return WalkState::Continue; // binary file
                    }
                    let mut matches = search_bytes(&re, &data, max.saturating_sub(total.load(Ordering::Relaxed)) as usize);
                    if matches.is_empty() {
                        return WalkState::Continue;
                    }
                    // Reserve the allowance atomically: workers that all read the same total would each emit up to
                    // `max`, and a slow client would have to queue `workers * max` results.
                    let n: u32 = matches.iter().map(|m| m.ranges.len() as u32).sum();
                    let before = total.fetch_add(n, Ordering::Relaxed);
                    if before >= max {
                        total.fetch_sub(n, Ordering::Relaxed);
                        return WalkState::Quit;
                    }
                    if before + n > max {
                        let allowed = max - before;
                        let mut kept = 0u32;
                        matches.retain(|m| {
                            let take = kept < allowed;
                            if take {
                                kept += (m.ranges.len() as u32).min(allowed - kept);
                            }
                            take
                        });
                        total.fetch_sub(n - kept, Ordering::Relaxed);
                    }
                    let rel = entry.path().strip_prefix(root).unwrap_or(entry.path());
                    let _ = tx.send(FileMatches { path: crate::relative_path(rel), matches });
                    WalkState::Continue
                })
            });
            drop(tx);
        });
        for fm in rx {
            emit(fm);
        }
    });
    let matches = total.load(Ordering::Relaxed);
    Ok(Summary { files: files.load(Ordering::Relaxed), matches, truncated: matches >= max, cancelled: cancel.load(Ordering::Relaxed) })
}

fn search_one_file(re: &Regex, path: &Path, max: u32, cancel: &AtomicBool, mut emit: impl FnMut(FileMatches)) -> Result<Summary, Error> {
    use std::io::BufRead;
    let file = std::fs::File::open(path).map_err(|e| Error::NotFound(format!("{}: {e}", path.display())))?;
    let mut reader = std::io::BufReader::with_capacity(256 * 1024, file);
    let mut line = Vec::new();
    let (mut matches, mut count, mut line_no) = (Vec::new(), 0u32, 0u32);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // Results are sent in chunks: the first matches show up without waiting for the end of the file.
    let mut flush = |matches: &mut Vec<LineMatch>| {
        if !matches.is_empty() {
            emit(FileMatches { path: name.clone(), matches: std::mem::take(matches) });
        }
    };
    loop {
        if count >= max || (line_no % 4096 == 0 && cancel.load(Ordering::Relaxed)) {
            break;
        }
        line.clear();
        if reader.read_until(b'\n', &mut line).map_err(|e| Error::Other(e.to_string()))? == 0 {
            break;
        }
        line_no += 1;
        if line_no == 1 && line[..line.len().min(8192)].contains(&0) {
            break; // binary file
        }
        let text = line.strip_suffix(b"\n").unwrap_or(&line);
        let text = text.strip_suffix(b"\r").unwrap_or(text);
        let ranges: Vec<(usize, usize)> = re.find_iter(text).map(|m| (m.start(), m.end())).filter(|(a, b)| b > a).collect();
        if ranges.is_empty() {
            continue;
        }
        count += ranges.len() as u32;
        matches.push(preview(line_no, text, &ranges));
        if matches.len() >= 500 {
            flush(&mut matches);
        }
    }
    flush(&mut matches);
    Ok(Summary { files: 1, matches: count, truncated: count >= max, cancelled: cancel.load(Ordering::Relaxed) })
}

/// Search line by line within a file.
pub fn search_bytes(re: &Regex, data: &[u8], limit: usize) -> Vec<LineMatch> {
    let mut out = Vec::new();
    let mut count = 0;
    for (ix, line) in data.split(|&b| b == b'\n').enumerate() {
        if count >= limit {
            break;
        }
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let ranges: Vec<(usize, usize)> = re.find_iter(line).map(|m| (m.start(), m.end())).filter(|(a, b)| b > a).collect();
        if ranges.is_empty() {
            continue;
        }
        count += ranges.len();
        out.push(preview(ix as u32 + 1, line, &ranges));
    }
    out
}

/// Where byte `off` of `line` lands in `String::from_utf8_lossy(line)`. An offset inside an invalid run maps to the start of
/// its replacement character, or to the end of it when `end` (so a range keeps covering what it covered).
fn lossy_offset(line: &[u8], off: usize, end: bool) -> usize {
    const REPLACEMENT: usize = '\u{FFFD}'.len_utf8();
    let (mut raw, mut lossy) = (0, 0);
    for chunk in line.utf8_chunks() {
        let valid = chunk.valid().len();
        // `<=`: an offset right at the end of the valid text is where a replacement character would start, so a range that ends
        // there (`ab` before an invalid byte) must not be stretched over the replacement.
        if off <= raw + valid {
            return lossy + (off - raw);
        }
        raw += valid;
        lossy += valid;
        let invalid = chunk.invalid().len();
        if invalid > 0 {
            if off < raw + invalid {
                return lossy + if end { REPLACEMENT } else { 0 };
            }
            raw += invalid;
            lossy += REPLACEMENT;
        }
    }
    lossy
}

/// Trim a long line around the first match; shift the ranges to match the preview.
fn preview(line_no: u32, line: &[u8], ranges: &[(usize, usize)]) -> LineMatch {
    let text = String::from_utf8_lossy(line);
    // The ranges are offsets into `line`; lossy decoding replaces each invalid run with a 3-byte U+FFFD, which moves
    // everything after it. Without the remap, offsets land inside a character and slicing `text` panics.
    let remapped: Vec<(usize, usize)>;
    let ranges = if matches!(text, std::borrow::Cow::Borrowed(_)) {
        ranges
    } else {
        remapped = ranges.iter().map(|&(a, b)| (lossy_offset(line, a, false), lossy_offset(line, b, true))).collect();
        &remapped
    };
    // A byte pattern (`(?-u:\xa9)`) can match inside a character: widen the range to the whole character, so that the client
    // can slice the preview with it.
    let ranges: Vec<(usize, usize)> = ranges.iter().map(|&(a, b)| (text.floor_char_boundary(a), text.ceil_char_boundary(b))).collect();
    let column = text[..text.floor_char_boundary(ranges[0].0)].chars().count() as u32;
    // Drop leading whitespace (so indentation doesn't take up preview space).
    let indent = text.len() - text.trim_start().len();
    let mut start = indent.min(ranges[0].0);
    let mut end = text.len();
    if text[start..].chars().count() > PREVIEW_CHARS {
        // Leave at most 40 characters before the first match.
        let before: usize = text[start..ranges[0].0.min(text.len())].chars().rev().take(40).map(char::len_utf8).sum();
        start = ranges[0].0.saturating_sub(before).max(start);
        start = text.floor_char_boundary(start);
        let after: usize = text[start..].chars().take(PREVIEW_CHARS).map(char::len_utf8).sum();
        end = start + after;
    }
    let preview = text[start..end].to_string();
    let ranges = ranges.iter().filter(|(a, b)| *a >= start && *b <= end).map(|(a, b)| ((a - start) as u32, (b - start) as u32)).collect();
    LineMatch { line: line_no, preview, ranges, column }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(pattern: &str) -> SearchQuery {
        SearchQuery {
            root: String::new(),
            pattern: pattern.into(),
            regex: false,
            case_sensitive: false,
            whole_word: false,
            include: vec![],
            exclude: vec![],
            max_results: 1000,
        }
    }

    #[test]
    fn a_single_big_file_is_streamed_without_the_size_cap() {
        let dir = std::env::temp_dir().join(format!("codeyard-agent-onefile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("büyük.log");
        // A file over 5 MB (skipped in folder search), the line to find is at the end.
        let mut text = "INFO sıradan satır\r\n".repeat(260_000);
        text.push_str("ERROR bozuldu\n");
        std::fs::write(&f, &text).unwrap();
        assert!(std::fs::metadata(&f).unwrap().len() > MAX_FILE_BYTES);
        let mut got = Vec::new();
        let sum = run(&q("error"), &f, &AtomicBool::new(false), |fm| got.push(fm)).unwrap();
        assert_eq!((sum.files, sum.matches, sum.truncated), (1, 1, false));
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].path.as_str(), got[0].matches[0].line, got[0].matches[0].preview.as_str()), ("büyük.log", 260_001, "ERROR bozuldu"));
        // Limit: at most max_results; chunks of 500 lines.
        let mut many = q("info");
        many.max_results = 1200;
        let mut chunks = Vec::new();
        let sum = run(&many, &f, &AtomicBool::new(false), |fm| chunks.push(fm.matches.len())).unwrap();
        assert!(sum.truncated && sum.matches == 1200, "{} matches", sum.matches);
        assert_eq!(chunks, [500, 500, 200]);
        // Folder search still skips the same file (size limit).
        let sum = run(&q("error"), &dir, &AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!(sum.matches, 0);
    }

    #[test]
    fn literal_is_escaped_and_case_insensitive() {
        let re = compile(&q("a.b(")).unwrap();
        let m = search_bytes(&re, b"x\n  A.B( ve a.b(\naxb(\n", 100);
        assert_eq!(m.len(), 1);
        assert_eq!((m[0].line, m[0].preview.as_str()), (2, "A.B( ve a.b("));
        assert_eq!(m[0].ranges, vec![(0, 4), (8, 12)]);
        assert_eq!(m[0].column, 2);
    }

    #[test]
    fn whole_word_regex_and_invalid_pattern() {
        let mut query = q("ad");
        query.whole_word = true;
        let re = compile(&query).unwrap();
        assert_eq!(search_bytes(&re, b"ad adres yeni_ad ad\n", 100)[0].ranges.len(), 2);
        let mut bad = q("(");
        bad.regex = true;
        assert!(matches!(compile(&bad), Err(Error::InvalidPattern(_))));
    }

    #[test]
    fn long_lines_are_clipped_around_the_match() {
        let line = format!("{}İĞNE{}", "ş".repeat(500), "x".repeat(500));
        let re = compile(&q("iğne")).unwrap();
        let m = &search_bytes(&re, line.as_bytes(), 10)[0];
        assert!(m.preview.chars().count() <= PREVIEW_CHARS);
        let (a, b) = m.ranges[0];
        assert_eq!(&m.preview[a as usize..b as usize], "İĞNE");
        assert_eq!(m.column, 500);
    }

    #[test]
    fn lossy_offsets_follow_the_replacement_characters() {
        // "ab", one invalid byte, "cd", a truncated 3-byte sequence (2 bytes, one replacement), "ef".
        let line = b"ab\xffcd\xe2\x82ef";
        assert_eq!(String::from_utf8_lossy(line), "ab\u{FFFD}cd\u{FFFD}ef");
        // (raw offset, start, end) in the lossy text; a replacement is 3 bytes.
        for (off, start, end) in [(0, 0, 0), (2, 2, 2), (3, 5, 5), (4, 6, 6), (5, 7, 7), (6, 7, 10), (7, 10, 10), (9, 12, 12)] {
            assert_eq!(lossy_offset(line, off, false), start, "start offset {off}");
            assert_eq!(lossy_offset(line, off, true), end, "end offset {off}");
        }
    }

    #[test]
    fn a_match_next_to_invalid_bytes_keeps_its_own_range() {
        let line = b"ab\xffcd";
        let re = compile(&q("ab")).unwrap();
        let m = &search_bytes(&re, line, 10)[0];
        let (a, b) = m.ranges[0];
        assert_eq!(&m.preview[a as usize..b as usize], "ab", "the range ends before the replacement character");
        let m = &search_bytes(&compile(&q("cd")).unwrap(), line, 10)[0];
        let (a, b) = m.ranges[0];
        assert_eq!(&m.preview[a as usize..b as usize], "cd");
        // A match that ends in the middle of an invalid run (here the first byte of a truncated 3-byte sequence) covers its whole replacement.
        let regex = SearchQuery { regex: true, ..q(r"(?-u:\xe2)") };
        let m = &search_bytes(&compile(&regex).unwrap(), b"x\xe2\x82y", 10)[0];
        let (a, b) = m.ranges[0];
        assert_eq!(&m.preview[a as usize..b as usize], "\u{FFFD}");
    }

    #[test]
    fn a_byte_pattern_matching_inside_a_character_keeps_the_preview_sliceable() {
        // `(?-u:\xa9)` matches the second byte of `é`: the range must not start or end inside the character, in a short
        // line and in a long one (which is clipped around the match).
        let regex = SearchQuery { regex: true, ..q(r"(?-u:\xa9)") };
        let re = compile(&regex).unwrap();
        for line in ["aéb".to_string(), format!("{}é{}", "ü".repeat(400), "z".repeat(400))] {
            let m = &search_bytes(&re, line.as_bytes(), 10)[0];
            for &(a, b) in &m.ranges {
                assert_eq!(&m.preview[a as usize..b as usize], "é", "{line:.12}");
            }
        }
    }

    #[test]
    fn invalid_utf8_before_a_match_does_not_panic_the_preview() {
        // Lossy decoding grows each bad byte to 3 bytes, so raw match offsets would land inside a U+FFFD.
        let mut line = vec![0xffu8; 300];
        line.extend_from_slice(b"needle");
        let re = compile(&q("needle")).unwrap();
        let m = &search_bytes(&re, &line, 10)[0];
        let (a, b) = m.ranges[0];
        assert_eq!(&m.preview[a as usize..b as usize], "needle");
        assert_eq!(m.column, 300);
    }

    #[test]
    fn walks_tree_with_ignore_include_and_cancel() {
        let d = std::env::temp_dir().join(format!("codeyard-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::write(d.join(".gitignore"), "gizli.txt\n").unwrap();
        std::fs::write(d.join("gizli.txt"), "hedef\n").unwrap();
        std::fs::write(d.join("src/a.rs"), "// hedef\nfn hedef() {}\n").unwrap();
        std::fs::write(d.join("notlar.md"), "hedef burada\n").unwrap();
        std::fs::write(d.join("resim.bin"), b"\x00\x01hedef").unwrap();

        let mut found = Vec::new();
        let s = run(&q("hedef"), &d, &AtomicBool::new(false), |fm| found.push(fm)).unwrap();
        found.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(found.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["notlar.md", "src/a.rs"]);
        assert_eq!(s.matches, 3);
        assert!(!s.truncated);

        let mut only_rs = q("hedef");
        only_rs.include = vec!["*.rs".into()];
        let mut found = Vec::new();
        run(&only_rs, &d, &AtomicBool::new(false), |fm| found.push(fm.path)).unwrap();
        assert_eq!(found, ["src/a.rs"]);

        let mut limited = q("hedef");
        limited.max_results = 1;
        let s = run(&limited, &d, &AtomicBool::new(false), |_| {}).unwrap();
        assert!(s.truncated);

        let s = run(&q("hedef"), &d, &AtomicBool::new(true), |_| panic!("no result expected once cancelled")).unwrap();
        assert!(s.cancelled);
    }

    /// `max_results` bounds what crosses the wire (and what a slow consumer can queue): the parallel walker must not let every
    /// worker take the whole allowance at once.
    #[test]
    fn parallel_workers_never_emit_more_than_max_results() {
        let d = std::env::temp_dir().join(format!("codeyard-search-cap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let body = "hedef\n".repeat(100);
        for i in 0..64 {
            std::fs::write(d.join(format!("f{i:02}.txt")), &body).unwrap();
        }
        for max in [1u32, 7, 150, 1000] {
            let mut query = q("hedef");
            query.max_results = max;
            let mut emitted = 0u32;
            let s = run(&query, &d, &AtomicBool::new(false), |fm| emitted += fm.matches.iter().map(|m| m.ranges.len() as u32).sum::<u32>()).unwrap();
            assert_eq!(emitted, max, "max_results {max}: emitted {emitted}");
            assert_eq!(s.matches, max);
            assert!(s.truncated);
        }
        // Below the cap nothing is cut.
        let mut query = q("hedef");
        query.max_results = 10_000;
        let mut emitted = 0u32;
        let sum = run(&query, &d, &AtomicBool::new(false), |fm| emitted += fm.matches.len() as u32).unwrap();
        assert_eq!((emitted, sum.matches, sum.truncated), (6400, 6400, false));
    }
}
