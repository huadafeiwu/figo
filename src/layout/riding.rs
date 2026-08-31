//! Riding-label primitives shared by the state and flowchart diagrams:
//! the sibling-aware wrap-width ladder, the riding directive consumed
//! by `Connector::draw_label`, and the overlap-cluster row allocation
//! that keeps same-gap riding blocks (state) and same-column riders
//! (flowchart) from covering each other.
//!
//! Everything here is pure geometry — no diagram types — so the layout
//! estimation passes and the draw passes read one source.

use std::collections::HashMap;

/// Wrap width for a label centered on a horizontal corridor of `len`
/// cells (both legs inclusive): the corridor minus `---` padding on
/// each side. Shared by the layout estimation (reserving rows) and the
/// drawing pass so the two never drift apart.
pub fn corridor_label_avail(len: usize, w_limit: usize) -> usize {
    len.saturating_sub(4).max(2).min(w_limit)
}

/// Wrap width for a label placed beside a vertical line at column `sx`,
/// bounded by the canvas width.
pub fn beside_line_label_avail(sx: usize, w_limit: usize) -> usize {
    w_limit.saturating_sub(sx + 2).max(2).min(w_limit)
}

/// Wrap width and center column for a label riding its own line at
/// `ride_col`, shared by the state and flowchart label placements (and
/// their row-reservation estimates) so both sides read one source.
///
/// Ladder, every step measured: (1) ride the line — wrap to the widest
/// block that stays clear of the nearest sibling leg column on either
/// side (covering one's own line is the riding convention); (2) when no
/// riding width exists (a sibling leg is immediately adjacent), center
/// in the wider free span beside the line.
pub fn riding_placement_cols(
    avoid_cols: &[usize],
    ride_col: usize,
    canvas_width: usize,
    avail: usize,
) -> (usize, usize) {
    // Distance to the nearest sibling leg column on either side.
    let nearest =
        avoid_cols.iter().copied().filter(|&c| c != ride_col).map(|c| c.abs_diff(ride_col)).min();

    match nearest.map(|d| 2 * d - 1) {
        // Room to ride the line: wrap to the widest width that keeps the
        // block clear of both sibling legs (greedy wrap = fewest lines
        // for that width).
        Some(max_lw) if max_lw >= 2 => (avail.min(max_lw), ride_col),
        // No riding width — fall back to the wider free span beside the
        // line, centered in that span (off the line, still clear of
        // every sibling leg).
        _ => {
            let left_bound = avoid_cols
                .iter()
                .copied()
                .filter(|&c| c < ride_col)
                .max()
                .map_or(0, |c| c.saturating_add(1));
            let right_bound =
                avoid_cols.iter().copied().filter(|&c| c > ride_col).min().unwrap_or(canvas_width);
            let left_w = ride_col.saturating_sub(left_bound);
            let right_w = right_bound.saturating_sub(ride_col + 1);
            if left_w >= right_w {
                (left_w, left_bound + left_w / 2)
            } else {
                (right_w, ride_col + 1 + right_w / 2)
            }
        }
    }
}

/// Top row of a corridor label block: centered on the corridor row `y`
/// but kept inside `[sy, ty - n + 1]` so the block never starts above
/// the source anchor or below the target. Shared by the draw pass and
/// the fork-riding row estimates.
pub fn corridor_label_block_top(y: usize, n: usize, sy: usize, ty: usize) -> usize {
    y.saturating_sub(n / 2).min(ty.saturating_sub(n.saturating_sub(1))).max(sy)
}

/// Riding directive for a purely vertical connector: the default label
/// anchor (the row just below the source) coincides with the corridor
/// row of same-source corridor siblings, so the block instead rides an
/// exclusive stretch of the leg. The caller — which sees every node's
/// geometry — measures the wrap width (sibling-aware ladder, see
/// [`riding_placement_cols`]) and picks the stretch clear of sibling
/// label blocks and intermediate boxes, nearest the target box.
#[derive(Clone, Copy, Debug)]
pub struct RidingLabel {
    pub wrap_w: usize,
    /// Column the block centers on: the leg column when the ladder kept
    /// the block on the line, or the wider free span's center column
    /// when it stepped off the line (the leg then stays fully visible
    /// through the block's rows).
    pub center_x: usize,
    pub block_top: usize,
}

/// A riding block candidate for row allocation: the block's column span
/// (half-open) and its wrapped line count. Candidates are given in
/// declaration order.
#[derive(Clone, Copy)]
pub struct RidingCandidate {
    /// Inclusive left column of the block's widest line.
    pub span_lo: usize,
    /// Exclusive right column of the block's widest line.
    pub span_hi: usize,
    /// Wrapped line count.
    pub lines: usize,
}

