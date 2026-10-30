//! Trim Formatter: a formatter. Codeyard asks it for edits when the user formats a Markdown or text file (or saves
//! one with format-on-save); it answers with the smallest edits that do the job, so the cursor and undo stay put.

use codeyard_extension::prelude::*;

struct TrimFormatter;

impl Extension for TrimFormatter {
    fn activate(ctx: Context) -> Result<()> {
        ctx.formatter(async |_ctx, doc, range, _options| Ok(trim(&doc.text, range)));
        Ok(())
    }
}

/// Columns are UTF-16 code units, as in the Language Server Protocol.
fn width(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

/// One edit per line with trailing spaces or tabs (only the lines in `range`, when given), and one for the end.
fn trim(text: &str, range: Option<Range>) -> Vec<TextEdit> {
    let lines: Vec<&str> = text.split('\n').collect();
    let (first, last) = range.map_or((0, lines.len() as u32), |r| (r.start.line, r.end.line + 1));
    let mut edits = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let i = i as u32;
        let line = line.strip_suffix('\r').unwrap_or(line);
        let kept = line.trim_end_matches([' ', '\t']);
        if i >= first && i < last && kept.len() < line.len() {
            edits.push(TextEdit::replace(Range::line(i, width(kept), width(line)), ""));
        }
    }
    // Whole-document formatting also ends the file with exactly one newline.
    if range.is_none() && !text.is_empty() {
        let body = text.trim_end_matches(['\n', '\r', ' ', '\t']);
        if &text[body.len()..] != "\n" {
            // From the end of the text to the end of the file, which covers the line edits from there on.
            let from = Position::new(body.matches('\n').count() as u32, width(body.rsplit('\n').next().unwrap_or("")));
            let end_line = lines.len() as u32 - 1;
            let to = Position::new(end_line, width(lines[end_line as usize]));
            edits.retain(|e| e.range.start.line < from.line);
            edits.push(TextEdit::replace(Range::new(from, to), "\n"));
        }
    }
    edits
}

codeyard_extension::export!(TrimFormatter);

#[cfg(test)]
mod tests {
    use super::*;
    use codeyard_extension_test::MockHost;

    fn format(text: &str, range: Option<Range>) -> Vec<TextEdit> {
        let host = MockHost::new();
        host.activate::<TrimFormatter>().unwrap();
        let doc = Document { uri: host.uri("notes.md"), language: "markdown".into(), version: 1, text: text.into() };
        host.format(doc, range, FormatOptions { tab_size: 4, insert_spaces: true }).unwrap()
    }

    /// The edits applied to `text`, last first so earlier positions stay valid.
    fn apply(text: &str, edits: &[TextEdit]) -> String {
        let mut lines: Vec<Vec<u16>> = text.split('\n').map(|l| l.encode_utf16().collect()).collect();
        let mut edits = edits.to_vec();
        edits.sort_by_key(|e| std::cmp::Reverse(e.range.start));
        for e in edits {
            let (s, t) = (e.range.start, e.range.end);
            let mut joined: Vec<u16> = lines[s.line as usize][..s.character as usize].to_vec();
            joined.extend(e.new_text.encode_utf16());
            joined.extend(&lines[t.line as usize][t.character as usize..]);
            let parts: Vec<Vec<u16>> = String::from_utf16(&joined).unwrap().split('\n').map(|l| l.encode_utf16().collect()).collect();
            lines.splice(s.line as usize..=t.line as usize, parts);
        }
        lines.iter().map(|l| String::from_utf16(l).unwrap()).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn trims_line_ends_and_the_end_of_the_file() {
        let text = "# Notes  \n\nçay \tvar\t\nend  \n\n\n";
        assert_eq!(apply(text, &format(text, None)), "# Notes\n\nçay \tvar\nend\n");
        let text = "no newline  ";
        assert_eq!(apply(text, &format(text, None)), "no newline\n");
        // A tidy file needs no edits.
        assert!(format("tidy\n", None).is_empty());
    }

    #[test]
    fn a_range_touches_only_its_lines() {
        let text = "a  \nb  \nc  \n";
        let edits = format(text, Some(Range::line(1, 0, 3)));
        assert_eq!(apply(text, &edits), "a  \nb\nc  \n");
    }
}
