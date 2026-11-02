//! Extension views (#103): where they appear, what the host keeps across renders, the 22 elements by keyboard and
//! screen reader, 10k-row lists, theme changes without an extension call, the consent sheet out of reach, and a real
//! exthost drawing a view and hearing its clicks.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ext_ui::{Choice, Element, HttpsUrl, IconName, ImageSource, Node, PageToken, Pair, Selection, Space, Tone};
use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::ui::ActiveTheme;
use crate::workbench::Workbench;

const PROBE: &[u8] = include_bytes!("../../ext-host/testdata/probe.wasm");

const MANIFEST: &str = r#"[extension]
id = "you.probe"
version = "1.0.0"
name = "Probe"
license = "MIT"

[code]
entry = "Cargo.toml"

[permissions]
read = ["a.txt"]
net = ["api.example.com"]
# The probe also imports `auth` and `secrets` (#105).
auth = ["github"]
secrets = true
why = "Tests every part of the host."

[[views]]
id = "counter"
title = "Counter"
location = "sidebar"

[[views]]
id = "log"
title = "Build Log"
location = "bottom"

[[views]]
id = "board"
title = "Board"
location = "editor"
"#;

pub(crate) const SIDE: &str = "you.probe.counter";
const BOTTOM: &str = "you.probe.log";
const EDITOR: &str = "you.probe.board";

#[path = "ext_views_when_images_tests.rs"]
mod when_images;

fn config_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("codeyard-extviews-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn install(cfg: &Path, manifest: &str) {
    let entries = vec![
        ext_pkg::Entry { path: ext_manifest::FILE_NAME.into(), data: manifest.as_bytes().to_vec(), executable: false },
        ext_pkg::Entry { path: "Cargo.toml".into(), data: Vec::new(), executable: false },
        ext_pkg::Entry { path: ext_pkg::COMPONENT.into(), data: PROBE.to_vec(), executable: false },
    ];
    let (bytes, _) = ext_pkg::write(entries).expect("the probe passes the import check");
    crate::ext::store(cfg).install(&bytes, ext_core::Source::File { path: "/downloads/probe.cyx".into() }).expect("installs");
}

/// A window with the probe installed. `offline`: requests and events are recorded instead of sent.
pub(crate) fn setup<'a>(cx: &'a mut TestAppContext, name: &str, offline: bool) -> (Entity<Workbench>, &'a mut VisualTestContext) {
    setup_with(cx, name, offline, MANIFEST, None)
}

/// `setup` with another manifest for the probe, and a `settings.jsonc`.
fn setup_with<'a>(
    cx: &'a mut TestAppContext,
    name: &str,
    offline: bool,
    manifest: &str,
    settings: Option<&str>,
) -> (Entity<Workbench>, &'a mut VisualTestContext) {
    let cfg = config_dir(name);
    install(&cfg, manifest);
    if let Some(s) = settings {
        std::fs::write(cfg.join("settings.jsonc"), s).unwrap();
    }
    cx.update(|cx| {
        crate::bind_keys(cx);
        cx.set_global(ActiveTheme(theme::DARK));
        cx.set_global(crate::settings::Settings { lsp_enabled: false, ..Default::default() });
    });
    let (wb, cx) = cx.add_window_view(move |window, cx| Workbench::with_state(state::State::in_memory().ok(), Some(cfg), window, cx));
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    wb.update_in(cx, |wb, window, cx| {
        // Offline, the probe never runs: it is loaded even where the exthost or its sandbox is missing.
        let _guard = offline.then(crate::ext_runtime::load_without_exthost_for_test);
        wb.reload_config_for_test(window, cx);
        if offline {
            wb.ext_views_offline_for_test();
        }
    });
    cx.run_until_parked();
    (wb, cx)
}

pub(crate) fn open(wb: &Entity<Workbench>, cx: &mut VisualTestContext, id: &str) {
    let id = id.to_string();
    wb.update_in(cx, |wb, window, cx| wb.open_ext_view_for_test(&id, window, cx));
    cx.run_until_parked();
}

