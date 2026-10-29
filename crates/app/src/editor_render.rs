//! Editor: painting (`Render`) and the line element.

use super::*;

impl Render for EditorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        RENDER_SLOT.with(|s| s.set(self.slot));
        let p = *palette(cx);
        let focused = self.focus_handle.is_focused(window);
        #[cfg(test)]
        let active = self.blink_test_active;
        #[cfg(not(test))]
        let active = focused && window.is_window_active();
        self.update_blink(active, cx);
        let lines = self.buffer.len_lines();
        let numbers_mode = cx.global::<Settings>().line_numbers.clone();
        let gutter_w = if numbers_mode == "off" {
            theme::SPACE[8] + 4.
        } else {
            lines.to_string().len().max(3) as f32 * cx.global::<Settings>().editor_font_size * 0.62 + theme::SPACE[8]
        };
        let cursor = self.buffer.cursor();
        let cursors = self.buffer.cursors();
        let selections: Vec<Range<usize>> = self.buffer.selections().into_iter().filter(|r| !r.is_empty()).collect();
        let cursor_lines: Vec<usize> = cursors.iter().map(|&c| self.buffer.char_to_line(c)).collect();
        let marked = self.marked.clone();
        let current_sel = self.buffer.selection_or_cursor();
        // With a single cursor and no selection: the word under the cursor and its bracket pair.
        let (word, brackets) = if selections.is_empty() && cursors.len() == 1 {
            let word = self.buffer.word_at(cursor).filter(|r| (2..=64).contains(&r.len())).map(|r| self.buffer.text_range(r));
            (word, cx.global::<Settings>().match_brackets.then(|| self.buffer.matching_bracket(cursor)).flatten())
        } else {
            (None, None)
        };
        let entity = cx.entity();
        let focus = self.focus_handle.clone();
        // While dragging: listen at window level (even if the mouse leaves the editor).
        let dragging = (self.selecting || self.scrollbar_drag.is_some()).then_some((cx.entity(), self.slot));
        let settings = cx.global::<Settings>();
        let (font_size, line_h) = (settings.editor_font_size, settings.editor_line_height);
        let font_family: SharedString = settings.editor_font_family.clone().map_or(MONO_FONT.into(), Into::into);
        let ligatures = settings.font_ligatures;
        let (word_highlight, indent_guides) = (settings.word_highlight, settings.indent_guides);
        let ws_mode = settings.render_whitespace.clone();
        let rulers: Arc<Vec<usize>> = Arc::new(match self.rules.max_line_length.filter(|_| settings.editorconfig) {
            Some(n) => vec![n],
            None => self.folder.as_ref().and_then(|f| f.rulers.clone()).unwrap_or_else(|| settings.rulers.clone()),
        });
        let cursor_shape = CursorShape::from_setting(&settings.cursor_style);
        let cursor_width = settings.cursor_width;
        let inline_blame = self.blame.clone();
        let (bracket_on, swatches_on, sticky_on, unicode_on) =
            (settings.bracket_colors, settings.color_decorators, settings.sticky_scroll, settings.unicode_highlight);
        let (line_hl_on, folding_on) = (settings.render_line_highlight, settings.folding);
        let spell_on = (settings.spelling, settings.spelling_comments);
        let named_colors = crate::color::named_colors_apply(&self.path);
        let cursor_line = self.buffer.char_to_line(cursor);
        // Width of one character for the wrap column (monospace font).
        let wrap = settings.word_wrap;
        let font_id = window.text_system().resolve_font(&gpui::font(font_family.clone()));
        let char_w = window.text_system().advance(font_id, px(font_size), 'm').map_or(px(font_size * 0.6), |s| s.width);
        self.layout_params = (wrap, self.indent_unit().max(1), char_w);
        self.refresh_display();
        let rows = self.display.row_count();
        let banner = self.banner(&p, cx);

        let find_bar = self.find.as_ref().map(|(b, _)| b.clone());
        // Popups are positioned using the previous frame's line layout; then the list is refreshed.
        let popups = self.popups(&p, font_family.clone(), font_size, line_h, cx);
        // If a positioned item is wanted and this frame's line layout turns out different, paint asks for one more frame.
        self.popup_basis = (self.visible_sig(), self.wants_anchored());
        let minimap = cx.global::<Settings>().minimap && !self.minimap_hidden;
        // `language=<id>` lets key bindings test `editor.language == '…'` (extension keymaps).
        let lang = crate::snippets::lang_of_path(&self.path);
        let key_context = match crate::ext::language_context(lang) {
            Some(l) if self.completion.is_some() => format!("Editor completing language={l}"),
            Some(l) => format!("Editor language={l}"),
            None if self.completion.is_some() => "Editor completing".to_string(),
            None => "Editor".to_string(),
        };
        if bracket_on {
            self.ensure_bracket_cache();
        }
        // Sticky headers: based on the previous frame's first visible line (off while scrolling, folding
        // or wrapping is active: the line <-> screen line mapping must not break).
        let sticky: Vec<usize> = match self.visible.first().map(|v| v.line) {
            Some(first) if sticky_on && !self.layout_params.0 && first > 0 && self.fold_headers().is_empty() => {
                let tab = self.tab_width();
                editor_decor::sticky_headers(first, 3, |l| display::line_indent_of(&self.buffer, l, tab))
            }
            _ => Vec::new(),
        };
        self.sticky_lines = sticky.clone();
        let sticky_rows: Vec<(usize, String)> = sticky.iter().map(|&l| (l, self.buffer.line(l).trim_end().replace('\t', "    "))).collect();
        self.visible.clear();
        let origin_sink = cx.entity();
        div()
            .on_action(cx.on(Self::find))
            .on_action(cx.on(Self::find_replace))
            .on_action(cx.on(Self::find_next))
            .on_action(cx.on(Self::find_prev))
            .flex()
            .flex_col()
            .size_full()
            .bg(c(p.bg))
            .text_color(c(p.text))
            .children(banner)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id("editor-surface")
                            // When the minimap is open, the text must not overflow beneath it.
                            .when(minimap, |d| d.pr(px(MINIMAP_W + 16.)))
                            // While the completion list is open, Up/Down/Enter/Tab/Esc go to the list.
                            .key_context(key_context.as_str())
                            .on_action(cx.on(Self::trigger_completion))
                            .on_action(cx.on(Self::completion_next))
                            .on_action(cx.on(Self::completion_prev))
                            .on_action(cx.on(Self::completion_accept))
                            .on_action(cx.on(Self::completion_dismiss))
                            .on_action(cx.on(Self::go_to_definition))
                            .on_action(cx.on(Self::show_hover))
                            .on_action(cx.on(Self::toggle_comment))
                            .on_action(cx.on(Self::outdent))
                            .on_action(cx.on(Self::move_line_up))
                            .on_action(cx.on(Self::move_line_down))
                            .on_action(cx.on(Self::copy_line_up))
                            .on_action(cx.on(Self::copy_line_down))
                            .on_action(cx.on(Self::delete_line))
                            .on_action(cx.on(Self::select_line))
                            .on_action(cx.on(Self::fold_region))
                            .on_action(cx.on(Self::select_all_occurrences))
                            .on_action(cx.on(Self::delete_to_line_start))
                            .on_action(cx.on(Self::insert_line_below))
                            .on_action(cx.on(Self::insert_line_above))
                            .on_action(cx.on(Self::indent_line))
                            .on_action(cx.on(Self::outdent_line))
                            .on_action(cx.on(Self::select_page_up))
                            .on_action(cx.on(Self::select_page_down))
                            .on_action(cx.on(Self::jump_to_bracket))
                            .on_action(cx.on(Self::cursor_undo))
                            .on_action(cx.on(Self::cursors_at_line_ends))
                            .on_action(cx.on(Self::toggle_bookmark))
                            .on_action(cx.on(Self::toggle_breakpoint))
                            .on_action(cx.on(Self::next_bookmark))
                            .on_action(cx.on(Self::previous_bookmark))
                            .on_action(cx.on(Self::clear_bookmarks))
                            .on_action(cx.on(Self::expand_selection))
                            .on_action(cx.on(Self::shrink_selection))
                            .on_action(cx.on(Self::join_lines))
                            .on_action(cx.on(Self::sort_lines_ascending))
                            .on_action(cx.on(Self::sort_lines_descending))
                            .on_action(cx.on(Self::transform_to_uppercase))
                            .on_action(cx.on(Self::transform_to_lowercase))
                            .on_action(cx.on(Self::transform_to_title_case))
                            .on_action(cx.on(Self::transform_to_camel_case))
                            .on_action(cx.on(Self::transform_to_pascal_case))
                            .on_action(cx.on(Self::transform_to_snake_case))
                            .on_action(cx.on(Self::transform_to_kebab_case))
                            .on_action(cx.on(Self::transform_to_constant_case))
                            .on_action(cx.on(Self::toggle_block_comment))
                            .on_action(cx.on(Self::transform_base64_encode))
                            .on_action(cx.on(Self::transform_base64_decode))
                            .on_action(cx.on(Self::transform_url_encode))
                            .on_action(cx.on(Self::transform_url_decode))
                            .on_action(cx.on(Self::transform_html_escape))
                            .on_action(cx.on(Self::transform_html_unescape))
                            .on_action(cx.on(Self::pick_color))
                            .on_action(cx.on(Self::sort_lines_insensitive))
                            .on_action(cx.on(Self::sort_lines_natural))
                            .on_action(cx.on(Self::remove_blank_lines))
                            .on_action(cx.on(Self::collapse_blank_lines))
                            .on_action(cx.on(Self::insert_date))
                            .on_action(cx.on(Self::insert_date_time))
                            .on_action(cx.on(Self::insert_uuid))
                            .on_action(cx.on(Self::duplicate_selection))
                            .on_action(cx.on(Self::trim_trailing_whitespace_cmd))
                            .on_action(cx.on(Self::stage_hunk))
                            .on_action(cx.on(Self::peek_definition))
                            .on_action(cx.on(Self::run_code_lens))
                            .on_action(cx.on(Self::show_incoming_calls))
                            .on_action(cx.on(Self::show_outgoing_calls))
                            .on_action(cx.on(Self::show_supertypes))
                            .on_action(cx.on(Self::show_subtypes))
                            .on_action(cx.on(Self::delete_all_right))
                            .on_action(cx.on(Self::select_to_bracket))
                            .on_action(cx.on(Self::add_selection_to_previous_find_match))
                            .on_action(cx.on(Self::add_line_comment))
                            .on_action(cx.on(Self::remove_line_comment))
                            .on_action(cx.on(Self::next_change))
                            .on_action(cx.on(Self::previous_change))
                            .on_action(cx.on(Self::fold_level_1))
                            .on_action(cx.on(Self::fold_level_2))
                            .on_action(cx.on(Self::fold_level_3))
                            .on_action(cx.on(Self::fold_level_4))
                            .on_action(cx.on(Self::fold_level_5))
                            .on_action(cx.on(Self::fold_level_6))
                            .on_action(cx.on(Self::fold_level_7))
                            .on_action(cx.on(Self::fold_recursively))
                            .on_action(cx.on(Self::unfold_recursively))
                            .on_action(cx.on(Self::column_select_up))
                            .on_action(cx.on(Self::column_select_down))
                            .on_action(cx.on(Self::column_select_left))
                            .on_action(cx.on(Self::column_select_right))
                            .on_action(cx.on(Self::revert_hunk))
                            .on_action(cx.on(Self::transpose_characters))
                            .on_action(cx.on(Self::reverse_lines))
                            .on_action(cx.on(Self::delete_duplicate_lines))
                            .on_action(cx.on(Self::next_problem))
                            .on_action(cx.on(Self::previous_problem))
                            .on_action(cx.on(Self::go_to_type_definition))
                            .on_action(cx.on(Self::go_to_implementation))
                            .on_action(cx.on(Self::revert_file))
                            .on_action(cx.on(Self::unfold_region))
                            .on_action(cx.on(Self::fold_all))
                            .on_action(cx.on(Self::unfold_all))
                            .track_focus(&self.focus_handle)
                            .on_action(cx.on(Self::move_left))
                            .on_action(cx.on(Self::move_right))
                            .on_action(cx.on(Self::move_up))
                            .on_action(cx.on(Self::move_down))
                            .on_action(cx.on(Self::select_left))
                            .on_action(cx.on(Self::select_right))
                            .on_action(cx.on(Self::select_up))
                            .on_action(cx.on(Self::select_down))
                            .on_action(cx.on(Self::move_word_left))
                            .on_action(cx.on(Self::move_word_right))
                            .on_action(cx.on(Self::select_word_left))
                            .on_action(cx.on(Self::select_word_right))
                            .on_action(cx.on(Self::delete_word_left))
                            .on_action(cx.on(Self::delete_word_right))
                            .on_action(cx.on(Self::move_home))
                            .on_action(cx.on(Self::move_end))
                            .on_action(cx.on(Self::select_home))
                            .on_action(cx.on(Self::select_end))
                            .on_action(cx.on(Self::doc_start))
                            .on_action(cx.on(Self::doc_end))
                            .on_action(cx.on(Self::select_doc_start))
                            .on_action(cx.on(Self::select_doc_end))
                            .on_action(cx.on(Self::page_up))
                            .on_action(cx.on(Self::page_down))
                            .on_action(cx.on(Self::select_all))
                            .on_action(cx.on(Self::escape))
                            .on_action(cx.on(Self::add_cursor_above))
                            .on_action(cx.on(Self::add_cursor_below))
                            .on_action(cx.on(Self::select_next_occurrence))
                            .on_action(cx.on(Self::backspace))
                            .on_action(cx.on(Self::delete))
                            .on_action(cx.on(Self::newline))
                            .on_action(cx.on(Self::tab))
                            .on_action(cx.on(Self::accept_current_change))
                            .on_action(cx.on(Self::accept_incoming_change))
                            .on_action(cx.on(Self::accept_both_changes))
                            .on_action(cx.on(Self::accept_all_current))
                            .on_action(cx.on(Self::accept_all_incoming))
                            .on_action(cx.on(Self::next_conflict))
                            .on_action(cx.on(Self::previous_conflict))
                            .on_action(cx.on(Self::undo))
                            .on_action(cx.on(Self::redo))
                            .on_action(cx.on(Self::copy))
                            .on_action(cx.on(Self::cut))
                            .on_action(cx.on(Self::paste))
                            .on_action(cx.on(Self::save))
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .cursor(CursorStyle::IBeam)
                            .font_family(font_family.clone())
                            .map(|mut d| {
                                // Ligature font feature: when off, `calt`/`liga` are disabled (for fonts like Fira Code).
                                if !ligatures {
                                    d.text_style().get_or_insert_with(Default::default).font_features = Some(gpui::FontFeatures::disable_ligatures());
                                }
                                d
                            })
                            .text_size(px(font_size))
                            .line_height(px(line_h))
                            .on_mouse_down(MouseButton::Left, cx.on(Self::on_mouse_down))
                            .on_mouse_down(MouseButton::Right, cx.on(Self::on_right_click))
                            .on_scroll_wheel(cx.on(move |e, ev: &gpui::ScrollWheelEvent, _, cx| {
                                let d = ev.delta.pixel_delta(px(line_h));
                                let dx = if ev.modifiers.shift && d.x == px(0.) { d.y } else { d.x };
                                if dx != px(0.) {
                                    e.scroll_x(dx, cx);
                                }
                            }))
                            .on_mouse_move(cx.on(Self::on_mouse_move))
                            .on_mouse_up(MouseButton::Left, cx.on(Self::on_mouse_up))
                            .on_mouse_up_out(MouseButton::Left, cx.on(Self::on_mouse_up))
                            .child(
                                uniform_list(
                                    "editor-lines",
                                    rows,
                                    cx.processor(move |this, range: Range<usize>, window, cx| {
                                        let entity = cx.entity();
                                        let (ws_mode, rulers, numbers_mode) = (ws_mode.clone(), rulers.clone(), numbers_mode.clone());
                                        let n_lines = this.buffer.len_lines();
                                        if range.is_empty() || n_lines == 0 {
                                            return Vec::new();
                                        }
                                        let rope = this.buffer.rope().clone();
                                        let first_line = this.display.line_at_row(range.start).0;
                                        let last_line = (this.display.line_at_row(range.end - 1).0 + 1).min(n_lines);
                                        let spans =
                                            this.syntax.as_ref().map(|s| s.highlights(&rope, rope.line_to_byte(first_line)..rope.line_to_byte(last_line)));
                                        let folded = this.fold_headers();
                                        let bookmarks = this.bookmark_lines();
                                        let breakpoints = this.breakpoints();
                                        let debug_line = this.debug_line;
                                        let tab = this.tab_width();
                                        // Segments of the same line come one after another: split once per line.
                                        let mut cache: Option<(usize, String, Vec<usize>)> = None;
                                        range
                                            .map(|row| {
                                                let (ix, seg) = this.display.line_at_row(row);
                                                if cache.as_ref().is_none_or(|(l, ..)| *l != ix) {
                                                    cache = Some((ix, this.buffer.line(ix), this.wrap_points_of(ix)));
                                                }
                                                let (_, full, points) = cache.as_ref().unwrap();
                                                let full_len = full.chars().count();
                                                let r = display::segment(points, seg, full_len);
                                                let last_seg = r.end == full_len;
                                                let line_start = this.buffer.line_to_char(ix);
                                                let (start, end) = (line_start + r.start, line_start + r.end);
                                                let (b0, b1) = (byte_of(full, r.start), byte_of(full, r.end));
                                                let text = full[b0..b1].to_string();
                                                let annotation = |id: &'static str, hint: SharedString, color: u32, opacity| {
                                                    let run = TextRun {
                                                        len: text.len(),
                                                        font: gpui::font(font_family.clone()),
                                                        color: c(p.text).into(),
                                                        background_color: None,
                                                        underline: None,
                                                        strikethrough: None,
                                                    };
                                                    let code = window.text_system().shape_line(text.clone().into(), px(font_size), &[run], None).width;
                                                    // Current layout clips to the space after code, including during a pane resize.
                                                    div()
                                                        .id((id, ix))
                                                        .debug_selector(move || id.to_string())
                                                        .absolute()
                                                        .left(px(gutter_w) + (code - this.hscroll).max(px(0.)) + px(20.))
                                                        .right(px(28.))
                                                        .min_w_0()
                                                        .overflow_hidden()
                                                        .top_0()
                                                        .h(px(line_h))
                                                        .flex()
                                                        .items_center()
                                                        .justify_end()
                                                        .text_size(px(font_size * 0.85))
                                                        .text_color(ui::ca(color, opacity))
                                                        .child(div().min_w_0().max_w(px(480.)).truncate().child(hint.clone()))
                                                        .tooltip(move |_, cx| cx.new(|_| crate::workbench::Tooltip(hint.clone())).into())
                                                };
                                                // A cursor at the end of a segment is drawn at the start of the next segment.
                                                let here = |c: usize| c >= start && (c < end || (c == end && last_seg));
                                                let line_cursors: Vec<usize> =
                                                    cursors.iter().filter(|&&c| here(c)).map(|&c| byte_of(&text, c - start)).collect();
                                                let sel: Vec<(usize, usize, bool)> = selections
                                                    .iter()
                                                    .filter_map(|s| {
                                                        let a = s.start.max(start);
                                                        let b = s.end.min(end);
                                                        (a <= b && s.start <= end && s.end > start)
                                                            .then(|| (byte_of(&text, a - start), byte_of(&text, b - start), s.end > end && last_seg))
                                                    })
                                                    .collect();
                                                let mark = marked.as_ref().and_then(|m| {
                                                    (m.start >= start && m.end <= end).then(|| byte_of(&text, m.start - start)..byte_of(&text, m.end - start))
                                                });
                                                let current = cursor_lines.contains(&ix);
                                                let line_diags: Vec<(Range<usize>, u8)> = if this.diagnostics.is_empty() {
                                                    Vec::new()
                                                } else {
                                                    this.line_diagnostics(ix, full)
                                                        .into_iter()
                                                        .filter_map(|(r, sev)| {
                                                            let (a, b) = (r.start.max(b0), r.end.min(b1));
                                                            (a < b || (a == b && r.start == r.end && a >= b0 && a <= b1)).then(|| (a - b0..b - b0, sev))
                                                        })
                                                        .collect()
                                                };
                                                let line_diags = {
                                                    let mut d = line_diags;
                                                    for sp in this.spelling_in_segment(ix, spans.as_deref(), rope.line_to_byte(ix) + b0, &text, spell_on) {
                                                        d.push((sp, super::spell_glue::SPELLING_SEVERITY));
                                                    }
                                                    d
                                                };
                                                let mut marks: Vec<(Range<usize>, Mark)> = Vec::new();
                                                if !this.symbol_hl.is_empty() && word_highlight {
                                                    // Usages reported by the server (more accurate than plain word matching).
                                                    let first = this.symbol_hl.partition_point(|m| m.end <= start);
                                                    for m in this.symbol_hl[first..].iter().take_while(|m| m.start < end) {
                                                        let (a, b) = (m.start.max(start), m.end.min(end));
                                                        if a < b {
                                                            marks.push((byte_of(&text, a - start)..byte_of(&text, b - start), Mark::Word));
                                                        }
                                                    }
                                                } else if let Some(w) = word.as_ref().filter(|_| word_highlight) {
                                                    marks.extend(word_occurrences(&text, w).into_iter().map(|r| (r, Mark::Word)));
                                                }
                                                // Find: matches intersecting the segment (clipped if multi-line).
                                                let first = this.matches.partition_point(|m| m.end <= start);
                                                for m in this.matches[first..].iter().take_while(|m| m.start <= end) {
                                                    let (a, b) = (m.start.max(start), m.end.min(end));
                                                    if a < b {
                                                        let kind = if *m == current_sel { Mark::FindCurrent } else { Mark::Find };
                                                        marks.push((byte_of(&text, a - start)..byte_of(&text, b - start), kind));
                                                    }
                                                }
                                                if let Some(scope) = this.find.as_ref().and_then(|_| this.find_scope()) {
                                                    let (a, b) = (scope.start.max(start), scope.end.min(end));
                                                    if a < b {
                                                        marks.insert(0, (byte_of(&text, a - start)..byte_of(&text, b - start), Mark::FindScope));
                                                    }
                                                }
                                                if let Some((a, b)) = brackets {
                                                    for pos in [a, b] {
                                                        if pos >= start && pos < end {
                                                            let col = pos - start;
                                                            marks.push((byte_of(&text, col)..byte_of(&text, col + 1), Mark::Bracket));
                                                        }
                                                    }
                                                }
                                                let whitespace = editor_decor::whitespace_marks(&text, &ws_mode);
                                                let swatches: Vec<(Range<usize>, u32, f32)> = if swatches_on {
                                                    crate::color::find_colors(&text, named_colors)
                                                        .into_iter()
                                                        .map(|f| (f.range, f.color.rgb(), f.color.a))
                                                        .collect()
                                                } else {
                                                    Vec::new()
                                                };
                                                let unicode = if unicode_on { editor_decor::suspicious_chars(&text) } else { Vec::new() };
                                                let brackets_col: Vec<(usize, u32)> = if bracket_on {
                                                    let mut depth = this.bracket_cache.1.get(ix).copied().unwrap_or(0);
                                                    let colors = [p.warning, p.syntax_function, p.info];
                                                    editor_decor::brackets_in_line(full, &mut depth)
                                                        .into_iter()
                                                        .filter(|(b, _)| *b >= b0 && *b < b1)
                                                        .map(|(b, k)| (b - b0, colors[k]))
                                                        .collect()
                                                } else {
                                                    Vec::new()
                                                };
                                                let is_folded = folded.contains(&ix);
                                                let foldable = seg == 0 && (is_folded || display::is_foldable_with(&this.buffer, ix, tab, &this.lsp_folds));
                                                div()
                                                    .flex()
                                                    .w_full()
                                                    .relative()
                                                    .h(px(line_h))
                                                    .map(|d| match this.conflict_tint(ix, &p) {
                                                        Some(tint) => d.bg(tint),
                                                        None if debug_line == Some(ix) => d.bg(ui::ca(p.warning, 0.22)),
                                                        None => d.when(current && line_hl_on, |d| d.bg(c(p.current_line))),
                                                    })
                                                    .when_some(this.ext_tone(ix, ext_api::value::DecorationPlace::Line), |d, tone| {
                                                        d.bg(ui::ca(super::tone_color(&p, tone), 0.14))
                                                    })
                                                    .child(
                                                        div()
                                                            .id(("gutter", row))
                                                            .group("gutter")
                                                            .w(px(gutter_w))
                                                            .flex_none()
                                                            .relative()
                                                            .flex()
                                                            .justify_end()
                                                            .pr(px(theme::SPACE[6]))
                                                            .text_color(c(if current { p.line_number_active } else { p.line_number }))
                                                            // Continuation segments of a wrapped line have no number.
                                                            .when_some(
                                                                editor_decor::line_number(&numbers_mode, ix, cursor_line).filter(|_| seg == 0),
                                                                |d, n| d.child(n),
                                                            )
                                                            // An extension's gutter decoration: a dot in its tone.
                                                            .when_some(
                                                                this.ext_tone(ix, ext_api::value::DecorationPlace::Gutter).filter(|_| seg == 0),
                                                                |d, tone| {
                                                                    d.child(
                                                                        div()
                                                                            .absolute()
                                                                            .left(px(theme::SPACE[4]))
                                                                            .top(px(line_h / 2. - 3.))
                                                                            .size(px(6.))
                                                                            .rounded_full()
                                                                            .bg(c(super::tone_color(&p, tone))),
                                                                    )
                                                                },
                                                            )
                                                            // Breakpoint: red dot; on an empty gutter a dim preview on hover, toggled on click.
                                                            .when(seg == 0, |d| {
                                                                let bp = breakpoints.binary_search_by_key(&ix, |(l, _)| *l).ok().map(|i| &breakpoints[i].1);
                                                                let on = bp.is_some();
                                                                // Conditional: orange, logpoint: blue, with hit count: accent color, plain: red. A disabled one
                                                                // is a muted ring, one the debugger could not bind a red ring.
                                                                let dot = match bp {
                                                                    Some(m) if m.disabled => p.text_muted,
                                                                    Some(m) if m.log.is_some() => p.info,
                                                                    Some(m) if m.condition.is_some() => p.warning,
                                                                    Some(m) if m.hit.is_some() => p.accent,
                                                                    _ => p.danger,
                                                                };
                                                                let ring =
                                                                    bp.is_some_and(|m| m.disabled || matches!(m.verified, crate::debug_model::Verified::No(_)));
                                                                d.child(
                                                                    div()
                                                                        .id(("bp", row))
                                                                        .absolute()
                                                                        .left(px(2.))
                                                                        .top_0()
                                                                        .bottom_0()
                                                                        .w(px(14.))
                                                                        .flex()
                                                                        .items_center()
                                                                        .justify_center()
                                                                        .cursor(CursorStyle::PointingHand)
                                                                        .child(
                                                                            div()
                                                                                .w(px(9.))
                                                                                .h(px(9.))
                                                                                .rounded_full()
                                                                                .when(!ring, |d| d.bg(c(dot)))
                                                                                .when(ring, |d| d.border_2().border_color(c(dot)))
                                                                                .when(!on, |d| d.opacity(0.).group_hover("gutter", |s| s.opacity(0.28))),
                                                                        )
                                                                        .on_mouse_down(
                                                                            MouseButton::Left,
                                                                            cx.on(move |e, _: &MouseDownEvent, _, cx| {
                                                                                cx.stop_propagation();
                                                                                e.toggle_breakpoint_at(ix, cx);
                                                                            }),
                                                                        ),
                                                                )
                                                            })
                                                            // Stopped line: yellow arrow.
                                                            .when(seg == 0 && debug_line == Some(ix), |d| {
                                                                d.child(div().absolute().left(px(1.)).top_0().bottom_0().flex().items_center().child(icon(
                                                                    "chevron-right",
                                                                    14.,
                                                                    p.warning,
                                                                )))
                                                            })
                                                            // Bookmark: a small marker at the left of the gutter.
                                                            .when(seg == 0 && bookmarks.binary_search(&ix).is_ok(), |d| {
                                                                d.child(
                                                                    div()
                                                                        .absolute()
                                                                        .left(px(4.))
                                                                        .top_0()
                                                                        .bottom_0()
                                                                        .flex()
                                                                        .items_center()
                                                                        .child(icon("bookmark", 12., p.accent)),
                                                                )
                                                            })
                                                            // Fold arrow: always if folded, on hover if foldable.
                                                            .when(foldable && (folding_on || is_folded), |d| {
                                                                d.child(
                                                                    div()
                                                                        .absolute()
                                                                        .right(px(1.))
                                                                        .top_0()
                                                                        .bottom_0()
                                                                        .flex()
                                                                        .items_center()
                                                                        .cursor(CursorStyle::PointingHand)
                                                                        .when(!is_folded, |d| d.opacity(0.).group_hover("gutter", |s| s.opacity(1.)))
                                                                        .child(icon(
                                                                            if is_folded { "chevron-right" } else { "chevron-down" },
                                                                            12.,
                                                                            p.text_muted,
                                                                        ))
                                                                        .on_mouse_down(
                                                                            MouseButton::Left,
                                                                            cx.on(move |e, _: &MouseDownEvent, _, cx| {
                                                                                cx.stop_propagation();
                                                                                e.toggle_fold_line(ix);
                                                                                cx.emit(EditorEvent::Changed);
                                                                                cx.notify();
                                                                            }),
                                                                        ),
                                                                )
                                                            })
                                                            // Git: strip for added/changed lines, a notch at the top for deleted lines.
                                                            .when_some(this.git_marks.get(ix).copied().flatten().filter(|_| seg == 0), |d, m| {
                                                                use crate::git::LineMark;
                                                                let bar = div().absolute().right(px(theme::SPACE[3])).w(px(3.));
                                                                d.child(match m {
                                                                    LineMark::Added => bar.top_0().bottom_0().bg(c(p.success)),
                                                                    LineMark::Modified => bar.top_0().bottom_0().bg(c(p.info)),
                                                                    LineMark::DeletedAbove => bar.top_0().h(px(3.)).w(px(7.)).bg(c(p.danger)),
                                                                })
                                                            }),
                                                    )
                                                    .when_some(
                                                        this.eol_hint(ix).filter(|_| seg == 0 && inline_blame.as_ref().is_none_or(|(l, ..)| *l != ix)),
                                                        |d, hint| d.child(annotation("line-end-hint", hint.into(), p.text_muted, 0.65)),
                                                    )
                                                    // Extension text after the line, when nothing of the editor's own is there.
                                                    .when_some(
                                                        this.ext_after(ix).filter(|_| {
                                                            last_seg && this.eol_hint(ix).is_none() && inline_blame.as_ref().is_none_or(|(l, ..)| *l != ix)
                                                        }),
                                                        |d, (text, tone, hover)| {
                                                            let shown = match hover {
                                                                Some(h) => format!("{text} · {h}"),
                                                                None => text,
                                                            };
                                                            d.child(annotation("extension-decoration", shown.into(), super::tone_color(&p, tone), 0.8))
                                                        },
                                                    )
                                                    // Right-aligned inside the space left after the shaped code.
                                                    .when_some(inline_blame.as_ref().filter(|(l, _, _)| *l == ix && seg == 0), |d, (_, _, text)| {
                                                        d.child(annotation("inline-blame", text.clone(), p.text_muted, 0.8))
                                                    })
                                                    .child(LineElement {
                                                        editor: entity.clone(),
                                                        line: ix,
                                                        start,
                                                        guides: if indent_guides && seg == 0 { this.guide_levels(ix) } else { 0 },
                                                        marks,
                                                        indent_unit: this.indent_unit(),
                                                        runs: {
                                                            let base = match &spans {
                                                                Some(sp) => syntax::line_runs(rope.line_to_byte(ix) + b0, text.len(), sp),
                                                                None => vec![(text.len(), None)],
                                                            };
                                                            match this.sem.get(&ix) {
                                                                Some(tokens) => {
                                                                    let spans: Vec<(Range<usize>, Role)> = tokens
                                                                        .iter()
                                                                        .filter_map(|&(s, l, role)| {
                                                                            let (a, b) = (
                                                                                editor_decor::utf16_to_byte(full, s).max(b0),
                                                                                editor_decor::utf16_to_byte(full, s.saturating_add(l)).min(b1),
                                                                            );
                                                                            (a < b).then(|| (a - b0..b - b0, role))
                                                                        })
                                                                        .collect();
                                                                    editor_decor::apply_roles(base, &spans)
                                                                }
                                                                None => base,
                                                            }
                                                        },
                                                        text: text.into(),
                                                        cursors: if focused && this.cursor_on { line_cursors } else { Vec::new() },
                                                        selections: sel,
                                                        dim_selection: !focused,
                                                        marked: mark,
                                                        diags: line_diags,
                                                        fold_marker: is_folded && last_seg,
                                                        rulers: rulers.clone(),
                                                        whitespace,
                                                        swatches,
                                                        unicode,
                                                        brackets: brackets_col,
                                                        cursor_shape,
                                                        cursor_width,
                                                        hscroll: this.hscroll,
                                                        palette: p,
                                                        line_h,
                                                    })
                                            })
                                            .collect::<Vec<_>>()
                                    }),
                                )
                                .track_scroll(self.scroll.clone())
                                .size_full(),
                            )
                            // Sticky scope headers: at the top, clicking goes to the line.
                            .when(!sticky_rows.is_empty(), |d| {
                                d.child(
                                    div().absolute().top_0().left_0().right_0().flex().flex_col().bg(c(p.bg)).border_b_1().border_color(c(p.border)).children(
                                        sticky_rows.iter().map(|(line, text)| {
                                            let line = *line;
                                            div()
                                                .id(("sticky", line))
                                                .h(px(line_h))
                                                .w_full()
                                                .flex()
                                                .cursor_pointer()
                                                .hover(|d| d.bg(c(p.surface_hover)))
                                                .child(
                                                    div()
                                                        .w(px(gutter_w))
                                                        .flex_none()
                                                        .flex()
                                                        .justify_end()
                                                        .pr(px(theme::SPACE[6]))
                                                        .text_color(c(p.line_number))
                                                        .child((line + 1).to_string()),
                                                )
                                                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_color(c(p.text)).child(text.clone()))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.on(move |e, _: &MouseDownEvent, window, cx| {
                                                        cx.stop_propagation();
                                                        e.go_to_line(line + 1, window, cx);
                                                    }),
                                                )
                                        }),
                                    ),
                                )
                            })
                            // Platform text input (including IME) is bound to this view.
                            .child(
                                canvas(
                                    |_, _, _| {},
                                    move |bounds, _, window, cx| {
                                        window.handle_input(&focus, ElementInputHandler::new(bounds, entity), cx);
                                        if let Some((ed, slot)) = dragging.clone() {
                                            let ed2 = ed.clone();
                                            window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
                                                if phase == gpui::DispatchPhase::Bubble {
                                                    ed.update(cx, |e, cx| {
                                                        let prev = e.slot;
                                                        e.enter(slot);
                                                        e.on_drag_move(ev, window, cx);
                                                        e.enter(prev);
                                                    });
                                                }
                                            });
                                            window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
                                                if phase == gpui::DispatchPhase::Bubble && ev.button == MouseButton::Left {
                                                    ed2.update(cx, |e, cx| {
                                                        let prev = e.slot;
                                                        e.enter(slot);
                                                        e.on_mouse_up(ev, window, cx);
                                                        e.enter(prev);
                                                    });
                                                }
                                            });
                                        }
                                        origin_sink.update(cx, |e, _| e.surface_origin = bounds.origin);
                                        if minimap {
                                            let start = origin_sink.read(cx).paint_minimap(bounds, line_h, &p, window);
                                            origin_sink.update(cx, |e, _| e.minimap_start = start);
                                        }
                                        // Overview ruler (inside the scrollbar): diagnostics, git
                                        // changes, find matches, cursor line.
                                        let marks = origin_sink.read(cx).overview_marks(&p);
                                        let view = bounds.size.height / px(1.);
                                        // A 2 px bucket per lane: thousands of matches shouldn't become thousands of
                                        // overlapping rectangles (paint order in GPUI got quadratically slower).
                                        let mut buckets: std::collections::BTreeMap<(u8, u32), gpui::Rgba> = std::collections::BTreeMap::new();
                                        for (frac, color, lane) in marks {
                                            let y = ((frac * view).min(view - 2.) / 2.) as u32;
                                            buckets.insert((lane, y), color); // the later (more important) one wins
                                        }
                                        for ((lane, y), color) in buckets {
                                            let (x, w) = match lane {
                                                0 => (bounds.right() - px(14.), px(4.)),
                                                1 => (bounds.right() - px(9.), px(4.)),
                                                _ => (bounds.right() - px(14.), px(14.)),
                                            };
                                            window.paint_quad(fill(Bounds::new(point(x, bounds.top() + px(y as f32 * 2.)), size(w, px(2.))), color));
                                        }
                                        let h = bounds.size.height;
                                        // Height was 0 in the first frame: the scrollbar and positioned items only land in the right place
                                        // on the next paint. A notify during painting is
                                        // cleared when the frame ends; explicitly request the next frame.
                                        let again = origin_sink.update(cx, |e, _| {
                                            let resized = e.surface_h != h;
                                            e.surface_h = h;
                                            let (sig, anchored) = e.popup_basis;
                                            resized || (anchored && sig != e.visible_sig())
                                        });
                                        if again {
                                            window.request_animation_frame();
                                        }
                                    },
                                )
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full(),
                            )
                            // Scrollbar: clicking jumps there, dragging scrolls.
                            .when_some(self.scrollbar_geometry(), |d, (_, _, thumb, top)| {
                                let dragging = self.scrollbar_drag.is_some();
                                d.child(
                                    div()
                                        .id("editor-scrollbar")
                                        .group("editor-scrollbar")
                                        .absolute()
                                        .top_0()
                                        .bottom_0()
                                        .right_0()
                                        .w(px(14.))
                                        .cursor(CursorStyle::Arrow)
                                        .on_mouse_down(MouseButton::Left, cx.on(Self::on_scrollbar_down))
                                        .child(
                                            div()
                                                .absolute()
                                                .top(px(top))
                                                .right(px(2.))
                                                .w(px(10.))
                                                .h(px(thumb))
                                                .rounded(px(2.))
                                                .bg(ui::ca(p.text_muted, if dragging { 0.55 } else { 0.28 }))
                                                .group_hover("editor-scrollbar", |s| s.bg(ui::ca(p.text_muted, 0.45))),
                                        ),
                                )
                            }),
                    )
                    .when(minimap, |d| {
                        d.child(
                            div()
                                .id("minimap")
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .right(px(12.))
                                .w(px(MINIMAP_W))
                                .cursor(CursorStyle::Arrow)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.on(|e, ev: &MouseDownEvent, _, cx| {
                                        cx.stop_propagation();
                                        e.minimap_jump(ev.position.y, cx);
                                    }),
                                )
                                .on_mouse_move(cx.on(|e, ev: &MouseMoveEvent, _, cx| {
                                    if ev.pressed_button == Some(MouseButton::Left) {
                                        e.minimap_jump(ev.position.y, cx);
                                    }
                                })),
                        )
                    })
                    .children(popups)
                    // The find bar floats above the text but outside the "Editor" context:
                    // keys inside it (Tab, Cmd+Z) must not go to the buffer.
                    .when_some(find_bar, |d, bar| {
                        d.child(
                            div()
                                .absolute()
                                .top(px(theme::SPACE[2]))
                                .left(px(theme::SPACE[2]))
                                .right(px(theme::SPACE[7]))
                                .flex()
                                .justify_end()
                                .child(div().max_w(gpui::relative(1.)).shadow_md().rounded(px(theme::radius::MD)).child(bar)),
                        )
                    }),
            )
    }
}

