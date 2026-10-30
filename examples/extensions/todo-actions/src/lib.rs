//! TODO Actions: code actions. For a line with `TODO`, Codeyard's light bulb offers "Mark as done" (edits that replace
//! the word) and "Collect this TODO" (no edits: Codeyard calls `on_code_action` with its id when it is chosen).

use codeyard_extension::prelude::*;

struct TodoActions;

const COLLECTED: &str = "collected";

impl Extension for TodoActions {
    fn activate(ctx: Context) -> Result<()> {
        ctx.code_actions(async |_ctx, doc, range, _diagnostics| Ok(actions(&doc, range)));
        ctx.on_code_action(async |ctx, id| {
            // The id carries the line: `collect:<text>`.
            if let Some(todo) = id.strip_prefix("collect:") {
                let mut all: Vec<String> = ctx.storage().get_json(COLLECTED)?.unwrap_or_default();
                all.push(todo.to_string());
                ctx.storage().set_json(COLLECTED, &all)?;
                ctx.notice(format!("Collected ({} so far)", all.len()));
            }
            Ok(())
        });
        ctx.command("list", async |ctx, _args| {
            let all: Vec<String> = ctx.storage().get_json(COLLECTED)?.unwrap_or_default();
            ctx.notice(if all.is_empty() { "No TODOs collected yet.".to_string() } else { all.join("\n") });
            Ok(all.len())
        });
        Ok(())
    }
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
}
