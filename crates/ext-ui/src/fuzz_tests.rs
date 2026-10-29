//! Random (but repeatable) inputs: a view comes from extension code, so no frame may panic the decoder, and the diff
//! must reproduce any new tree from any old one.
//!
//! `cargo test` runs a fixed number of rounds. The fuzz job (`.github/workflows/fuzz.yml`) sets `CODEYARD_FUZZ_SECS`
//! to keep going for that long and `CODEYARD_FUZZ_SEED` to explore other inputs; a failure prints the seed and round.

use std::time::{Duration, Instant};

use crate::*;

/// Rounds of one fuzz test: at least `min`, and with `CODEYARD_FUZZ_SECS` until that many seconds have passed.
struct Budget {
    seed: u64,
    round: usize,
    min: usize,
    deadline: Option<Instant>,
}

impl Budget {
    fn new(base_seed: u64, min: usize) -> (Budget, Rng) {
        let extra: u64 = std::env::var("CODEYARD_FUZZ_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
        let secs: Option<u64> = std::env::var("CODEYARD_FUZZ_SECS").ok().and_then(|s| s.parse().ok());
        let seed = (base_seed ^ extra.wrapping_mul(0x9e37_79b9_7f4a_7c15)).max(1);
        (Budget { seed, round: 0, min, deadline: secs.map(|s| Instant::now() + Duration::from_secs(s)) }, Rng(seed))
    }

    fn next(&mut self) -> bool {
        self.round += 1;
        self.round <= self.min || self.deadline.is_some_and(|d| Instant::now() < d)
    }
}

impl Drop for Budget {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("fuzz: failed at round {} of seed {:#x} (rerun with the same CODEYARD_FUZZ_SEED)", self.round, self.seed);
        }
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

const WORDS: &[&str] = &["a", "b", "c", "d", "e", "f", "g", "h", "", "ş", "🦀", "\u{202e}", "\n", "x y"];

fn element(rng: &mut Rng) -> Element {
    let word = |rng: &mut Rng| rng.pick(WORDS).to_string();
    let tone = |rng: &mut Rng| [Tone::Neutral, Tone::Accent, Tone::Success, Tone::Warning, Tone::Danger, Tone::Muted][rng.below(6)];
    match rng.below(12) {
        0 => Element::Column { gap: Space(rng.below(6) as u8), pad: Space(0) },
        1 => Element::Row { gap: Space(0), pad: Space(rng.below(6) as u8) },
        2 => Element::List { label: word(rng), selection: Selection::Many },
        3 => Element::Tree {
            label: word(rng),
            description: None,
            icon: None,
            more: (rng.below(2) == 0).then(|| PageToken(word(rng))),
            selection: Selection::One,
        },
        4 => Element::Table { label: word(rng), columns: vec![word(rng); rng.below(3)], selection: Selection::None },
        5 => Element::Button { label: word(rng), icon: None, tone: tone(rng), disabled: rng.below(2) == 0 },
        6 => Element::TextInput { label: word(rng), placeholder: String::new(), value: word(rng), multiline: false, disabled: false },
        7 => Element::Checkbox { label: word(rng), checked: rng.below(2) == 0, disabled: false },
        8 => Element::Badge { text: word(rng), tone: tone(rng) },
        9 => Element::Divider,
        10 => Element::EmptyState { title: word(rng), text: word(rng), icon: None },
        _ => Element::Text { text: word(rng), tone: tone(rng) },
    }
}

/// A random tree with sibling keys drawn from a small pool, so two trees share keys and the diff has moves to find.
fn tree(rng: &mut Rng, depth: usize) -> Node {
    let mut node = Node::new(element(rng));
    if rng.below(3) > 0 {
        node.key = Some(Key::new(rng.pick(&WORDS[..8])));
    }
    if depth > 0 && node.kind().takes_children() {
        for _ in 0..rng.below(7) {
            node.children.push(tree(rng, depth - 1));
        }
        dedup(&mut node.children);
    }
    node
}

/// Mostly unique sibling keys, like a valid view; a duplicate is left in now and then to test the fallback.
fn dedup(children: &mut [Node]) {
    let mut seen = std::collections::HashSet::new();
    for c in children {
        if let Some(k) = &c.key
            && !seen.insert(k.clone())
        {
            c.key = None;
        }
    }
}

/// `a` changed in a few random places: rows removed, added, moved, re-keyed, changed in kind or properties.
fn mutate(rng: &mut Rng, a: &Node) -> Node {
    let mut b = a.clone();
    for _ in 0..1 + rng.below(5) {
        let mut at = &mut b;
        while !at.children.is_empty() && rng.below(3) > 0 {
            let i = rng.below(at.children.len());
            at = &mut at.children[i];
        }
        let n = at.children.len();
        match rng.below(6) {
            0 if n > 0 => {
                at.children.remove(rng.below(n));
            }
            1 if n > 1 => {
                let c = at.children.remove(rng.below(n));
                at.children.insert(rng.below(n), c);
            }
            2 => at.children.insert(rng.below(n + 1), tree(rng, 1)),
            3 => at.key = (rng.below(2) == 0).then(|| Key::new(rng.pick(&WORDS[..8]))),
            4 => at.element = element(rng),
            _ => at.children.reverse(),
        }
        dedup(&mut at.children);
    }
    if rng.below(20) == 0 {
        // A duplicate key among siblings, as an unvalidated tree could have.
        if let [x, y, ..] = b.children.as_mut_slice() {
            y.key = x.key.clone();
        }
    }
    b
}

#[test]
fn fuzz_diff_then_apply_gives_the_new_tree() {
    let (mut budget, mut rng) = Budget::new(0xd1ff_0001, 3000);
    while budget.next() {
        let a = tree(&mut rng, 4);
        let b = if rng.below(4) == 0 { tree(&mut rng, 4) } else { mutate(&mut rng, &a) };
        let ops = diff(&a, &b);
        let mut patched = a.clone();
        apply(&mut patched, ops.clone()).expect("a diff always fits its old tree");
        assert_eq!(patched, b, "ops: {ops:?}");
        assert!(diff(&b, &b).is_empty());
        let bytes = postcard::to_stdvec(&ops).expect("ops serialize");
        assert_eq!(postcard::from_bytes::<Vec<Op>>(&bytes).expect("ops decode"), ops);

        // Input state never panics and holds only elements of the new tree.
        let mut state = State::new();
        state.sync(None, &a);
        state.sync(Some(&a), &b);
        let mut fresh = State::new();
        fresh.sync(None, &b);
        assert_eq!(state.len(), fresh.len());
    }
}

#[test]
fn fuzz_decoder_never_panics() {
    let (mut budget, mut rng) = Budget::new(0xdec0_de01, 3000);
    while budget.next() {
        let mut bytes = encode(&if rng.below(2) == 0 { crate::tests::every_element() } else { tree(&mut rng, 4) });
        match rng.below(5) {
            0 => {}
            1 => bytes = (0..rng.below(64)).map(|_| rng.next() as u8).collect(),
            _ => {
                for _ in 0..1 + rng.below(4) {
                    let at = rng.below(bytes.len() + 1);
                    match rng.below(4) {
                        0 if at < bytes.len() => bytes[at] = rng.next() as u8,
                        1 if at < bytes.len() => {
                            bytes.remove(at);
                        }
                        2 => bytes.insert(at, [0, 1, 0x7f, 0x80, 0xff][rng.below(5)]),
                        _ => bytes.truncate(at),
                    }
                }
            }
        }
        if let Ok(view) = decode(&bytes) {
            // Whatever is accepted keeps the rules and survives another round trip.
            assert!(validate(&view).is_ok());
            assert_eq!(decode(&encode(&view)).as_ref(), Ok(&view));
        }
    }
}
