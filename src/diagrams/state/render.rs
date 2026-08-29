//! FSM state diagram renderer.
//!
//! Renders states as rounded pills with centered labels. Accepting states
//! get a double rounded border. Transitions are routed orthogonally with
//! arrowheads and optional labels.

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::canvas::{Canvas, Layer};
use crate::diagrams::state::layout::{LayoutParams, StateLayout, layout_states};
use crate::diagrams::state::sugiyama;
use crate::diagrams::state::types::{StateNode, StateType, Transition};
use crate::error::{FigoError, Result};
use crate::render::node::Node;
use crate::render::surface::Surface;
use crate::render::widget::{LayoutContext, MeasureContext, PaintContext, Rect, Widget};
use crate::style::{BorderStyle, Charset, LineStyle};
use unicode_width::UnicodeWidthStr;

/// Builder for FSM state diagrams.
pub struct StateDiagram<'a> {
    width: usize,
    charset: Charset,
    states: Vec<StateNode>,
    initial: Option<&'a str>,
    transitions: Vec<Transition>,
    color: bool,
}

impl<'a> StateDiagram<'a> {
    /// Create a new FSM diagram builder.
    pub fn new(width: usize, charset: Charset) -> Self {
        Self {
            width,
            charset,
            states: Vec::new(),
            initial: None,
            transitions: Vec::new(),
            color: false,
        }
    }

    /// Add a state.
    pub fn add_state(mut self, state: StateNode) -> Self {
        self.states.push(state);
        self
    }

    /// Set the initial state (entry point).
    pub fn initial(mut self, state_id: &'a str) -> Self {
        self.initial = Some(state_id);
        self
    }

    /// Add a directed transition between two states.
    pub fn add_transition(mut self, from: &str, to: &str, label: Option<&str>) -> Self {
        self.transitions.push(Transition {
            from: from.to_string(),
            to: to.to_string(),
            label: label.map(String::from),
        });
        self
    }

    /// Enable or disable color output.
    pub fn color(mut self, enabled: bool) -> Self {
        self.color = enabled;
        self
    }

    /// Render and return as a `String`.
    pub fn build(&self) -> Result<String> {
        if self.states.is_empty() {
            return Err(FigoError::MissingFields("no states specified".into()));
        }

        let params = LayoutParams::default();
        let mut layouts =
            layout_states(&self.states, &self.transitions, self.initial, self.width, &params);

        // Sugiyama crossing reduction + coordinate assignment is now done
        // inside layout_states, so recenter and expand_corridors are no
        // longer needed — column gaps already account for label widths.

        let label_rows = compute_label_rows(&self.transitions, &layouts);

        // Compute per-gap max row and expand gaps accordingly.
        let gap_extra = compute_gap_expansion(&self.transitions, &layouts, &label_rows);
        apply_gap_expansion(&mut layouts, &gap_extra, &params);

        // Shift states down for top margin (room for labels above topmost state).
        let max_label_row = label_rows.values().copied().max().unwrap_or(0);
        if max_label_row > 0 {
            shift_layouts(&mut layouts, max_label_row + 1);
        }

        let id_to_layout = build_id_map(&layouts);

        // --- Single source of truth: compute all transition geometry once ---
        // Re-sort layouts to declaration order so that layouts[i] == states[i].
        // apply_gap_expansion sorts by y, breaking the declaration order
        // established by layout_states.
        let id_to_idx: std::collections::HashMap<&str, usize> =
            self.states.iter().enumerate().map(|(i, s)| (s.id.as_str(), i)).collect();
        layouts.sort_by_key(|l| id_to_idx[l.id.as_str()]);
        let trans_geoms =
            sugiyama::compute_trans_geoms(&layouts, &self.transitions, &id_to_idx, self.width);

        let mut total_w = compute_canvas_width(&layouts, &params).max(self.width);

        // Ensure canvas is wide enough for all transition labels.
        // Uses the pre-computed geometry (single source of truth) instead
        // of independently recalculating corridor_w / embed / avail.
        for (idx, t) in self.transitions.iter().enumerate() {
            let Some(text) = t.label.as_ref() else { continue };
            let geom = &trans_geoms[idx];
            let row = label_rows.get(&idx).copied().unwrap_or(0);
            // Same anchor the draw side uses (see TransGeom): stacked
            // labels (row > 0) travel along a leg, row-0 labels center on
            // the corridor. Sizing with a different anchor under-sizes
            // the canvas and forces the draw-side clamp to move labels.
            let base_x = if row > 0 { geom.stacked_base_x } else { geom.base_x };
            let avail = if geom.avail > 0 {
                geom.avail
            } else {
                total_w.saturating_sub(geom.from_cx + 2).max(2)
            };
            let (_, _, max_lw) = crate::text::wrap_label(text, avail);
            let lw = max_lw;
            let lx = base_x.saturating_sub(lw / 2);
            total_w = total_w.max(lx + lw);
        }

        let total_h = compute_canvas_height(&layouts, &params, max_label_row)
            + self_loop_label_height(&self.transitions, &layouts, total_w);

        let mut canvas = Canvas::new(total_w, total_h);

        {
            let mut surface = Surface::new(&mut canvas);
            let ctx = PaintContext { charset: self.charset, color: self.color };

            draw_initial_arrow(&mut surface, &id_to_layout, self.initial, self.charset);
            draw_states(&mut surface, &layouts, &ctx)?;
            draw_transitions(
                &mut surface,
                &self.transitions,
                &id_to_layout,
                &label_rows,
                &ctx,
                total_w,
                &layouts,
                &trans_geoms,
            );
        }

        canvas.repair_connector_junctions(LineStyle::Simple, self.charset);
        Ok(canvas.render(self.color))
    }

