use super::*;

fn record(events: &mut Vec<Result<String, Fault>>, event: Event<'_>) {
    events.push(match event {
        Event::Line(line) => Ok(line.into()),
        Event::Fault(fault) => Err(fault),
    });
}

#[test]
fn task_output_preserves_unicode_and_strips_controls_for_every_chunk_boundary() {
    let text = "\x1b[31mşablon.ts:3:4: error: bad\x1b[0m\r\n";
    for split in 0..=text.len() {
        let mut decoder = Decoder::default();
        let mut events = Vec::new();
        decoder.push(&text.as_bytes()[..split], |event| record(&mut events, event));
        decoder.push(&text.as_bytes()[split..], |event| record(&mut events, event));
        decoder.finish(|event| record(&mut events, event));
        assert_eq!(events, [Ok("şablon.ts:3:4: error: bad".into())], "split {split}");
    }
}

#[test]
fn task_output_ignores_osc_titles_and_links_and_replaces_progress_before_matching() {
    let mut decoder = Decoder::default();
    let mut events = Vec::new();
    decoder.push(b"\x1b]0;fake READY\nnot a diagnostic\x1b\\10%\r\x1b[32mREADY\x1b[0m\r\n", |event| record(&mut events, event));
    decoder.push(b"\x1b]8;;https://example.invalid\x07file.ts\x1b]8;;\x07:1: error: actual\n", |event| record(&mut events, event));
    assert_eq!(events, [Ok("READY".into()), Ok("file.ts:1: error: actual".into())]);
}

#[test]
fn task_output_rejects_oversized_lines_and_gaps_without_matching_their_suffixes() {
    let mut decoder = Decoder::default();
    let mut events = Vec::new();
    decoder.push(&vec![b'x'; MAX_LINE + 1], |event| record(&mut events, event));
    decoder.push(b"fake.rs:1: error: suffix\nvalid.rs:2: error: next\npartial", |event| record(&mut events, event));
    decoder.gap(|event| record(&mut events, event));
    decoder.push(b"fake.rs:3: error: lost prefix\nrestored.rs:4: error: actual\n", |event| record(&mut events, event));
    assert_eq!(events, [Err(Fault::LongLine), Ok("valid.rs:2: error: next".into()), Err(Fault::Gap), Ok("restored.rs:4: error: actual".into())]);
}

#[test]
fn task_output_bounds_control_sequences_and_rejects_invalid_or_incomplete_utf8() {
    let mut decoder = Decoder::default();
    let mut events = Vec::new();
    decoder.push(b"\x1b[", |event| record(&mut events, event));
    decoder.push(&vec![b'1'; MAX_CONTROL + 1], |event| record(&mut events, event));
    decoder.push(b"mignored\ninvalid\xff\npartial\xc5", |event| record(&mut events, event));
    decoder.finish(|event| record(&mut events, event));
    assert_eq!(events, [Err(Fault::LongControl), Err(Fault::InvalidUtf8), Err(Fault::InvalidUtf8)]);
    events.clear();
    decoder.push(b"partial\x1b]unterminated", |event| record(&mut events, event));
    decoder.finish(|event| record(&mut events, event));
    assert_eq!(events, [Err(Fault::IncompleteControl)]);
}

#[test]
fn task_output_flushes_final_lines_and_handles_utf8_backspace_without_corrupting_paths() {
    let mut decoder = Decoder::default();
    let mut events = Vec::new();
    decoder.push("file ş\u{8}x.rs:1: error: final".as_bytes(), |event| record(&mut events, event));
    decoder.finish(|event| record(&mut events, event));
    assert_eq!(events, [Ok("file x.rs:1: error: final".into())]);
}
