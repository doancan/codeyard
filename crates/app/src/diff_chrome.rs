//! What surrounds the diff's rows: the header (change counter, buttons), the side titles, notices, the fold rows and the
//! overview ruler at the right edge.

use std::time::Duration;

use gpui::{
    AnyElement, ClipboardItem, Context, MouseButton, MouseDownEvent, MouseMoveEvent, SharedString, Stateful, Window, div, point, prelude::*, px, relative,
};

use crate::a11y_controls::{self, Control};
use crate::diff::fold::Item;
use crate::diff::guard::{self, Unsupported};
use crate::diff::ruler::{self, Kind};
use crate::diff::whitespace::Whitespace;
use crate::diff::{DiffView, PICK_W, RowStyle, ToggleCollapseUnchanged, ToggleIgnoreWhitespace, ToggleInline};
use crate::i18n::{t, tf, tn};
use crate::ui::{self, c, ca, icon, text};

/// Width of the overview ruler.
const RULER_W: f32 = 14.;
/// A marker is never thinner than this, so a one-line change in a long file can be seen and hit.
const MARKER_MIN_H: f32 = 2.;
/// How long the copy button says "Copied".
const COPIED_FOR: Duration = Duration::from_millis(1500);

fn tooltip(text: String) -> impl Fn(&mut Window, &mut gpui::App) -> gpui::AnyView + 'static {
    let text: SharedString = text.into();
    move |_, cx| cx.new(|_| crate::workbench::Tooltip(text.clone())).into()
}

/// A shortcut as the platform writes it.
fn chord(mac: &'static str, other: &'static str) -> &'static str {
    if cfg!(target_os = "macos") { mac } else { other }
}

fn with_keys(label: &str, keys: &str) -> String {
    tf("{} ({})", &[&label, &keys])
}

