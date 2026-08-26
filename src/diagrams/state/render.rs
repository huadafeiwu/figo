//! FSM state diagram renderer.
//!
//! Renders states as rounded pills with centered labels. Accepting states
//! get a double rounded border. Transitions are routed orthogonally with
//! arrowheads and optional labels.

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::canvas::{Canvas, Layer};
use crate::diagrams::state::layout::{LayoutParams, StateLayout, layout_states};
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

        // Expand horizontal spacing so corridors are wide enough for labels.
        expand_corridors_for_labels(&mut layouts, &self.transitions, &params, self.width);

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
        let mut total_w = compute_canvas_width(&layouts, &params).max(self.width);

        // Ensure canvas is wide enough for all transition labels.
        for (idx, t) in self.transitions.iter().enumerate() {
            let Some(text) = t.label.as_ref() else { continue };
            let Some(from) = id_to_layout.get(&t.from) else { continue };
            let Some(to) = id_to_layout.get(&t.to) else { continue };
            let from_cx = from.rect.x + from.rect.w / 2;
            let to_cx = to.rect.x + to.rect.w / 2;
            // Mirror draw_external_transition's base_x logic exactly.
            let row = label_rows.get(&idx).copied().unwrap_or(0);
            let fwd = from.rect.y < to.rect.y;
            let base_x =
                if row > 0 { if fwd { from_cx } else { to_cx } } else { (from_cx + to_cx) / 2 };
            // Use the wrapped max line width (not the full label width) for
            // sizing, since long labels are now wrapped to corridor width.
            let corridor_w =
                if from_cx != to_cx { from_cx.abs_diff(to_cx) + 1 } else { self.width };
            let avail = corridor_w.max(10);
            let (_, _, max_lw) = crate::text::wrap_label(text, avail);
            let lw = max_lw;
            let lx = base_x.saturating_sub(lw / 2);
            total_w = total_w.max(lx + lw);
        }

        let total_h = compute_canvas_height(&layouts, &params, max_label_row);

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
    params: &LayoutParams,
    max_label_row: usize,
) -> usize {
    let bottommost = layouts.iter().map(|l| l.rect.bottom()).max().unwrap_or(0);
    // Account for multi-line labels: each row adds 2 rows, and each
    // label may wrap into multiple lines. Use a generous estimate.
    bottommost + params.gap_y * 2 + max_label_row * 2 + 6
}

// ── Label row computation ─────────────────────────────────────────────

#[derive(Debug)]
struct LabelInfo {
    transition_index: usize,
    x: usize,
    width: usize,
    row: usize,
    gap_key: (usize, usize, usize, usize),
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
        // Group labels by their gap (identified by both y and x of endpoints)
        // so labels in different gaps don't push each other to higher rows.
        let gap_key = if from.rect.y < to.rect.y {
            (from.rect.y, to.rect.y, from_cx, to_cx)
        } else {
            (to.rect.y, from.rect.y, to_cx, from_cx)
        };
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
    let mut gap_groups: HashMap<(usize, usize, usize, usize), Vec<usize>> = HashMap::new();
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

/// Expand horizontal spacing so corridor between from and to states is wide
/// enough to fit the transition label (with 2 cells of `---` on each side).
/// Shifts the appropriate state in the direction that widens the corridor.
/// Shifts are applied incrementally so each transition sees the updated
/// layout from prior shifts (prevents corridor width miscalculation when
/// one shift moves a state that is the `from` of a later transition).
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
        let corridor_w = from_cx.abs_diff(to_cx) + 1;
        // Wrap the label to the corridor width first, then use the wrapped
        // max line width (not the full unwrapped width) to compute how
        // much extra space we need. This ensures labels that fit within
        // the corridor after wrapping don't trigger unnecessary expansion,
        // and labels that still don't fit after wrapping are expanded
        // proportionally to the wrapped width, bounded by canvas_width.
        let (_, _, max_lw) = crate::text::wrap_label(text, corridor_w);
        let needed = max_lw + 4;
        if needed <= corridor_w {
            continue;
        }
        // Cap the expansion so the layout doesn't exceed canvas_width.
        let max_needed = canvas_width.min(max_lw + 4);
        if max_needed <= corridor_w {
            continue;
        }

