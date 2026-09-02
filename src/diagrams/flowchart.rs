//! Flowchart diagrams with nodes and orthogonally-routed connections.
//!
//! Nodes support three shapes: `Rectangle`, `Rounded`, and `Diamond`
//! (decision). The layout engine stacks nodes vertically and routes
//! connectors orthogonally. Forward edges flow top-to-bottom; back-edges
//! (target above source) and forward edges whose every V-H-V corridor
//! row would pierce an obstacle both route via a side corridor to the
//! right of every node so they do not punch through intermediates.

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::canvas::{Canvas, Layer};
use crate::error::{FigoError, Result};
use crate::layout::connector::Connector;
use crate::layout::geom::{Anchor, Rect};
use crate::layout::{
    RAIL_OFFSET, RidingCandidate, RidingLabel, allocate_riding_rows, beside_line_label_avail,
    corridor_label_avail, corridor_label_block_top, forward_edge_side_routed, h_corridor_len,
    riding_placement_cols, side_route_column,
};
use crate::style::{BorderStyle, Charset, LineStyle};
use crate::text::wrap_label;
use unicode_width::UnicodeWidthStr;

use super::flowchart_riding::{PlacedRider, RidingRequest, place_riding_label};
use super::flowchart_shape::{draw_diamond, node_dims};

// Re-export NodeShape so the public API path `figo::diagrams::flowchart::NodeShape`
// stays stable even though the enum now lives in `flowchart_shape`.
pub use super::flowchart_shape::NodeShape;

/// A node in the flowchart.
#[derive(Debug, Clone)]
pub struct FlowNode {
    /// Unique id; matches a connection's `from`/`to`.
    pub id: String,
    /// Text shown inside the node.
    pub label: String,
    /// Visual shape.
    pub shape: NodeShape,
    /// `(x, y)` override; auto-layout assigns when `None`. Required for `Layout::Manual`.
    pub position: Option<(usize, usize)>,
}

/// A connection between two nodes.
#[derive(Debug, Clone)]
pub struct FlowConnection {
    /// Source node id.
    pub from: String,
    /// Target node id.
    pub to: String,
    /// Optional label drawn beside the connector.
    pub label: Option<String>,
}

/// Layout mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Automatic layered layout (default).
    Auto,
    /// User-supplied positions.
    Manual,
}

/// Builder for flowchart diagrams.
pub struct Flowchart {
    width: usize,
    charset: Charset,
    nodes: Vec<FlowNode>,
    connections: Vec<FlowConnection>,
    layout: Layout,
    color: bool,
}

#[derive(Clone)]
struct PositionedNode {
    node: FlowNode,
    rect: Rect,
}

/// Vertical stride (rows) between successive nodes in auto-layout.
/// Shared by `layout_auto` and `render_positions` canvas sizing.
const AUTO_STRIDE_ROWS: usize = 4;

/// Bottom margin below the deepest structure on the canvas (breathing
/// room, one stride, and slack). Shared by the canvas-height
/// calculation and the riding row limit so the two never diverge.
const CANVAS_BOTTOM_MARGIN: usize = 2 + AUTO_STRIDE_ROWS + 4;

/// Floor for the right-hand reservation beyond the widest node: always
/// room for the side rail (`RAIL_OFFSET`) plus a short label. Also the
/// provisional width headroom used to pin the rail column before the
/// final `side_room_for_side_routes` is known (see `render_positions`).
const MIN_SIDE_ROOM: usize = 6;

/// Resolve a connection's two endpoint nodes, `None` when either id
/// dangles (no matching node). Shared by every pre-pass and Phase 2 in
/// `render_positions` so endpoint resolution cannot drift.
fn conn_endpoints<'a>(
    pos_map: &HashMap<&str, &'a PositionedNode>,
    conn: &FlowConnection,
) -> Option<(&'a PositionedNode, &'a PositionedNode)> {
    Some((pos_map.get(conn.from.as_str()).copied()?, pos_map.get(conn.to.as_str()).copied()?))
}

/// Every node rect except the connection's two endpoints — the obstacle
/// set a connector must avoid. Rect equality, not id equality: the
/// layout never stacks two nodes on the same rect, and the riding
/// pre-pass historically matched by rect.
fn avoid_rects(
    positions: &[PositionedNode],
    from: &PositionedNode,
    to: &PositionedNode,
) -> Vec<Rect> {
    positions.iter().map(|p| p.rect).filter(|r| *r != from.rect && *r != to.rect).collect()
}

impl Flowchart {
    /// Create a new flowchart builder.
    pub fn new(width: usize, charset: Charset) -> Self {
        Self {
            width,
            charset,
            nodes: Vec::new(),
            connections: Vec::new(),
            layout: Layout::Auto,
            color: false,
        }
    }

    /// Add a node.
    pub fn add_node(mut self, node: FlowNode) -> Self {
        self.nodes.push(node);
        self
    }

    /// Add a connection between two nodes.
    pub fn connect(mut self, from: &str, to: &str, label: Option<&str>) -> Self {
        self.connections.push(FlowConnection {
            from: from.to_string(),
            to: to.to_string(),
            label: label.map(String::from),
        });
        self
    }

    /// Set the layout mode.
    pub fn layout(mut self, layout: Layout) -> Self {
        self.layout = layout;
        self
    }

    /// Enable or disable color output.
    pub fn color(mut self, enabled: bool) -> Self {
        self.color = enabled;
        self
    }

    /// Render and return as a `String`.
    pub fn build(&self) -> Result<String> {
        if self.nodes.is_empty() {
            return Err(FigoError::MissingFields("flowchart must have nodes".into()));
        }
        let positions = match self.layout {
            Layout::Manual => self.layout_manual()?,
            Layout::Auto => self.layout_auto()?,
        };
        self.render_positions(&positions)
    }

    fn layout_manual(&self) -> Result<Vec<PositionedNode>> {
        let mut out = Vec::new();
        for node in &self.nodes {
            let pos = node.position.ok_or_else(|| {
                FigoError::MissingFields(format!(
                    "node '{}' has no position in manual layout",
                    node.id
                ))
            })?;
            let (w, h) = node_dims(&node.label, node.shape, self.width);
            out.push(PositionedNode { node: node.clone(), rect: Rect::new(pos.0, pos.1, w, h) });
        }
        Ok(out)
    }

    fn layout_auto(&self) -> Result<Vec<PositionedNode>> {
        let dims: Vec<(usize, usize)> =
            self.nodes.iter().map(|n| node_dims(&n.label, n.shape, self.width)).collect();
        let adj = self.build_adjacency();
        let (order, back_edges) = self.detect_back_edges(&adj);
        let layers = self.compute_layers(&adj, &order, &back_edges);
        self.assign_positions(&dims, &layers)
    }

