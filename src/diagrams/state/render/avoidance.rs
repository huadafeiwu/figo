//! Collision avoidance helpers for transition drawing: vertical-leg
//! rerouting around intermediate boxes (scene B+G), label box avoidance
//! (scene D), and sibling-junction avoidance (root cause 3 cleanup).

use crate::diagrams::state::layout::StateLayout;
use crate::render::widget::Rect;

/// Scene B+G: If a vertical leg at column `cx` would pass through an
/// intermediate box (not from/to), reroute it to the box's nearest
/// left or right edge so the leg goes around the box instead of
/// visually passing through it.
pub(super) fn reroute_leg_around_boxes(
    cx: usize,
    anchor_y: usize,
    route_y: usize,
    from: Rect,
    to: Rect,
    all_layouts: &[StateLayout],
) -> usize {
    let lo = anchor_y.min(route_y);
    let hi = anchor_y.max(route_y);
    for layout in all_layouts {
        let r = &layout.rect;
        if r == &from || r == &to {
            continue;
        }
        // Only reroute if the box is strictly between the leg's endpoints
        // (not at the same layer as from or to).
        if r.y <= from.y || r.y >= to.y {
            continue;
        }
        // Skip if cx is the center of this box — the leg passing through
        // to reach a box below at the same column is correct UML behavior.
        let box_cx = r.x + r.w / 2;
        if cx == box_cx {
            continue;
        }
        // Does the leg's y range [lo, hi] overlap the box's y range?
        if lo < r.bottom() && hi >= r.y && cx >= r.x && cx < r.right() {
            // Leg passes through this box. Reroute to nearest edge.
            let dist_left = cx.saturating_sub(r.x);
            let dist_right = r.right().saturating_sub(cx + 1);
            return if dist_left <= dist_right { r.x.saturating_sub(1) } else { r.right() };
        }
    }
    cx
}

/// Scene D: Adjust label_x so the label [label_x, label_x+lw) does not
/// overlap any box at row label_y. If it does, shift it left or right
/// to the nearest non-overlapping position. A shift that would push the
/// label past the right canvas edge is never taken — out-of-bounds
/// writes are silently dropped by the canvas, which would lose label
/// characters.
pub(super) fn avoid_box_x(
    mut label_x: usize,
    lw: usize,
    label_y: usize,
    from: Rect,
    to: Rect,
    all_layouts: &[StateLayout],
    canvas_width: usize,
) -> usize {
    for layout in all_layouts {
        let r = &layout.rect;
        if r == &from || r == &to {
            continue;
        }
        // Does label overlap this box at this y? label_end is recomputed
        // each iteration — label_x may already have moved for an earlier
        // box in this same loop.
        let label_end = label_x + lw;
        if label_y >= r.y && label_y < r.bottom() && label_end > r.x && label_x < r.right() {
            // Shifting right past the box is only valid when the label
            // still fits inside the canvas. Shifting left always fits:
            // the label ends at r.x, which is inside the canvas.
            let shift_right_ok = r.right() + lw <= canvas_width;
            let shift_right = r.right();
            // Try shifting left before the box.
            let shift_left = r.x.saturating_sub(lw);
            // Pick the valid shift closer to original position.
            if shift_right_ok
                && shift_right.saturating_sub(label_x) <= label_x.saturating_sub(shift_left)
            {
                label_x = shift_right;
            } else {
                label_x = shift_left;
            }
        }
    }
    label_x
}

/// Root cause 3 cleanup: shift [label_x, label_x+lw) off any junction
/// column in `cols`. A label covering a sibling's junction cell hides
/// its `+` and corridor start (Label layer > Connector layer, the write
/// is dropped). Shifts toward the side closer to the current position.
pub(super) fn shift_off_junctions(mut label_x: usize, lw: usize, cols: &[usize]) -> usize {
    for _ in 0..=cols.len() {
        let Some(&c) = cols.iter().find(|&&c| label_x <= c && c < label_x + lw) else {
            break;
        };
        let new_x = if c >= label_x + lw / 2 { c.saturating_sub(lw) } else { c + 1 };
        if new_x == label_x {
            break;
        }
        label_x = new_x;
    }
    label_x
}
