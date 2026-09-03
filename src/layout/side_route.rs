//! Side-route geometry — the right-hand rail around every node.
//!
//! Both back-edges and forward edges whose every V-H-V corridor row
//! would pierce an obstacle travel this rail (see
//! [`crate::layout::connector::Connector::render_side_route`]). The
//! segment builder here is the single geometry source: the obstacle
//! check (`forward_edge_side_routed`) and the drawing code read the
//! same segments, so the two can never drift apart.

use super::geom::{Anchor, Rect};
use super::routing::{Segment, path_intersects_any, snap_outside, vertical_flow_path};

/// Segments of the right side route around every node with the natural
/// leg rows: out of the source's east side to the rail column, down
/// the rail, into the target's east side. Obstacle checks and rendering
/// share these segments, so the two can never drift apart.
pub fn side_route_segments(src: &Rect, tgt: &Rect, rail_x: usize) -> Vec<Segment> {
    side_route_segments_at(src, tgt, rail_x, src.cy(), tgt.cy())
}

/// Segments of the right side route with explicit leg rows (see
/// [`side_route_leg_rows`]). Natural rows reproduce
/// [`side_route_segments`]; a detour exit row sits just outside the
/// source's span, so its horizontal leg starts at the anchor column —
/// directly above the top vertex — and reads as leaving the endpoint's
/// top, the hand-drawn convention for a back edge dodging a sibling.
pub fn side_route_segments_at(
    src: &Rect,
    tgt: &Rect,
    rail_x: usize,
    exit_row: usize,
    entry_row: usize,
) -> Vec<Segment> {
    let src_right = src.right();
    let tgt_right = tgt.right();
    let mut segs: Vec<Segment> = Vec::new();
    if rail_x > src_right {
        let start = if exit_row == src.cy() { src_right } else { src.cx() };
        segs.push(Segment::H { x: start, y: exit_row, len: rail_x - start });
    }
    let (lo, hi) =
        if exit_row <= entry_row { (exit_row, entry_row) } else { (entry_row, exit_row) };
    segs.push(Segment::V { x: rail_x, y: lo, len: hi - lo + 1 });
    if rail_x > tgt_right {
        segs.push(Segment::H { x: tgt_right, y: entry_row, len: rail_x - tgt_right });
    }
    segs
}

/// The row for one side-route leg: its natural row when the eastward
/// span is clear, else the nearest clear detour row (the leg then
/// starts at `detour_x`, just outside the endpoint). No clear row
/// keeps the natural row — the line runs behind the obstacle as
/// before, so the edge is never dropped.
fn leg_row(
    natural_row: usize,
    natural_x: usize,
    detour_x: usize,
    detour_rows: &[usize],
    avoid: &[Rect],
    rail_x: usize,
) -> usize {
    let clear = |x: usize, y: usize| {
        rail_x <= x || !path_intersects_any(&[Segment::H { x, y, len: rail_x - x }], avoid)
    };
    if clear(natural_x, natural_row) {
        return natural_row;
    }
    detour_rows.iter().copied().find(|&row| clear(detour_x, row)).unwrap_or(natural_row)
}

/// Exit and entry rows for a side route whose horizontal legs must
/// clear every avoidance rect. The natural rows are the endpoints'
/// center rows; when a leg's eastward span at its natural row crosses
/// a rect (a same-layer sibling standing between the endpoint and the
/// rail), the leg shifts to the nearest clear row just outside the
/// endpoint — above first, since back edges travel up. `tgt_flex` is
/// false for diamond targets: their east side is a single vertex, so
/// only the center row can receive the arrowhead.
pub fn side_route_leg_rows(
    src: &Rect,
    tgt: &Rect,
    avoid: &[Rect],
    rail_x: usize,
    tgt_flex: bool,
) -> (usize, usize) {
    let detours = |r: &Rect| {
        [r.y.checked_sub(1), Some(r.bottom() + 1), r.y.checked_sub(2), Some(r.bottom() + 2)]
            .into_iter()
            .flatten()
            .collect::<Vec<usize>>()
    };
    let exit_row = leg_row(src.cy(), src.right(), src.cx(), &detours(src), avoid, rail_x);
    let tgt_detours = if tgt_flex { detours(tgt) } else { Vec::new() };
    let entry_row = leg_row(tgt.cy(), tgt.right(), tgt.right(), &tgt_detours, avoid, rail_x);
    (exit_row, entry_row)
}

/// True when a forward edge — downward flow, exiting the source's
/// South anchor and entering the target's North anchor — cannot take
/// any V-H-V path (straight or corridor) without piercing an avoidance
/// rect, while the right side route at `rail_x` is clean. The caller
/// should render such an edge via
/// [`crate::layout::connector::Connector::render_side_route`] instead
/// of the three-segment path, whose detour would silently cut through
/// the obstacles standing in the source column.
pub fn forward_edge_side_routed(src: &Rect, tgt: &Rect, avoid: &[Rect], rail_x: usize) -> bool {
    let source = snap_outside(*src, Anchor::South);
    let target = snap_outside(*tgt, Anchor::North);
    let vertical = vertical_flow_path(source, target, src, tgt, avoid);
    if !path_intersects_any(&vertical, avoid) {
        return false;
    }
    let side = side_route_segments(src, tgt, rail_x);
    !path_intersects_any(&side, avoid)
}
