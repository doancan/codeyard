use ext_manifest::limits;

use crate::*;

fn text(t: &str) -> Node {
    Node::new(Element::Text { text: t.into(), tone: Tone::Neutral })
}

fn button(key: &str, label: &str) -> Node {
    Node::new(Element::Button { label: label.into(), icon: None, tone: Tone::Neutral, disabled: false }).key(key)
}

fn column(children: impl IntoIterator<Item = Node>) -> Node {
    Node::new(Element::Column { gap: Space(1), pad: Space(0) }).children(children)
}

fn list(key: &str, rows: impl IntoIterator<Item = Node>) -> Node {
    Node::new(Element::List { label: "Files".into(), selection: Selection::Many }).key(key).children(rows)
}

fn item(key: &str, more: Option<&str>) -> Node {
    Node::new(Element::Tree { label: key.into(), description: None, icon: None, more: more.map(|t| PageToken(t.into())), selection: Selection::None }).key(key)
}

fn input(key: &str, value: &str) -> Node {
    Node::new(Element::TextInput { label: "Name".into(), placeholder: String::new(), value: value.into(), multiline: false, disabled: false }).key(key)
}

/// A valid view with every element kind.
pub(crate) fn every_element() -> Node {
    let icon = |n: &str| IconName(n.into());
    column([
        Node::new(Element::Heading { text: "Deployments".into(), level: 1 }),
        Node::new(Element::Banner { text: "Token expires soon".into(), tone: Tone::Warning }),
        Node::new(Element::Row { gap: Space(2), pad: Space(1) }).children([
            Node::new(Element::Icon { icon: icon("rocket"), tone: Tone::Accent, label: None }),
            text("Production"),
            Node::new(Element::Badge { text: "ready".into(), tone: Tone::Success }),
        ]),
        Node::new(Element::Image { source: ImageSource::Bytes(vec![0x89, b'P', b'N', b'G']), alt: "Logo".into() }),
        Node::new(Element::Image { source: ImageSource::Url(HttpsUrl::parse("https://api.vercel.com/a.png").unwrap()), alt: String::new() }),
        Node::new(Element::Button { label: "Deploy".into(), icon: Some(icon("play")), tone: Tone::Accent, disabled: false }).key("deploy"),
        input("name", "web"),
        Node::new(Element::Checkbox { label: "Preview only".into(), checked: true, disabled: false }).key("preview"),
        Node::new(Element::Select {
            label: "Region".into(),
            options: vec![Choice { key: "fra".into(), label: "Frankfurt".into() }, Choice { key: "iad".into(), label: "Virginia".into() }],
            selected: Some("fra".into()),
            disabled: false,
        })
        .key("region"),
        list("deploys", [text("one").key("1"), text("two").key("2")]),
        Node::new(Element::Tree { label: "Projects".into(), description: None, icon: None, more: Some(PageToken("p2".into())), selection: Selection::One })
            .key("projects")
            .children([item("web", Some("")).child(item("api", None)), item("docs", None)]),
        Node::new(Element::Table { label: "Builds".into(), columns: vec!["Id".into(), "State".into()], selection: Selection::None })
            .key("builds")
            .child(Node::new(Element::Row { gap: Space(0), pad: Space(0) }).key("b1").children([text("b1"), text("ok")])),
        Node::new(Element::Progress { label: "Uploading".into(), percent: Some(40) }),
        Node::new(Element::Markdown { source: "# Notes\n\n- one\n".into() }),
        Node::new(Element::Code { source: "fn main() {}\n".into(), language: Some("rust".into()) }),
        Node::new(Element::KeyValue { pairs: vec![Pair { name: "Region".into(), value: "fra1".into() }] }),
        Node::new(Element::Link { label: "Dashboard".into(), url: Some(HttpsUrl::parse("https://vercel.com/dash").unwrap()) }).key("dash"),
        Node::new(Element::Divider),
        Node::new(Element::EmptyState { title: "No deployments".into(), text: "Push to deploy.".into(), icon: Some(icon("cloud")) })
            .child(button("connect", "Connect")),
    ])
}

