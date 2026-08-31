//! Fork-riding label placement for flowcharts.
//!
//! Riding blocks are placed sequentially in connection declaration
//! order, BEFORE the canvas is sized. Each rider's candidate rows are
//! the clear stretches of its column span — cut by intermediate boxes
//! and by the already-placed riding blocks sharing its columns; when no
//! on-line stretch fits the whole block, the rider falls back beside
//! the blocker cluster. Because placement happens pre-canvas, every
//! block's bottom row participates in the canvas-height calculation
//! exactly like a box bottom — a block can never be silently dropped
//! past the canvas edge.
//!
//! Capacity ladder (every tier geometric, no fixed offsets):
//! 1. an on-line stretch (between the sibling blocks and the target's
//!    top) with room for the block plus its `|` rails, nearest the
//!    target;
//! 2. the same without rails;
//! 3. beside the blocker cluster — the wider free span to either side
//!    of the columns occupied by the blocking boxes and the target,
//!    rows bounded by the pre-riding canvas height, sitting as close to
//!    the target's top row (and then to the leg column) as possible;
//! 4. the widest clear stretch of any candidate span, block at its top
//!    (the canvas grows to keep the block's rows).

use crate::layout::geom::Rect;
use crate::layout::{RidingLabel, beside_line_label_avail, riding_placement_cols};
use crate::text::wrap_label;

/// An already-placed riding block: later riders treat its rows as
/// blocked wherever their column spans intersect — the same rows are
/// never handed to two riders.
pub(crate) struct PlacedRider {
    /// Widest-case column span of the block (half-open).
    pub span: (usize, usize),
    /// Block rows (half-open).
    pub rows: (usize, usize),
}

/// A placed riding block: the directive the connector consumes, plus
/// the geometry later riders and the canvas sizing need.
pub(crate) struct RidingPlacement {
    pub riding: RidingLabel,
    /// Exclusive bottom row of the block (`block_top + line count`) —
    /// for canvas-height accounting.
    pub bottom: usize,
    /// Widest-case column span of the block (half-open) — for later
    /// riders' blockers.
    pub span: (usize, usize),
}

/// One rider's placement request, bundled to keep the argument list
/// small (the same packing pattern as the state side's `DrawStage`).
pub(crate) struct RidingRequest<'a> {
    pub label: &'a str,
    pub from_rect: Rect,
    pub to_rect: Rect,
    /// Same-source corridor siblings (far leg column, label block
    /// bottom row) — the reason the label rides at all.
    pub sibs: &'a [(usize, usize)],
    /// Every node rect except the rider's own endpoints.
    pub boxes: &'a [Rect],
    /// Riders already placed in this pass, declaration order.
    pub placed: &'a [PlacedRider],
    /// User-specified width: the hard wrap limit.
    pub user_width: usize,
    /// Canvas width (the right bound for beside-the-line spans).
    pub canvas_w: usize,
    /// Pre-riding canvas height: the row ceiling for beside-the-line
    /// placement.
    pub row_limit: usize,
}

/// Best tier-3 candidate so far (beside the blocker cluster).
struct BesideCandidate {
    /// The stretch fits the block plus both `|` rails.
    rails: bool,
    /// Distance from the block to the target's top row.
    dist_ty: usize,
    /// Distance from the span's center to the leg column — attribution
    /// keeps the block near the edge being labeled.
    dist_leg: usize,
    block_top: usize,
    span: (usize, usize),
}

impl BesideCandidate {
    fn better(&self, rails: bool, dist_ty: usize, dist_leg: usize) -> bool {
        if rails != self.rails {
            rails
        } else if dist_ty != self.dist_ty {
            dist_ty < self.dist_ty
        } else {
            dist_leg < self.dist_leg
        }
    }
}

/// The widest clear stretch seen across all candidate spans, for tier 4.
struct WidestStretch {
    height: usize,
    block_top: usize,
    span: (usize, usize),
}

