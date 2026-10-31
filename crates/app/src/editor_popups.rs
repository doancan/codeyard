//! Editor: painting the minimap and popups (completion, hover, peek).

use super::*;
use crate::i18n::{t, tf};

/// A popup part whose painted bounds the accessibility tree reports.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PopupFrame {
    /// The suggestion list, and suggestion `i` (its index among all suggestions).
    Suggestions,
    Suggestion(usize),
    /// The hover popup and its diagnostic line `i`, and the signature popup.
    Hover,
    HoverLine(usize),
    Signature,
}

/// A line of the hover for the accessibility tree: its text, whether a click opens it (a related location) and where
/// it was painted.
#[derive(Clone, Debug, PartialEq)]
pub struct HoverLineA11y {
    pub text: String,
    pub opens: bool,
    pub bounds: Option<accesskit::Rect>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HoverA11y {
    pub lines: Vec<HoverLineA11y>,
    pub bounds: Option<accesskit::Rect>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SignatureA11y {
    pub label: String,
    /// The parameter being typed.
    pub active: Option<String>,
    pub bounds: Option<accesskit::Rect>,
}

/// The editor's hover and signature popups as the accessibility tree shows them (`a11y_popups.rs`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PopupsA11y {
    pub hover: Option<HoverA11y>,
    pub signature: Option<SignatureA11y>,
}

impl PopupsA11y {
    /// Drop the painted positions (the editor was not drawn in the window asking).
    pub fn forget_frames(&mut self) {
        if let Some(h) = &mut self.hover {
            h.bounds = None;
            h.lines.iter_mut().for_each(|l| l.bounds = None);
        }
        if let Some(s) = &mut self.signature {
            s.bounds = None;
        }
    }
}

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
        // This view's popups record where they are painted in this frame.
        let slot = self.slot;
        self.a11y_popups.borrow_mut().retain(|(s, _), _| *s != slot);
        let probe = |part: PopupFrame| crate::painted::probe(&self.a11y_popups, (slot, part));
        // To the right of the conflict header: "Accept Current | Accept Incoming | Accept Both" (like VS Code's CodeLens).
        for (ix, conflict) in self.conflicts.iter().enumerate() {
            let head = self.buffer.line_to_char(conflict.start);
            let Some(v) = self.visible.iter().find(|v| v.line == conflict.start && v.start == head) else { continue };
            let x = v.text_left + v.layout.width + px(theme::SPACE[8]) - self.surface_origin.x;
            let y = v.bounds.top() - self.surface_origin.y;
            let at = t("Conflict at line {}").replace("{}", &(conflict.start + 1).to_string());
            let link = |id: &'static str, label: &'static str, choice: crate::conflict::Choice| {
                div()
                    .id((id, ix))
                    .relative()
                    .child(crate::a11y_controls::control(
                        crate::a11y_controls::Control::new(self.name.to_string(), format!("{id}{ix}"), accesskit::Role::Link, label)
                            .description(at.clone())
                            .on_entity(cx.entity().downgrade(), move |e: &mut Self, _, cx| e.resolve_conflict(ix, choice, cx)),
                    ))
                    .cursor_pointer()
                    .text_color(c(p.text_muted))
                    .hover(|d| d.text_color(c(p.accent)).underline())
                    .child(label)
                    .on_mouse_down(
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
                            .relative()
                            .child(crate::a11y_controls::control(
                                crate::a11y_controls::Control::new(
                                    self.name.to_string(),
                                    format!("cf-merge{ix}"),
                                    accesskit::Role::Link,
                                    t("Open Merge Editor"),
                                )
                                .description(at.clone())
                                .on_entity(cx.entity().downgrade(), |_: &mut Self, _, cx| cx.emit(EditorEvent::OpenMergeEditor)),
                            ))
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
                    .child(crate::a11y_controls::control(
                        crate::a11y_controls::Control::button(self.name.to_string(), format!("http-send{line}"), t("Send Request"), |_, _| {})
                            .description(t("line {}").replace("{}", &(line + 1).to_string()))
                            .on_entity(cx.entity().downgrade(), move |_: &mut Self, _, cx| cx.emit(EditorEvent::SendRequest(line))),
                    ))
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
                    .relative()
                    .child(probe(PopupFrame::Suggestion(pos)))
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
                    // An extension's item names the extension.
                    .children(it.source.clone().map(|s| div().flex_none().text_size(px(font_size - 2.)).text_color(c(p.text_muted)).child(s)))
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
                    .child(probe(PopupFrame::Suggestions))
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
                    .child(probe(PopupFrame::Signature))
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
            let diag_rows = hover_rows(&h.diagnostics, &self.path, self.lsp_root.as_deref()).into_iter().enumerate().map(|(i, row)| {
                let line = match row {
                    HoverRow::Diagnostic { severity, text } => {
                        let (glyph, color) = match severity {
                            1 => ("circle-x", p.danger),
                            2 => ("alert", p.warning),
                            _ => ("info", p.info),
                        };
                        div().flex().gap(px(theme::SPACE[2])).child(icon(glyph, 14., color)).child(div().flex_1().child(text))
                    }
                    HoverRow::Related { text, .. } => div()
                        .pl(px(theme::SPACE[6]))
                        .text_color(c(p.text_muted))
                        .cursor_pointer()
                        .hover(|d| d.text_color(c(p.accent)))
                        .child(text)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |e, _, _, cx| {
                                cx.stop_propagation();
                                e.open_hover_row(i, cx);
                            }),
                        ),
                    HoverRow::More(text) => div().pl(px(theme::SPACE[6])).text_color(c(p.text_muted)).child(text),
                };
                line.relative().child(probe(PopupFrame::HoverLine(i)))
            });
            out.push(
                div()
                    .id("hover")
                    .absolute()
                    .child(probe(PopupFrame::Hover))
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
                    // Each extension's text under its name, after the language server's.
                    .children(h.ext.iter().map(|(ext, t)| {
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(theme::SPACE[1]))
                            .child(div().text_size(px(font_size - 2.)).text_color(c(p.text_muted)).child(ext.clone()))
                            .child(div().font_family(font.clone()).text_color(c(p.text)).child(t.clone()))
                    }))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .into_any_element(),
            );
        }
        out
    }

    /// Where the suggestion list and the first `count` suggestions of this view were painted in its last frame
    /// (physical pixels at `scale`; a suggestion scrolled out of the list has none).
    pub fn a11y_completion_frames(&self, count: usize, scale: f32) -> (Option<accesskit::Rect>, Vec<Option<accesskit::Rect>>) {
        let painted = self.a11y_popups.borrow();
        let frame = |part| painted.get(&(self.slot, part)).and_then(|b| crate::painted::physical_scaled(*b, scale));
        (frame(PopupFrame::Suggestions), (0..count).map(|i| frame(PopupFrame::Suggestion(i))).collect())
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

    /// The hover and signature popups of this view, with where they were painted in its last frame (at `scale`).
    pub fn a11y_popups(&self, scale: f32) -> PopupsA11y {
        let painted = self.a11y_popups.borrow();
        let frame = |part| painted.get(&(self.slot, part)).and_then(|b| crate::painted::physical_scaled(*b, scale));
        let hover = self.hover.as_ref().map(|h| {
            let mut lines: Vec<HoverLineA11y> = hover_rows(&h.diagnostics, &self.path, self.lsp_root.as_deref())
                .into_iter()
                .enumerate()
                .map(|(i, row)| {
                    let (text, opens) = match row {
                        HoverRow::Diagnostic { severity, text } => {
                            let kind = match severity {
                                1 => t("Error"),
                                2 => t("Warning"),
                                _ => t("Information"),
                            };
                            (format!("{kind}: {text}"), false)
                        }
                        HoverRow::Related { text, .. } => (text, true),
                        HoverRow::More(text) => (text, false),
                    };
                    HoverLineA11y { text, opens, bounds: frame(PopupFrame::HoverLine(i)) }
                })
                .collect();
            let texts = h.text.iter().map(|t| t.to_string()).chain(h.ext.iter().map(|(ext, text)| format!("{ext}: {text}")));
            lines.extend(texts.filter(|t| !t.trim().is_empty()).map(|text| HoverLineA11y { text, opens: false, bounds: None }));
            HoverA11y { lines, bounds: frame(PopupFrame::Hover) }
        });
        let signature = self.signature.as_ref().filter(|_| self.completion.is_none()).map(|(label, active)| SignatureA11y {
            label: label.clone(),
            active: active.clone().and_then(|r| label.get(r)).map(str::to_string),
            bounds: frame(PopupFrame::Signature),
        });
        PopupsA11y { hover, signature }
    }

    /// Accessibility `ShowTooltip`: the hover at the caret, as "Editor: Show Hover".
    pub fn a11y_show_hover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_hover(&ShowHover, window, cx);
    }

    /// Accessibility `HideTooltip`: the hover and the signature help close.
    pub fn a11y_hide_popups(&mut self, cx: &mut Context<Self>) {
        self.hover = None;
        self.signature = None;
        cx.notify();
    }

    /// Accessibility `Click` on hover line `i`, while it still reads `text`.
    pub fn a11y_open_hover_line(&mut self, i: usize, text: &str, cx: &mut Context<Self>) {
        if self.a11y_popups(1.).hover.and_then(|h| h.lines.into_iter().nth(i)).is_some_and(|l| l.opens && l.text == text) {
            self.open_hover_row(i, cx);
        }
    }

    #[cfg(test)]
    pub fn set_signature_for_test(&mut self, label: &str, active: Option<std::ops::Range<usize>>, cx: &mut Context<Self>) {
        self.signature = Some((label.to_string(), active));
        cx.notify();
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