fn kinds(n: &Node, out: &mut Vec<Kind>) {
    out.push(n.kind());
    for c in &n.children {
        kinds(c, out);
    }
}

#[test]
fn every_element_round_trips() {
    let view = every_element();
    let mut seen = Vec::new();
    kinds(&view, &mut seen);
    for k in Kind::ALL {
        assert!(seen.contains(&k), "the sample has no {k}");
    }
    validate(&view).unwrap();
    let bytes = encode(&view);
    assert_eq!(decode(&bytes).unwrap(), view);
    // Compact: no field names on the wire.
    assert!(!bytes.windows(5).any(|w| w == b"label"));
}

#[test]
fn events_and_patches_round_trip() {
    let events = vec![
        Event::Click { path: vec!["deploy".into()] },
        Event::Change { path: vec!["name".into()], value: Value::Text("web".into()) },
        Event::Change { path: vec!["preview".into()], value: Value::Checked(false) },
        Event::Change { path: vec!["region".into()], value: Value::Choice("iad".into()) },
        Event::Submit { path: vec!["name".into()], text: "web".into() },
        Event::Select { path: vec!["deploys".into()], rows: vec!["1".into(), "2".into()] },
        Event::Expand { path: vec!["projects".into()], item: Some("web".into()), page: PageToken(String::new()) },
        Event::Link { path: vec!["dash".into()] },
    ];
    let bytes = postcard::to_stdvec(&events).unwrap();
    assert_eq!(postcard::from_bytes::<Vec<Event>>(&bytes).unwrap(), events);

    let ops = diff(&column([text("a")]), &every_element());
    let bytes = postcard::to_stdvec(&ops).unwrap();
    assert_eq!(postcard::from_bytes::<Vec<Op>>(&bytes).unwrap(), ops);
}

/// `view` is refused with a problem whose message contains `message`.
#[track_caller]
fn refused(view: Node, message: &str) {
    let problems = validate(&view).expect_err("view should be refused");
    assert!(problems.iter().any(|p| p.message.contains(message)), "no problem with '{message}' in {problems:?}");
}

