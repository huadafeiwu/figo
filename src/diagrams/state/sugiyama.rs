//! Sugiyama-style crossing reduction and coordinate assignment.
//!
//! Implements phases 2 (crossing reduction via median heuristic) and 3
//! (coordinate assignment via a simplified Brandes-Köpf alignment) of the
//! Sugiyama framework. Phase 1 (layering) and phase 4 (edge routing) are
//! handled by `layout.rs` and `render.rs` respectively.
//!
//! The goal is to produce x-coordinates where:
//! - Vertically connected states (linear chain, no fork) share the same
//!   column, so `|` legs are aligned.
//! - Fork children get distinct columns.
//! - Join nodes are centered between their predecessors.
//! - Column gaps are wide enough to embed transition labels.

use crate::diagrams::state::types::Transition;
use crate::render::widget::Size;
use std::collections::HashMap;

/// Phase 2: Reduce edge crossings using the median heuristic.
///
/// Iterates up (bottom-to-top) and down (top-to-bottom) sweeps, sorting
/// each layer's nodes by the median position of their neighbours in the
/// adjacent layer. Returns the best ordering found (lowest crossing count).
pub fn reduce_crossings(
    layers: &[Vec<usize>],
    adj: &[Vec<usize>],
    predecessors: &[Vec<usize>],
) -> Vec<Vec<usize>> {
    if layers.is_empty() {
        return Vec::new();
    }

    // Work on a mutable copy.
    let mut current: Vec<Vec<usize>> = layers.to_vec();
    let mut best = current.clone();
    let mut best_crossings = count_total_crossings(&current, adj, predecessors);

    const MAX_ROUNDS: usize = 24;
    for round in 0..MAX_ROUNDS {
        if round % 2 == 0 {
            // Bottom-up sweep: sort each layer by median of successors below.
            for i in (0..current.len().saturating_sub(1)).rev() {
                let (left, right) = current.split_at_mut(i + 1);
                sort_by_median(&mut left[i], &right[0], adj);
            }
        } else {
            // Top-down sweep: sort each layer by median of predecessors above.
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

/// Sort `layer` nodes by the median position of their neighbours in
/// `adjacent_layer`.
fn sort_by_median(layer: &mut [usize], adjacent: &[usize], neighbours_of: &[Vec<usize>]) {
    if layer.len() <= 1 {
        return;
    }

    // Build position map: node index → position in adjacent layer.
    let pos: HashMap<usize, usize> =
        adjacent.iter().enumerate().map(|(pos, &node)| (node, pos)).collect();

    // Compute median neighbour position for each node in layer.
    let medians: Vec<(usize, usize)> = layer
        .iter()
        .map(|&node| {
            let positions: Vec<usize> =
                neighbours_of[node].iter().filter_map(|nb| pos.get(nb).copied()).collect();
            let median = if positions.is_empty() {
                // No neighbours: keep relative order (use layer index).
                layer.iter().position(|&n| n == node).unwrap_or(0)
            } else {
                positions[positions.len() / 2]
            };
            (node, median)
        })
        .collect();

    // Sort by median (stable for equal medians).
    layer.sort_by_key(|&node| {
        medians.iter().find(|&&(n, _)| n == node).map(|&(_, m)| m).unwrap_or(0)
    });
}

/// Count total edge crossings across all adjacent layer pairs.
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

/// Count crossings between two adjacent layers.
fn count_layer_pair_crossings(
    upper: &[usize],
    lower: &[usize],
    adj: &[Vec<usize>],
    _predecessors: &[Vec<usize>],
) -> usize {
    let upper_pos: HashMap<usize, usize> = upper.iter().enumerate().map(|(p, &n)| (n, p)).collect();

    let mut crossings = 0;
    for (j, &lower_node) in lower.iter().enumerate() {
        for &upper_node in &_predecessors_placeholder(lower_node, adj) {
            let Some(&i) = upper_pos.get(&upper_node) else { continue };
            // Count how many edges from upper to lower cross this one.
            for (j2, &lower_node2) in lower.iter().enumerate() {
                if j2 <= j {
                    continue;
                }
                for &upper_node2 in &_predecessors_placeholder(lower_node2, adj) {
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

/// Helper: get predecessors of a node from the adjacency list (reverse lookup).
/// This builds predecessors on the fly since we only have `adj`.
fn _predecessors_placeholder(node: usize, adj: &[Vec<usize>]) -> Vec<usize> {
    let mut preds = Vec::new();
    for (i, succs) in adj.iter().enumerate() {
        if succs.contains(&node) {
            preds.push(i);
        }
    }
    preds
}

/// Phase 3: Assign x-coordinates using a simplified Brandes-Köpf approach.
///
/// 1. Build vertical alignment blocks (chains of nodes connected by
///    "median-direction" edges).
/// 2. Assign x-coordinates left-to-right, respecting minimum gaps.
/// 3. Center the whole layout in the canvas.
///
/// Returns a `Vec<usize>` of x-coordinates indexed by node index.
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

    // Build predecessor map.
    let mut predecessors: Vec<Vec<usize>> = vec![Vec::new(); node_count];
    for (i, succs) in adj.iter().enumerate() {
        for &s in succs {
            if s < node_count && i != s {
                predecessors[s].push(i);
            }
        }
    }

    // Build alignment: for each node, find the "aligned" predecessor
    // (the one whose median direction points to this node).
    // aligned_up[node] = the node above that this node aligns with (or None).
    let mut aligned_up: Vec<Option<usize>> = vec![None; node_count];
    let mut aligned_down: Vec<Option<usize>> = vec![None; node_count];

    for (layer_i, layer) in layers.iter().enumerate() {
        if layer_i == 0 {
            continue;
        }
        let upper = &layers[layer_i - 1];

        // For each node in upper layer, its successors' positions in this layer.
        let mut r = 0usize; // rightmost used position in current layer
        for &u in upper {
            let succs: Vec<usize> =
                adj[u].iter().filter(|&&s| layer.contains(&s)).copied().collect();
            if succs.is_empty() {
                continue;
            }
            // Median successor position.
            let positions: Vec<usize> =
                succs.iter().map(|s| layer.iter().position(|&n| n == *s).unwrap_or(0)).collect();
            let med = positions[positions.len() / 2];

            // Find the median-direction successor that is to the right of r.
            for &s in &succs {
                let p = layer.iter().position(|&n| n == s).unwrap_or(0);
                if p >= r && p <= med + (positions.len() / 2) {
                    // Align u → s.
                    if aligned_up[s].is_none() {
                        aligned_up[s] = Some(u);
                        aligned_down[u] = Some(s);
                        r = p + 1;
                        break;
                    }
                }
            }
        }
    }

    // Build blocks: each node belongs to a block (chain of aligned nodes).
    // Block root = topmost node in the chain.
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

    // Propagate block x to all members (ensure alignment).
    for n in 0..node_count {
        let root = block_root[n];
        if let Some(&bx) = block_x.get(&root) {
            x[n] = bx;
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

/// Phase 3b: Adjust x-coordinates so column gaps are wide enough for
/// transition labels.
///
/// For each transition with a label, the gap between from.x and to.x must
/// be at least `label_w + 4` (2 cells padding per side). This function
/// widens gaps by shifting nodes rightward as needed.
pub fn compute_column_gaps(
    x: &mut [usize],
    transitions: &[Transition],
    id_to_idx: &HashMap<&str, usize>,
    sizes: &[Size],
    min_gap: usize,
) {
    // Build a list of (from_idx, to_idx, required_gap) for each transition.
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
        if x[from_i] == x[to_i] {
            continue; // Same column, no gap needed.
        }
        if let Some(label) = &t.label {
            let lw = unicode_width::UnicodeWidthStr::width(label.as_str());
            let needed = lw + 4; // 2 padding per side
            gap_reqs.push((from_i, to_i, needed));
        } else {
            gap_reqs.push((from_i, to_i, min_gap));
        }
    }

    // Iteratively widen gaps. Sort by from x (leftmost first).
    gap_reqs.sort_by_key(|&(from_i, _, _)| x[from_i]);

    for &(from_i, to_i, needed) in &gap_reqs {
        let from_x = x[from_i];
        let from_w = sizes[from_i].w;
        let to_x = x[to_i];
        let to_w = sizes[to_i].w;

        let (left, left_w, right) =
            if from_x < to_x { (from_x, from_w, to_x) } else { (to_x, to_w, from_x) };

        let current_gap = right.saturating_sub(left + left_w);
        if current_gap < needed {
            let extra = needed - current_gap;
            // Shift the right node and all nodes to its right.
            let shift_from = right;
            for xi in x.iter_mut() {
                if *xi >= shift_from {
                    *xi += extra;
                }
            }
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
        // A → B → C, all in separate layers, should share same x.
        let sizes = vec![Size::new(14, 3), Size::new(14, 3), Size::new(14, 3)];
        let layers = vec![vec![0], vec![1], vec![2]];
        let adj = vec![vec![1], vec![2], vec![]];
        let x = assign_coordinates(&layers, &adj, &sizes, 6, 80, 0);
        // All three should be at the same x (centered).
        let cx0 = x[0] + sizes[0].w / 2;
        let cx1 = x[1] + sizes[1].w / 2;
        let cx2 = x[2] + sizes[2].w / 2;
        assert_eq!(cx0, cx1, "A and B should be aligned: {} vs {}", cx0, cx1);
        assert_eq!(cx1, cx2, "B and C should be aligned: {} vs {}", cx1, cx2);
    }

    #[test]
    fn assign_coordinates_fork_children_distinct() {
        // A → B, A → C, B and C in same layer.
        let sizes = vec![Size::new(14, 3), Size::new(14, 3), Size::new(14, 3)];
        let layers = vec![vec![0], vec![1, 2]];
        let adj = vec![vec![1, 2], vec![], vec![]];
        let x = assign_coordinates(&layers, &adj, &sizes, 6, 80, 0);
        // B and C should be at different x.
        assert_ne!(x[1], x[2], "fork children should be at different x");
    }
}
