//! Vertical gap growth: how many extra rows each vertical gap needs for
//! its stacked/wrapped labels, and the downward shift that makes room.

use std::collections::{HashMap, HashSet};

use crate::diagrams::state::layout::{LayoutParams, StateLayout};
use crate::diagrams::state::render::label_rows::gap_key_of;
use crate::diagrams::state::sugiyama::TransGeom;
use crate::diagrams::state::types::Transition;
use crate::text::wrap_label;

use super::riding_plan::plan_gap_riders;

/// Per-gap label space budget: stacked block heights above the corridor
/// row (indexed by stacking row), plus the riding blocks that live on
/// the exclusive leg segments — below the corridor row for forward
/// edges, above it for upward edges (see `own_leg_placement` in
/// `label.rs`).
#[derive(Default)]
pub(super) struct GapLabelBudget {
    pub above: Vec<usize>,
    pub below: usize,
    pub above_ride: usize,
}

/// Per-gap wrapped label heights: for each vertical gap, the tallest
/// label block on each stacking row (rows are the x-overlap stacking
/// groups from `compute_label_rows`), and the tallest below-corridor
/// rider stack. Line counts use the same wrap widths the draw pass uses
/// (including the sibling-aware width ladder for riding labels, planned
/// in `riding_plan`), so the vertical space reserved here always
/// matches what actually gets drawn. `width` is the canvas width shared
/// with the draw pass.
pub(super) fn gap_label_heights(
    transitions: &[Transition],
    layouts: &[StateLayout],
    label_rows: &HashMap<usize, usize>,
    trans_geoms: &[TransGeom],
    width: usize,
) -> HashMap<(usize, usize), GapLabelBudget> {
    let id_to_layout: HashMap<&str, &StateLayout> =
        layouts.iter().map(|l| (l.id.as_str(), l)).collect();

    // Riding labels: their wrap widths, cluster stacking, and half-gap
    // demands all come from one plan (see `riding_plan`) — the same
    // plan the draw pass reads.
    let plans = plan_gap_riders(transitions, layouts, trans_geoms, width);
    let mut rider_idx: HashSet<usize> = HashSet::new();
    let mut gaps: HashMap<(usize, usize), GapLabelBudget> = HashMap::new();
    for (key, group) in &plans {
        // The riding convention keeps one leg row on each side of the
        // block (`| label |`), mirroring the corridor embed's
        // `---label---` padding — without the lead-in row the fork
        // junction glyph degrades to a corner (the leg continues behind
        // the label, but the repair pass cannot see it). The demand is
        // the TALLEST overlap cluster's stack (clusters sit in
        // different columns and share rows), not the sum over riders.
        let budget = gaps.entry(*key).or_default();
        if group.below_lines > 0 {
            budget.below = budget.below.max(group.below_lines + 2);
        }
        if group.above_lines > 0 {
            budget.above_ride = budget.above_ride.max(group.above_lines + 2);
        }
        for r in &group.riders {
            rider_idx.insert(r.idx);
        }
    }

    for (idx, t) in transitions.iter().enumerate() {
        if t.from == t.to || rider_idx.contains(&idx) {
            continue;
        }
        let Some(from) = id_to_layout.get(t.from.as_str()) else { continue };
        let Some(to) = id_to_layout.get(t.to.as_str()) else { continue };
        let Some(label) = &t.label else { continue };
        let row = label_rows.get(&idx).copied().unwrap_or(0);
        let key = gap_key_of(from.rect, to.rect);
        let budget = gaps.entry(key).or_default();
        let n = wrap_label(label, trans_geoms[idx].avail).line_count;
        if budget.above.len() <= row {
            budget.above.resize(row + 1, 0);
        }
        budget.above[row] = budget.above[row].max(n);
    }
    gaps
}

/// Compute how many extra rows each vertical gap needs to fit its label
/// stack: every above-corridor row's block plus one blank separator row
/// between rows, plus the corridor row, plus the tallest below-corridor
/// block — minus the rows the layout already provides between the two
/// boxes.
pub(super) fn compute_gap_expansion(
    transitions: &[Transition],
    layouts: &[StateLayout],
    label_rows: &HashMap<usize, usize>,
    trans_geoms: &[TransGeom],
    width: usize,
) -> HashMap<(usize, usize), usize> {
    let heights = gap_label_heights(transitions, layouts, label_rows, trans_geoms, width);

    let mut expansion = HashMap::new();
    for ((upper_y, lower_y), budget) in heights {
        let stack_rows: usize =
            budget.above.iter().sum::<usize>() + budget.above.len().saturating_sub(1);
        // + the corridor row itself + the below-corridor riding block.
        let mut needed = stack_rows + 1;
        if budget.below > 0 || budget.above_ride > 0 {
            // Riding blocks live in one half of the gap, and the corridor
            // row sits at the gap's midpoint — both halves draw from one
            // span. Guarantee each half holds its demand: the lower half
            // the forward riding block plus the rows a row-0 corridor
            // label dips below the corridor row (odd-height blocks
            // straddle it), the upper half the above-corridor stack plus
            // an upward riding block.
            let h0 = budget.above.first().copied().unwrap_or(0);
            let row0_dip = h0.div_ceil(2).saturating_sub(1);
            let below_demand = row0_dip + budget.below;
            let above_demand = stack_rows.saturating_sub(h0.div_ceil(2)) + budget.above_ride;
            needed = needed.max(1 + 2 * above_demand.max(below_demand));
        }
        // Rows the layout already leaves between the two layers: the
        // gap runs from the deepest box on the upper layer down to the
        // lower layer's top.
        let upper_bottom =
            layouts.iter().filter(|l| l.rect.y == upper_y).map(|l| l.rect.bottom()).max();
        let lower_top = layouts.iter().find(|l| l.rect.y == lower_y).map(|l| l.rect.y);
        let available = match (upper_bottom, lower_top) {
            (Some(ub), Some(lt)) => lt.saturating_sub(ub),
            _ => 0,
        };
        let extra = needed.saturating_sub(available);
        if extra > 0 {
            expansion.insert((upper_y, lower_y), extra);
        }
    }
    expansion
}

/// Expand gaps between states by inserting extra rows. States below a
/// gap are shifted down by the accumulated extra.
pub(super) fn apply_gap_expansion(
    layouts: &mut [StateLayout],
    gap_extra: &HashMap<(usize, usize), usize>,
    _params: &LayoutParams,
) {
    if gap_extra.is_empty() {
        return;
    }
    // Deduplicate extras by ty (the lower y of each gap). Multiple gaps
    // sharing the same ty should only expand once (take the max extra),
    // not once per state.
    let mut ty_extras: HashMap<usize, usize> = HashMap::new();
    for ((_fy, ty), extra) in gap_extra {
        ty_extras.entry(*ty).and_modify(|v| *v = (*v).max(*extra)).or_insert(*extra);
    }

    // Sort layouts by y to process top-to-bottom.
    layouts.sort_by_key(|l| l.rect.y);

    // Accumulate expansion downward; each ty's extra is counted once
    // even if multiple states share the same y.
    let mut cumul = 0usize;
    let mut fired: HashSet<usize> = HashSet::new();
    for layout in layouts.iter_mut() {
        let orig_y = layout.rect.y;
        for (&ty, &extra) in &ty_extras {
            if ty == orig_y && !fired.contains(&ty) {
                cumul += extra;
                fired.insert(ty);
            }
        }
        layout.rect.y = orig_y + cumul;
    }
}
