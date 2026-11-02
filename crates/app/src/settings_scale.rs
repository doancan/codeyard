//! The interface scale setting (`ui.scale`): its range, validation, file format and the Zoom In / Zoom Out steps.

use serde_json::Value;

/// Smallest and largest interface scale (`ui.scale`): 75 % and 200 %.
pub const UI_SCALE_MIN: f32 = 0.75;
pub const UI_SCALE_MAX: f32 = 2.;
/// What a bad `ui.scale` reports; the scale then stays at 100 %.
pub const UI_SCALE_ERROR: &str = crate::i18n::tr("Interface scale must be between 75% and 200%.");
/// The scales Zoom In and Zoom Out step through: 75 %, 80 %, then every 10 % up to 200 %.
pub const UI_SCALE_STEPS: &[f32] = &[0.75, 0.8, 0.9, 1., 1.1, 1.2, 1.3, 1.4, 1.5, 1.6, 1.7, 1.8, 1.9, 2.];

/// A `ui.scale` value: a finite number within `UI_SCALE_MIN..=UI_SCALE_MAX`, rounded to whole percent. Anything else
/// (text, a negative or oversized number) is `None`, and the caller keeps 100 %. Rounding keeps the number of distinct
/// scales finite: glyphs and icons are rasterised and cached once per scale.
pub fn parse_ui_scale(value: &Value) -> Option<f32> {
    let n = value.as_f64().filter(|n| n.is_finite())?;
    (f64::from(UI_SCALE_MIN)..=f64::from(UI_SCALE_MAX)).contains(&n).then(|| ((n * 100.).round() / 100.) as f32)
}

/// A zoom that could not be written to the user's settings (no settings folder, a read-only folder): it stays in force
/// for this session while the file's own `ui.scale` is still `file`, so a settings reload does not undo it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnsavedScale {
    pub scale: f32,
    pub file: f32,
}

/// The session's unsaved zoom, shared by every window.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SessionScale(pub Option<UnsavedScale>);

impl gpui::Global for SessionScale {}

impl SessionScale {
    /// The scale to apply after the user's file was read with `file` as its `ui.scale`. Once the file says something
    /// else (the user edited it, or a later zoom was saved), the file wins and the unsaved zoom is forgotten.
    pub fn resolve(&mut self, file: f32) -> f32 {
        match self.0 {
            Some(unsaved) if unsaved.file == file => unsaved.scale,
            Some(_) => {
                self.0 = None;
                file
            }
            None => file,
        }
    }

    /// The `ui.scale` the user's file had when last read, given the scale in force.
    pub fn file_value(&self, in_force: f32) -> f32 {
        self.0.map_or(in_force, |unsaved| unsaved.file)
    }
}

/// The scale as it is written to the settings file: rounded to whole percent, so 1.1 is stored as `1.1` and not as
/// the `f32` approximation `1.100000023841858`.
pub fn ui_scale_value(scale: f32) -> Value {
    let rounded = (f64::from(scale) * 100.).round() / 100.;
    serde_json::Number::from_f64(rounded).map_or(Value::Null, Value::Number)
}

/// The next scale for Zoom In (`up`) or Zoom Out from `current`. A value between two steps (typed into the file)
/// moves to the neighbouring step; at either end the scale stays where it is.
pub fn step_ui_scale(current: f32, up: bool) -> f32 {
    const EPS: f32 = 0.001;
    let current = if current.is_finite() { current.clamp(UI_SCALE_MIN, UI_SCALE_MAX) } else { 1. };
    let next = if up { UI_SCALE_STEPS.iter().copied().find(|s| *s > current + EPS) } else { UI_SCALE_STEPS.iter().rev().copied().find(|s| *s < current - EPS) };
    next.unwrap_or(current)
}

/// The scale as a whole percentage (1.1 → 110).
pub fn ui_scale_percent(scale: f32) -> u32 {
    (scale * 100.).round() as u32
}
