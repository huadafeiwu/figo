//! Vertical gap growth: how many extra rows each vertical gap needs for
//! its stacked/wrapped labels, and the downward shift that makes room.

use std::collections::{HashMap, HashSet};

use crate::diagrams::state::layout::{LayoutParams, StateLayout};
use crate::diagrams::state::types::Transition;
use crate::text::wrap_label;

/// Compute how many extra rows each vertical gap needs to fit its labels.
/// Returns a map from gap key to extra rows needed.
pub(super) fn compute_gap_expansion(
    transitions: &[Transition],
    layouts: &[StateLayout],
    label_rows: &HashMap<usize, usize>,
) -> HashMap<(usize, usize), usize> {
    let id_to_layout: HashMap<&str, &StateLayout> =
        layouts.iter().map(|l| (l.id.as_str(), l)).collect();

    // Track max (row + num_lines) per gap — the vertical space needed is
    // determined by how many rows of multi-line labels stack up.
    let mut gap_max_extent: HashMap<(usize, usize), usize> = HashMap::new();

    for (idx, t) in transitions.iter().enumerate() {
        if t.from == t.to {
            continue;
        }
        let Some(from) = id_to_layout.get(t.from.as_str()) else { continue };
        let Some(to) = id_to_layout.get(t.to.as_str()) else { continue };
        let row = label_rows.get(&idx).copied().unwrap_or(0);
        let from_cx = from.rect.x + from.rect.w / 2;
        let to_cx = to.rect.x + to.rect.w / 2;
        let gap_key = if from.rect.y < to.rect.y {
            (from.rect.y, to.rect.y)
        } else {
            (to.rect.y, from.rect.y)
        };

        // Estimate the label's line count for wrapping.
        let num_lines = if let Some(text) = &t.label {
            let corridor_w = if from_cx != to_cx { from_cx.abs_diff(to_cx) + 1 } else { 80 };
            let avail = corridor_w.max(10);
            wrap_label(text, avail).line_count
        } else {
            1
        };

        // The vertical extent needed is row * 3 (spacing between rows) +
        // num_lines (the label block itself).
        let extent = row * 3 + num_lines;
        gap_max_extent.entry(gap_key).and_modify(|r| *r = (*r).max(extent)).or_insert(extent);
    }

    // Extra rows needed: the extent minus what's already available (1 row
    // for the corridor itself).
    let mut expansion = HashMap::new();
    for (key, extent) in gap_max_extent {
        if extent > 1 {
            expansion.insert(key, extent * 3);
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
