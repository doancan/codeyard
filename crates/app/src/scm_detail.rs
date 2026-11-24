//! Source Control's commit details: the selected commit's message, author, files and line counts.

use gpui::{Context, IntoElement, SharedString, div, prelude::*, px};

use super::{AccessFrame, DETAIL_FILE_ROWS, HistoryFrame, ScmEvent, ScmPanel, avatar, status_color};
use crate::graph::CommitDetail;
use crate::sections;
use crate::ui::{self, c, icon, layout, palette, text};

impl ScmPanel {
    /// "COMMIT": the selected commit's message, author, files and line counts.
    pub(super) fn detail_section(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let commit = self.selected.clone()?;
        let p = *palette(cx);
        let open = self.sections.is_open("details", true);
        let title = crate::i18n::tf("Commit {}", &[&commit.short]);
        let probe = |part: HistoryFrame| crate::painted::probe(&self.a11y_frames, AccessFrame::History(part));
        let header = sections::header("scm-detail-header", &title, None, open, &p)
            .relative()
            .child(probe(HistoryFrame::Header("details")))
            .child(
                ui::icon_button("scm-detail-close", "x", crate::i18n::t("Close details"), &p)
                    .relative()
                    .child(probe(HistoryFrame::Button("details-close")))
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.selected = None;
                        this.detail = None;
                        cx.notify();
                    })),
            )
            .on_click(cx.listener(|this, _, _, cx| this.toggle_section("details", cx)));
        let height = self.section_height("details", 170.);
        let hash = commit.hash.clone();
        let body = open.then(|| {
            let mut col = div()
                .id("scm-detail")
                .size_full()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(theme::SPACE[3]))
                .px(px(theme::SPACE[5]))
                .py(px(theme::SPACE[3]));
            let head = |col: gpui::Stateful<gpui::Div>, d: Option<&CommitDetail>| {
                let message = d.map_or(commit.subject.clone(), |d| d.message.clone());
                let (subject, rest) = message.split_once('\n').map_or((message.as_str(), ""), |(a, b)| (a, b.trim()));
                col.child(div().font_weight(gpui::FontWeight::SEMIBOLD).text_size(px(text::SM.0)).child(subject.to_string()))
                    .when(!rest.is_empty(), |c0| c0.child(div().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(rest.to_string())))
                    // Single-line meta: author (email and date in the tooltip), time and hash; icons instead of text.
                    .child({
                        let who: SharedString = format!("{} <{}>\n{}", commit.author, commit.email, commit.date).into();
                        div()
                            .flex()
                            .items_center()
                            .gap(px(theme::SPACE[4]))
                            .text_size(px(text::XS.0))
                            .child(
                                div()
                                    .id("scm-detail-author")
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap(px(theme::SPACE[2]))
                                    .child(avatar(&commit.author, 16., &p))
                                    .child(div().overflow_hidden().whitespace_nowrap().child(commit.author.clone()))
                                    .tooltip(move |_, cx| cx.new(|_| crate::workbench::Tooltip(who.clone())).into()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .gap(px(theme::SPACE[1]))
                                    .text_color(c(p.text_muted))
                                    .child(icon("clock", 12., p.text_muted))
                                    .child(crate::git::short_ago(&commit.when)),
                            )
                            .child(
                                div()
                                    .id("scm-detail-hash")
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .gap(px(theme::SPACE[1]))
                                    .cursor_pointer()
                                    .font_family(ui::MONO_FONT)
                                    .text_size(px(text::XXS.0))
                                    .text_color(c(p.accent))
                                    .child(icon("git-commit", 12., p.text_muted))
                                    .child(commit.short.clone())
                                    .tooltip(|_, cx| cx.new(|_| crate::workbench::Tooltip(crate::i18n::t("Copy the full hash").into())).into())
                                    .on_click({
                                        let h = hash.clone();
                                        move |_, _, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(h.clone()))
                                    }),
                            )
                    })
            };
            match (&self.detail, &self.detail_error) {
                (_, Some(e)) => {
                    head(col, None).child(div().text_color(c(p.danger)).child(crate::i18n::tf("Could not read the commit: {}", &[&e]))).into_any_element()
                }
                (None, None) => head(col, None).child(div().text_color(c(p.text_muted)).child(crate::i18n::t("Loading…"))).into_any_element(),
                (Some(d), None) => {
                    let (added, deleted) = d.totals();
                    col = head(col, Some(d)).child(
                        div()
                            .flex()
                            .gap(px(theme::SPACE[3]))
                            .text_size(px(text::XS.0))
                            .text_color(c(p.text_muted))
                            .items_center()
                            .child(icon("files", 12., p.text_muted))
                            .child(d.files.len().to_string())
                            .child(div().text_color(c(p.success)).child(format!("+{added}")))
                            .child(div().text_color(c(p.danger)).child(format!("−{deleted}"))),
                    );
                    let hidden = d.files.len().saturating_sub(DETAIL_FILE_ROWS);
                    let rows: Vec<_> = d
                        .files
                        .iter()
                        .take(DETAIL_FILE_ROWS)
                        .enumerate()
                        .map(|(i, f)| {
                            let (entry, file) = (commit.clone(), f.clone());
                            let dir = f.path.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
                            div()
                                .id(("scm-detail-file", i))
                                .relative()
                                .child(crate::painted::probe(&self.a11y_frames, AccessFrame::History(HistoryFrame::DetailFile(i))))
                                .h(px(layout::ROW_H))
                                .flex()
                                .flex_none()
                                .items_center()
                                .overflow_hidden()
                                .gap(px(theme::SPACE[3]))
                                .px(px(theme::SPACE[2]))
                                .rounded(px(theme::radius::SM))
                                .cursor_pointer()
                                .hover(|d| d.bg(c(p.surface_hover)))
                                .child(crate::icon_theme::file_icon_el(ui::file_name(&f.path), 14., &p))
                                .child(
                                    div()
                                        .flex_shrink()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_size(px(text::SM.0))
                                        .child(ui::file_name(&f.path).to_string()),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_size(px(text::XXS.0))
                                        .text_color(c(p.text_muted))
                                        .child(dir),
                                )
                                .when_some(f.added, |d, a| {
                                    d.child(div().flex_none().text_size(px(text::XXS.0)).text_color(c(p.success)).child(format!("+{a}")))
                                })
                                .when_some(f.deleted, |d, a| {
                                    d.child(div().flex_none().text_size(px(text::XXS.0)).text_color(c(p.danger)).child(format!("−{a}")))
                                })
                                .child(
                                    div()
                                        .flex_none()
                                        .w(px(12.))
                                        .text_size(px(text::XS.0))
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .text_color(c(status_color(f.status, &p)))
                                        .child(f.status.to_string()),
                                )
                                .on_click(cx.listener(move |_, _, _, cx| cx.emit(ScmEvent::OpenCommitFile(entry.clone(), file.clone()))))
                        })
                        .collect();
                    // This list is drawn whole on every frame: a commit of tens of thousands of files must not freeze the panel.
                    let more = (hidden > 0).then(|| {
                        div()
                            .debug_selector(|| "scm-detail-more".into())
                            .px(px(theme::SPACE[4]))
                            .py(px(theme::SPACE[2]))
                            .text_size(px(text::XS.0))
                            .text_color(c(p.text_muted))
                            .child(crate::i18n::tf("{} more files. Double-click the commit to see them all.", &[&hidden]))
                    });
                    col.children(rows).children(more).into_any_element()
                }
            }
        });
        Some(
            div()
                .flex()
                .flex_col()
                .flex_shrink()
                .min_h(px(layout::ROW_H + 4.))
                .relative()
                .child(probe(HistoryFrame::Section("details")))
                .child(self.top_sash("details", &p, cx))
                .child(header)
                .when_some(body, |d, body| d.child(div().h(px(height)).flex_shrink().min_h(px(60.)).child(body))),
        )
    }
}
