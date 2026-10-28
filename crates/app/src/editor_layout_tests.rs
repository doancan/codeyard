use super::*;

#[test]
fn editor_grid_splits_in_each_direction_and_preserves_group_identity() {
    let mut layout = Layout::default();
    let right = layout.split(0, Direction::Right).unwrap();
    let below = layout.split(right, Direction::Down).unwrap();
    let above = layout.split(0, Direction::Up).unwrap();
    let left = layout.split(below, Direction::Left).unwrap();
    assert_eq!(layout.groups(), [above, 0, right, left, below]);
    assert_eq!(layout.focused(), left);
    assert!(matches!(layout.root(), Node::Split { axis: Axis::Horizontal, .. }));
    let area: f32 = layout.rects().iter().map(|(_, r)| r.width * r.height).sum();
    assert!((area - 1.).abs() < 0.0001);
    for (_, r) in layout.rects() {
        assert!(r.width > 0. && r.height > 0. && r.x >= 0. && r.y >= 0.);
    }
}

#[test]
fn editor_grid_directional_focus_follows_spatial_neighbors() {
    let mut layout = Layout::default();
    let right = layout.split(0, Direction::Right).unwrap();
    let bottom_right = layout.split(right, Direction::Down).unwrap();
    let bottom_left = layout.split(0, Direction::Down).unwrap();
    layout.focus(0);
    assert_eq!(layout.neighbor(Direction::Right), Some(right));
    assert_eq!(layout.neighbor(Direction::Down), Some(bottom_left));
    assert_eq!(layout.neighbor(Direction::Left), None);
    layout.focus(bottom_right);
    assert_eq!(layout.neighbor(Direction::Left), Some(bottom_left));
    assert_eq!(layout.neighbor(Direction::Up), Some(right));
    assert_eq!(layout.neighbor(Direction::Down), None);
}

#[test]
fn editor_grid_resizes_only_the_requested_split_and_ignores_invalid_values() {
    let mut layout = Layout::default();
    let right = layout.split(0, Direction::Right).unwrap();
    layout.split(right, Direction::Down).unwrap();
    assert!(layout.set_ratio(0, 0.7));
    assert_eq!(layout.rects()[0].1.width, 0.7);
    let saved = layout.clone();
    assert!(!layout.set_ratio(0, f32::NAN));
    assert!(!layout.set_ratio(99, 0.3));
    assert_eq!(layout, saved);
    assert!(layout.set_ratio(0, 5.));
    assert!(matches!(layout.root(), Node::Split { ratio, .. } if *ratio == 0.9));
}

#[test]
fn editor_grid_closes_empty_groups_without_renumbering_survivors_or_losing_focus() {
    let mut layout = Layout::default();
    let right = layout.split(0, Direction::Right).unwrap();
    let lower = layout.split(right, Direction::Down).unwrap();
    assert!(layout.remove(right));
    assert_eq!(layout.groups(), [0, lower]);
    assert_eq!(layout.focused(), lower);
    assert!(layout.remove(lower));
    assert_eq!(layout.groups(), [0]);
    assert_eq!(layout.focused(), 0);
    assert!(!layout.remove(0));
    assert!(!layout.remove(31));
}

#[test]
fn editor_grid_round_trip_preserves_geometry_focus_and_split_ratios() {
    let mut layout = Layout::default();
    let next = layout.split(0, Direction::Left).unwrap();
    layout.split(next, Direction::Up).unwrap();
    layout.set_ratio(0, 0.35);
    let restored = Layout::from_json(&layout.to_json().unwrap()).unwrap();
    assert_eq!(restored, layout);
    assert_eq!(restored.rects(), layout.rects());
}

#[test]
fn editor_grid_rejects_corrupt_versions_duplicates_and_unbounded_input() {
    let mut layout = Layout::default();
    layout.split(0, Direction::Right).unwrap();
    let valid = layout.to_json().unwrap();
    for invalid in [
        valid.replace("\"version\":1", "\"version\":2"),
        valid.replace("\"focused\":1", "\"focused\":31"),
        valid.replace("\"ratio\":0.5", "\"ratio\":0.0"),
        valid.replace("\"id\":1", "\"id\":0"),
        " ".repeat(MAX_JSON_BYTES + 1),
    ] {
        assert_eq!(Layout::from_json(&invalid), Err(Error::Invalid), "{invalid}");
    }
}

#[test]
fn editor_grid_group_limits_leave_the_current_layout_unchanged() {
    let mut layout = Layout::default();
    for i in 1..MAX_GROUPS {
        layout.split((i - 1) as u8, Direction::Right).unwrap();
    }
    let saved = layout.clone();
    assert_eq!(layout.split(0, Direction::Down), Err(Error::GroupLimit));
    assert_eq!(layout, saved);
    assert!(!layout.focus(99));
    assert_eq!(layout.split(99, Direction::Down), Err(Error::MissingGroup));
    assert_eq!(Layout::from_json(&layout.to_json().unwrap()).unwrap(), layout);
}

#[test]
fn editor_grid_mixed_operations_preserve_a_nonoverlapping_complete_partition() {
    let mut layout = Layout::default();
    let mut seed = 0x6179_21b3_u32;
    for _ in 0..500 {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let groups = layout.groups();
        let group = groups[seed as usize % groups.len()];
        match seed % 4 {
            0 => {
                let _ = layout.split(group, [Direction::Left, Direction::Right, Direction::Up, Direction::Down][(seed >> 3) as usize % 4]);
            }
            1 => {
                layout.remove(group);
            }
            2 => {
                layout.focus(group);
            }
            _ => {
                let mut splits = HashSet::new();
                collect_split_ids(layout.root(), &mut splits);
                for id in splits {
                    layout.set_ratio(id, (seed % 100) as f32 / 100.);
                }
            }
        }
        let rects = layout.rects();
        assert!(rects.iter().any(|(id, _)| *id == layout.focused()));
        let area: f32 = rects.iter().map(|(_, r)| r.width * r.height).sum();
        assert!((area - 1.).abs() < 0.0001);
        for (i, (_, a)) in rects.iter().enumerate() {
            for (_, b) in &rects[i + 1..] {
                let overlap_x = (a.x + a.width).min(b.x + b.width) - a.x.max(b.x);
                let overlap_y = (a.y + a.height).min(b.y + b.height) - a.y.max(b.y);
                assert!(overlap_x <= 0.00001 || overlap_y <= 0.00001);
            }
        }
        assert_eq!(Layout::from_json(&layout.to_json().unwrap()).unwrap(), layout);
    }
}
