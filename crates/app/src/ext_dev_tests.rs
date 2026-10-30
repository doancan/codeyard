//! Hot reload of the extension in development (`cyx dev`, workbench_ext_dev.rs): a new `extension.wasm` in the dev
//! folder replaces the running exthost, which starts again with the activation reason `reload`. Skipped when
//! codeyard-exthost is not built.

use std::path::Path;
use std::time::{Duration, Instant};

use gpui::TestAppContext;

use crate::ext_code_tests::{MANIFEST, PROBE, config_dir, notice_has, setup, wait_for};

/// Put `wasm` in place the way `cyx dev` does: written beside, then renamed over the old build.
fn swap_build(dir: &Path, wasm: &[u8]) {
    let tmp = dir.join(".extension.wasm.tmp");
    std::fs::write(&tmp, wasm).unwrap();
    std::fs::rename(&tmp, dir.join(ext_pkg::COMPONENT)).unwrap();
    // The same bytes again must still count as a new build.
    let f = std::fs::File::options().write(true).open(dir.join(ext_pkg::COMPONENT)).unwrap();
    f.set_modified(std::time::SystemTime::now() + Duration::from_secs(1)).unwrap();
}

#[gpui::test]
fn a_new_build_replaces_the_running_exthost(cx: &mut TestAppContext) {
    if !crate::ext_runtime::code_runs_for_test() {
        return;
    }
    let cfg = config_dir("dev-reload");
    let dev = cfg.join("probe");
    std::fs::create_dir_all(&dev).unwrap();
    std::fs::write(dev.join(ext_manifest::FILE_NAME), MANIFEST).unwrap();
    std::fs::write(dev.join("Cargo.toml"), "").unwrap();
    std::fs::write(dev.join(ext_pkg::COMPONENT), PROBE).unwrap();
    crate::ext::set_dev(Some(dev.clone()));
    let (wb, cx) = setup(cx, &cfg);
    // Not running: a new build changes nothing until the extension starts.
    swap_build(&dev, PROBE);
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    cx.run_until_parked();
    assert!(!wb.read_with(cx, |wb, _| wb.extension_running_for_test("you.probe")));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Probe: Notice", window, cx));
    wait_for(cx, &wb, |wb| notice_has(wb, "you.probe: Pick"));
    assert_eq!(wb.read_with(cx, |wb, _| wb.extension_activation_for_test("you.probe")), Some(("command:notice".into(), 1)));
    // Running: the new build is swapped in and activated with the reason `reload`.
    swap_build(&dev, PROBE);
    wb.update_in(cx, |wb, window, cx| wb.reload_config_for_test(window, cx));
    wait_for(cx, &wb, |wb| wb.extension_activation_for_test("you.probe") == Some(("reload".into(), 2)));
    assert!(wb.read_with(cx, |wb, _| wb.extension_running_for_test("you.probe")));
    // The old exthost's end is not reported as the extension stopping, and the new one answers.
    wb.update_in(cx, |wb, _, cx| wb.set_notice_for_test("", false, cx));
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test("Probe: Notice", window, cx));
    wait_for(cx, &wb, |wb| notice_has(wb, "you.probe: Pick"));
    let log = wb.read_with(cx, |wb, _| wb.extension_log_for_test("you.probe")).join("\n");
    assert!(log.contains("new build: reloading") && !log.contains("stopped:"), "{log}");
    crate::ext::set_dev(None);
}

/// The hot-reload scenario of scripts/perf/run.sh (`CODEYARD_PERF_RELOAD`): the first command starts the extension, a
/// build put in place is swapped in and asked the command, and the window settles once the new build has answered.
#[gpui::test]
fn the_perf_reload_swaps_in_a_build_and_asks_it(cx: &mut TestAppContext) {
    if let Err(why) = crate::ext_runtime::code_runs_here(&"tests") {
        eprintln!("skipped: {why}");
        return;
    }
    let cfg = config_dir("dev-perf");
    let dev = cfg.join("probe");
    std::fs::create_dir_all(&dev).unwrap();
    std::fs::write(dev.join(ext_manifest::FILE_NAME), MANIFEST).unwrap();
    std::fs::write(dev.join("Cargo.toml"), "").unwrap();
    std::fs::write(dev.join(ext_pkg::COMPONENT), PROBE).unwrap();
    crate::ext::set_dev(Some(dev.clone()));
    let (wb, cx) = setup(cx, &cfg);
    assert!(!wb.read_with(cx, |wb, _| wb.dev_settled_for_perf()), "not running yet");
    assert!(wb.update_in(cx, |wb, window, cx| wb.run_dev_command_for_perf(window, cx)));
    wait_for(cx, &wb, |wb| wb.dev_settled_for_perf());
    // The same bytes count as a new build: the modification time moved.
    std::thread::sleep(Duration::from_millis(20));
    assert!(wb.update_in(cx, |wb, window, cx| wb.hot_reload_for_perf(PROBE, window, cx)));
    assert!(!wb.read_with(cx, |wb, _| wb.dev_settled_for_perf()), "waits for the new build");
    wait_for(cx, &wb, |wb| wb.dev_settled_for_perf());
    assert_eq!(wb.read_with(cx, |wb, _| wb.extension_activation_for_test("you.probe")), Some(("reload".into(), 2)));
    crate::ext::set_dev(None);
}