    pub fn render(&self) -> Result<String> {
        self.build()
    }
}

impl fmt::Display for StateDiagram<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.build() {
            Ok(s) => write!(f, "{s}"),
            Err(e) => write!(f, "[figo error: {e}]"),
        }
    }
}

// ── ID mapping ────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct StateLayoutRef {
    rect: Rect,
}

fn build_id_map(layouts: &[StateLayout]) -> HashMap<String, StateLayoutRef> {
    layouts.iter().map(|l| (l.id.clone(), StateLayoutRef { rect: l.rect })).collect()
}

// ── Canvas sizing ─────────────────────────────────────────────────────

fn compute_canvas_width(layouts: &[StateLayout], params: &LayoutParams) -> usize {
    let rightmost = layouts.iter().map(|l| l.rect.right()).max().unwrap_or(0);
    rightmost + params.gap_x
}

fn compute_canvas_height(
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
fn self_loop_label_height(
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
        let (_, n, _) = crate::text::wrap_label(label, avail);
        // Label starts at rect.y and goes down n lines.
        let label_bottom = layout.rect.y + n;
        let extra = label_bottom.saturating_sub(layout.rect.bottom());
        max_extra = max_extra.max(extra);
    }
    max_extra
}

// ── Label row computation ─────────────────────────────────────────────

#[derive(Debug)]
struct LabelInfo {
    transition_index: usize,
    x: usize,
    width: usize,
    row: usize,
    gap_key: (usize, usize),
}

