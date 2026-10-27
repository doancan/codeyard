//! Quiet, interruptible feedback shared by draggable panel and section edges.

use std::time::{Duration, Instant};

use gpui::{App, InteractiveElement as _, IntoElement, RenderOnce, StatefulInteractiveElement as _, Styled as _, Window, div};

#[derive(IntoElement)]
pub struct Highlight {
    accent: u32,
    dragging: bool,
}

pub fn highlight(accent: u32, dragging: bool) -> Highlight {
    Highlight { accent, dragging }
}

struct Fade {
    hovered: bool,
    from: f32,
    to: f32,
    started: Instant,
    duration: Duration,
}

impl Fade {
    fn value(&self, now: Instant) -> f32 {
        let t = (now.duration_since(self.started).as_secs_f32() / self.duration.as_secs_f32()).min(1.);
        self.from + (self.to - self.from) * (1. - (1. - t).powi(3))
    }

    fn target(&mut self, target: f32, now: Instant) {
        if self.to != target {
            self.from = self.value(now);
            self.duration = Duration::from_millis(if target > self.to { 130 } else { 100 });
            self.to = target;
            self.started = now;
        }
    }
}

impl RenderOnce for Highlight {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        // State belongs to the parent sash's element path, so unrelated rerenders do not restart it.
        let fade = window.use_state(cx, |_, _| Fade { hovered: false, from: 0., to: 0., started: Instant::now(), duration: Duration::from_millis(130) });
        let (alpha, moving) = fade.update(cx, |fade, _| {
            let now = Instant::now();
            if self.dragging {
                // A grabbed edge stays unmistakable even when the pointer leaves its hitbox.
                fade.from = 1.;
                fade.to = 1.;
            } else {
                fade.target(if fade.hovered { 0.65 } else { 0. }, now);
            }
            (fade.value(now), fade.from != fade.to && now.duration_since(fade.started) < fade.duration)
        });
        if moving {
            window.request_animation_frame();
        }
        // Only color changes: no spatial motion, layout animation or idle frame loop.
        div().id("sash-highlight").size_full().bg(crate::ui::ca(self.accent, alpha)).on_hover(move |hovered, _, cx| {
            fade.update(cx, |fade, cx| {
                fade.hovered = *hovered;
                cx.notify();
            });
        })
    }
}
