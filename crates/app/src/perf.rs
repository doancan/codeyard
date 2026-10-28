//! Performance samples for `scripts/perf` (see `scripts/perf/README.md`). Off unless `CODEYARD_PERF=<file>` is set;
//! when off, every call is one relaxed atomic load.
//!
//! - `CODEYARD_PERF=<file>`: append one JSON line per sample, `{"metric":"…","value":…}`.
//! - `CODEYARD_PERF_TYPING=<n>`: after the first frame settles, type `n` characters into the focused editor and record
//!   keystroke → frame for each.
//! - `CODEYARD_PERF_EXIT=1`: quit when the scenario is done.

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

/// Time between typed characters: long enough for the previous frame to finish.
const TYPING_GAP: Duration = Duration::from_millis(60);
/// Wait after the first frame before typing: restored state, highlighting and the minimap settle.
const SETTLE: Duration = Duration::from_millis(1500);

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

/// Called once the first workbench window is open: records startup marks and runs the scripted scenario.
pub fn first_window(handle: WindowHandle<Workbench>, cx: &mut App) {
    if !enabled() {
        return;
    }
    mark("startup.window_open_ms");
    let typing: usize = std::env::var("CODEYARD_PERF_TYPING").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let exit = std::env::var_os("CODEYARD_PERF_EXIT").is_some();
    handle
        .update(cx, |_, window, _| {
            after_next_frame(window, move |_, cx| {
                mark("startup.first_frame_ms");
                cx.spawn(async move |cx| {
                    if typing > 0 {
                        cx.background_executor().timer(SETTLE).await;
                        for _ in 0..typing {
                            // Through the window, not the workbench entity: the key handler updates the workbench itself.
                            let typed = cx.update_window(handle.into(), |_, window, cx| {
                                let t0 = Instant::now();
                                window.dispatch_keystroke(Keystroke::parse("a").expect("valid keystroke"), cx);
                                after_next_frame(window, move |_, _| sample("typing.keystroke_frame_ms", ms(t0.elapsed())));
                            });
                            if typed.is_err() {
                                break;
                            }
                            cx.background_executor().timer(TYPING_GAP).await;
                        }
                        // Save, so the harness can check that the keys reached the editor (scripts/perf/run.sh).
                        cx.update_window(handle.into(), |_, window, cx| {
                            window.dispatch_keystroke(Keystroke::parse("cmd-s").expect("valid keystroke"), cx);
                        })
                        .ok();
                        cx.background_executor().timer(SETTLE).await;
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
