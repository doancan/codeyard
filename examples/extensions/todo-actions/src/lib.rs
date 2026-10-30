//! TODO Actions: code actions, hover and completion. For a line with `TODO`, Codeyard's light bulb offers "Mark as
//! done" (edits that replace the word) and "Collect this TODO" (no edits: Codeyard calls `on_code_action` with its id
//! when it is chosen). Resting the mouse on a marker explains it, and in a comment the markers are offered as you type.
//!
//! Code lenses and inlay hints: each TODO in view gets a "Collect" lens, which runs the `collect` command with the TODO
//! as its argument, and a hint such as `2 of 5`. Codeyard asks only for the lines in view, again after an edit.

use codeyard_extension::prelude::*;

struct TodoActions;

const COLLECTED: &str = "collected";

impl Extension for TodoActions {
    fn activate(ctx: Context) -> Result<()> {
        ctx.code_actions(async |_ctx, doc, range, _diagnostics| Ok(actions(&doc, range)));
        ctx.on_code_action(async |ctx, id| {
            // The id carries the line: `collect:<text>`.
            if let Some(todo) = id.strip_prefix("collect:") {
                collect(&ctx, todo)?;
            }
            Ok(())
        });
        // Providers answer from the document they are given, at once: Codeyard drops answers that come too late.
        ctx.hover(async |_ctx, doc, position| Ok(hover(&doc, position)));
        ctx.completion(async |_ctx, doc, position, _trigger| Ok(completions(&doc, position)));
        ctx.command("collect", async |ctx, args| {
            let todo = args.as_str().ok_or_else(|| Error::invalid("collect takes the TODO's text"))?;
            collect(&ctx, todo)
        });
        ctx.code_lenses(async |_ctx, doc, range| {
            let lenses = todos(&doc, range).map(|(line, _, text)| CodeLens::new(line, "Collect", "collect", json!(text))).collect();
            Ok(lenses)
        });
        ctx.inlay_hints(async |_ctx, doc, range| {
            let total = doc.text.lines().filter(|l| l.contains("TODO")).count();
            let before = doc.text.lines().take(range.start.line as usize).filter(|l| l.contains("TODO")).count();
            let hints = todos(&doc, range)
                .enumerate()
                .map(|(k, (line, end, _))| InlayHint::new(Position::new(line, end), format!("{} of {total}", before + k + 1)))
                .collect();
            Ok(hints)
        });
        ctx.command("list", async |ctx, _args| {
            let all: Vec<String> = ctx.storage().get_json(COLLECTED)?.unwrap_or_default();
            ctx.notice(if all.is_empty() { "No TODOs collected yet.".to_string() } else { all.join("\n") });
            Ok(all.len())
        });
        Ok(())
    }
}

fn collect(ctx: &Context, todo: &str) -> Result<usize> {
    let mut all: Vec<String> = ctx.storage().get_json(COLLECTED)?.unwrap_or_default();
    all.push(todo.to_string());
    ctx.storage().set_json(COLLECTED, &all)?;
    ctx.notice(format!("Collected ({} so far)", all.len()));
    Ok(all.len())
}

/// The TODOs on the lines in view (`range` ends at the start of the line after the last): (line, UTF-16 length of the
/// line, the TODO's text).
fn todos(doc: &Document, range: Range) -> impl Iterator<Item = (u32, u32, String)> + '_ {
    let lines = doc.text.lines().enumerate().skip(range.start.line as usize).take(range.end.line.saturating_sub(range.start.line) as usize);
    lines.filter_map(|(i, line)| Some((i as u32, line.encode_utf16().count() as u32, line[line.find("TODO")?..].trim().to_string())))
}

/// The actions for the TODOs on the lines of `range`.
fn actions(doc: &Document, range: Range) -> Vec<CodeAction> {
    let mut out = Vec::new();
    for (i, line) in doc.text.lines().enumerate().skip(range.start.line as usize).take((range.end.line - range.start.line + 1) as usize) {
        let Some(at) = line.find("TODO") else { continue };
        let i = i as u32;
        // UTF-16 columns.
        let col = line[..at].encode_utf16().count() as u32;
        out.push(CodeAction {
            id: format!("done:{i}"),
            title: "Mark as done".into(),
            edits: vec![(doc.uri.clone(), vec![TextEdit::replace(Range::line(i, col, col + 4), "DONE")])],
            preferred: true,
            ..Default::default()
        });
        out.push(CodeAction {
            id: format!("collect:{}", line[at..].trim()),
            title: "Collect this TODO".into(),
            command: Some("collect".into()),
            ..Default::default()
        });
    }
    out
}

