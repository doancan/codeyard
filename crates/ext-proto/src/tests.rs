use std::io::Cursor;

use ext_api::value::{Platform, ThemeKind};

use super::*;

fn env() -> EnvInfo {
    EnvInfo {
        codeyard_version: "0.5.0".into(),
        api_version: ext_api::VERSION.to_string(),
        language: "en".into(),
        theme: ThemeKind::Dark,
        platform: Platform::Macos,
        remote: false,
        workspace_trusted: true,
    }
}

fn hello() -> Hello {
    Hello {
        version: VERSION,
        build: "abc123".into(),
        extension: "dogan.deploy-status".into(),
        package_dir: "/data/extensions/store/x".into(),
        compiled: "/data/extensions/compiled/e/x.cwasm".into(),
        compiled_sha256: [7; 32],
        data_dir: "/data/extensions/data/dogan.deploy-status".into(),
        granted: vec!["net".into()],
        untrusted: false,
        env: env(),
        settings: vec![("interval".into(), Some("60".into()))],
        folders: vec!["file:///work".into()],
        reason: "command:refresh".into(),
        sandbox: true,
    }
}

fn round_trip<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(msg: T) {
    let mut buf = Vec::new();
    write(&mut buf, &msg).unwrap();
    let mut r = Reader::limited(Cursor::new(buf), Instant::now());
    assert_eq!(r.read::<T>(Instant::now()).unwrap(), msg);
    assert!(matches!(r.read::<T>(Instant::now()), Err(Error::Closed)));
}

#[test]
fn every_frame_kind_round_trips() {
    round_trip(ToHost::Hello(Box::new(hello())));
    round_trip(ToHost::Handle { id: 1, deadline_ms: 500, request: Request::Command { command: "refresh".into(), args_json: "[]".into() } });
    round_trip(ToHost::Cancel { id: 1 });
    round_trip(ToHost::Complete { ticket: 9, completion: Completion::Bytes(vec![1, 2, 3]) });
    round_trip(ToHost::Event { ticket: 9, event: StreamEvent::Stdout(b"hi\n".to_vec()) });
    round_trip(ToHost::Settings(vec![("interval".into(), None)]));
    round_trip(ToHost::Env(env()));
    round_trip(ToHost::Shutdown);
    round_trip(ToApp::Ready { version: VERSION, build: "abc123".into() });
    round_trip(ToApp::Mismatch { version: VERSION, build: "old".into() });
    round_trip(ToApp::Activated(Err("no config".into())));
    round_trip(ToApp::Start { ticket: 3, call: Start::ReadFile { uri: "file:///work/vercel.json".into() } });
    round_trip(ToApp::respond(1, Err(ApiError::Cancelled)));
    round_trip(ToApp::Trap(Trap { message: "unreachable".into(), frames: vec!["main (src/lib.rs:3)".into()] }));
}

#[test]
fn the_handshake_needs_the_same_version_and_build() {
    let h = hello();
    assert!(handshake_ok(&h, "abc123"));
    assert!(!handshake_ok(&h, "def456"), "an app updated in place restarts its exthosts");
    assert!(!handshake_ok(&Hello { version: VERSION + 1, ..h }, "abc123"));
}

#[test]
fn an_oversized_frame_is_refused_before_its_body_is_read() {
    let mut bytes = ((limits::FRAME_BYTES + 1) as u32).to_le_bytes().to_vec();
    bytes.extend([0; 16]);
    let mut r = Reader::limited(Cursor::new(bytes), Instant::now());
    assert!(matches!(r.read::<ToApp>(Instant::now()), Err(Error::TooLarge { len }) if len == limits::FRAME_BYTES + 1));
}

#[test]
fn a_call_too_large_to_send_fails_at_the_sender() {
    let big = ToApp::Start { ticket: 1, call: Start::WriteClipboard { text: "x".repeat(limits::FRAME_BYTES) } };
    let mut buf = Vec::new();
    assert!(matches!(write(&mut buf, &big), Err(Error::TooLarge { .. })));
    assert!(buf.is_empty(), "nothing is written");
}

