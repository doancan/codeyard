//! Performance samples for `scripts/perf` (see `scripts/perf/README.md`). Off unless `CODEYARD_PERF=<file>` is set;
//! when off, every call is one relaxed atomic load.
//!
//! - `CODEYARD_PERF=<file>`: append one JSON line per sample, `{"metric":"…","value":…}`.
//! - `CODEYARD_PERF_TYPING=<n>`: after the first frame settles, type `n` characters into the focused editor and record
//!   keystroke → frame for each.
//! - `CODEYARD_PERF_MARKET=<n>`: after the first frame settles, open the Market, wait for its catalogue, then type `n`
//!   keys into its search (a word, then erasing it, over and over) and record keystroke → results in the view, and
//!   keystroke → the frame that shows them.
//! - `CODEYARD_PERF_DECORATIONS=<n>`: after the first frame settles, run `decorate` in every extension that decorates
//!   the open file's language, wait for their decorations, then type `n` characters and record keystroke → frame under
//!   `typing.keystroke_frame_decorations_ms`.
//! - `CODEYARD_PERF_LENSES=<n>`: after the first frame settles, wait for the code lenses and inlay hints of the
//!   extension that provides them for the open file's language, then type `n` characters and record keystroke → frame
//!   under `typing.keystroke_frame_lenses_ms` (each edit asks the provider again once typing pauses).
//! - `CODEYARD_PERF_PROVIDERS=<n>`: ask the task providers for their tasks `n` times, one round after the other;
//!   each answer records `extensions.provider_overhead_ms` (requests to a running extension only).
//! - `CODEYARD_PERF_COMPLETION=<n>`: after the first frame settles, type `n` keys into the focused editor, a new word
//!   every third key, so the extensions that complete its language are asked again and again, and record keystroke
//!   → frame under `typing.keystroke_frame_completion_ms`.
//! - `CODEYARD_PERF_WORDS=<n>`: after the first frame settles, type the completion scenario's `n` keys into the focused
//!   editor, with no extension asked, and record keystroke → frame under `typing.keystroke_frame_words_ms`: the
//!   completion row's reference (words typed at the top of the file cost the editor more than one long word).
//! - `CODEYARD_PERF_VIEWS=<n>`: after the first frame settles, open the first declared extension view, wait until it is
//!   drawn and holds the keyboard, then press PageDown `n` times (`views.keystroke_frame_ms`, below).
//! - `CODEYARD_PERF_RELOAD=<n>`: with `--extension-dev <folder>`, run the extension's first command, then `n + 1` times
//!   put the other of two builds in place (`a.wasm` and `b.wasm` beside the folder) and run the command again. The first
//!   swap fills the incremental compile cache and is not recorded; after a pause, each of the `n` others records its
//!   new build's first answer under `extensions.hot_reload_ms` (workbench_ext_dev.rs).
//! - `CODEYARD_PERF_EXIT=1`: quit when the scenario is done.
//!
//! Whenever `CODEYARD_PERF` is set, a key that reaches an extension view is recorded as `views.keystroke_frame_ms`.

use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, Keystroke, Window, WindowHandle};

use crate::workbench::Workbench;

static ON: AtomicBool = AtomicBool::new(false);
static START: OnceLock<Instant> = OnceLock::new();
static OUT: Mutex<Option<File>> = Mutex::new(None);
/// When the Market key being measured was pressed; taken when its results are on screen.
static MARKET_KEY: Mutex<Option<Instant>> = Mutex::new(None);
/// The hot reload scenario's warm-up swap is under way: its time is not recorded.
static RELOAD_WARMING: AtomicBool = AtomicBool::new(false);

/// Time between typed characters: long enough for the previous frame to finish.
const TYPING_GAP: Duration = Duration::from_millis(60);
/// Wait after the first frame before typing: restored state, highlighting and the minimap settle.
const SETTLE: Duration = Duration::from_millis(1500);
/// Wait after the hot reload scenario's warm-up swap: longer than the once-a-second settings watcher of the dev
/// folder, which notices the new build too and would restart a measured swap's build.
const RELOAD_SETTLE: Duration = Duration::from_millis(2500);

/// First thing in `main`: start the clock and open the output file.
pub fn init() {
    START.get_or_init(Instant::now);
    let Some(path) = std::env::var_os("CODEYARD_PERF") else { return };
    match File::options().create(true).append(true).open(&path) {
        Ok(f) => {
            *OUT.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
            ON.store(true, Ordering::Relaxed);
        }
        Err(e) => eprintln!("CODEYARD_PERF: cannot open {}: {e}", path.to_string_lossy()),
    }
}