/// The hot-reload row of docs/plugin-architecture.md §10: new build in place → the first command answered (cyx's
/// compiler excluded; the exthost's compile, the old exthost's stop and the new one's start included; the watch's
/// 100 ms poll not). `CODEYARD_RELOAD_BENCH` names a folder with `ext/` (a dev
/// extension whose first command answers `ctx.state` "tally" + 1) and two builds of it, `a.wasm` and `b.wasm`:
///
///   CODEYARD_RELOAD_BENCH=/path cargo test -p app --release ext_dev_tests::reload_time -- --ignored --nocapture
#[gpui::test]
#[ignore = "a measurement; needs CODEYARD_RELOAD_BENCH"]
fn reload_time(cx: &mut TestAppContext) {
    let Some(bench) = std::env::var_os("CODEYARD_RELOAD_BENCH").map(std::path::PathBuf::from) else { return };
    let dev = bench.join("ext");
    let builds = [std::fs::read(bench.join("a.wasm")).unwrap(), std::fs::read(bench.join("b.wasm")).unwrap()];
    let text = std::fs::read_to_string(dev.join(ext_manifest::FILE_NAME)).unwrap();
    let m = ext_manifest::parse_with(&text, ext_manifest::Options { require_why: false }).unwrap();
    let id = m.id.to_string();
    let title = m.contributions.commands[0].title.clone();
    let cfg = config_dir("dev-bench");
    swap_build(&dev, &builds[1]);
    crate::ext::set_dev(Some(dev.clone()));
    let (wb, cx) = setup(cx, &cfg);
    let answered = |cx: &mut gpui::VisualTestContext, want: u64| {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            cx.run_until_parked();
            if wb.read_with(cx, |wb, _| wb.extension_answer_for_test(&id)).and_then(|a| a.parse::<u64>().ok()) == Some(want) {
                return;
            }
            assert!(Instant::now() < deadline, "no answer {want}: {:?}", wb.read_with(cx, |wb, _| wb.extension_log_for_test(&id)));
            std::thread::sleep(Duration::from_micros(200));
        }
    };
    wb.update_in(cx, |wb, window, cx| wb.run_command_for_test(&title, window, cx));
    answered(cx, 1);
    let mut times = Vec::new();
    for run in 0..21u64 {
        swap_build(&dev, &builds[(run % 2) as usize]);
        let t0 = Instant::now();
        wb.update_in(cx, |wb, window, cx| {
            wb.reload_dev_code_for_test(window, cx);
            wb.run_command_for_test(&title, window, cx);
        });
        // `ctx.state` came along: the tally goes on.
        answered(cx, run + 2);
        // The first run fills the incremental cache.
        if run > 0 {
            times.push(t0.elapsed());
        }
    }
    if std::env::var_os("CODEYARD_RELOAD_BENCH_LOG").is_some() {
        println!("{}", wb.read_with(cx, |wb, _| wb.extension_log_for_test(&id)).join("\n"));
    }
    times.sort();
    let at = |q: f64| times[((times.len() as f64 - 1.0) * q).round() as usize].as_secs_f64() * 1000.0;
    let size = builds[0].len() as f64 / (1 << 20) as f64;
    println!("hot reload of {id} ({size:.2} MB): p50 {:.0} ms, p95 {:.0} ms, max {:.0} ms ({} runs)", at(0.5), at(0.95), at(1.0), times.len());
    crate::ext::set_dev(None);
}
