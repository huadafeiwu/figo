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

/// Segments of the right side route around every node: out of the
/// source's east side to the rail column, down the rail, into the
/// target's east side. [`crate::layout::connector::Connector::render_side_route`]
/// draws exactly these segments, so obstacle checks and rendering share
/// one geometry source and cannot drift apart.
pub fn side_route_segments(src: &Rect, tgt: &Rect, rail_x: usize) -> Vec<Segment> {
    let src_right = src.right();
    let tgt_right = tgt.right();
    let src_cy = src.cy();
    let tgt_cy = tgt.cy();
    let mut segs: Vec<Segment> = Vec::new();
    if rail_x > src_right {
        segs.push(Segment::H { x: src_right, y: src_cy, len: rail_x - src_right });
    }
    let (lo, hi) = if src_cy < tgt_cy { (src_cy, tgt_cy) } else { (tgt_cy, src_cy) };
    segs.push(Segment::V { x: rail_x, y: lo, len: hi - lo + 1 });
    if rail_x > tgt_right {
        segs.push(Segment::H { x: tgt_right, y: tgt_cy, len: rail_x - tgt_right });
    }
    segs
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
