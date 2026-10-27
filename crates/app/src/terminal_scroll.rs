//! Preserve fractional wheel motion across events and calibrate it in terminal rows.

use alacritty_terminal::term::TermMode;
use gpui::{ScrollWheelEvent, TouchPhase, px};

#[derive(Default)]
pub struct WheelScroll {
    remainder: f32,
    mode: Option<TermMode>,
}

impl WheelScroll {
    pub fn lines(&mut self, event: &ScrollWheelEvent, line_h: f32, sensitivity: f32, mode: TermMode) -> i32 {
        // Mouse reporting and the alternate screen must never inherit a fraction from scrollback.
        let mode = mode & (TermMode::MOUSE_MODE | TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL | TermMode::APP_CURSOR);
        let delta = event.delta.pixel_delta(px(line_h)).y / px(line_h) * sensitivity;
        if !delta.is_finite() {
            return 0;
        }
        if matches!(event.touch_phase, TouchPhase::Started) || self.mode != Some(mode) || self.remainder * delta < 0. {
            self.remainder = 0.;
        }
        self.mode = Some(mode);
        let total = (self.remainder + delta).clamp(-10_000., 10_000.);
        let lines = total.trunc();
        self.remainder = total - lines;
        lines as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Modifiers, ScrollDelta, point};

    fn wheel(delta: f32, phase: TouchPhase) -> ScrollWheelEvent {
        ScrollWheelEvent { position: point(px(0.), px(0.)), delta: ScrollDelta::Lines(point(0., delta)), modifiers: Modifiers::none(), touch_phase: phase }
    }

    #[test]
    fn wheel_calibration_preserves_motion_and_resets_between_gestures_and_modes() {
        let mut scroll = WheelScroll::default();
        let mode = TermMode::empty();
        assert_eq!(scroll.lines(&wheel(3., TouchPhase::Moved), 20., 2., mode), 6);
        assert_eq!(scroll.lines(&wheel(0.4, TouchPhase::Moved), 20., 1., mode), 0);
        assert_eq!(scroll.lines(&wheel(0.4, TouchPhase::Moved), 20., 1., mode), 0);
        assert_eq!(scroll.lines(&wheel(0.4, TouchPhase::Moved), 20., 1., mode), 1);
        assert_eq!(scroll.lines(&wheel(-0.6, TouchPhase::Moved), 20., 1., mode), 0);
        assert_eq!(scroll.lines(&wheel(-0.6, TouchPhase::Moved), 20., 1., mode), -1, "reversing must respond immediately");
        assert_eq!(scroll.lines(&wheel(0.8, TouchPhase::Moved), 20., 1., mode), 0);
        assert_eq!(scroll.lines(&wheel(0.4, TouchPhase::Started), 20., 1., mode), 0, "new gesture starts fresh");
        assert_eq!(scroll.lines(&wheel(0.8, TouchPhase::Moved), 20., 1., TermMode::ALT_SCREEN), 0, "do not leak into a terminal program");
        assert_eq!(scroll.lines(&wheel(f32::NAN, TouchPhase::Moved), 20., 1., mode), 0);
    }
}
