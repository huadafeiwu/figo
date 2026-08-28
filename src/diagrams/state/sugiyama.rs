//! Sugiyama-style crossing reduction and coordinate assignment.
//!
//! Implements phases 2 (crossing reduction via median heuristic) and 3
//! (coordinate assignment via a simplified Brandes-Köpf alignment) of the
//! Sugiyama framework. Phase 1 (layering) and phase 4 (edge routing) are
//! handled by `layout.rs` and `render.rs` respectively.
//!
//! After coordinate assignment, `compute_trans_geoms` produces a single
//! source of truth for each transition's geometry (corridor width, label
//! placement, embed decision). `render.rs` reads these pre-computed values
//! instead of re-deriving them — eliminating the multi-copy inconsistency
//! that caused repeated alignment bugs.

use crate::diagrams::state::types::Transition;
use crate::render::widget::{Rect, Size};
use std::collections::HashMap;

/// Pre-computed geometry for a single transition.
///
/// This is the **single source of truth** for corridor width, label
/// placement, and embed decisions. Both `build()` (canvas sizing) and
/// `draw_external_transition()` (rendering) read from this struct instead
/// of independently recalculating.
#[derive(Clone, Debug, Default)]
pub struct TransGeom {
    /// Center column of the `from` box.
    pub from_cx: usize,
    /// Center column of the `to` box.
    pub to_cx: usize,
    /// Left end of the horizontal corridor (= min(from_cx, to_cx)).
    pub left_x: usize,
    /// Right end (= max(from_cx, to_cx)).
    pub right_x: usize,
    /// Corridor width = right_x - left_x + 1, or 0 if same column.
    pub corridor_w: usize,
    /// Whether the label should be embedded in the corridor line.
    pub embed: bool,
    /// Wrap width for the label.
    pub avail: usize,
    /// X-center for the label block.
    pub base_x: usize,
    /// Whether from and to are in the same layer (horizontal arrow).
    pub same_layer: bool,
}

/// Result of layout: positioned states + pre-computed transition geometry.
pub struct LayoutResult {
    pub layouts: Vec<crate::diagrams::state::layout::StateLayout>,
    pub trans_geoms: Vec<TransGeom>,
}

/// Phase 2: Reduce edge crossings using the median heuristic.
pub fn reduce_crossings(
    layers: &[Vec<usize>],
    adj: &[Vec<usize>],
    predecessors: &[Vec<usize>],
) -> Vec<Vec<usize>> {
    if layers.is_empty() {
        return Vec::new();
    }

    let mut current: Vec<Vec<usize>> = layers.to_vec();
    let mut best = current.clone();
    let mut best_crossings = count_total_crossings(&current, adj, predecessors);

    const MAX_ROUNDS: usize = 24;
    for round in 0..MAX_ROUNDS {
        if round % 2 == 0 {
            for i in (0..current.len().saturating_sub(1)).rev() {
                let (left, right) = current.split_at_mut(i + 1);
                sort_by_median(&mut left[i], &right[0], adj);
            }
        } else {
            for i in 1..current.len() {
                let (left, right) = current.split_at_mut(i);
                sort_by_median(&mut right[0], &left[i - 1], predecessors);
            }
        }

        let crossings = count_total_crossings(&current, adj, predecessors);
        if crossings < best_crossings {
            best_crossings = crossings;
            best = current.clone();
        }
    }

    best
}

fn sort_by_median(layer: &mut [usize], adjacent: &[usize], neighbours_of: &[Vec<usize>]) {
    if layer.len() <= 1 {
        return;
    }

    let pos: HashMap<usize, usize> =
        adjacent.iter().enumerate().map(|(pos, &node)| (node, pos)).collect();

    let medians: Vec<(usize, usize)> = layer
        .iter()
        .map(|&node| {
            let positions: Vec<usize> =
                neighbours_of[node].iter().filter_map(|nb| pos.get(nb).copied()).collect();
            let median = if positions.is_empty() {
                layer.iter().position(|&n| n == node).unwrap_or(0)
            } else {
                positions[positions.len() / 2]
            };
            (node, median)
        })
        .collect();

    layer.sort_by_key(|&node| {
        medians.iter().find(|&&(n, _)| n == node).map(|&(_, m)| m).unwrap_or(0)
    });
}

