//! Turning server responses into editor types: completions and snippets, locations,
//! symbols, edits, hints, lenses, semantic tokens.

use serde_json::Value;

use super::{Range16, path_to_uri, uri_to_path};

// --- Diagnostics ---

#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    /// (line, UTF-16 column), as the server sent it.
    pub start: (u32, u32),
    pub end: (u32, u32),
    /// 1 error, 2 warning, 3 information, 4 hint.
    pub severity: u8,
    pub message: String,
    pub source: Option<String>,
}

fn position(v: &Value) -> (u32, u32) {
    (v["line"].as_u64().unwrap_or(0) as u32, v["character"].as_u64().unwrap_or(0) as u32)
}

pub fn parse_range(v: &Value) -> Range16 {
    (position(&v["start"]), position(&v["end"]))
}

pub fn parse_diagnostics(params: &Value) -> Option<(String, Vec<Diagnostic>)> {
    let path = uri_to_path(params["uri"].as_str()?)?;
    let diags = params["diagnostics"]
        .as_array()?
        .iter()
        .map(|d| {
            let (start, end) = parse_range(&d["range"]);
            Diagnostic {
                start,
                end,
                severity: d["severity"].as_u64().unwrap_or(1).clamp(1, 4) as u8,
                message: d["message"].as_str().unwrap_or_default().to_string(),
                source: d["source"].as_str().map(str::to_string),
            }
        })
        .collect();
    Some((path, diags))
}

// --- Parsing responses ---

/// Completion suggestion: the edit to apply, the server's range or the word start.
#[derive(Debug, Clone, PartialEq)]
pub struct CompletionItem {
    pub label: String,
    pub detail: Option<String>,
    /// LSP CompletionItemKind (3 function, 6 variable...).
    pub kind: u8,
    pub filter: String,
    pub sort: String,
    /// Text to insert (expanded form if it is a snippet) and the cursor position within it (characters).
    pub insert: String,
    pub cursor: Option<usize>,
    /// Snippet tab stops (character ranges within the text, in navigation order).
    pub stops: Vec<(u32, std::ops::Range<usize>)>,
    /// Range given by the server (if absent, the word being typed is replaced).
    pub range: Option<Range16>,
    /// Additional edits such as auto-import.
    pub extra: Vec<(Range16, String)>,
    /// Documentation (converted to plain text); shown next to the selected item.
    pub doc: Option<String>,
    /// The server's original item for `completionItem/resolve` (only those carrying `data`).
    pub resolve: Option<std::sync::Arc<Value>>,
}

/// Words in the document when there is no language server (VS Code `editor.wordBasedSuggestions`).
/// `text`: the text around the cursor, `cursor`: byte position within it. Excludes the word
/// being typed (the one containing the cursor); words near the cursor come first. At most `limit` words.
pub fn buffer_words(text: &str, cursor: usize, limit: usize) -> Vec<CompletionItem> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut best: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut start = None;
    for (i, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        match (is_word(c), start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                start = None;
                let word = &text[s..i];
                // The word containing the cursor is the one being typed; single letters and numbers are not suggestions.
                if (s..=i).contains(&cursor) || word.chars().count() < 2 || word.chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }
                let dist = if i <= cursor { cursor - i } else { s - cursor };
                best.entry(word).and_modify(|d| *d = (*d).min(dist)).or_insert(dist);
            }
            _ => {}
        }
    }
    let mut words: Vec<(&str, usize)> = best.into_iter().collect();
    words.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(b.0)));
    words.truncate(limit);
    words
        .into_iter()
        .enumerate()
        .map(|(rank, (w, _))| CompletionItem {
            label: w.to_string(),
            detail: None,
            kind: 1, // text
            filter: w.to_string(),
            sort: format!("{rank:06}"),
            insert: w.to_string(),
            cursor: None,
            stops: Vec::new(),
            range: None,
            extra: Vec::new(),
            doc: None,
            resolve: None,
        })
        .collect()
}