#[test]
fn every_rule_is_refused_with_its_message() {
    let el = |e: Element| column([Node::new(e)]);
    let long = "x".repeat(limits::UI_LABEL_LEN + 1);
    refused(el(Element::Column { gap: Space(5), pad: Space(0) }), "the gap is step 5; use 0 to 4");
    refused(el(Element::Row { gap: Space(0), pad: Space(9) }), "the pad is step 9");
    refused(el(Element::Heading { text: "H".into(), level: 4 }), "the level is 4");
    refused(el(Element::Heading { text: " ".into(), level: 1 }), "the text is empty");
    refused(el(Element::Badge { text: long.clone(), tone: Tone::Muted }), "longer than 256 characters");
    refused(el(Element::Badge { text: "ok\u{202e}".into(), tone: Tone::Muted }), "invisible or control character U+202E");
    refused(el(Element::Badge { text: "a\u{200b}b".into(), tone: Tone::Muted }), "U+200B");
    refused(el(Element::Badge { text: "a\nb".into(), tone: Tone::Muted }), "U+000A");
    refused(el(Element::Text { text: "x".repeat(limits::UI_TEXT_LEN + 1), tone: Tone::Neutral }), "longer than 4096 characters");
    refused(el(Element::Text { text: "bell\u{7}".into(), tone: Tone::Neutral }), "control character U+0007");
    refused(el(Element::Markdown { source: "x".repeat(limits::UI_DOCUMENT_BYTES + 1) }), "the markdown is 65537 bytes");
    refused(el(Element::Code { source: "\u{0}".into(), language: None }), "the code contains the control character U+0000");
    refused(el(Element::Icon { icon: IconName("Bad Icon".into()), tone: Tone::Neutral, label: None }), "the icon name");
    refused(el(Element::Image { source: ImageSource::Bytes(Vec::new()), alt: String::new() }), "the image has no bytes");
    refused(el(Element::Image { source: ImageSource::Bytes(vec![0; limits::UI_IMAGE_BYTES + 1]), alt: String::new() }), "the limit is 1048576");
    refused(el(Element::Image { source: ImageSource::Url(HttpsUrl("http://a.com/x.png".into())), alt: String::new() }), "must start with https://");
    refused(el(Element::Progress { label: "P".into(), percent: Some(101) }), "the percent is 101");
    refused(el(Element::KeyValue { pairs: vec![Pair { name: "n".into(), value: "v".into() }; limits::UI_CHOICES + 1] }), "pairs; the limit is 256");
    refused(el(Element::Link { label: "L".into(), url: None }), "a link needs a key");

    let select =
        |options: Vec<Choice>, selected: Option<&str>| el(Element::Select { label: "S".into(), options, selected: selected.map(Key::from), disabled: false });
    let choice = |k: &str| Choice { key: k.into(), label: k.into() };
    let keyed = |mut v: Node| {
        v.children[0].key = Some("s".into());
        v
    };
    refused(keyed(select(vec![choice("a"), choice("a")], None)), "the option key 'a' is used twice");
    refused(keyed(select(vec![choice("a")], Some("b"))), "the selected option 'b' is not one of the options");
    refused(keyed(select(vec![choice(""); limits::UI_CHOICES + 1], None)), "has 257 options");
    refused(keyed(select(vec![choice("")], None)), "an option key is empty");

    refused(column([button(&"k".repeat(limits::UI_KEY_LEN + 1), "B")]), "the key is longer than 128 bytes");
    refused(column([button("a\tb", "B")]), "the key contains the control character U+0009");
    refused(column([button("same", "A"), button("same", "B")]), "the key 'same' is used twice");
    // An unkeyed row does not start a new scope, so its children and its siblings share one.
    refused(column([button("same", "A"), Node::new(Element::Row { gap: Space(0), pad: Space(0) }).child(button("same", "B"))]), "used twice");
    refused(column([text("t").child(text("u"))]), "a text has no children");
    refused(list("l", [text("row")]), "every row of a list needs a key");
    refused(list("l", (0..=limits::UI_ROWS_PER_PAGE).map(|i| text("r").key(i.to_string().as_str()))), "has 10001 rows; send at most 10000");

    let table =
        |columns: Vec<String>, rows: Vec<Node>| Node::new(Element::Table { label: "T".into(), columns, selection: Selection::None }).key("t").children(rows);
    let row = |cells: usize| Node::new(Element::Row { gap: Space(0), pad: Space(0) }).key("r").children((0..cells).map(|_| text("c")));
    refused(table(vec!["A".into()], vec![row(2)]), "row 0 has 2 cells for 1 columns");
    refused(table(vec!["A".into()], vec![text("x").key("r")]), "row 0 is a text");
    refused(table(Vec::new(), Vec::new()), "has 0 columns");

    refused(item("t", None).key("t").child(text("x").key("x")), "the items of a tree are trees");
    refused(item("t", None).child(item("a", None).child(item("b", None))).child(item("b", None)), "item keys must be unique in the whole tree");
    refused(item("t", Some(&"p".repeat(limits::UI_KEY_LEN + 1))), "the page token is longer");
    refused(
        Node::new(Element::EmptyState { title: "E".into(), text: String::new(), icon: None }).child(text("x")),
        "an empty-state takes only buttons and links",
    );

    let mut deep = text("leaf");
    for _ in 0..limits::UI_DEPTH {
        deep = column([deep]);
    }
    refused(deep, "nested deeper than 32 levels");
    let big = column((0..limits::UI_NODES).map(|_| Node::new(Element::Divider)));
    refused(big, "a view may have at most 50000");
}

