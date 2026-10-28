//! Workbench: rendering: status bar.

use super::*;

/// Status bar entries: icon size, gap between icon and label, and horizontal padding of the hover area.
const SB_ICON: f32 = 16.;
const SB_GAP: f32 = 5.;
const SB_PAD: f32 = 8.;

impl Workbench {
    pub(super) fn status_bar(&self, p: &Palette, width: f32, cx: &mut Context<Self>) -> impl IntoElement {
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
        let item = || div().flex().flex_none().items_center().gap(px(SB_GAP)).px(px(SB_PAD)).min_w(px(28.)).h_full();
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
        let busy = usize::from(conflicts > 0)
            + usize::from(editor_info.as_ref().is_some_and(|info| info.3 || info.4))
            + usize::from(self.active_forwards() > 0)
            + usize::from(self.transfer_active().is_some())
            + usize::from(self.update_status_item().is_some());
        // Operational signals take priority over secondary editor metadata.
        let detail_width = width - busy as f32 * 140.;
        let labels = width >= 1100.;
        let branch = self.vcs.branch.clone();
        // Errors go in the card at bottom right (error_toast); only info here.
        let notice = match &self.notice {
            Some(Notice::Info(m)) => Some((m.clone(), p.text_muted)),
            _ => None,
        };

        div()
            .id("status-bar")
            .relative()
            .debug_selector(|| "status-bar".into())
            .h(px(layout::STATUSBAR_H))
            .flex_none()
            .flex()
            .items_center()
            .bg(c(p.surface))
            .border_t_1()
            .border_color(c(p.border))
            .text_size(px(text::XS.0))
            .text_color(c(p.text_muted))
            .child(a11y::landmark(a11y::Landmark::Status, cx))
            .when_some(self.roots.file_operation.clone(), |d, progress| {
                let processed = progress.completed.load(std::sync::atomic::Ordering::Acquire);
                let label = crate::i18n::tf("Files: {0}/{1}", &[&processed, &progress.total]);
                d.child(
                    item()
                        .id("file-operation-progress")
                        .cursor_pointer()
                        .child(ui::spinner("file-operation-spinner", SB_ICON, p.text_muted))
                        .child(label)
                        .tooltip(|_, cx| cx.new(|_| Tooltip(crate::i18n::t("Cancel file operation").into())).into())
                        .on_click(cx.listener(|this, _, window, cx| this.cancel_file_operation(&CancelFileOperation, window, cx))),
                )
            })
            .child(
                item()
                    .id("remote-indicator")
                    .flex_shrink()
                    .cursor_pointer()
                    .when_some(remote_bg, |d, bg| d.bg(c(bg)))
                    .text_color(c(remote_fg))
                    .child(if remote_icon == "loader" {
                        ui::spinner("remote-status", SB_ICON, remote_fg).into_any_element()
                    } else {
                        icon(remote_icon, SB_ICON, remote_fg).into_any_element()
                    })
                    .when(width >= 640., |d| d.child(div().min_w_0().max_w(px(180.)).truncate().child(remote_label)))
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
                // A star after the branch name: the working tree has changes that are not committed yet.
                let dirty = self.git_status().is_some_and(|s| !s.changes.is_empty());
                let running = self.vcs.sync.running == Some(sync_glue::Running::Sync);
                let tip = self.sync_tooltip();
                d.child(
                    item()
                        .id("sb-branch")
                        .flex_shrink()
                        .cursor_pointer()
                        .hover(|d| d.bg(c(p.surface_hover)))
                        .child(icon("git-branch", SB_ICON, p.text_muted))
                        .child(div().min_w_0().max_w(px(if width < 800. { 100. } else { 180. })).truncate().child(if dirty {
                            format!("{b}*")
                        } else {
                            b.to_string()
                        }))
                        .tooltip(move |_, cx| cx.new(|_| Tooltip(b.clone())).into())
                        .on_click(cx.listener(|this, _, window, cx| this.show_branch_picker(window, cx))),
                )
                .child(
                    item()
                        .id("sb-sync")
                        .cursor_pointer()
                        .hover(|d| d.bg(c(p.surface_hover)))
                        .child(if running {
                            ui::spinner("sb-sync-busy", SB_ICON, p.text_muted).into_any_element()
                        } else {
                            icon(if unpublished { "cloud-upload" } else { "refresh-cw" }, SB_ICON, p.text_muted).into_any_element()
                        })
                        // The counts are what tell you a sync is due, so they stay at every width; pending commits read in
                        // the regular text color.
                        .when(!sync.is_empty(), |d| d.text_color(c(p.text)).child(div().max_w(px(96.)).truncate().child(sync)))
                        .when(labels && unpublished && !running, |d| d.child(crate::i18n::t("Publish Branch")))
                        .tooltip(move |_, cx| cx.new(|_| Tooltip(tip.clone())).into())
                        .on_click(cx.listener(|this, _, window, cx| this.git_sync_auto(window, cx))),
                )
            })
            .child({
                // Problems: error and warning counts (click: open the panel).
                let (errors, warnings) = self.problem_counts();
                let count = |glyph: &'static str, n: usize, color: u32| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(SB_GAP))
                        .child(icon(glyph, SB_ICON, if n > 0 { color } else { p.text_muted }))
                        .child(div().max_w(px(36.)).truncate().child(n.to_string()))
                };
                item()
                    .id("problems-indicator")
                    .gap(px(SB_PAD))
                    .cursor_pointer()
                    .hover(|d| d.bg(c(p.surface_hover)))
                    .child(count("circle-x", errors, p.danger))
                    .child(count("alert", warnings, p.warning))
                    .tooltip(move |_, cx| cx.new(|_| Tooltip(crate::i18n::tf("{0} errors, {1} warnings", &[&errors, &warnings]).into())).into())
                    .on_click(cx.listener(|this, _, w, cx| this.show_problems(&ShowProblems, w, cx)))
            })
            .when(self.active_forwards() > 0 && width >= 800., |d| {
                let n = self.active_forwards();
                d.child(
                    item()
                        .id("ports-indicator")
                        .cursor_pointer()
                        .hover(|d| d.bg(c(p.surface_hover)))
                        .child(icon("plug", SB_ICON, p.text_muted))
                        .when(labels, |d| d.child(div().max_w(px(80.)).truncate().child(crate::i18n::tf("{} forwarded", &[&n]))))
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
                        .when(labels, |d| d.child(div().max_w(px(120.)).truncate().child(text)))
                        .tooltip(|_, cx| cx.new(|_| Tooltip(crate::i18n::t("File transfer in progress. Click to cancel.").into())).into())
                        .on_click(cx.listener(|this, _, w, cx| this.ask_cancel_transfer(w, cx))),
                )
            })
            .when_some(self.lsp.progress.clone().filter(|_| detail_width >= 1000.), |d, t| {
                d.child(
                    item().flex_shrink().child(ui::spinner("lsp-progress", SB_ICON, p.text_muted)).child(div().min_w_0().max_w(px(180.)).truncate().child(t)),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .when_some(notice, |d, (m, color)| d.child(div().px(px(SB_PAD)).text_color(c(color)).truncate().child(m))),
            )
            .children(self.web_preview_status(p, cx))
            .child(self.chrome.resources.clone())
            .child(
                item()
                    .id("sb-terminal")
                    .cursor_pointer()
                    .hover(|d| d.bg(c(p.surface_hover)))
                    .child(icon("terminal", SB_ICON, if self.chrome.panel_open { p.text } else { p.text_muted }))
                    .tooltip(|_, cx| cx.new(|_| Tooltip(crate::i18n::t("Toggle Panel (⌘J)").into())).into())
                    .on_click(cx.listener(|this, _, w, cx| this.toggle_panel(&TogglePanel, w, cx))),
            )
            .when(conflicts > 0, |d| {
                d.child(
                    item()
                        .id("sb-conflicts")
                        .debug_selector(|| "sb-conflicts".into())
                        .cursor_pointer()
                        .hover(|d| d.bg(c(p.surface_hover)))
                        .text_color(c(p.warning))
                        .child(icon("git-compare", SB_ICON, p.warning))
                        .when(labels, |d| d.child(div().max_w(px(80.)).truncate().child(crate::i18n::tn("{} conflict", "{} conflicts", conflicts, &[]))))
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
            .when_some(self.data_path_label(cx).filter(|_| detail_width >= 1000.), |d, label| {
                d.child(
                    item()
                        .id("sb-data-path")
                        .cursor_pointer()
                        .hover(|d| d.bg(c(p.surface_hover)))
                        .child(div().max_w(px(140.)).truncate().child(label))
                        .tooltip(|_, cx| cx.new(|_| Tooltip(crate::i18n::t("Copy the path at the cursor").into())).into())
                        .on_click(cx.listener(|this, _, window, cx| this.data_copy_path(&DataCopyPath, window, cx))),
                )
            })
            .when_some(editor_info, |d, (pos, indent, eol, dirty, saving, language, encoding)| {
                d.when(saving, |d| {
                    d.child(
                        item()
                            .id("sb-saving")
                            .child(ui::spinner("sb-saving", SB_ICON, p.text_muted))
                            .when(labels, |d| d.child(crate::i18n::t("Saving…")))
                            .tooltip(|_, cx| cx.new(|_| Tooltip(crate::i18n::t("Saving…").into())).into()),
                    )
                })
                .when(dirty && !saving, |d| {
                    d.child(
                        item()
                            .id("sb-unsaved")
                            .debug_selector(|| "sb-unsaved".into())
                            .text_color(c(p.text))
                            .when(labels, |d| d.child(crate::i18n::t("● Unsaved")))
                            .when(!labels, |d| d.child(div().size(px(6.)).rounded_full().bg(c(p.text))))
                            .tooltip(|_, cx| cx.new(|_| Tooltip(crate::i18n::t("● Unsaved").into())).into()),
                    )
                })
                .when(detail_width >= 720., |d| {
                    d.child(
                        item()
                            .id("sb-position")
                            .cursor_pointer()
                            .hover(|d| d.bg(c(p.surface_hover)))
                            .child(div().max_w(px(180.)).truncate().child(pos))
                            .tooltip(|_, cx| cx.new(|_| Tooltip(crate::i18n::t("Go to Line/Column (⌃G)").into())).into())
                            .on_click(cx.listener(|this, _, window, cx| this.go_to_line(&GoToLine, window, cx))),
                    )
                })
                .when(detail_width >= 1100., |d| {
                    d.child(
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
                })
                .when(detail_width >= 900., |d| {
                    d.child(
                        item()
                            .id("sb-language")
                            .cursor_pointer()
                            .hover(|d| d.bg(c(p.surface_hover)))
                            .child(div().max_w(px(120.)).truncate().child(language))
                            .tooltip(|_, cx| cx.new(|_| Tooltip(crate::i18n::t("Select Language Mode").into())).into())
                            .on_click(cx.listener(|this, _, window, cx| this.show_language_picker(window, cx))),
                    )
                })
            })
            .when_some(self.update_status_item(), |d, item| d.child(self.update_status_button(item, labels, p, cx)))
    }
}