    /// Build adjacency list by node index from the declared connections.
    fn build_adjacency(&self) -> Vec<Vec<usize>> {
        let id_to_idx: HashMap<&str, usize> =
            self.nodes.iter().enumerate().map(|(i, n)| (n.id.as_str(), i)).collect();
        let mut adj = vec![Vec::new(); self.nodes.len()];
        for conn in &self.connections {
            if let (Some(&from), Some(&to)) =
                (id_to_idx.get(conn.from.as_str()), id_to_idx.get(conn.to.as_str()))
            {
                adj[from].push(to);
            }
        }
        adj
    }

    /// Detect back-edges via DFS so the remaining graph is a DAG.
    /// Returns the topological order (post-order) and the set of back-edges.
    fn detect_back_edges(&self, adj: &[Vec<usize>]) -> (Vec<usize>, HashSet<(usize, usize)>) {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum State {
            Unvisited,
            Visiting,
            Visited,
        }
        let mut states = vec![State::Unvisited; self.nodes.len()];
        let mut back_edges: HashSet<(usize, usize)> = HashSet::new();
        let mut order = Vec::new();

        fn dfs(
            node: usize,
            adj: &[Vec<usize>],
            states: &mut Vec<State>,
            back_edges: &mut HashSet<(usize, usize)>,
            order: &mut Vec<usize>,
        ) {
            states[node] = State::Visiting;
            for &next in &adj[node] {
                match states[next] {
                    State::Unvisited => dfs(next, adj, states, back_edges, order),
                    State::Visiting => {
                        back_edges.insert((node, next));
                    }
                    State::Visited => {}
                }
            }
            states[node] = State::Visited;
            order.push(node);
        }

        for i in 0..self.nodes.len() {
            if matches!(states[i], State::Unvisited) {
                dfs(i, adj, &mut states, &mut back_edges, &mut order);
            }
        }
        (order, back_edges)
    }

    /// Assign layers by longest path in the DAG (ignoring back-edges).
    fn compute_layers(
        &self,
        adj: &[Vec<usize>],
        order: &[usize],
        back_edges: &HashSet<(usize, usize)>,
    ) -> Vec<usize> {
        let mut layers = vec![0usize; self.nodes.len()];
        for &node in order.iter().rev() {
            for &next in &adj[node] {
                if back_edges.contains(&(node, next)) {
                    continue;
                }
                let candidate = layers[node] + 1;
                if candidate > layers[next] {
                    layers[next] = candidate;
                }
            }
        }
        layers
    }