fn compute_label_rows(
    transitions: &[Transition],
    layouts: &[StateLayout],
) -> HashMap<usize, usize> {
    let id_to_layout: HashMap<&str, &StateLayout> =
        layouts.iter().map(|l| (l.id.as_str(), l)).collect();
    let mut labels: Vec<LabelInfo> = Vec::new();

    for (idx, t) in transitions.iter().enumerate() {
        if t.from == t.to || t.label.is_none() {
            continue;
        }
        let Some(from) = id_to_layout.get(t.from.as_str()) else { continue };
        let Some(to) = id_to_layout.get(t.to.as_str()) else { continue };
        let text = t.label.as_ref().unwrap();

        let from_cx = from.rect.x + from.rect.w / 2;
        let to_cx = to.rect.x + to.rect.w / 2;
        let label_x = (from_cx + to_cx) / 2;
        let label_x = label_x.saturating_sub(text.width() / 2);
        // Group labels by their vertical gap (y pair only) so labels
        // in the same route_y row are compared for overlap even if they
        // connect to different sibling states.
        let gap_key = if from.rect.y < to.rect.y {
            (from.rect.y, to.rect.y)
        } else {
            (to.rect.y, from.rect.y)
        };
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

fn shift_layouts(layouts: &mut [StateLayout], dy: usize) {
    for layout in layouts.iter_mut() {
        layout.rect.y += dy;
    }
}

/// Recenter each layer's states horizontally within `canvas_width`.
/// Groups states by their y coordinate, finds the layer's content bounds,
/// and shifts them so the content is centered.
///
/// For single-state layers, center the box on `canvas_width / 2` directly
/// so that all single-column states share the same center column regardless
/// of their individual widths (prevents integer-division misalignment
/// between odd/even-width boxes in vertical transitions).
#[allow(dead_code)]
fn recenter_layouts(layouts: &mut [StateLayout], canvas_width: usize) {
    use std::collections::HashMap;
    let mut y_groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, l) in layouts.iter().enumerate() {
        y_groups.entry(l.rect.y).or_default().push(i);
    }
    let center = canvas_width / 2;
    for indices in y_groups.values() {
        if indices.len() == 1 {
            let i = indices[0];
            let w = layouts[i].rect.w;
            layouts[i].rect.x = center.saturating_sub(w / 2);
        } else {
            let min_x = indices.iter().map(|&i| layouts[i].rect.x).min().unwrap_or(0);
            let max_right = indices.iter().map(|&i| layouts[i].rect.right()).max().unwrap_or(0);
            let content_w = max_right.saturating_sub(min_x);
            let target_start = canvas_width.saturating_sub(content_w) / 2;
            let shift = target_start as isize - min_x as isize;
            if shift != 0 {
                for &i in indices {
                    layouts[i].rect.x = (layouts[i].rect.x as isize + shift).max(0) as usize;
                }
            }
        }
    }
}

/// Expand horizontal spacing so corridor between from and to states is wide
/// enough to fit the transition label (with 2 cells of `---` on each side).
/// Shifts the appropriate state in the direction that widens the corridor.
/// Shifts are applied incrementally so each transition sees the updated
/// layout from prior shifts (prevents corridor width miscalculation when
/// one shift moves a state that is the `from` of a later transition).
#[allow(dead_code)]
fn expand_corridors_for_labels(
    layouts: &mut [StateLayout],
    transitions: &[Transition],
    _params: &LayoutParams,
    canvas_width: usize,
) {
    // Pre-resolve transition endpoints to indices so we don't hold an
    // immutable borrow of `layouts` inside the mutable shift loop.
    let resolved: Vec<(usize, usize, Option<String>)> = {
        let id_to_idx: HashMap<&str, usize> =
            layouts.iter().enumerate().map(|(i, l)| (l.id.as_str(), i)).collect();
        transitions
            .iter()
            .filter_map(|t| {
                if t.from == t.to {
                    return None;
                }
                let from_idx = id_to_idx.get(t.from.as_str()).copied()?;
                let to_idx = id_to_idx.get(t.to.as_str()).copied()?;
                Some((from_idx, to_idx, t.label.clone()))
            })
            .collect()
    };

    for (from_idx, to_idx, label_opt) in resolved {
        let Some(text) = label_opt.as_ref() else { continue };

        // Recompute cx from the current (possibly already-shifted) layout.
        let from_cx = layouts[from_idx].rect.x + layouts[from_idx].rect.w / 2;
        let to_cx = layouts[to_idx].rect.x + layouts[to_idx].rect.w / 2;
        if from_cx == to_cx {
            continue;
        }
        // For cross-layer transitions, the corridor is the horizontal
        // distance between the two center columns (V-H-V path).
        // For same-layer transitions, the corridor is the gap between
        // the two box edges (draw uses edge distance, not center distance).
        // Use the same measurement the draw code uses to avoid mismatch.
        let corridor_w = if layouts[from_idx].rect.y == layouts[to_idx].rect.y {
            // Same layer: gap between box edges
            let (left_rect, right_rect) = if from_cx < to_cx {
                (&layouts[from_idx].rect, &layouts[to_idx].rect)
            } else {
                (&layouts[to_idx].rect, &layouts[from_idx].rect)
            };
            right_rect.x.saturating_sub(left_rect.x + left_rect.w)
        } else {
            // Cross-layer: center distance
            from_cx.abs_diff(to_cx) + 1
        };
        // Use the full unwrapped label width to decide whether the corridor
        // needs expanding. This ensures the corridor is widened enough to
        // fit the label on a single line whenever possible.
        let lw = text.width();
        let needed = lw + 4;
        if needed <= corridor_w {
            continue;
        }
        // Cap the expansion by canvas_width so the layout doesn't exceed
        // the user-specified width. Within that limit, expand enough to
        // fit the full label on one line whenever possible.
        let raw_extra = needed - corridor_w;
        let extra = raw_extra.min(canvas_width);

        // When `to` is to the right of `from`, shift `to` (and same-y peers
        // to its right) rightward to widen the corridor.
        // When `to` is to the LEFT of `from`, shift `from` (and same-y peers
        // to its right) rightward instead — this widens the corridor without
        // pushing any state leftward (which causes visual misalignment).
        if to_cx > from_cx {
            let y = layouts[to_idx].rect.y;
            let x = layouts[to_idx].rect.x;
            for layout in layouts.iter_mut() {
                if layout.rect.y == y && layout.rect.x >= x {
                    layout.rect.x += extra;
                }
            }
        } else {
            let y = layouts[from_idx].rect.y;
            let x = layouts[from_idx].rect.x;
            for layout in layouts.iter_mut() {
                if layout.rect.y == y && layout.rect.x >= x {
                    layout.rect.x += extra;
                }
            }
        }
    }
}

/// Compute how many extra rows each vertical gap needs to fit its labels.
/// Returns a map from gap key to extra rows needed.
fn compute_gap_expansion(
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
            let (_, n, _) = crate::text::wrap_label(text, avail);
            n
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
fn apply_gap_expansion(
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

// ── Drawing helpers ───────────────────────────────────────────────────

fn draw_initial_arrow(
    surface: &mut Surface<'_>,
    id_to_layout: &HashMap<String, StateLayoutRef>,
    initial: Option<&str>,
    charset: Charset,
) {
    let Some(init_id) = initial else { return };
    let Some(layout) = id_to_layout.get(init_id) else { return };

    let init_x = layout.rect.x.saturating_sub(6);
    let init_y = layout.rect.y + 1;
    let dot = if charset == Charset::Ascii { '*' } else { '●' };
    surface.put(init_x, init_y, dot);
    for dx in 1..5 {
        surface.put(init_x + dx, init_y, '─');
    }
    surface.put(init_x + 5, init_y, '>');
}

fn draw_states(
    surface: &mut Surface<'_>,
    layouts: &[StateLayout],
    ctx: &PaintContext,
) -> Result<()> {
    let measure_ctx = MeasureContext { charset: ctx.charset };
    let mut layout_ctx = LayoutContext { charset: ctx.charset, bounds: Rect::default() };

    for layout in layouts {
        draw_state(surface, layout, ctx, &measure_ctx, &mut layout_ctx);
    }
    Ok(())
}

fn draw_state(
    surface: &mut Surface<'_>,
    layout: &StateLayout,
    ctx: &PaintContext,
    measure_ctx: &MeasureContext,
    layout_ctx: &mut LayoutContext,
) {
    match layout.state_type {
        StateType::Simple => draw_simple_state(surface, layout, ctx, measure_ctx, layout_ctx),
        StateType::Accepting => draw_accepting_state(surface, layout, ctx, measure_ctx, layout_ctx),
    }
}

fn draw_simple_state(
    surface: &mut Surface<'_>,
    layout: &StateLayout,
    ctx: &PaintContext,
    measure_ctx: &MeasureContext,
    layout_ctx: &mut LayoutContext,
) {
    let mut node = Node::new(ctx.charset)
        .border(BorderStyle::Rounded)
        .content(vec![layout.label.clone()])
        .align(crate::style::HAlign::Center, crate::style::VAlign::Middle);
    node.measure(measure_ctx);
    node.layout(layout_ctx, layout.rect);
    node.paint(ctx, surface);
}

fn draw_accepting_state(
    surface: &mut Surface<'_>,
    layout: &StateLayout,
    ctx: &PaintContext,
    measure_ctx: &MeasureContext,
    layout_ctx: &mut LayoutContext,
) {
    // Outer rounded border.
    draw_simple_state(surface, layout, ctx, measure_ctx, layout_ctx);

    // Inner rounded border — inset by 1 cell on all sides.
    if layout.rect.w >= 4 && layout.rect.h >= 4 {
        let glyphs = BorderStyle::Rounded.glyphs(ctx.charset);
        let ix = layout.rect.x + 1;
        let iy = layout.rect.y + 1;
        let iw = layout.rect.w - 2;
        let ih = layout.rect.h - 2;
        surface.draw_rect(ix, iy, iw, ih, &glyphs);
        // Clear interior of inner border so label is readable.
        for ry in 1..ih.saturating_sub(1) {
            surface.put_horizontal(ix + 1, iy + ry, iw.saturating_sub(2), ' ', Layer::NodeContent);
        }
        // Re-draw the label at the center.
        let lx = layout.rect.x + (layout.rect.w.saturating_sub(layout.label.width())) / 2;
        let ly = layout.rect.y + layout.rect.h / 2;
        surface.put_str_layered(lx, ly, &layout.label, Layer::NodeContent);
    }
}

// ── Transition drawing ────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn draw_transitions(
    surface: &mut Surface<'_>,
    transitions: &[Transition],
    id_to_layout: &HashMap<String, StateLayoutRef>,
    label_rows: &HashMap<usize, usize>,
    ctx: &PaintContext,
    canvas_width: usize,
    layouts: &[StateLayout],
    trans_geoms: &[sugiyama::TransGeom],
) {
    // Junction columns (corridor endpoints) per vertical gap. A label
    // drawn over a sibling transition's junction cell hides its `+` and
    // corridor start (Label layer > Connector layer, the write is
    // dropped), so label placement steers clear of these columns on the
    // corridor row.
    let mut gap_junctions: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    for (idx, t) in transitions.iter().enumerate() {
        if t.from == t.to {
            continue;
        }
        let (Some(from), Some(to)) = (id_to_layout.get(&t.from), id_to_layout.get(&t.to)) else {
            continue;
        };
        let geom = &trans_geoms[idx];
        if geom.corridor_w == 0 {
            continue; // aligned edge: no corridor, no junctions
        }
        let key = gap_key_of(from.rect, to.rect);
        let entry = gap_junctions.entry(key).or_default();
        entry.push(geom.left_x);
        entry.push(geom.right_x);
    }

    for (idx, t) in transitions.iter().enumerate() {
        let Some(from) = id_to_layout.get(&t.from) else { continue };
        let Some(to) = id_to_layout.get(&t.to) else { continue };

        if t.from == t.to {
            draw_self_loop(surface, from.rect, t.label.as_deref(), ctx, canvas_width);
            continue;
        }

        let mut avoid_cols: Vec<usize> =
            gap_junctions.get(&gap_key_of(from.rect, to.rect)).cloned().unwrap_or_default();
        avoid_cols.sort_unstable();
        avoid_cols.dedup();

        let row = label_rows.get(&idx).copied().unwrap_or(0);
        let geom = &trans_geoms[idx];
        draw_external_transition(
            surface,
            from.rect,
            to.rect,
            t.label.as_deref(),
            row,
            ctx,
            canvas_width,
            layouts,
            geom,
            &avoid_cols,
        );
    }
}

/// Vertical gap key of a transition: the (upper, lower) y pair of its
/// endpoint boxes. Transitions sharing a gap share a corridor row.
fn gap_key_of(from: Rect, to: Rect) -> (usize, usize) {
    if from.y < to.y { (from.y, to.y) } else { (to.y, from.y) }
}

#[allow(clippy::too_many_arguments)]
fn draw_external_transition(
    surface: &mut Surface<'_>,
    from: Rect,
    to: Rect,
    label: Option<&str>,
    row: usize,
    ctx: &PaintContext,
    canvas_width: usize,
    all_layouts: &[StateLayout],
    geom: &sugiyama::TransGeom,
    avoid_junction_cols: &[usize],
) {
    let glyphs = BorderStyle::Single.glyphs(ctx.charset);
    let from_cx = geom.from_cx;
    let to_cx = geom.to_cx;

    // Same-layer transition: draw a direct horizontal arrow between
    // the two boxes — but only if no other box sits between them.
    if geom.same_layer && from_cx != to_cx {
        let left_edge = geom.left_x;
        let right_edge = geom.right_x;
        let ly = from.y + from.h / 2;

        // Check if any other box in the same layer sits between from and to.
        let has_obstacle = all_layouts.iter().any(|layout| {
            let r = &layout.rect;
            r != &from && r != &to && r.y == from.y && r.right() > left_edge && r.x < right_edge
        });

        if !has_obstacle {
            let left_to_right = from_cx < to_cx;
            if right_edge > left_edge {
                surface.put_horizontal(
                    left_edge,
                    ly,
                    right_edge - left_edge,
                    glyphs.horizontal,
                    Layer::Connector,
                );
            }
            let arrow = if left_to_right {
                match ctx.charset {
                    Charset::Ascii => '>',
                    Charset::Unicode => '▶',
                }
            } else {
                match ctx.charset {
                    Charset::Ascii => '<',
                    Charset::Unicode => '◀',
                }
            };
            surface.put_layered(right_edge, ly, arrow, Layer::ConnectorEnd);

            if let Some(text) = label {
                use crate::text::wrap_label;
                let avail = geom.avail;
                let (lines, n, _) = wrap_label(text, avail);
                let mid_x = geom.base_x;
                let block_top = ly.saturating_sub(n / 2);

                for (i, line) in lines.iter().enumerate() {
                    let lw = UnicodeWidthStr::width(line.as_str());
                    let mut label_x = mid_x.saturating_sub(lw / 2);
                    label_x = label_x.max(left_edge).min(right_edge.saturating_sub(lw));
                    let label_y = block_top + i;
                    // Scene D: don't let label cover any box.
                    label_x =
                        avoid_box_x(label_x, lw, label_y, from, to, all_layouts, canvas_width);
                    surface.put_str_layered(label_x, label_y, line, Layer::Label);

                    // Restore `---` on both sides of the label on the corridor row.
                    if geom.corridor_w > 0 {
                        let label_end = label_x + lw;
                        if label_x > left_edge {
                            surface.put_horizontal(
                                left_edge,
                                label_y,
                                label_x - left_edge,
                                glyphs.horizontal,
                                Layer::Connector,
                            );
                        }
                        if label_end <= right_edge {
                            surface.put_horizontal(
                                label_end,
                                label_y,
                                right_edge.saturating_sub(label_end),
                                glyphs.horizontal,
                                Layer::Connector,
                            );
                        }
                    }
                }
            }
            return;
        }
        // If there's an obstacle, fall through to V-H-V path.
    }

    let forward = from.y < to.y;

    let from_anchor = if forward { from.y + from.h } else { from.y };
    let to_anchor = if forward { to.y } else { to.y + to.h - 1 };

    let route_y = (from_anchor + to_anchor) / 2;

    let left_x = geom.left_x;
    let right_x = geom.right_x.min(canvas_width.saturating_sub(1));

    // Repeatedly check if effective_route_y falls inside another state's box.
    let mut effective_route_y = route_y;
    for _ in 0..=all_layouts.len() {
        let mut pushed = false;
        for layout in all_layouts {
            let r = &layout.rect;
            if r == &from || r == &to {
                continue;
            }
            if effective_route_y >= r.y
                && effective_route_y < r.bottom()
                && from_cx.min(to_cx) <= r.right()
                && from_cx.max(to_cx) >= r.x
            {
                if r.y > 0 {
                    effective_route_y = r.y.saturating_sub(1);
                } else {
                    effective_route_y = r.bottom();
                }
                pushed = true;
                break;
            }
        }
        if !pushed {
            break;
        }
    }

    // Scene B+G: Vertical leg avoidance — if from_cx or to_cx falls
    // inside an intermediate box's x range, reroute the leg to the
    // box's nearest edge column so it doesn't visually pass through.
    let from_leg_cx =
        reroute_leg_around_boxes(from_cx, from_anchor, effective_route_y, from, to, all_layouts);
    let to_leg_cx =
        reroute_leg_around_boxes(to_cx, to_anchor, effective_route_y, from, to, all_layouts);

    // Vertical legs from each anchor to the corridor.
    let (from_start, from_len) = if from_anchor < effective_route_y {
        (from_anchor, effective_route_y - from_anchor + 1)
    } else {
        (effective_route_y, from_anchor - effective_route_y + 1)
    };
    let (to_start, to_len) = if to_anchor < effective_route_y {
        (to_anchor, effective_route_y - to_anchor + 1)
    } else {
        (effective_route_y, to_anchor - effective_route_y + 1)
    };
    surface.put_vertical(from_leg_cx, from_start, from_len, glyphs.vertical, Layer::Connector);
    surface.put_vertical(to_leg_cx, to_start, to_len, glyphs.vertical, Layer::Connector);

    // Horizontal corridor connecting the two vertical legs.
    let corridor_left = from_leg_cx.min(to_leg_cx);
    let corridor_right = from_leg_cx.max(to_leg_cx);
    if corridor_right > corridor_left {
        surface.put_horizontal(
            corridor_left,
            effective_route_y,
            corridor_right - corridor_left + 1,
            glyphs.horizontal,
            Layer::Connector,
        );
    }

    // Arrowhead pointing into the target.
    let arrow_ch = match (forward, ctx.charset) {
        (true, Charset::Ascii) => 'v',
        (true, Charset::Unicode) => '▼',
        (false, Charset::Ascii) => '^',
        (false, Charset::Unicode) => '▲',
    };
    let arrow_y = if forward { to.y } else { to.y + to.h - 1 };
    surface.put_layered(to_leg_cx, arrow_y, arrow_ch, Layer::ConnectorEnd);

    // Label: use pre-computed geometry (single source of truth).
    if let Some(text) = label {
        use crate::text::wrap_label;

        let corridor_w = geom.corridor_w;
        let embed_in_corridor = geom.embed;
        let avail = geom.avail;

        let (lines, num_lines, _) = wrap_label(text, avail);

        // Same anchor build() sizes the canvas with (see TransGeom).
        let base_x = if row > 0 { geom.stacked_base_x } else { geom.base_x };

        let block_top = if row == 0 {
            effective_route_y.saturating_sub(num_lines / 2)
        } else {
            effective_route_y.saturating_sub(row * 3).saturating_sub(num_lines / 2)
        };

        // Draw all label lines first.
        let mut label_positions: Vec<(usize, usize, usize)> = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            let lw = UnicodeWidthStr::width(line.as_str());
            let mut label_x = base_x.saturating_sub(lw / 2);
            label_x = label_x.min(canvas_width.saturating_sub(lw));
            // Clamp label to corridor bounds when embedded.
            if embed_in_corridor && lw < corridor_w {
                label_x = label_x.max(left_x).min((right_x + 1).saturating_sub(lw));
            }
            let label_y = block_top + i;
            // Root cause 3 cleanup: a label covering a sibling's junction
            // cell hides its `+` and corridor start (Label > Connector,
            // the write is dropped). Shift the corridor-row line off any
            // same-gap sibling junction column — toward the side that is
            // closer to the current position.
            if label_y == effective_route_y && !avoid_junction_cols.is_empty() {
                for _ in 0..=avoid_junction_cols.len() {
                    let Some(&c) =
                        avoid_junction_cols.iter().find(|&&c| label_x <= c && c < label_x + lw)
                    else {
                        break;
                    };
                    let new_x = if c >= label_x + lw / 2 { c.saturating_sub(lw) } else { c + 1 };
                    if new_x == label_x {
                        break;
                    }
                    label_x = new_x;
                }
                label_x = label_x.min(canvas_width.saturating_sub(lw));
            }
            // Scene D: don't let label cover any box.
            label_x = avoid_box_x(label_x, lw, label_y, from, to, all_layouts, canvas_width);
            surface.put_str_layered(label_x, label_y, line, Layer::Label);
            label_positions.push((label_x, lw, label_y));
        }

        // Restore corridor `---` ONLY on the corridor row + write `+` at junctions.
        if corridor_w > 0
            && effective_route_y >= block_top
            && effective_route_y < block_top + num_lines
        {
            let idx = effective_route_y - block_top;
            if let Some(&(lx, lw, _)) = label_positions.get(idx) {
                let label_end = lx + lw;
                if lx > corridor_left {
                    surface.put_horizontal(
                        corridor_left,
                        effective_route_y,
                        lx - corridor_left,
                        glyphs.horizontal,
                        Layer::Connector,
                    );
                }
                if label_end <= corridor_right {
                    surface.put_horizontal(
                        label_end,
                        effective_route_y,
                        corridor_right - label_end + 1,
                        glyphs.horizontal,
                        Layer::Connector,
                    );
                }
                let junction_ch = if ctx.charset == Charset::Ascii { '+' } else { '┼' };
                if lx > corridor_left {
                    surface.put_layered(
                        corridor_left,
                        effective_route_y,
                        junction_ch,
                        Layer::Connector,
                    );
                }
                if label_end <= corridor_right {
                    surface.put_layered(
                        corridor_right,
                        effective_route_y,
                        junction_ch,
                        Layer::Connector,
                    );
                }
            }
        }

        // Restore vertical legs `|` around the label block.
        if row > 0 {
            let vcol = if forward { from_leg_cx } else { to_leg_cx };
            let top_y = if forward { from_anchor } else { to_anchor };
            let leg_top = top_y.min(effective_route_y);
            for ly in leg_top..block_top {
                surface.put_layered(vcol, ly, glyphs.vertical, Layer::Connector);
            }
            for ly in (block_top + num_lines)..=effective_route_y {
                surface.put_layered(vcol, ly, glyphs.vertical, Layer::Connector);
            }
        } else if corridor_w > 0 {
            let vcol = if forward { from_leg_cx } else { to_leg_cx };
            for ly in (block_top + num_lines)..=effective_route_y {
                surface.put_layered(vcol, ly, glyphs.vertical, Layer::Connector);
            }
        }
    }
}

/// Scene B+G: If a vertical leg at column `cx` would pass through an
/// intermediate box (not from/to), reroute it to the box's nearest
/// left or right edge so the leg goes around the box instead of
/// visually passing through it.
fn reroute_leg_around_boxes(
    cx: usize,
    anchor_y: usize,
    route_y: usize,
    from: Rect,
    to: Rect,
    all_layouts: &[StateLayout],
) -> usize {
    let lo = anchor_y.min(route_y);
    let hi = anchor_y.max(route_y);
    for layout in all_layouts {
        let r = &layout.rect;
        if r == &from || r == &to {
            continue;
        }
        // Only reroute if the box is strictly between the leg's endpoints
        // (not at the same layer as from or to).
        if r.y <= from.y || r.y >= to.y {
            continue;
        }
        // Skip if cx is the center of this box — the leg passing through
        // to reach a box below at the same column is correct UML behavior.
        let box_cx = r.x + r.w / 2;
        if cx == box_cx {
            continue;
        }
        // Does the leg's y range [lo, hi] overlap the box's y range?
        if lo < r.bottom() && hi >= r.y && cx >= r.x && cx < r.right() {
            // Leg passes through this box. Reroute to nearest edge.
            let dist_left = cx.saturating_sub(r.x);
            let dist_right = r.right().saturating_sub(cx + 1);
            return if dist_left <= dist_right { r.x.saturating_sub(1) } else { r.right() };
        }
    }
    cx
}

/// Scene D: Adjust label_x so the label [label_x, label_x+lw) does not
/// overlap any box at row label_y. If it does, shift it left or right
/// to the nearest non-overlapping position. A shift that would push the
/// label past the right canvas edge is never taken — out-of-bounds
/// writes are silently dropped by the canvas, which would lose label
/// characters.
fn avoid_box_x(
    mut label_x: usize,
    lw: usize,
    label_y: usize,
    from: Rect,
    to: Rect,
    all_layouts: &[StateLayout],
    canvas_width: usize,
) -> usize {
    for layout in all_layouts {
        let r = &layout.rect;
        if r == &from || r == &to {
            continue;
        }
        // Does label overlap this box at this y? label_end is recomputed
        // each iteration — label_x may already have moved for an earlier
        // box in this same loop.
        let label_end = label_x + lw;
        if label_y >= r.y && label_y < r.bottom() && label_end > r.x && label_x < r.right() {
            // Shifting right past the box is only valid when the label
            // still fits inside the canvas. Shifting left always fits:
            // the label ends at r.x, which is inside the canvas.
            let shift_right_ok = r.right() + lw <= canvas_width;
            let shift_right = r.right();
            // Try shifting left before the box.
            let shift_left = r.x.saturating_sub(lw);
            // Pick the valid shift closer to original position.
            if shift_right_ok
                && shift_right.saturating_sub(label_x) <= label_x.saturating_sub(shift_left)
            {
                label_x = shift_right;
            } else {
                label_x = shift_left;
            }
        }
    }
    label_x
}

fn draw_self_loop(
    surface: &mut Surface<'_>,
    rect: Rect,
    label: Option<&str>,
    ctx: &PaintContext,
    canvas_width: usize,
) {
    let glyphs = BorderStyle::Single.glyphs(ctx.charset);
    let loop_x = rect.x + rect.w + 1;
    let top = rect.y;
    let mid = rect.y + rect.h / 2;
    let bot = rect.y + rect.h - 1;

    surface.put_vertical(loop_x, top, rect.h, glyphs.vertical, Layer::Connector);
    surface.put_horizontal(
        rect.x + rect.w,
        bot,
        loop_x - (rect.x + rect.w) + 1,
        glyphs.horizontal,
        Layer::Connector,
    );

    let top_right = if ctx.charset == Charset::Ascii { '+' } else { '┐' };
    let bottom_right = if ctx.charset == Charset::Ascii { '+' } else { '┘' };
    surface.put_layered(loop_x, top, top_right, Layer::Connector);
    surface.put_layered(loop_x, bot, bottom_right, Layer::Connector);

    surface.put_layered(rect.x + rect.w, mid, '<', Layer::ConnectorEnd);

    // Draw the label to the right of the loop, wrapped to the remaining
    // canvas width.
    if let Some(text) = label {
        let avail = canvas_width.saturating_sub(loop_x + 2).max(10);
        let (lines, _, _) = crate::text::wrap_label(text, avail);
        for (i, line) in lines.iter().enumerate() {
            let lw = UnicodeWidthStr::width(line.as_str());
            let lx = (loop_x + 2).min(canvas_width.saturating_sub(lw));
            surface.put_str_layered(lx, top + i, line, Layer::Label);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_layout(id: &str, x: usize, y: usize, w: usize, h: usize) -> StateLayout {
        StateLayout {
            id: id.into(),
            label: id.into(),
            state_type: StateType::Simple,
            rect: Rect::new(x, y, w, h),
        }
    }

    #[test]
    fn avoid_box_x_right_shift_never_overflows_canvas() {
        // Box near the right edge: shifting right would push a 20-wide
        // label past the canvas (90 + 20 > 100) — must shift left instead.
        // The old code returned 90, silently dropping the last 10 chars.
        let from = Rect::new(0, 0, 10, 3);
        let to = Rect::new(0, 10, 10, 3);
        let layouts = vec![
            state_layout("from", 0, 0, 10, 3),
            state_layout("to", 0, 10, 10, 3),
            state_layout("obs", 60, 4, 30, 3),
        ];
        let x = avoid_box_x(62, 20, 5, from, to, &layouts, 100);
        assert!(x + 20 <= 100, "label overflows canvas: x={x}, lw=20, canvas=100");
        assert_eq!(x, 40, "should shift left before the box at x=60");
    }

    #[test]
    fn avoid_box_x_right_shift_when_it_fits() {
        // Shifting right past the box is closer and fits: take it.
        let from = Rect::new(0, 0, 10, 3);
        let to = Rect::new(0, 10, 10, 3);
        let layouts = vec![
            state_layout("from", 0, 0, 10, 3),
            state_layout("to", 0, 10, 10, 3),
            state_layout("obs", 20, 4, 8, 3),
        ];
        let x = avoid_box_x(22, 8, 5, from, to, &layouts, 100);
        assert_eq!(x, 28, "closer valid shift is right past the box (right edge 28)");
    }

    #[test]
    fn avoid_box_x_prefers_left_when_right_does_not_fit() {
        // Right edge of the box + lw exceeds the canvas even though the
        // right shift is closer — the left shift must win.
        let from = Rect::new(0, 0, 10, 3);
        let to = Rect::new(0, 10, 10, 3);
        let layouts = vec![
            state_layout("from", 0, 0, 10, 3),
            state_layout("to", 0, 10, 10, 3),
            state_layout("obs", 70, 4, 8, 3),
        ];
        let x = avoid_box_x(71, 8, 5, from, to, &layouts, 80);
        assert!(x + 8 <= 80, "label overflows canvas: x={x}");
        assert_eq!(x, 62, "right shift (78+8>80) invalid, left shift to 70-8=62");
    }

    #[test]
    fn avoid_box_x_no_overlap_untouched() {
        // Label doesn't overlap any box: position unchanged.
        let from = Rect::new(0, 0, 10, 3);
        let to = Rect::new(0, 10, 10, 3);
        let layouts = vec![
            state_layout("from", 0, 0, 10, 3),
            state_layout("to", 0, 10, 10, 3),
            state_layout("obs", 50, 4, 8, 3),
        ];
        let x = avoid_box_x(20, 8, 5, from, to, &layouts, 100);
        assert_eq!(x, 20);
    }
}
