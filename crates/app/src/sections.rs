//! Sidebar sections (OPEN EDITORS, GRAPH, TIMELINE…): collapsible, resized by
//! dragging their edge. State (open, height) lives here and is persisted as
//! "id:open:height,…"; drawing helpers are below.

use gpui::{Div, InteractiveElement as _, ParentElement as _, Stateful, Styled as _, div, px};

use crate::ui::{self, Palette, c, icon};

pub const MIN_H: f32 = 48.;
pub const MAX_H: f32 = 900.;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sections {
    /// id → (open, height). No entry: the caller's default applies.
    items: Vec<(String, bool, Option<f32>)>,
    /// Section being dragged: (id, start y, start height, direction). Direction +1: edge at the bottom
    /// (dragging down grows it), -1: edge at the top (dragging up grows it).
    drag: Option<(String, f32, f32, f32)>,
}

impl Sections {
    fn entry(&mut self, id: &str) -> &mut (String, bool, Option<f32>) {
        if let Some(i) = self.items.iter().position(|e| e.0 == id) {
            return &mut self.items[i];
        }
        self.items.push((id.to_string(), true, None));
        self.items.last_mut().unwrap()
    }

    pub fn is_open(&self, id: &str, default: bool) -> bool {
        self.items.iter().find(|e| e.0 == id).map_or(default, |e| e.1)
    }

    pub fn set_open(&mut self, id: &str, open: bool) {
        self.entry(id).1 = open;
    }

    pub fn toggle(&mut self, id: &str, default: bool) {
        let open = !self.is_open(id, default);
        self.set_open(id, open);
    }

    pub fn height(&self, id: &str, default: f32) -> f32 {
        self.items.iter().find(|e| e.0 == id).and_then(|e| e.2).unwrap_or(default)
    }

    /// The user has dragged this section to a height of their own.
    pub fn has_height(&self, id: &str) -> bool {
        self.items.iter().any(|e| e.0 == id && e.2.is_some())
    }

    pub fn set_height(&mut self, id: &str, h: f32) {
        self.entry(id).2 = Some(h.clamp(MIN_H, MAX_H));
    }

    pub fn begin_drag(&mut self, id: &str, y: f32, current_h: f32, sign: f32) {
        self.drag = Some((id.to_string(), y, current_h, sign));
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Mouse moved to `y`; `true` if the height changed. `max`: tallest the section can be.
    pub fn drag_to(&mut self, y: f32, max: f32) -> bool {
        let Some((id, y0, h0, sign)) = self.drag.clone() else { return false };
        let h = (h0 + (y - y0) * sign).clamp(MIN_H, max.clamp(MIN_H, MAX_H));
        if self.height(&id, f32::NAN) == h {
            return false;
        }
        self.set_height(&id, h);
        true
    }

    /// Drag finished; `true` if there was a drag (should be saved).
    pub fn end_drag(&mut self) -> bool {
        self.drag.take().is_some()
    }

    pub fn encode(&self) -> String {
        self.items
            .iter()
            .map(|(id, open, h)| format!("{id}:{}:{}", u8::from(*open), h.map_or(String::new(), |h| format!("{h:.0}"))))
            .collect::<Vec<_>>()
            .join(",")
    }

    pub fn decode(text: &str) -> Self {
        let items = text
            .split(',')
            .filter_map(|part| {
                let mut f = part.split(':');
                let id = f.next().filter(|s| !s.is_empty())?.to_string();
                let open = f.next()? != "0";
                let h = f.next().and_then(|h| h.parse::<f32>().ok()).map(|h| h.clamp(MIN_H, MAX_H));
                Some((id, open, h))
            })
            .collect();
        Sections { items, drag: None }
    }
}

/// Section header: arrow, name, optional counter. The caller adds click and action buttons.
pub fn header(id: impl Into<gpui::ElementId>, title: &str, count: Option<usize>, open: bool, p: &Palette) -> Stateful<Div> {
    div()
        .id(id)
        .w_full()
        .h(px(theme::layout::ROW_H))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(theme::SPACE[2]))
        .pl(px(theme::SPACE[3]))
        .pr(px(theme::SPACE[3]))
        .border_t_1()
        .border_color(c(p.border))
        .text_size(px(ui::text::XXS.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(c(p.text_muted))
        .cursor_pointer()
        .hover(|d| d.bg(c(p.surface_hover)))
        .child(icon(if open { "chevron-down" } else { "chevron-right" }, 14., p.text_muted))
        .child(div().flex_1().whitespace_nowrap().overflow_hidden().child(crate::i18n::upper(title)))
        .children(count.filter(|n| *n > 0).map(|n| {
            div()
                .flex_none()
                .min_w(px(18.))
                .h(px(16.))
                .px(px(theme::SPACE[2]))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.))
                .bg(c(p.surface_active))
                .text_size(px(ui::text::BADGE.0))
                .text_color(c(p.text))
                .child(n.to_string())
        }))
}

/// Height handle: thin line, accent on hover. The caller wires the mouse press.
pub fn sash(id: impl Into<gpui::ElementId>, p: &Palette) -> Stateful<Div> {
    let accent = p.accent;
    div().id(id).w_full().h(px(4.)).flex_none().cursor(gpui::CursorStyle::ResizeUpDown).hover(move |d| d.bg(c(accent)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips() {
        let mut s = Sections::default();
        s.set_open("graph", false);
        s.set_height("details", 210.);
        let back = Sections::decode(&s.encode());
        assert!(!back.is_open("graph", true));
        assert_eq!(back.height("details", 99.), 210.);
        assert!(back.is_open("unknown", true));
        assert_eq!(back.height("graph", 99.), 99.);
    }

    #[test]
    fn decode_ignores_garbage() {
        let s = Sections::decode(",x,:1:2,a:1:notanumber,b:0:5000");
        assert!(s.is_open("a", false));
        assert_eq!(s.height("a", 7.), 7.);
        assert_eq!(s.height("b", 7.), MAX_H);
    }

    #[test]
    fn dragging_a_top_edge_up_grows_the_section() {
        let mut s = Sections::default();
        s.begin_drag("graph", 500., 200., -1.);
        assert!(s.drag_to(450., 800.));
        assert_eq!(s.height("graph", 0.), 250.);
        assert!(s.drag_to(5000., 800.));
        assert_eq!(s.height("graph", 0.), MIN_H, "dragged far down: clamped to the minimum");
        assert!(s.end_drag());
        assert!(!s.end_drag());
    }
}

#[cfg(test)]
mod hardening {
    use super::*;

    #[test]
    fn random_state_text_and_drags_never_panic() {
        let alphabet = ["a", "b", ":", ",", "0", "1", "99999", "-5", "NaN", "inf", "é", "::", ",,"];
        let mut x = 3u64;
        for _ in 0..2000 {
            let n = 1 + (x % 12) as usize;
            let t: String = (0..n)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    alphabet[(x % alphabet.len() as u64) as usize]
                })
                .collect();
            let mut s = Sections::decode(&t);
            s.begin_drag("a", (x % 2000) as f32, s.height("a", 100.), if x.is_multiple_of(2) { 1. } else { -1. });
            s.drag_to((x % 5000) as f32 - 1000., (x % 1200) as f32);
            let h = s.height("a", 100.);
            assert!((MIN_H..=MAX_H).contains(&h) || h == 100., "{t:?} {h}");
            let _ = Sections::decode(&s.encode());
        }
    }
}
