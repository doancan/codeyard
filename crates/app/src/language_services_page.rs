//! Explicit host-local setup, with bounded progress, retry and cancellation.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use gpui::{App, Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent, Task, Window, div, prelude::*, px};
use host::Host;

use crate::i18n::{t, tf};
use crate::language_service_install::{self as installer, Error, Installed, Progress, Stage};
use crate::language_services::Catalog;
use crate::ui::{c, palette, text};

pub struct InstalledEvent;

enum Status {
    Checking,
    Available(Option<Installed>),
    Installing,
    Failed(Error),
}

pub struct LanguageServicesPage {
    host: Arc<dyn Host>,
    destination: Result<String, Error>,
    status: Status,
    progress: Arc<Progress>,
    worker: Option<Task<()>>,
    pulse: Option<Task<()>>,
    focus: FocusHandle,
}

impl EventEmitter<InstalledEvent> for LanguageServicesPage {}
impl Focusable for LanguageServicesPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl LanguageServicesPage {
    pub fn new(host: Arc<dyn Host>, cx: &mut Context<Self>) -> Self {
        let destination = installer::destination(host.as_ref());
        let mut page = Self { host, destination, status: Status::Checking, progress: Arc::default(), worker: None, pulse: None, focus: cx.focus_handle() };
        if let Ok(root) = page.destination.clone() {
            let host = page.host.clone();
            let task = cx.background_executor().spawn(async move { installer::discover(host.as_ref(), &root) });
            page.worker = Some(cx.spawn(async move |this, cx| {
                let result = task.await;
                this.update(cx, |this, cx| {
                    this.worker = None;
                    this.status = match result {
                        Ok(installed) => Status::Available(installed),
                        Err(error) => Status::Failed(error),
                    };
                    cx.notify();
                })
                .ok();
            }));
        } else if let Err(error) = page.destination.clone() {
            page.status = Status::Failed(error);
        }
        page
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        let Ok(root) = self.destination.clone() else { return };
        if self.worker.is_some() {
            return;
        }
        self.progress = Arc::default();
        self.status = Status::Installing;
        let (host, progress) = (self.host.clone(), self.progress.clone());
        // The installer waits on host process channels. Keep those blocking waits off GPUI's
        // cooperative executor, including its deterministic test executor.
        let (sender, task) = futures::channel::oneshot::channel();
        if let Err(error) = std::thread::Builder::new().name("language-setup".into()).spawn(move || {
            sender.send(installer::install(host.as_ref(), &root, &progress)).ok();
        }) {
            self.status = Status::Failed(Error::Failed(error.to_string()));
            cx.notify();
            return;
        }
        self.worker = Some(cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or(Err(Error::Uncertain));
            this.update(cx, |this, cx| {
                this.worker = None;
                this.pulse = None;
                this.status = match result {
                    Ok(installed) => {
                        cx.emit(InstalledEvent);
                        Status::Available(Some(installed))
                    }
                    Err(error) => Status::Failed(error),
                };
                cx.notify();
            })
            .ok();
        }));
        self.pulse = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(200)).await;
                if this
                    .update(cx, |this, cx| {
                        cx.notify();
                        this.worker.is_some()
                    })
                    .ok()
                    != Some(true)
                {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if matches!(self.status, Status::Installing) && !matches!(self.progress.snapshot().stage, Stage::Publishing | Stage::Ready) {
            self.progress.cancel.store(true, Ordering::Release);
            cx.notify();
        }
    }

    fn key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "enter" if self.worker.is_none() => {
                self.start(cx);
                cx.stop_propagation();
            }
            "escape" if matches!(self.status, Status::Installing) => {
                self.cancel(cx);
                cx.stop_propagation();
            }
            _ => {}
        }
    }
}

impl Drop for LanguageServicesPage {
    fn drop(&mut self) {
        self.progress.cancel.store(true, Ordering::Release);
    }
}

fn error_text(error: &Error) -> String {
    match error {
        Error::Cancelled => t("Installation cancelled. The previous language services are unchanged.").into(),
        Error::Uncertain => t("The host stopped responding. Installation files were retained because a process may still be running. Reconnect and inspect the destination before retrying.").into(),
        Error::InvalidCatalog => t("The bundled service catalogue is invalid. Reinstall Codeyard before retrying.").into(),
        Error::UnsupportedRuntime => t("A compatible Node.js runtime could not be used on this host. Install Node.js 22.22.2 or newer with npm, then retry.").into(),
        Error::Integrity => t("The runtime checksum did not match. Nothing was activated. Check the connection and retry.").into(),
        Error::InvalidInstallation => t("The managed installation is incomplete or belongs to another catalogue. Reinstall the services to repair it.").into(),
        Error::Failed(detail) => tf("Language service setup failed: {}. Check the log and connection, then retry.", &[detail]),
    }
}

