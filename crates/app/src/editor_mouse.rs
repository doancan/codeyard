//! Editor: mouse, dragging, column selection and scrollbar.

use super::*;

impl EditorView {
    /// Test windows are never active: start the loop by hand, read the cursor visibility.
    #[cfg(test)]
    pub fn blink_for_test(&mut self, active: bool, cx: &mut Context<Self>) -> bool {
        self.blink_test_active = active;
        self.update_blink(active, cx);
        self.refresh_conflicts_soon(cx);
        self.cursor_on
    }

    /// On typing, cursor movement or focus change the cursor becomes visible and the loop restarts
    /// from the beginning. If left untouched for a while it stays solid: an idle editor does not paint.
    pub(super) fn update_blink(&mut self, focused: bool, cx: &mut Context<Self>) {
        const HALF_PERIOD: std::time::Duration = std::time::Duration::from_millis(530);
        // ~5 s: every frame pins ~110 MB of transient GPU buffers on macOS; endless blinking
        // would have held idle memory at 200 MB instead of 88 MB.
        const BLINKS: usize = 10;
        let blink = focused && cx.global::<Settings>().cursor_blinking && !crate::system_prefs::reduce_motion(cx);
        let sig = (self.buffer.version(), self.buffer.cursor(), blink);
        if sig == self.blink_sig {
            return;
        }
        self.blink_sig = sig;
        self.cursor_on = true;
        self.blink_task = blink.then(|| {
            cx.spawn(async move |this, cx| {
                for i in 1..=BLINKS {
                    cx.background_executor().timer(HALF_PERIOD).await;
                    let last = i == BLINKS;
                    let alive = this.update(cx, |e, cx| {
                        e.cursor_on = last || !e.cursor_on;
                        cx.notify();
                    });
                    if alive.is_err() {
                        return;
                    }
                }
            })
        });
    }

    /// If the right click is outside the selection, move the cursor there (if inside the selection it stays, and Cut/Copy apply to it).
    pub(super) fn on_right_click(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        self.hover = None;
        self.completion = None;
        if let Some(ix) = self.index_for_position(ev.position)
            && !self.buffer.selections().iter().any(|r| r.start <= ix && ix <= r.end && !r.is_empty())
        {
            self.buffer.set_cursor(ix, false);
            cx.emit(EditorEvent::Changed);
        }
        cx.emit(EditorEvent::ContextMenu(ev.position));
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        // A click anywhere outside the open color picker: it closes.
        self.close_picker(cx);
        if let Some(ix) = self.index_for_position(ev.position) {
            if self.try_open_picker_at(ev, ix, cx) {
                return;
            }
            self.hover = None;
            self.completion = None;
            if ev.modifiers.secondary() && !ev.modifiers.alt && !ev.modifiers.shift {
                // Cmd+click (Ctrl+click): go to definition.
                self.buffer.set_cursor(ix, false);
                if !self.open_link_at(ix, cx) {
                    self.definition_at(ix, cx);
                }
                cx.notify();
                return;
            }
            if ev.modifiers.alt && ev.modifiers.shift {
                // Shift+Alt+drag: column (box) selection.
                let (line, col) = self.line_col_of(ix, Some(ev.position));
                self.column = Some((line, col, line, col));
                self.column_drag = true;
                self.selecting = true;
                self.word_drag = None;
                self.buffer.select_columns((line, col), (line, col), self.tab_width());
                self.marked = None;
                cx.emit(EditorEvent::Changed);
                cx.notify();
                return;
            }
            if ev.modifiers.alt {
                // Alt+click: add/remove a cursor (no drag selection).
                self.buffer.toggle_cursor(ix);
                self.marked = None;
                cx.emit(EditorEvent::Changed);
                cx.notify();
                return;
            }
            match ev.click_count {
                // Double click: word, triple click: line.
                2 => match self.buffer.word_at(ix) {
                    Some(w) => {
                        self.buffer.set_selection(w.clone(), false);
                        self.word_drag = Some(w);
                    }
                    None => self.buffer.set_cursor(ix, false),
                },
                n if n >= 3 => self.buffer.select_line_at(ix),
                _ => self.buffer.set_cursor(ix, ev.modifiers.shift),
            }
            if ev.click_count != 2 {
                self.word_drag = None;
            }
            self.selecting = ev.click_count == 1 || self.word_drag.is_some();
            self.marked = None;
            cx.emit(EditorEvent::Changed);
            cx.notify();
        }
    }