    /// Group nodes by layer and compute their (x, y) coordinates.
    fn assign_positions(
        &self,
        dims: &[(usize, usize)],
        layers: &[usize],
    ) -> Result<Vec<PositionedNode>> {
        let max_layer = *layers.iter().max().unwrap_or(&0);
        let mut layers_map: HashMap<usize, Vec<usize>> = HashMap::new();
        for (idx, &layer) in layers.iter().enumerate() {
            layers_map.entry(layer).or_default().push(idx);
        }

        // Sort each layer by original node index for stable ordering.
        let mut layer_positions: Vec<Vec<usize>> = Vec::new();
        for layer in 0..=max_layer {
            if let Some(mut indices) = layers_map.remove(&layer) {
                indices.sort();
                layer_positions.push(indices);
            }
        }

        // Pass 1: x positions per layer. Single-node layers align to the
        // global canvas center so vertical connectors stay in one column;
        // multi-node layers center their content. Nodes wider than the
        // canvas (e.g. diamonds, which never shrink to wrap a label) start
        // at the left edge and the canvas grows around them.
        let id_to_idx: HashMap<&str, usize> =
            self.nodes.iter().enumerate().map(|(i, n)| (n.id.as_str(), i)).collect();
        let id_to_layer: HashMap<usize, usize> =
            self.nodes.iter().enumerate().map(|(i, _)| (i, layers[i])).collect();
        let mut xs: Vec<usize> = vec![0; self.nodes.len()];
        for layer_indices in layer_positions.iter() {
            // gap_x: default 6, expanded when a same-layer connection
            // label needs the extra corridor width.
            let mut gap_x = 6usize;
            for i in 0..layer_indices.len().saturating_sub(1) {
                let left_idx = layer_indices[i];
                let right_idx = layer_indices[i + 1];
                for conn in &self.connections {
                    let (Some(&from_i), Some(&to_i)) =
                        (id_to_idx.get(conn.from.as_str()), id_to_idx.get(conn.to.as_str()))
                    else {
                        continue;
                    };
                    let is_pair = (from_i == left_idx && to_i == right_idx)
                        || (from_i == right_idx && to_i == left_idx);
                    if !is_pair {
                        continue;
                    }
                    if let Some(label) = &conn.label {
                        let lw = unicode_width::UnicodeWidthStr::width(label.as_str());
                        let needed = lw + 4;
                        if needed > gap_x {
                            gap_x = needed.min(self.width / 2);
                        }
                    }
                }
            }
            let total_w: usize = layer_indices.iter().map(|&idx| dims[idx].0).sum::<usize>()
                + layer_indices.len().saturating_sub(1) * gap_x;
            let start_x = if layer_indices.len() == 1 {
                (self.width / 2).saturating_sub(dims[layer_indices[0]].0 / 2)
            } else {
                self.width.saturating_sub(total_w) / 2
            };
            let mut x = start_x;
            for &idx in layer_indices {
                xs[idx] = x;
                x += dims[idx].0 + gap_x;
            }
        }

        // Pass 2: wrapped label line counts per layer gap, using the SAME
        // avail formulas the draw side (Connector::draw_label) derives
        // from the routed geometry: the V-H-V corridor spans both leg
        // columns inclusive, and pure vertical connectors bound the label
        // by the canvas. Estimating with any other width under-sizes the
        // stride, and the drawn block then gets clamped into the corridor
        // — silently losing label lines.
        let mut max_label_lines_per_gap: HashMap<usize, usize> = HashMap::new();
        // Fork-riding data per source node: a purely vertical connector
        // anchors its label at the row just below the source — exactly
        // where same-source corridor siblings place their corridor row —
        // so its label must ride the exclusive leg segment below every
        // sibling label block instead. Track each source's corridor
        // siblings (far leg column + wrapped line count).
        let mut source_corridor_sibs: HashMap<&str, Vec<(usize, usize)>> = HashMap::new();
        for conn in &self.connections {
            let (Some(&from_idx), Some(&to_idx)) =
                (id_to_idx.get(conn.from.as_str()), id_to_idx.get(conn.to.as_str()))
            else {
                continue;
            };
            let from_layer = id_to_layer.get(&from_idx).copied().unwrap_or(0);
            let to_layer = id_to_layer.get(&to_idx).copied().unwrap_or(0);
            if from_layer >= to_layer {
                continue; // back-edge / same-layer: side route, no stride need
            }
            let from_cx = xs[from_idx] + dims[from_idx].0 / 2;
            let to_cx = xs[to_idx] + dims[to_idx].0 / 2;
            let avail = if from_cx != to_cx {
                corridor_label_avail(h_corridor_len(from_cx, to_cx), self.width)
            } else {
                beside_line_label_avail(from_cx, self.width)
            };
            let n = conn.label.as_ref().map(|l| wrap_label(l, avail).line_count).unwrap_or(0);
            if from_cx != to_cx {
                source_corridor_sibs.entry(conn.from.as_str()).or_default().push((to_cx, n));
            }
            if conn.label.is_some() {
                // The stride below a layer must fit every label block whose
                // corridor leaves that layer.
                let gap = from_layer;
                max_label_lines_per_gap.entry(gap).and_modify(|v| *v = (*v).max(n)).or_insert(n);
            }
        }

        // Fork-riding stride demand: the riding label lives in the layer
        // gap just above its target box (for skip-layer connectors the
        // block lands below the transit layers). Same-gap forks also
        // carry the sibling corridor label's spill below the corridor
        // row. Riders whose spans overlap stack vertically (the same
        // `allocate_riding_rows` the draw-side placement uses), so the
        // stride must fit the tallest cluster stack, not one rider.
        let mut riding_stride_demand: HashMap<usize, usize> = HashMap::new();
        {
            // Per gap: riders as (span, lines, same-gap corridor spill).
            let mut gap_riders: HashMap<usize, Vec<(RidingCandidate, usize)>> = HashMap::new();
            for conn in &self.connections {
                let Some(label) = &conn.label else { continue };
                let (Some(&from_idx), Some(&to_idx)) =
                    (id_to_idx.get(conn.from.as_str()), id_to_idx.get(conn.to.as_str()))
                else {
                    continue;
                };
                let from_layer = id_to_layer.get(&from_idx).copied().unwrap_or(0);
                let to_layer = id_to_layer.get(&to_idx).copied().unwrap_or(0);
                if from_layer >= to_layer {
                    continue;
                }
                let from_cx = xs[from_idx] + dims[from_idx].0 / 2;
                let to_cx = xs[to_idx] + dims[to_idx].0 / 2;
                if from_cx != to_cx {
                    continue; // corridor connector: no riding
                }
                let Some(sibs) = source_corridor_sibs.get(conn.from.as_str()) else { continue };
                let far_cols: Vec<usize> = sibs.iter().map(|&(c, _)| c).collect();
                let avail = beside_line_label_avail(from_cx, self.width);
                let (wrap_w, _) = riding_placement_cols(&far_cols, from_cx, self.width, avail);
                let wrapped = wrap_label(label, wrap_w);
                let lo = from_cx.saturating_sub(wrap_w / 2);
                // Same-gap forks share the gap with their sibling
                // corridor labels' spill below the corridor row.
                let spill = if to_layer == from_layer + 1 {
                    sibs.iter().map(|&(_, n)| n).max().unwrap_or(0).saturating_sub(2)
                } else {
                    0
                };
                gap_riders.entry(to_layer - 1).or_default().push((
                    RidingCandidate {
                        span_lo: lo,
                        span_hi: lo + wrap_w,
                        lines: wrapped.line_count,
                    },
                    spill,
                ));
            }
            for (gap, riders) in gap_riders {
                let candidates: Vec<RidingCandidate> = riders.iter().map(|(c, _)| *c).collect();
                let (_, demand) = allocate_riding_rows(&candidates);
                // One `|` rail rides on each side of the block, plus
                // the same-gap corridor spill.
                let spill = riders.iter().map(|(_, s)| *s).max().unwrap_or(0);
                let d = demand + 4 + spill;
                riding_stride_demand.entry(gap).and_modify(|v| *v = (*v).max(d)).or_insert(d);
            }
        }

        // Pass 3: y positions. The stride after each layer grows to fit
        // the tallest label block crossing that gap.
        let mut y = 1usize;
        let mut out: Vec<Option<PositionedNode>> = vec![None; self.nodes.len()];
        for (layer_i, layer_indices) in layer_positions.iter().enumerate() {
            let max_h = layer_indices.iter().map(|&idx| dims[idx].1).max().unwrap_or(0);
            for &idx in layer_indices {
                out[idx] = Some(PositionedNode {
                    node: self.nodes[idx].clone(),
                    rect: Rect::new(xs[idx], y, dims[idx].0, dims[idx].1),
                });
            }
            let label_lines = max_label_lines_per_gap.get(&layer_i).copied().unwrap_or(0).max(1);
            let mut stride = AUTO_STRIDE_ROWS.max(label_lines + 2);
            if let Some(&demand) = riding_stride_demand.get(&layer_i) {
                // Fork-riding labels live below the sibling corridor
                // block: fit its spill, one `|` rail on each side of the
                // riding block, and the block itself.
                stride = stride.max(demand);
            }
            y += max_h + stride;
        }

        Ok(out.into_iter().map(Option::unwrap).collect())
    }