fn note_widest(widest: &mut Option<WidestStretch>, stretch: (usize, usize), span: (usize, usize)) {
    let height = stretch.1 - stretch.0;
    if widest.as_ref().is_none_or(|w| height > w.height) {
        *widest = Some(WidestStretch { height, block_top: stretch.0, span });
    }
}

/// Place one riding label (see the module docs for the capacity
/// ladder).
pub(crate) fn place_riding_label(req: &RidingRequest<'_>) -> RidingPlacement {
    let (sy, ty) = (req.from_rect.bottom(), req.to_rect.y);
    let leg_cx = req.from_rect.x + req.from_rect.w / 2;
    // The rider starts below every sibling's corridor label block.
    let below_row = req.sibs.iter().map(|&(_, bb)| bb + 1).max().unwrap_or(sy + 1);
    let far_cols: Vec<usize> = req.sibs.iter().map(|&(c, _)| c).collect();
    let avail = beside_line_label_avail(leg_cx, req.user_width);
    let (wrap_w, center_x) = riding_placement_cols(&far_cols, leg_cx, req.canvas_w, avail);
    let wrapped = wrap_label(req.label, wrap_w);
    let n = wrapped.line_count;
    // The block's widest-case span centered on the ladder's center
    // column (the leg, or the ladder's own beside-span when it stepped
    // off the line).
    let lo = center_x.saturating_sub(wrap_w / 2);
    let span = (lo, lo + wrap_w);

    // Tiers 1/2 — on the ladder's span, between the sibling blocks and
    // the target's top: prefer the stretch nearest the target with room
    // for the block plus its `|` rails, then without rails.
    let blockers = blockers_overlapping(span, req.boxes, req.placed);
    let stretches = clear_stretches(below_row, ty, &blockers);
    if let Some(&(slo, shi)) = stretches
        .iter()
        .rev()
        .find(|&&(lo, hi)| hi - lo >= n + 2)
        .or_else(|| stretches.iter().rev().find(|&&(lo, hi)| hi - lo >= n))
    {
        let block_top = slo + (shi - slo).saturating_sub(n) / 2;
        return RidingPlacement {
            riding: RidingLabel { wrap_w, center_x, block_top },
            bottom: block_top + n,
            span,
        };
    }

    // Tier 3 — beside the blocker cluster: escape the columns occupied
    // by the boxes that blocked the on-line attempt (plus the target
    // box) to the left and right. Rows run to the pre-riding canvas
    // height — beside the cluster, below-target rows are free whenever
    // no box or placed block occupies them.
    let mut cluster_left = req.to_rect.x;
    let mut cluster_right = req.to_rect.right();
    for r in req.boxes {
        if r.y < ty && r.bottom() > below_row && r.right() > span.0 && r.x < span.1 {
            cluster_left = cluster_left.min(r.x);
            cluster_right = cluster_right.max(r.right());
        }
    }
    let left_w = cluster_left.min(wrap_w);
    let right_w = req.canvas_w.saturating_sub(cluster_right).min(wrap_w);
    let left_span = (cluster_left - left_w, cluster_left);
    let right_span = (cluster_right, cluster_right + right_w);
    // Wider side first for determinism; the tier's own preference
    // (rails, then closeness to the target top, then to the leg column)
    // does the ordering.
    let mut beside: Vec<(usize, usize)> =
        if left_w >= right_w { vec![left_span, right_span] } else { vec![right_span, left_span] };
    // A side is usable only when its span holds the block's widest
    // wrapped line (otherwise a centered line would spill past the span
    // and off the canvas).
    beside.retain(|&(s_lo, s_hi)| s_hi > s_lo && s_hi - s_lo >= wrapped.max_width);

    let mut tier3: Option<BesideCandidate> = None;
    let mut widest: Option<WidestStretch> = None;
    for &(s_lo, s_hi) in &stretches {
        note_widest(&mut widest, (s_lo, s_hi), span);
    }
    for bs in beside {
        let blockers = blockers_overlapping(bs, req.boxes, req.placed);
        for (s_lo, s_hi) in clear_stretches(below_row, req.row_limit, &blockers) {
            note_widest(&mut widest, (s_lo, s_hi), bs);
            let height = s_hi - s_lo;
            if height < n {
                continue;
            }
            let rails = height >= n + 2;
            // Sit as close to the target's top row as the stretch
            // allows, then prefer the span nearer the leg column.
            let top = ty.saturating_sub(n).clamp(s_lo, s_hi - n);
            let dist_ty = if top >= ty { top - ty } else { ty.saturating_sub(top + n) };
            let span_center = bs.0 + (bs.1 - bs.0) / 2;
            let dist_leg = span_center.abs_diff(leg_cx);
            let better = match &tier3 {
                None => true,
                Some(best) => best.better(rails, dist_ty, dist_leg),
            };
            if better {
                tier3 =
                    Some(BesideCandidate { rails, dist_ty, dist_leg, block_top: top, span: bs });
            }
        }
    }
    if let Some(best) = tier3 {
        let center = best.span.0 + (best.span.1 - best.span.0) / 2;
        return RidingPlacement {
            riding: RidingLabel { wrap_w, center_x: center, block_top: best.block_top },
            bottom: best.block_top + n,
            span: best.span,
        };
    }

    // Tier 4 — nothing fits anywhere: the widest clear stretch of any
    // candidate span, block at its top. The canvas height accounts for
    // the block's bottom, so no row is silently dropped.
    let (block_top, f_span, f_center) = match widest {
        Some(w) => (w.block_top, w.span, w.span.0 + (w.span.1 - w.span.0) / 2),
        None => (below_row, span, center_x), // degenerate: no clear stretch at all
    };
    RidingPlacement {
        riding: RidingLabel { wrap_w, center_x: f_center, block_top },
        bottom: block_top + n,
        span: f_span,
    }
}