    /// Movement over the editor: hover only (dragging is tracked at window level).
    pub(super) fn on_mouse_move(&mut self, ev: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.selecting || self.scrollbar_drag.is_some() {
            return;
        }
        self.on_drag_move(ev, window, cx);
    }

    /// Every mouse move while dragging (outside the editor too): GPUI's element
    /// listener is only called while the mouse is over it, so the selection would break off at the edge.
    pub(super) fn on_drag_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.picker_drag_move(ev.position, ev.pressed_button == Some(MouseButton::Left), cx) {
            return;
        }
        if let Some(grab) = self.scrollbar_drag {
            if ev.pressed_button == Some(MouseButton::Left) {
                self.scrollbar_to(ev.position.y - self.surface_origin.y - grab, cx);
            } else {
                self.scrollbar_drag = None;
            }
            return;
        }
        if ev.pressed_button.is_none() {
            self.schedule_hover(ev.position, cx);
            return;
        }
        if !self.selecting || ev.pressed_button != Some(MouseButton::Left) {
            self.drag_outside = None;
            return;
        }
        // Below/above the text area: keep selecting while scrolling as fast as the overshoot.
        let (top, bottom) = (self.surface_origin.y, self.surface_origin.y + self.surface_h);
        let line_h = self.visible.first().map_or(px(20.), |v| v.bounds.size.height);
        // Half a line of slack: dragging to a partly visible line at the edge is a normal selection.
        let edge = line_h / 2.;
        let outside = if self.surface_h > px(0.) && ev.position.y > bottom + edge {
            Some(1 + ((ev.position.y - bottom - edge) / line_h) as isize)
        } else if self.surface_h > px(0.) && ev.position.y < top - edge {
            Some(-1 - ((top - edge - ev.position.y) / line_h) as isize)
        } else {
            None
        };
        if let Some(step) = outside {
            self.drag_step(step, cx);
            self.drag_outside = Some(step);
            self.start_autoscroll(cx);
            return;
        }
        self.drag_outside = None;
        if let Some(ix) = self.index_for_position(ev.position) {
            if self.column_drag {
                return self.column_to(ix, ev.position, cx);
            }
            self.extend_drag_to(ix);
            self.after_move(cx);
        }
    }

    /// (line, visual column) 0-based. If `pos` is given and `ix` is at the end of the line, when the mouse is
    /// beyond the end of the line the column extends that far (invisible whitespace; the box is still clipped).
    pub(super) fn line_col_of(&self, ix: usize, pos: Option<gpui::Point<Pixels>>) -> (usize, usize) {
        let line = self.buffer.char_to_line(ix);
        let col = ix - self.buffer.line_to_char(line);
        let mut v = self.buffer.visual_col(line, col, self.tab_width());
        if let Some(pos) = pos
            && col >= self.buffer.line_len_chars(line)
            && let Some((_, x_end)) = self.screen_pos(ix)
        {
            let char_w = self.layout_params.2;
            if char_w > px(0.) && pos.x > x_end {
                v += ((pos.x - x_end) / char_w).round() as usize;
            }
        }
        (line, v)
    }

    /// Extend the column selection to the dragged position.
    pub(super) fn column_to(&mut self, ix: usize, pos: gpui::Point<Pixels>, cx: &mut Context<Self>) {
        let (line, col) = self.line_col_of(ix, Some(pos));
        if let Some((al, ac, ..)) = self.column {
            self.column = Some((al, ac, line, col));
            self.buffer.select_columns((al, ac), (line, col), self.tab_width());
            self.after_move_keep_column(cx);
        }
    }

    /// Shift+Alt+Cmd + arrow: grow or shrink the column selection by one line / column.
    pub(super) fn column_step(&mut self, drow: isize, dcol: isize, cx: &mut Context<Self>) {
        let tab = self.tab_width();
        let (al, ac, hl, hc) = self.column.unwrap_or_else(|| {
            let (l, c) = self.line_col_of(self.buffer.cursor(), None);
            (l, c, l, c)
        });
        let last = self.buffer.len_lines().saturating_sub(1);
        let al = al.min(last);
        let hl = (hl.min(last) as isize + drow).clamp(0, last as isize) as usize;
        // The column cannot exceed the widest line in the box (no invisible whitespace).
        let widest = (al.min(hl)..=al.max(hl)).map(|l| self.buffer.visual_col(l, self.buffer.line_len_chars(l), tab)).max().unwrap_or(0);
        let hc = (hc as isize + dcol).clamp(0, widest as isize) as usize;
        self.column = Some((al, ac, hl, hc));
        self.buffer.select_columns((al, ac), (hl, hc), tab);
        self.after_move_keep_column(cx);
    }

    pub(super) fn column_select_up(&mut self, _: &ColumnSelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.column_step(-1, 0, cx);
    }

    pub(super) fn column_select_down(&mut self, _: &ColumnSelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.column_step(1, 0, cx);
    }

    pub(super) fn column_select_left(&mut self, _: &ColumnSelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.column_step(0, -1, cx);
    }

    pub(super) fn column_select_right(&mut self, _: &ColumnSelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.column_step(0, 1, cx);
    }

    /// Extend the dragged selection to `ix` (to word boundaries if it started with a double click).
    pub(super) fn extend_drag_to(&mut self, ix: usize) {
        match self.word_drag.clone() {
            Some(anchor) => {
                let w = self.buffer.word_at(ix).unwrap_or(ix..ix);
                if w.start < anchor.start {
                    self.buffer.set_selection(w.start..anchor.end, true);
                } else {
                    self.buffer.set_selection(anchor.start..w.end.max(anchor.end), false);
                }
            }
            None => self.buffer.set_cursor(ix, true),
        }
    }

    /// One step while dragging outside: the selection end moves by `step` screen lines, the view follows.
    pub(super) fn drag_step(&mut self, step: isize, cx: &mut Context<Self>) {
        self.move_rows(step, true);
        if self.word_drag.is_some() {
            let ix = self.buffer.cursor();
            self.extend_drag_to(ix);
        }
        self.after_move(cx);
    }

    /// Keep scrolling even if the mouse is outside and not moving (like VS Code).
    pub(super) fn start_autoscroll(&mut self, cx: &mut Context<Self>) {
        if self.autoscroll_task.is_some() {
            return;
        }
        self.autoscroll_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(60)).await;
                let go = this
                    .update(cx, |e, cx| match (e.selecting, e.drag_outside) {
                        (true, Some(step)) => {
                            e.drag_step(step, cx);
                            true
                        }
                        _ => false,
                    })
                    .unwrap_or(false);
                if !go {
                    break;
                }
            }
            this.update(cx, |e, _| e.autoscroll_task = None).ok();
        }));
    }

    pub(super) fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.finish_picker_drag(cx);
        self.selecting = false;
        self.column_drag = false;
        self.drag_outside = None;
        self.word_drag = None;
        if self.scrollbar_drag.take().is_some() {
            cx.notify();
        }
    }

    /// Scrollbar geometry: (content height, visible height, thumb
    /// length, thumb top). `None` if the content fits.
    pub(super) fn scrollbar_geometry(&self) -> Option<(f32, f32, f32, f32)> {
        let line_h = self.visible.first().map_or(20., |v| v.bounds.size.height / px(1.));
        let content = line_h * self.display.row_count() as f32;
        let view = self.surface_h / px(1.);
        if view <= 0. || content <= view {
            return None;
        }
        let thumb = (view * view / content).max(24.);
        let offset = -self.scroll.0.borrow().base_handle.offset().y / px(1.);
        let top = (offset / (content - view) * (view - thumb)).clamp(0., view - thumb);
        Some((content, view, thumb, top))
    }

    /// Ruler marks: (position in document 0..1, color, lane: 0 left, 1 right, 2 full).
    pub(super) fn overview_marks(&self, p: &Palette) -> Vec<(f32, gpui::Rgba, u8)> {
        let rows = self.display.row_count().max(1) as f32;
        let at = |line: usize| self.display.first_row(line) as f32 / rows;
        let mut out = Vec::new();
        // Git: left lane.
        for (line, m) in self.git_marks.iter().enumerate() {
            if let Some(m) = m {
                use crate::git::LineMark;
                let color = match m {
                    LineMark::Added => p.success,
                    LineMark::Modified => p.info,
                    LineMark::DeletedAbove => p.danger,
                };
                out.push((at(line), ui::ca(color, 0.8), 0));
            }
        }
        // Conflicts: both sides, in the left lane.
        for c in &self.conflicts {
            out.push((at(c.start), ui::ca(p.success, 0.9), 0));
            out.push((at(c.sep), ui::ca(p.info, 0.9), 0));
        }
        // Find matches (one per line if there are many): right lane.
        let mut last = usize::MAX;
        for m in &self.matches {
            let line = self.buffer.char_to_line(m.start);
            if line != last {
                out.push((at(line), ui::ca(p.warning, 0.8), 1));
                last = line;
            }
        }
        // Diagnostics: right lane (added last so errors are painted on top).
        let mut diags: Vec<_> = self.diagnostics.iter().filter(|d| d.severity <= 2).collect();
        diags.sort_by_key(|d| std::cmp::Reverse(d.severity));
        for d in diags {
            let color = if d.severity == 1 { p.danger } else { p.warning };
            out.push((at(d.start.0 as usize), c(color), 1));
        }
        // Cursor line: a thin line at full width.
        out.push((at(self.buffer.char_to_line(self.buffer.cursor())), ui::ca(p.text, 0.6), 2));
        out
    }

    /// Bring the thumb top to `y` (relative to the surface).
    pub(super) fn scrollbar_to(&mut self, y: Pixels, cx: &mut Context<Self>) {
        let Some((content, view, thumb, _)) = self.scrollbar_geometry() else { return };
        let frac = ((y / px(1.)) / (view - thumb)).clamp(0., 1.);
        let handle = self.scroll.0.borrow().base_handle.clone();
        let x = handle.offset().x;
        handle.set_offset(point(x, px(-(frac * (content - view)))));
        cx.notify();
    }

    /// Scrollbar pressed: hold it if on the thumb, otherwise jump there and hold.
    pub(super) fn on_scrollbar_down(&mut self, ev: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let Some((_, _, thumb, top)) = self.scrollbar_geometry() else { return };
        let y = ev.position.y - self.surface_origin.y;
        let grab = if y >= px(top) && y <= px(top + thumb) { y - px(top) } else { px(thumb / 2.) };
        self.scrollbar_drag = Some(grab);
        self.scrollbar_to(y - grab, cx);
    }

    pub(super) fn banner(&self, p: &Palette, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if self.conflict {
            return Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[5]))
                    .px(px(theme::SPACE[6]))
                    .py(px(theme::SPACE[3]))
                    .bg(c(p.surface_raised))
                    .border_b_1()
                    .border_color(c(p.warning))
                    .font_family(ui::UI_FONT)
                    .text_size(px(text::SM.0))
                    .child(icon("alert", 16., p.warning))
                    .child(div().flex_1().child(crate::i18n::t("The file changed on disk while you were editing. Nothing was saved.")))
                    .child(button("overwrite", crate::i18n::t("Overwrite"), true, p, cx.on(|this, _, _, cx| this.write_to_disk(true, cx))))
                    .child(button("reload", crate::i18n::t("Discard mine, load from disk"), false, p, cx.on(|this, _, _, cx| this.reload(cx))))
                    .into_any_element(),
            );
        }
        if self.deleted_on_disk {
            return Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(theme::SPACE[5]))
                    .px(px(theme::SPACE[6]))
                    .py(px(theme::SPACE[3]))
                    .bg(c(p.surface_raised))
                    .border_b_1()
                    .border_color(c(p.warning))
                    .font_family(ui::UI_FONT)
                    .text_size(px(text::SM.0))
                    .child(icon("alert", 16., p.warning))
                    .child(div().flex_1().child(crate::i18n::t("This file was deleted on disk. Saving will create it again.")))
                    .into_any_element(),
            );
        }
        self.error.clone().map(|e| {
            div()
                .flex()
                .items_center()
                .gap(px(theme::SPACE[5]))
                .px(px(theme::SPACE[6]))
                .py(px(theme::SPACE[3]))
                .bg(c(p.surface_raised))
                .border_b_1()
                .border_color(c(p.danger))
                .font_family(ui::UI_FONT)
                .text_size(px(text::SM.0))
                .child(icon("circle-x", 16., p.danger))
                .child(div().flex_1().child(e))
                .child(button("retry", crate::i18n::t("Try again"), true, p, cx.on(|this, _, _, cx| this.write_to_disk(false, cx))))
                .into_any_element()
        })
    }
}