    fn render_positions(&self, positions: &[PositionedNode]) -> Result<String> {
        let pos_map: HashMap<&str, &PositionedNode> =
            positions.iter().map(|p| (p.node.id.as_str(), p)).collect();
        let all_rects: Vec<Rect> = positions.iter().map(|p| p.rect).collect();

        let max_w = positions.iter().map(|p| p.rect.right()).max().unwrap_or(0).max(self.width);
        // Forward edges whose every V-H-V corridor row would pierce an
        // obstacle route around the right side on the same rail the
        // back-edges use. Decide them up front: the fork-sibling and
        // riding pre-passes below must ignore them (their labels ride
        // the rail, not the fork), and their labels widen the right
        // reservation. `side_room_for_side_routes` never returns less
        // than MIN_SIDE_ROOM, so the provisional width below never
        // clamps the rail column — it stays at max_right + RAIL_OFFSET
        // here and when Phase 2 recomputes it against the final canvas.
        let rail_x = side_route_column(&all_rects, max_w + MIN_SIDE_ROOM);
        let mut side_routed: HashSet<usize> = HashSet::new();
        let mut side_label_w = 0usize;
        for (ci, conn) in self.connections.iter().enumerate() {
            let Some((from, to)) = conn_endpoints(&pos_map, conn) else {
                continue;
            };
            if from.rect.y >= to.rect.y {
                continue; // back-edges already take the side rail
            }
            let avoid = avoid_rects(positions, from, to);
            if forward_edge_side_routed(&from.rect, &to.rect, &avoid, rail_x) {
                side_routed.insert(ci);
                if let Some(label) = &conn.label {
                    // Wrapped width bounded by self.width, matching the
                    // back-edge reservation below.
                    side_label_w = side_label_w.max(wrap_label(label, self.width).max_width);
                }
            }
        }
        // Reserve room on the right for side corridors and their labels.
        let side_room = self.side_room_for_side_routes(positions, side_label_w);
        let total_w = max_w + side_room;

        // Fork-riding aggregation: a purely vertical forward connector
        // (same center column as its target) anchors its label at the
        // row just below the source — exactly where same-source corridor
        // siblings place their corridor row (`natural_mid_y` = bottom +
        // 1). The two label blocks then interleave on the same rows and
        // characters are lost. Record each source's corridor siblings
        // (far leg column, label block bottom row) so the vertical
        // connector's label can ride the exclusive leg segment below
        // every sibling block instead.
        let mut fork_sibs: HashMap<&str, Vec<(usize, usize)>> = HashMap::new();
        for (ci, conn) in self.connections.iter().enumerate() {
            let Some((from, to)) = conn_endpoints(&pos_map, conn) else {
                continue;
            };
            if from.rect.y >= to.rect.y {
                continue; // back-edge: side route, never meets the fork
            }
            if side_routed.contains(&ci) {
                continue; // side rail: its label rides the rail, not the fork
            }
            let from_cx = from.rect.x + from.rect.w / 2;
            let to_cx = to.rect.x + to.rect.w / 2;
            if from_cx == to_cx {
                continue; // vertical connector: the rider, not the sibling
            }
            let (sy, ty) = (from.rect.bottom(), to.rect.y);
            // Corridor row and label block bottom, using the same
            // formulas the sibling's own draw pass uses.
            let block_bottom = match &conn.label {
                Some(label) => {
                    let avail = corridor_label_avail(h_corridor_len(from_cx, to_cx), self.width);
                    let n = wrap_label(label, avail).line_count;
                    corridor_label_block_top(sy + 1, n, sy, ty) + n - 1
                }
                None => sy + 1,
            };
            fork_sibs.entry(conn.from.as_str()).or_default().push((to_cx, block_bottom));
        }

        // Fork-riding placement pre-pass, in connection declaration
        // order: each rider's stretch search treats the riders already
        // placed in this pass as blockers (the same rows are never
        // handed to two riders), and a block taller than every clear
        // on-line stretch falls back beside the blocker cluster (see
        // `flowchart_riding`). Placing before the canvas is sized lets
        // every block bottom count toward the canvas height exactly
        // like a box bottom, so no block is silently dropped past the
        // canvas edge.
        let box_bottom = positions.iter().map(|p| p.rect.bottom()).max().unwrap_or(10);
        let row_limit = box_bottom + CANVAS_BOTTOM_MARGIN;
        let mut placed_riders: Vec<PlacedRider> = Vec::new();
        let mut riding: HashMap<usize, RidingLabel> = HashMap::new();
        let mut riding_bottom = 0usize;
        for (ci, conn) in self.connections.iter().enumerate() {
            let Some((from, to)) = conn_endpoints(&pos_map, conn) else {
                continue;
            };
            if from.rect.y >= to.rect.y {
                continue; // back-edge: side route, never rides
            }
            if side_routed.contains(&ci) {
                continue; // side rail: label drawn by render_side_route
            }
            let Some(label) = conn.label.as_deref() else { continue };
            let from_cx = from.rect.x + from.rect.w / 2;
            let to_cx = to.rect.x + to.rect.w / 2;
            if from_cx != to_cx {
                continue; // corridor connector: label embeds in the corridor
            }
            let Some(sibs) = fork_sibs.get(conn.from.as_str()) else { continue };
            let boxes = avoid_rects(positions, from, to);
            let placement = place_riding_label(&RidingRequest {
                label,
                from_rect: from.rect,
                to_rect: to.rect,
                sibs,
                boxes: &boxes,
                placed: &placed_riders,
                user_width: self.width,
                canvas_w: total_w,
                row_limit,
            });
            riding_bottom = riding_bottom.max(placement.bottom);
            placed_riders.push(PlacedRider {
                span: placement.span,
                rows: (placement.riding.block_top, placement.bottom),
            });
            riding.insert(ci, placement.riding);
        }

        // Canvas height: riding block bottoms are treated exactly like
        // box bottoms — same margin expression.
        let max_h = box_bottom.max(riding_bottom) + CANVAS_BOTTOM_MARGIN;
        let mut canvas = Canvas::new(total_w, max_h);

        // Phase 1 — node borders and labels.
        for pos in positions {
            self.draw_node(&mut canvas, pos);
        }

        // Phase 2 — connectors. Riding directives come from the
        // pre-pass above (one placement per rider, blockers included).
        for (ci, conn) in self.connections.iter().enumerate() {
            let Some((from, to)) = conn_endpoints(&pos_map, conn) else {
                continue;
            };
            // Treat same-layer connections as back-edges so they route around
            // nodes instead of punching straight through them.
            let is_back = from.rect.y >= to.rect.y;
            let mut connector = Connector::new(
                from.rect,
                to.rect,
                Anchor::South,
                Anchor::North,
                LineStyle::Simple,
                self.charset,
            )
            .with_avoids(avoid_rects(positions, from, to))
            .with_user_width(self.width);
            if let Some(label) = &conn.label {
                connector.label = Some(label.clone());
            }
            if let Some(rl) = riding.get(&ci) {
                connector = connector.with_riding_label(*rl);
            }
            if is_back || side_routed.contains(&ci) {
                let w = canvas.width();
                connector.render_side_route(&mut canvas, &all_rects, w);
            } else {
                connector.render(&mut canvas);
            }
        }

        // Phase 3 — repair connector junctions so corners and crossings
        // use proper Unicode box-drawing glyphs.
        canvas.repair_connector_junctions(LineStyle::Simple, self.charset);

        Ok(canvas.render(self.color))
    }