#[test]
fn problems_name_the_element_and_stop_at_the_cap() {
    let view = column([list("files", [text("a").key("a").child(button("open", "\u{202e}"))])]);
    let problems = validate(&view).unwrap_err();
    assert!(problems.iter().any(|p| p.at == "column > list[files] > text[a]"), "{problems:?}");
    assert!(problems.iter().any(|p| p.to_string().starts_with("column > list[files] > text[a] > button[open]: the label")), "{problems:?}");

    let many = column((0..1000).map(|_| Node::new(Element::Heading { text: String::new(), level: 9 })));
    assert_eq!(validate(&many).unwrap_err().len(), MAX_PROBLEMS);
}

#[test]
fn decode_refuses_hostile_frames() {
    assert!(matches!(decode(&vec![0; limits::UI_VIEW_BYTES + 1]), Err(DecodeError::TooLarge(_))));
    assert!(matches!(decode(&[]), Err(DecodeError::Malformed(_))));
    // An empty list of nodes.
    assert_eq!(decode(&[0]), Err(DecodeError::Malformed("the view is empty".into())));
    let mut bytes = encode(&text("a"));
    bytes.push(0);
    assert_eq!(decode(&bytes), Err(DecodeError::Malformed("1 bytes after the view".into())));
    // A root that claims a child, followed by nothing.
    let mut claims = encode(&column([text("a")]));
    claims[0] = 1;
    claims.truncate(claims.len() - encode(&text("a")).len() + 1);
    assert!(matches!(decode(&claims), Err(DecodeError::Malformed(_))));
    // Two roots.
    let one = encode(&text("a"));
    let mut two = vec![2];
    two.extend_from_slice(&one[1..]);
    two.extend_from_slice(&one[1..]);
    assert_eq!(decode(&two), Err(DecodeError::Malformed("1 nodes after the root".into())));
    // Deep nesting is refused while the tree is rebuilt, before anything recurses over it.
    let mut deep = column([]);
    for _ in 0..200 {
        deep = column([deep]);
    }
    let e = decode(&encode(&deep)).unwrap_err();
    assert_eq!(e, DecodeError::Malformed("nested deeper than 32 levels".into()));
    // Valid wire form, broken rule.
    assert!(matches!(decode(&encode(&column([button("", "B")]))), Err(DecodeError::Invalid(_))));
}

#[test]
fn urls_are_https_with_a_host() {
    let u = HttpsUrl::parse("https://API.example.com:8443/x?y#z").unwrap();
    assert_eq!(u.host(), "api.example.com");
    for (bad, why) in [
        ("http://a.com", "https://"),
        ("https://", "no host"),
        ("https:///path", "no host"),
        ("https://user@a.com/", "user name"),
        ("https://a.com/a b", "U+0020"),
        ("https://a_b.com/", "no host"),
        ("https://[::1]/", "no host"),
    ] {
        let e = HttpsUrl::parse(bad).unwrap_err();
        assert!(e.contains(why), "{bad}: {e}");
    }
    assert!(HttpsUrl::parse(&format!("https://a.com/{}", "x".repeat(limits::UI_URL_LEN))).is_err());
}

fn rows(keys: &[&str]) -> Node {
    list("l", keys.iter().map(|k| text(k).key(*k)))
}

#[track_caller]
fn round(old: &Node, new: &Node) -> Vec<Op> {
    let ops = diff(old, new);
    let mut tree = old.clone();
    apply(&mut tree, ops.clone()).unwrap();
    assert_eq!(&tree, new);
    ops
}