pub(crate) fn render(wb: &Entity<Workbench>, cx: &mut VisualTestContext, id: &str, tree: &Node) {
    let (id, json) = (id.to_string(), ext_ui::encode_json(tree));
    wb.update_in(cx, |wb, window, cx| wb.ext_view_render_for_test(&id, &json, window, cx));
    cx.run_until_parked();
}

/// A key pressed and released (buttons, checkboxes and links act on the release, as everywhere in the app).
fn press(cx: &mut VisualTestContext, key: &str) {
    let keystroke = gpui::Keystroke::parse(key).unwrap();
    cx.update(|window, cx| {
        window.dispatch_event(gpui::PlatformInput::KeyDown(gpui::KeyDownEvent { keystroke: keystroke.clone(), is_held: false }), cx);
        window.dispatch_event(gpui::PlatformInput::KeyUp(gpui::KeyUpEvent { keystroke }), cx);
    });
    cx.run_until_parked();
}

pub(crate) fn text(t: &str) -> Node {
    Node::new(Element::Text { text: t.into(), tone: Tone::Neutral })
}

pub(crate) fn column(children: impl IntoIterator<Item = Node>) -> Node {
    Node::new(Element::Column { gap: Space(2), pad: Space(0) }).children(children)
}

pub(crate) fn button(key: &str, label: &str) -> Node {
    Node::new(Element::Button { label: label.into(), icon: None, tone: Tone::Neutral, disabled: false }).key(key)
}

fn input(key: &str, value: &str) -> Node {
    Node::new(Element::TextInput { label: "Name".into(), placeholder: "your name".into(), value: value.into(), multiline: false, disabled: false }).key(key)
}

fn item(key: &str, more: Option<&str>) -> Node {
    Node::new(Element::Tree { label: key.into(), description: None, icon: None, more: more.map(|m| PageToken(m.into())), selection: Selection::None }).key(key)
}