fn count_total_crossings(
    layers: &[Vec<usize>],
    adj: &[Vec<usize>],
    predecessors: &[Vec<usize>],
) -> usize {
    let mut total = 0;
    for i in 0..layers.len().saturating_sub(1) {
        total += count_layer_pair_crossings(&layers[i], &layers[i + 1], adj, predecessors);
    }
    total
}

fn count_layer_pair_crossings(
    upper: &[usize],
    lower: &[usize],
    _adj: &[Vec<usize>],
    predecessors: &[Vec<usize>],
) -> usize {
    let upper_pos: HashMap<usize, usize> = upper.iter().enumerate().map(|(p, &n)| (n, p)).collect();

    let mut crossings = 0;
    for (j, &lower_node) in lower.iter().enumerate() {
        for &upper_node in &predecessors[lower_node] {
            let Some(&i) = upper_pos.get(&upper_node) else { continue };
            for (j2, &lower_node2) in lower.iter().enumerate() {
                if j2 <= j {
                    continue;
                }
                for &upper_node2 in &predecessors[lower_node2] {
                    let Some(&i2) = upper_pos.get(&upper_node2) else { continue };
                    if i2 < i {
                        crossings += 1;
                    }
                }
            }
        }
    }
    crossings
}

/// Phase 3: Assign x-coordinates using a simplified Brandes-Köpf approach.
///
/// Alignment blocks share the same **center column** (not left boundary),
/// so different-width boxes in the same block have aligned `|` legs.
pub fn assign_coordinates(
    layers: &[Vec<usize>],
    adj: &[Vec<usize>],
    sizes: &[Size],
    min_gap: usize,
    canvas_width: usize,
    left_margin: usize,
) -> Vec<usize> {
    let node_count = sizes.len();
    if node_count == 0 {
        return Vec::new();
    }

    let mut predecessors: Vec<Vec<usize>> = vec![Vec::new(); node_count];
    for (i, succs) in adj.iter().enumerate() {
        for &s in succs {
            if s < node_count && i != s {
                predecessors[s].push(i);
            }
        }
    }

    let mut aligned_up: Vec<Option<usize>> = vec![None; node_count];
    let mut aligned_down: Vec<Option<usize>> = vec![None; node_count];

    for (layer_i, layer) in layers.iter().enumerate() {
        if layer_i == 0 {
            continue;
        }
        let upper = &layers[layer_i - 1];

        let mut r = 0usize;
        for &u in upper {
            let succs: Vec<usize> =
                adj[u].iter().filter(|&&s| layer.contains(&s)).copied().collect();
            if succs.is_empty() {
                continue;
            }
            let positions: Vec<usize> =
                succs.iter().map(|s| layer.iter().position(|&n| n == *s).unwrap_or(0)).collect();
            let med = positions[positions.len() / 2];

            for &s in &succs {
                let p = layer.iter().position(|&n| n == s).unwrap_or(0);
                if p >= r && p <= med + (positions.len() / 2) && aligned_up[s].is_none() {
                    aligned_up[s] = Some(u);
                    aligned_down[u] = Some(s);
                    r = p + 1;
                    break;
                }
            }
        }
    }

    let mut block_root: Vec<usize> = (0..node_count).collect();
    for n in 0..node_count {
        if let Some(u) = aligned_up[n] {
            block_root[n] = block_root[u];
        }
    }

    // Assign x-coordinates: left-to-right sweep.
    let mut x: Vec<usize> = vec![0; node_count];
    let mut block_x: HashMap<usize, usize> = HashMap::new();

    for layer in layers.iter() {
        let mut prev_right = 0usize;
        for &node in layer.iter() {
            let root = block_root[node];
            let w = sizes[node].w;
            let min_x = prev_right + if prev_right > 0 { min_gap } else { 0 };
            let existing = block_x.get(&root).copied().unwrap_or(usize::MAX);
            let placed_x =
                if existing == usize::MAX { min_x.max(left_margin) } else { existing.max(min_x) };
            block_x.insert(root, placed_x);
            x[node] = placed_x;
            prev_right = placed_x + w;
        }
    }

    // Propagate block x to all members.
    for n in 0..node_count {
        let root = block_root[n];
        if let Some(&bx) = block_x.get(&root) {
            x[n] = bx;
        }
    }

    // --- Center normalization: align by CENTER column, not left boundary ---
    // For each block, compute the shared center column and adjust each
    // member's x so that x + w/2 equals the block center. This ensures
    // different-width boxes in the same block have aligned `|` legs.
    let mut block_center: HashMap<usize, usize> = HashMap::new();
    for n in 0..node_count {
        let root = block_root[n];
        let cx = x[n] + sizes[n].w / 2;
        block_center.entry(root).and_modify(|c| *c = (*c).max(cx)).or_insert(cx);
    }
    for n in 0..node_count {
        let root = block_root[n];
        if let Some(&cx) = block_center.get(&root) {
            x[n] = cx.saturating_sub(sizes[n].w / 2);
        }
    }

    // --- Collision resolution: after center normalization, boxes in the
    // same layer may overlap. Sort by current x, then sweep left-to-right
    // and push overlapping boxes rightward, preserving min_gap.
    for layer in layers.iter() {
        let mut sorted: Vec<usize> = layer.to_vec();
        sorted.sort_by_key(|&n| x[n]);
        let mut prev_right: Option<usize> = None;
        for &node in &sorted {
            let min_x = prev_right.map(|r| r + min_gap).unwrap_or(0);
            if x[node] < min_x {
                x[node] = min_x;
            }
            prev_right = Some(x[node] + sizes[node].w);
        }
    }

    // Center globally.
    let max_right = x.iter().zip(sizes.iter()).map(|(&xi, s)| xi + s.w).max().unwrap_or(0);
    let total_w = max_right;
    let offset = if total_w < canvas_width { (canvas_width - total_w) / 2 } else { 0 };
    for xi in x.iter_mut() {
        *xi += offset;
    }

    x
}

