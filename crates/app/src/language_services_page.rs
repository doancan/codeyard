//! Explicit host-local setup, with bounded progress, retry and cancellation.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AccessAction {
    Install,
    Cancel,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AccessView {
    pub token: u64,
    pub revision: u64,
    pub summary: String,
    pub status: String,
    pub install_label: String,
    pub busy: bool,
    pub error: bool,
    pub can_install: bool,
    pub can_cancel: bool,
    pub cancel_visible: bool,
    pub focused: bool,
    pub control_focus: Option<AccessAction>,
    pub scrollable: bool,
    pub bounds: [Option<accesskit::Rect>; 5],
}

pub struct LanguageServicesPage {
    host: Arc<dyn Host>,
    destination: Result<String, Error>,
    status: Status,
    progress: Arc<Progress>,
    worker: Option<Task<()>>,
    pulse: Option<Task<()>>,
    focus: FocusHandle,
    install_focus: FocusHandle,
    cancel_focus: FocusHandle,
    scroll: gpui::ScrollHandle,
    bounds: Rc<Cell<[Option<accesskit::Rect>; 5]>>,
    token: u64,
    revision: u64,
}

impl EventEmitter<InstalledEvent> for LanguageServicesPage {}
impl Focusable for LanguageServicesPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl LanguageServicesPage {
    pub fn new(host: Arc<dyn Host>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let destination = installer::destination(host.as_ref());
        let mut page = Self {
            host,
            destination,
            status: Status::Checking,
            progress: Arc::default(),
            worker: None,
            pulse: None,
            focus: cx.focus_handle(),
            install_focus: cx.focus_handle(),
            cancel_focus: cx.focus_handle(),
            scroll: gpui::ScrollHandle::new(),
            bounds: Rc::new(Cell::new([None; 5])),
            token: NEXT.fetch_add(1, Ordering::Relaxed),
            revision: 0,
        };
        if let Ok(root) = page.destination.clone() {
            let host = page.host.clone();
            let task = cx.background_executor().spawn(async move { installer::discover(host.as_ref(), &root) });
            page.worker = Some(cx.spawn_in(window, async move |this, cx| {
                let result = task.await;
                this.update_in(cx, |this, window, cx| {
                    this.worker = None;
                    this.revision = this.revision.wrapping_add(1);
                    this.status = match result {
                        Ok(installed) => Status::Available(installed),
                        Err(error) => Status::Failed(error),
                    };
                    this.refocus(window, cx);
                    cx.notify();
                })
                .ok();
            }));
        } else if let Err(error) = page.destination.clone() {
            page.status = Status::Failed(error);
        }
        page
    }