#[test]
fn a_malformed_frame_is_an_error() {
    let mut bytes = 3u32.to_le_bytes().to_vec();
    bytes.extend([0xff, 0xff, 0xff]);
    let mut r = Reader::limited(Cursor::new(bytes), Instant::now());
    assert!(matches!(r.read::<ToApp>(Instant::now()), Err(Error::Decode(_))));
    // A pipe that closes inside a frame is an I/O error, not a clean close.
    let mut r = Reader::new(Cursor::new(10u32.to_le_bytes().to_vec()));
    assert!(matches!(r.read::<ToApp>(Instant::now()), Err(Error::Io(_))));
}

#[test]
fn the_rate_budget_allows_a_burst_then_the_sustained_rate() {
    let t0 = Instant::now();
    let mut b = RateBudget::new(t0);
    b.take(limits::IPC_BURST_BYTES, t0).unwrap();
    assert!(matches!(b.take(1, t0), Err(Error::RateExceeded)));
    let second = t0 + Duration::from_secs(1);
    b.take(limits::IPC_BYTES_PER_SECOND, second).unwrap();
    assert!(b.take(1024, second).is_err());
    assert!(b.wait_for(1024) > Duration::ZERO);
    // Idle time refills only up to the burst.
    let later = second + Duration::from_secs(3600);
    b.take(limits::IPC_BURST_BYTES, later).unwrap();
    assert!(b.take(1, later).is_err());
}

#[test]
fn an_exthost_flooding_the_app_is_cut_off() {
    // Frames of 1 MiB sent all at once: the burst passes, the next frame does not.
    let frame = ToApp::Notify(Notify::Log { level: ext_api::value::LogLevel::Info, message: "x".repeat(1 << 20) });
    let mut buf = Vec::new();
    let n = (limits::IPC_BURST_BYTES >> 20) + 1;
    for _ in 0..n {
        write(&mut buf, &frame).unwrap();
    }
    let t = Instant::now();
    let mut r = Reader::limited(Cursor::new(buf), t);
    let mut ok = 0;
    let err = loop {
        match r.read::<ToApp>(t) {
            Ok(_) => ok += 1,
            Err(e) => break e,
        }
    };
    assert!(matches!(err, Error::RateExceeded), "{err}");
    assert!(ok >= 60 && ok < n, "{ok}");
    // The exthost reading the app has no budget.
    let mut buf = Vec::new();
    for _ in 0..n {
        write(&mut buf, &ToHost::Complete { ticket: 1, completion: Completion::Bytes(vec![0; 1 << 20]) }).unwrap();
    }
    let mut r = Reader::new(Cursor::new(buf));
    for _ in 0..n {
        r.read::<ToHost>(t).unwrap();
    }
}

/// Random bytes never panic the reader: they decode, or fail with an error. `cargo test` runs a fixed number of
/// rounds; the fuzz job sets `CODEYARD_FUZZ_SECS` to keep going and `CODEYARD_FUZZ_SEED` to explore other inputs.
#[test]
fn fuzz_frames() {
    let extra: u64 = std::env::var("CODEYARD_FUZZ_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    let secs: Option<u64> = std::env::var("CODEYARD_FUZZ_SECS").ok().and_then(|s| s.parse().ok());
    let seed = (0x5eed_f4a3_u64 ^ extra.wrapping_mul(0x9e37_79b9_7f4a_7c15)).max(1);
    let deadline = secs.map(|s| Instant::now() + Duration::from_secs(s));
    let mut state = seed;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut seeds = Vec::new();
    for m in [
        ToApp::Start { ticket: 3, call: Start::ReadFile { uri: "file:///a".into() } },
        ToApp::respond(1, Ok(Response::Edits(vec![]))),
        ToApp::Notify(Notify::SetDiagnostics { uri: "file:///a".into(), diagnostics: vec![] }),
    ] {
        let mut b = Vec::new();
        write(&mut b, &m).unwrap();
        seeds.push(b);
    }
    let mut round = 0u64;
    while round < 20_000 || deadline.is_some_and(|d| Instant::now() < d) {
        round += 1;
        let mut bytes = seeds[(next() % seeds.len() as u64) as usize].clone();
        for _ in 0..=(next() % 8) {
            let i = (next() % bytes.len() as u64) as usize;
            bytes[i] = next() as u8;
        }
        let r = std::panic::catch_unwind(|| {
            let mut r = Reader::limited(Cursor::new(bytes), Instant::now());
            let _ = r.read::<ToApp>(Instant::now());
        });
        assert!(r.is_ok(), "fuzz: failed at round {round} of seed {seed:#x} (rerun with the same CODEYARD_FUZZ_SEED)");
    }
}