pub fn parse_completions(v: &Value) -> (Vec<CompletionItem>, bool) {
    let (items, incomplete) = match v {
        Value::Array(a) => (a.as_slice(), false),
        Value::Object(_) => (v["items"].as_array().map(Vec::as_slice).unwrap_or(&[]), v["isIncomplete"].as_bool().unwrap_or(false)),
        _ => (&[][..], false),
    };
    let out = items
        .iter()
        .enumerate()
        .map(|(idx, it)| {
            let label = it["label"].as_str().unwrap_or_default().to_string();
            let snippet = it["insertTextFormat"].as_u64() == Some(2);
            let (raw, range) = match it.get("textEdit") {
                Some(te) if te.is_object() => {
                    let r = if te.get("range").is_some() { &te["range"] } else { &te["replace"] };
                    (te["newText"].as_str().unwrap_or(&label).to_string(), Some(parse_range(r)))
                }
                _ => (it["insertText"].as_str().unwrap_or(&label).to_string(), None),
            };
            let (insert, stops) = if snippet { parse_snippet(&raw) } else { (raw, Vec::new()) };
            let cursor = stops.first().map(|(_, r)| r.start);
            let extra = it["additionalTextEdits"]
                .as_array()
                .map(|a| a.iter().map(|e| (parse_range(&e["range"]), e["newText"].as_str().unwrap_or_default().to_string())).collect())
                .unwrap_or_default();
            CompletionItem {
                filter: it["filterText"].as_str().unwrap_or(&label).to_string(),
                sort: it["sortText"].as_str().unwrap_or(&label).to_string(),
                detail: it["detail"].as_str().map(|d| d.lines().next().unwrap_or(d).to_string()),
                kind: it["kind"].as_u64().unwrap_or(0) as u8,
                label,
                insert,
                cursor,
                stops,
                range,
                extra,
                doc: it.get("documentation").and_then(|d| hover_text(&serde_json::json!({ "contents": d }))).filter(|d| !d.trim().is_empty()),
                resolve: (idx < 1000).then(|| it.get("data").map(|_| std::sync::Arc::new(it.clone()))).flatten(),
            }
        })
        .collect();
    (out, incomplete)
}

/// Expand a snippet to plain text: `${1:name}` → `name`, `$1`/`$0` vanish; the cursor goes to the first
/// tab stop (or `$0` if none). Nested and choice stops are simplified.
#[cfg(test)]
pub fn expand_snippet(s: &str) -> (String, Option<usize>) {
    let (text, stops) = parse_snippet(s);
    (text, stops.first().map(|(_, r)| r.start))
}

/// Snippet → (text, tab stops). Stops are ordered: 1, 2, … then $0; each is a character range
/// within the text (the placeholder's default text; empty for `$1`). Nested
/// placeholders are resolved, and the first of the choices (`${1|a,b|}`) is written.
pub fn parse_snippet(s: &str) -> (String, Vec<(u32, std::ops::Range<usize>)>) {
    fn parse(chars: &[char], i: &mut usize, out: &mut String, count: &mut usize, stops: &mut Vec<(u32, std::ops::Range<usize>)>, until_brace: bool) {
        while *i < chars.len() {
            let c = chars[*i];
            if until_brace && c == '}' {
                return;
            }
            if c == '\\' && *i + 1 < chars.len() {
                out.push(chars[*i + 1]);
                *count += 1;
                *i += 2;
                continue;
            }
            if c == '$' && *i + 1 < chars.len() && chars[*i + 1].is_ascii_digit() {
                let mut j = *i + 1;
                let mut n = 0u32;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    n = n * 10 + chars[j].to_digit(10).unwrap_or(0);
                    j += 1;
                }
                stops.push((n, *count..*count));
                *i = j;
                continue;
            }
            if c == '$' && *i + 1 < chars.len() && chars[*i + 1] == '{' {
                let mut j = *i + 2;
                let mut n = 0u32;
                let digits = j;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    n = n * 10 + chars[j].to_digit(10).unwrap_or(0);
                    j += 1;
                }
                let start = *count;
                if j == digits {
                    // ${name} variable: no default, skip.
                    while j < chars.len() && chars[j] != '}' {
                        j += 1;
                    }
                    *i = j + 1;
                    continue;
                }
                match chars.get(j) {
                    Some(':') => {
                        *i = j + 1;
                        parse(chars, i, out, count, stops, true);
                        j = *i;
                    }
                    Some('|') => {
                        j += 1;
                        while j < chars.len() && chars[j] != ',' && chars[j] != '|' {
                            out.push(chars[j]);
                            *count += 1;
                            j += 1;
                        }
                        while j < chars.len() && chars[j] != '}' {
                            j += 1;
                        }
                    }
                    _ => {}
                }
                stops.push((n, start..*count));
                *i = j + 1;
                continue;
            }
            out.push(c);
            *count += 1;
            *i += 1;
        }
    }
    let chars: Vec<char> = s.chars().collect();
    let (mut out, mut count, mut stops, mut i) = (String::new(), 0usize, Vec::new(), 0usize);
    parse(&chars, &mut i, &mut out, &mut count, &mut stops, false);
    // The first of the stops with the same number; order is 1, 2, … , with 0 last.
    let mut seen = std::collections::HashSet::new();
    stops.retain(|(n, _)| seen.insert(*n));
    stops.sort_by_key(|(n, _)| if *n == 0 { u32::MAX } else { *n });
    (out, stops)
}

