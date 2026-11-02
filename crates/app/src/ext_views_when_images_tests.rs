//! Extension views (#103): `image` elements with an `https` URL, fetched through the extension's `net` permission from
//! a fake server, and a view's `when`.

use std::sync::Arc;

use ext_api::value::Why;

use super::*;
use crate::ext_fetch::tests::Fake;

/// 1×1 PNG.
const PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120,
    218, 99, 100, 96, 248, 95, 15, 0, 2, 135, 1, 128, 235, 71, 186, 146, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

fn image(url: &str) -> Node {
    Node::new(Element::Image { source: ImageSource::Url(HttpsUrl::parse(url).unwrap()), alt: format!("alt of {url}") })
}

/// A fake server with an image, one too large, one that is not an image and one that is missing.
fn server() -> Arc<Fake> {
    let fake = Fake::default();
    {
        let mut routes = fake.routes.lock().unwrap();
        routes.push(("https://api.example.com/logo.png".into(), 200, PNG.to_vec()));
        routes.push(("https://api.example.com/big.png".into(), 200, [PNG, &vec![0; ext_manifest::limits::UI_IMAGE_BYTES]].concat()));
        routes.push(("https://api.example.com/page.html".into(), 200, b"<html></html>".to_vec()));
    }
    Arc::new(fake)
}

fn sent_urls(fake: &Fake) -> Vec<String> {
    fake.sent().into_iter().map(|s| s.url).collect()
}

fn use_server(wb: &Entity<Workbench>, cx: &mut VisualTestContext, fake: &Arc<Fake>) {
    let t: Arc<dyn crate::ext_fetch::Transport> = fake.clone();
    wb.update(cx, |wb, _| wb.ext_view_transport_for_test(t));
}

