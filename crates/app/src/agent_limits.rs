//! Usage-limit messages of the agent command-line tools, found in the text a terminal shows.
//!
//! None of the tools has an interface that says "this account is out of quota", so the one signal is what they print. The
//! match is deliberately narrow (a phrase the tool prints, near the bottom of the screen) because a false "limit reached"
//! sends the person to another account for nothing. A miss is corrected by hand (Agents: Mark Account as Limited).

/// A limit message on screen, with the reset time the tool printed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limit {
    pub resets: Option<String>,
}

/// Only the last rows with text count (a screen has empty rows below the text): an old message that scrolled away is not a
/// limit now.
const LAST_ROWS: usize = 20;

/// Phrases the tools print, lower case. Claude Code: "Claude usage limit reached. Your limit will reset at 3pm",
/// "5-hour limit reached ∙ resets 3pm", "You've hit your limit". Codex: "You've hit your usage limit".
const MARKERS: &[&str] =
    &["usage limit reached", "hit your usage limit", "hit your limit", "limit reached ∙ resets", "limit reached · resets", "limit reached - resets"];

/// Phrases that introduce the reset time, lower case.
const RESET_WORDS: &[&str] = &["resets ", "reset at ", "try again at "];

/// A limit message among the last rows of `rows` (screen rows, top to bottom).
pub fn detect(rows: &[String]) -> Option<Limit> {
    rows.iter().rev().filter(|row| !row.trim().is_empty()).take(LAST_ROWS).find_map(|row| {
        let lower = row.to_lowercase();
        MARKERS.iter().any(|m| lower.contains(m)).then(|| Limit { resets: resets(row, &lower) })
    })
}

/// The reset time after "resets", "reset at" or "try again at", without the sentence's full stop.
fn resets(row: &str, lower: &str) -> Option<String> {
    RESET_WORDS.iter().find_map(|word| {
        // `to_lowercase` can change byte lengths; the offset is only trusted when both strings have the same length.
        let at = lower.find(word)?;
        if lower.len() != row.len() {
            return None;
        }
        let rest = row.get(at + word.len()..)?.trim();
        let rest = rest.trim_end_matches(['.', ' ']);
        let rest: String = rest.chars().take(40).collect();
        (!rest.is_empty()).then_some(rest)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(lines: &[&str]) -> Vec<String> {
        lines.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn claude_code_limit_messages_are_found_with_their_reset_time() {
        let found = detect(&rows(&["", "Claude usage limit reached. Your limit will reset at 3pm (Europe/Istanbul)."])).expect("a limit");
        assert_eq!(found.resets.as_deref(), Some("3pm (Europe/Istanbul)"), "a bracket is kept whole, the full stop is not part of it");
        assert_eq!(detect(&rows(&["5-hour limit reached ∙ resets 3pm"])).expect("a limit").resets.as_deref(), Some("3pm"));
        assert!(detect(&rows(&["You've hit your limit"])).is_some());
    }

    #[test]
    fn codex_limit_messages_are_found() {
        let found = detect(&rows(&["You've hit your usage limit. Try again at 3:40 PM."])).expect("a limit");
        assert_eq!(found.resets.as_deref(), Some("3:40 PM"));
    }

    #[test]
    fn text_that_only_mentions_limits_is_not_a_limit() {
        for line in
            ["the rate limit of the API is 5 requests", "const LIMIT: usize = 20; // limit reached when full", "ulimit -n 4096", "git log --limit", "limit"]
        {
            assert_eq!(detect(&rows(&[line])), None, "{line}");
        }
    }

    #[test]
    fn an_old_message_that_scrolled_up_is_not_a_limit_now() {
        let mut screen = rows(&["Claude usage limit reached."]);
        screen.extend((0..LAST_ROWS).map(|i| format!("line {i}")));
        assert_eq!(detect(&screen), None);
    }

    #[test]
    fn the_empty_rows_below_the_text_of_a_screen_do_not_hide_a_message() {
        let mut screen = rows(&["Claude usage limit reached. Your limit will reset at 3pm."]);
        screen.extend((0..30).map(|_| String::new()));
        assert!(detect(&screen).is_some());
    }

    #[test]
    fn a_message_without_a_time_is_still_a_limit() {
        assert_eq!(detect(&rows(&["Claude usage limit reached."])), Some(Limit { resets: None }));
    }

    #[test]
    fn a_row_whose_lower_case_changes_its_length_does_not_break_the_reset_time() {
        // "İ" lower-cases to two chars; the offset of the lower-case text cannot be used on the original.
        let found = detect(&rows(&["İİİ usage limit reached, resets 3pm"])).expect("a limit");
        assert_eq!(found.resets, None);
    }
}
