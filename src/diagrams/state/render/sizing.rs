//! Canvas extent calculations: width from the rightmost layout edge,
//! height from the bottom edge, and the extra height self-loop labels
//! need below their state box.

use std::collections::HashMap;

use crate::diagrams::state::layout::{LayoutParams, StateLayout};
use crate::diagrams::state::types::Transition;
use crate::text::wrap_label;

/// Compute the canvas width: rightmost layout edge plus the x gap.
pub(super) fn compute_canvas_width(layouts: &[StateLayout], params: &LayoutParams) -> usize {
    let rightmost = layouts.iter().map(|l| l.rect.right()).max().unwrap_or(0);
    rightmost + params.gap_x
}

/// Compute the canvas height: bottommost layout edge plus a margin.
pub(super) fn compute_canvas_height(
    layouts: &[StateLayout],
    _params: &LayoutParams,
    _max_label_row: usize,
) -> usize {
    let bottommost = layouts.iter().map(|l| l.rect.bottom()).max().unwrap_or(0);
    // apply_gap_expansion already shifted states down to make room for
    // multi-line labels, so bottommost already includes that space.
    // Just add a small bottom margin.
    bottommost + 1
}

/// Compute extra height needed for self-loop labels that may wrap into
/// multiple lines and extend below the state box.
pub(super) fn self_loop_label_height(
    transitions: &[Transition],
    layouts: &[StateLayout],
    canvas_width: usize,
) -> usize {
    let id_to_layout: HashMap<&str, &StateLayout> =
        layouts.iter().map(|l| (l.id.as_str(), l)).collect();
    let mut max_extra = 0usize;
    for t in transitions {
        if t.from != t.to {
            continue;
        }
        let Some(label) = t.label.as_ref() else { continue };
        let Some(layout) = id_to_layout.get(t.from.as_str()) else { continue };
        let loop_x = layout.rect.x + layout.rect.w + 1;
        let avail = canvas_width.saturating_sub(loop_x + 2).max(10);
        let n = wrap_label(label, avail).line_count;
        // Label starts at rect.y and goes down n lines.
        let label_bottom = layout.rect.y + n;
        let extra = label_bottom.saturating_sub(layout.rect.bottom());
        max_extra = max_extra.max(extra);
    }
    max_extra
}
