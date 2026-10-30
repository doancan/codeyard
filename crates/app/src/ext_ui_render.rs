//! Workbench: drawing extension views (docs/extensions/reference/ui.md). Every element is drawn from the theme on each
//! frame: tones are theme colours and spacing steps are the theme's spacing scale, so a theme change redraws a view
//! without asking its extension. Lists, tables and trees are `uniform_list`s that build only the rows on screen.
//! Everything stays inside the view's frame (no element floats or overlays), and the frame names the publisher.
//! State and events are in `workbench_ext_views.rs`.

use ext_manifest::ViewLocation;
use ext_ui::{Element, ImageSource, InputState, Key, KeyPath, Kind, Node, PageToken, Selection, Space, Tone};
use gpui::{ElementId, KeyDownEvent, uniform_list};

use super::ext_views_glue::{HostedView, content_hash};
use super::*;

/// A spacing step (0–4) on the theme's spacing scale.
pub(super) fn space(s: Space) -> f32 {
    theme::SPACE[[0, 2, 4, 5, 6][s.0.min(Space::MAX) as usize]]
}

/// The theme colour of a tone.
pub(super) fn tone_color(t: Tone, p: &Palette) -> u32 {
    match t {
        Tone::Neutral => p.text,
        Tone::Accent => p.accent,
        Tone::Success => p.success,
        Tone::Warning => p.warning,
        Tone::Danger => p.danger,
        Tone::Muted => p.text_muted,
    }
}

/// An extension's icon: a product icon by name, or a neutral placeholder for a name this version does not have.
fn ext_icon(name: &str, size: f32, color: u32) -> gpui::Svg {
    icon(if ui::has_icon(name) { name } else { "circle-dashed" }, size, color)
}

/// One row of a tree as drawn: an item (`at`: child indices from the tree) at `depth`, or the "Load more" row of a level
/// (`more`: the item whose level it is, none for the top, and the page token).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct TreeRow {
    pub(super) depth: usize,
    pub(super) at: Vec<u32>,
    pub(super) more: Option<(Option<Key>, PageToken)>,
}

/// The rows of a tree on screen: items of expanded levels in order, each loaded level followed by its "Load more" row
/// when it has a `more` token.
pub(super) fn tree_rows(tree: &Node, st: Option<&InputState>) -> Vec<TreeRow> {
    fn walk(node: &Node, at: &mut Vec<u32>, depth: usize, item: Option<&Key>, st: Option<&InputState>, out: &mut Vec<TreeRow>) {
        for (i, c) in node.children.iter().enumerate() {
            at.push(i as u32);
            out.push(TreeRow { depth, at: at.clone(), more: None });
            let open = c.key.as_ref().is_some_and(|k| st.is_some_and(|s| s.expanded.contains(k)));
            if open && !c.children.is_empty() {
                walk(c, at, depth + 1, c.key.as_ref(), st, out);
            }
            at.pop();
        }
        if let Element::Tree { more: Some(page), .. } = &node.element
            && (depth == 0 || !node.children.is_empty())
        {
            out.push(TreeRow { depth, at: at.clone(), more: Some((item.cloned(), page.clone())) });
        }
    }
    let mut out = Vec::new();
    walk(tree, &mut Vec::new(), 0, None, st, &mut out);
    out
}

/// The elements Tab reaches, in order: interactive elements with a key, and lists, tables and trees as one stop each
/// (their rows are reached with the arrow keys).
pub(super) fn focus_order(tree: &Node) -> Vec<KeyPath> {
    let mut out = Vec::new();
    let mut stack: Vec<(&Node, KeyPath)> = vec![(tree, Vec::new())];
    while let Some((n, mut path)) = stack.pop() {
        if let Some(k) = &n.key {
            path.push(k.clone());
            if n.kind().is_interactive() && !disabled(&n.element) {
                out.push(path.clone());
            }
        }
        if !n.kind().has_rows() {
            stack.extend(n.children.iter().rev().map(|c| (c, path.clone())));
        }
    }
    out
}

fn disabled(e: &Element) -> bool {
    matches!(
        e,
        Element::Button { disabled: true, .. }
            | Element::TextInput { disabled: true, .. }
            | Element::Checkbox { disabled: true, .. }
            | Element::Select { disabled: true, .. }
    )
}