/// Row allocation for a group of riding blocks sharing one leg
/// segment: blocks whose column spans overlap (transitively) would
/// collide on the same rows, so each overlap cluster is stacked on
/// successive row bands in declaration order, filling from the
/// fork-row end of the segment toward the target box (one rule for
/// both directions). Blocks clear of every other block keep their own
/// centered position.
///
/// Returns, per candidate in input order, `Some(row offset from the
/// segment's fork-row end)` for stacked blocks and `None` for solo
/// blocks, plus the group's total row demand — the tallest stack
/// (solo blocks share rows with clusters, so demands combine by max,
/// not sum).
pub fn allocate_riding_rows(candidates: &[RidingCandidate]) -> (Vec<Option<usize>>, usize) {
    let n = candidates.len();
    // Transitive overlap clustering via label propagation to a
    // fixpoint (rider groups are tiny; this stays flat and allocation
    // order equals declaration order).
    let mut cluster: Vec<usize> = (0..n).collect();
    loop {
        let mut changed = false;
        for i in 0..n {
            for j in 0..n {
                if i == j || cluster[i] == cluster[j] {
                    continue;
                }
                let overlap = candidates[i].span_lo < candidates[j].span_hi
                    && candidates[j].span_lo < candidates[i].span_hi;
                if overlap {
                    let merged = cluster[i].min(cluster[j]);
                    cluster[i] = merged;
                    cluster[j] = merged;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    // Solo candidates (no overlap with anyone) keep their centered
    // position; only real clusters stack.
    let mut sizes: HashMap<usize, usize> = HashMap::new();
    for &c in &cluster {
        *sizes.entry(c).or_insert(0) += 1;
    }

    // Successive row bands per cluster: candidate i in a cluster starts
    // at the sum of its predecessors' line counts.
    let mut offset: Vec<Option<usize>> = vec![None; n];
    let mut next_row: HashMap<usize, usize> = HashMap::new();
    let mut height: HashMap<usize, usize> = HashMap::new();
    for (i, cand) in candidates.iter().enumerate() {
        if sizes[&cluster[i]] == 1 {
            continue;
        }
        let c = cluster[i];
        let start = *next_row.entry(c).or_insert(0);
        offset[i] = Some(start);
        next_row.insert(c, start + cand.lines);
        *height.entry(c).or_insert(0) += cand.lines;
    }

    let mut demand = 0;
    for h in height.values() {
        demand = demand.max(*h);
    }
    for (i, cand) in candidates.iter().enumerate() {
        if offset[i].is_none() {
            demand = demand.max(cand.lines);
        }
    }
    (offset, demand)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_rides_line_between_sibling_legs() {
        // Distance 4 to the nearest sibling leg: widest on-line block is
        // 2*4-1 = 7, centered on the leg column.
        let (w, center) = riding_placement_cols(&[34, 50], 30, 80, 48);
        assert_eq!((w, center), (7, 30));
    }

    #[test]
    fn ladder_fallback_centers_in_wider_span_excluding_the_line() {
        // Sibling leg immediately adjacent (d=1): no riding width, so
        // the block moves into the wider free span beside the line. The
        // returned center column must lie strictly inside the span,
        // clear of both the line and the sibling leg.
        let (w, center) = riding_placement_cols(&[21], 20, 80, 58);
        assert_eq!(w, 20, "wider span is the left one (cols 0..20)");
        assert!(center < 20, "center must sit left of the leg column, got {center}");
    }

    #[test]
    fn overlapping_riders_stack_in_declaration_order() {
        // Two blocks whose spans overlap on 9 columns: they stack, the
        // second starting below the first, and the demand is the sum.
        let (offsets, demand) = allocate_riding_rows(&[
            RidingCandidate { span_lo: 16, span_hi: 45, lines: 1 },
            RidingCandidate { span_lo: 38, span_hi: 62, lines: 2 },
        ]);
        assert_eq!(offsets, vec![Some(0), Some(1)]);
        assert_eq!(demand, 3);
    }

    #[test]
    fn disjoint_riders_keep_centered_positions() {
        // No span overlap: both stay solo (None) and the demand is the
        // taller block, not the sum.
        let (offsets, demand) = allocate_riding_rows(&[
            RidingCandidate { span_lo: 0, span_hi: 10, lines: 1 },
            RidingCandidate { span_lo: 20, span_hi: 30, lines: 3 },
        ]);
        assert_eq!(offsets, vec![None, None]);
        assert_eq!(demand, 3);
    }

    #[test]
    fn transitive_overlap_forms_one_cluster() {
        // A overlaps B, B overlaps C, A and C do not overlap: label
        // propagation must merge all three into one stack.
        let (offsets, demand) = allocate_riding_rows(&[
            RidingCandidate { span_lo: 0, span_hi: 10, lines: 1 },
            RidingCandidate { span_lo: 5, span_hi: 15, lines: 1 },
            RidingCandidate { span_lo: 12, span_hi: 22, lines: 2 },
        ]);
        assert_eq!(offsets, vec![Some(0), Some(1), Some(2)]);
        assert_eq!(demand, 4);
    }
}