/// A single line: shapes the text, paints the selection and cursor, reports its layout to the editor.
struct LineElement {
    editor: Entity<EditorView>,
    line: usize,
    /// Segments covering the line (byte length, syntax role).
    runs: Vec<(usize, Option<Role>)>,
    /// Number of indent guide lines and the column width of one level.
    guides: usize,
    marks: Vec<(Range<usize>, Mark)>,
    indent_unit: usize,
    text: SharedString,
    /// First character of the segment in the document (not the line start if it isn't the first segment of a wrapped line).
    start: usize,
    /// Folded header: a "⋯" marker at the end of the text.
    fold_marker: bool,
    /// Vertical rulers (column counts).
    rulers: Arc<Vec<usize>>,
    /// Whitespace to show (byte range, kind).
    whitespace: Vec<(Range<usize>, editor_decor::Ws)>,
    /// Color literals (hex, rgb(), hsl(), CSS name): a color strip is drawn under them. (range, 0xRRGGBB, alpha)
    swatches: Vec<(Range<usize>, u32, f32)>,
    /// Invisible / confusable characters: (byte range, is invisible). Drawn with a box.
    unicode: Vec<(Range<usize>, bool)>,
    /// Brackets colored by depth: (byte, color).
    brackets: Vec<(usize, u32)>,
    cursor_shape: CursorShape,
    cursor_width: f32,
    /// Cursors on this line (bytes).
    cursors: Vec<usize>,
    /// (start byte, end byte, whether it also covers the line end)
    selections: Vec<(usize, usize, bool)>,
    /// Editor not focused: the selection is dimmed (VS Code's inactiveSelection).
    dim_selection: bool,
    marked: Option<Range<usize>>,
    /// Diagnostics: (byte range, severity) -> wavy underline.
    diags: Vec<(Range<usize>, u8)>,
    hscroll: Pixels,
    palette: Palette,
    line_h: f32,
}