#[gpui::test]
fn url_images_come_from_declared_hosts_only_and_are_kept_by_url(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx, "images", true);
    let fake = server();
    use_server(&wb, cx, &fake);
    open(&wb, cx, SIDE);
    let urls = [
        "https://api.example.com/logo.png",
        "https://tracker.example.org/pixel.png",
        "https://api.example.com/big.png",
        "https://api.example.com/page.html",
        "https://api.example.com/missing.png",
    ];
    render(&wb, cx, SIDE, &column(urls.iter().map(|u| image(u))));
    cx.run_until_parked();
    let remote = wb.read_with(cx, |wb, _| wb.ext_view_remote_for_test(SIDE));
    let state = |url: &str| remote.iter().find(|(u, _, _)| u == url).map(|(_, drawn, failed)| (*drawn, *failed));
    assert_eq!(state(urls[0]), Some((true, false)), "the image from a host in `net` is drawn: {remote:?}");
    for u in &urls[1..] {
        assert_eq!(state(u), Some((false, true)), "{u} keeps its alt text: {remote:?}");
    }
    // Nothing was sent to the host the extension did not declare; the refusal is in the Inspector with its fix.
    assert!(!sent_urls(&fake).iter().any(|u| u.contains("tracker.example.org")), "{:?}", sent_urls(&fake));
    let denials = wb.read_with(cx, |wb, _| wb.denials_for_test("you.probe"));
    assert!(
        denials.iter().any(|(call, perm, why, fix)| call == "ui.image"
            && perm == r#"net = ["tracker.example.org"]"#
            && *why == Why::NotDeclared
            && fix.as_deref() == Some(r#"net = ["tracker.example.org"]"#)),
        "{denials:?}"
    );
    // The view draws again with the same image: it is not fetched again.
    let before = sent_urls(&fake).len();
    render(&wb, cx, SIDE, &column([text("again"), image(urls[0])]));
    cx.run_until_parked();
    assert_eq!(sent_urls(&fake).len(), before, "kept by URL while the view is open");
    // The image is drawn: the screen reader has it under its alt text.
    let nodes = wb.update_in(cx, |wb, window, _| wb.ext_view_access_for_test(window));
    assert!(nodes.iter().any(|(r, l, _)| r.starts_with("Image") && l == &format!("alt of {}", urls[0])), "{nodes:#?}");
}

#[gpui::test]
fn a_view_off_screen_fetches_nothing_until_it_is_shown(cx: &mut TestAppContext) {
    let (wb, cx) = setup(cx, "images-hidden", true);
    let fake = server();
    use_server(&wb, cx, &fake);
    open(&wb, cx, SIDE);
    wb.update_in(cx, |wb, window, cx| wb.show_panel_named("explorer", window, cx));
    cx.run_until_parked();
    assert_eq!(wb.read_with(cx, |wb, _| wb.ext_view_shown_for_test()).0, None);
    // An `invalidate` while the view is hidden brings a tree with an image.
    render(&wb, cx, SIDE, &column([image("https://api.example.com/logo.png")]));
    cx.run_until_parked();
    assert!(fake.sent().is_empty(), "a view not on screen fetched {:?}", sent_urls(&fake));
    open(&wb, cx, SIDE);
    cx.run_until_parked();
    assert_eq!(sent_urls(&fake), ["https://api.example.com/logo.png"]);
}

#[gpui::test]
fn url_images_are_refused_in_an_untrusted_folder(cx: &mut TestAppContext) {
    let (wb, cx) = setup_with(cx, "images-untrusted", true, MANIFEST, Some(r#"{ "security.workspaceTrust.enabled": true }"#));
    let fake = server();
    use_server(&wb, cx, &fake);
    let project = config_dir("images-untrusted-project");
    let root = project.to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_path(&root, window, cx));
    cx.run_until_parked();
    assert!(!wb.read_with(cx, |wb, cx| wb.is_trusted_root_for_test(&root, cx)));
    open(&wb, cx, SIDE);
    render(&wb, cx, SIDE, &column([image("https://api.example.com/logo.png")]));
    cx.run_until_parked();
    assert!(fake.sent().is_empty(), "an untrusted folder sends nothing: {:?}", sent_urls(&fake));
    assert_eq!(wb.read_with(cx, |wb, _| wb.ext_view_remote_for_test(SIDE)), [("https://api.example.com/logo.png".to_string(), false, true)]);
    let denials = wb.read_with(cx, |wb, _| wb.denials_for_test("you.probe"));
    assert!(denials.iter().any(|(call, _, why, fix)| call == "ui.image" && *why == Why::UntrustedWorkspace && fix.is_none()), "{denials:?}");
}

const WHEN_MANIFEST: &str = r#"[extension]
id = "you.probe"
version = "1.0.0"
name = "Probe"
license = "MIT"

[code]
entry = "Cargo.toml"

[permissions]
read = ["a.txt"]
net = ["api.example.com"]
auth = ["github"]
secrets = true
why = "Tests every part of the host."

[[views]]
id = "counter"
title = "Counter"
location = "sidebar"
when = "editor.language == 'rust'"

[[views]]
id = "log"
title = "Build Log"
location = "bottom"
when = "editor && !ext.never"

[[views]]
id = "board"
title = "Board"
location = "editor"
"#;

fn labels(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<String> {
    wb.read_with(cx, |wb, _| wb.command_labels_for_test())
}

#[gpui::test]
fn a_view_is_offered_while_its_when_holds_and_closes_when_it_turns_false(cx: &mut TestAppContext) {
    let (wb, cx) = setup_with(cx, "when", true, WHEN_MANIFEST, None);
    // No file is open: only the view without a `when` is offered.
    let l = labels(&wb, cx);
    assert!(l.iter().any(|l| l == "Open View: Board (you)"), "{l:?}");
    assert!(!l.iter().any(|l| l == "Open View: Counter (you)" || l == "Open View: Build Log (you)"), "{l:?}");
    assert!(cx.debug_bounds("act-ext-view-0").is_none(), "no activity bar button");
    open(&wb, cx, SIDE);
    assert_eq!(wb.read_with(cx, |wb, _| wb.ext_view_shown_for_test()).0, None, "a view whose `when` is false does not open");
    // A Rust file is open: both are offered and open.
    let dir = config_dir("when-files");
    std::fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(dir.join("notes.txt"), "notes\n").unwrap();
    let rs = dir.join("main.rs").to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_path(&rs, window, cx));
    cx.run_until_parked();
    let l = labels(&wb, cx);
    assert!(l.iter().any(|l| l == "Open View: Counter (you)") && l.iter().any(|l| l == "Open View: Build Log (you)"), "{l:?}");
    assert!(cx.debug_bounds("act-ext-view-0").is_some(), "the activity bar button is back");
    open(&wb, cx, SIDE);
    open(&wb, cx, BOTTOM);
    assert!(cx.debug_bounds("ext-view-tab-0").is_some(), "the panel tab is shown");
    let shown = wb.read_with(cx, |wb, _| wb.ext_view_shown_for_test());
    assert_eq!((shown.0.as_deref(), shown.1.as_deref()), (Some(SIDE), Some(BOTTOM)));
    // A text file in front: the Rust view closes and leaves the palette; the other stays.
    let txt = dir.join("notes.txt").to_string_lossy().into_owned();
    wb.update_in(cx, |wb, window, cx| wb.open_path(&txt, window, cx));
    cx.run_until_parked();
    let shown = wb.read_with(cx, |wb, _| wb.ext_view_shown_for_test());
    assert_eq!((shown.0.as_deref(), shown.1.as_deref()), (None, Some(BOTTOM)));
    assert!(wb.read_with(cx, |wb, _| wb.ext_view_error_for_test(SIDE)).is_none());
    let l = labels(&wb, cx);
    assert!(!l.iter().any(|l| l == "Open View: Counter (you)"), "{l:?}");
}