    /// Draw a single node (border + centered label) onto the canvas.
    fn draw_node(&self, canvas: &mut Canvas, pos: &PositionedNode) {
        match pos.node.shape {
            NodeShape::Diamond => {
                // draw_diamond fills the interior at NodeContent, hiding
                // connectors routed behind the node exactly like the
                // rectangle fill below.
                draw_diamond(canvas, pos.rect.x, pos.rect.y, pos.rect.w, pos.rect.h, self.charset);
            }
            NodeShape::Rounded | NodeShape::Rectangle => {
                let style = if pos.node.shape == NodeShape::Rounded {
                    BorderStyle::Rounded
                } else {
                    BorderStyle::Single
                };
                // draw_border only errors on sub-2x2 rects, which node_dims
                // prevents; discard the impossible error rather than propagate.
                let _ = canvas.draw_border(
                    pos.rect.x,
                    pos.rect.y,
                    pos.rect.w,
                    pos.rect.h,
                    style,
                    self.charset,
                );
                // Fill the interior so connectors routed behind the node are
                // hidden by the higher-layer background.
                for ry in 1..pos.rect.h.saturating_sub(1) {
                    canvas.put_horizontal_layered(
                        pos.rect.x + 1,
                        pos.rect.y + ry,
                        pos.rect.w.saturating_sub(2),
                        ' ',
                        Layer::NodeContent,
                    );
                }
            }
        }
        // Centered label. Wrap to multiple lines if it doesn't fit the box.
        let inner_w = pos.rect.w.saturating_sub(2).max(2);
        let label_lines = wrap_label(&pos.node.label, inner_w).lines;
        let num_lines = label_lines.len().max(1);
        let content_h = pos.rect.h.saturating_sub(2);
        let start_y = pos.rect.y + 1 + content_h.saturating_sub(num_lines) / 2;
        for (i, line) in label_lines.iter().enumerate() {
            let line_w = UnicodeWidthStr::width(line.as_str());
            let lx = pos.rect.x + (pos.rect.w.saturating_sub(line_w)) / 2;
            canvas.put_str_layered(lx, start_y + i, line, Layer::NodeContent, None);
        }
    }

    /// Compute the extra columns needed on the right for side corridors
    /// — back-edges and side-routed forward edges share one rail — and
    /// their labels: the rail offset plus one gap cell, plus the longest
    /// wrapped label placed to the right of the rail. `extra_label_w`
    /// carries the side-routed forward edges' wrapped label width (the
    /// back-edge scan below cannot see those — whether an edge
    /// side-routes is only known after positions, in
    /// `render_positions`).
    fn side_room_for_side_routes(
        &self,
        positions: &[PositionedNode],
        extra_label_w: usize,
    ) -> usize {
        let id_y: HashMap<&str, usize> =
            positions.iter().map(|p| (p.node.id.as_str(), p.rect.y)).collect();
        let back_edge_label_w = self
            .connections
            .iter()
            .filter(|c| {
                matches!(
                    (id_y.get(c.from.as_str()), id_y.get(c.to.as_str())),
                    (Some(fy), Some(ty)) if fy > ty
                )
            })
            .map(|c| {
                c.label
                    .as_ref()
                    .map(|l| {
                        // Use the wrapped width (bounded by self.width) not
                        // the full unwrapped label width.
                        wrap_label(l, self.width).max_width
                    })
                    .unwrap_or(0)
            })
            .max()
            .unwrap_or(0);
        (RAIL_OFFSET + 1 + back_edge_label_w.max(extra_label_w)).max(MIN_SIDE_ROOM)
    }

    pub fn render(&self) -> Result<String> {
        self.build()
    }
}