/// One of each of the 22 elements.
fn every_element() -> Node {
    // 1×1 PNG.
    let png = vec![
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84,
        120, 218, 99, 100, 96, 248, 95, 15, 0, 2, 135, 1, 128, 235, 71, 186, 146, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    column([
        Node::new(Element::Heading { text: "Deployments".into(), level: 1 }),
        Node::new(Element::Banner { text: "Token expires soon".into(), tone: Tone::Warning }),
        Node::new(Element::Row { gap: Space(2), pad: Space(1) }).children([
            Node::new(Element::Icon { icon: IconName("rocket".into()), tone: Tone::Accent, label: Some("Live".into()) }),
            text("Production"),
            Node::new(Element::Badge { text: "ready".into(), tone: Tone::Success }),
        ]),
        Node::new(Element::Image { source: ImageSource::Bytes(png), alt: "Logo".into() }),
        Node::new(Element::Button { label: "Deploy".into(), icon: Some(IconName("play".into())), tone: Tone::Accent, disabled: false }).key("deploy"),
        input("name", "web"),
        Node::new(Element::Checkbox { label: "Preview only".into(), checked: true, disabled: false }).key("preview"),
        Node::new(Element::Select {
            label: "Region".into(),
            options: vec![Choice { key: "fra".into(), label: "Frankfurt".into() }, Choice { key: "iad".into(), label: "Virginia".into() }],
            selected: Some("fra".into()),
            disabled: false,
        })
        .key("region"),
        Node::new(Element::List { label: "Recent".into(), selection: Selection::One }).key("recent").children([text("one").key("1"), text("two").key("2")]),
        Node::new(Element::Tree { label: "Projects".into(), description: None, icon: None, more: Some(PageToken("p2".into())), selection: Selection::One })
            .key("projects")
            .children([item("web", None).child(item("api", None)), item("docs", Some(""))]),
        Node::new(Element::Table { label: "Builds".into(), columns: vec!["Id".into(), "State".into()], selection: Selection::None })
            .key("builds")
            .child(Node::new(Element::Row { gap: Space(0), pad: Space(0) }).key("b1").children([text("b1"), text("ok")])),
        Node::new(Element::Progress { label: "Uploading".into(), percent: Some(40) }),
        Node::new(Element::Markdown { source: "# Notes\n\nSee [the docs](https://example.com) ![remote](https://example.com/a.png)\n".into() }),
        Node::new(Element::Code { source: "fn main() {}\n".into(), language: Some("rust".into()) }),
        Node::new(Element::KeyValue { pairs: vec![Pair { name: "Region".into(), value: "fra1".into() }] }),
        Node::new(Element::Link { label: "Dashboard".into(), url: Some(HttpsUrl::parse("https://example.com/dash").unwrap()) }).key("dash"),
        Node::new(Element::Divider),
        Node::new(Element::EmptyState { title: "No deployments".into(), text: "Push to deploy.".into(), icon: Some(IconName("cloud".into())) })
            .child(button("connect", "Connect")),
    ])
}

#[gpui::test]
fn views_appear_in_their_places_and_name_the_publisher(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx, "places", true);
    let labels = wb.read_with(cx, |wb, _| wb.command_labels_for_test());
    for title in ["Counter", "Build Log", "Board"] {
        assert!(labels.iter().any(|l| l == &format!("Open View: {title} (you)")), "{labels:?}");
    }
    assert!(cx.debug_bounds("act-ext-view-0").is_some(), "the sidebar view has an activity bar button");
    open(&wb, cx, SIDE);
    open(&wb, cx, BOTTOM);
    open(&wb, cx, EDITOR);
    let shown = wb.read_with(cx, |wb, _| wb.ext_view_shown_for_test());
    assert_eq!(shown, (Some(SIDE.into()), Some(BOTTOM.into()), Some(EDITOR.into())));
    assert_eq!(wb.read_with(cx, |wb, _| wb.ext_view_renders_for_test()), 3, "opening each view asks for its tree once");
    assert!(cx.debug_bounds("ext-view-tab-0").is_some(), "the bottom view has a panel tab");
    // Each view's frame names the publisher.
    for id in [SIDE, BOTTOM, EDITOR] {
        render(&wb, cx, id, &column([text(id)]));
    }
    for sel in ["ext-view-publisher-0", "ext-view-publisher-1", "ext-view-publisher-2"] {
        assert!(cx.debug_bounds(sel).is_some(), "{sel}: every view shows its publisher");
    }
}

