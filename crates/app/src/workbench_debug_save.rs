//! Save workspace buffers before a debug build. Conflicts, save failures, Stop and a workspace switch cancel the start.

use super::*;
use std::time::{Duration, Instant};

type Resume = Box<dyn FnOnce(&mut Workbench, &mut Window, &mut Context<Workbench>)>;

enum Progress {
    Waiting,
    Ready,
    Failed(String),
    Cancelled,
}

impl Workbench {
    pub(in crate::workbench) fn debug_unsaved_editors(&self, root: &str, cx: &gpui::App) -> Vec<Entity<crate::editor::EditorView>> {
        self.all_editors()
            .into_iter()
            .filter(|ed| {
                let ed = ed.read(cx);
                ed.uses_host(&self.remote.host) && ed.path.starts_with(&format!("{}/", root.trim_end_matches('/'))) && (ed.is_dirty() || ed.is_saving())
            })
            .collect()
    }

    pub(in crate::workbench) fn cancel_pending_debug_start(&mut self, cx: &mut Context<Self>) {
        self.dbg.prepare_epoch = self.dbg.prepare_epoch.wrapping_add(1);
        self.dbg.save_task = None;
        if matches!(&self.notice, Some(Notice::Info(message)) if [crate::i18n::t("Saving files before debugging…"), crate::i18n::t("Building the tests…")].contains(&message.as_ref()))
        {
            self.notice = None;
        }
        cx.notify();
    }

    pub(in crate::workbench) fn save_before_debug(&mut self, root: String, cx: &mut Context<Self>, resume: Resume) {
        let editors = self.debug_unsaved_editors(&root, cx);
        for editor in &editors {
            if let Err(error) = editor.update(cx, |e, cx| e.save_for_build(cx)) {
                self.notice = Some(Notice::Error(crate::i18n::tf("Could not save files before debugging: {}", &[&error]).into()));
                cx.notify();
                return;
            }
        }
        let generation = self.lsp.generation;
        let asked = self.active_root(cx);
        self.notice = Some(Notice::Info(crate::i18n::t("Saving files before debugging…").into()));
        self.dbg.save_task = Some(cx.spawn(async move |this, cx| {
            let started = Instant::now();
            loop {
                cx.background_executor().timer(Duration::from_millis(50)).await;
                let progress = this.update(cx, |this, cx| {
                    if this.lsp.generation != generation || this.active_root(cx) != asked {
                        return Progress::Cancelled;
                    }
                    for editor in &editors {
                        if let Some(error) = editor.read(cx).build_save_error() {
                            return Progress::Failed(error);
                        }
                    }
                    if this.debug_unsaved_editors(&root, cx).is_empty() {
                        Progress::Ready
                    } else if started.elapsed() >= Duration::from_secs(30) {
                        Progress::Failed(crate::i18n::t("Files are still unsaved. Resolve editor errors, then start debugging again.").to_string())
                    } else {
                        Progress::Waiting
                    }
                });
                match progress {
                    Ok(Progress::Waiting) => {}
                    Ok(Progress::Ready) => {
                        with_window(&this, cx, move |this, window, cx| {
                            this.dbg.save_task = None;
                            this.notice = None;
                            resume(this, window, cx);
                        });
                        break;
                    }
                    Ok(Progress::Failed(error)) => {
                        this.update(cx, |this, cx| {
                            this.dbg.save_task = None;
                            this.notice = Some(Notice::Error(crate::i18n::tf("Could not save files before debugging: {}", &[&error]).into()));
                            cx.notify();
                        })
                        .ok();
                        break;
                    }
                    Ok(Progress::Cancelled) => {
                        this.update(cx, |this, cx| this.cancel_pending_debug_start(cx)).ok();
                        break;
                    }
                    Err(_) => break,
                }
            }
        }));
        cx.notify();
    }
}