impl IntoElement for LineElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for LineElement {
    type RequestLayoutState = ();
    /// (line layout, width of one column)
    type PrepaintState = Option<(ShapedLine, Pixels)>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = px(self.line_h).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _cx: &mut App,
    ) -> Option<(ShapedLine, Pixels)> {
        let style = window.text_style();
        let p = &self.palette;
        let mut runs: Vec<TextRun> = self
            .runs
            .iter()
            .map(|&(len, role)| {
                let mut font = style.font();
                let color = match role {
                    None => p.text,
                    Some(Role::Keyword) => p.syntax_keyword,
                    Some(Role::String) => p.syntax_string,
                    Some(Role::Number) => p.syntax_number,
                    Some(Role::Function) => p.syntax_function,
                    Some(Role::Type) => p.syntax_type,
                    Some(Role::Comment) => {
                        font.style = gpui::FontStyle::Italic;
                        p.syntax_comment
                    }
                    Some(Role::Tag) => p.syntax_tag,
                    Some(Role::Punct) => p.syntax_punct,
                };
                TextRun { len, font, color: c(color).into(), background_color: None, underline: None, strikethrough: None }
            })
            .filter(|r| r.len > 0)
            .collect();
        // IME composition underline, diagnostic squiggles (error red, warning yellow, info blue) and bracket colours.
        let diags: Vec<(Range<usize>, gpui::Hsla, bool)> = self
            .diags
            .iter()
            .map(|(r, sev)| {
                let color = match sev {
                    1 => p.danger,
                    2 => p.warning,
                    // Spelling: a muted blue-gray so it is distinguishable from diagnostics.
                    9 => ui::mix(p.info, p.text_muted, 0.5),
                    _ => p.info,
                };
                (r.clone(), c(color).into(), *sev == 1)
            })
            .collect();
        let brackets: Vec<(usize, gpui::Hsla)> = self.brackets.iter().map(|&(b, color)| (b, c(color).into())).collect();
        runs = crate::editor_overlay::overlay_runs(runs, self.marked.as_ref(), &diags, &brackets);
        let font_size = style.font_size.to_pixels(window.rem_size());
        let space = TextRun { len: 1, font: style.font(), color: c(p.text).into(), background_color: None, underline: None, strikethrough: None };
        let col_w = window.text_system().shape_line(" ".into(), font_size, &[space], None).width;
        Some((window.text_system().shape_line(self.text.clone(), font_size, &runs, None), col_w))
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        layout: &mut Option<(ShapedLine, Pixels)>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some((layout, col_w)) = layout.take() else { return };
        let (top, bottom) = (bounds.top(), bounds.bottom());
        // Horizontal scroll: everything shifts left by the offset; outside the line area (gutter) is clipped.
        let left = bounds.left() - self.hscroll;
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for (r, mark) in &self.marks {
                let b = Bounds::from_corners(point(left + layout.x_for_index(r.start), top), point(left + layout.x_for_index(r.end), bottom));
                match mark {
                    Mark::Word => window.paint_quad(fill(b, ui::ca(self.palette.selection, 0.55))),
                    Mark::Find => window.paint_quad(fill(b, ui::ca(self.palette.warning, 0.22))),
                    Mark::FindScope => window.paint_quad(fill(b, ui::ca(self.palette.selection, 0.35))),
                    Mark::FindCurrent => window.paint_quad(gpui::outline(b, c(self.palette.warning), gpui::BorderStyle::Solid)),
                    Mark::Bracket => {
                        window.paint_quad(fill(b, ui::ca(self.palette.text_muted, 0.15)));
                        window.paint_quad(gpui::outline(b, c(self.palette.text_muted), gpui::BorderStyle::Solid));
                    }
                }
            }
            for k in 0..self.guides {
                let x = left + col_w * (k * self.indent_unit) as f32;
                window.paint_quad(fill(Bounds::new(point(x, top), size(px(1.), bottom - top)), c(self.palette.border)));
            }
            for &col in self.rulers.iter() {
                let x = left + col_w * col as f32;
                window.paint_quad(fill(Bounds::new(point(x, top), size(px(1.), bottom - top)), ui::ca(self.palette.text_muted, 0.28)));
            }
            for &(a, b, past_eol) in &self.selections {
                let x0 = left + layout.x_for_index(a);
                let mut x1 = left + layout.x_for_index(b);
                if past_eol {
                    x1 += px(self.line_h * 0.4); // show that the line end is selected
                }
                let color = if self.dim_selection { ui::ca(self.palette.selection, 0.5) } else { c(self.palette.selection) };
                window.paint_quad(fill(Bounds::from_corners(point(x0, top), point(x1, bottom)), color));
            }
            let _ = layout.paint(point(left, top), px(self.line_h), window, cx);
            let mid = top + (bottom - top) / 2.;
            for (r, kind) in &self.whitespace {
                let (x0, x1) = (left + layout.x_for_index(r.start), left + layout.x_for_index(r.end));
                let color = ui::ca(self.palette.text_muted, 0.55);
                match kind {
                    editor_decor::Ws::Space => {
                        let d = px(2.);
                        window.paint_quad(fill(Bounds::new(point((x0 + x1) / 2. - d / 2., mid - d / 2.), size(d, d)), color).corner_radii(d / 2.));
                    }
                    editor_decor::Ws::Tab => {
                        window.paint_quad(fill(Bounds::from_corners(point(x0 + px(2.), mid), point(x1 - px(2.), mid + px(1.))), color));
                        let d = px(3.);
                        window.paint_quad(fill(Bounds::new(point(x1 - px(4.), mid - d / 2. + px(0.5)), size(px(2.), d)), color));
                    }
                }
            }
            for (r, rgb, alpha) in &self.swatches {
                let (x0, x1) = (left + layout.x_for_index(r.start), left + layout.x_for_index(r.end));
                let bar = Bounds::from_corners(point(x0, bottom - px(5.)), point(x1, bottom - px(1.)));
                // So white on white doesn't vanish: a thin border; semi-transparent colors are used as-is (blending with the background).
                window.paint_quad(fill(bar, ui::ca(*rgb, *alpha)).corner_radii(px(2.)));
                window.paint_quad(gpui::outline(bar, ui::ca(self.palette.text, 0.28), gpui::BorderStyle::Solid).corner_radii(px(2.)));
            }
            for (r, invisible) in &self.unicode {
                let (x0, x1) = (left + layout.x_for_index(r.start), left + layout.x_for_index(r.end));
                // So a zero-width character is also visible: at least half a cell.
                let w = (x1 - x0).max(col_w * 0.5);
                let b = Bounds::new(point(x0, top + px(1.)), size(w, bottom - top - px(2.)));
                let color = if *invisible { self.palette.danger } else { self.palette.warning };
                window.paint_quad(fill(b, ui::ca(color, 0.18)));
                window.paint_quad(gpui::outline(b, ui::ca(color, 0.8), gpui::BorderStyle::Solid));
            }
            for &col in &self.cursors {
                let x = left + layout.x_for_index(col);
                let next = self.text[col.min(self.text.len())..].chars().next().map_or(col, |ch| col + ch.len_utf8());
                let w = if next > col { (layout.x_for_index(next) - layout.x_for_index(col)).max(px(2.)) } else { col_w };
                match self.cursor_shape {
                    CursorShape::Line => window.paint_quad(fill(Bounds::new(point(x, top), size(px(self.cursor_width), bottom - top)), c(self.palette.accent))),
                    CursorShape::Block => window.paint_quad(fill(Bounds::new(point(x, top), size(w, bottom - top)), ui::ca(self.palette.accent, 0.6))),
                    CursorShape::Underline => window.paint_quad(fill(Bounds::new(point(x, bottom - px(2.)), size(w, px(2.))), c(self.palette.accent))),
                }
            }
            if self.fold_marker {
                // A small "⋯" pill in place of the folded lines.
                let h = px(self.line_h * 0.62);
                let x = left + layout.width + px(8.);
                let y = top + (bottom - top - h) / 2.;
                window.paint_quad(fill(Bounds::new(point(x, y), size(px(26.), h)), ui::ca(self.palette.text_muted, 0.18)).corner_radii(px(4.)));
                for k in 0..3 {
                    let d = px(3.);
                    window.paint_quad(
                        fill(Bounds::new(point(x + px(6. + 5. * k as f32), y + (h - d) / 2.), size(d, d)), c(self.palette.text_muted)).corner_radii(d / 2.),
                    );
                }
            }
        });
        let (line, start, text) = (self.line, self.start, self.text.clone());
        let text_left = bounds.left() - self.hscroll;
        let width = bounds.size.width;
        self.editor.update(cx, |e, cx| {
            e.visible.push(VisibleLine { line, start, text, bounds, layout, text_left });
            // The wrap column comes from the text area width: repaint when it changes.
            if (e.text_width - width).abs() > px(0.5) {
                e.text_width = width;
                if e.layout_params.0 {
                    cx.notify();
                }
            }
        });
    }
}

