//! Unit tests for the label/leg avoidance helpers.

use super::avoidance::avoid_box_x;
use crate::diagrams::state::layout::StateLayout;
use crate::diagrams::state::types::StateType;
use crate::render::widget::Rect;

fn state_layout(id: &str, x: usize, y: usize, w: usize, h: usize) -> StateLayout {
    StateLayout {
        id: id.into(),
        label: id.into(),
        state_type: StateType::Simple,
        rect: Rect::new(x, y, w, h),
    }
}

#[test]
fn avoid_box_x_right_shift_never_overflows_canvas() {
    // Box near the right edge: shifting right would push a 20-wide
    // label past the canvas (90 + 20 > 100) — must shift left instead.
    // The old code returned 90, silently dropping the last 10 chars.
    let from = Rect::new(0, 0, 10, 3);
    let to = Rect::new(0, 10, 10, 3);
    let layouts = vec![
        state_layout("from", 0, 0, 10, 3),
        state_layout("to", 0, 10, 10, 3),
        state_layout("obs", 60, 4, 30, 3),
    ];
    let x = avoid_box_x(62, 20, 5, from, to, &layouts, 100);
    assert!(x + 20 <= 100, "label overflows canvas: x={x}, lw=20, canvas=100");
    assert_eq!(x, 40, "should shift left before the box at x=60");
}

#[test]
fn avoid_box_x_right_shift_when_it_fits() {
    // Shifting right past the box is closer and fits: take it.
    let from = Rect::new(0, 0, 10, 3);
    let to = Rect::new(0, 10, 10, 3);
    let layouts = vec![
        state_layout("from", 0, 0, 10, 3),
        state_layout("to", 0, 10, 10, 3),
        state_layout("obs", 20, 4, 8, 3),
    ];
    let x = avoid_box_x(22, 8, 5, from, to, &layouts, 100);
    assert_eq!(x, 28, "closer valid shift is right past the box (right edge 28)");
}

#[test]
fn avoid_box_x_prefers_left_when_right_does_not_fit() {
    // Right edge of the box + lw exceeds the canvas even though the
    // right shift is closer — the left shift must win.
    let from = Rect::new(0, 0, 10, 3);
    let to = Rect::new(0, 10, 10, 3);
    let layouts = vec![
        state_layout("from", 0, 0, 10, 3),
        state_layout("to", 0, 10, 10, 3),
        state_layout("obs", 70, 4, 8, 3),
    ];
    let x = avoid_box_x(71, 8, 5, from, to, &layouts, 80);
    assert!(x + 8 <= 80, "label overflows canvas: x={x}");
    assert_eq!(x, 62, "right shift (78+8>80) invalid, left shift to 70-8=62");
}

#[test]
fn avoid_box_x_no_overlap_untouched() {
    // Label doesn't overlap any box: position unchanged.
    let from = Rect::new(0, 0, 10, 3);
    let to = Rect::new(0, 10, 10, 3);
    let layouts = vec![
        state_layout("from", 0, 0, 10, 3),
        state_layout("to", 0, 10, 10, 3),
        state_layout("obs", 50, 4, 8, 3),
    ];
    let x = avoid_box_x(20, 8, 5, from, to, &layouts, 100);
    assert_eq!(x, 20);
}
