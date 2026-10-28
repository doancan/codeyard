//! Editor: painting the minimap and popups (completion, hover, peek).

use super::*;
use crate::i18n::{t, tf};

impl EditorView {
    /// Minimap: each line 2 px, each character 1 px, in syntax color; the visible area
    /// is framed. Only lines that fit in the strip are drawn. Returns: first line.
    pub(super) fn paint_minimap(&self, bounds: Bounds<Pixels>, line_h: f32, p: &Palette, window: &mut Window) -> usize {
        const ROW: f32 = 2.;
        let height = bounds.size.height / px(1.);
        let x0 = bounds.right() - px(MINIMAP_W + 12.);
        let total = self.buffer.len_lines();
        let rows_fit = (height / ROW).floor().max(1.) as usize;
        let offset = -self.scroll.0.borrow().base_handle.offset().y / px(1.);
        let first = (offset / line_h).floor().max(0.) as usize;
        let visible = (height / line_h).ceil() as usize;
        // If the document doesn't fit the strip, the strip scrolls proportionally to the editor (as VS Code does).
        let start = if total <= rows_fit {
            0
        } else {
            let max_start = total - rows_fit;
            let frac = first as f32 / (total.saturating_sub(visible)).max(1) as f32;
            ((frac * max_start as f32) as usize).min(max_start)
        };
        let end = (start + rows_fit).min(total);
        window.paint_quad(fill(Bounds::new(point(x0, bounds.top()), size(px(MINIMAP_W), bounds.size.height)), ui::ca(p.text, 0.02)));
        let vy = (first.saturating_sub(start)) as f32 * ROW;
        window.paint_quad(fill(Bounds::new(point(x0, bounds.top() + px(vy)), size(px(MINIMAP_W), px(visible as f32 * ROW))), ui::ca(p.text_muted, 0.14)));
        let rope = self.buffer.rope();
        let spans = self.syntax.as_ref().map(|s| s.highlights(rope, rope.line_to_byte(start)..rope.line_to_byte(end)));
        let max_cols = MINIMAP_W as usize;
        for (row, line) in (start..end).enumerate() {
            let text = self.buffer.line(line);
            let runs = match &spans {
                Some(sp) => syntax::line_runs(rope.line_to_byte(line), text.len(), sp),
                None => vec![(text.len(), None)],
            };
            let y = bounds.top() + px(row as f32 * ROW);
            let (mut byte, mut col) = (0usize, 0usize);
            for (len, role) in runs {
                let color = role_color(role, p);
                let piece = &text[byte..byte + len];
                byte += len;
                let mut seg: Option<(usize, usize)> = None; // (start column, length)
                for ch in piece.chars() {
                    let w = if ch == '\t' { 4 } else { 1 };
                    if col >= max_cols {
                        break;
                    }
                    if ch.is_whitespace() {
                        if let Some((a, n)) = seg.take() {
                            window.paint_quad(fill(Bounds::new(point(x0 + px(a as f32), y), size(px(n as f32), px(1.4))), ui::ca(color, 0.7)));
                        }
                    } else {
                        seg = Some(seg.map_or((col, w), |(a, n)| (a, n + w)));
                    }
                    col += w;
                }
                if let Some((a, n)) = seg {
                    window.paint_quad(fill(Bounds::new(point(x0 + px(a as f32), y), size(px(n as f32), px(1.4))), ui::ca(color, 0.7)));
                }
            }
        }
        start
    }

    /// Center the line clicked/dragged in the minimap.
    pub(super) fn minimap_jump(&mut self, y: Pixels, cx: &mut Context<Self>) {
        let row = ((y - self.surface_origin.y) / px(2.)).max(0.) as usize;
        let line = (self.minimap_start + row).min(self.buffer.len_lines().saturating_sub(1));
        let row = self.row_of_line(line);
        self.scroll.scroll_to_item_strict(row, ScrollStrategy::Center);
        cx.notify();
    }