/// The node holds a list, table or tree, so it takes the height left and lets its rows scroll.
fn holds_rows(n: &Node) -> bool {
    let mut stack = vec![n];
    while let Some(n) = stack.pop() {
        if n.kind().has_rows() {
            return true;
        }
        stack.extend(&n.children);
    }
    false
}

/// What every element of one view is drawn with.
struct Draw<'a> {
    id: &'a str,
    /// Prefix of element ids: the view's index, so two views never share an id.
    tag: usize,
    h: &'a HostedView,
    p: Palette,
}

impl Draw<'_> {
    fn eid(&self, slot: &str) -> ElementId {
        ElementId::Name(format!("xv{}:{slot}", self.tag).into())
    }
}

/// A focus ring that shows only while the element has the keyboard.
fn focus_ring<E: Styled + InteractiveElement>(e: E, p: &Palette) -> E {
    let ring = p.focus_ring;
    e.border_1().border_color(ui::ca(ring, 0.)).focus(move |s| s.border_color(c(ring)))
}

impl Workbench {
    /// The sidebar's or the bottom panel's extension view.
    pub(super) fn ext_view_in(&self, location: ViewLocation, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let id = match location {
            ViewLocation::Sidebar => self.cfg.exts.views.sidebar.clone(),
            ViewLocation::Bottom => self.cfg.exts.views.bottom.clone(),
            ViewLocation::Editor => None,
        };
        match id {
            Some(id) => self.ext_view_frame(&id, p, cx),
            None => div().into_any_element(),
        }
    }

