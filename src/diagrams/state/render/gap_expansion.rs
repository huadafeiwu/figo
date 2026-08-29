//! Vertical gap growth: how many extra rows each vertical gap needs for
//! its stacked/wrapped labels, and the downward shift that makes room.

use std::collections::{HashMap, HashSet};

use crate::diagrams::state::layout::{LayoutParams, StateLayout};
use crate::diagrams::state::render::label_rows::gap_key_of;
use crate::diagrams::state::sugiyama::TransGeom;
use crate::diagrams::state::types::Transition;
use crate::text::wrap_label;

/// Per-gap wrapped label heights: for each vertical gap, the tallest
/// label block on each row (rows are the x-overlap stacking groups from
/// `compute_label_rows`). Line counts come from the transition geoms'
/// `avail` — the same wrap width the draw pass uses, so the vertical
/// space reserved here always matches what actually gets drawn.
pub(super) fn gap_label_heights(
    transitions: &[Transition],
    layouts: &[StateLayout],
    label_rows: &HashMap<usize, usize>,
    trans_geoms: &[TransGeom],
) -> HashMap<(usize, usize), Vec<usize>> {
    let id_to_layout: HashMap<&str, &StateLayout> =
        layouts.iter().map(|l| (l.id.as_str(), l)).collect();

    let mut gaps: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    for (idx, t) in transitions.iter().enumerate() {
        if t.from == t.to {
            continue;
        }
        let Some(from) = id_to_layout.get(t.from.as_str()) else { continue };
        let Some(to) = id_to_layout.get(t.to.as_str()) else { continue };
        let Some(label) = &t.label else { continue };
        let row = label_rows.get(&idx).copied().unwrap_or(0);
        let n = wrap_label(label, trans_geoms[idx].avail).line_count;
        let heights = gaps.entry(gap_key_of(from.rect, to.rect)).or_default();
        if heights.len() <= row {
            heights.resize(row + 1, 0);
        }
        heights[row] = heights[row].max(n);
    }
    gaps
}

/// Compute how many extra rows each vertical gap needs to fit its label
/// stack: every row's block plus one blank separator row between rows,
/// plus the corridor row, minus the rows the layout already provides
/// between the two boxes.
pub(super) fn compute_gap_expansion(
    transitions: &[Transition],
    layouts: &[StateLayout],
    label_rows: &HashMap<usize, usize>,
    trans_geoms: &[TransGeom],
) -> HashMap<(usize, usize), usize> {
    let heights = gap_label_heights(transitions, layouts, label_rows, trans_geoms);

    let mut expansion = HashMap::new();
    for ((upper_y, lower_y), hs) in heights {
        let stack_rows: usize = hs.iter().sum::<usize>() + hs.len().saturating_sub(1);
        let needed = stack_rows + 1; // + the corridor row itself
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
