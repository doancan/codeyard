//! The JSON form extensions send (`response.view`) and receive (`request.ui-event`), and the reference page that
//! documents it (docs/extensions/reference/ui.md).

use crate::tests::every_element;
use crate::*;

#[test]
fn every_element_round_trips_as_json() {
    let view = every_element();
    let json = encode_json(&view);
    assert_eq!(decode_json(&json).unwrap(), view);
    // Kinds and tones by the names authors read in the reference.
    assert!(json.contains(r#""text-input":"#) && json.contains(r#""tone":"warning""#), "{json}");
    // Image bytes travel as base64.
    assert!(json.contains(r#""bytes":"iVBORw==""#), "{json}");
}

#[test]
fn optional_properties_may_be_left_out() {
    let json = r#"{"element": {"column": {}}, "children": [
        {"element": {"text": {"text": "Hello"}}},
        {"key": "go", "element": {"button": {"label": "Go", "tone": "accent"}}},
        {"element": "divider"}
    ]}"#;
    let view = decode_json(json).unwrap();
    assert_eq!(view.children.len(), 3);
    assert_eq!(view.children[1].element, Element::Button { label: "Go".into(), icon: None, tone: Tone::Accent, disabled: false });
}

#[test]
fn json_mistakes_are_refused_with_a_reason() {
    let malformed = |json: &str| match decode_json(json) {
        Err(DecodeError::Malformed(e)) => e,
        other => panic!("{json} should be malformed, got {other:?}"),
    };
    assert!(malformed(r#"{"element": {"buton": {"label": "Go"}}}"#).contains("buton"));
    assert!(malformed(r#"{"element": {"button": {"label": "Go", "colour": "red"}}}"#).contains("colour"));
    assert!(malformed(r#"{"element": {"text": {"text": "a"}}, "kids": []}"#).contains("kids"));
    assert!(malformed(r#"{"element": {"image": {"source": {"bytes": "!!"}, "alt": ""}}}"#).contains("base64"));
    // Deep JSON stops in the parser, before anything recurses far.
    let deep = format!("{}{}{}", r#"{"element": {"column": {}}, "children": ["#.repeat(500), r#"{"element": "divider"}"#, "]}".repeat(500));
    assert!(malformed(&deep).contains("recursion"));
    // Well formed but too deep for a view.
    let nested = format!("{}{}{}", r#"{"element": {"column": {}}, "children": ["#.repeat(40), r#"{"element": "divider"}"#, "]}".repeat(40));
    assert!(matches!(decode_json(&nested), Err(DecodeError::Invalid(_))));
    // The size cap comes first.
    assert!(matches!(decode_json(&" ".repeat(ext_manifest::limits::UI_VIEW_BYTES + 1)), Err(DecodeError::TooLarge(_))));
}

#[test]
fn events_are_json_with_kebab_case_names() {
    let e = Event::Expand { path: vec!["projects".into()], item: Some("web".into()), page: PageToken("p2".into()) };
    assert_eq!(e.to_json(), r#"{"expand":{"path":["projects"],"item":"web","page":"p2"}}"#);
    assert_eq!(Event::from_json(&e.to_json()).unwrap(), e);
    let c = Event::Change { path: vec!["preview".into()], value: Value::Checked(true) };
    assert_eq!(c.to_json(), r#"{"change":{"path":["preview"],"value":{"checked":true}}}"#);
}

/// The reference page names every element, and each of its examples is accepted as written.
#[test]
fn the_reference_page_matches_the_code() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/extensions/reference/ui.md");
    let doc = std::fs::read_to_string(path).expect("docs/extensions/reference/ui.md exists");
    for k in Kind::ALL {
        assert!(doc.contains(&format!("### `{}`", k.name())), "ui.md has no section for {k}");
    }
    let mut views = 0;
    let mut events = 0;
    let mut rest = doc.as_str();
    while let Some(start) = rest.find("```json") {
        let after = &rest[start + 7..];
        let (tag, body) = after.split_once('\n').expect("a fence line");
        let end = body.find("```").expect("a closing fence");
        let block = &body[..end];
        match tag.trim() {
            "view" => {
                decode_json(block).unwrap_or_else(|e| panic!("this example in ui.md is refused: {e}\n{block}"));
                views += 1;
            }
            "event" => {
                Event::from_json(block).unwrap_or_else(|e| panic!("this event in ui.md does not parse: {e}\n{block}"));
                events += 1;
            }
            _ => {}
        }
        rest = &body[end + 3..];
    }
    assert!(views >= 3 && events >= 6, "ui.md has {views} view and {events} event examples");
}