    /// A view with its header (title and publisher) and its body, clipped to its place.
    pub(super) fn ext_view_frame(&self, id: &str, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some((tag, h)) = self.cfg.exts.views.hosted.iter().enumerate().find(|(_, h)| h.decl.id == id) else {
            return div().p(px(theme::SPACE[6])).text_color(c(p.text_muted)).child(crate::i18n::t("This view is not available.")).into_any_element();
        };
        let d = &h.decl;
        let sidebar = d.location == ViewLocation::Sidebar;
        let header = div()
            .h(px(layout::VIEW_HEADER_H))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(theme::SPACE[2]))
            .px(px(theme::SPACE[6]))
            .text_size(px(text::XXS.0))
            .text_color(c(p.text_muted))
            .child(div().min_w_0().truncate().font_weight(gpui::FontWeight::SEMIBOLD).child(if sidebar {
                crate::i18n::upper(&d.title)
            } else {
                d.title.clone()
            }))
            .child(div().flex_none().debug_selector(move || format!("ext-view-publisher-{tag}")).child(crate::i18n::tf("by {}", &[&d.publisher()])));
        let draw = Draw { id, tag, h, p: *p };
        let body: AnyElement = match (&h.error, &h.tree) {
            (Some(e), _) => {
                let view = id.to_string();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(theme::SPACE[2]))
                    .p(px(theme::SPACE[4]))
                    .child(banner(crate::i18n::tf("{0} could not draw this view: {1}", &[&d.extension, e]), Tone::Danger, p))
                    .child(
                        super::ext_view_glue::action_button("ext-view-retry", crate::i18n::t("Try Again"), p)
                            .on_click(cx.listener(move |this, _, w, cx| this.request_ext_view_render(&view, w, cx))),
                    )
                    .into_any_element()
            }
            (None, None) => div()
                .flex()
                .items_center()
                .gap(px(theme::SPACE[2]))
                .p(px(theme::SPACE[4]))
                .text_color(c(p.text_muted))
                .child(ui::spinner("ext-view-loading", 14., p.text_muted))
                .child(crate::i18n::t("Loading…"))
                .into_any_element(),
            (None, Some(tree)) => {
                let root = self.ext_node(&draw, tree, &Vec::new(), "r", &mut Vec::new(), cx);
                let grow = holds_rows(tree);
                div()
                    .id(draw.eid("body"))
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .px(px(theme::SPACE[4]))
                    .pb(px(theme::SPACE[4]))
                    .when(!grow, |d| d.overflow_y_scroll())
                    .child(root)
                    .into_any_element()
            }
        };
        #[cfg(test)]
        self.cfg.exts.views.drawn_with.set(p.bg);
        div()
            .id(draw.eid("frame"))
            .debug_selector(move || format!("ext-view-{tag}"))
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .text_size(px(text::SM.0))
            .text_color(c(p.text))
            .tab_group()
            // `views.keystroke_frame_ms` (CODEYARD_PERF): a key that reaches the view, to the frame that shows its effect.
            .capture_key_down(|_, window, _| crate::perf::view_key(window))
            .on_key_down(|event: &KeyDownEvent, window, cx| {
                let m = event.keystroke.modifiers;
                if event.keystroke.key == "tab" && !m.control && !m.alt && !m.platform && !m.function {
                    if m.shift {
                        window.focus_prev();
                    } else {
                        window.focus_next();
                    }
                    window.prevent_default();
                    cx.stop_propagation();
                }
            })
            .child(header)
            .child(body)
            .into_any_element()
    }

    /// One element and its children. `path`: the keys above it; `slot`: its place by index, for element ids.
    fn ext_node(&self, draw: &Draw, node: &Node, path: &KeyPath, slot: &str, ancestors: &mut Vec<Kind>, cx: &mut Context<Self>) -> AnyElement {
        let mut path = path.clone();
        if let Some(k) = &node.key {
            path.push(k.clone());
        }
        let p = &draw.p;
        let view = draw.id.to_string();
        let focus = draw.h.focus.get(&path).cloned();
        let children = |this: &Self, ancestors: &mut Vec<Kind>, cx: &mut Context<Self>| -> Vec<AnyElement> {
            ancestors.push(node.kind());
            let out = node.children.iter().enumerate().map(|(i, c)| this.ext_node(draw, c, &path, &format!("{slot}.{i}"), ancestors, cx)).collect();
            ancestors.pop();
            out
        };
        match &node.element {
            Element::Column { gap, pad } => div()
                .flex()
                .flex_col()
                .gap(px(space(*gap)))
                .p(px(space(*pad)))
                .when(holds_rows(node), |d| d.flex_1().min_h_0())
                .children(children(self, ancestors, cx))
                .into_any_element(),
            Element::Row { gap, pad } => div()
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(space(*gap)))
                .p(px(space(*pad)))
                .when(holds_rows(node), |d| d.flex_1().min_h_0())
                .children(children(self, ancestors, cx))
                .into_any_element(),
            Element::Text { text, tone } => div().min_w_0().text_color(c(tone_color(*tone, p))).child(text.clone()).into_any_element(),
            Element::Heading { text: t, level } => {
                let size = match level {
                    1 => text::XL,
                    2 => text::LG,
                    _ => text::BASE,
                };
                div()
                    .pt(px(theme::SPACE[2]))
                    .text_size(px(size.0))
                    .line_height(px(size.1))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(t.clone())
                    .into_any_element()
            }
            Element::Icon { icon: name, tone, .. } => ext_icon(&name.0, layout::ICON, tone_color(*tone, p)).into_any_element(),
            Element::Image { source, alt } => {
                let image = match source {
                    ImageSource::Bytes(b) => draw.h.images.get(&content_hash(b)).cloned().flatten(),
                    ImageSource::Url(u) => draw.h.remote.image(u.as_str()),
                };
                match image {
                    Some(img) => gpui::img(gpui::ImageSource::Image(img)).max_w_full().max_h(px(320.)).object_fit(gpui::ObjectFit::Contain).into_any_element(),
                    None => div()
                        .flex()
                        .items_center()
                        .gap(px(theme::SPACE[2]))
                        .text_color(c(p.text_muted))
                        .child(icon("file-image", layout::ICON, p.text_muted))
                        .child(alt.clone())
                        .into_any_element(),
                }
            }
            Element::Button { label, icon: glyph, tone, disabled } => {
                let primary = *tone == Tone::Accent;
                let (bg, fg) =
                    if primary { (p.accent, p.on_accent) } else { (p.surface_hover, if *tone == Tone::Neutral { p.text } else { tone_color(*tone, p) }) };
                let b = focus_ring(
                    div()
                        .id(draw.eid(slot))
                        .flex_none()
                        .min_h(px(26.))
                        .px(px(theme::SPACE[4]))
                        .flex()
                        .items_center()
                        .gap(px(theme::SPACE[2]))
                        .rounded(px(theme::radius::SM))
                        .bg(c(bg))
                        .text_color(c(fg))
                        .children(glyph.as_ref().map(|g| ext_icon(&g.0, 14., fg)))
                        .child(label.clone()),
                    p,
                );
                if *disabled {
                    return b.opacity(0.5).into_any_element();
                }
                let hover = if primary { ui::darken(p.accent) } else { p.surface_active };
                b.cursor_pointer()
                    .hover(move |s| s.bg(c(hover)))
                    .when_some(focus, |b, f| b.track_focus(&f))
                    .on_click(cx.listener(move |this, _, w, cx| this.ext_view_click(&view, path.clone(), w, cx)))
                    .into_any_element()
            }
            Element::TextInput { label, disabled, multiline, .. } => {
                let field = draw.h.fields.get(&path).map(|(f, _)| f.clone());
                let input = match field {
                    Some(f) if !*disabled => ui::input_box(draw.eid(slot), p, p.bg, false)
                        .when(*multiline, |d| d.h_auto().min_h(px(56.)).py(px(theme::SPACE[2])))
                        .child(f)
                        .into_any_element(),
                    _ => ui::input_box(draw.eid(slot), p, p.surface, false)
                        .opacity(0.6)
                        .child(draw.h.state.get(&path).map(|s| s.text.clone()).unwrap_or_default())
                        .into_any_element(),
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(theme::SPACE[1]))
                    .child(div().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(label.clone()))
                    .child(input)
                    .into_any_element()
            }
            Element::Checkbox { label, disabled, .. } => {
                let checked = draw.h.state.get(&path).is_some_and(|s| s.checked);
                let b = focus_ring(
                    div()
                        .id(draw.eid(slot))
                        .flex()
                        .items_center()
                        .gap(px(theme::SPACE[2]))
                        .px(px(theme::SPACE[1]))
                        .rounded(px(theme::radius::SM))
                        .child(
                            div()
                                .size(px(14.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(theme::radius::SM))
                                .border_1()
                                .border_color(c(if checked { p.accent } else { ui::input_border(p) }))
                                .when(checked, |d| d.bg(c(p.accent)).child(icon("check", 12., p.on_accent))),
                        )
                        .child(label.clone()),
                    p,
                );
                if *disabled {
                    return b.opacity(0.5).into_any_element();
                }
                b.cursor_pointer()
                    .when_some(focus, |b, f| b.track_focus(&f))
                    .on_click(cx.listener(move |this, _, w, cx| this.ext_view_toggle(&view, path.clone(), w, cx)))
                    .into_any_element()
            }
            Element::Select { label, options, disabled, .. } => {
                let chosen = draw.h.state.get(&path).and_then(|s| s.choice.clone());
                let shown = chosen.and_then(|k| options.iter().find(|o| o.key == k)).map_or_else(|| crate::i18n::t("Choose…").to_string(), |o| o.label.clone());
                let b = focus_ring(
                    div()
                        .id(draw.eid(slot))
                        .h(px(28.))
                        .px(px(theme::SPACE[3]))
                        .flex()
                        .items_center()
                        .gap(px(theme::SPACE[2]))
                        .rounded(px(theme::radius::MD))
                        .bg(c(p.bg))
                        .child(div().flex_1().min_w_0().truncate().child(shown))
                        .child(icon("chevron-down", 14., p.text_muted)),
                    p,
                );
                let b = if *disabled {
                    b.opacity(0.5)
                } else {
                    let (v1, v2, p1, p2) = (view.clone(), view, path.clone(), path);
                    b.cursor_pointer()
                        .when_some(focus, |b, f| b.track_focus(&f))
                        .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, w, cx| {
                            let at = ev.position();
                            this.ext_view_open_select(&v1, p1.clone(), gpui::point(at.x, at.y + px(12.)), w, cx)
                        }))
                        .on_key_down(cx.listener(move |this, ev: &KeyDownEvent, w, cx| {
                            let step = match ev.keystroke.key.as_str() {
                                "up" => -1,
                                "down" => 1,
                                _ => return,
                            };
                            this.ext_view_step_choice(&v2, p2.clone(), step, w, cx);
                            cx.stop_propagation();
                        }))
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(theme::SPACE[1]))
                    .child(div().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(label.clone()))
                    .child(b)
                    .into_any_element()
            }
            Element::List { .. } | Element::Table { .. } | Element::Tree { .. } => self.ext_rows(draw, node, path, slot, ancestors, focus, cx),
            Element::Badge { text: t, tone } => {
                let color = tone_color(*tone, p);
                div()
                    .flex_none()
                    .px(px(theme::SPACE[2]))
                    .rounded(px(theme::radius::LG))
                    .bg(c(ui::tinted_bg(p.surface, color, 0.18, p.text)))
                    .text_color(c(if *tone == Tone::Neutral { p.text } else { color }))
                    .text_size(px(text::BADGE.0))
                    .line_height(px(text::BADGE.1))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(t.clone())
                    .into_any_element()
            }
            Element::Progress { label, percent } => {
                let bar = match percent {
                    Some(pc) => div()
                        .h(px(4.))
                        .w_full()
                        .rounded(px(2.))
                        .bg(c(p.surface_active))
                        .child(div().h_full().rounded(px(2.)).bg(c(p.accent)).w(gpui::relative(f32::from(*pc) / 100.)))
                        .into_any_element(),
                    None => ui::activity_bars(draw.eid(slot), 14., p.accent),
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(theme::SPACE[1]))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_size(px(text::XS.0))
                            .child(label.clone())
                            .children(percent.map(|pc| div().text_color(c(p.text_muted)).child(format!("{pc}%")))),
                    )
                    .child(bar)
                    .into_any_element()
            }
            Element::Markdown { source } => {
                let blocks = draw.h.markdown.get(&content_hash(source)).cloned().unwrap_or_default();
                let link = cx.entity().downgrade();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(theme::SPACE[2]))
                    .children(crate::markdown::untrusted_document_view(&blocks, &format!("xv{}:{slot}:md", draw.tag), p, move |url, window, cx| {
                        link.update(cx, |this, cx| this.confirm_open_link(url.to_string(), window, cx)).ok();
                    }))
                    .into_any_element()
            }
            Element::Code { source, .. } => div()
                .id(draw.eid(slot))
                .overflow_x_scroll()
                .p(px(theme::SPACE[3]))
                .rounded(px(theme::radius::SM))
                .bg(c(p.surface))
                .font_family(ui::MONO_FONT)
                .text_size(px(text::CODE.0))
                .line_height(px(text::CODE.1))
                .children(source.lines().map(|l| div().whitespace_nowrap().child(if l.is_empty() { " ".to_string() } else { l.to_string() })))
                .into_any_element(),
            Element::KeyValue { pairs } => div()
                .flex()
                .flex_col()
                .gap(px(theme::SPACE[1]))
                .children(pairs.iter().map(|pair| {
                    div()
                        .flex()
                        .gap(px(theme::SPACE[3]))
                        .child(div().w(px(120.)).flex_none().truncate().text_color(c(p.text_muted)).child(pair.name.clone()))
                        .child(div().flex_1().min_w_0().child(pair.value.clone()))
                }))
                .into_any_element(),
            Element::Banner { text: t, tone } => banner(t.clone(), *tone, p).into_any_element(),
            Element::Link { label, url } => {
                let tip = url.as_ref().map(|u| u.as_str().to_string());
                focus_ring(div().id(draw.eid(slot)).rounded(px(theme::radius::SM)), p)
                    .flex_none()
                    .cursor_pointer()
                    .text_color(c(p.accent))
                    .hover(|s| s.underline())
                    .child(label.clone())
                    .when_some(focus, |b, f| b.track_focus(&f))
                    .when_some(tip, |b, tip| b.tooltip(move |_, cx| cx.new(|_| Tooltip(tip.clone().into())).into()))
                    .on_click(cx.listener(move |this, _, w, cx| this.ext_view_link(&view, path.clone(), w, cx)))
                    .into_any_element()
            }
            Element::Divider => div().h(px(1.)).w_full().flex_none().bg(c(p.border)).into_any_element(),
            Element::EmptyState { title, text: t, icon: glyph } => div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(theme::SPACE[3]))
                .py(px(theme::SPACE[7]))
                .px(px(theme::SPACE[4]))
                .text_center()
                .children(glyph.as_ref().map(|g| ext_icon(&g.0, 32., p.text_muted)))
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(title.clone()))
                .when(!t.is_empty(), |d| d.child(div().text_color(c(p.text_muted)).child(t.clone())))
                .child(div().flex().flex_wrap().justify_center().gap(px(theme::SPACE[2])).children(children(self, ancestors, cx)))
                .into_any_element(),
        }
    }

    /// A list, table or tree: a header for a table, then only the rows on screen. The container takes the keyboard:
    /// arrows move between rows, Space and Enter select, Right and Left expand and collapse tree items.
    #[allow(clippy::too_many_arguments)]
    fn ext_rows(
        &self,
        draw: &Draw,
        node: &Node,
        path: KeyPath,
        slot: &str,
        ancestors: &[Kind],
        focus: Option<FocusHandle>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = &draw.p;
        let kind = node.kind();
        let st = draw.h.state.get(&path);
        let count = match kind {
            Kind::Tree => tree_rows(node, st).len(),
            _ => node.children.len(),
        };
        let header = match &node.element {
            Element::Table { columns, .. } => Some(
                div()
                    .flex()
                    .flex_none()
                    .h(px(layout::ROW_H))
                    .items_center()
                    .gap(px(theme::SPACE[3]))
                    .px(px(theme::SPACE[2]))
                    .border_b_1()
                    .border_color(c(p.border))
                    .text_size(px(text::XS.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(c(p.text_muted))
                    .children(columns.iter().map(|h| div().flex_1().min_w_0().truncate().child(h.clone()))),
            ),
            _ => None,
        };
        let (view, rows_path) = (draw.id.to_string(), path.clone());
        let mut ancestors = ancestors.to_vec();
        ancestors.push(kind);
        let tag = draw.tag;
        let list_slot = slot.to_string();
        let list = uniform_list(
            draw.eid(&format!("{slot}:rows")),
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                let n = range.len();
                let rows = this.ext_row_range(&view, tag, &rows_path, &list_slot, &ancestors, range, window, cx);
                this.cfg.exts.views.built_rows.insert(rows_path.clone(), n);
                rows
            }),
        )
        .when_some(draw.h.scroll.get(&path), |l, s| l.track_scroll(s.clone()))
        .flex_1()
        .min_h(px(layout::ROW_H * (count.clamp(1, 3) as f32)));
        let (v1, p1) = (draw.id.to_string(), path.clone());
        focus_ring(div().id(draw.eid(&format!("{slot}:box"))), p)
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .rounded(px(theme::radius::SM))
            .when_some(focus, |d, f| d.track_focus(&f))
            .on_key_down(cx.listener(move |this, ev: &KeyDownEvent, w, cx| {
                if this.ext_rows_key(&v1, &p1, &ev.keystroke, w, cx) {
                    cx.stop_propagation();
                }
            }))
            .children(header)
            .child(list)
            .into_any_element()
    }

    /// Rows `range` of the list, table or tree at `path`.
    #[allow(clippy::too_many_arguments)]
    fn ext_row_range(
        &self,
        id: &str,
        tag: usize,
        path: &KeyPath,
        slot: &str,
        ancestors: &[Kind],
        range: std::ops::Range<usize>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(h) = self.ext_view(id) else { return Vec::new() };
        let Some(node) = h.find(path) else { return Vec::new() };
        let draw = Draw { id, tag, h, p: *palette(cx) };
        let p = draw.p;
        let st = h.state.get(path);
        let focused = h.focus.get(path).is_some_and(|f| f.is_focused(window));
        let cursor = h.cursor.get(path).copied().unwrap_or(0);
        let selectable = !matches!(
            node.element,
            Element::List { selection: Selection::None, .. }
                | Element::Table { selection: Selection::None, .. }
                | Element::Tree { selection: Selection::None, .. }
        );
        let tree = (node.kind() == Kind::Tree).then(|| tree_rows(node, st));
        let mut out = Vec::with_capacity(range.len());
        for ix in range {
            let row_base = |selected: bool| {
                div()
                    .id(ElementId::Name(format!("xv{tag}:{slot}:row{ix}").into()))
                    .h(px(layout::ROW_H))
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[2]))
                    .px(px(theme::SPACE[2]))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .when(selected, |d| d.bg(c(if focused { ui::focused_selection(&p) } else { p.surface_active })))
                    .when(!selected, |d| d.hover(|d| d.bg(c(p.surface_hover))))
                    .when(focused && ix == cursor, |d| d.border_1().border_color(c(p.focus_ring)))
            };
            let mut anc = ancestors.to_vec();
            let el = match &tree {
                None => {
                    let Some(row) = node.children.get(ix) else { continue };
                    let key = row.key.clone().unwrap_or_else(|| Key::new(""));
                    let selected = st.is_some_and(|s| s.selected.contains(&key));
                    let cells: Vec<AnyElement> = if node.kind() == Kind::Table {
                        anc.push(Kind::Row);
                        let mut rp = path.clone();
                        rp.push(key.clone());
                        row.children
                            .iter()
                            .enumerate()
                            .map(|(i, cell)| {
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .child(self.ext_node(&draw, cell, &rp, &format!("{slot}:{ix}.{i}"), &mut anc, cx))
                                    .into_any_element()
                            })
                            .collect()
                    } else {
                        vec![self.ext_node(&draw, row, path, &format!("{slot}:{ix}"), &mut anc, cx)]
                    };
                    let (view, lp) = (id.to_string(), path.clone());
                    row_base(selected)
                        .gap(px(theme::SPACE[3]))
                        .children(cells)
                        .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, w, cx| {
                            this.ext_view_move_cursor(&view, &lp, ix, cx);
                            if let Some(f) = this.ext_view(&view).and_then(|h| h.focus.get(&lp)).cloned() {
                                w.focus(&f);
                            }
                            if selectable {
                                let m = ev.modifiers();
                                this.ext_view_select_row(&view, lp.clone(), key.clone(), m.platform || m.control, w, cx);
                            }
                        }))
                        .into_any_element()
                }
                Some(rows) => {
                    let Some(r) = rows.get(ix) else { continue };
                    let indent = px(theme::SPACE[2] + layout::TREE_INDENT * r.depth as f32);
                    let (view, tp) = (id.to_string(), path.clone());
                    match &r.more {
                        Some((item, page)) => {
                            let (item, page) = (item.clone(), page.clone());
                            row_base(false)
                                .pl(indent + px(layout::TREE_INDENT))
                                .text_color(c(p.accent))
                                .child(crate::i18n::t("Load more"))
                                .on_click(cx.listener(move |this, _, w, cx| {
                                    this.ext_view_move_cursor(&view, &tp, ix, cx);
                                    this.ext_view_load_more(&view, tp.clone(), item.clone(), page.clone(), w, cx)
                                }))
                                .into_any_element()
                        }
                        None => {
                            let Some(item) = node.get(&r.at) else { continue };
                            let Element::Tree { label, description, icon: glyph, more, .. } = &item.element else { continue };
                            let key = item.key.clone().unwrap_or_else(|| Key::new(""));
                            let expandable = !item.children.is_empty() || more.is_some();
                            let open = st.is_some_and(|s| s.expanded.contains(&key));
                            let selected = st.is_some_and(|s| s.selected.contains(&key));
                            let (v2, tp2, k2) = (view.clone(), tp.clone(), key.clone());
                            row_base(selected)
                                .pl(indent)
                                .child(div().id(ElementId::Name(format!("xv{tag}:{slot}:tw{ix}").into())).size(px(layout::ICON)).flex_none().when(
                                    expandable,
                                    |d| {
                                        d.child(icon(if open { "chevron-down" } else { "chevron-right" }, 14., p.text_muted)).on_click(cx.listener(
                                            move |this, _, w, cx| {
                                                cx.stop_propagation();
                                                this.ext_view_expand(&v2, tp2.clone(), k2.clone(), !open, w, cx)
                                            },
                                        ))
                                    },
                                ))
                                .children(glyph.as_ref().map(|g| ext_icon(&g.0, layout::ICON, p.text_muted)))
                                .child(div().min_w_0().truncate().child(label.clone()))
                                .children(
                                    description.clone().map(|d| div().min_w_0().truncate().text_size(px(text::XS.0)).text_color(c(p.text_muted)).child(d)),
                                )
                                .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, w, cx| {
                                    this.ext_view_move_cursor(&view, &tp, ix, cx);
                                    if let Some(f) = this.ext_view(&view).and_then(|h| h.focus.get(&tp)).cloned() {
                                        w.focus(&f);
                                    }
                                    if selectable {
                                        let m = ev.modifiers();
                                        this.ext_view_select_row(&view, tp.clone(), key.clone(), m.platform || m.control, w, cx);
                                    } else if expandable {
                                        this.ext_view_expand(&view, tp.clone(), key.clone(), !open, w, cx);
                                    }
                                }))
                                .into_any_element()
                        }
                    }
                }
            };
            out.push(el);
        }
        out
    }

    /// A key on a focused list, table or tree. True when it was used.
    fn ext_rows_key(&mut self, id: &str, path: &KeyPath, key: &gpui::Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(h) = self.ext_view(id) else { return false };
        let Some(node) = h.find(path) else { return false };
        let st = h.state.get(path);
        let tree = (node.kind() == Kind::Tree).then(|| tree_rows(node, st));
        let count = tree.as_ref().map_or(node.children.len(), Vec::len);
        if count == 0 {
            return false;
        }
        let cursor = h.cursor.get(path).copied().unwrap_or(0).min(count - 1);
        let page = 10;
        let to = match key.key.as_str() {
            "up" => Some(cursor.saturating_sub(1)),
            "down" => Some((cursor + 1).min(count - 1)),
            "home" => Some(0),
            "end" => Some(count - 1),
            "pageup" => Some(cursor.saturating_sub(page)),
            "pagedown" => Some((cursor + page).min(count - 1)),
            _ => None,
        };
        if let Some(to) = to {
            self.ext_view_move_cursor(id, path, to, cx);
            return true;
        }
        let toggle = key.modifiers.platform || key.modifiers.control;
        match (key.key.as_str(), &tree) {
            ("space" | "enter", None) => {
                let Some(k) = node.children.get(cursor).and_then(|r| r.key.clone()) else { return false };
                self.ext_view_select_row(id, path.clone(), k, toggle || key.key == "space", window, cx);
                true
            }
            (k, Some(rows)) => {
                let Some(r) = rows.get(cursor) else { return false };
                if let Some((item, page)) = &r.more {
                    if k == "enter" || k == "space" {
                        self.ext_view_load_more(id, path.clone(), item.clone(), page.clone(), window, cx);
                        return true;
                    }
                    return false;
                }
                let Some(item) = node.get(&r.at) else { return false };
                let Some(ik) = item.key.clone() else { return false };
                let open = st.is_some_and(|s| s.expanded.contains(&ik));
                match k {
                    "right" if !open => self.ext_view_expand(id, path.clone(), ik, true, window, cx),
                    "right" => self.ext_view_move_cursor(id, path, (cursor + 1).min(count - 1), cx),
                    "left" if open => self.ext_view_expand(id, path.clone(), ik, false, window, cx),
                    "left" => {
                        // To the parent item: the nearest row above with a smaller depth.
                        if let Some(up) = rows[..cursor].iter().rposition(|x| x.more.is_none() && x.depth < r.depth) {
                            self.ext_view_move_cursor(id, path, up, cx);
                        }
                    }
                    "space" | "enter" => self.ext_view_select_row(id, path.clone(), ik, toggle || k == "space", window, cx),
                    _ => return false,
                }
                true
            }
            _ => false,
        }
    }
}

/// A line of text on a tinted background with the tone's icon.
fn banner(t: String, tone: Tone, p: &Palette) -> gpui::Div {
    let color = tone_color(tone, p);
    let glyph = match tone {
        Tone::Danger => "circle-x",
        Tone::Warning => "alert",
        Tone::Success => "circle-check",
        _ => "info",
    };
    div()
        .flex()
        .items_start()
        .gap(px(theme::SPACE[2]))
        .px(px(theme::SPACE[3]))
        .py(px(theme::SPACE[2]))
        .rounded(px(theme::radius::SM))
        .bg(c(ui::tinted_bg(p.surface, color, 0.14, p.text)))
        .border_l_2()
        .border_color(c(color))
        .child(icon(glyph, 14., color))
        .child(div().flex_1().min_w_0().child(t))
}