    /// Completion and hover popups (absolutely positioned relative to the surface).
    pub(super) fn popups(&self, p: &Palette, font: SharedString, font_size: f32, line_h: f32, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let mut out = Vec::new();
        // To the right of the conflict header: "Accept Current | Accept Incoming | Accept Both" (like VS Code's CodeLens).
        for (ix, conflict) in self.conflicts.iter().enumerate() {
            let head = self.buffer.line_to_char(conflict.start);
            let Some(v) = self.visible.iter().find(|v| v.line == conflict.start && v.start == head) else { continue };
            let x = v.text_left + v.layout.width + px(theme::SPACE[8]) - self.surface_origin.x;
            let y = v.bounds.top() - self.surface_origin.y;
            let link = |id: &'static str, label: &'static str, choice: crate::conflict::Choice| {
                div().id((id, ix)).cursor_pointer().text_color(c(p.text_muted)).hover(|d| d.text_color(c(p.accent)).underline()).child(label).on_mouse_down(
                    MouseButton::Left,
                    cx.on(move |e, _, _, cx| {
                        cx.stop_propagation();
                        e.resolve_conflict(ix, choice, cx);
                    }),
                )
            };
            let bar = || div().text_color(c(p.border)).child("|");
            use crate::conflict::Choice;
            out.push(
                div()
                    .absolute()
                    .left(x)
                    .top(y)
                    .h(px(line_h))
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[3]))
                    .text_size(px(text::XS.0))
                    .whitespace_nowrap()
                    .child(link("cf-current", t("Accept Current Change"), Choice::Current))
                    .child(bar())
                    .child(link("cf-incoming", t("Accept Incoming Change"), Choice::Incoming))
                    .child(bar())
                    .child(link("cf-both", t("Accept Both Changes"), Choice::Both))
                    .child(bar())
                    .child(
                        div()
                            .id(("cf-merge", ix))
                            .cursor_pointer()
                            .text_color(c(p.accent))
                            .hover(|d| d.underline())
                            .child(t("Open Merge Editor"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.on(move |_, _: &MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    cx.emit(EditorEvent::OpenMergeEditor);
                                }),
                            ),
                    )
                    .into_any_element(),
            );
        }
        out.extend(self.color_picker_popup(p, font.clone(), font_size, cx));
        // `.http`: "Send Request" to the right of each request's line.
        for line in self.http_request_lines() {
            let head = self.buffer.line_to_char(line);
            let Some(v) = self.visible.iter().find(|v| v.line == line && v.start == head) else { continue };
            out.push(
                div()
                    .id(("http-send", line))
                    .absolute()
                    .left(v.text_left + v.layout.width + px(theme::SPACE[8]) - self.surface_origin.x)
                    .top(v.bounds.top() - self.surface_origin.y)
                    .h(px(line_h))
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[2]))
                    .cursor_pointer()
                    .text_size(px(text::XS.0))
                    .whitespace_nowrap()
                    .text_color(c(p.text_muted))
                    .hover(|d| d.text_color(c(p.accent)))
                    .child(icon("play", 12., p.accent))
                    .child(t("Send Request"))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.on(move |_, _: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            cx.emit(EditorEvent::SendRequest(line));
                        }),
                    )
                    .into_any_element(),
            );
        }
        if let Some(comp) = &self.completion
            && let Some((v, x)) = self.screen_pos(comp.start)
        {
            let x = x - self.surface_origin.x;
            const ROWS: usize = 10;
            // If there's no room below, open above the line (as VS Code does).
            let row_h = line_h + 2.;
            let list_h = px(comp.shown.len().min(ROWS) as f32 * row_h + theme::SPACE[1] * 2. + 2.);
            let below = v.bounds.bottom() - self.surface_origin.y;
            let above = v.bounds.top() - self.surface_origin.y - list_h;
            let y = if below + list_h > self.surface_h && above >= px(0.) { above } else { below };
            let first = comp.selected.saturating_sub(ROWS - 1).min(comp.shown.len().saturating_sub(ROWS));
            let rows = comp.shown.iter().enumerate().skip(first).take(ROWS).map(|(pos, &i)| {
                let it = &comp.items[i];
                let selected = pos == comp.selected;
                let (glyph, color) = kind_glyph(it.kind, p);
                let hits = crate::fuzzy::score(&comp.prefix, &it.label).map(|(_, h)| h).unwrap_or_default();
                let highlights = crate::fuzzy::hit_ranges(&it.label, &hits)
                    .into_iter()
                    .map(|r| (r, gpui::HighlightStyle { color: Some(c(p.accent).into()), font_weight: Some(gpui::FontWeight::BOLD), ..Default::default() }));
                let label = gpui::StyledText::new(it.label.clone()).with_highlights(highlights);
                div()
                    .id(("completion", pos))
                    .h(px(row_h))
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[3]))
                    .px(px(theme::SPACE[3]))
                    .when(selected, |d| d.bg(c(p.surface_active)))
                    .child(div().w(px(16.)).flex_none().text_color(c(color)).child(glyph))
                    .child(div().flex_none().text_color(c(p.text)).child(label))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(font_size - 1.))
                            .text_color(c(p.text_muted))
                            .flex()
                            .justify_end()
                            .children(it.detail.clone()),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.on(move |e, _, window, cx| {
                            cx.stop_propagation();
                            if let Some(comp) = &mut e.completion {
                                comp.selected = pos;
                            }
                            e.completion_accept(&CompletionAccept, window, cx);
                        }),
                    )
            });
            out.push(
                div()
                    .absolute()
                    .left(x - px(theme::SPACE[3] + 16. + theme::SPACE[3]))
                    .top(y)
                    .w(px(440.))
                    .flex()
                    .flex_col()
                    .py(px(theme::SPACE[1]))
                    .bg(c(p.surface_raised))
                    .border_1()
                    .border_color(c(p.border))
                    .rounded(px(theme::radius::MD))
                    .shadow_md()
                    .font_family(font.clone())
                    .text_size(px(font_size))
                    .children(rows)
                    .into_any_element(),
            );
            // The selected item's docs go to the right of the list (clipped if long).
            if let Some(it) = comp.shown.get(comp.selected).map(|&i| &comp.items[i])
                && let Some(doc) = &it.doc
            {
                let text: String = doc.lines().take(24).collect::<Vec<_>>().join("\n");
                out.push(
                    div()
                        .absolute()
                        .left(x - px(theme::SPACE[3] + 16. + theme::SPACE[3]) + px(440. + 4.))
                        .top(y)
                        .w(px(360.))
                        .max_h(px(row_h * ROWS as f32 * 1.5))
                        .overflow_hidden()
                        .flex()
                        .flex_col()
                        .gap(px(theme::SPACE[2]))
                        .p(px(theme::SPACE[3]))
                        .bg(c(p.surface_raised))
                        .border_1()
                        .border_color(c(p.border))
                        .rounded(px(theme::radius::MD))
                        .shadow_md()
                        .text_size(px(font_size - 1.))
                        .children(it.detail.clone().map(|d| div().font_family(font.clone()).text_color(c(p.text_muted)).child(d)))
                        .child(div().text_color(c(p.text)).child(text))
                        .into_any_element(),
                );
            }
        }
        // Signature help: above the cursor's line, active parameter in bold.
        if let Some((label, active)) = self.signature.as_ref().filter(|_| self.completion.is_none())
            && let Some((v, x)) = self.screen_pos(self.buffer.cursor())
        {
            let x = x - self.surface_origin.x;
            let y = v.bounds.top() - self.surface_origin.y - px(line_h + 10.);
            let highlights = active
                .clone()
                .into_iter()
                .map(|r| (r, gpui::HighlightStyle { font_weight: Some(gpui::FontWeight::BOLD), color: Some(c(p.accent).into()), ..Default::default() }));
            out.push(
                div()
                    .absolute()
                    .left(x)
                    .top(y.max(px(0.)))
                    .max_w(px(640.))
                    .px(px(theme::SPACE[3]))
                    .py(px(theme::SPACE[1]))
                    .bg(c(p.surface_raised))
                    .border_1()
                    .border_color(c(p.border))
                    .rounded(px(theme::radius::MD))
                    .shadow_md()
                    .font_family(font.clone())
                    .text_size(px(font_size - 1.))
                    .text_color(c(p.text))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(gpui::StyledText::new(label.clone()).with_highlights(highlights))
                    .into_any_element(),
            );
        }
        if let Some(pk) = &self.peek
            && let Some((v, x)) = self.screen_pos(self.buffer.cursor())
        {
            let x = (x - self.surface_origin.x).max(px(0.));
            let y = v.bounds.bottom() - self.surface_origin.y;
            let (path, line, col) = (pk.path.clone(), pk.line, pk.col);
            let rows = pk.lines.iter().enumerate().map(|(i, t)| {
                let n = pk.first + i;
                let hit = n == pk.line as usize;
                div()
                    .flex()
                    .h(px(line_h))
                    .when(hit, |d| d.bg(ui::ca(p.accent, 0.18)))
                    .child(div().w(px(44.)).flex_none().text_color(c(p.line_number)).child((n + 1).to_string()))
                    .child(div().flex_1().whitespace_nowrap().overflow_hidden().text_color(c(p.text)).child(t.replace('\t', "    ")))
            });
            let rel = pk.path.rsplit('/').next().unwrap_or(&pk.path).to_string();
            out.push(
                div()
                    .id("peek")
                    .absolute()
                    .left(x)
                    .top(y)
                    .w(px(640.))
                    .flex()
                    .flex_col()
                    .bg(c(p.surface_raised))
                    .border_1()
                    .border_color(c(p.accent))
                    .rounded(px(theme::radius::MD))
                    .shadow_md()
                    .font_family(font.clone())
                    .text_size(px(font_size))
                    .child(
                        div()
                            .id("peek-open")
                            .h(px(line_h + 4.))
                            .px(px(theme::SPACE[3]))
                            .flex()
                            .items_center()
                            .gap(px(theme::SPACE[3]))
                            .cursor_pointer()
                            .border_b_1()
                            .border_color(c(p.border))
                            .text_size(px(font_size - 1.))
                            .text_color(c(p.text_muted))
                            .hover(|d| d.text_color(c(p.accent)))
                            .child(format!("{rel}:{}", pk.line.saturating_add(1)))
                            .when(pk.more > 0, |d| d.child(tf("(+{} more)", &[&pk.more])))
                            .child(div().flex_1())
                            .child(t("click to open · Esc to close"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.on(move |e, _, _, cx| {
                                    cx.stop_propagation();
                                    e.peek = None;
                                    cx.emit(EditorEvent::OpenLocation { path: path.clone(), line, col });
                                    cx.notify();
                                }),
                            ),
                    )
                    .children(rows)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .into_any_element(),
            );
        }
        if let Some(h) = &self.hover {
            let x = h.position.x - self.surface_origin.x;
            let y = self.screen_pos(h.at).map_or(h.position.y, |(v, _)| v.bounds.bottom()) - self.surface_origin.y;
            let diag_rows = hover_rows(&h.diagnostics, &self.path, self.lsp_root.as_deref()).into_iter().enumerate().map(|(i, row)| match row {
                HoverRow::Diagnostic { severity, text } => {
                    let (glyph, color) = match severity {
                        1 => ("circle-x", p.danger),
                        2 => ("alert", p.warning),
                        _ => ("info", p.info),
                    };
                    div().flex().gap(px(theme::SPACE[2])).child(icon(glyph, 14., color)).child(div().flex_1().child(text))
                }
                HoverRow::Related { text, .. } => {
                    div().pl(px(theme::SPACE[6])).text_color(c(p.text_muted)).cursor_pointer().hover(|d| d.text_color(c(p.accent))).child(text).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |e, _, _, cx| {
                            cx.stop_propagation();
                            e.open_hover_row(i, cx);
                        }),
                    )
                }
                HoverRow::More(text) => div().pl(px(theme::SPACE[6])).text_color(c(p.text_muted)).child(text),
            });
            out.push(
                div()
                    .id("hover")
                    .absolute()
                    .left(x)
                    .top(y)
                    .max_w(px(560.))
                    .max_h(px(320.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(theme::SPACE[2]))
                    .p(px(theme::SPACE[3]))
                    .bg(c(p.surface_raised))
                    .border_1()
                    .border_color(c(p.border))
                    .rounded(px(theme::radius::MD))
                    .shadow_md()
                    .text_size(px(font_size - 1.))
                    .children(diag_rows)
                    .when_some(h.text.clone(), |d, t| d.child(div().font_family(font.clone()).text_color(c(p.text)).child(t)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .into_any_element(),
            );
        }
        out
    }

    // --- Git gutter ---
}