/// Hover contents → plain text (markdown markup is simplified).
pub fn hover_text(v: &Value) -> Option<String> {
    fn piece(v: &Value) -> Option<String> {
        match v {
            Value::String(s) => Some(s.clone()),
            Value::Object(o) => o.get("value").and_then(Value::as_str).map(str::to_string),
            _ => None,
        }
    }
    let contents = &v["contents"];
    let text = match contents {
        Value::Array(a) => a.iter().filter_map(piece).collect::<Vec<_>>().join("\n\n"),
        other => piece(other)?,
    };
    // Reduce markdown to plain text: code fences, heading markers, separators,
    // inline code and bold markers go away; consecutive blank lines collapse to one.
    let mut lines: Vec<String> = Vec::new();
    for l in text.lines() {
        let t = l.trim_start();
        if t.starts_with("```") || matches!(t.trim(), "---" | "***" | "___") {
            continue;
        }
        let l = l.trim_start_matches('#').trim_start_matches(' ');
        let l = l.replace("**", "").replace('`', "");
        if l.trim().is_empty() && lines.last().is_none_or(|p| p.trim().is_empty()) {
            continue;
        }
        lines.push(l);
    }
    let out = lines.join("\n").trim().to_string();
    (!out.is_empty()).then_some(out)
}