    fn owns_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus.contains_focused(window, cx) || self.install_focus.is_focused(window) || self.cancel_focus.is_focused(window)
    }

    fn can_install(&self) -> bool {
        self.worker.is_none() && self.destination.is_ok()
    }
    fn cancel_visible(&self) -> bool {
        matches!(self.status, Status::Installing) && !matches!(self.progress.snapshot().stage, Stage::Publishing | Stage::Ready)
    }
    fn can_cancel(&self) -> bool {
        self.cancel_visible() && !self.progress.cancel.load(Ordering::Acquire)
    }

    fn refocus(&self, window: &mut Window, cx: &App) {
        if self.owns_focus(window, cx) {
            window.focus(if self.can_install() {
                &self.install_focus
            } else if self.can_cancel() {
                &self.cancel_focus
            } else {
                &self.focus
            });
        }
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(root) = self.destination.clone() else { return };
        if !self.can_install() {
            return;
        }
        self.progress = Arc::default();
        self.status = Status::Installing;
        self.revision = self.revision.wrapping_add(1);
        let (host, progress) = (self.host.clone(), self.progress.clone());
        // Host process waits belong on a dedicated worker, including under GPUI's cooperative test executor.
        let (sender, task) = futures::channel::oneshot::channel();
        if let Err(error) = std::thread::Builder::new().name("language-setup".into()).spawn(move || {
            sender.send(installer::install(host.as_ref(), &root, &progress)).ok();
        }) {
            self.status = Status::Failed(Error::Failed(error.to_string()));
            self.refocus(window, cx);
            cx.notify();
            return;
        }
        self.worker = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await.unwrap_or(Err(Error::Uncertain));
            this.update_in(cx, |this, window, cx| {
                this.worker = None;
                this.revision = this.revision.wrapping_add(1);
                this.pulse = None;
                this.status = match result {
                    Ok(installed) => {
                        cx.emit(InstalledEvent);
                        Status::Available(Some(installed))
                    }
                    Err(error) => Status::Failed(error),
                };
                this.refocus(window, cx);
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
        self.refocus(window, cx);
        cx.notify();
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.can_cancel() {
            self.progress.cancel.store(true, Ordering::Release);
            self.revision = self.revision.wrapping_add(1);
            self.refocus(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn perform(&mut self, action: AccessAction, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !(match action {
            AccessAction::Install => self.can_install(),
            AccessAction::Cancel => self.can_cancel(),
        }) {
            return false;
        }
        window.focus(&self.focus);
        match action {
            AccessAction::Install => self.start(window, cx),
            AccessAction::Cancel => self.cancel(window, cx),
        }
        true
    }

    pub(crate) fn access_focus(&self, control: Option<AccessAction>, window: &mut Window, cx: &App) {
        match control {
            Some(AccessAction::Install) if self.can_install() => window.focus(&self.install_focus),
            Some(AccessAction::Cancel) if self.can_cancel() => window.focus(&self.cancel_focus),
            None => window.focus(&self.focus),
            _ => {}
        }
        let _ = cx;
    }

    pub(crate) fn access_scroll(&mut self, down: bool, cx: &mut Context<Self>) {
        let mut offset = self.scroll.offset();
        let delta = self.scroll.bounds().size.height * if down { -0.8 } else { 0.8 };
        offset.y = (offset.y + delta).clamp(-self.scroll.max_offset().height, px(0.));
        self.scroll.set_offset(offset);
        cx.notify();
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "enter" | "space" if self.cancel_focus.is_focused(window) && self.can_cancel() => {
                self.perform(AccessAction::Cancel, window, cx);
                cx.stop_propagation();
            }
            "enter" if self.can_install() => {
                self.perform(AccessAction::Install, window, cx);
                cx.stop_propagation();
            }
            "space" if self.install_focus.is_focused(window) && self.can_install() => {
                self.perform(AccessAction::Install, window, cx);
                cx.stop_propagation();
            }
            "escape" if self.can_cancel() => {
                self.cancel(window, cx);
                cx.stop_propagation();
            }
            "tab" => {
                let control = if self.can_install() {
                    Some(AccessAction::Install)
                } else if self.can_cancel() {
                    Some(AccessAction::Cancel)
                } else {
                    None
                };
                if self.install_focus.is_focused(window) || self.cancel_focus.is_focused(window) {
                    window.focus(&self.focus);
                } else {
                    self.access_focus(control, window, cx);
                }
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    fn status_text(&self, stage: Stage) -> String {
        match &self.status {
            Status::Checking => t("Checking installed language services…").into(),
            Status::Available(None) => t("Managed language services are not installed on this host.").into(),
            Status::Available(Some(installed)) => tf("Language services are ready with Node.js {}.", &[&installed.node_version]),
            Status::Failed(error) => error_text(error),
            Status::Installing if self.progress.cancel.load(Ordering::Acquire) => t("Stopping installation and cleaning temporary files…").into(),
            Status::Installing => match stage {
                Stage::Checking => t("Checking the host runtime…"),
                Stage::RuntimeDownload => t("Downloading the pinned Node.js runtime…"),
                Stage::RuntimeVerify => t("Verifying the runtime checksum…"),
                Stage::Packages => t("Installing the locked language service packages…"),
                Stage::Verifying => t("Verifying installed service files…"),
                Stage::Publishing | Stage::Ready => t("Activating the completed installation…"),
            }
            .into(),
        }
    }

    fn install_label(&self) -> &'static str {
        if matches!(self.status, Status::Available(Some(_))) { t("Reinstall Language Services") } else { t("Install Language Services") }
    }

    fn versions(&self) -> String {
        Catalog::bundled()
            .ok()
            .map(|catalog| {
                tf(
                    "TypeScript {} · TypeScript Language Server {} · HTML/CSS {}",
                    &[&catalog.versions["typescript"], &catalog.versions["typescript-language-server"], &catalog.versions["vscode-langservers-extracted"]],
                )
            })
            .unwrap_or_else(|| error_text(&Error::InvalidCatalog))
    }

    pub(crate) fn access_view(&self, window: &Window, cx: &App) -> AccessView {
        let host = if self.host.is_remote() { self.host.label() } else { t("Local").into() };
        let summary = format!(
            "{}\n{}\n{}\n{}",
            tf("Install on {}", &[&host]),
            self.versions(),
            tf("Destination: {}", &[&self.destination.as_deref().unwrap_or("—")]),
            t(
                "Existing custom server commands are preserved. Services start only in trusted folders. Closing this tab cancels an installation that has not reached activation."
            )
        );
        AccessView {
            token: self.token,
            revision: self.revision,
            summary: crate::lsp::clip_text(&summary, 8192),
            status: self.status_text(self.progress.snapshot().stage),
            install_label: self.install_label().into(),
            busy: self.worker.is_some(),
            error: matches!(self.status, Status::Failed(ref error) if *error != Error::Cancelled),
            can_install: self.can_install(),
            can_cancel: self.can_cancel(),
            cancel_visible: self.cancel_visible(),
            focused: self.owns_focus(window, cx),
            control_focus: if self.install_focus.is_focused(window) {
                Some(AccessAction::Install)
            } else if self.cancel_focus.is_focused(window) {
                Some(AccessAction::Cancel)
            } else {
                None
            },
            scrollable: self.scroll.max_offset().height > px(0.),
            bounds: self.bounds.get(),
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

fn measure(index: usize, bounds: Rc<Cell<[Option<accesskit::Rect>; 5]>>) -> impl IntoElement {
    gpui::canvas(
        |bounds, window, _| {
            let bounds = bounds.intersect(&window.content_mask().bounds);
            let scale = f64::from(window.scale_factor());
            if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) || !scale.is_finite() || scale <= 0. {
                return None;
            }
            Some(accesskit::Rect::new(
                f64::from(bounds.left() / px(1.)) * scale,
                f64::from(bounds.top() / px(1.)) * scale,
                f64::from(bounds.right() / px(1.)) * scale,
                f64::from(bounds.bottom() / px(1.)) * scale,
            ))
        },
        move |_, measured, _, _| {
            let mut values = bounds.get();
            values[index] = measured;
            bounds.set(values);
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

impl Render for LanguageServicesPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        let snapshot = self.progress.snapshot();
        let status = self.status_text(snapshot.stage);
        let is_error = matches!(self.status, Status::Failed(ref error) if *error != Error::Cancelled);
        let button = |id: &'static str, label: &'static str, enabled: bool, focus: &FocusHandle, index| {
            let bg = if enabled { p.accent } else { p.surface_hover };
            div()
                .id(id)
                .relative()
                .min_w_0()
                .max_w_full()
                .px(px(theme::SPACE[5]))
                .py(px(theme::SPACE[3]))
                .border_2()
                .border_color(c(if focus.is_focused(window) { p.text } else { bg }))
                .rounded(px(theme::radius::SM))
                .bg(c(bg))
                .text_color(c(if enabled { p.on_accent } else { p.text_muted }))
                .track_focus(focus)
                .child(label)
                .child(measure(index, self.bounds.clone()))
                .when(enabled, |d| d.cursor_pointer())
        };
        let host = if self.host.is_remote() { self.host.label() } else { t("Local").into() };
        div().id("language-services-page").relative().key_context("LanguageServices").track_focus(&self.focus).on_key_down(cx.listener(Self::key))
            .size_full().min_w_0().bg(c(p.bg)).text_color(c(p.text)).flex().flex_col()
            .child(div().flex_none().px(px(theme::SPACE[5])).py(px(theme::SPACE[4])).border_b_1().border_color(c(p.border)).text_size(px(text::LG.0)).child(t("Language Services")))
            .child(div().flex_none().px(px(theme::SPACE[5])).py(px(theme::SPACE[4])).border_b_1().border_color(c(p.border)).flex().flex_wrap().gap(px(theme::SPACE[4]))
                .child(button("install-language-services", self.install_label(), self.can_install(), &self.install_focus, 3)
                    .on_click(cx.listener(|this, _, window, cx| { this.perform(AccessAction::Install, window, cx); })))
                .when(self.cancel_visible(), |d| d.child(button("cancel-language-services", t("Cancel"), self.can_cancel(), &self.cancel_focus, 4)
                    .on_click(cx.listener(|this, _, window, cx| { this.perform(AccessAction::Cancel, window, cx); })))))
            .child(div().id("language-service-details").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.scroll).p(px(theme::SPACE[5])).flex().flex_col().gap(px(theme::SPACE[5]))
                .child(div().relative().text_color(c(if is_error { p.danger } else { p.text })).child(status).child(measure(2, self.bounds.clone())))
                .child(div().relative().flex().flex_col().gap(px(theme::SPACE[5]))
                    .child(tf("Install on {}", &[&host])).child(self.versions())
                    .child(tf("Destination: {}", &[&self.destination.as_deref().unwrap_or("—")]))
                    .child(t("Installs JavaScript/TypeScript, HTML and CSS services from the locked npm catalogue. A pinned Node.js runtime is downloaded from nodejs.org when a compatible runtime with npm is unavailable. Package scripts are disabled."))
                    .child(t("Existing custom server commands are preserved. Services start only in trusted folders. Closing this tab cancels an installation that has not reached activation."))
                    .child(measure(1, self.bounds.clone())))
                .child(t("Enter installs or retries. Escape cancels while installation is running."))
                .when(!snapshot.log.is_empty(), |d| d.child(div().id("language-service-log").max_h(px(280.)).overflow_y_scroll().p(px(theme::SPACE[4])).bg(c(p.surface)).font_family(crate::ui::MONO_FONT).text_size(px(text::XS.0)).child(snapshot.log))))
            .child(measure(0, self.bounds.clone()))
    }
}

#[cfg(test)]
#[path = "language_services_page_tests.rs"]
mod tests;