pub fn enabled() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Record a value under `metric`.
pub fn sample(metric: &str, value: f64) {
    if !enabled() {
        return;
    }
    if let Some(f) = OUT.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        let _ = writeln!(f, "{{\"metric\":\"{metric}\",\"value\":{value:.3}}}");
    }
}

/// A new build of the extension in development answered its first request `ms` after it was noticed:
/// `extensions.hot_reload_ms`, unless it is the hot reload scenario's warm-up swap.
pub fn hot_reload(ms: f64) {
    if !RELOAD_WARMING.load(Ordering::Relaxed) {
        sample("extensions.hot_reload_ms", ms);
    }
}

/// Record milliseconds since `init` under `metric`.
pub fn mark(metric: &str) {
    if enabled() {
        sample(metric, ms(START.get_or_init(Instant::now).elapsed()));
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Run `f` once the frame after the next one starts. GPUI runs frame callbacks before it draws, so this is the first
/// moment the next frame is known to be drawn and presented. It overstates by at most one display refresh.
fn after_next_frame(window: &Window, f: impl FnOnce(&mut Window, &mut App) + 'static) {
    window.on_next_frame(move |window, _| window.on_next_frame(f));
}

/// The Market applied search results for the measured keystroke, if one is pending: `market.search_ms` is the time
/// to that point (the search, off the UI thread, and its hand-over), `market.search_frame_ms` to the frame after the
/// one that draws them (it includes up to two display refreshes, like `typing.keystroke_frame_ms`).
pub fn market_results_shown(window: &Window) {
    if !enabled() {
        return;
    }
    if let Some(t0) = MARKET_KEY.lock().unwrap_or_else(|e| e.into_inner()).take() {
        sample("market.search_ms", ms(t0.elapsed()));
        after_next_frame(window, move |_, _| sample("market.search_frame_ms", ms(t0.elapsed())));
    }
}

/// A key reached an extension view: `views.keystroke_frame_ms` is the time to the frame after the one that shows its
/// effect, like `typing.keystroke_frame_ms`.
pub fn view_key(window: &Window) {
    if !enabled() {
        return;
    }
    let t0 = Instant::now();
    after_next_frame(window, move |_, _| sample("views.keystroke_frame_ms", ms(t0.elapsed())));
}

/// Keys the completion scenario types, and its reference without extensions: a new word every third key.
const WORD_KEYS: [&str; 3] = ["a", "b", "space"];

/// Keys the Market scenario types: a word letter by letter, then erased, so every key changes the query.
fn market_key(i: usize) -> &'static str {
    const KEYS: [&str; 10] = ["n", "i", "g", "h", "t", "backspace", "backspace", "backspace", "backspace", "backspace"];
    KEYS[i % KEYS.len()]
}

/// Type `n` characters into the focused editor, recording keystroke → frame under `metric`, then save so the harness
/// can check that the keys reached the editor (scripts/perf/run.sh).
async fn type_keys(handle: WindowHandle<Workbench>, n: usize, metric: &'static str, cx: &mut gpui::AsyncApp) {
    type_these(handle, n, &["a"], metric, cx).await
}

/// `type_keys` with `keys` typed in turn.
async fn type_these(handle: WindowHandle<Workbench>, n: usize, keys: &'static [&'static str], metric: &'static str, cx: &mut gpui::AsyncApp) {
    for i in 0..n {
        // Through the window, not the workbench entity: the key handler updates the workbench itself.
        let typed = cx.update_window(handle.into(), |_, window, cx| {
            let t0 = Instant::now();
            window.dispatch_keystroke(Keystroke::parse(keys[i % keys.len()]).expect("valid keystroke"), cx);
            after_next_frame(window, move |_, _| sample(metric, ms(t0.elapsed())));
        });
        if typed.is_err() {
            break;
        }
        cx.background_executor().timer(TYPING_GAP).await;
    }
    cx.update_window(handle.into(), |_, window, cx| {
        window.dispatch_keystroke(Keystroke::parse("cmd-s").expect("valid keystroke"), cx);
    })
    .ok();
    cx.background_executor().timer(SETTLE).await;
}

