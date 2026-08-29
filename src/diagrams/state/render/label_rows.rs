//! Row assignment for transition labels that share a vertical gap.
//!
//! Labels in the same gap whose x-ranges overlap are stacked on different
//! rows (0, 1, 2, …) so they don't collide; the draw side offsets each
//! row's label block 3 rows above the corridor row.

use std::collections::HashMap;

use unicode_width::UnicodeWidthStr;

use crate::diagrams::state::layout::StateLayout;
use crate::diagrams::state::types::Transition;
use crate::render::widget::Rect;

#[derive(Debug)]
struct LabelInfo {
    transition_index: usize,
    x: usize,
    width: usize,
    row: usize,
    gap_key: (usize, usize),
}

/// Vertical gap key of a transition: the (upper, lower) y pair of its
/// endpoint boxes. Transitions sharing a gap share a corridor row.
pub(super) fn gap_key_of(from: Rect, to: Rect) -> (usize, usize) {
    if from.y < to.y { (from.y, to.y) } else { (to.y, from.y) }
}

/// Assign each labeled transition a row within its vertical gap.
/// Returns a map from transition index to row.
pub(super) fn compute_label_rows(
    transitions: &[Transition],
    layouts: &[StateLayout],
) -> HashMap<usize, usize> {
    let id_to_layout: HashMap<&str, &StateLayout> =
        layouts.iter().map(|l| (l.id.as_str(), l)).collect();
    let mut labels: Vec<LabelInfo> = Vec::new();

    for (idx, t) in transitions.iter().enumerate() {
        let Some(text) = t.label.as_ref() else { continue };
        if t.from == t.to {
            continue;
        }
        let Some(from) = id_to_layout.get(t.from.as_str()) else { continue };
        let Some(to) = id_to_layout.get(t.to.as_str()) else { continue };

        let from_cx = from.rect.x + from.rect.w / 2;
        let to_cx = to.rect.x + to.rect.w / 2;
        let label_x = (from_cx + to_cx) / 2;
        let label_x = label_x.saturating_sub(text.width() / 2);
        let gap_key = gap_key_of(from.rect, to.rect);
        // Skip duplicate labels (same text + same gap) — only render the
        // first occurrence to avoid showing the same label twice.
        let is_dup = labels.iter().any(|l| {
            l.gap_key == gap_key
                && transitions[l.transition_index].label.as_deref().is_some_and(|prev| prev == text)
        });
        if is_dup {
            continue;
        }
        labels.push(LabelInfo {
            transition_index: idx,
            x: label_x,
            width: text.width(),
            row: 0,
            gap_key,
        });
    }

    if labels.is_empty() {
        return HashMap::new();
    }

    // Group labels by gap_key, then assign rows within each group.
    let mut gap_groups: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    for (i, label) in labels.iter().enumerate() {
        gap_groups.entry(label.gap_key).or_default().push(i);
    }

    for (_, mut group_indices) in gap_groups {
        // Sort within the group by x for stable row assignment.
        group_indices.sort_by(|a, b| labels[*a].x.cmp(&labels[*b].x));

        let mut rows: Vec<Vec<usize>> = Vec::new();
        for orig_idx in group_indices {
            let label = &labels[orig_idx];
            let mut placed = false;
            for (row_idx, row) in rows.iter_mut().enumerate() {
                let overlaps = row.iter().any(|&other_idx| {
                    let other = &labels[other_idx];
                    label.x < other.x + other.width && label.x + label.width > other.x
                });
                if !overlaps {
                    labels[orig_idx].row = row_idx;
                    row.push(orig_idx);
                    placed = true;
                    break;
                }
            }
            if !placed {
                let row_idx = rows.len();
                rows.push(vec![orig_idx]);
                labels[orig_idx].row = row_idx;
            }
        }
    }

    labels.into_iter().map(|l| (l.transition_index, l.row)).collect()
}

/// Shift all layouts down by `dy` rows.
pub(super) fn shift_layouts(layouts: &mut [StateLayout], dy: usize) {
    for layout in layouts.iter_mut() {
        layout.rect.y += dy;
    }
}
