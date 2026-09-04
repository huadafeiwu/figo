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
    side_route_segments_at(src, tgt, rail_x, src.cy(), tgt.cy(), None)
}

/// A side-route rendering plan: everything the connector needs beyond
/// the two endpoint rects — the rail column the caller reserved for
/// this edge, the leg rows (see [`side_route_leg_rows`]), and the
/// back-edge readability directives. [`SideRoutePlan::natural`]
/// reproduces the plain rail ride; the flowchart layout resolves the
/// rest per edge.
#[derive(Clone)]
pub struct SideRoutePlan {
    /// The rail column: forward side-routed edges ride
    /// `side_route_column`, back-edges a dedicated rail `RAIL_OFFSET`
    /// columns further right, so a rail segment is never shared by two
    /// edges running opposite directions.
    pub rail_x: usize,
    pub exit_row: usize,
    pub entry_row: usize,
    /// Back-edge labels embed in the exit horizontal leg next to the
    /// source instead of riding the rail end.
    pub label_near_source: bool,
    /// Columns of same-layer siblings' incoming arrowheads (and their
    /// legs) that exit-leg runs and labels must dodge.
    pub sibling_arrows: Vec<usize>,
    /// Reroute the exit leg around a sibling's arrowhead (see
    /// [`side_route_exit_jog`]): `(corner column, upper row)`.
    pub jog: Option<(usize, usize)>,
}

impl SideRoutePlan {
    /// The plain side ride on `rail_x` with natural leg rows.
    pub fn natural(rail_x: usize, src: &Rect, tgt: &Rect) -> Self {
        SideRoutePlan {
            rail_x,
            exit_row: src.cy(),
            entry_row: tgt.cy(),
            label_near_source: false,
            sibling_arrows: Vec::new(),
            jog: None,
        }
    }
}

/// Columns between the jog corner and the dodged arrowhead: one blank
/// column (a dash there would read as a T-junction into the arrow) plus
/// one dash (so the lower run still reads as a line, not a stub).
const JOG_ARROW_CLEARANCE: usize = 2;

/// The exit leg's jog around a same-layer sibling's incoming arrowhead:
/// `(corner column, upper row)`. Only the north-detour exit row
/// (`src.y - 1`) needs this: every same-layer sibling's incoming arrow
/// lands on that same row (same-layer nodes share `y`).
///
/// The upper row is found by walking up from `src.y - 2`, the first row
/// above the arrow row: with the usual layer gap (auto stride >= 4) that
/// row carries nothing but plain vertical legs, which the run crosses as
/// clean `+` junctions. A row whose `[corner, rail]` stretch overlaps an
/// occupied horizontal run in `occupied` — fork corridors, their label
/// blocks, riding blocks (a tight three-row layer gap puts the
/// layer-above's fork corridor exactly on the first candidate) — is
/// skipped, because merging into it would chain the two edges into one
/// line. A row that would pierce a rect in `blockers` ends the search:
/// a higher run only burrows deeper into the layer above. `None` when no
/// arrow needs dodging, the corner would collide with the source column,
/// or every row up to the canvas top is busy or blocked.
pub fn side_route_exit_jog(
    src: &Rect,
    rail_x: usize,
    exit_row: usize,
    sibling_arrow_cols: &[usize],
    occupied: &[(usize, usize, usize)],
    blockers: &[Rect],
) -> Option<(usize, usize)> {
    if src.y < 2 || exit_row + 1 != src.y {
        return None;
    }
    let start = src.cx();
    let first =
        sibling_arrow_cols.iter().copied().filter(|&a| a > start + 1 && a < rail_x).min()?;
    let jog = first - JOG_ARROW_CLEARANCE;
    if jog <= start {
        return None;
    }
    let mut up = src.y - 2;
    loop {
        let run = Segment::H { x: jog, y: up, len: rail_x - jog };
        if path_intersects_any(&[run], blockers) {
            return None;
        }
        if !occupied.iter().any(|&(r, lo, hi)| r == up && hi >= jog && lo <= rail_x) {
            return Some((jog, up));
        }
        if up == 0 {
            return None;
        }
        up -= 1;
    }
}

