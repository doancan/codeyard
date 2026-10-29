//! Product and connection information in the editor's standard page surface.
use gpui::{App, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable, FontWeight, KeyDownEvent, Render, Window, div, prelude::*, px};

use crate::ui::{c, icon, palette, text};

pub struct CloseAbout;

pub struct AboutPage {
    focus: FocusHandle,
    remote: Option<(String, u32)>,
    copied: bool,
}

impl AboutPage {
    pub fn new(remote: Option<(String, u32)>, cx: &mut Context<Self>) -> Self {
        Self { focus: cx.focus_handle(), remote, copied: false }
    }

    pub(crate) fn a11y_details(&self) -> Vec<(String, String)> {
        let mut rows = vec![
            (crate::i18n::t("Version").into(), env!("CARGO_PKG_VERSION").into()),
            (crate::i18n::t("Platform").into(), format!("{} · {}", std::env::consts::OS, std::env::consts::ARCH)),
            (crate::i18n::t("Client protocol").into(), proto::VERSION.to_string()),
        ];
        if let Some((host, protocol)) = &self.remote {
            rows.push((crate::i18n::t("Remote connection").into(), host.clone()));
            rows.push((crate::i18n::t("Remote agent protocol").into(), protocol.to_string()));
        }
        rows
    }

    pub(crate) fn copy_version(&mut self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(format!("Codeyard {}", env!("CARGO_PKG_VERSION"))));
        self.copied = true;
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => {
                cx.stop_propagation();
                cx.emit(CloseAbout);
            }
            "enter" | "space" => {
                cx.stop_propagation();
                self.copy_version(cx);
            }
            _ => {}
        }
    }
}

impl EventEmitter<CloseAbout> for AboutPage {}
impl Focusable for AboutPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for AboutPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *palette(cx);
        let focused = self.focus.is_focused(window);
        let (tile, edge, mark) = crate::workbench::welcome_glue::brand_colors(&p);
        let row = |(label, value): (String, String)| {
            div()
                .flex()
                .flex_col()
                .gap(px(theme::SPACE[2]))
                .py(px(theme::SPACE[4]))
                .child(div().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(label))
                .child(div().text_size(px(text::BASE.0)).text_color(c(p.text)).child(value))
        };
        let info = div().flex().flex_col().children(self.a11y_details().into_iter().map(row));
        div().id("about-page").key_context("AboutPage").size_full().min_h_0().overflow_y_scroll().bg(c(p.bg)).child(
            div()
                .w_full()
                .max_w(px(660.))
                .mx_auto()
                .px(px(theme::SPACE[8]))
                .py(px(theme::SPACE[9]))
                .flex()
                .flex_col()
                .gap(px(theme::SPACE[8]))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(theme::SPACE[6]))
                        .child(
                            div()
                                .size(px(56.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(14.))
                                .bg(c(tile))
                                .border_1()
                                .border_color(c(edge))
                                .child(icon("codeyard-mark", 32., mark)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(theme::SPACE[2]))
                                .child(div().text_size(px(text::DISPLAY.0)).font_weight(FontWeight::SEMIBOLD).text_color(c(p.text)).child("Codeyard")) // i18n-keep: product name
                                .child(div().text_size(px(text::SM.0)).text_color(c(p.text_muted)).child(crate::i18n::t("About Codeyard"))),
                        ),
                )
                .child(div().h(px(1.)).bg(c(p.border)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(theme::SPACE[3]))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(text::LG.0))
                                .text_color(c(p.text))
                                .child(crate::i18n::t("Application details")),
                        )
                        .child(info),
                )
                .child(
                    div().flex().child(
                        div()
                            .id("about-copy")
                            .debug_selector(|| "about-copy".into())
                            .track_focus(&self.focus)
                            .min_h(px(32.))
                            .px(px(theme::SPACE[5]))
                            .flex()
                            .items_center()
                            .gap(px(theme::SPACE[3]))
                            .rounded(px(theme::radius::SM))
                            .border_1()
                            .border_color(c(if focused { p.accent } else { p.border }))
                            .bg(c(p.surface_hover))
                            .text_color(c(p.text))
                            .cursor_pointer()
                            .hover(|d| d.bg(c(p.surface_active)))
                            .child(icon("copy", 14., p.text_muted))
                            .child(if self.copied { crate::i18n::t("Copied") } else { crate::i18n::t("Copy Version") })
                            .on_key_down(cx.listener(Self::key_down))
                            .on_click(cx.listener(|this, _, window, cx| {
                                window.focus(&this.focus);
                                this.copy_version(cx)
                            })),
                    ),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn shows_client_and_known_remote_protocols(cx: &mut gpui::TestAppContext) {
        cx.update(|app| app.set_global(crate::ui::ActiveTheme(theme::DARK)));
        let (page, cx) = cx.add_window_view(|_, cx| AboutPage::new(Some(("ssh://example".into(), 37)), cx));
        let rows = page.read_with(cx, |p, _| p.a11y_details());
        assert!(rows.contains(&("Client protocol".into(), proto::VERSION.to_string())));
        assert!(rows.contains(&("Remote agent protocol".into(), "37".into())));
        assert!(rows.contains(&("Remote connection".into(), "ssh://example".into())));
        let (local, cx) = cx.add_window_view(|_, cx| AboutPage::new(None, cx));
        let rows = local.read_with(cx, |p, _| p.a11y_details());
        assert!(!rows.iter().any(|(label, _)| label == "Remote agent protocol"));
    }
}
