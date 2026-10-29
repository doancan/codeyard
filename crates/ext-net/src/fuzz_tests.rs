//! Random (but repeatable) URLs, redirect locations and addresses: they come from extensions and from servers, so no
//! input may panic, nothing accepted may name an IP address, and no refused range may slip through the address check.
//!
//! `cargo test` runs a fixed number of rounds. The fuzz job (`.github/workflows/fuzz.yml`) sets `CODEYARD_FUZZ_SECS`
//! to keep going for that long and `CODEYARD_FUZZ_SEED` to explore other inputs; a failure prints the seed and round.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::{Duration, Instant};

use crate::addr;
use crate::target::{self, Target};

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
}

const PIECES: &[&str] = &[
    "https://",
    "http://",
    "HTTPS://",
    "//",
    "/",
    ":",
    "::",
    "@",
    "[",
    "]",
    "?",
    "#",
    ".",
    "..",
    "%",
    "%2e",
    "0x",
    "0",
    "1",
    "127",
    "255",
    "10",
    "169.254.169.254",
    "::ffff:",
    "64:ff9b::",
    "api",
    "example",
    "test",
    "com",
    "-",
    "_",
    " ",
    "\t",
    "\n",
    "\\",
    "ş",
    "İ",
    "🦀",
    "\u{202e}",
    "443",
    "65536",
    "user:pw@",
    "a.b.c",
    "xn--",
    "*",
    "ftp:",
    "javascript:",
];

fn random_text(rng: &mut Rng, max: usize) -> String {
    (0..rng.below(max)).map(|_| PIECES[rng.below(PIECES.len())]).collect()
}

/// What every accepted URL holds.
fn accepted(t: &Target) {
    assert!(!t.host.is_empty() && !t.host.ends_with('.'));
    assert!(!t.host.bytes().any(|b| b.is_ascii_uppercase() || b == b'[' || b == b'@' || b == b':'), "{t:?}");
    assert!(t.host.parse::<IpAddr>().is_err(), "{t:?}");
    assert!(t.port != 0 && t.path.starts_with('/'), "{t:?}");
    assert_eq!(target::parse(&t.url()).as_ref(), Ok(t), "{t:?} does not survive a round trip");
}

#[test]
fn random_urls_and_locations_never_panic() {
    let (mut budget, mut rng) = Budget::new(0x00e7_5eed, 6000);
    let base = Target { host: "api.example.test".into(), port: 443, path: "/a/b?c".into() };
    let net: Vec<String> = vec!["api.example.test".into(), "*.example.com".into()];
    while budget.next() {
        let text = random_text(&mut rng, 16);
        for url in [text.clone(), target::join(&base, &text)] {
            if let Ok(t) = target::parse(&url) {
                accepted(&t);
                if ext_manifest::net_allows(&net, &t.host) {
                    assert!(t.host == "api.example.test" || t.host.ends_with(".example.com"), "{t:?}");
                }
            }
        }
    }
}

fn v6_around(v4: Ipv4Addr, rng: &mut Rng) -> Ipv6Addr {
    let [a, b, c, d] = v4.octets();
    let (hi, lo) = (u16::from_be_bytes([a, b]), u16::from_be_bytes([c, d]));
    match rng.below(5) {
        0 => v4.to_ipv6_mapped(),
        1 => Ipv6Addr::new(0, 0, 0, 0, 0, 0, hi, lo),
        2 => Ipv6Addr::new(0x2002, hi, lo, 0, 0, 0, 0, 1),
        3 => Ipv6Addr::new(0x64, 0xff9b, 0, 0, 0, 0, hi, lo),
        _ => Ipv6Addr::new(0x64, 0xff9b, 1, 0, 0, 0, hi, lo),
    }
}

#[test]
fn no_special_address_passes_the_check() {
    let (mut budget, mut rng) = Budget::new(0x0add_7e55, 50_000);
    while budget.next() {
        // Bias toward the edges of the special ranges.
        let first = [0u8, 10, 100, 127, 169, 172, 192, 198, 203, 224, 240, 255][rng.below(12)];
        let v4 = if rng.below(2) == 0 { Ipv4Addr::from(rng.next() as u32) } else { Ipv4Addr::from(u32::from(first) << 24 | (rng.next() as u32 >> 8)) };
        let ok = addr::check(IpAddr::V4(v4)).is_ok();
        if ok {
            let special = v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.octets()[0] == 0;
            assert!(!special, "{v4} passed");
        }
        // The same address inside IPv6 never fares better.
        let v6 = v6_around(v4, &mut rng);
        if addr::check(IpAddr::V6(v6)).is_ok() {
            assert!(ok && v6.to_ipv4_mapped() == Some(v4), "{v6} passed");
        }
        assert_eq!(addr::check(IpAddr::V6(v4.to_ipv6_mapped())), addr::check(IpAddr::V4(v4)), "{v4}");
        let raw = Ipv6Addr::from(u128::from(rng.next()) << 64 | u128::from(rng.next()));
        if addr::check(IpAddr::V6(raw)).is_ok() {
            assert!((0x2000..0x4000).contains(&raw.segments()[0]), "{raw} passed");
            assert!(!raw.is_multicast() && !raw.is_loopback() && !raw.is_unspecified(), "{raw} passed");
        }
    }
}