#[test]
fn diff_is_keyed_and_small() {
    let view = every_element();
    assert!(round(&view, &view).is_empty());

    assert_eq!(round(&rows(&["a", "b", "c"]), &rows(&["a", "c"])), vec![Op::Remove { parent: vec![], index: 1 }]);
    assert_eq!(round(&rows(&["a", "c"]), &rows(&["a", "b", "c"])), vec![Op::Insert { parent: vec![], index: 1, node: text("b").key("b") }]);
    // The first row moved to the end: one move, not a shift of every row.
    assert_eq!(round(&rows(&["a", "b", "c", "d"]), &rows(&["b", "c", "d", "a"])), vec![Op::Move { parent: vec![], from: 0, to: 3 }]);
    assert_eq!(round(&rows(&["a", "b", "c", "d"]), &rows(&["d", "a", "b", "c"])), vec![Op::Move { parent: vec![], from: 3, to: 0 }]);

    // A changed property is one update at its path; children stay.
    let mut changed = rows(&["a", "b"]);
    changed.children[1].element = Element::Text { text: "B".into(), tone: Tone::Danger };
    assert_eq!(round(&rows(&["a", "b"]), &changed), vec![Op::Update { at: vec![1], element: changed.children[1].element.clone() }]);

    // A key or kind change replaces the element.
    let ops = round(&column([text("a")]), &column([Node::new(Element::Divider)]));
    assert!(matches!(ops.as_slice(), [Op::Replace { at, .. }] if at == &[0]));
    let ops = round(&column([text("a").key("x")]), &column([text("a").key("y")]));
    assert_eq!(ops.len(), 2);
    let ops = round(&text("a"), &column([]));
    assert!(matches!(ops.as_slice(), [Op::Replace { at, .. }] if at.is_empty()));

    // 10k rows, one moved and one changed: two operations.
    let keys: Vec<String> = (0..10_000).map(|i| i.to_string()).collect();
    let a = list("l", keys.iter().map(|k| text(k).key(k.as_str())));
    let mut b = a.clone();
    let row = b.children.remove(10);
    b.children.insert(9_000, row);
    b.children[5].element = Element::Text { text: "five".into(), tone: Tone::Neutral };
    assert_eq!(round(&a, &b).len(), 2);

    // Duplicate keys (an unvalidated tree) still diff correctly.
    round(&column([text("a").key("k"), text("b").key("k")]), &column([text("b").key("k")]));
}

#[test]
fn apply_refuses_operations_that_do_not_fit() {
    let mut tree = rows(&["a"]);
    assert_eq!(apply(&mut tree, [Op::Remove { parent: vec![], index: 1 }]), Err(PatchError { op: 0 }));
    assert_eq!(apply(&mut tree, [Op::Update { at: vec![0, 0], element: Element::Divider }]), Err(PatchError { op: 0 }));
    let insert = Op::Insert { parent: vec![], index: 2, node: text("b") };
    assert_eq!(apply(&mut tree, [Op::Move { parent: vec![], from: 0, to: 0 }, insert]), Err(PatchError { op: 1 }));
}

#[test]
fn input_state_survives_renders_and_goes_with_its_element() {
    let v1 = column([input("name", "web"), rows(&["a", "b"])]);
    let mut state = State::new();
    state.sync(None, &v1);
    assert_eq!(state.len(), 2);
    let path = |k: &str| vec![Key::from(k)];
    assert_eq!(state.get(&path("name")).unwrap().text, "web");

    // The user types and selects; a re-render with the same starting value keeps both.
    state.get_mut(&path("name")).unwrap().text = "website".into();
    state.get_mut(&path("l")).unwrap().selected.extend(["a".into(), "b".into()]);
    state.get_mut(&path("l")).unwrap().first_row = 1;
    let v2 = column([text("hint"), input("name", "web"), rows(&["b", "c"])]);
    state.sync(Some(&v1), &v2);
    assert_eq!(state.get(&path("name")).unwrap().text, "website");
    let l = state.get(&path("l")).unwrap();
    assert_eq!(l.selected.iter().map(Key::as_str).collect::<Vec<_>>(), ["b"], "the removed row is no longer selected");
    assert_eq!(l.first_row, 1);

    // The extension sets a new starting value: it replaces what was typed.
    let v3 = column([input("name", "api"), rows(&["b"])]);
    state.sync(Some(&v2), &v3);
    assert_eq!(state.get(&path("name")).unwrap().text, "api");

    // Gone, or a different kind under the same key: the entry is dropped.
    let v4 = column([Node::new(Element::Checkbox { label: "C".into(), checked: true, disabled: false }).key("name")]);
    state.sync(Some(&v3), &v4);
    assert_eq!(state.len(), 1);
    assert!(state.get(&path("l")).is_none());
    assert!(state.get(&path("name")).unwrap().checked);
}