/// Definition locations: Location, Location[] or LocationLink[]. (path, start)
pub fn parse_locations(v: &Value) -> Vec<(String, (u32, u32))> {
    let one = |l: &Value| -> Option<(String, (u32, u32))> {
        let uri = l["uri"].as_str().or(l["targetUri"].as_str())?;
        let range = if l.get("targetSelectionRange").is_some() { &l["targetSelectionRange"] } else { &l["range"] };
        Some((uri_to_path(uri)?, parse_range(range).0))
    };
    match v {
        Value::Array(a) => a.iter().filter_map(one).collect(),
        Value::Object(_) => one(v).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// WorkspaceEdit → edits per file. `changes` and `documentChanges`
/// (TextDocumentEdit) are supported; file create/delete/move is counted but not applied.
pub fn parse_workspace_edit(v: &Value) -> (Vec<(String, Vec<(Range16, String)>)>, usize) {
    let mut out: Vec<(String, Vec<(Range16, String)>)> = Vec::new();
    let mut skipped = 0;
    let mut add = |uri: &str, edits: &Value| {
        let Some(path) = uri_to_path(uri) else { return };
        let list: Vec<(Range16, String)> = edits
            .as_array()
            .map(|a| a.iter().map(|e| (parse_range(&e["range"]), e["newText"].as_str().unwrap_or_default().to_string())).collect())
            .unwrap_or_default();
        match out.iter_mut().find(|(p, _)| *p == path) {
            Some((_, l)) => l.extend(list),
            None => out.push((path, list)),
        }
    };
    if let Some(changes) = v["changes"].as_object() {
        for (uri, edits) in changes {
            add(uri, edits);
        }
    }
    if let Some(dc) = v["documentChanges"].as_array() {
        for d in dc {
            match d["textDocument"]["uri"].as_str() {
                Some(uri) => add(uri, &d["edits"]),
                None => skipped += 1, // create / rename / delete
            }
        }
    }
    (out, skipped)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Symbol {
    pub name: String,
    pub detail: String,
    pub kind: u8,
    pub line: u32,
    pub col: u32,
    pub depth: usize,
}

pub fn symbol_kind(kind: u8) -> &'static str {
    match kind {
        1 => crate::i18n::t("file"),
        2 => crate::i18n::t("module"),
        3 => crate::i18n::t("namespace"),
        4 => crate::i18n::t("package"),
        5 => crate::i18n::t("class"),
        6 => crate::i18n::t("method"),
        7 => crate::i18n::t("property"),
        8 => crate::i18n::t("field"),
        9 => crate::i18n::t("constructor"),
        10 => crate::i18n::t("enum"),
        11 => crate::i18n::t("interface"),
        12 => crate::i18n::t("function"),
        13 => crate::i18n::t("variable"),
        14 => crate::i18n::t("constant"),
        15 => crate::i18n::t("string"),
        22 => crate::i18n::t("enum member"),
        23 => crate::i18n::t("struct"),
        24 => crate::i18n::t("event"),
        25 => crate::i18n::t("operator"),
        26 => crate::i18n::t("type parameter"),
        _ => crate::i18n::t("symbol"),
    }
}

/// DocumentSymbol[] (nested) or SymbolInformation[] → flat list in document order.
pub fn parse_symbols(v: &Value) -> Vec<Symbol> {
    fn walk(items: &[Value], depth: usize, out: &mut Vec<Symbol>) {
        for it in items {
            let range = if it.get("selectionRange").is_some() { &it["selectionRange"] } else { &it["location"]["range"] };
            let (line, col) = parse_range(range).0;
            out.push(Symbol {
                name: it["name"].as_str().unwrap_or_default().to_string(),
                detail: it["detail"].as_str().or(it["containerName"].as_str()).unwrap_or_default().to_string(),
                kind: it["kind"].as_u64().unwrap_or(0) as u8,
                line,
                col,
                depth,
            });
            if let Some(children) = it["children"].as_array() {
                walk(children, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    if let Some(a) = v.as_array() {
        walk(a, 0, &mut out);
    }
    out.sort_by_key(|s| (s.line, s.col));
    out
}

/// A symbol found in the project (`workspace/symbol`).
#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceSymbol {
    pub name: String,
    pub container: String,
    pub kind: u8,
    pub path: String,
    pub line: u32,
    pub col: u32,
}

/// SymbolInformation[] or WorkspaceSymbol[] (the location may be just a `uri`:
/// then the start of the file). The server's order is kept.
pub fn parse_workspace_symbols(v: &Value) -> Vec<WorkspaceSymbol> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|it| {
            let loc = &it["location"];
            let path = uri_to_path(loc["uri"].as_str()?)?;
            let (line, col) = if loc.get("range").is_some() { parse_range(&loc["range"]).0 } else { (0, 0) };
            Some(WorkspaceSymbol {
                name: it["name"].as_str()?.to_string(),
                container: it["containerName"].as_str().unwrap_or_default().to_string(),
                kind: it["kind"].as_u64().unwrap_or(0) as u8,
                path,
                line,
                col,
            })
        })
        .collect()
}

/// Code action: the edit may come ready-made or `codeAction/resolve` may be needed.
#[derive(Debug, Clone)]
pub struct CodeAction {
    pub title: String,
    pub kind: String,
    pub preferred: bool,
    pub raw: Value,
}

pub fn parse_code_actions(v: &Value) -> Vec<CodeAction> {
    let mut out: Vec<CodeAction> = v
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|it| it["disabled"].is_null())
                .map(|it| CodeAction {
                    title: it["title"].as_str().unwrap_or_default().to_string(),
                    kind: it["kind"].as_str().unwrap_or_default().to_string(),
                    preferred: it["isPreferred"].as_bool().unwrap_or(false),
                    raw: it.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    // Preferred and quick fixes first.
    out.sort_by_key(|a| (!a.preferred, !a.kind.starts_with("quickfix")));
    out
}

/// Signature help: (label, byte range of the active parameter within the label).
/// Hint placed at the end of a line (inlay hint): 0-based line, label, kind (1 type, 2 parameter).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlayHint {
    pub line: usize,
    pub label: String,
    pub kind: u8,
}

pub fn parse_inlay_hints(v: &Value) -> Vec<InlayHint> {
    let Some(a) = v.as_array() else { return Vec::new() };
    let mut out: Vec<InlayHint> = a
        .iter()
        .filter_map(|h| {
            let line = h["position"]["line"].as_u64()? as usize;
            let label = match &h["label"] {
                Value::String(s) => s.clone(),
                Value::Array(parts) => parts.iter().filter_map(|p| p["value"].as_str()).collect::<String>(),
                _ => return None,
            };
            let label = label.trim().to_string();
            (!label.is_empty()).then(|| InlayHint { line, label, kind: h["kind"].as_u64().unwrap_or(1) as u8 })
        })
        .take(20_000)
        .collect();
    out.sort_by_key(|h| h.line);
    out
}

/// `foldingRange` response: (first line, last line); single-line ranges are dropped.
pub fn parse_folding_ranges(v: &Value) -> Vec<(usize, usize)> {
    let Some(a) = v.as_array() else { return Vec::new() };
    let mut out: Vec<(usize, usize)> =
        a.iter().filter_map(|r| Some((r["startLine"].as_u64()? as usize, r["endLine"].as_u64()? as usize))).filter(|(s, e)| e > s).collect();
    out.sort_unstable();
    out
}

/// `documentLink` response: (range, target URI). Links without a target are dropped.
pub fn parse_document_links(v: &Value) -> Vec<(Range16, String)> {
    let Some(a) = v.as_array() else { return Vec::new() };
    a.iter().filter_map(|l| Some((parse_range(&l["range"]), l["target"].as_str()?.to_string()))).take(20_000).collect()
}

/// `prepareRename` response: `None` cannot be renamed; `Some(None)` word at the position;
/// `Some(Some(range))` the range the server indicated.
pub fn parse_prepare_rename(v: &Value) -> Option<Option<Range16>> {
    if v.is_null() {
        return None;
    }
    if v["defaultBehavior"].as_bool() == Some(true) {
        return Some(None);
    }
    if v.get("range").is_some() {
        return Some(Some(parse_range(&v["range"])));
    }
    if v.get("start").is_some() {
        return Some(Some(parse_range(v)));
    }
    Some(None)
}

/// Rewrite the URIs under `old` (file or folder) in a `WorkspaceEdit` to be under `new`
/// (for `willRenameFiles` edits applied after a move).
pub fn remap_edit_uris(edit: &Value, old: &str, new: &str) -> Value {
    let (old_uri, new_uri) = (path_to_uri(old), path_to_uri(new));
    let map = |uri: &str| -> String {
        if uri == old_uri {
            new_uri.clone()
        } else if let Some(rest) = uri.strip_prefix(&format!("{}/", old_uri.trim_end_matches('/'))) {
            format!("{}/{rest}", new_uri.trim_end_matches('/'))
        } else {
            uri.to_string()
        }
    };
    let mut out = edit.clone();
    if let Some(changes) = edit["changes"].as_object() {
        let mapped: serde_json::Map<String, Value> = changes.iter().map(|(k, v)| (map(k), v.clone())).collect();
        out["changes"] = Value::Object(mapped);
    }
    if let Some(docs) = out["documentChanges"].as_array_mut() {
        for d in docs {
            if let Some(u) = d["textDocument"]["uri"].as_str().map(map) {
                d["textDocument"]["uri"] = Value::String(u);
            }
        }
    }
    out
}

/// Call / type hierarchy response → (file, (line, column)). `key`: the item inside the entry
/// (`from`, `to`); `None`: the entry itself is the item. Position is the start of the item's `selectionRange`.
pub fn parse_hierarchy(v: &Value, key: Option<&str>) -> Vec<(String, (u32, u32))> {
    let Some(a) = v.as_array() else { return Vec::new() };
    a.iter()
        .filter_map(|e| {
            let item = key.map_or(e, |k| &e[k]);
            let path = uri_to_path(item["uri"].as_str()?)?;
            let start = if item["selectionRange"].is_object() { &item["selectionRange"]["start"] } else { &item["range"]["start"] };
            Some((path, (start["line"].as_u64()? as u32, start["character"].as_u64().unwrap_or(0) as u32)))
        })
        .collect()
}

/// Code lens: line, title and the command to run (the `command` object).
#[derive(Debug, Clone, PartialEq)]
pub struct Lens {
    pub line: usize,
    pub title: String,
    pub command: Value,
}

/// `codeLens` response. Items without a command (unresolved) go to the `unresolved` list in raw form.
pub fn parse_code_lenses(v: &Value) -> (Vec<Lens>, Vec<Value>) {
    let (mut lenses, mut unresolved) = (Vec::new(), Vec::new());
    for item in v.as_array().map(Vec::as_slice).unwrap_or(&[]) {
        let Some(line) = item["range"]["start"]["line"].as_u64().map(|l| l as usize) else { continue };
        match item["command"]["title"].as_str() {
            Some(t) if !t.is_empty() => lenses.push(Lens { line, title: t.to_string(), command: item["command"].clone() }),
            _ => unresolved.push(item.clone()),
        }
    }
    (lenses, unresolved)
}

/// Semantic token: line, start column (UTF-16), length (UTF-16), type name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemToken {
    pub line: usize,
    pub start: u32,
    pub len: u32,
    pub kind: String,
}

/// `semanticTokens/full` data (relative quintuples) → tokens. `legend`: the server's type table.
pub fn decode_semantic_tokens(data: &Value, legend: &[String]) -> Vec<SemToken> {
    const MAX_TOKENS: usize = 200_000;
    let Some(a) = data.as_array() else { return Vec::new() };
    // Malformed data (a non-number item, a length not a multiple of five) would shift all tokens: drop it.
    let Some(nums) = a.iter().map(Value::as_u64).collect::<Option<Vec<u64>>>() else { return Vec::new() };
    if nums.len() % 5 != 0 {
        return Vec::new();
    }
    let to_u32 = |n: u64| u32::try_from(n).unwrap_or(u32::MAX);
    let (mut line, mut col) = (0usize, 0u32);
    let mut out = Vec::new();
    for t in nums.as_chunks::<5>().0.iter().take(MAX_TOKENS) {
        let dl = usize::try_from(t[0]).unwrap_or(usize::MAX);
        if dl > 0 {
            line = line.saturating_add(dl);
            col = to_u32(t[1]);
        } else {
            col = col.saturating_add(to_u32(t[1]));
        }
        if let Some(k) = usize::try_from(t[3]).ok().and_then(|i| legend.get(i)).filter(|k| !k.is_empty()) {
            out.push(SemToken { line, start: col, len: to_u32(t[2]), kind: k.clone() });
        }
    }
    out
}

pub fn parse_signature(v: &Value) -> Option<(String, Option<std::ops::Range<usize>>)> {
    let sigs = v["signatures"].as_array()?;
    let active = v["activeSignature"].as_u64().unwrap_or(0) as usize;
    let sig = sigs.get(active).or(sigs.first())?;
    let label = sig["label"].as_str()?.to_string();
    let param_ix = sig["activeParameter"].as_u64().or(v["activeParameter"].as_u64()).unwrap_or(0) as usize;
    let range = sig["parameters"].as_array().and_then(|ps| ps.get(param_ix)).and_then(|p| match &p["label"] {
        Value::String(s) => label.find(s.as_str()).map(|i| i..i + s.len()),
        Value::Array(a) if a.len() == 2 => {
            // UTF-16 boundaries → bytes.
            let (a16, b16) = (a[0].as_u64()? as usize, a[1].as_u64()? as usize);
            let to_byte = |u: usize| {
                let mut units = 0;
                label
                    .char_indices()
                    .find(|(_, c)| {
                        let hit = units >= u;
                        units += c.len_utf16();
                        hit
                    })
                    .map_or(label.len(), |(i, _)| i)
            };
            Some(to_byte(a16)..to_byte(b16))
        }
        _ => None,
    });
    Some((label, range))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn workspace_symbols_with_and_without_range() {
        let v = json!([
            { "name": "Buffer", "kind": 23, "containerName": "buffer", "location": { "uri": "file:///p/a.rs", "range": { "start": { "line": 4, "character": 11 }, "end": { "line": 4, "character": 17 } } } },
            { "name": "main", "kind": 12, "location": { "uri": "file:///p/b%20c.rs" } },
            { "kind": 12, "location": { "uri": "file:///p/x.rs" } }
        ]);
        let s = parse_workspace_symbols(&v);
        assert_eq!(s.len(), 2, "unnamed is skipped");
        assert_eq!((s[0].path.as_str(), s[0].line, s[0].col, s[0].container.as_str()), ("/p/a.rs", 4, 11, "buffer"));
        assert_eq!((s[1].path.as_str(), s[1].line), ("/p/b c.rs", 0));
    }

    #[test]
    fn snippets_expand_with_cursor() {
        assert_eq!(expand_snippet("println!(\"$1\")$0"), ("println!(\"\")".into(), Some(10)));
        assert_eq!(expand_snippet("fn ${1:name}(${2:args}) {\n\t$0\n}"), ("fn name(args) {\n\t\n}".into(), Some(3)));
        assert_eq!(expand_snippet("plain"), ("plain".into(), None));
        assert_eq!(expand_snippet("${1|a,b|}x"), ("ax".into(), Some(0)));
        assert_eq!(expand_snippet("a\\$b"), ("a$b".into(), None));
        let (text, stops) = parse_snippet("topla(${1:int a}, ${2:int b})$0");
        assert_eq!(text, "topla(int a, int b)");
        assert_eq!(stops, vec![(1, 6..11), (2, 13..18), (0, 19..19)]);
        let (text, stops) = parse_snippet("${1:outer ${2:inner}}");
        assert_eq!((text.as_str(), stops), ("outer inner", vec![(1, 0..11), (2, 6..11)]));
    }

    #[test]
    fn word_suggestions_skip_the_typed_word_and_prefer_nearby() {
        let text = "uzakkelime\nmerhaba dünya\nmer| yakınkelime merhaba 42 a\n";
        let cursor = text.find('|').unwrap();
        let text = text.replace('|', "");
        let words: Vec<String> = buffer_words(&text, cursor, 100).into_iter().map(|w| w.label).collect();
        assert!(!words.contains(&"mer".to_string()), "the word being typed is not suggested: {words:?}");
        assert!(!words.contains(&"42".to_string()) && !words.contains(&"a".to_string()), "no numbers or single letters");
        assert_eq!(words.iter().filter(|w| *w == "merhaba").count(), 1, "no duplicates");
        let pos = |w: &str| words.iter().position(|x| x == w).unwrap();
        assert!(pos("yakınkelime") < pos("uzakkelime"), "nearby first: {words:?}");
        assert!(words.contains(&"dünya".to_string()), "Turkish letters are word characters");
        assert_eq!(buffer_words(&"kelime ".repeat(10), 0, 5).len(), 1);
        assert!(buffer_words("", 0, 10).is_empty());
    }

    #[test]
    fn parses_completion_and_locations() {
        let v = json!({ "isIncomplete": true, "items": [
            { "label": "push", "kind": 2, "detail": "fn(&mut self, T)", "insertTextFormat": 2,
              "textEdit": { "range": { "start": {"line": 1, "character": 4}, "end": {"line": 1, "character": 6} }, "newText": "push(${1:value})" } },
            { "label": "len" }
        ]});
        let (items, incomplete) = parse_completions(&v);
        assert!(incomplete);
        assert_eq!(items[0].insert, "push(value)");
        assert_eq!(items[0].cursor, Some(5));
        assert_eq!(items[0].range, Some(((1, 4), (1, 6))));
        assert_eq!(items[1].insert, "len");
        assert_eq!(items[1].doc, None);
        let (docs, _) = parse_completions(&json!([
            { "label": "a", "documentation": "düz metin" },
            { "label": "b", "documentation": { "kind": "markdown", "value": "```rust\nfn b()\n```\n**Kalın** açıklama" } },
            { "label": "c", "documentation": "  " },
        ]));
        assert_eq!(docs.iter().map(|d| d.doc.as_deref()).collect::<Vec<_>>(), [Some("düz metin"), Some("fn b()\nKalın açıklama"), None]);
        let locs = parse_locations(
            &json!([{ "targetUri": "file:///x/a.rs", "targetRange": {}, "targetSelectionRange": { "start": {"line": 3, "character": 7}, "end": {"line": 3, "character": 9} } }]),
        );
        assert_eq!(locs, vec![("/x/a.rs".to_string(), (3, 7))]);
        assert_eq!(hover_text(&json!({ "contents": { "kind": "markdown", "value": "```rust\nfn main()\n```\nDocs" } })).as_deref(), Some("fn main()\nDocs"));
        let md = "### function `topla`\n\n---\n\n→ `int`\n**Parameters**:\n- `int a`";
        assert_eq!(hover_text(&json!({ "contents": { "kind": "markdown", "value": md } })).as_deref(), Some("function topla\n\n→ int\nParameters:\n- int a"));
    }

    #[test]
    fn parses_edits_symbols_actions_signature() {
        let we = json!({ "documentChanges": [
            { "textDocument": { "uri": "file:///p/a.rs", "version": 3 }, "edits": [
                { "range": { "start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 6} }, "newText": "yeni" } ] },
            { "kind": "rename", "oldUri": "file:///p/b.rs", "newUri": "file:///p/c.rs" }
        ], "changes": { "file:///p/d.rs": [ { "range": { "start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 0} }, "newText": "x" } ] } });
        let (files, skipped) = parse_workspace_edit(&we);
        assert_eq!(skipped, 1);
        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|(p, e)| p == "/p/a.rs" && e[0] == (((0, 3), (0, 6)), "yeni".to_string())));

        let syms = parse_symbols(&json!([{ "name": "Nokta", "kind": 23, "range": {}, "selectionRange": { "start": {"line": 4, "character": 7}, "end": {} },
            "children": [{ "name": "x", "kind": 8, "detail": "i32", "range": {}, "selectionRange": { "start": {"line": 5, "character": 4}, "end": {} } }] }]));
        assert_eq!(syms.iter().map(|s| (s.name.as_str(), s.depth, s.line)).collect::<Vec<_>>(), vec![("Nokta", 0, 4), ("x", 1, 5)]);
        assert_eq!(symbol_kind(23), "struct");

        let acts =
            parse_code_actions(&json!([{ "title": "Yeniden düzenle", "kind": "refactor" }, { "title": "Düzelt", "kind": "quickfix", "isPreferred": true }]));
        assert_eq!(acts[0].title, "Düzelt", "preferred first");

        let sig = parse_signature(&json!({ "signatures": [{ "label": "topla(a: i32, b: i32) -> i32", "parameters": [{ "label": [6, 12] }, { "label": "b: i32" }], "activeParameter": 1 }] })).unwrap();
        assert_eq!(&sig.0[sig.1.clone().unwrap()], "b: i32");
    }

    #[test]
    fn hierarchy_results_become_locations() {
        let v = serde_json::json!([
            { "from": { "uri": "file:///a/b.rs", "selectionRange": { "start": { "line": 3, "character": 4 }, "end": { "line": 3, "character": 9 } }, "range": { "start": { "line": 1, "character": 0 } } }, "fromRanges": [] },
            { "from": { "uri": "http://x/y" } },
            { "from": { "uri": "file:///c.rs", "range": { "start": { "line": 7, "character": 1 } } } }
        ]);
        assert_eq!(parse_hierarchy(&v, Some("from")), vec![("/a/b.rs".to_string(), (3, 4)), ("/c.rs".to_string(), (7, 1))]);
        let direct = serde_json::json!([{ "uri": "file:///t.rs", "selectionRange": { "start": { "line": 0, "character": 2 } } }]);
        assert_eq!(parse_hierarchy(&direct, None), vec![("/t.rs".to_string(), (0, 2))]);
        assert!(parse_hierarchy(&serde_json::Value::Null, None).is_empty());
    }

    #[test]
    fn code_lenses_and_semantic_tokens_decode() {
        let v = serde_json::json!([
            { "range": { "start": { "line": 4, "character": 0 } }, "command": { "title": "3 references", "command": "x.showRefs" } },
            { "range": { "start": { "line": 9, "character": 0 } }, "data": { "id": 1 } },
            { "command": { "title": "no range" } }
        ]);
        let (lenses, unresolved) = parse_code_lenses(&v);
        assert_eq!((lenses.len(), unresolved.len()), (1, 1));
        assert_eq!((lenses[0].line, lenses[0].title.as_str()), (4, "3 references"));
        // Relative encoding: (0,2,3,k0) (0,5,2,k1) (2,1,4,k0)
        let data = serde_json::json!([0, 2, 3, 0, 0, 0, 5, 2, 1, 0, 2, 1, 4, 0, 0]);
        let legend = vec!["function".to_string(), "variable".to_string()];
        let t = decode_semantic_tokens(&data, &legend);
        assert_eq!(
            t,
            vec![
                SemToken { line: 0, start: 2, len: 3, kind: "function".into() },
                SemToken { line: 0, start: 7, len: 2, kind: "variable".into() },
                SemToken { line: 2, start: 1, len: 4, kind: "function".into() },
            ]
        );
        assert!(decode_semantic_tokens(&serde_json::Value::Null, &legend).is_empty());
    }

    #[test]
    fn hostile_semantic_tokens_and_uri_remapping() {
        let legend = vec!["function".to_string()];
        let huge = serde_json::json!([0, 4294967295u64, 4294967295u64, 0, 0, 0, 4294967295u64, 1, 0, 0]);
        let t = decode_semantic_tokens(&huge, &legend);
        assert_eq!(t.len(), 2);
        assert_eq!(t[1].start, u32::MAX, "overflow saturates, no panic");
        assert!(decode_semantic_tokens(&serde_json::json!([0, 1, 2, 0]), &legend).is_empty(), "length is not a multiple of five");
        assert!(decode_semantic_tokens(&serde_json::json!([0, "x", 2, 0, 0]), &legend).is_empty(), "non-number item");
        assert!(decode_semantic_tokens(&serde_json::json!([0, 1, 2, 9, 0]), &legend).is_empty(), "type not in the legend");
        let edit = serde_json::json!({
            "changes": { "file:///a/old/x.ts": [], "file:///a/other.ts": [] },
            "documentChanges": [{ "textDocument": { "uri": "file:///a/old.ts", "version": 1 }, "edits": [] }]
        });
        let moved = remap_edit_uris(&edit, "/a/old", "/a/new");
        assert!(moved["changes"].get("file:///a/new/x.ts").is_some() && moved["changes"].get("file:///a/other.ts").is_some());
        let moved_file = remap_edit_uris(&edit, "/a/old.ts", "/a/new.ts");
        assert_eq!(moved_file["documentChanges"][0]["textDocument"]["uri"], "file:///a/new.ts");
    }
}