/// One line of the diagnostics in a hover: a diagnostic, a related location under it (clickable), or how many more there are.
#[derive(Debug, Clone, PartialEq)]
pub enum HoverRow {
    Diagnostic { severity: u8, text: String },
    Related { text: String, path: String, line: u32, col: u32 },
    More(String),
}

/// The hover's diagnostic lines: each diagnostic (code first), then its related locations.
pub fn hover_rows(diagnostics: &[crate::lsp::Diagnostic], file: &str, root: Option<&str>) -> Vec<HoverRow> {
    let mut rows = Vec::new();
    for d in diagnostics {
        rows.push(HoverRow::Diagnostic { severity: d.severity, text: d.headline() });
        for r in &d.related {
            rows.push(HoverRow::Related { text: crate::lsp::related_label(r, file, root), path: r.path.clone(), line: r.start.0, col: r.start.1 });
        }
        if d.more_related > 0 {
            rows.push(HoverRow::More(tf("(+{} more)", &[&d.more_related])));
        }
    }
    rows
}

impl EditorView {
    /// A click on hover line `i`: a related location opens (the workbench handles `OpenLocation`, remote paths included).
    pub(super) fn open_hover_row(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(h) = &self.hover else { return };
        if let Some(HoverRow::Related { path, line, col, .. }) = hover_rows(&h.diagnostics, &self.path, self.lsp_root.as_deref()).into_iter().nth(i) {
            self.hover = None;
            cx.emit(EditorEvent::OpenLocation { path, line, col });
            cx.notify();
        }
    }

    #[cfg(test)]
    pub fn hover_rows_for_test(&self) -> Vec<HoverRow> {
        self.hover.as_ref().map(|h| hover_rows(&h.diagnostics, &self.path, self.lsp_root.as_deref())).unwrap_or_default()
    }

    #[cfg(test)]
    pub fn click_hover_row_for_test(&mut self, i: usize, cx: &mut Context<Self>) {
        self.open_hover_row(i, cx);
    }
}
