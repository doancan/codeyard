//! Workbench: rust-analyzer's health and check in the status bar, and the details page behind the item.

use gpui::{AnyElement, Context, Window, div, px};

use super::super::lsp_glue::ServerSlot;
use super::super::*;
#[cfg(not(test))]
use super::RUST_REQUEST_TIMEOUT;
use crate::lsp::{self, ServerEvent};
use crate::rust_toolchain::status::Tone;

/// Same rhythm as the other status bar items.
const ICON: f32 = 16.;
const GAP: f32 = 5.;
const PAD: f32 = 8.;
/// Width from which the status bar has room for the item's words.
const LABEL_FROM_WIDTH: f32 = 1100.;

impl Workbench {
    pub(in super::super) fn on_rust_server_event(&mut self, key: &str, ev: ServerEvent, cx: &mut Context<Self>) {
        let info = self.lsp.rust.servers.entry(key.to_string()).or_default();
        match ev {
            ServerEvent::Status(status) => {
                // The server finished loading: what it answered while it was still loading (no runnables, no tokens) is asked again.
                if status.quiescent
                    && info.loading()
                    && let Some(ServerSlot::Running(client)) = self.lsp.servers.get(key)
                {
                    for ed in self.all_editors() {
                        if ed.read(cx).lsp_client().is_some_and(|c| Arc::ptr_eq(c, client)) {
                            ed.update(cx, |e, cx| e.schedule_lsp_extras(200, cx));
                        }
                    }
                }
                let info = self.lsp.rust.servers.entry(key.to_string()).or_default();
                info.apply_status(status);
            }
            ServerEvent::Flycheck { token, step } => info.apply_flycheck(token, step),
            _ => return,
        }
        cx.notify();
    }

    /// The running rust-analyzer a Rust command talks to: the active file's, else the only one running.
    pub(in super::super) fn rust_server(&self, cx: &gpui::App) -> Option<(String, Arc<lsp::Client>)> {
        let running = |key: &String, slot: &ServerSlot| match slot {
            ServerSlot::Running(client) if client.config.language == "rust" => Some((key.clone(), client.clone())),
            _ => None,
        };
        let active = self.active_editor().and_then(|ed| ed.read(cx).lsp_client().cloned());
        let mut all: Vec<(String, Arc<lsp::Client>)> = self.lsp.servers.iter().filter_map(|(key, slot)| running(key, slot)).collect();
        all.sort_by(|a, b| a.0.cmp(&b.0));
        match active {
            Some(client) => all.into_iter().find(|(_, c)| Arc::ptr_eq(c, &client)),
            None if all.len() == 1 => all.pop(),
            None => None,
        }
    }

    /// The status bar item for the server of the active file; nothing until the server has reported.
    pub(in super::super) fn rust_status_item(&self, p: &Palette, width: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (key, _) = self.rust_server(cx)?;
        let info = self.lsp.rust.servers.get(&key).filter(|info| info.is_known())?;
        let (glyph, color) = match info.tone() {
            Tone::Error => ("circle-x", p.danger),
            Tone::Warning => ("alert", p.warning),
            Tone::Ok => ("circle-check", p.text_muted),
        };
        let description = info.description();
        let label = info.label();
        let hint = crate::i18n::t("Click for details.");
        let tip: gpui::SharedString = format!("{description}. {hint}").into();
        let el = div()
            .id("sb-rust")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(GAP))
            .px(px(PAD))
            .min_w(px(28.))
            .h_full()
            .cursor_pointer()
            .hover(|d| d.bg(c(p.surface_hover)))
            .child(if info.busy() { ui::spinner("sb-rust-busy", ICON, p.text_muted) } else { icon(glyph, ICON, color).into_any_element() })
            .when(width >= LABEL_FROM_WIDTH, |d| d.child(div().max_w(px(200.)).truncate().child(label)))
            .tooltip(move |_, cx| cx.new(|_| Tooltip(tip.clone())).into());
        let control = crate::a11y_controls::Control::button(crate::i18n::t("Status bar"), "rust-analyzer-status", description, |_, _| {})
            .description(crate::i18n::t("Opens the rust-analyzer status page"));
        Some(self.pressable(el, control, cx, |this, window, cx| this.show_rust_status(window, cx)).into_any_element())
    }

    /// *Rust: Show rust-analyzer Status*: the state, the server's message and its own report, as a read-only page.
    pub(in super::super) fn show_rust_status(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, client)) = self.rust_server(cx) else {
            self.notice = Some(Notice::Info(crate::i18n::t("rust-analyzer is not running for this file").into()));
            cx.notify();
            return;
        };
        let info = self.lsp.rust.servers.get(&key).cloned().unwrap_or_default();
        let document = self.active_editor().map(|ed| lsp::path_to_uri(&ed.read(cx).path));
        let params = serde_json::json!({ "textDocument": document.map(|uri| serde_json::json!({ "uri": uri })) });
        #[cfg(test)]
        let timeout = self.rust_request_timeout;
        #[cfg(not(test))]
        let timeout = RUST_REQUEST_TIMEOUT;
        // The server's report is asked for in the background; the page opens with it, or without when it does not answer.
        let report = cx.background_executor().spawn(async move { client.request(lsp::ANALYZER_STATUS, params, timeout) });
        cx.spawn_in(window, async move |this, cx| {
            let report = report.await.ok().and_then(|v| v.as_str().map(str::to_string));
            this.update_in(cx, |this, window, cx| {
                let name = format!("{}.md", crate::i18n::t("rust-analyzer status"));
                this.show_pull_page(name, &info.details_markdown(&key, report.as_deref()), window, cx);
            })
            .ok();
        })
        .detach();
    }
}
