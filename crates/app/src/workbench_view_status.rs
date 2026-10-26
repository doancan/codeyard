//! Workbench: rendering: status bar.

use super::*;

/// Status bar entries: icon size, gap between icon and label, and horizontal padding of the hover area.
const SB_ICON: f32 = 14.;
const SB_GAP: f32 = 5.;
const SB_PAD: f32 = 8.;

impl Workbench {
    pub(super) fn status_bar(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let (remote_label, remote_bg, remote_fg, remote_icon) = match &self.remote.conn {
            Conn::Local => (crate::i18n::t("Local").to_string(), None, p.text_muted, "plug"),
            Conn::Connecting { alias, step } => {
                // The separate server panel on the left is gone; the connection step shows here.
                let phase = match step.load(std::sync::atomic::Ordering::Relaxed) {
                    s if s == host::ssh::Step::Uploading as u8 => format!(" ({})", crate::i18n::t("uploading agent")),
                    s if s == host::ssh::Step::Starting as u8 => format!(" ({})", crate::i18n::t("starting")),
                    _ => String::new(),
                };
                (format!("{}{phase}", crate::i18n::tf("Connecting to {}…", &[alias])), None, p.text, "loader")
            }
            Conn::Connected { alias } => (format!("ssh://{alias}"), Some(p.accent), p.on_accent, "server"),
            Conn::Reconnecting { alias, attempt } => (crate::i18n::tf("Reconnecting to {}… (attempt {})", &[alias, attempt]), None, p.warning, "loader"),
            Conn::Failed { alias, .. } => (crate::i18n::tf("{}: connection failed", &[alias]), Some(p.danger), p.on_accent, "circle-x"),
        };
        // One rhythm for every entry: same icon size, same icon-to-text gap, same side padding.
        let item = || div().flex().items_center().gap(px(SB_GAP)).px(px(SB_PAD)).h_full();
        let conflicts = self.active_editor().map_or(0, |e| e.read(cx).conflict_count());
        let editor_info = self.active_editor().map(|e| {
            let e = e.read(cx);
            let (l, col) = e.cursor_position();
            let pos = match (e.cursor_count(), e.selection_len()) {
                (n, _) if n > 1 => crate::i18n::tn("{} cursor", "{} cursors", n, &[]),
                (_, Some(n)) => crate::i18n::tf("Ln {}, Col {} ({} selected)", &[&l, &col, &n]),
                _ => crate::i18n::tf("Ln {}, Col {}", &[&l, &col]),
            };
            (pos, e.indent_label(), e.line_ending(), e.is_dirty(), e.is_saving(), e.language_label(), e.encoding().label())
        });
        let branch = self.vcs.branch.clone();
        // Errors go in the card at bottom right (error_toast); only info here.
        let notice = match &self.notice {
            Some(Notice::Info(m)) => Some((m.clone(), p.text_muted)),
            _ => None,
        };

        div()
            .h(px(layout::STATUSBAR_H))
            .flex_none()
            .flex()
            .items_center()
            .bg(c(p.surface))
            .border_t_1()
            .border_color(c(p.border))
            .text_size(px(text::XS.0))
            .text_color(c(p.text_muted))
            .child(
                item()
                    .id("remote-indicator")
                    .cursor_pointer()
                    .when_some(remote_bg, |d, bg| d.bg(c(bg)))
                    .text_color(c(remote_fg))
                    .child(if remote_icon == "loader" {
                        ui::spinner("remote-status", SB_ICON, remote_fg).into_any_element()
                    } else {
                        icon(remote_icon, SB_ICON, remote_fg).into_any_element()
                    })
                    .child(remote_label)
                    .when_some(
                        match &self.remote.conn {
                            Conn::Failed { message, .. } => Some(message.clone()),
                            Conn::Reconnecting { .. } => {
                                Some(crate::i18n::t("The connection dropped; trying to reconnect. Unsaved changes are kept.").to_string().into())
                            }
                            _ => None,
                        },
                        |d, tip| d.tooltip(move |_, cx| cx.new(|_| Tooltip(tip.clone())).into()),
                    )
                    .on_click(cx.listener(|this, _, w, cx| this.show_remote(&ShowRemote, w, cx))),
            )
            .when_some(branch, |d, b| {
                let (ahead, behind) = self.git_status().map_or((0, 0), |s| (s.ahead, s.behind));
                let sync = crate::git::counts_label(ahead, behind);
                let unpublished = self.git_status().is_some_and(|s| s.branch.is_some() && s.upstream.is_none());
                let tip = self.sync_tooltip();
                d.child(
                    item()
                        .id("sb-branch")
                        .cursor_pointer()
                        .hover(|d| d.bg(c(p.surface_hover)))
                        .child(icon("git-branch", SB_ICON, p.text_muted))
                        .child(b)
                        .on_click(cx.listener(|this, _, window, cx| this.show_branch_picker(window, cx))),
                )
                .child(
                    item()
                        .id("sb-sync")
                        .cursor_pointer()
                        .hover(|d| d.bg(c(p.surface_hover)))
                        .child(if self.vcs.sync.running == Some(sync_glue::Running::Sync) {
                            ui::spinner("sb-sync-busy", SB_ICON, p.text_muted).into_any_element()
                        } else {
                            icon(if unpublished { "cloud-upload" } else { "refresh-cw" }, SB_ICON, p.text_muted).into_any_element()
                        })
                        .when(!sync.is_empty(), |d| d.child(sync))
                        .tooltip(move |_, cx| cx.new(|_| Tooltip(tip.clone())).into())
                        .on_click(cx.listener(|this, _, window, cx| this.git_sync_auto(window, cx))),
                )
            })
            .child({
                // Problems: error and warning counts (click: open the panel).
                let (errors, warnings) = self.problem_counts();
                let count = |glyph: &'static str, n: usize, color: u32| {
                    div().flex().items_center().gap(px(SB_GAP)).child(icon(glyph, SB_ICON, if n > 0 { color } else { p.text_muted })).child(n.to_string())
                };
                item()
                    .id("problems-indicator")
                    .gap(px(SB_PAD))
                    .cursor_pointer()
                    .hover(|d| d.bg(c(p.surface_hover)))
                    .child(count("circle-x", errors, p.danger))
                    .child(count("alert", warnings, p.warning))
                    .tooltip(|_, cx| cx.new(|_| Tooltip(crate::i18n::t("Problems (⇧⌘M)").into())).into())
                    .on_click(cx.listener(|this, _, w, cx| this.show_problems(&ShowProblems, w, cx)))
            })
            .when(!self.ports.forwards.is_empty(), |d| {
                let n = self.ports.forwards.len();
                d.child(
                    item()
                        .id("ports-indicator")
                        .cursor_pointer()
                        .hover(|d| d.bg(c(p.surface_hover)))
                        .child(icon("plug", SB_ICON, p.text_muted))
                        .child(crate::i18n::tf("{} forwarded", &[&n]))
                        .on_click(cx.listener(|this, _, w, cx| this.show_ports(&ShowPorts, w, cx))),
                )
            })
            .when_some(self.transfer_active().map(|t| t.status()), |d, text| {
                d.child(
                    item()
                        .id("sb-transfer")
                        .cursor_pointer()
                        .hover(|d| d.bg(c(p.surface_hover)))
                        .text_color(c(p.text))
                        .child(ui::spinner("transfer-progress", SB_ICON, p.text_muted))
                        .child(text)
                        .tooltip(|_, cx| cx.new(|_| Tooltip(crate::i18n::t("File transfer in progress. Click to cancel.").into())).into())
                        .on_click(cx.listener(|this, _, w, cx| this.ask_cancel_transfer(w, cx))),
                )
            })
            .when_some(self.lsp.progress.clone(), |d, t| d.child(item().child(ui::spinner("lsp-progress", SB_ICON, p.text_muted)).child(t)))
            .when_some(notice, |d, (m, color)| d.child(item().text_color(c(color)).child(m)))
            .child(div().flex_1())
            .child(
                item()
                    .id("sb-terminal")
                    .cursor_pointer()
                    .hover(|d| d.bg(c(p.surface_hover)))
                    .child(icon("terminal", SB_ICON, if self.chrome.panel_open { p.text } else { p.text_muted }))
                    .on_click(cx.listener(|this, _, w, cx| this.toggle_panel(&TogglePanel, w, cx))),
            )
            .when(conflicts > 0, |d| {
                d.child(
                    item()
                        .id("sb-conflicts")
                        .cursor_pointer()
                        .hover(|d| d.bg(c(p.surface_hover)))
                        .text_color(c(p.warning))
                        .child(icon("git-compare", SB_ICON, p.warning))
                        .child(crate::i18n::tn("{} conflict", "{} conflicts", conflicts, &[]))
                        .tooltip(|_, cx| cx.new(|_| Tooltip(crate::i18n::t("Go to the next merge conflict").into())).into())
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some(ix) = this.tabs.active {
                                this.activate(ix, window, cx);
                            }
                            if let Some(ed) = this.active_editor().cloned() {
                                ed.update(cx, |e, cx| e.jump_conflict(true, cx));
                            }
                        })),
                )
            })
            // --- data ---
            .when_some(self.data_path_label(cx), |d, label| {
                d.child(
                    item()
                        .id("sb-data-path")
                        .cursor_pointer()
                        .hover(|d| d.bg(c(p.surface_hover)))
                        .child(label)
                        .tooltip(|_, cx| cx.new(|_| Tooltip(crate::i18n::t("Copy the path at the cursor").into())).into())
                        .on_click(cx.listener(|this, _, window, cx| this.data_copy_path(&DataCopyPath, window, cx))),
                )
            })
            .when_some(editor_info, |d, (pos, indent, eol, dirty, saving, language, encoding)| {
                d.when(saving, |d| d.child(item().child(crate::i18n::t("Saving…"))))
                    .when(dirty && !saving, |d| d.child(item().text_color(c(p.text)).child(crate::i18n::t("● Unsaved"))))
                    .child(
                        item()
                            .id("sb-position")
                            .cursor_pointer()
                            .hover(|d| d.bg(c(p.surface_hover)))
                            .child(pos)
                            .tooltip(|_, cx| cx.new(|_| Tooltip(crate::i18n::t("Go to Line/Column (⌃G)").into())).into())
                            .on_click(cx.listener(|this, _, window, cx| this.go_to_line(&GoToLine, window, cx))),
                    )
                    .child(
                        item()
                            .id("sb-indent")
                            .cursor_pointer()
                            .hover(|d| d.bg(c(p.surface_hover)))
                            .child(indent)
                            .on_click(cx.listener(|this, _, window, cx| this.show_indent_picker(window, cx))),
                    )
                    .child(item().child(encoding))
                    .child(
                        item()
                            .id("sb-eol")
                            .cursor_pointer()
                            .hover(|d| d.bg(c(p.surface_hover)))
                            .child(eol)
                            .on_click(cx.listener(|this, _, window, cx| this.show_eol_picker(window, cx))),
                    )
                    .child(
                        item()
                            .id("sb-language")
                            .cursor_pointer()
                            .hover(|d| d.bg(c(p.surface_hover)))
                            .child(language)
                            .tooltip(|_, cx| cx.new(|_| Tooltip(crate::i18n::t("Select Language Mode").into())).into())
                            .on_click(cx.listener(|this, _, window, cx| this.show_language_picker(window, cx))),
                    )
            })
            .when_some(self.update_status_item(), |d, item| d.child(self.update_status_button(item, p, cx)))
    }
}
