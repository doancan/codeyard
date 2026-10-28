//! Random (but repeatable) packages: a `.cyx` comes from strangers, so no input may panic or allocate past the
//! limits, and whatever `read` accepts must have a digest that matches its files.
//!
//! `cargo test` runs a fixed number of rounds. The fuzz job (`.github/workflows/fuzz.yml`) sets `CODEYARD_FUZZ_SECS`
//! to keep going for that long and `CODEYARD_FUZZ_SEED` to explore other inputs; a failure prints the seed and round.

use std::io::Write as _;
use std::time::{Duration, Instant};

use crate::tar::{self, Entry};
use crate::{CONTENTS, read, write};

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

const MANIFEST: &str =
    "[extension]\nid = \"f.z\"\nversion = \"1.0.0\"\nname = \"F\"\nlicense = \"MIT\"\n[[snippets]]\nlanguage = \"rust\"\npath = \"s/rust.json\"\n";

fn seed_package() -> Vec<u8> {
    let files = vec![
        Entry { path: ext_manifest::FILE_NAME.into(), data: MANIFEST.as_bytes().to_vec(), executable: false },
        Entry { path: "s/rust.json".into(), data: b"{}".to_vec(), executable: false },
        Entry { path: "bin/tool".into(), data: vec![7; 700], executable: true },
    ];
    write(files).unwrap().0
}

fn untar(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(bytes), &mut out).unwrap();
    out
}

fn gzip(tar: &[u8]) -> Vec<u8> {
    let mut gz = flate2::GzBuilder::new().write(Vec::new(), flate2::Compression::fast());
    gz.write_all(tar).unwrap();
    gz.finish().unwrap()
}

fn check(bytes: &[u8]) {
    if let Ok(p) = read(bytes) {
        // Whatever is accepted is self-consistent: re-packaging gives the same digest.
        let files: Vec<Entry> = p.files.values().filter(|e| e.path != CONTENTS).cloned().collect();
        assert_eq!(write(files).expect("an accepted package re-packages").1, p.digest);
    }
}

#[test]
fn mutated_tars_never_panic() {
    let (mut budget, mut rng) = Budget::new(0x7a7a_0001, 3000);
    let tar = untar(&seed_package());
    while budget.next() {
        let mut t = tar.clone();
        for _ in 0..1 + rng.below(4) {
            if t.is_empty() {
                break;
            }
            let at = rng.below(t.len());
            match rng.below(4) {
                0 => t[at] = rng.next() as u8,
                1 => t[at] ^= 1 << rng.below(8),
                2 => t.truncate(at),
                _ => {
                    // Rewrite a header field with digits, often keeping the checksum right so the deeper checks run.
                    let h = (at / tar::BLOCK) * tar::BLOCK;
                    let field = [100, 124, 136, 156, 257, 345][rng.below(6)];
                    if h + field + 8 <= t.len() {
                        for b in &mut t[h + field..h + field + 8] {
                            *b = b"01234567 \0x/."[rng.below(13)];
                        }
                        if h + tar::BLOCK <= t.len() && rng.below(2) == 0 {
                            t[h + 148..h + 156].copy_from_slice(b"        ");
                            let sum: u32 = t[h..h + tar::BLOCK].iter().map(|&b| u32::from(b)).sum();
                            t[h + 148..h + 156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
                        }
                    }
                }
            }
        }
        check(&gzip(&t));
    }
}

#[test]
fn mutated_gzip_never_panics() {
    let (mut budget, mut rng) = Budget::new(0x7a7a_0002, 3000);
    let pkg = seed_package();
    while budget.next() {
        let mut p = pkg.clone();
        for _ in 0..1 + rng.below(3) {
            if p.is_empty() {
                break;
            }
            let at = rng.below(p.len());
            p[at] = rng.next() as u8;
        }
        if rng.below(4) == 0 {
            p.truncate(rng.below(p.len()));
        }
        check(&p);
    }
}

#[test]
fn random_bytes_never_panic() {
    let (mut budget, mut rng) = Budget::new(0x7a7a_0003, 2000);
    while budget.next() {
        let mut b = vec![0x1f, 0x8b];
        b.extend((0..rng.below(600)).map(|_| rng.next() as u8));
        check(&b);
        check(&gzip(&b[2..]));
    }
}
