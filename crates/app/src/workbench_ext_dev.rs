//! Workbench: hot reload of the extension in development (`cyx dev`, docs/extensions/developer-guide.md). `cyx dev`
//! rebuilds `extension.wasm` when a source file is saved; a 100 ms watch here notices the new build (and reloads the
//! extension set first when the manifest changed with it). If the extension is running, its exthost is stopped (`deactivate`
//! runs, and the SDKs keep `ctx.state` under `codeyard.reload-state` in its storage), the new build is compiled with
//! Cranelift's incremental cache (`Cache::compiled_dev`) and started with the activation reason `reload`, and its open
//! views are drawn again. Calls made meanwhile wait for the new build.

use std::time::{Duration, Instant, SystemTime};

use ext_api::value::LogLevel;

use super::*;

/// How often the build output is looked at: one `stat` of `extension.wasm`.
const POLL: Duration = Duration::from_millis(100);

/// The activation reason of a hot reload; the SDKs restore `ctx.state` for it.
pub(super) const RELOAD: &str = "reload";

/// What the window knows about the dev extension's build.
#[derive(Default)]
pub(super) struct DevCode {
    /// The build and the manifest's modification time the watch last saw.
    polled: (Option<(u64, SystemTime)>, Option<SystemTime>),
    /// The build the window last acted on (size and modification time of `extension.wasm`).
    seen: Option<(u64, SystemTime)>,
    /// The extension being swapped and when its new build was noticed, until the new build answers its first request
    /// (`extensions.hot_reload_ms`, and a line in the extension's log).
    since: Option<(String, Instant)>,
}

/// The size and modification time of the dev extension's component, if there is one.
fn build_stamp() -> Option<(u64, SystemTime)> {
    let m = std::fs::metadata(crate::ext::dev_dir()?.join(ext_pkg::COMPONENT)).ok()?;
    Some((m.len(), m.modified().ok()?))
}

fn manifest_stamp() -> Option<SystemTime> {
    std::fs::metadata(crate::ext::dev_dir()?.join(ext_manifest::FILE_NAME)).and_then(|m| m.modified()).ok()
}

impl Workbench {
    /// With `--extension-dev`: watch for a new build of the extension and swap it in. Other files of the folder are
    /// the settings watcher's (`ext::watch_stamp`, once a second).
    pub(super) fn watch_dev_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if crate::ext::dev_dir().is_none() {
            return;
        }
        let now = (build_stamp(), manifest_stamp());
        self.cfg.exts.code.dev.seen = now.0;
        self.cfg.exts.code.dev.polled = now;
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let stamp = (build_stamp(), manifest_stamp());
                let alive = this.update_in(cx, |this, window, cx| {
                    // A build being written is picked up once complete: cyx renames it into place.
                    let dev = &mut this.cfg.exts.code.dev;
                    if stamp.0.is_none() || dev.polled.0 == stamp.0 {
                        return;
                    }
                    let manifest_changed = dev.polled.1 != stamp.1;
                    dev.polled = stamp;
                    if manifest_changed {
                        // The full reload (`sync_running_extensions` swaps the build after it).
                        this.reload_config(window, cx);
                    } else {
                        this.reload_dev_code(window, cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// After a reload of the extension set: if the dev extension has a new build and is running, swap it in.
    pub(super) fn reload_dev_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = crate::ext::dev_dir() else { return };
        let stamp = build_stamp();
        let dev = &mut self.cfg.exts.code.dev;
        if stamp.is_none() || dev.seen == stamp {
            return;
        }
        dev.seen = stamp;
        let noticed = Instant::now();
        let reg = crate::ext::current();
        let Some(active) = reg.active.iter().find(|e| e.dir == dir && e.manifest.code.is_some()) else { return };
        // Not running: its next start loads the new build anyway.
        if self.cfg.exts.code.running(active.id()) {
            self.cfg.exts.code.dev.since = Some((active.id().to_string(), noticed));
            self.restart_dev_extension(active.id().to_string(), window, cx);
        }
    }

    fn restart_dev_extension(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let code = &mut self.cfg.exts.code;
        // Still starting the previous build: swap once it is up.
        if code.queued.contains_key(&id) {
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(Duration::from_millis(20)).await;
                let _ = this.update_in(cx, |this, window, cx| this.restart_dev_extension(id, window, cx));
            })
            .detach();
            return;
        }
        let Some(old) = code.procs.remove(&id) else { return };
        code.pending.lock().unwrap_or_else(|p| p.into_inner()).fail_all(&id, crate::i18n::t("the extension was reloaded"));
        code.timers.retain(|(e, _), _| *e != id);
        code.record().exited(&id);
        // Calls from now on wait here for the new build.
        code.queued.insert(id.clone(), Vec::new());
        self.forget_extension_marks(&id, cx);
        self.log_extension(&id, LogLevel::Info, "new build: reloading".into());
        let stopped = cx.background_executor().spawn(async move { old.retire() });
        cx.spawn_in(window, async move |this, cx| {
            stopped.await;
            let _ = this.update_in(cx, |this, window, cx| this.start_new_build(&id, window, cx));
        })
        .detach();
    }

    /// The old build has stopped (its state is saved): start the new one, hand it the calls that waited, and draw its
    /// open views again.
    fn start_new_build(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let waiting = self.cfg.exts.code.queued.remove(id).unwrap_or_default();
        let waiting_was_empty = waiting.is_empty();
        let reg = crate::ext::current();
        let Some(active) = reg.active.iter().find(|e| e.id() == id).cloned() else { return };
        self.send_to_extension(&active, RELOAD, None, window, cx);
        for m in waiting {
            self.send_to_extension(&active, RELOAD, Some(m), window, cx);
        }
        let views: Vec<String> = self.cfg.exts.views.hosted.iter().filter(|h| h.decl.extension == id).map(|h| h.decl.id.clone()).collect();
        let asked = !waiting_was_empty || !views.is_empty();
        for v in views {
            self.request_ext_view_render(&v, window, cx);
        }
        let dev = &mut self.cfg.exts.code.dev;
        if let Some((_, since)) = &dev.since {
            let ms = since.elapsed().as_millis();
            // Nothing asked of the new build yet: its first answer, whenever it comes, says nothing about the reload.
            if !asked {
                dev.since = None;
            }
            self.log_extension(id, LogLevel::Info, format!("old build stopped after {ms} ms; starting the new one"));
        }
    }

    /// A request of `id` was answered: the first answer of a new build ends the hot reload's measurement.
    pub(super) fn dev_request_answered(&mut self, id: &str) {
        let code = &mut self.cfg.exts.code;
        if !code.dev.since.as_ref().is_some_and(|(e, _)| e == id) || !code.procs.contains_key(id) {
            return;
        }
        let Some((_, since)) = code.dev.since.take() else { return };
        let ms = since.elapsed().as_secs_f64() * 1000.0;
        crate::perf::sample("extensions.hot_reload_ms", ms);
        self.log_extension(id, LogLevel::Info, format!("reloaded: the new build answered {ms:.0} ms after it was noticed"));
    }
}

#[cfg(test)]
impl Workbench {
    /// Why the extension's exthost last started, and how many times it has.
    pub fn extension_activation_for_test(&self, id: &str) -> Option<(String, u32)> {
        self.cfg.exts.code.record().activations.get(id).map(|a| (a.reason.clone(), a.count))
    }

    /// What the watch does when only the build changed.
    pub fn reload_dev_code_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reload_dev_code(window, cx);
    }
}