        let extra = (max_needed - corridor_w) as isize;
        let direction = if to_cx > from_cx { 1isize } else { -1 };
        let shift = extra * direction;

        // Apply this shift immediately to the `to` state and all same-y
        // peers in the shift direction.
        let y = layouts[to_idx].rect.y;
        let x = layouts[to_idx].rect.x;
        for layout in layouts.iter_mut() {
            if layout.rect.y != y {
                continue;
            }
            if shift > 0 && layout.rect.x >= x || shift < 0 && layout.rect.x <= x {
                layout.rect.x = (layout.rect.x as isize + shift).max(0) as usize;
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
) -> HashMap<(usize, usize, usize, usize), usize> {
    let id_to_layout: HashMap<&str, &StateLayout> =
        layouts.iter().map(|l| (l.id.as_str(), l)).collect();

    // Track max (row + num_lines) per gap — the vertical space needed is
    // determined by how many rows of multi-line labels stack up.
    let mut gap_max_extent: HashMap<(usize, usize, usize, usize), usize> = HashMap::new();

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
            (from.rect.y, to.rect.y, from_cx, to_cx)
        } else {
            (to.rect.y, from.rect.y, to_cx, from_cx)
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

        // The vertical extent needed is row * 2 (spacing between rows) +
        // num_lines (the label block itself).
        let extent = row * 2 + num_lines;
        gap_max_extent.entry(gap_key).and_modify(|r| *r = (*r).max(extent)).or_insert(extent);
    }

    // Extra rows needed: the extent minus what's already available (1 row
    // for the corridor itself).
    let mut expansion = HashMap::new();
    for (key, extent) in gap_max_extent {
        if extent > 1 {
            expansion.insert(key, extent * 2);
        }
    }
    expansion
}

/// Expand gaps between states by inserting extra rows. States below a
/// gap are shifted down by the accumulated extra.
fn apply_gap_expansion(
    layouts: &mut [StateLayout],
    gap_extra: &HashMap<(usize, usize, usize, usize), usize>,
    _params: &LayoutParams,
) {
    if gap_extra.is_empty() {
        return;
    }
    // Deduplicate extras by ty (the lower y of each gap). Multiple gaps
    // sharing the same ty should only expand once (take the max extra),
    // not once per state.
    let mut ty_extras: HashMap<usize, usize> = HashMap::new();
    for ((_fy, ty, _fx, _tx), extra) in gap_extra {
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

fn draw_transitions(
    surface: &mut Surface<'_>,
    transitions: &[Transition],
    id_to_layout: &HashMap<String, StateLayoutRef>,
    label_rows: &HashMap<usize, usize>,
    ctx: &PaintContext,
    canvas_width: usize,
) {
    for (idx, t) in transitions.iter().enumerate() {
        let Some(from) = id_to_layout.get(&t.from) else { continue };
        let Some(to) = id_to_layout.get(&t.to) else { continue };

        if t.from == t.to {
            draw_self_loop(surface, from.rect, ctx);
            continue;
        }

        let row = label_rows.get(&idx).copied().unwrap_or(0);
        draw_external_transition(
            surface,
            from.rect,
            to.rect,
            t.label.as_deref(),
            row,
            ctx,
            canvas_width,
        );
    }
}

fn draw_external_transition(
    surface: &mut Surface<'_>,
    from: Rect,
    to: Rect,
    label: Option<&str>,
    row: usize,
    ctx: &PaintContext,
    canvas_width: usize,
) {
    let glyphs = BorderStyle::Single.glyphs(ctx.charset);
    let from_cx = from.x + from.w / 2;
    let to_cx = to.x + to.w / 2;
    let forward = from.y < to.y;

    // Anchor points: exit from the edge of `from` that faces `to`.
    let from_anchor = if forward {
        from.y + from.h // source bottom, one cell below
    } else {
        from.y // source top
    };
    let to_anchor = if forward {
        to.y // target top
    } else {
        to.y + to.h - 1 // target bottom
    };

    // Horizontal corridor in the gap between the two anchor points.
    let route_y = (from_anchor + to_anchor) / 2;

    // Vertical legs from each anchor to the route corridor.
    let (from_start, from_len) = if from_anchor < route_y {
        (from_anchor, route_y - from_anchor + 1)
    } else {
        (route_y, from_anchor - route_y + 1)
    };
    let (to_start, to_len) = if to_anchor < route_y {
        (to_anchor, route_y - to_anchor + 1)
    } else {
        (route_y, to_anchor - route_y + 1)
    };
    surface.put_vertical(from_cx, from_start, from_len, glyphs.vertical, Layer::Connector);
    surface.put_vertical(to_cx, to_start, to_len, glyphs.vertical, Layer::Connector);

    // Horizontal corridor connecting the two vertical legs.
    let (left_x, right_x) = if from_cx < to_cx { (from_cx, to_cx) } else { (to_cx, from_cx) };
    if right_x > left_x {
        surface.put_horizontal(
            left_x,
            route_y,
            right_x - left_x + 1,
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
    surface.put_layered(to_cx, arrow_y, arrow_ch, Layer::ConnectorEnd);

    // Label embedded in the corridor/vertical line (replaces a segment).
    // row 0: label sits on route_y (the corridor/line itself).
    // row > 0: label sits above to avoid x overlap with another label.
    if let Some(text) = label {
        use crate::text::wrap_label;

        // Determine available width for wrapping.
        let avail = if right_x > left_x {
            (right_x - left_x + 1).max(10)
        } else {
            // Same-column: use remaining canvas width to the right.
            canvas_width.saturating_sub(from_cx + 2).max(10)
        };

        let (lines, num_lines, _) = wrap_label(text, avail);

        // row 0: label on corridor (center of from_cx/to_cx).
        // row>0: label on the vertical leg that exists above route_y:
        //   forward → from-leg (from_cx), reverse → to-leg (to_cx).
        let base_x =
            if row > 0 { if forward { from_cx } else { to_cx } } else { (from_cx + to_cx) / 2 };

        // Center the multi-line block on route_y (row 0) or route_y - row*2 (row>0).
        let block_top = if row == 0 {
            route_y.saturating_sub(num_lines / 2)
        } else {
            route_y.saturating_sub(row * 2).saturating_sub(num_lines / 2)
        };

        for (i, line) in lines.iter().enumerate() {
            let lw = UnicodeWidthStr::width(line.as_str());
            let mut label_x = base_x.saturating_sub(lw / 2);
            // Right-edge clamp.
            label_x = label_x.min(canvas_width.saturating_sub(lw));
            // Corridor clamping only when label is on the corridor (row 0).
            if row == 0 && right_x > left_x && lw < right_x - left_x + 1 {
                label_x = label_x.max(left_x).min(right_x + 1 - lw);
            }
            let label_y = block_top + i;
            surface.put_str_layered(label_x, label_y, line, Layer::Label);

            // Restore corridor line on both sides of the label so it stays
            // connected (label only replaces the segment it occupies).
            if label_y == route_y && right_x > left_x {
                let label_end = label_x + lw;
                if label_x > left_x {
                    surface.put_horizontal(
                        left_x,
                        route_y,
                        label_x - left_x,
                        glyphs.horizontal,
                        Layer::Connector,
                    );
                }
                if label_end < right_x {
                    surface.put_horizontal(
                        label_end,
                        route_y,
                        right_x - label_end + 1,
                        glyphs.horizontal,
                        Layer::Connector,
                    );
                }
            }
        }

        // For row>0, restore vertical leg above and below the label so the
        // arrowhead stays connected to the corridor.
        if row > 0 {
            let vcol = if forward { from_cx } else { to_cx };
            let top_y = if forward { from_anchor } else { to_anchor };
            let leg_top = top_y.min(route_y);
            for ly in leg_top..block_top {
                surface.put_layered(vcol, ly, glyphs.vertical, Layer::Connector);
            }
            for ly in (block_top + num_lines)..=route_y {
                surface.put_layered(vcol, ly, glyphs.vertical, Layer::Connector);
            }
        }
    }
}

fn draw_self_loop(surface: &mut Surface<'_>, rect: Rect, ctx: &PaintContext) {
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
}
