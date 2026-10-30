//! Workbench: the resident-memory cap of exthosts on macOS and Linux (docs/extensions-spike.md decision 5). While any
//! exthost runs, the window reads their resident sizes every few seconds (`ext_memory`) on a background thread and
//! kills one that is over the cap. That counts as a crash: three within a minute turn the extension off.

use std::sync::Arc;
use std::time::Duration;

use ext_api::value::LogLevel;

use super::*;
use crate::ext_runtime::Process;

pub(super) struct MemoryState {
    cap: u64,
    every: Duration,
    /// A check is scheduled; it ends on its own when no exthost runs.
    watching: bool,
}

impl Default for MemoryState {
    fn default() -> Self {
        MemoryState { cap: ext_manifest::limits::EXTHOST_MEMORY_BYTES, every: crate::ext_memory::EVERY, watching: false }
    }
}

impl Workbench {
    /// Start checking the exthosts' memory, if this system needs it and no check runs yet. Called when one starts.
    pub(super) fn watch_extension_memory(&mut self, cx: &mut Context<Self>) {
        let m = &mut self.cfg.exts.memory;
        if !crate::ext_memory::CHECKED_HERE || m.watching {
            return;
        }
        m.watching = true;
        let every = m.every;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(every).await;
                let running = this.update(cx, |this, _| {
                    let procs: Vec<(String, Arc<Process>)> = this.cfg.exts.code.procs.iter().map(|(id, p)| (id.clone(), p.clone())).collect();
                    if procs.is_empty() {
                        this.cfg.exts.memory.watching = false;
                    }
                    (procs, this.cfg.exts.memory.cap)
                });
                let Ok((procs, cap)) = running else { break };
                if procs.is_empty() {
                    break;
                }
                let measured = cx
                    .background_executor()
                    .spawn(async move { procs.into_iter().map(|(id, p)| (crate::ext_memory::resident_bytes(p.pid()), id, p)).collect::<Vec<_>>() })
                    .await;
                let sizes: Vec<(String, Option<u64>)> = measured.iter().map(|(b, id, _)| (id.clone(), *b)).collect();
                let over = crate::ext_memory::over(&sizes, cap);
                if over.is_empty() {
                    continue;
                }
                let over: Vec<(String, u64, Arc<Process>)> =
                    over.into_iter().filter_map(|(id, b)| measured.iter().find(|(_, i, _)| *i == id).map(|(_, _, p)| (id, b, p.clone()))).collect();
                if this.update(cx, |this, cx| this.stop_over_memory(over, cap, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Kill each exthost that went over the cap, if it is still the one running. Its exit is reported as a crash.
    fn stop_over_memory(&mut self, over: Vec<(String, u64, Arc<Process>)>, cap: u64, cx: &mut Context<Self>) {
        for (id, bytes, p) in over {
            if !self.cfg.exts.code.procs.get(&id).is_some_and(|q| Arc::ptr_eq(q, &p)) {
                continue;
            }
            let reason = crate::ext_memory::reason(bytes, cap);
            self.log_extension(&id, LogLevel::Error, format!("memory: {} bytes resident, over the cap of {cap} bytes; killed", bytes));
            self.notice = Some(Notice::Error(format!("{id}: {reason}").into()));
            std::thread::spawn(move || p.kill_as_crash(reason));
        }
        cx.notify();
    }
}

#[cfg(test)]
impl Workbench {
    /// A lower cap and a shorter interval, so a test can go over without using 512 MB.
    pub(crate) fn set_extension_memory_cap_for_test(&mut self, cap: u64, every: Duration) {
        self.cfg.exts.memory.cap = cap;
        self.cfg.exts.memory.every = every;
    }

    /// Exthosts running or starting in this window.
    pub(crate) fn extension_processes_for_test(&self) -> usize {
        crate::ext::current().active.iter().filter(|e| self.cfg.exts.code.running(e.id())).count()
    }

    /// The exthost's process id, while it runs.
    pub(crate) fn extension_pid_for_test(&self, extension: &str) -> Option<u32> {
        self.cfg.exts.code.procs.get(extension).map(|p| p.pid())
    }
}
