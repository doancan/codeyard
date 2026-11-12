//! Zoom and pan of the image viewer, as plain numbers (no UI): which scale a picture is drawn at and where it sits in the
//! viewport. `pan` is how far the picture's centre is from the viewport's centre, in pixels, so a picture that fits stays
//! centred and a bigger one can be dragged until its edge meets the viewport's edge.

/// Smallest zoom, unless the picture needs less to fit.
const MIN_SCALE: f32 = 0.05;
/// Largest zoom: 3200 %.
const MAX_SCALE: f32 = 32.;
/// A side drawn longer than this makes the graphics card's coordinates imprecise, so a huge picture stops zooming earlier.
const MAX_DISPLAY_PX: f32 = 60_000.;
/// What the zoom buttons and keys step through.
const STEPS: [f32; 17] = [0.0625, 0.125, 0.25, 0.333, 0.5, 0.667, 0.75, 1., 1.5, 2., 3., 4., 6., 8., 12., 16., 32.];

pub type Size = (f32, f32);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Zoom {
    /// The whole picture in the viewport, never enlarged beyond its own size.
    Fit,
    Scale(f32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewState {
    pub zoom: Zoom,
    pub pan: (f32, f32),
}

impl Default for ViewState {
    fn default() -> Self {
        ViewState { zoom: Zoom::Fit, pan: (0., 0.) }
    }
}

fn usable(size: Size) -> bool {
    size.0 > 0. && size.1 > 0. && size.0.is_finite() && size.1.is_finite()
}

/// The scale at which the whole picture fits (not capped at 100 %).
fn fit_uncapped(image: Size, viewport: Size) -> f32 {
    if !usable(image) || !usable(viewport) {
        return 1.;
    }
    (viewport.0 / image.0).min(viewport.1 / image.1)
}

fn max_scale(image: Size) -> f32 {
    if !usable(image) {
        return MAX_SCALE;
    }
    MAX_SCALE.min(MAX_DISPLAY_PX / image.0.max(image.1)).max(1.)
}

fn min_scale(image: Size, viewport: Size) -> f32 {
    MIN_SCALE.min(fit_uncapped(image, viewport)).max(f32::MIN_POSITIVE)
}

impl ViewState {
    pub fn is_fit(&self) -> bool {
        self.zoom == Zoom::Fit
    }

    /// The scale the picture is drawn at now.
    pub fn scale(&self, image: Size, viewport: Size) -> f32 {
        match self.zoom {
            Zoom::Fit => fit_uncapped(image, viewport).min(1.),
            Zoom::Scale(s) => s,
        }
    }

    /// "100%", "12.5%": the scale for people (a whole number where it is one).
    pub fn label(&self, image: Size, viewport: Size) -> String {
        let percent = self.scale(image, viewport) * 100.;
        if (percent - percent.round()).abs() < 0.05 { format!("{}%", percent.round()) } else { format!("{percent:.1}%") }
    }

    pub fn fit(&mut self) {
        *self = ViewState::default();
    }

    /// 100 %, keeping the middle of what is shown in the middle.
    pub fn actual_size(&mut self, image: Size, viewport: Size) {
        self.set_scale(1., (0., 0.), image, viewport);
    }

    pub fn zoom_in(&mut self, image: Size, viewport: Size) {
        let now = self.scale(image, viewport);
        let next = STEPS.iter().copied().find(|s| *s > now * 1.001).unwrap_or(f32::MAX);
        self.set_scale(next, (0., 0.), image, viewport);
    }

    pub fn zoom_out(&mut self, image: Size, viewport: Size) {
        let now = self.scale(image, viewport);
        let next = STEPS.iter().rev().copied().find(|s| *s < now * 0.999).unwrap_or(0.);
        self.set_scale(next, (0., 0.), image, viewport);
    }

    /// Zoom by `factor` (a wheel turn) so that the point under `anchor` (from the viewport's centre) stays where it is.
    pub fn zoom_by(&mut self, factor: f32, anchor: (f32, f32), image: Size, viewport: Size) {
        if factor.is_finite() && factor > 0. {
            let now = self.scale(image, viewport);
            self.set_scale(now * factor, anchor, image, viewport);
        }
    }

    /// Draw at `scale` (kept within the limits) with the picture's point under `anchor` staying under it.
    pub fn set_scale(&mut self, scale: f32, anchor: (f32, f32), image: Size, viewport: Size) {
        let before = self.scale(image, viewport);
        let after = scale.clamp(min_scale(image, viewport), max_scale(image));
        let pan = self.current_pan();
        // The point of the picture under the anchor, in picture pixels from its centre.
        let at = ((anchor.0 - pan.0) / before, (anchor.1 - pan.1) / before);
        self.zoom = Zoom::Scale(after);
        self.pan = (anchor.0 - at.0 * after, anchor.1 - at.1 * after);
        self.clamp(image, viewport);
    }

    fn current_pan(&self) -> (f32, f32) {
        if self.is_fit() { (0., 0.) } else { self.pan }
    }

    /// Drag by `delta` pixels.
    pub fn pan_by(&mut self, delta: (f32, f32), image: Size, viewport: Size) {
        if self.is_fit() {
            return;
        }
        self.pan = (self.pan.0 + delta.0, self.pan.1 + delta.1);
        self.clamp(image, viewport);
    }

    /// Keep the picture covering the viewport where it is bigger than it, and centred where it is smaller.
    pub fn clamp(&mut self, image: Size, viewport: Size) {
        let shown = (image.0 * self.scale(image, viewport), image.1 * self.scale(image, viewport));
        let limit = |shown: f32, view: f32| ((shown - view) / 2.).max(0.);
        let (lx, ly) = (limit(shown.0, viewport.0), limit(shown.1, viewport.1));
        self.pan = (self.pan.0.clamp(-lx, lx), self.pan.1.clamp(-ly, ly));
        if !self.pan.0.is_finite() || !self.pan.1.is_finite() {
            self.pan = (0., 0.);
        }
    }
}

#[cfg(test)]
#[path = "large_image_zoom_tests.rs"]
mod tests;