#[test]
fn tree_state_follows_its_items() {
    let tree = |items: Vec<Node>| {
        Node::new(Element::Tree { label: "T".into(), description: None, icon: None, more: None, selection: Selection::One }).key("t").children(items)
    };
    let v1 = tree(vec![item("a", None).child(item("a1", None)), item("b", Some("p"))]);
    let mut state = State::new();
    state.sync(None, &v1);
    assert_eq!(state.len(), 1, "items keep their state in the tree's entry");
    let s = state.get_mut(&["t".into()]).unwrap();
    s.expanded.extend(["a".into(), "b".into()]);
    s.selected.extend(["a1".into(), "b".into()]);
    let v2 = tree(vec![item("b", None)]);
    state.sync(Some(&v1), &v2);
    let s = state.get(&["t".into()]).unwrap();
    assert_eq!(s.expanded.iter().map(Key::as_str).collect::<Vec<_>>(), ["b"]);
    assert_eq!(s.selected.len(), 1, "single selection keeps one row");

    // The select's choice resets when its option disappears.
    let select = |opts: &[&str]| {
        let options = opts.iter().map(|k| Choice { key: (*k).into(), label: (*k).into() }).collect();
        Node::new(Element::Select { label: "S".into(), options, selected: Some("x".into()), disabled: false }).key("s")
    };
    let mut state = State::new();
    state.sync(None, &select(&["x", "y"]));
    state.get_mut(&["s".into()]).unwrap().choice = Some("y".into());
    state.sync(Some(&select(&["x", "y"])), &select(&["x", "y"]));
    assert_eq!(state.get(&["s".into()]).unwrap().choice, Some("y".into()));
    state.sync(Some(&select(&["x", "y"])), &select(&["x"]));
    assert_eq!(state.get(&["s".into()]).unwrap().choice, Some("x".into()));
}

#[test]
fn every_element_has_a_role_and_label() {
    let view = every_element();
    let mut stack: Vec<(&Node, Vec<Kind>)> = vec![(&view, Vec::new())];
    while let Some((n, ancestors)) = stack.pop() {
        let a = n.access(&ancestors);
        let silent = matches!(n.kind(), Kind::Column | Kind::Row | Kind::Divider) || a.role == Role::Hidden;
        assert!(silent || !a.label.is_empty(), "{} has no label: {a:?}", n.kind());
        if n.kind().is_interactive() {
            assert_ne!(a.role, Role::Hidden, "{} is hidden", n.kind());
        }
        let mut next = ancestors.clone();
        next.push(n.kind());
        for c in &n.children {
            stack.push((c, next.clone()));
        }
    }
    let row = Node::new(Element::Row { gap: Space(0), pad: Space(0) }).key("r").children([text("Ada"), button("x", "Remove")]);
    assert_eq!(row.access(&[Kind::List]), Access { role: Role::ListItem, label: "Ada, Remove".into(), description: None });
    assert_eq!(text("c").access(&[Kind::Table, Kind::Row]).role, Role::Cell);
    assert_eq!(item("a", None).access(&[Kind::Tree]).role, Role::TreeItem);
    assert_eq!(item("a", None).access(&[Kind::Column]).role, Role::Tree);
    let danger = Node::new(Element::Banner { text: "Failed".into(), tone: Tone::Danger });
    assert_eq!(danger.access(&[]).role, Role::Alert);
    let long = text(&"é".repeat(400)).access(&[Kind::List]).label;
    assert_eq!(long.chars().count(), limits::UI_LABEL_LEN + 1);
}