/// Phase 3b: Widen column gaps to fit transition labels.
///
/// Uses **center distance** (not edge distance) to match the corridor_w
/// calculation in `compute_trans_geoms`. The needed gap = label_w + 4
/// (2 cells padding per side). Canvas width is expanded by `build()` to
/// fit, so gaps are not capped here.
pub fn compute_column_gaps(
    x: &mut [usize],
    transitions: &[Transition],
    id_to_idx: &HashMap<&str, usize>,
    sizes: &[Size],
    min_gap: usize,
    _canvas_width: usize,
) {
    let mut gap_reqs: Vec<(usize, usize, usize)> = Vec::new();
    for t in transitions {
        if t.from == t.to {
            continue;
        }
        let Some(&from_i) = id_to_idx.get(t.from.as_str()) else { continue };
        let Some(&to_i) = id_to_idx.get(t.to.as_str()) else { continue };
        if from_i >= x.len() || to_i >= x.len() {
            continue;
        }
        let from_cx = x[from_i] + sizes[from_i].w / 2;
        let to_cx = x[to_i] + sizes[to_i].w / 2;
        if from_cx == to_cx {
            continue;
        }
        if let Some(label) = &t.label {
            let lw = unicode_width::UnicodeWidthStr::width(label.as_str());
            let needed = lw + 4;
            gap_reqs.push((from_i, to_i, needed));
        } else {
            gap_reqs.push((from_i, to_i, min_gap));
        }
    }

    gap_reqs.sort_by_key(|&(from_i, _, _)| x[from_i]);

    for &(from_i, to_i, needed) in &gap_reqs {
        let from_cx = x[from_i] + sizes[from_i].w / 2;
        let to_cx = x[to_i] + sizes[to_i].w / 2;

        // Use center distance (matches corridor_w in compute_trans_geoms).
        let current_corridor = from_cx.abs_diff(to_cx) + 1;
        if current_corridor >= needed {
            continue;
        }

        // Need to widen: shift the right node (and all nodes with cx >=
        // right cx) by the extra amount. Use cx (not x) to determine which
        // nodes are "to the right" — nodes in the same alignment block may
        // share the same x but have different cx.
        let extra = needed - current_corridor;
        let right_idx = if from_cx < to_cx { to_i } else { from_i };
        let right_cx = x[right_idx] + sizes[right_idx].w / 2;
        for (i, xi) in x.iter_mut().enumerate() {
            let cx = *xi + sizes[i].w / 2;
            if cx >= right_cx && i != right_idx {
                *xi += extra;
            }
        }
        x[right_idx] += extra;
    }
}