#[cfg(test)]
mod annotation_tests {
    use super::*;

    #[gpui::test]
    fn inline_annotations_yield_to_code_when_the_editor_narrows(cx: &mut gpui::TestAppContext) {
        let (wb, cx) = crate::qa_workbench_tests::setup(cx);
        let project = crate::qa_workbench_tests::project("annotation-width");
        let path = project.join("src/main.rs");
        std::fs::write(&path, "fn main() { println!(\"Codeyard production review\"); }\n").unwrap();
        crate::qa_workbench_tests::open_in(&wb, &path, cx);
        let editor = wb.read_with(cx, |wb, _| wb.active_editor_entity().unwrap());
        editor.update(cx, |ed, cx| {
            ed.blame = Some((0, ed.buffer.version(), "QA author, an hour ago · Initial workspace".into()));
            cx.notify();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("inline-blame").unwrap().size.width > px(0.), "wide editors have room for the annotation");
        cx.simulate_resize(gpui::size(px(640.), px(800.)));
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("inline-blame").unwrap().size.width == px(0.),
            "a short character count does not imply there is space beside the code: width {:?}, bounds {:?}",
            editor.read_with(cx, |ed, _| ed.text_width),
            cx.debug_bounds("inline-blame")
        );
        cx.simulate_resize(gpui::size(px(1280.), px(800.)));
        cx.run_until_parked();
        assert!(cx.debug_bounds("inline-blame").unwrap().size.width > px(0.), "resizing back restores the annotation");
    }
}