/// Segments of the right side route with explicit leg rows (see
/// [`side_route_leg_rows`]). Natural rows reproduce
/// [`side_route_segments`]; a detour exit row sits just outside the
/// source's span, so its horizontal leg starts at the anchor column —
/// directly above the top vertex — and reads as leaving the endpoint's
/// top, the hand-drawn convention for a back edge dodging a sibling.
/// `jog` reroutes the exit leg around a sibling's arrowhead (see
/// [`side_route_exit_jog`]): east on the detour row to the corner
/// column, up to the chosen upper row, then east to the rail.
pub fn side_route_segments_at(
    src: &Rect,
    tgt: &Rect,
    rail_x: usize,
    exit_row: usize,
    entry_row: usize,
    jog: Option<(usize, usize)>,
) -> Vec<Segment> {
    let src_right = src.right();
    let tgt_right = tgt.right();
    let mut segs: Vec<Segment> = Vec::new();
    if let Some((jog_col, up_row)) = jog {
        let start = src.cx();
        if jog_col > start {
            // Spans include their corner columns (the inclusive
            // convention of `build_three_segment`): a corner cell is
            // covered by both its segments, which the crossing pass's
            // arm validation relies on.
            segs.push(Segment::H { x: start, y: exit_row, len: jog_col - start + 1 });
        }
        segs.push(Segment::V { x: jog_col, y: up_row, len: exit_row - up_row + 1 });
        if rail_x > jog_col {
            segs.push(Segment::H { x: jog_col, y: up_row, len: rail_x - jog_col + 1 });
        }
        let (lo, hi) = if up_row <= entry_row { (up_row, entry_row) } else { (entry_row, up_row) };
        segs.push(Segment::V { x: rail_x, y: lo, len: hi - lo + 1 });
        if rail_x > tgt_right {
            segs.push(Segment::H { x: tgt_right, y: entry_row, len: rail_x - tgt_right + 1 });
        }
        return segs;
    }
    if rail_x > src_right {
        let start = if exit_row == src.cy() { src_right } else { src.cx() };
        segs.push(Segment::H { x: start, y: exit_row, len: rail_x - start + 1 });
    }
    let (lo, hi) =
        if exit_row <= entry_row { (exit_row, entry_row) } else { (entry_row, exit_row) };
    segs.push(Segment::V { x: rail_x, y: lo, len: hi - lo + 1 });
    if rail_x > tgt_right {
        segs.push(Segment::H { x: tgt_right, y: entry_row, len: rail_x - tgt_right + 1 });
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
        // Above: the two rows directly over the top. Below: skip one
        // row past the bottom — `Rect::bottom()` is exclusive, so
        // bottom() itself is the row of the endpoint's own south
        // arrowhead (snap_outside(South)), which a detour leg must not
        // overwrite.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_jog_walks_up_past_occupied_rows() {
        // Source at y=10 (cx=14); the sibling arrow sits at column 20 on
        // the exit row 9; the rail is at 30, so the corner lands at
        // 20 - JOG_ARROW_CLEARANCE = 18. Rows 8 and 7 are occupied
        // across the run's stretch — the jog must hop to row 6.
        let src = Rect::new(10, 10, 9, 7);
        let jog = side_route_exit_jog(&src, 30, 9, &[20], &[(8, 18, 30), (7, 18, 30)], &[]);
        assert_eq!(jog, Some((18, 6)));
    }

    #[test]
    fn exit_jog_stops_at_node_blocker() {
        // Same geometry, but a node stands over rows 5-7 in the run's
        // stretch: row 8 is occupied and row 7 would pierce the node —
        // the search must give up rather than burrow into the layer
        // above.
        let src = Rect::new(10, 10, 9, 7);
        let blocker = Rect::new(16, 5, 12, 3);
        let jog = side_route_exit_jog(&src, 30, 9, &[20], &[(8, 18, 30)], &[blocker]);
        assert_eq!(jog, None);
    }

    #[test]
    fn exit_jog_declines_when_nothing_to_dodge() {
        let src = Rect::new(10, 10, 9, 7);
        // No sibling arrow east of the source column.
        assert_eq!(side_route_exit_jog(&src, 30, 9, &[], &[], &[]), None);
        // Arrow so close that the corner would collide with the source
        // column (jog == start).
        assert_eq!(side_route_exit_jog(&src, 30, 9, &[16], &[], &[]), None);
    }
}
