//! Flowchart diagrams with nodes and orthogonally-routed connections.
//!
//! Nodes support three shapes: `Rectangle`, `Rounded`, and `Diamond`
//! (decision). The layout engine stacks nodes vertically and routes
//! connectors orthogonally. Forward edges flow top-to-bottom; back-edges
//! (target above source) route via a side corridor to the right of every
//! node so they do not punch through intermediates.

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::canvas::{Canvas, Layer};
use crate::error::{FigoError, Result};
use crate::layout::connector::Connector;
use crate::layout::geom::{Anchor, Rect};
use crate::layout::{
    RidingLabel, beside_line_label_avail, corridor_label_avail, corridor_label_block_top,
    h_corridor_len, riding_placement_cols,
};
use crate::style::{BorderStyle, Charset, LineStyle};
use crate::text::wrap_label;
use unicode_width::UnicodeWidthStr;

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
        // row. The wrap width is the same sibling-aware ladder the draw
        // pass uses; one `|` rail rides on each side of the block.
        let mut riding_stride_demand: HashMap<usize, usize> = HashMap::new();
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
            let spill = sibs.iter().map(|&(_, n)| n).max().unwrap_or(0).saturating_sub(2);
            let far_cols: Vec<usize> = sibs.iter().map(|&(c, _)| c).collect();
            let avail = beside_line_label_avail(from_cx, self.width);
            let (wrap_w, _) = riding_placement_cols(&far_cols, from_cx, self.width, avail);
            let n_ride = wrap_label(label, wrap_w).line_count;
            let mut demand = n_ride + 4;
            if to_layer == from_layer + 1 {
                demand += spill; // the fork and the stretch share one gap
            }
            riding_stride_demand
                .entry(to_layer - 1)
                .and_modify(|v| *v = (*v).max(demand))
                .or_insert(demand);
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
        // Reserve room on the right for back-edge side corridors and labels.
        let side_room = self.side_room_for_back_edges(positions);
        let total_w = max_w + side_room;

        let max_h = positions.iter().map(|p| p.rect.bottom()).max().unwrap_or(10)
            + 2
            + AUTO_STRIDE_ROWS
            + 4;
        let mut canvas = Canvas::new(total_w, max_h);

        // Phase 1 — node borders and labels.
        for pos in positions {
            self.draw_node(&mut canvas, pos);
        }

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
        for conn in &self.connections {
            let (Some(&from), Some(&to)) =
                (pos_map.get(conn.from.as_str()), pos_map.get(conn.to.as_str()))
            else {
                continue;
            };
            if from.rect.y >= to.rect.y {
                continue; // back-edge: side route, never meets the fork
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

        // Phase 2 — connectors.
        for conn in &self.connections {
            let (Some(&from), Some(&to)) =
                (pos_map.get(conn.from.as_str()), pos_map.get(conn.to.as_str()))
            else {
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
            .with_avoids(
                positions
                    .iter()
                    .filter(|p| p.node.id != from.node.id && p.node.id != to.node.id)
                    .map(|p| p.rect),
            )
            .with_user_width(self.width);
            if let Some(label) = &conn.label {
                connector.label = Some(label.clone());
            }
            // Fork riding for labeled vertical forward connectors with
            // corridor siblings: measure the sibling-aware wrap width,
            // then pick the leg stretch clear of sibling label blocks
            // and intermediate boxes — preferring the stretch nearest
            // the target box — and center the block within it.
            if !is_back && conn.label.is_some() {
                let from_cx = from.rect.x + from.rect.w / 2;
                let to_cx = to.rect.x + to.rect.w / 2;
                if from_cx == to_cx {
                    if let Some(sibs) = fork_sibs.get(conn.from.as_str()) {
                        let label = conn.label.as_deref().unwrap();
                        let (sy, ty) = (from.rect.bottom(), to.rect.y);
                        let below_row = sibs.iter().map(|&(_, bb)| bb + 1).max().unwrap_or(sy + 1);
                        let far_cols: Vec<usize> = sibs.iter().map(|&(c, _)| c).collect();
                        let avail = beside_line_label_avail(from_cx, self.width);
                        let (wrap_w, _) =
                            riding_placement_cols(&far_cols, from_cx, canvas.width(), avail);
                        let n = wrap_label(label, wrap_w).line_count;
                        // The block's widest line spans this column range
                        // when centered on the leg.
                        let lx = from_cx.saturating_sub(wrap_w / 2);
                        let span = (lx, lx + wrap_w);
                        // Intermediate boxes that overlap the span cut the
                        // candidate rows [below_row, ty) into stretches.
                        let mut blockers: Vec<(usize, usize)> = positions
                            .iter()
                            .map(|p| p.rect)
                            .filter(|r| *r != from.rect && *r != to.rect)
                            .filter(|r| {
                                r.y < ty
                                    && r.bottom() > below_row
                                    && r.right() > span.0
                                    && r.x < span.1
                            })
                            .map(|r| (r.y.max(below_row), r.bottom().min(ty)))
                            .collect();
                        blockers.sort_by_key(|&(lo, _)| lo);
                        let mut stretches: Vec<(usize, usize)> = Vec::new();
                        let mut start = below_row;
                        for (b_lo, b_hi) in blockers {
                            if b_lo > start {
                                stretches.push((start, b_lo));
                            }
                            start = start.max(b_hi);
                        }
                        if ty > start {
                            stretches.push((start, ty));
                        }
                        // Prefer the stretch nearest the target with room
                        // for the block plus its `|` rails, then without
                        // rails, then the widest anywhere.
                        let chosen = stretches
                            .iter()
                            .rev()
                            .find(|&&(lo, hi)| hi - lo >= n + 2)
                            .or_else(|| stretches.iter().rev().find(|&&(lo, hi)| hi - lo >= n))
                            .or_else(|| stretches.iter().max_by_key(|&&(lo, hi)| hi - lo));
                        let block_top = match chosen {
                            Some(&(lo, hi)) => lo + (hi - lo).saturating_sub(n) / 2,
                            None => below_row, // degenerate: no clear stretch
                        };
                        connector = connector.with_riding_label(RidingLabel { wrap_w, block_top });
                    }
                }
            }
            if is_back {
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

    /// Compute the extra columns needed on the right for back-edge side
    /// corridors. Reserves enough room for the corridor plus the longest
    /// back-edge label placed to the right of the corridor.
    fn side_room_for_back_edges(&self, positions: &[PositionedNode]) -> usize {
        let id_y: HashMap<&str, usize> =
            positions.iter().map(|p| (p.node.id.as_str(), p.rect.y)).collect();
        let max_label_len = self
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
        (3 + max_label_len).max(6)
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
}
