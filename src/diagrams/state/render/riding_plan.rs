//! Per-gap riding-label plan: which aligned edges ride their leg, the
//! sibling-aware wrap width and center column for each rider, and the
//! overlap-cluster row allocation that stacks riders whose blocks would
//! collide. Built once from `trans_geoms` (pre-reroute geometry) and
//! consumed by BOTH the gap expansion (row budget) and the label draw
//! pass (block placement) so the two can never drift.

use std::collections::HashMap;

use crate::diagrams::state::layout::StateLayout;
use crate::diagrams::state::render::label_rows::gap_key_of;
use crate::diagrams::state::sugiyama::TransGeom;
use crate::diagrams::state::types::Transition;
use crate::layout::{RidingCandidate, allocate_riding_rows, riding_placement_cols};
use crate::text::wrap_label;

/// One riding label's plan: the transition it belongs to and every
/// geometric decision about its block, made once and shared by the row
/// budget and the draw pass.
pub(super) struct RiderPlan {
    /// Transition index (declaration order).
    pub idx: usize,
    /// Sibling-aware wrap width from the ladder; the avoid set is the
    /// gap's corridor junction columns plus every aligned edge's leg
    /// column in the gap (the rider's own leg is filtered out by the
    /// ladder, so no rider's block ever covers another edge's leg).
    pub wrap_w: usize,
    /// Column the block centers on: the leg column, or the beside-span
    /// center column when the ladder stepped off the line.
    pub center_x: usize,
    /// The block's column span (half-open), derived from its widest
    /// wrapped line centered on `center_x` — the cluster key.
    pub span: (usize, usize),
    /// Wrapped line count.
    pub lines: usize,
    /// `Some(row offset from the segment's fork-row end)` when stacked
    /// in an overlap cluster; `None` when clear of every other rider
    /// (the block centers within its segment).
    pub row_offset: Option<usize>,
    /// Forward edge (target below the source): rides the below-fork
    /// half of the gap; upward edges ride the above-fork half.
    pub forward: bool,
}

/// All riders of one vertical gap, with the group's stack demands.
#[derive(Default)]
pub(super) struct GapRiderGroup {
    /// Tallest below-fork stack (forward riders); rail rows not included.
    pub below_lines: usize,
    /// Tallest above-fork stack (upward riders); rail rows not included.
    pub above_lines: usize,
    /// The riders, in declaration order.
    pub riders: Vec<RiderPlan>,
}

/// Plan every gap's riding labels. An aligned edge (corridor_w == 0)
/// with a label whose gap also carries a corridor edge rides the
/// exclusive leg segment beyond the fork — its default mid-route
/// position coincides with the sibling corridor row, which makes the
/// branch attribution ambiguous.
pub(super) fn plan_gap_riders(
    transitions: &[Transition],
    layouts: &[StateLayout],
    trans_geoms: &[TransGeom],
    width: usize,
) -> HashMap<(usize, usize), GapRiderGroup> {
    let id_to_layout: HashMap<&str, &StateLayout> =
        layouts.iter().map(|l| (l.id.as_str(), l)).collect();

    // Pass 1: per gap, the corridor junction columns (the riding
    // trigger) and every aligned edge's leg column (the ladder's avoid
    // set — mechanism 1 of the rider-vs-rider fix: a rider's block must
    // clear every other aligned edge's leg, labeled or not).
    let mut gap_junctions: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    let mut gap_legs: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    for (idx, t) in transitions.iter().enumerate() {
        if t.from == t.to {
            continue;
        }
        let (Some(from), Some(to)) =
            (id_to_layout.get(t.from.as_str()), id_to_layout.get(t.to.as_str()))
        else {
            continue;
        };
        let geom = &trans_geoms[idx];
        let key = gap_key_of(from.rect, to.rect);
        if geom.corridor_w > 0 {
            let entry = gap_junctions.entry(key).or_default();
            entry.push(geom.left_x);
            entry.push(geom.right_x);
        } else {
            gap_legs.entry(key).or_default().push(geom.to_cx);
        }
    }

    // Pass 2: riders = labeled aligned edges in junction-carrying gaps.
    let mut groups: HashMap<(usize, usize), GapRiderGroup> = HashMap::new();
    for (idx, t) in transitions.iter().enumerate() {
        if t.from == t.to {
            continue;
        }
        let Some(label) = t.label.as_ref() else { continue };
        let (Some(from), Some(to)) =
            (id_to_layout.get(t.from.as_str()), id_to_layout.get(t.to.as_str()))
        else {
            continue;
        };
        let geom = &trans_geoms[idx];
        if geom.corridor_w != 0 {
            continue;
        }
        let key = gap_key_of(from.rect, to.rect);
        let Some(junctions) = gap_junctions.get(&key) else { continue };

        let mut avoid: Vec<usize> = junctions.clone();
        if let Some(legs) = gap_legs.get(&key) {
            avoid.extend_from_slice(legs);
        }
        avoid.sort_unstable();
        avoid.dedup();
        let (wrap_w, center_x) = riding_placement_cols(&avoid, geom.to_cx, width, geom.avail);
        let wrapped = wrap_label(label, wrap_w);
        let lo = center_x.saturating_sub(wrapped.max_width / 2);
        groups.entry(key).or_default().riders.push(RiderPlan {
            idx,
            wrap_w,
            center_x,
            span: (lo, lo + wrapped.max_width),
            lines: wrapped.line_count,
            row_offset: None,
            forward: from.rect.y < to.rect.y,
        });
    }

    // Pass 3: stack overlapping riders, per half — forward riders share
    // the below-fork rows, upward riders the above-fork rows, so the
    // two halves never contend for the same rows.
    for group in groups.values_mut() {
        for forward in [true, false] {
            let candidates: Vec<RidingCandidate> = group
                .riders
                .iter()
                .filter(|r| r.forward == forward)
                .map(|r| RidingCandidate { span_lo: r.span.0, span_hi: r.span.1, lines: r.lines })
                .collect();
            if candidates.is_empty() {
                continue;
            }
            let (offsets, demand) = allocate_riding_rows(&candidates);
            let mut k = 0;
            for rider in group.riders.iter_mut() {
                if rider.forward == forward {
                    rider.row_offset = offsets[k];
                    k += 1;
                }
            }
            if forward {
                group.below_lines = demand;
            } else {
                group.above_lines = demand;
            }
        }
    }
    groups
}