/// Row ranges blocked for a column span: every box overlapping the
/// span's columns, plus every placed riding block whose span intersects
/// it.
fn blockers_overlapping(
    span: (usize, usize),
    boxes: &[Rect],
    placed: &[PlacedRider],
) -> Vec<(usize, usize)> {
    let mut blockers: Vec<(usize, usize)> = boxes
        .iter()
        .filter(|r| r.right() > span.0 && r.x < span.1)
        .map(|r| (r.y, r.bottom()))
        .collect();
    for p in placed {
        if p.span.1 > span.0 && p.span.0 < span.1 {
            blockers.push(p.rows);
        }
    }
    blockers
}

/// Split `[row_lo, row_hi)` into maximal stretches clear of every
/// blocked row range (each clipped to the window first).
fn clear_stretches(
    row_lo: usize,
    row_hi: usize,
    blockers: &[(usize, usize)],
) -> Vec<(usize, usize)> {
    let mut blocked: Vec<(usize, usize)> = blockers
        .iter()
        .map(|&(lo, hi)| (lo.max(row_lo), hi.min(row_hi)))
        .filter(|&(lo, hi)| lo < hi)
        .collect();
    blocked.sort_unstable();
    let mut stretches: Vec<(usize, usize)> = Vec::new();
    let mut start = row_lo;
    for (b_lo, b_hi) in blocked {
        if b_lo > start {
            stretches.push((start, b_lo));
        }
        start = start.max(b_hi);
    }
    if row_hi > start {
        stretches.push((start, row_hi));
    }
    stretches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stretches_split_by_clipped_blockers() {
        // (1,4) clips away entirely (empty against the window); (15,20)
        // clips to (15,16), blocking the window's last row.
        let stretches = clear_stretches(6, 16, &[(1, 4), (7, 10), (12, 15), (15, 20)]);
        assert_eq!(stretches, vec![(6, 7), (10, 12)]);
    }

    #[test]
    fn touching_blockers_leave_no_empty_stretch() {
        let stretches = clear_stretches(0, 10, &[(3, 5), (5, 8)]);
        assert_eq!(stretches, vec![(0, 3), (8, 10)]);
    }
}
