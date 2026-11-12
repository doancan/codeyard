use super::*;

const VIEW: Size = (800., 600.);

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.01
}

#[test]
fn a_picture_starts_fitted_and_centred() {
    let v = ViewState::default();
    assert!(v.is_fit());
    assert_eq!(v.pan, (0., 0.));
}

#[test]
fn fit_shrinks_a_big_picture_to_the_viewport() {
    let v = ViewState::default();
    assert!(near(v.scale((1600., 1200.), VIEW), 0.5));
    assert!(near(v.scale((4000., 400.), VIEW), 0.2), "the wide side decides");
    assert!(near(v.scale((400., 3000.), VIEW), 0.2), "the tall side decides");
}

#[test]
fn fit_never_enlarges_a_small_picture() {
    assert_eq!(ViewState::default().scale((16., 16.), VIEW), 1.);
}

#[test]
fn the_label_is_a_whole_percent_where_it_can_be() {
    let mut v = ViewState::default();
    assert_eq!(v.label((16., 16.), VIEW), "100%");
    v.set_scale(0.125, (0., 0.), (1000., 1000.), VIEW);
    assert_eq!(v.label((1000., 1000.), VIEW), "12.5%");
    assert_eq!(ViewState::default().label((1600., 1200.), VIEW), "50%");
}

#[test]
fn the_buttons_step_through_round_values_from_any_scale() {
    let image = (1600., 1200.);
    let mut v = ViewState::default(); // fit = 50 %
    v.zoom_in(image, VIEW);
    assert_eq!(v.zoom, Zoom::Scale(0.667), "the next step above 50 %");
    v.zoom_in(image, VIEW);
    v.zoom_in(image, VIEW);
    assert_eq!(v.zoom, Zoom::Scale(1.));
    v.zoom_out(image, VIEW);
    assert_eq!(v.zoom, Zoom::Scale(0.75));
    for _ in 0..40 {
        v.zoom_out(image, VIEW);
    }
    assert!(v.scale(image, VIEW) >= MIN_SCALE.min(0.5) - 1e-6, "stops at the smallest zoom");
    for _ in 0..40 {
        v.zoom_in(image, VIEW);
    }
    assert!(near(v.scale(image, VIEW), 32.) || v.scale(image, VIEW) <= MAX_DISPLAY_PX / 1600., "stops at the largest zoom");
}

#[test]
fn a_huge_picture_stops_zooming_before_its_side_is_unreasonably_long() {
    let image = (30_000., 30_000.);
    let mut v = ViewState::default();
    for _ in 0..40 {
        v.zoom_in(image, VIEW);
    }
    assert!(image.0 * v.scale(image, VIEW) <= MAX_DISPLAY_PX + 1., "{}", v.scale(image, VIEW));
}

#[test]
fn a_picture_too_big_for_the_smallest_zoom_can_still_fit() {
    let image = (50_000., 50_000.);
    let v = ViewState::default();
    assert!(v.scale(image, VIEW) < MIN_SCALE);
    let mut z = ViewState::default();
    z.zoom_out(image, VIEW);
    assert!(z.scale(image, VIEW) > 0.);
}

#[test]
fn zooming_with_the_wheel_keeps_the_point_under_the_pointer() {
    let image = (2000., 1500.);
    let mut v = ViewState::default();
    v.actual_size(image, VIEW);
    v.pan = (-200., 100.);
    let anchor = (120., -80.);
    let under = |v: &ViewState| {
        let s = v.scale(image, VIEW);
        ((anchor.0 - v.pan.0) / s, (anchor.1 - v.pan.1) / s)
    };
    let before = under(&v);
    v.zoom_by(1.5, anchor, image, VIEW);
    let after = under(&v);
    assert!(near(before.0, after.0) && near(before.1, after.1), "{before:?} {after:?}");
    assert!(near(v.scale(image, VIEW), 1.5));
}

#[test]
fn zooming_from_fit_leaves_fit_mode() {
    let mut v = ViewState::default();
    v.zoom_by(2., (10., 10.), (1600., 1200.), VIEW);
    assert!(!v.is_fit());
    assert!(near(v.scale((1600., 1200.), VIEW), 1.));
}

#[test]
fn dragging_stops_at_the_edges_of_the_picture() {
    let image = (2000., 1000.);
    let mut v = ViewState::default();
    v.actual_size(image, VIEW);
    v.pan_by((10_000., 10_000.), image, VIEW);
    assert_eq!(v.pan, ((2000. - 800.) / 2., (1000. - 600.) / 2.));
    v.pan_by((-10_000., -10_000.), image, VIEW);
    assert_eq!(v.pan, (-(2000. - 800.) / 2., -(1000. - 600.) / 2.));
}

#[test]
fn a_picture_that_fits_cannot_be_dragged_away() {
    let mut v = ViewState::default();
    v.pan_by((50., 50.), (100., 100.), VIEW);
    assert_eq!(v.pan, (0., 0.));
    v.actual_size((100., 100.), VIEW);
    v.pan_by((50., 50.), (100., 100.), VIEW);
    assert_eq!(v.pan, (0., 0.), "100 % of a small picture is smaller than the viewport");
}

#[test]
fn shrinking_recentres_a_picture_that_now_fits() {
    let image = (2000., 1000.);
    let mut v = ViewState::default();
    v.actual_size(image, VIEW);
    v.pan_by((300., 100.), image, VIEW);
    v.set_scale(0.25, (0., 0.), image, VIEW);
    assert_eq!(v.pan, (0., 0.));
}

#[test]
fn fit_resets_the_pan() {
    let image = (2000., 1000.);
    let mut v = ViewState::default();
    v.actual_size(image, VIEW);
    v.pan_by((300., 100.), image, VIEW);
    v.fit();
    assert_eq!(v, ViewState::default());
}

#[test]
fn a_viewport_that_has_no_size_yet_gives_no_nan() {
    let mut v = ViewState::default();
    for viewport in [(0., 0.), (0., 600.), (f32::NAN, 5.)] {
        v.zoom_in((100., 100.), viewport);
        v.zoom_by(2., (1., 1.), (100., 100.), viewport);
        v.pan_by((5., 5.), (100., 100.), viewport);
        assert!(v.scale((100., 100.), viewport).is_finite(), "{viewport:?}");
        assert!(v.pan.0.is_finite() && v.pan.1.is_finite());
    }
    let v = ViewState::default();
    assert!(v.scale((0., 0.), VIEW).is_finite());
    assert!(v.scale((f32::INFINITY, 1.), VIEW).is_finite());
}

#[test]
fn nonsense_wheel_factors_are_ignored() {
    let mut v = ViewState::default();
    let before = v;
    for f in [0., -1., f32::NAN, f32::INFINITY] {
        v.zoom_by(f, (0., 0.), (100., 100.), VIEW);
    }
    assert_eq!(v.pan, before.pan);
}