impl DiffView {
    /// A small icon button of the header: `toggled` is the state of a toggle button (None: not a toggle); `enabled` false dims it and ignores clicks.
    #[allow(clippy::too_many_arguments)]
    fn tool_button(
        &self,
        id: &'static str,
        glyph: &'static str,
        label: &str,
        tip: String,
        (toggled, enabled): (Option<bool>, bool),
        p: ui::Palette,
        cx: &mut Context<Self>,
        f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<gpui::Div> {
        let el = div()
            .id(id)
            .size(px(22.))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .rounded(px(theme::radius::SM))
            .when(toggled == Some(true), |d| d.bg(c(p.surface_active)))
            .when(enabled, |d| d.cursor_pointer().hover(|d| d.bg(c(p.surface_hover))))
            .when(!enabled, |d| d.opacity(0.4))
            .child(icon(glyph, 14., if toggled == Some(true) { p.text } else { p.text_muted }))
            .tooltip(tooltip(tip));
        let mut control = Control::button(self.path.clone(), id, label, |_, _| {});
        control.disabled = !enabled;
        if let Some(on) = toggled {
            control = control.toggled(on);
        }
        a11y_controls::press_on(el, control, cx, move |this, w, cx| {
            if enabled {
                f(this, w, cx)
            }
        })
    }

    fn separator(p: ui::Palette) -> gpui::Div {
        div().w(px(1.)).h(px(14.)).flex_none().bg(c(p.border))
    }

    /// The header: file, comparison, staging status, change counter with its buttons, stats and the view options.
    pub(crate) fn header(&self, status: Option<AnyElement>, p: ui::Palette, cx: &mut Context<Self>) -> gpui::Div {
        let (added, removed) = self.stats;
        let changes = self.hunks.len();
        let at = self.current.min(changes.saturating_sub(1));
        let counter = if changes == 0 { String::new() } else { tf("{} of {}", &[&(at + 1), &changes]) };
        let summary = format!(
            "{} · {} · {}",
            self.label,
            tf("{} added", &[&added]),
            if self.hunks.is_empty() { t("No changes").to_string() } else { tf("{} removed", &[&removed]) }
        );
        let prev_keys = chord("⇧F7", "Shift+F7");
        let has_changes = changes > 0;
        let copy_label = t("Copy path");
        let (ws_text, ws_tip) = match self.ws {
            Whitespace::Exact => (t("Whitespace"), t("Whitespace differences are shown. Click to ignore trailing whitespace")),
            Whitespace::Trailing => (t("Ignore trailing"), t("Trailing whitespace is ignored. Click to ignore all whitespace")),
            Whitespace::All => (t("Ignore all"), t("All whitespace is ignored. Click to show whitespace differences")),
        };
        let inline_label = if self.inline { t("Inline") } else { t("Side by side") };
        let pill = |id: &'static str| {
            div()
                .id(id)
                .relative()
                .flex_none()
                .h(px(22.))
                .flex()
                .items_center()
                .px(px(theme::SPACE[3]))
                .rounded(px(theme::radius::MD))
                .cursor_pointer()
                .hover(|s| s.bg(c(p.surface_hover)))
        };
        let ws_pill = a11y_controls::press_on(
            pill("diff-whitespace")
                .when(self.ws.ignores(), |d| d.bg(c(p.surface_active)))
                .text_color(c(if self.ws.ignores() { p.text } else { p.text_muted }))
                .child(ws_text)
                .tooltip(tooltip(with_keys(ws_tip, chord("⌘K ⌘H", "Ctrl+K Ctrl+H")))),
            Control::button(self.path.clone(), "whitespace", t("Ignore whitespace"), |_, _| {}).description(ws_text).toggled(self.ws.ignores()),
            cx,
            |this, w, cx| this.toggle_whitespace(&ToggleIgnoreWhitespace, w, cx),
        );
        let inline_pill = a11y_controls::press_on(
            pill("diff-inline-toggle")
                .text_color(c(p.text))
                .child(inline_label)
                .tooltip(tooltip(with_keys(t("Toggle inline view"), chord("⌘K ⌘I", "Ctrl+K Ctrl+I")))),
            Control::button(self.path.clone(), "inline", t("Toggle inline view"), |_, _| {}).description(inline_label),
            cx,
            |this, w, cx| this.toggle_inline(&ToggleInline, w, cx),
        );
        div()
            .h(px(30.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(theme::SPACE[3]))
            .px(px(theme::SPACE[6]))
            .border_b_1()
            .border_color(c(p.border))
            .text_size(px(text::XS.0))
            .text_color(c(p.text_muted))
            .relative()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[3]))
                    .child(icon("git-compare", 14., p.text_muted))
                    .child(div().min_w_0().truncate().text_color(c(p.text)).child(self.path.clone()))
                    .child(self.tool_button(
                        "copy-path",
                        if self.copied { "check" } else { "copy" },
                        copy_label,
                        if self.copied { t("Copied").to_string() } else { copy_label.to_string() },
                        (None, true),
                        p,
                        cx,
                        |this, _, cx| this.copy_path(cx),
                    ))
                    .child(div().min_w_0().truncate().child(self.label.clone()))
                    .when_some(status, |d, status| d.child(div().min_w_0().overflow_hidden().child(status))),
            )
            .child(a11y_controls::said(self.path.clone(), "summary", accesskit::Role::Heading, summary))
            .child(Self::separator(p))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[3]))
                    .child(self.tool_button(
                        "prev-change",
                        "chevron-up",
                        t("Previous change"),
                        with_keys(t("Previous change"), prev_keys),
                        (None, has_changes),
                        p,
                        cx,
                        |this, w, cx| this.prev_change(&crate::diff::PrevChange, w, cx),
                    ))
                    .child(div().flex_none().min_w(px(40.)).flex().justify_center().text_color(c(p.text)).child(counter.clone()))
                    .child(self.tool_button(
                        "next-change",
                        "chevron-down",
                        t("Next change"),
                        with_keys(t("Next change"), "F7"),
                        (None, has_changes),
                        p,
                        cx,
                        |this, w, cx| this.next_change(&crate::diff::NextChange, w, cx),
                    ))
                    .when(has_changes, |d| {
                        d.child(a11y_controls::said(self.path.clone(), "counter", accesskit::Role::Status, tf("Change {} of {}", &[&(at + 1), &changes])))
                    })
                    .child(Self::separator(p))
                    .child(div().flex_none().text_color(c(p.success)).child(format!("+{added}")))
                    .child(div().flex_none().text_color(c(p.danger)).child(format!("−{removed}")))
                    .child(Self::separator(p))
                    .child(self.tool_button(
                        "collapse",
                        "chevrons-down-up",
                        t("Collapse unchanged lines"),
                        with_keys(t("Collapse unchanged lines"), chord("⌘K ⌘O", "Ctrl+K Ctrl+O")),
                        (Some(self.collapse), true),
                        p,
                        cx,
                        |this, w, cx| this.toggle_collapse(&ToggleCollapseUnchanged, w, cx),
                    ))
                    .child(ws_pill)
                    .child(inline_pill)
                    .when(self.hunks.is_empty() && !self.can_stage() && self.unsupported.is_none(), |d| d.child(t("No changes"))),
            )
    }

    /// The names of the two sides above the rows, each over its column.
    pub(crate) fn titles_band(&self, st: RowStyle) -> Option<gpui::Div> {
        let (left, right) = self.titles.clone().filter(|_| self.unsupported.is_none())?;
        let p = st.p;
        let dot = |color: u32| div().size(px(6.)).flex_none().rounded_full().bg(c(color));
        let band = div()
            .h(px(22.))
            .flex_none()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(c(p.border))
            .bg(c(p.surface))
            .font_family(ui::UI_FONT)
            .text_size(px(text::XS.0))
            .text_color(c(p.text_muted));
        let side = |name: SharedString, color: u32| {
            div().flex_1().min_w_0().flex().items_center().gap(px(theme::SPACE[3])).pl(px(st.num_w)).child(dot(color)).child(div().truncate().child(name))
        };
        Some(if self.inline {
            band.pl(px(theme::SPACE[6]))
                .gap(px(theme::SPACE[3]))
                .child(dot(p.danger))
                .child(left)
                .child(icon("arrow-right", 12., p.text_muted))
                .child(dot(p.success))
                .child(right)
        } else {
            band.when(self.can_stage(), |d| d.child(div().w(px(PICK_W)).flex_none()))
                .child(side(left, p.danger))
                .child(div().w(px(1.)).h_full().flex_none().bg(c(p.border)))
                .child(side(right, p.success))
        })
    }

    /// A line under the titles for what the reader should know about the texts.
    pub(crate) fn notice_strip(&self, p: ui::Palette) -> Option<gpui::Div> {
        let (glyph, message) = if self.unsupported.is_some() {
            return None;
        } else if self.invalid_bytes {
            ("alert", t("Some bytes are not valid UTF-8 and are shown as replacement characters"))
        } else if self.ws.ignores() && self.blocks.is_empty() && self.old_text != self.new_text {
            ("info", t("The two sides differ only in whitespace"))
        } else {
            return None;
        };
        let color = if glyph == "alert" { p.warning } else { p.info };
        Some(
            div()
                .h(px(24.))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(theme::SPACE[3]))
                .px(px(theme::SPACE[6]))
                .relative()
                .bg(ca(color, 0.1))
                .border_b_1()
                .border_color(c(p.border))
                .font_family(ui::UI_FONT)
                .text_size(px(text::XS.0))
                .text_color(c(p.text))
                .child(icon(glyph, 13., color))
                .child(div().truncate().child(message))
                .child(a11y_controls::said(self.path.clone(), "notice", accesskit::Role::Status, message)),
        )
    }

    /// Body message for texts that are not lined up.
    pub(crate) fn unsupported_state(&self, why: Unsupported, p: ui::Palette) -> gpui::Div {
        let message = match why {
            Unsupported::Binary => t("This file is binary, so there is no line diff to show").to_string(),
            Unsupported::TooLarge(bytes) => {
                let mb = |n: usize| format!("{:.1}", n as f64 / (1024. * 1024.));
                tf("This file is too large to compare here ({} MB; the limit is {} MB)", &[&mb(bytes), &mb(guard::MAX_BYTES)])
            }
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(theme::SPACE[4]))
            .font_family(ui::UI_FONT)
            .text_size(px(text::SM.0))
            .text_color(c(p.text_muted))
            .child(icon("file", 28., p.text_muted))
            .child(div().relative().child(message.clone()).child(a11y_controls::said(self.path.clone(), "unsupported", accesskit::Role::Label, message)))
    }

    /// "N unchanged lines": one list row that opens the fold it stands for.
    pub(crate) fn fold_row(&self, start: usize, len: usize, line_h: f32, p: ui::Palette, cx: &mut Context<Self>) -> AnyElement {
        let label = tn("{} unchanged line", "{} unchanged lines", len, &[]);
        let el = div()
            .id(SharedString::from(format!("diff-fold-{start}")))
            .w_full()
            .h(px(line_h))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(theme::SPACE[3]))
            .bg(ca(p.accent, 0.07))
            .border_y_1()
            .border_color(ca(p.border, 0.6))
            .font_family(ui::UI_FONT)
            .text_size(px(text::XS.0))
            .text_color(c(p.text_muted))
            .cursor_pointer()
            .hover(|d| d.bg(c(p.surface_hover)).text_color(c(p.text)))
            .child(icon("chevrons-up-down", 12., p.text_muted))
            .child(label.clone())
            .tooltip(tooltip(t("Show these lines").to_string()));
        a11y_controls::press_on(
            el,
            Control::button(self.path.clone(), format!("fold-{start}"), label, |_, _| {}).description(t("Show these lines")),
            cx,
            move |this, _, cx| this.expand_fold(start, cx),
        )
        .into_any_element()
    }

    pub(crate) fn copy_path(&mut self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.path.clone()));
        self.copied = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_FOR).await;
            this.update(cx, |this, cx| {
                this.copied = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The list's scroll position in px (0 at the top, negative below it). A scroll asked for but not yet laid out counts.
    fn scroll_offset(&self, line_h: f32) -> f32 {
        let state = self.scroll.0.borrow();
        match state.deferred_scroll_to_item {
            Some(d) => -(d.item_index.saturating_sub(d.offset) as f32 * line_h),
            None => f32::from(state.base_handle.offset().y),
        }
    }

    /// The viewport's height as the list last laid it out.
    fn viewport_height(&self) -> f32 {
        self.scroll.0.borrow().last_item_size.map_or(0., |s| f32::from(s.item.height))
    }

    /// Scroll to `progress` of the way from the top (0) to the end (1): what the ruler's slider says.
    pub(crate) fn scroll_to_progress(&mut self, progress: f32, cx: &mut Context<Self>) {
        let line_h = cx.global::<crate::settings::Settings>().editor_line_height;
        let offset = ruler::offset_at(progress, self.viewport_height(), self.items.len() as f32 * line_h);
        let base = self.scroll.0.borrow().base_handle.clone();
        base.set_offset(point(base.offset().x, px(offset)));
        cx.notify();
    }

    /// Scroll so that the point `fraction` (0..=1) down the list is centered.
    pub(crate) fn scroll_to_fraction(&mut self, fraction: f32, cx: &mut Context<Self>) {
        let line_h = cx.global::<crate::settings::Settings>().editor_line_height;
        let offset = ruler::offset_for(fraction, self.viewport_height(), self.items.len() as f32 * line_h);
        let base = self.scroll.0.borrow().base_handle.clone();
        base.set_offset(point(base.offset().x, px(offset)));
        cx.notify();
    }

    fn ruler_pointer(&mut self, y: gpui::Pixels, window: &Window, cx: &mut Context<Self>) {
        let Some(lane) = a11y_controls::bounds_of(cx, window, &self.path, "ruler") else { return };
        let fraction = ruler::fraction_at(f32::from(y), f32::from(lane.origin.y), f32::from(lane.size.height));
        self.scroll_to_fraction(fraction, cx);
    }

    /// The overview ruler: a marker for each change in proportion to the list, a thumb over what is on screen and the
    /// change under F7 outlined. A click or a drag scrolls there.
    pub(crate) fn ruler(&self, line_h: f32, p: ui::Palette, cx: &mut Context<Self>) -> AnyElement {
        let total = self.items.len();
        let content = total as f32 * line_h;
        let thumb = ruler::thumb(self.scroll_offset(line_h), self.viewport_height(), content);
        let marker = |items: &std::ops::Range<usize>, kind: Kind| {
            let (top, height) = ruler::fraction(items, total);
            let lane = div().absolute().left(px(2.)).right(px(2.)).top(relative(top)).h(relative(height)).min_h(px(MARKER_MIN_H));
            match kind {
                Kind::Added => lane.bg(c(p.success)),
                Kind::Removed => lane.bg(c(p.danger)),
                Kind::Modified => lane.flex().child(div().flex_1().h_full().bg(c(p.danger))).child(div().flex_1().h_full().bg(c(p.success))),
            }
        };
        let current = self.hunks.get(self.current).map(|&row| {
            let first = self.list_index(row);
            let block = self.blocks.get(self.current).cloned().unwrap_or(row..row + 1);
            let len = self.items[first..].iter().take_while(|item| matches!(item, Item::Row(ix) if block.contains(&self.src_of(*ix)))).count();
            first..first + len.max(1)
        });
        let position = ruler::progress(self.scroll_offset(line_h), self.viewport_height(), content);
        let changes = self.hunks.len();
        let heard = if changes == 0 { t("No changes").to_string() } else { tf("{} changes", &[&changes]) };
        let weak = cx.entity().downgrade();
        div()
            .id("diff-ruler")
            .relative()
            .w(px(RULER_W))
            .h_full()
            .flex_none()
            .bg(c(p.surface))
            .border_l_1()
            .border_color(c(p.border))
            .cursor_pointer()
            .children(self.ruler_spans.iter().map(|span| marker(&span.items, span.kind)))
            .children(current.map(|items| {
                let (top, height) = ruler::fraction(&items, total);
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(relative(top))
                    .h(relative(height))
                    .min_h(px(MARKER_MIN_H * 2.))
                    .border_1()
                    .border_color(c(p.accent))
                    .bg(ca(p.accent, 0.25))
            }))
            .children(thumb.map(|(top, height)| {
                div().absolute().left_0().right_0().top(relative(top)).h(relative(height)).bg(ca(p.text, 0.12)).border_y_1().border_color(ca(p.text, 0.3))
            }))
            .child(a11y_controls::control(Control::new(self.path.clone(), "ruler", accesskit::Role::Slider, t("Change overview")).description(heard).slider(
                a11y_controls::Range { value: (position * 100.) as f64, min: 0., max: 100., step: 10. },
                move |to, _, cx| {
                    weak.update(cx, |this, cx| this.scroll_to_progress(to as f32 / 100., cx)).ok();
                },
            )))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    this.ruler_drag = true;
                    this.ruler_pointer(ev.position.y, window, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, cx| {
                if this.ruler_drag && ev.dragging() {
                    this.ruler_pointer(ev.position.y, window, cx);
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.ruler_drag = false))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, _| this.ruler_drag = false))
            .into_any_element()
    }
}