/// The markers the extension knows, with what each means.
const MARKERS: [(&str, &str); 3] = [("TODO", "Work left to do."), ("FIXME", "Something known to be broken."), ("NOTE", "Worth knowing.")];

/// The marker under `position`, explained.
fn hover(doc: &Document, position: Position) -> Option<Hover> {
    let line = doc.text.lines().nth(position.line as usize)?;
    for (marker, meaning) in MARKERS {
        for (at, _) in line.match_indices(marker) {
            // UTF-16 columns.
            let start = line[..at].encode_utf16().count() as u32;
            let end = start + marker.len() as u32;
            if (start..end).contains(&position.character) {
                let range = Range::line(position.line, start, end);
                return Some(Hover { contents: format!("**{marker}**: {meaning} The light bulb marks it done."), range: Some(range) });
            }
        }
    }
    None
}

/// Markers, offered only inside a `//` comment.
fn completions(doc: &Document, position: Position) -> Vec<CompletionItem> {
    let Some(line) = doc.text.lines().nth(position.line as usize) else { return Vec::new() };
    let before: String = String::from_utf16_lossy(&line.encode_utf16().take(position.character as usize).collect::<Vec<u16>>());
    if !before.contains("//") {
        return Vec::new();
    }
    MARKERS
        .iter()
        .map(|(marker, meaning)| CompletionItem {
            detail: Some((*meaning).into()),
            insert_text: Some(format!("{marker}: $0")),
            snippet: true,
            ..CompletionItem::new(*marker, CompletionKind::Keyword)
        })
        .collect()
}

codeyard_extension::export!(TodoActions);

#[cfg(test)]
mod tests {
    use super::*;
    use codeyard_extension_test::MockHost;

    #[test]
    fn offers_an_edit_and_a_command() {
        let host = MockHost::new();
        host.activate::<TodoActions>().unwrap();
        let doc = Document { uri: host.uri("src/main.rs"), language: "rust".into(), version: 1, text: "fn main() {}\n// çok TODO: tidy up\n".into() };
        let actions = host.code_actions(doc.clone(), Range::line(1, 0, 0), Vec::new()).unwrap();
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].edits[0].1, [TextEdit::replace(Range::line(1, 7, 11), "DONE")]);
        // Nothing on a line without one.
        assert!(host.code_actions(doc, Range::line(0, 0, 0), Vec::new()).unwrap().is_empty());
        host.run_code_action(&actions[1].id).unwrap();
        assert_eq!(host.command("list", Value::Null).unwrap(), json!(1));
        assert_eq!(host.notices(), ["Collected (1 so far)", "TODO: tidy up"]);
    }

    #[test]
    fn explains_markers_and_offers_them_in_comments() {
        let host = MockHost::new();
        host.activate::<TodoActions>().unwrap();
        let doc = Document { uri: host.uri("src/main.rs"), language: "rust".into(), version: 1, text: "let a = 1; // FIXME later\n".into() };
        let h = host.hover(doc.clone(), Position::new(0, 15)).unwrap().unwrap();
        assert!(h.contents.starts_with("**FIXME**"));
        assert_eq!(h.range, Some(Range::line(0, 14, 19)));
        assert_eq!(host.hover(doc.clone(), Position::new(0, 2)).unwrap(), None);
        let in_comment = host.completion(doc.clone(), Position::new(0, 13), Some("/")).unwrap();
        assert_eq!(in_comment.items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(), ["TODO", "FIXME", "NOTE"]);
        assert_eq!(in_comment.items[0].insert_text.as_deref(), Some("TODO: $0"));
        assert!(host.completion(doc, Position::new(0, 4), None).unwrap().items.is_empty(), "nothing outside a comment");
    }

    #[test]
    fn a_lens_collects_its_todo_and_a_hint_counts_them() {
        let host = MockHost::new();
        host.activate::<TodoActions>().unwrap();
        let text = "// TODO: one\nfn main() {}\n// TODO: two\n// TODO: three\n";
        let doc = Document { uri: host.uri("src/main.rs"), language: "rust".into(), version: 1, text: text.into() };
        // Only the lines in view: 1 and 2.
        let view = Range::new(Position::new(1, 0), Position::new(3, 0));
        let lenses = host.code_lenses(doc.clone(), view).unwrap();
        assert_eq!(lenses.len(), 1);
        assert_eq!(lenses[0].range.start.line, 2);
        assert_eq!(host.click_code_lens(&lenses[0]).unwrap(), json!(1));
        assert_eq!(host.command("list", Value::Null).unwrap(), json!(1));
        assert_eq!(host.notices(), ["Collected (1 so far)", "TODO: two"]);
        let hints = host.inlay_hints(doc, view).unwrap();
        assert_eq!(hints, [InlayHint::new(Position::new(2, 12), "2 of 3")]);
    }
}