/// Compute the complete geometry for every transition — the **single source
/// of truth** used by both `build()` (canvas sizing) and
/// `draw_external_transition()` (rendering).
///
/// This eliminates the multi-copy inconsistency where corridor width, embed
/// decision, and label placement were independently recalculated in 4 places.
pub fn compute_trans_geoms(
    layouts: &[crate::diagrams::state::layout::StateLayout],
    transitions: &[Transition],
    id_to_idx: &HashMap<&str, usize>,
    canvas_width: usize,
) -> Vec<TransGeom> {
    transitions
        .iter()
        .map(|t| {
            let from_i = id_to_idx.get(t.from.as_str()).copied().unwrap_or(0);
            let to_i = id_to_idx.get(t.to.as_str()).copied().unwrap_or(0);

            if from_i >= layouts.len() || to_i >= layouts.len() || t.from == t.to {
                return TransGeom::default();
            }

            let from = &layouts[from_i].rect;
            let to = &layouts[to_i].rect;
            let same_layer = from.y == to.y;

            compute_single_geom(from, to, same_layer, t.label.as_deref(), canvas_width)
        })
        .collect()
}

/// Compute geometry for a single transition.
fn compute_single_geom(
    from: &Rect,
    to: &Rect,
    same_layer: bool,
    label: Option<&str>,
    canvas_width: usize,
) -> TransGeom {
    let from_cx = from.x + from.w / 2;
    let to_cx = to.x + to.w / 2;

    if same_layer {
        // Same-layer: horizontal arrow between two boxes.
        let (left_x, right_x) =
            if from_cx < to_cx { (from.x + from.w, to.x) } else { (to.x + to.w, from.x) };
        let corridor_w = right_x.saturating_sub(left_x);
        let label_w = label.map(unicode_width::UnicodeWidthStr::width).unwrap_or(0);
        let embed = corridor_w > 4 && corridor_w >= label_w + 4;
        let avail = if embed { corridor_w - 4 } else { corridor_w.max(2) };
        let base_x = (left_x + right_x) / 2;
        TransGeom {
            from_cx,
            to_cx,
            left_x,
            right_x,
            corridor_w,
            embed,
            avail,
            base_x,
            same_layer: true,
        }
    } else {
        // Cross-layer: V-H-V path, corridor is center-to-center.
        let (left_x, right_x) = if from_cx < to_cx { (from_cx, to_cx) } else { (to_cx, from_cx) };
        let corridor_w = if right_x > left_x { right_x - left_x + 1 } else { 0 };
        let label_w = label.map(unicode_width::UnicodeWidthStr::width).unwrap_or(0);
        let embed = corridor_w > 0 && corridor_w >= label_w + 4;
        let avail = if corridor_w > 0 {
            if embed { corridor_w - 4 } else { corridor_w.max(2) }
        } else {
            canvas_width.saturating_sub(from_cx + 2).max(2)
        };
        // For aligned edges (corridor_w == 0), label goes beside the leg.
        let base_x = if corridor_w == 0 { from_cx } else { (from_cx + to_cx) / 2 };
        TransGeom {
            from_cx,
            to_cx,
            left_x,
            right_x,
            corridor_w,
            embed,
            avail,
            base_x,
            same_layer: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduce_crossings_empty() {
        let result = reduce_crossings(&[], &[], &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn assign_coordinates_single_node() {
        let sizes = vec![Size::new(14, 3)];
        let layers = vec![vec![0]];
        let x = assign_coordinates(&layers, &[], &sizes, 6, 80, 0);
        assert_eq!(x.len(), 1);
        assert!(x[0] < 80);
    }

    #[test]
    fn assign_coordinates_linear_chain_aligned() {
        let sizes = vec![Size::new(14, 3), Size::new(14, 3), Size::new(14, 3)];
        let layers = vec![vec![0], vec![1], vec![2]];
        let adj = vec![vec![1], vec![2], vec![]];
        let x = assign_coordinates(&layers, &adj, &sizes, 6, 80, 0);
        let cx0 = x[0] + sizes[0].w / 2;
        let cx1 = x[1] + sizes[1].w / 2;
        let cx2 = x[2] + sizes[2].w / 2;
        assert_eq!(cx0, cx1, "A and B should be aligned: {} vs {}", cx0, cx1);
        assert_eq!(cx1, cx2, "B and C should be aligned: {} vs {}", cx1, cx2);
    }

    #[test]
    fn assign_coordinates_different_width_aligned() {
        // A(w=14) → B(w=16), should have same center.
        let sizes = vec![Size::new(14, 3), Size::new(16, 3)];
        let layers = vec![vec![0], vec![1]];
        let adj = vec![vec![1], vec![]];
        let x = assign_coordinates(&layers, &adj, &sizes, 6, 80, 0);
        let cx0 = x[0] + sizes[0].w / 2;
        let cx1 = x[1] + sizes[1].w / 2;
        assert_eq!(
            cx0, cx1,
            "different-width aligned boxes should share center: {} vs {}",
            cx0, cx1
        );
    }

    #[test]
    fn assign_coordinates_fork_children_distinct() {
        let sizes = vec![Size::new(14, 3), Size::new(14, 3), Size::new(14, 3)];
        let layers = vec![vec![0], vec![1, 2]];
        let adj = vec![vec![1, 2], vec![], vec![]];
        let x = assign_coordinates(&layers, &adj, &sizes, 6, 80, 0);
        assert_ne!(x[1], x[2], "fork children should be at different x");
    }

    #[test]
    fn compute_trans_geoms_cross_layer() {
        let layouts = vec![
            crate::diagrams::state::layout::StateLayout {
                id: "a".into(),
                label: "A".into(),
                state_type: crate::diagrams::state::types::StateType::Simple,
                rect: Rect::new(33, 2, 14, 3),
            },
            crate::diagrams::state::layout::StateLayout {
                id: "b".into(),
                label: "B".into(),
                state_type: crate::diagrams::state::types::StateType::Simple,
                rect: Rect::new(33, 8, 14, 3),
            },
        ];
        let transitions =
            vec![Transition { from: "a".into(), to: "b".into(), label: Some("go".into()) }];
        let mut id_map = HashMap::new();
        id_map.insert("a", 0);
        id_map.insert("b", 1);
        let geoms = compute_trans_geoms(&layouts, &transitions, &id_map, 80);
        assert_eq!(geoms.len(), 1);
        // Same column (cx=40), corridor_w=0, not embedded.
        assert_eq!(geoms[0].corridor_w, 0);
        assert!(!geoms[0].embed);
        assert!(!geoms[0].same_layer);
    }
}