/// `CODEYARD_PERF_DECORATIONS` and `CODEYARD_PERF_PROVIDERS`: the decoration providers mark the open file (up to 30 s
/// for them to start), then the keys are typed and the task providers asked.
async fn extension_scenario(handle: WindowHandle<Workbench>, decorations: usize, providers: usize, cx: &mut gpui::AsyncApp) {
    cx.background_executor().timer(SETTLE).await;
    let asked = handle.update(cx, |wb, window, cx| wb.start_decorations_for_perf(window, cx)).unwrap_or(0);
    for _ in 0..300 {
        if handle.read_with(cx, |wb, cx| wb.decoration_sets_for_perf(cx)).unwrap_or(asked) >= asked {
            break;
        }
        cx.background_executor().timer(Duration::from_millis(100)).await;
    }
    if decorations > 0 {
        type_keys(handle, decorations, "typing.keystroke_frame_decorations_ms", cx).await;
    }
    for _ in 0..providers {
        let Ok(done) = handle.update(cx, |wb, window, cx| wb.ask_tasks_for_perf(window, cx)) else { break };
        while !done.get() {
            cx.background_executor().timer(Duration::from_millis(2)).await;
        }
        cx.background_executor().timer(TYPING_GAP).await;
    }
}

/// `CODEYARD_PERF_COMPLETION`: wait (up to 30 s) until an extension completes the open file's language, then type.
async fn completion_scenario(handle: WindowHandle<Workbench>, n: usize, cx: &mut gpui::AsyncApp) {
    cx.background_executor().timer(SETTLE).await;
    for _ in 0..300 {
        if handle.read_with(cx, |wb, cx| wb.completion_providers_for_perf(cx)).unwrap_or(1) > 0 {
            break;
        }
        cx.background_executor().timer(Duration::from_millis(100)).await;
    }
    type_these(handle, n, &WORD_KEYS, "typing.keystroke_frame_completion_ms", cx).await;
}

/// `CODEYARD_PERF_LENSES`: the lens and hint provider answers for the lines in view (up to 30 s for it to start), then
/// the keys are typed.
async fn lens_scenario(handle: WindowHandle<Workbench>, keys: usize, cx: &mut gpui::AsyncApp) {
    cx.background_executor().timer(SETTLE).await;
    for _ in 0..300 {
        if handle.read_with(cx, |wb, cx| wb.lens_marks_for_perf(cx)).unwrap_or(1) > 0 {
            break;
        }
        cx.background_executor().timer(Duration::from_millis(100)).await;
    }
    type_keys(handle, keys, "typing.keystroke_frame_lenses_ms", cx).await;
}

/// `CODEYARD_PERF_VIEWS`: open the view, wait for its tree and focus (up to 30 s for its extension to start and draw
/// 10 000 rows), then press PageDown `n` times; the view records each key.
async fn views_scenario(handle: WindowHandle<Workbench>, n: usize, cx: &mut gpui::AsyncApp) {
    cx.background_executor().timer(SETTLE).await;
    let Some(id) = handle.update(cx, |wb, window, cx| wb.open_ext_view_for_perf(window, cx)).ok().flatten() else { return };
    let focus = |cx: &mut gpui::AsyncApp| handle.update(cx, |wb, window, cx| wb.focus_ext_view_for_perf(&id, window, cx)).unwrap_or(true);
    for _ in 0..300 {
        if focus(cx) {
            break;
        }
        cx.background_executor().timer(Duration::from_millis(100)).await;
    }
    // The first frame of 10 000 rows, and the layout of the list, settle before the keys; on a fresh profile the
    // welcome screen may have taken the keyboard back meanwhile.
    cx.background_executor().timer(SETTLE).await;
    focus(cx);
    for _ in 0..n {
        let pressed = cx.update_window(handle.into(), |_, window, cx| {
            window.dispatch_keystroke(Keystroke::parse("pagedown").expect("valid keystroke"), cx);
        });
        if pressed.is_err() {
            break;
        }
        cx.background_executor().timer(TYPING_GAP).await;
    }
}