#[gpui::test]
fn renders_are_diffed_by_key_and_input_survives_them(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx, "diff", true);
    open(&wb, cx, SIDE);
    render(&wb, cx, SIDE, &column([text("a"), input("name", "web"), button("go", "Go")]));
    // The user types; the extension draws again with the same starting value: the typed text stays.
    wb.update_in(cx, |wb, window, cx| wb.focus_ext_view_for_test(SIDE, window, cx));
    cx.simulate_keystrokes("end - a p p");
    assert_eq!(wb.read_with(cx, |wb, _| wb.ext_view_text_for_test(SIDE, &["name"])).as_deref(), Some("web-app"));
    render(&wb, cx, SIDE, &column([text("b"), input("name", "web"), button("go", "Go")]));
    assert_eq!(wb.read_with(cx, |wb, _| wb.ext_view_text_for_test(SIDE, &["name"])).as_deref(), Some("web-app"));
    assert_eq!(wb.read_with(cx, |wb, _| wb.ext_view_last_ops_for_test(SIDE)), 1, "one text changed: one operation");
    // A pause in typing sends one `change`; Enter sends `submit`.
    cx.executor().advance_clock(Duration::from_millis(600));
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    let events = wb.read_with(cx, |wb, _| wb.ext_view_events_for_test());
    assert!(events.contains(&r#"{"change":{"path":["name"],"value":{"text":"web-app"}}}"#.to_string()), "{events:?}");
    assert!(events.contains(&r#"{"submit":{"path":["name"],"text":"web-app"}}"#.to_string()), "{events:?}");
    // A refused tree is shown as an error in place of the view, with the reason.
    let id = SIDE.to_string();
    wb.update_in(cx, |wb, window, cx| wb.ext_view_render_for_test(&id, r#"{"element": {"buton": {}}}"#, window, cx));
    assert!(wb.read_with(cx, |wb, _| wb.ext_view_error_for_test(SIDE)).is_some_and(|e| e.contains("buton")));
    // `invalidate` asks again; at most one request is on its way.
    let before = wb.read_with(cx, |wb, _| wb.ext_view_renders_for_test());
    wb.update_in(cx, |wb, window, cx| {
        wb.ext_view_invalidate_for_test("you.probe", "counter", window, cx);
        wb.ext_view_invalidate_for_test("you.probe", "counter", window, cx);
        wb.ext_view_invalidate_for_test("you.probe", "log", window, cx);
    });
    assert_eq!(wb.read_with(cx, |wb, _| wb.ext_view_renders_for_test()), before + 1, "a view not opened is not drawn; two invalidations are one render");
}

#[gpui::test]
fn ten_thousand_rows_build_only_the_rows_on_screen(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx, "rows", true);
    open(&wb, cx, EDITOR);
    let rows = (0..10_000).map(|i| text(&format!("row {i}")).key(format!("r{i}").as_str()));
    let list = Node::new(Element::List { label: "Rows".into(), selection: Selection::One }).key("rows").children(rows);
    render(&wb, cx, EDITOR, &column([Node::new(Element::Heading { text: "Big".into(), level: 2 }), list]));
    let built = wb.read_with(cx, |wb, _| wb.ext_view_built_rows_for_test())["rows"];
    assert!(built > 0 && built < 100, "{built} rows were built for one screen");
    // The keyboard walks the list; every keystroke is one frame of on-screen rows.
    wb.update_in(cx, |wb, window, cx| wb.focus_ext_view_for_test(EDITOR, window, cx));
    assert_eq!(wb.update_in(cx, |wb, window, _| wb.ext_view_focused_for_test(EDITOR, window)).as_deref(), Some("rows"));
    let start = Instant::now();
    let keys = 60;
    for _ in 0..keys {
        cx.simulate_keystrokes("pagedown");
        cx.run_until_parked();
    }
    let per_key = start.elapsed() / keys;
    let built = wb.read_with(cx, |wb, _| wb.ext_view_built_rows_for_test())["rows"];
    assert!(built < 100, "{built} rows were built after scrolling");
    // Generous for debug builds on shared CI machines; a frame that walked all 10k rows would take far longer.
    assert!(per_key < Duration::from_millis(250), "keystroke to frame took {per_key:?}");
    cx.simulate_keystrokes("space");
    let events = wb.read_with(cx, |wb, _| wb.ext_view_events_for_test());
    assert_eq!(events.last().map(String::as_str), Some(r#"{"select":{"path":["rows"],"rows":["r600"]}}"#), "{events:?}");
}

#[gpui::test]
fn every_element_is_reachable_by_keyboard_and_announced(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx, "access", true);
    open(&wb, cx, EDITOR);
    render(&wb, cx, EDITOR, &every_element());
    assert_eq!(wb.read_with(cx, |wb, _| wb.ext_view_error_for_test(EDITOR)), None);
    // Tab goes through every interactive element once, in order.
    let order = wb.read_with(cx, |wb, _| wb.ext_view_focus_order_for_test(EDITOR));
    assert_eq!(order, ["deploy", "name", "preview", "region", "recent", "projects", "builds", "dash", "connect"]);
    wb.update_in(cx, |wb, window, cx| wb.focus_ext_view_for_test(EDITOR, window, cx));
    let mut seen = Vec::new();
    for _ in 0..order.len() {
        let now = wb.update_in(cx, |wb, window, _| wb.ext_view_focused_for_test(EDITOR, window));
        seen.push(now.unwrap_or_default());
        cx.simulate_keystrokes("tab");
    }
    assert_eq!(seen, order, "Tab order");
    // Each element is announced with its role and label.
    let nodes = wb.update_in(cx, |wb, window, _| wb.ext_view_access_for_test(window));
    let has = |role: &str, label: &str| nodes.iter().any(|(r, l, _)| r.starts_with(role) && l.contains(label));
    for (role, label) in [
        ("Group", "Board, by you"),
        ("Heading", "Deployments"),
        ("Alert", "Token expires soon"),
        ("Image", "Live"),
        ("Label", "Production"),
        ("Status", "ready"),
        ("Image", "Logo"),
        ("Button", "Deploy"),
        ("TextInput", "Name"),
        ("CheckBox", "Preview only"),
        ("ComboBox", "Region"),
        ("List", "Recent"),
        ("ListItem", "two"),
        ("Tree", "Projects"),
        ("TreeItem", "docs"),
        ("Button", "Load more"),
        ("Table", "Builds"),
        ("Row", "b1, ok"),
        ("Cell", "ok"),
        ("ProgressIndicator", "Uploading"),
        ("Document", "Notes"),
        ("Code", "fn main"),
        ("DescriptionList", "Region: fra1"),
        ("Link", "Dashboard"),
        ("Separator", ""),
        ("Group", "No deployments"),
        ("Button", "Connect"),
    ] {
        assert!(has(role, label), "no {role} “{label}” in {nodes:#?}");
    }
    // Keys act on what has the keyboard: Space ticks the checkbox, Down picks the next region, Right opens a tree item.
    wb.update_in(cx, |wb, window, cx| wb.focus_ext_view_for_test(EDITOR, window, cx));
    cx.simulate_keystrokes("tab tab");
    press(cx, "space");
    cx.simulate_keystrokes("tab down tab tab right");
    let events = wb.read_with(cx, |wb, _| wb.ext_view_events_for_test());
    for e in [r#"{"change":{"path":["preview"],"value":{"checked":false}}}"#, r#"{"change":{"path":["region"],"value":{"choice":"iad"}}}"#] {
        assert!(events.iter().any(|x| x == e), "{e} not in {events:?}");
    }
    // Right opened "web"; below it "api", then "docs", which has no children yet but a `more` token: opening it asks
    // for its first page.
    cx.simulate_keystrokes("down down right");
    let events = wb.read_with(cx, |wb, _| wb.ext_view_events_for_test());
    assert!(events.iter().any(|x| x == r#"{"expand":{"path":["projects"],"item":"docs","page":""}}"#), "{events:?}");
}

#[gpui::test]
fn a_theme_change_redraws_views_without_asking_the_extension(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx, "theme", true);
    open(&wb, cx, SIDE);
    render(&wb, cx, SIDE, &every_element());
    let drawn = |wb: &Entity<Workbench>, cx: &mut VisualTestContext| wb.read_with(cx, |wb, _| wb.ext_view_drawn_with_for_test());
    let before = drawn(&wb, cx);
    let other = if before == theme::DARK.bg { theme::LIGHT } else { theme::DARK };
    let (renders, events) = wb.read_with(cx, |wb, _| (wb.ext_view_renders_for_test(), wb.ext_view_events_for_test().len()));
    cx.update(|window, cx| {
        cx.set_global(ActiveTheme(other));
        window.refresh();
    });
    cx.run_until_parked();
    assert_eq!(drawn(&wb, cx), other.bg, "the view was drawn again with the new theme");
    assert_eq!(
        wb.read_with(cx, |wb, _| (wb.ext_view_renders_for_test(), wb.ext_view_events_for_test().len())),
        (renders, events),
        "and the extension was not asked"
    );
}

#[gpui::test]
fn an_extension_cannot_cover_or_replace_the_consent_sheet(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx, "consent", true);
    // A sidebar and a bottom view are open and drawn.
    open(&wb, cx, SIDE);
    open(&wb, cx, BOTTOM);
    render(&wb, cx, SIDE, &every_element());
    render(&wb, cx, BOTTOM, &every_element());
    // Another package waits on the consent sheet.
    let src = config_dir("consent-src");
    std::fs::write(
        src.join("codeyard-extension.toml"),
        "[extension]\nid = \"you.night\"\nversion = \"1.0.0\"\nname = \"Night\"\nlicense = \"MIT\"\n[[themes]]\nlabel = \"Night\"\npath = \"t.json\"\n",
    )
    .unwrap();
    std::fs::write(src.join("t.json"), r##"{"name":"x","colors":{"editor.background":"#101418"}}"##).unwrap();
    let path = src.to_string_lossy().to_string();
    wb.update_in(cx, |wb, window, cx| wb.review_extension_for_test(&path, false, window, cx));
    cx.run_until_parked();
    let focus = wb.update_in(cx, |wb, window, _| wb.consent_for_test(window)).expect("the sheet is shown").3;
    // The extension opens its editor view, draws all its views again and asks for the keyboard: the sheet stays in
    // front, keeps the keyboard, and no view's frame overlaps it.
    open(&wb, cx, EDITOR);
    for id in [SIDE, BOTTOM, EDITOR] {
        render(&wb, cx, id, &every_element());
    }
    wb.update_in(cx, |wb, window, cx| {
        wb.ext_view_invalidate_for_test("you.probe", "counter", window, cx);
        wb.focus_ext_view_for_test(SIDE, window, cx);
    });
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.page_for_test()), Some(("extension", true)), "the consent page is still in front");
    assert_eq!(wb.update_in(cx, |wb, window, _| wb.consent_for_test(window)).unwrap().3, focus, "and keeps the keyboard");
    let sheet = cx.debug_bounds("container-button-Install").expect("Install is drawn");
    let cancel = cx.debug_bounds("container-button-Cancel").expect("Cancel is drawn");
    let frames: Vec<_> = ["ext-view-0", "ext-view-1", "ext-view-2"].into_iter().filter_map(|sel| cx.debug_bounds(sel)).collect();
    assert!(frames.len() >= 2, "the sidebar and bottom views are drawn");
    for f in frames {
        assert!(!f.intersects(&sheet) && !f.intersects(&cancel), "a view frame {f:?} overlaps the sheet");
    }
    // The buttons still take the click.
    cx.simulate_click(cancel.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert!(wb.update_in(cx, |wb, window, _| wb.consent_for_test(window)).is_none(), "Cancel answered the sheet");
}

/// codeyard-exthost runs the probe: opening its view starts it with `view:counter`, its tree is drawn, a click goes back
/// as `ui-event`, and its `ui.invalidate` draws the new count.
#[gpui::test]
fn a_real_extension_draws_its_view_and_hears_clicks(cx: &mut TestAppContext) {
    if !crate::ext_runtime::code_runs_for_test() {
        return;
    }
    let (wb, cx) = setup(cx, "probe", false);
    open(&wb, cx, SIDE);
    let wait = |cx: &mut VisualTestContext, want: &str| {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            cx.run_until_parked();
            if wb.read_with(cx, |wb, _| wb.ext_view_texts_for_test(SIDE)).iter().any(|t| t == want) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for “{want}”; error: {:?}", wb.read_with(cx, |wb, _| wb.ext_view_error_for_test(SIDE)));
    };
    wait(cx, "counter: 0 clicks");
    assert!(cx.debug_bounds("ext-view-publisher-0").is_some(), "the view is drawn with its publisher");
    wb.update_in(cx, |wb, window, cx| wb.focus_ext_view_for_test(SIDE, window, cx));
    press(cx, "enter");
    wait(cx, "counter: 1 clicks");
}
