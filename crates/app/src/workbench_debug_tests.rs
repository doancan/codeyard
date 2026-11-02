//! Inline values, console limits and stop messages.

use super::*;

#[test]
fn inline_values_cover_the_function_up_to_the_stop_line() {
    let src: Vec<String> = ["int helper(void);", "", "int add(int a, int b) {", "    int sum = a + b;", "    // sum is unused here", "    return sum;", "}"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let locals = vec![("a".to_string(), "3".to_string()), ("b".to_string(), "4".to_string()), ("sum".to_string(), "7".to_string())];
    let v = inline_values(&src, 5, &locals);
    assert_eq!(v.get(&2).map(String::as_str), Some("a = 3  b = 4"), "signature line");
    assert_eq!(v.get(&3).map(String::as_str), Some("a = 3  b = 4  sum = 7"));
    assert_eq!(v.get(&4), None, "a name in a comment does not count");
    assert_eq!(v.get(&5).map(String::as_str), Some("sum = 7"));
    assert_eq!(v.get(&0), None, "other function");
    assert!(inline_values(&src, 5, &[]).is_empty() && inline_values(&src, 99, &locals).is_empty());
    let long = inline_values(&src, 3, &[("sum".into(), "x".repeat(50))]);
    assert!(long[&3].ends_with('…') && long[&3].chars().count() < 45);
}

#[test]
fn the_stop_on_entry_reads_as_entry_and_a_later_sigstop_as_an_exception() {
    let stop = |reason: &str, text: Option<&str>, entry| stop_message(reason.into(), text.map(str::to_string), entry);
    let entry = ("entry".to_string(), Some(("console", "Paused on entry".to_string())));
    // lldb-dap's stop on entry, the first stop of a stopOnEntry session.
    assert_eq!(stop("exception", Some("signal SIGSTOP"), true), entry);
    assert_eq!(stop("entry", None, false), entry, "an adapter that says entry");
    // Not the first stop, or no stopOnEntry: a SIGSTOP is a real signal.
    let real = ("exception".to_string(), Some(("stderr", "Paused on exception: signal SIGSTOP".to_string())));
    assert_eq!(stop("exception", Some("signal SIGSTOP"), false), real);
    // Another exception on the first stop stays one.
    assert_eq!(stop("exception", Some("signal SIGSEGV"), true).0, "exception");
    assert_eq!(stop("breakpoint", None, true), ("breakpoint".to_string(), None));
}

#[test]
fn console_lines_are_cut_and_capped_whatever_the_adapter_sends() {
    let (mut console, mut partial) = (ConsoleLines::new(), None);
    // 300 chunks of 64 KB without a newline: the unfinished line never grows past PARTIAL_LIMIT.
    let chunk = "x".repeat(64 * 1024);
    for _ in 0..300 {
        console_append(&mut console, &mut partial, "stdout", &chunk);
        assert!(partial.as_ref().is_none_or(|(_, p): &(String, String)| p.len() <= PARTIAL_LIMIT));
    }
    assert!(!console.is_empty() && console.iter().all(|(_, l)| l.chars().count() <= LINE_LIMIT + 1), "every line is cut");
    // Many short lines: the console keeps CONSOLE_LIMIT.
    console_append(&mut console, &mut partial, "stdout", &"a\n".repeat(CONSOLE_LIMIT * 2));
    assert_eq!(console.len(), CONSOLE_LIMIT);
    console_push(&mut console, "stderr", &"y".repeat(LINE_LIMIT * 3));
    assert_eq!(console.len(), CONSOLE_LIMIT);
    assert!(console.last().unwrap().1.chars().count() <= LINE_LIMIT + 1);
}