/// `CODEYARD_PERF_RELOAD`: start the extension in development, then swap in its two builds by turns, `n + 1` times, each
/// time waiting for the new build's first answer (up to 30 s). The first swap fills the incremental compile cache and
/// is not recorded; the measured ones start after a pause.
async fn reload_scenario(handle: WindowHandle<Workbench>, n: usize, cx: &mut gpui::AsyncApp) {
    let Some(parent) = crate::ext::dev_dir().and_then(|d| d.parent().map(std::path::Path::to_path_buf)) else { return };
    let (Ok(a), Ok(b)) = (std::fs::read(parent.join("a.wasm")), std::fs::read(parent.join("b.wasm"))) else {
        eprintln!("CODEYARD_PERF_RELOAD: a.wasm and b.wasm must be beside {}", parent.display());
        return;
    };
    cx.background_executor().timer(SETTLE).await;
    let settled = async |cx: &mut gpui::AsyncApp| {
        for _ in 0..300 {
            if handle.read_with(cx, |wb, _| wb.dev_settled_for_perf()).unwrap_or(true) {
                return;
            }
            cx.background_executor().timer(Duration::from_millis(100)).await;
        }
    };
    if !handle.update(cx, |wb, window, cx| wb.run_dev_command_for_perf(window, cx)).unwrap_or(false) {
        return;
    }
    settled(cx).await;
    for i in 0..=n {
        RELOAD_WARMING.store(i == 0, Ordering::Relaxed);
        let build = if i % 2 == 0 { &b } else { &a };
        if !handle.update(cx, |wb, window, cx| wb.hot_reload_for_perf(build, window, cx)).unwrap_or(false) {
            break;
        }
        settled(cx).await;
        cx.background_executor().timer(if i == 0 { RELOAD_SETTLE } else { TYPING_GAP }).await;
    }
    RELOAD_WARMING.store(false, Ordering::Relaxed);
}

/// Called once the first workbench window is open: records startup marks and runs the scripted scenario.
pub fn first_window(handle: WindowHandle<Workbench>, cx: &mut App) {
    if !enabled() {
        return;
    }
    mark("startup.window_open_ms");
    let typing: usize = std::env::var("CODEYARD_PERF_TYPING").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let market: usize = std::env::var("CODEYARD_PERF_MARKET").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let decorations: usize = std::env::var("CODEYARD_PERF_DECORATIONS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let providers: usize = std::env::var("CODEYARD_PERF_PROVIDERS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let completion: usize = std::env::var("CODEYARD_PERF_COMPLETION").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let words: usize = std::env::var("CODEYARD_PERF_WORDS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let lenses: usize = std::env::var("CODEYARD_PERF_LENSES").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let views: usize = std::env::var("CODEYARD_PERF_VIEWS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let reload: usize = std::env::var("CODEYARD_PERF_RELOAD").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let exit = std::env::var_os("CODEYARD_PERF_EXIT").is_some();
    handle
        .update(cx, |_, window, _| {
            after_next_frame(window, move |_, cx| {
                mark("startup.first_frame_ms");
                cx.spawn(async move |cx| {
                    if typing > 0 {
                        cx.background_executor().timer(SETTLE).await;
                        type_keys(handle, typing, "typing.keystroke_frame_ms", cx).await;
                    }
                    if words > 0 {
                        cx.background_executor().timer(SETTLE).await;
                        type_these(handle, words, &WORD_KEYS, "typing.keystroke_frame_words_ms", cx).await;
                    }
                    if decorations > 0 || providers > 0 {
                        extension_scenario(handle, decorations, providers, cx).await;
                    }
                    if completion > 0 {
                        completion_scenario(handle, completion, cx).await;
                    }
                    if lenses > 0 {
                        lens_scenario(handle, lenses, cx).await;
                    }
                    if views > 0 {
                        views_scenario(handle, views, cx).await;
                    }
                    if reload > 0 {
                        reload_scenario(handle, reload, cx).await;
                    }
                    if market > 0 {
                        cx.background_executor().timer(SETTLE).await;
                        cx.update_window(handle.into(), |_, window, cx| window.dispatch_action(Box::new(crate::workbench::ShowMarket), cx)).ok();
                        // The catalogue is read in the background; up to 30 s for a cold disk and a first fetch.
                        for _ in 0..300 {
                            if handle.read_with(cx, |wb, _| wb.market_ready_for_perf()).unwrap_or(true) {
                                break;
                            }
                            cx.background_executor().timer(Duration::from_millis(100)).await;
                        }
                        for i in 0..market {
                            let typed = cx.update_window(handle.into(), |_, window, cx| {
                                *MARKET_KEY.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
                                window.dispatch_keystroke(Keystroke::parse(market_key(i)).expect("valid keystroke"), cx);
                            });
                            if typed.is_err() {
                                break;
                            }
                            cx.background_executor().timer(TYPING_GAP).await;
                        }
                    }
                    if exit {
                        cx.update(|cx| cx.quit()).ok();
                    }
                })
                .detach();
            });
        })
        .ok();
}