impl Render for LanguageServicesPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        let snapshot = self.progress.snapshot();
        let busy = self.worker.is_some();
        let status = match &self.status {
            Status::Checking => t("Checking installed language services…").to_string(),
            Status::Available(None) => t("Managed language services are not installed on this host.").into(),
            Status::Available(Some(installed)) => tf("Language services are ready with Node.js {}.", &[&installed.node_version]),
            Status::Failed(error) => error_text(error),
            Status::Installing if self.progress.cancel.load(Ordering::Acquire) => t("Stopping installation and cleaning temporary files…").into(),
            Status::Installing => match snapshot.stage {
                Stage::Checking => t("Checking the host runtime…"),
                Stage::RuntimeDownload => t("Downloading the pinned Node.js runtime…"),
                Stage::RuntimeVerify => t("Verifying the runtime checksum…"),
                Stage::Packages => t("Installing the locked language service packages…"),
                Stage::Verifying => t("Verifying installed service files…"),
                Stage::Publishing | Stage::Ready => t("Activating the completed installation…"),
            }
            .into(),
        };
        let install_label = if matches!(self.status, Status::Available(Some(_))) { t("Reinstall Language Services") } else { t("Install Language Services") };
        let can_cancel = matches!(self.status, Status::Installing) && !matches!(snapshot.stage, Stage::Publishing | Stage::Ready);
        let versions = Catalog::bundled()
            .ok()
            .map(|catalog| {
                tf(
                    "TypeScript {} · TypeScript Language Server {} · HTML/CSS {}",
                    &[&catalog.versions["typescript"], &catalog.versions["typescript-language-server"], &catalog.versions["vscode-langservers-extracted"]],
                )
            })
            .unwrap_or_else(|| error_text(&Error::InvalidCatalog));
        let button = |id: &'static str, label: &'static str, enabled: bool| {
            div()
                .id(id)
                .px(px(theme::SPACE[5]))
                .py(px(theme::SPACE[3]))
                .rounded(px(theme::radius::SM))
                .bg(c(if enabled { p.accent } else { p.surface_hover }))
                .text_color(c(if enabled { p.on_accent } else { p.text_muted }))
                .child(label)
                .when(enabled, |d| d.cursor_pointer())
        };
        div().id("language-services-page").key_context("LanguageServices").track_focus(&self.focus).on_key_down(cx.listener(Self::key))
            .size_full().min_w_0().bg(c(p.bg)).text_color(c(p.text)).flex().flex_col()
            .child(div().flex_none().px(px(theme::SPACE[9])).py(px(theme::SPACE[5])).border_b_1().border_color(c(p.border)).text_size(px(text::LG.0)).child(t("Language Services")))
            .child(div().id("language-service-details").flex_1().min_h_0().overflow_y_scroll().p(px(theme::SPACE[9])).flex().flex_col().gap(px(theme::SPACE[5]))
                .child(tf("Install on {}", &[&self.host.label()]))
                .child(versions)
                .child(tf("Destination: {}", &[&self.destination.as_deref().unwrap_or("—")]))
                .child(t("Installs JavaScript/TypeScript, HTML and CSS services from the locked npm catalogue. A pinned Node.js runtime is downloaded from nodejs.org when a compatible runtime with npm is unavailable. Package scripts are disabled."))
                .child(t("Existing custom server commands are preserved. Services start only in trusted folders. Closing this tab cancels an installation that has not reached activation."))
                .child(div().text_color(c(if matches!(self.status, Status::Failed(Error::Cancelled)) { p.text } else if matches!(self.status, Status::Failed(_)) { p.danger } else { p.text })).child(status))
                .child(div().flex().flex_wrap().gap(px(theme::SPACE[4]))
                    .child(button("install-language-services", install_label, !busy && self.destination.is_ok()).on_click(cx.listener(|this, _, _, cx| this.start(cx))))
                    .when(can_cancel, |d| d.child(button("cancel-language-services", t("Cancel"), true).on_click(cx.listener(|this, _, _, cx| this.cancel(cx))))))
                .child(t("Enter installs or retries. Escape cancels while installation is running."))
                .when(!snapshot.log.is_empty(), |d| d.child(div().id("language-service-log").max_h(px(280.)).overflow_y_scroll().p(px(theme::SPACE[4])).bg(c(p.surface)).font_family(crate::ui::MONO_FONT).text_size(px(text::XS.0)).child(snapshot.log))))
    }
}

#[cfg(test)]
#[path = "language_services_page_tests.rs"]
mod tests;