impl fmt::Display for Flowchart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.build() {
            Ok(s) => write!(f, "{s}"),
            Err(e) => write!(f, "[figo error: {e}]"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_flowchart() {
        let fc = Flowchart::new(80, Charset::Unicode)
            .add_node(FlowNode {
                id: "a".into(),
                label: "Start".into(),
                shape: NodeShape::Rounded,
                position: None,
            })
            .add_node(FlowNode {
                id: "b".into(),
                label: "End".into(),
                shape: NodeShape::Rounded,
                position: None,
            })
            .connect("a", "b", None);
        let out = fc.build().unwrap();
        assert!(out.contains("Start"));
        assert!(out.contains("End"));
    }

    #[test]
    fn test_empty_nodes() {
        assert!(Flowchart::new(80, Charset::Unicode).build().is_err());
    }

    #[test]
    fn test_diamond_shape_renders() {
        // The spec's primary example uses a diamond decision node.
        let fc = Flowchart::new(80, Charset::Unicode)
            .add_node(FlowNode {
                id: "start".into(),
                label: "Start".into(),
                shape: NodeShape::Rounded,
                position: None,
            })
            .add_node(FlowNode {
                id: "decision".into(),
                label: "Is valid?".into(),
                shape: NodeShape::Diamond,
                position: None,
            })
            .add_node(FlowNode {
                id: "end".into(),
                label: "End".into(),
                shape: NodeShape::Rounded,
                position: None,
            })
            .connect("start", "decision", None)
            .connect("decision", "end", Some("yes"));
        let out = fc.build().unwrap();
        assert!(out.contains("Is valid?"), "diamond label missing:\n{out}");
        // Diamond border uses ^ (top apex) and v (bottom apex).
        assert!(out.contains('^'), "diamond top apex missing:\n{out}");
        assert!(out.contains('v'), "diamond bottom apex missing:\n{out}");
    }

    #[test]
    fn test_back_edge_routes_via_side() {
        // decision -> process (back-edge, target above source) must NOT
        // punch a vertical line straight through the gap. The side route
        // pushes the vertical leg to the right of every node.
        let fc = Flowchart::new(80, Charset::Unicode)
            .add_node(FlowNode {
                id: "start".into(),
                label: "Start".into(),
                shape: NodeShape::Rounded,
                position: None,
            })
            .add_node(FlowNode {
                id: "process".into(),
                label: "Process".into(),
                shape: NodeShape::Rectangle,
                position: None,
            })
            .add_node(FlowNode {
                id: "decision".into(),
                label: "Valid?".into(),
                shape: NodeShape::Diamond,
                position: None,
            })
            .connect("start", "process", None)
            .connect("process", "decision", None)
            .connect("decision", "process", Some("no"));
        let out = fc.build().unwrap();
        // The "no" label must appear (side-route draws it by the corridor).
        assert!(out.contains("no"), "back-edge label 'no' missing:\n{out}");
    }

    #[test]
    fn test_fork_riding_label_no_char_loss() {
        // A purely vertical connector (A→C, same center column) shares
        // its source's south anchor row with corridor siblings (A→B):
        // the vertical label used to anchor exactly on the sibling
        // corridor row (natural_mid_y = bottom + 1), interleaving with
        // and overwriting the sibling label — characters were lost. The
        // vertical label now rides an exclusive stretch of its leg
        // (clear of sibling blocks and intermediate boxes): every
        // character of both labels survives, on rows of their own.
        let fc = Flowchart::new(60, Charset::Ascii)
            .add_node(FlowNode {
                id: "A".into(),
                label: "A".into(),
                shape: NodeShape::Rectangle,
                position: None,
            })
            .add_node(FlowNode {
                id: "X".into(),
                label: "X".into(),
                shape: NodeShape::Rectangle,
                position: None,
            })
            .add_node(FlowNode {
                id: "B".into(),
                label: "B".into(),
                shape: NodeShape::Rectangle,
                position: None,
            })
            .add_node(FlowNode {
                id: "C".into(),
                label: "C".into(),
                shape: NodeShape::Rectangle,
                position: None,
            })
            .connect("A", "C", Some("AAAAAAAAAAAAAAAA"))
            .connect("A", "B", Some("BBBBBBBBBBBBBB"))
            .connect("A", "X", None)
            .connect("X", "C", None);
        let out = fc.build().unwrap();
        // 16 label A's + the node label "A"; 14 label B's + node "B".
        assert!(out.matches('A').count() >= 17, "riding label A chars lost:\n{out}");
        assert!(out.matches('B').count() >= 15, "corridor label B chars lost:\n{out}");
        // The riding label's rows never interleave with the sibling's.
        for line in out.lines() {
            if line.contains("AAAA") {
                assert!(!line.contains('B'), "labels interleaved on one row:\n{line}");
            }
        }
    }

    #[test]
    fn test_decision_fork_labels_stay_apart() {
        // The common decision-diamond fork: the aligned edge's label
        // ("retry again") used to fuse with the two corridor labels on
        // the fork row into an unreadable single row. It now rides its
        // own leg stretch below the branch boxes, unsplit and separate.
        let fc = Flowchart::new(60, Charset::Ascii)
            .add_node(FlowNode {
                id: "D".into(),
                label: "Check?".into(),
                shape: NodeShape::Diamond,
                position: None,
            })
            .add_node(FlowNode {
                id: "X".into(),
                label: "X".into(),
                shape: NodeShape::Rectangle,
                position: None,
            })
            .add_node(FlowNode {
                id: "Y".into(),
                label: "Y".into(),
                shape: NodeShape::Rectangle,
                position: None,
            })
            .add_node(FlowNode {
                id: "Z".into(),
                label: "Z".into(),
                shape: NodeShape::Rectangle,
                position: None,
            })
            .connect("D", "Z", Some("retry again"))
            .connect("D", "Y", Some("yes"))
            .connect("D", "X", Some("no"))
            .connect("X", "Z", None)
            .connect("Y", "Z", None);
        let out = fc.build().unwrap();
        let row = out.lines().find(|l| l.contains("retry again")).expect("label missing");
        assert!(
            !row.contains("yes") && !row.contains("no"),
            "aligned label fused with corridor labels:\n{row}"
        );
        insta::assert_snapshot!(out);
    }

    /// Regression for z-layer protection: a connector's vertical leg
    /// passes through B's column, but B's borders (Layer::NodeBorder=3)
    /// and label (Layer::NodeContent=5) must remain visible because both
    /// z-layers outrank Layer::Connector=1.
    fn three_node_stack() -> Flowchart {
        Flowchart::new(40, Charset::Unicode)
            .layout(Layout::Manual)
            .add_node(FlowNode {
                id: "A".into(),
                label: "A".into(),
                shape: NodeShape::Rectangle,
                position: Some((17, 1)),
            })
            .add_node(FlowNode {
                id: "B".into(),
                label: "B".into(),
                shape: NodeShape::Rectangle,
                position: Some((17, 9)),
            })
            .add_node(FlowNode {
                id: "C".into(),
                label: "C".into(),
                shape: NodeShape::Rectangle,
                position: Some((17, 17)),
            })
            // A -> C is a forward edge whose vertical leg spans B's row.
            .connect("A", "C", None)
    }

    #[test]
    fn test_intermediate_node_survives_connector() {
        let out = three_node_stack().build().unwrap();
        // Every label must survive.
        for label in ['A', 'B', 'C'] {
            assert!(out.contains(label), "node label {label:?} missing:\n{out}");
        }
        // Rectangle border glyphs must remain — the connector never
        // produces them and must not overwrite them.
        for glyph in ['┌', '┐', '└', '┘'] {
            assert!(
                out.contains(glyph),
                "rectangle border glyph {glyph:?} missing — NodeBorder \
                 may have been overwritten by Connector:\n{out}",
            );
        }
    }

    #[test]
    fn test_multi_branch_places_children_side_by_side() {
        let fc = Flowchart::new(80, Charset::Unicode)
            .add_node(FlowNode {
                id: "a".into(),
                label: "A".into(),
                shape: NodeShape::Rounded,
                position: None,
            })
            .add_node(FlowNode {
                id: "b".into(),
                label: "B".into(),
                shape: NodeShape::Rounded,
                position: None,
            })
            .add_node(FlowNode {
                id: "c".into(),
                label: "C".into(),
                shape: NodeShape::Rounded,
                position: None,
            })
            .connect("a", "b", None)
            .connect("a", "c", None);
        let out = fc.build().unwrap();
        assert!(out.contains("B"), "child B missing:\n{out}");
        assert!(out.contains("C"), "child C missing:\n{out}");
        // Both children should appear in the rendered output.
    }

    #[test]
    fn test_cyclic_layout_renders_without_infinite_loop() {
        let fc = Flowchart::new(80, Charset::Unicode)
            .add_node(FlowNode {
                id: "a".into(),
                label: "A".into(),
                shape: NodeShape::Rounded,
                position: None,
            })
            .add_node(FlowNode {
                id: "b".into(),
                label: "B".into(),
                shape: NodeShape::Rounded,
                position: None,
            })
            .add_node(FlowNode {
                id: "c".into(),
                label: "C".into(),
                shape: NodeShape::Rounded,
                position: None,
            })
            .connect("a", "b", None)
            .connect("b", "a", Some("retry"))
            .connect("b", "c", None);
        let out = fc.build().unwrap();
        assert!(out.contains("A"), "node A missing:\n{out}");
        assert!(out.contains("B"), "node B missing:\n{out}");
        assert!(out.contains("C"), "node C missing:\n{out}");
        assert!(out.contains("retry"), "back-edge label missing:\n{out}");
    }

    #[test]
    fn test_arrowhead_points_down() {
        // Forward edge (South -> North) must render a downward arrow (↓)
        // at the target top, not a rightward arrow (→).
        let fc = Flowchart::new(80, Charset::Unicode)
            .add_node(FlowNode {
                id: "a".into(),
                label: "A".into(),
                shape: NodeShape::Rounded,
                position: None,
            })
            .add_node(FlowNode {
                id: "b".into(),
                label: "B".into(),
                shape: NodeShape::Rounded,
                position: None,
            })
            .connect("a", "b", None);
        let out = fc.build().unwrap();
        assert!(out.contains('↓'), "expected downward arrowhead ↓:\n{out}");
    }

    #[test]
    fn test_long_forward_edge_label_wraps() {
        let long_label = "开始执行驱动注册流程并等待总线回调返回结果";
        let fc = Flowchart::new(80, Charset::Unicode)
            .add_node(FlowNode {
                id: "a".into(),
                label: "Start".into(),
                shape: NodeShape::Rounded,
                position: None,
            })
            .add_node(FlowNode {
                id: "b".into(),
                label: "End".into(),
                shape: NodeShape::Rounded,
                position: None,
            })
            .connect("a", "b", Some(long_label));
        let out = fc.build().unwrap();
        for ch in long_label.chars() {
            assert!(out.contains(ch), "label char '{ch}' missing:\n{out}");
        }
    }

    #[test]
    fn test_diamond_wider_than_canvas_renders() {
        // Regression: a diamond never shrinks to wrap its label, so a long
        // label makes it wider than the canvas. Centering it used to
        // underflow and panic; now it renders with the canvas grown
        // around it.
        let long_label: String = "a".repeat(59);
        let fc = Flowchart::new(40, Charset::Ascii).add_node(FlowNode {
            id: "d".into(),
            label: long_label.clone(),
            shape: NodeShape::Diamond,
            position: None,
        });
        let out = fc.build().unwrap();
        assert_eq!(out.matches('a').count(), 59, "label chars lost:\n{out}");
    }

    #[test]
    fn test_narrow_corridor_label_lines_not_lost() {
        // Regression: when a connection's corridor is narrow, its label
        // wraps to several lines. The stride used to be estimated with a
        // wider wrap width than the draw side used, so the drawn block
        // overflowed its reserved gap and got clamped row-by-row —
        // collapsing lines onto one row where they overwrote each other.
        let label = "abcdefghijklmno";
        let fc = Flowchart::new(60, Charset::Ascii)
            .add_node(FlowNode {
                id: "a".into(),
                label: "A".into(),
                shape: NodeShape::Rounded,
                position: None,
            })
            .add_node(FlowNode {
                id: "b".into(),
                label: "B".into(),
                shape: NodeShape::Rounded,
                position: None,
            })
            .add_node(FlowNode {
                id: "c".into(),
                label: "C".into(),
                shape: NodeShape::Rounded,
                position: None,
            })
            .connect("a", "b", None)
            .connect("a", "c", Some(label));
        let out = fc.build().unwrap();
        for ch in label.chars() {
            assert!(out.contains(ch), "label char '{ch}' missing:\n{out}");
        }
    }

    #[test]
    fn test_riding_fallback_moves_off_the_line() {
        // Ladder fallback (sibling leg immediately adjacent, d=1): the
        // wrap ladder returns "move into the wider free span beside the
        // line" with a new center column. The exe-verified bug dropped
        // that center column and kept the block centered on the leg,
        // covering the sibling leg's cells. The block must land in the
        // free span, clear of both legs.
        let fc = Flowchart::new(80, Charset::Ascii)
            .layout(Layout::Manual)
            .add_node(FlowNode {
                id: "s".into(),
                label: "S".into(),
                shape: NodeShape::Rectangle,
                position: Some((17, 1)),
            })
            .add_node(FlowNode {
                id: "t1".into(),
                label: "T1".into(),
                shape: NodeShape::Rectangle,
                position: Some((17, 12)),
            })
            .add_node(FlowNode {
                id: "t2".into(),
                label: "T2".into(),
                shape: NodeShape::Rectangle,
                position: Some((18, 12)),
            })
            .connect("s", "t1", Some("overlap_test_label"))
            .connect("s", "t2", None);
        let out = fc.build().unwrap();
        assert!(out.contains("overlap_test_label"), "label lost:\n{out}");
        // The block must sit in the left free span (its right edge well
        // clear of the leg column 20 and the sibling leg column 21),
        // not centered on the leg.
        let label_row = out.lines().find(|l| l.contains("overlap_test_label")).expect("label row");
        let label_end = label_row.rfind('l').expect("label end column");
        assert!(
            label_end < 20,
            "fallback block must move off the line, clear of both legs:\n{label_row}"
        );
        insta::assert_snapshot!(out);
    }

    #[test]
    fn test_riding_block_never_covers_target_box() {
        // The riding block is taller than every clear on-line stretch
        // (a blocker box eats the leg's middle): the exe-verified bug
        // let the block overflow the widest stretch into the target
        // box's top border, replacing all six border characters. The
        // block must fall back beside the blocker cluster instead.
        let fc = Flowchart::new(80, Charset::Ascii)
            .layout(Layout::Manual)
            .add_node(FlowNode {
                id: "s".into(),
                label: "S".into(),
                shape: NodeShape::Rectangle,
                position: Some((17, 1)),
            })
            .add_node(FlowNode {
                id: "x".into(),
                label: "X".into(),
                shape: NodeShape::Rectangle,
                position: Some((17, 6)),
            })
            .add_node(FlowNode {
                id: "t2".into(),
                label: "T2".into(),
                shape: NodeShape::Rectangle,
                position: Some((26, 6)),
            })
            .add_node(FlowNode {
                id: "t".into(),
                label: "T".into(),
                shape: NodeShape::Rectangle,
                position: Some((17, 11)),
            })
            .connect("s", "t", Some("alpha beta gamma delta epsilon zeta eta"))
            .connect("s", "t2", None);
        let out = fc.build().unwrap();
        // No label characters lost.
        for word in ["alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta"] {
            assert!(out.contains(word), "label word '{word}' lost:\n{out}");
        }
        // The target box keeps a complete top border row.
        let lines: Vec<&str> = out.lines().collect();
        let t_row = lines.iter().position(|l| l.contains("| T ")).expect("T box row");
        assert!(
            lines[t_row - 1].contains("+----+"),
            "T top border overwritten by the riding block:\n{out}"
        );
        insta::assert_snapshot!(out);
    }

    #[test]
    fn test_same_column_riders_keep_their_own_rows() {
        // Two riding connectors on the same column (D→Z and X→Z) each
        // prefer the stretch nearest the target: the exe-verified bug
        // let the later block overwrite the earlier one entirely (14
        // characters lost, not even a fragment survived). Sequential
        // placement must give each rider rows of its own.
        let fc = Flowchart::new(80, Charset::Ascii)
            .layout(Layout::Manual)
            .add_node(FlowNode {
                id: "d".into(),
                label: "D".into(),
                shape: NodeShape::Rectangle,
                position: Some((17, 1)),
            })
            .add_node(FlowNode {
                id: "x".into(),
                label: "X".into(),
                shape: NodeShape::Rectangle,
                position: Some((17, 7)),
            })
            .add_node(FlowNode {
                id: "y".into(),
                label: "Y".into(),
                shape: NodeShape::Rectangle,
                position: Some((40, 12)),
            })
            .add_node(FlowNode {
                id: "z".into(),
                label: "Z".into(),
                shape: NodeShape::Rectangle,
                position: Some((17, 16)),
            })
            .add_node(FlowNode {
                id: "b".into(),
                label: "B".into(),
                shape: NodeShape::Rectangle,
                position: Some((40, 16)),
            })
            .connect("d", "z", Some("down rider one"))
            .connect("x", "z", Some("down rider two"))
            .connect("d", "b", None)
            .connect("x", "y", None);
        let out = fc.build().unwrap();
        assert!(out.contains("down rider one"), "first rider's label lost:\n{out}");
        assert!(out.contains("down rider two"), "second rider's label lost:\n{out}");
        insta::assert_snapshot!(out);
    }

    /// A wide blocker under two diamonds forces their long "no" edges to a
    /// shared fail box (in another column): the blocker stops the natural
    /// corridor, and the vertical chain stops the below-everything detour.
    fn converging_flowchart() -> Flowchart {
        let wide: String = "w".repeat(40);
        Flowchart::new(100, Charset::Ascii)
            .add_node(FlowNode {
                id: "d1".into(),
                label: "d1?".into(),
                shape: NodeShape::Diamond,
                position: None,
            })
            .add_node(FlowNode {
                id: "d2".into(),
                label: "d2?".into(),
                shape: NodeShape::Diamond,
                position: None,
            })
            .add_node(FlowNode {
                id: "wide".into(),
                label: wide,
                shape: NodeShape::Rectangle,
                position: None,
            })
            .add_node(FlowNode {
                id: "d3".into(),
                label: "d3?".into(),
                shape: NodeShape::Diamond,
                position: None,
            })
            .add_node(FlowNode {
                id: "pass".into(),
                label: "pass".into(),
                shape: NodeShape::Rounded,
                position: None,
            })
            .add_node(FlowNode {
                id: "fail".into(),
                label: "fail".into(),
                shape: NodeShape::Rounded,
                position: None,
            })
            .connect("d1", "d2", None)
            .connect("d2", "wide", None)
            .connect("wide", "d3", None)
            .connect("d3", "pass", Some("yes"))
            .connect("d3", "fail", Some("no"))
            .connect("d1", "fail", Some("no"))
            .connect("d2", "fail", Some("no"))
    }

    #[test]
    fn test_converging_detoured_edges_keep_labels() {
        // The two long "no" edges cannot take any V-H-V corridor row
        // cleanly, so they side-route around the right; the bottom
        // diamond keeps its natural fork corridor. All three "no"
        // labels must render on separate rows — historically the
        // superposed bottom corridors overwrote each other's labels
        // and the edges looked silently dropped.
        let fc = converging_flowchart();
        let out = fc.build().unwrap();
        assert_eq!(out.matches("no").count(), 3, "all three no-edge labels must render:\n{out}");
        assert_eq!(out.matches("yes").count(), 1, "the yes label must render:\n{out}");
        let rows_with_no = out.lines().filter(|l| l.contains("no")).count();
        assert_eq!(rows_with_no, 3, "the three no labels must sit on separate rows:\n{out}");
    }

    #[test]
    fn test_long_forward_edge_side_routes_clean() {
        // A forward edge whose every V-H-V corridor row pierces an
        // obstacle (the wide box blocks the target column, the chain
        // blocks the source column) must route around the right side:
        // exit the source's east side, descend the side rail, enter the
        // target's east edge with `<`. The three-segment fallback used
        // to cut a vertical line straight through the d2/d3 diamonds —
        // visible because diamonds have no interior fill — and drag the
        // branch labels to the bottom corridor, stacking them on d3's
        // own fork like duplicates.
        let fc = converging_flowchart();
        let out = fc.build().unwrap();
        // No diamond is pierced: the spine rows `/|\` and `\|/` are gone.
        assert!(!out.contains("/|\\"), "vertical line pierces a diamond top:\n{out}");
        assert!(!out.contains("\\|/"), "vertical line pierces a diamond bottom:\n{out}");
        // The two long edges exit east at their source's row and keep
        // their labels on the side rail (`...--+no`), not on the bottom.
        let rail_rows = out.lines().filter(|l| l.contains("+no")).count();
        assert_eq!(rail_rows, 2, "two side-rail no labels expected:\n{out}");
        // The side route enters the fail box from the east with `<`.
        assert!(out.contains("fail |<"), "east entry into fail missing:\n{out}");
    }

    #[test]
    fn test_fallback_pierce_hidden_by_diamond_fill() {
        // When even the side route is blocked (BLK sits east of T at T's
        // row, so the rail's entry H would cut through it), the straight
        // vertical fallback still runs the line through the diamond
        // standing in its column. The diamond's interior fill must hide
        // the line — the same treatment rectangles already get.
        let fc = Flowchart::new(60, Charset::Ascii)
            .layout(Layout::Manual)
            .add_node(FlowNode {
                id: "s".into(),
                label: "S".into(),
                shape: NodeShape::Rectangle,
                position: Some((15, 1)),
            })
            .add_node(FlowNode {
                id: "x".into(),
                label: "X?".into(),
                shape: NodeShape::Diamond,
                position: Some((15, 6)),
            })
            .add_node(FlowNode {
                id: "t".into(),
                label: "T".into(),
                shape: NodeShape::Rectangle,
                position: Some((15, 12)),
            })
            .add_node(FlowNode {
                id: "blk".into(),
                label: "BLK".into(),
                shape: NodeShape::Rectangle,
                position: Some((26, 12)),
            })
            .connect("s", "t", Some("edge label"))
            .connect("s", "blk", None);
        let out = fc.build().unwrap();
        assert!(out.contains("X?"), "diamond missing:\n{out}");
        assert!(
            !out.contains("/|\\") && !out.contains("\\|/"),
            "fallback line shows through the diamond interior:\n{out}"
        );
        assert!(out.contains("edge label"), "edge label lost:\n{out}");
    }
}
