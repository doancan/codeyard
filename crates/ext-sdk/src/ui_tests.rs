//! What `ui` builds is what Codeyard reads: every builder and option, decoded by `ext_ui::decode_json` and compared with
//! the tree ext-ui itself would send.

use ext_ui::{Choice, Element as E, HttpsUrl, IconName, ImageSource, Node as N, PageToken, Pair, Selection as S, Space, Tone as T};

use crate::types::Tone;
use crate::ui::{self, Selection};

fn decode(tree: impl serde::Serialize) -> N {
    let json = serde_json::to_string(&tree).unwrap();
    ext_ui::decode_json(&json).unwrap_or_else(|e| panic!("Codeyard refuses {json}: {e}"))
}

fn icon(s: &str) -> Option<IconName> {
    Some(IconName(s.into()))
}

#[test]
fn every_builder_writes_the_form_codeyard_reads() {
    let built = ui::column()
        .gap(2)
        .pad(1)
        .child(ui::row().gap(1).child(ui::text("plain")).child(ui::text("muted").tone(Tone::Muted)))
        .child(ui::heading("Title", 2))
        .child(ui::icon("refresh").tone(Tone::Accent).label("Refresh"))
        .child(ui::image_url("https://example.com/a.png", "A"))
        .child(ui::image_bytes(&[0x89, b'P', b'N', b'G'], ""))
        .child(ui::button("go", "Go").icon("play").tone(Tone::Accent).disabled(true))
        .child(ui::text_input("name", "Name").placeholder("web").value("api").multiline(true).disabled(false))
        .child(ui::checkbox("ok", "OK").checked(true))
        .child(ui::select("pick", "Pick", [("a", "A"), ("b", "B")]).selected("b"))
        .child(ui::list("files", "Files").selection(Selection::Many).child(ui::text("a.txt").key("a")))
        .child(
            ui::tree("projects", "Projects").selection(Selection::One).more("p2").child(ui::tree("web", "web").description("3 builds").icon("folder").more("")),
        )
        .child(ui::table("builds", "Builds", ["Id", "State"]).child(ui::row().key("b1").child(ui::text("b1")).child(ui::badge("ok").tone(Tone::Success))))
        .child(ui::progress("Loading").percent(40))
        .child(ui::progress("Waiting"))
        .child(ui::markdown("# Hi"))
        .child(ui::code("fn main() {}").language("rust"))
        .child(ui::key_value([("Region", "eu")]))
        .child(ui::banner("Careful").tone(Tone::Warning))
        .child(ui::link("docs", "Docs").url("https://example.com/docs"))
        .child(ui::link("help", "Help"))
        .child(ui::divider())
        .child(ui::empty_state("Nothing", "Yet").icon("inbox").child(ui::button("add", "Add")));

    let text = |t: &str, tone| N::new(E::Text { text: t.into(), tone });
    let want = N::new(E::Column { gap: Space(2), pad: Space(1) })
        .child(N::new(E::Row { gap: Space(1), pad: Space(0) }).child(text("plain", T::Neutral)).child(text("muted", T::Muted)))
        .child(N::new(E::Heading { text: "Title".into(), level: 2 }))
        .child(N::new(E::Icon { icon: IconName("refresh".into()), tone: T::Accent, label: Some("Refresh".into()) }))
        .child(N::new(E::Image { source: ImageSource::Url(HttpsUrl::parse("https://example.com/a.png").unwrap()), alt: "A".into() }))
        .child(N::new(E::Image { source: ImageSource::Bytes(vec![0x89, b'P', b'N', b'G']), alt: String::new() }))
        .child(N::new(E::Button { label: "Go".into(), icon: icon("play"), tone: T::Accent, disabled: true }).key("go"))
        .child(N::new(E::TextInput { label: "Name".into(), placeholder: "web".into(), value: "api".into(), multiline: true, disabled: false }).key("name"))
        .child(N::new(E::Checkbox { label: "OK".into(), checked: true, disabled: false }).key("ok"))
        .child(
            N::new(E::Select {
                label: "Pick".into(),
                options: vec![Choice { key: "a".into(), label: "A".into() }, Choice { key: "b".into(), label: "B".into() }],
                selected: Some("b".into()),
                disabled: false,
            })
            .key("pick"),
        )
        .child(N::new(E::List { label: "Files".into(), selection: S::Many }).key("files").child(text("a.txt", T::Neutral).key("a")))
        .child(
            N::new(E::Tree { label: "Projects".into(), description: None, icon: None, more: Some(PageToken("p2".into())), selection: S::One })
                .key("projects")
                .child(
                    N::new(E::Tree {
                        label: "web".into(),
                        description: Some("3 builds".into()),
                        icon: icon("folder"),
                        more: Some(PageToken(String::new())),
                        selection: S::None,
                    })
                    .key("web"),
                ),
        )
        .child(
            N::new(E::Table { label: "Builds".into(), columns: vec!["Id".into(), "State".into()], selection: S::None }).key("builds").child(
                N::new(E::Row { gap: Space(0), pad: Space(0) })
                    .key("b1")
                    .child(text("b1", T::Neutral))
                    .child(N::new(E::Badge { text: "ok".into(), tone: T::Success })),
            ),
        )
        .child(N::new(E::Progress { label: "Loading".into(), percent: Some(40) }))
        .child(N::new(E::Progress { label: "Waiting".into(), percent: None }))
        .child(N::new(E::Markdown { source: "# Hi".into() }))
        .child(N::new(E::Code { source: "fn main() {}".into(), language: Some("rust".into()) }))
        .child(N::new(E::KeyValue { pairs: vec![Pair { name: "Region".into(), value: "eu".into() }] }))
        .child(N::new(E::Banner { text: "Careful".into(), tone: T::Warning }))
        .child(N::new(E::Link { label: "Docs".into(), url: Some(HttpsUrl::parse("https://example.com/docs").unwrap()) }).key("docs"))
        .child(N::new(E::Link { label: "Help".into(), url: None }).key("help"))
        .child(N::new(E::Divider))
        .child(
            N::new(E::EmptyState { title: "Nothing".into(), text: "Yet".into(), icon: icon("inbox") })
                .child(N::new(E::Button { label: "Add".into(), icon: None, tone: T::Neutral, disabled: false }).key("add")),
        );
    assert_eq!(decode(&built), want);
}

#[test]
fn the_template_tree_is_the_reference_form() {
    let tree = ui::column().child(ui::heading("Clicks", 2)).child(ui::text("2")).child(ui::button("add", "Add one"));
    assert_eq!(
        serde_json::to_value(&tree).unwrap(),
        serde_json::json!({
            "element": {"column": {}},
            "children": [
                {"element": {"heading": {"text": "Clicks", "level": 2}}},
                {"element": {"text": {"text": "2"}}},
                {"key": "add", "element": {"button": {"label": "Add one"}}}
            ]
        })
    );
}

#[test]
fn image_bytes_are_padded_base64() {
    for len in 0..8 {
        let bytes: Vec<u8> = (0..len).map(|i| i * 37 + 1).collect();
        let json = serde_json::to_value(ui::image_bytes(&bytes, "")).unwrap();
        let text = json["element"]["image"]["source"]["bytes"].as_str().unwrap().to_string();
        let node: N = serde_json::from_value(json).unwrap();
        let E::Image { source: ImageSource::Bytes(b), .. } = node.element else { panic!("{text}") };
        assert_eq!(b, bytes, "{text}");
    }
}
