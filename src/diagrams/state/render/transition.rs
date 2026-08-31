//! Transition drawing orchestration: iterates transitions, precomputes
//! same-gap junction columns, and routes each transition. Same-layer
//! pairs draw a direct horizontal arrow; everything else routes V-H-V
//! (vertical leg, horizontal corridor, vertical leg) with obstacle
//! avoidance, then delegates label placement to `label`.

use std::collections::HashMap;

use crate::canvas::Layer;
use crate::diagrams::state::layout::StateLayout;
use crate::diagrams::state::render::StateLayoutRef;
use crate::diagrams::state::sugiyama::TransGeom;
use crate::diagrams::state::types::Transition;
use crate::render::surface::Surface;
use crate::render::widget::{PaintContext, Rect};
use crate::style::{BorderStyle, Charset};
use crate::text::wrap_label;

use super::avoidance::reroute_leg_around_boxes;
use super::gap_expansion::{GapLabelBudget, gap_label_heights};
use super::label::{RouteGeometry, draw_transition_label};
use super::label_rows::gap_key_of;
use super::riding_plan::{RiderPlan, plan_gap_riders};
use super::same_layer::draw_same_layer_transition;
use super::self_loop::draw_self_loop;

/// Everything the transition-drawing stage needs, bundled to keep the
/// draw entry points within the clippy argument limit.
pub(super) struct DrawStage<'a> {
    pub surface: &'a mut Surface<'a>,
    pub ctx: &'a PaintContext,
    pub canvas_width: usize,
    pub transitions: &'a [Transition],
    pub layouts: &'a [StateLayout],
    pub trans_geoms: &'a [TransGeom],
    pub label_rows: &'a HashMap<usize, usize>,
    pub id_to_layout: &'a HashMap<String, StateLayoutRef>,
}

/// Per-transition immutable drawing context shared by the routing,
/// same-layer, and label placement stages.
pub(super) struct TransitionCtx<'a> {
    pub ctx: &'a PaintContext,
    pub from: Rect,
    pub to: Rect,
    pub canvas_width: usize,
    pub all_layouts: &'a [StateLayout],
    pub geom: &'a TransGeom,
    pub avoid_junction_cols: &'a [usize],
    /// Distance from the corridor row up to the label block's top row
    /// (see `stack_offset_for`).
    pub stack_offset: usize,
    /// Wrapped line count of the gap's tallest row-0 label block: such
    /// blocks straddle the corridor row, and the rows they overhang
    /// above/below it are what a riding label's exclusive leg segment
    /// must clear (see `own_leg_placement`).
    pub gap_row0_lines: usize,
    /// Riding plan for this transition's label, when it rides its leg
    /// (aligned edge with corridor siblings in its gap); `None` for
    /// default placement. The plan is the single source shared with the
    /// gap expansion — same wrap width, center column, and cluster
    /// stacking on both sides.
    pub riding: Option<&'a RiderPlan>,
}

/// Draw all transitions (self-loops, same-layer arrows, V-H-V routes).
pub(super) fn draw_transitions(stage: &mut DrawStage<'_>) {
    // Junction columns (corridor endpoints) per vertical gap. A label
    // drawn over a sibling transition's junction cell hides its `+` and
    // corridor start (Label layer > Connector layer, the write is
    // dropped), so label placement steers clear of these columns on the
    // corridor row.
    let mut gap_junctions: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    for (idx, t) in stage.transitions.iter().enumerate() {
        if t.from == t.to {
            continue;
        }
        let (Some(from), Some(to)) =
            (stage.id_to_layout.get(&t.from), stage.id_to_layout.get(&t.to))
        else {
            continue;
        };
        let geom = &stage.trans_geoms[idx];
        if geom.corridor_w == 0 {
            continue; // aligned edge: no corridor, no junctions
        }
        let key = gap_key_of(from.rect, to.rect);
        let entry = gap_junctions.entry(key).or_default();
        entry.push(geom.left_x);
        entry.push(geom.right_x);
    }

    // Height-aware label stacking: labels sharing a gap whose x-ranges
    // overlap occupy successive rows, and each row's block sits directly
    // above the block below (one blank row between). Derived from the
    // wrapped line counts — no fixed row stride — so a short block can
    // never land inside a tall sibling block and overwrite its label.
    let gap_heights = gap_label_heights(
        stage.transitions,
        stage.layouts,
        stage.label_rows,
        stage.trans_geoms,
        stage.canvas_width,
    );

    // Riding plans (single source with the gap expansion above): which
    // labels ride their leg, at what wrap width and center column, and
    // how overlapping riders stack. Built from the same pre-reroute
    // geometry the budget read.
    let plans =
        plan_gap_riders(stage.transitions, stage.layouts, stage.trans_geoms, stage.canvas_width);
    let mut rider_by_idx: HashMap<usize, &RiderPlan> = HashMap::new();
    for group in plans.values() {
        for r in &group.riders {
            rider_by_idx.insert(r.idx, r);
        }
    }

    for (idx, t) in stage.transitions.iter().enumerate() {
        let Some(from) = stage.id_to_layout.get(&t.from) else { continue };
        let Some(to) = stage.id_to_layout.get(&t.to) else { continue };

        if t.from == t.to {
            draw_self_loop(
                stage.surface,
                from.rect,
                t.label.as_deref(),
                stage.ctx,
                stage.canvas_width,
            );
            continue;
        }

        let mut avoid_cols: Vec<usize> =
            gap_junctions.get(&gap_key_of(from.rect, to.rect)).cloned().unwrap_or_default();
        avoid_cols.sort_unstable();
        avoid_cols.dedup();

        let row = stage.label_rows.get(&idx).copied().unwrap_or(0);
        let gap_key = gap_key_of(from.rect, to.rect);
        let gap_row0_lines =
            gap_heights.get(&gap_key).and_then(|b| b.above.first().copied()).unwrap_or(0);
        // The block's own height, wrapped at the same avail the label
        // pass uses (single geometry source).
        let own_lines = t
            .label
            .as_ref()
            .map(|l| wrap_label(l, stage.trans_geoms[idx].avail).line_count)
            .unwrap_or(1);
        let stack_offset = stack_offset_for(gap_heights.get(&gap_key), row, own_lines);
        let tcx = TransitionCtx {
            ctx: stage.ctx,
            from: from.rect,
            to: to.rect,
            canvas_width: stage.canvas_width,
            all_layouts: stage.layouts,
            geom: &stage.trans_geoms[idx],
            avoid_junction_cols: &avoid_cols,
            stack_offset,
            gap_row0_lines,
            riding: rider_by_idx.get(&idx).copied(),
        };
        draw_external_transition(stage.surface, &tcx, t.label.as_deref(), row);
    }
}

/// Distance from the corridor row up to a label block's top row. Row 0
/// centers its own block on the corridor row; row r sits above the
/// block below it, one blank row between blocks:
/// `offset(r) = h(0)/2 + Σ_{j=1..r} h(j)`. Only above-corridor rows
/// count — a riding label (aligned edge with corridor siblings) lives
/// below the corridor row and never contributes here.
fn stack_offset_for(budget: Option<&GapLabelBudget>, row: usize, own_lines: usize) -> usize {
    if row == 0 {
        return own_lines / 2;
    }
    let Some(hs) = budget.map(|b| &b.above) else { return own_lines / 2 + row };
    let mut off = hs.first().copied().unwrap_or(0) / 2;
    for j in 1..=row {
        off += hs.get(j).copied().unwrap_or(0);
    }
    off
}

/// Draw a single (non-self-loop) transition: same-layer horizontal arrow
/// when possible, otherwise the V-H-V route with avoidance.
fn draw_external_transition(
    surface: &mut Surface<'_>,
    tcx: &TransitionCtx<'_>,
    label: Option<&str>,
    row: usize,
) {
    let glyphs = BorderStyle::Single.glyphs(tcx.ctx.charset);
    let from_cx = tcx.geom.from_cx;
    let to_cx = tcx.geom.to_cx;

    // Same-layer transition: draw a direct horizontal arrow between
    // the two boxes — but only if no other box sits between them.
    if tcx.geom.same_layer && from_cx != to_cx && draw_same_layer_transition(surface, tcx, label) {
        return;
        // If there's an obstacle, draw_same_layer_transition returns
        // false and we fall through to the V-H-V path.
    }

    let from = tcx.from;
    let to = tcx.to;
    let forward = from.y < to.y;

    let from_anchor = if forward { from.y + from.h } else { from.y };
    let to_anchor = if forward { to.y } else { to.y + to.h - 1 };

    let route_y = (from_anchor + to_anchor) / 2;

    let left_x = tcx.geom.left_x;
    let right_x = tcx.geom.right_x.min(tcx.canvas_width.saturating_sub(1));

    // Repeatedly check if effective_route_y falls inside another state's box.
    let mut effective_route_y = route_y;
    for _ in 0..=tcx.all_layouts.len() {
        let mut pushed = false;
        for layout in tcx.all_layouts {
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
    let from_leg_cx = reroute_leg_around_boxes(
        from_cx,
        from_anchor,
        effective_route_y,
        from,
        to,
        tcx.all_layouts,
    );
    let to_leg_cx =
        reroute_leg_around_boxes(to_cx, to_anchor, effective_route_y, from, to, tcx.all_layouts);

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
    let arrow_ch = match (forward, tcx.ctx.charset) {
        (true, Charset::Ascii) => 'v',
        (true, Charset::Unicode) => '▼',
        (false, Charset::Ascii) => '^',
        (false, Charset::Unicode) => '▲',
    };
    let arrow_y = if forward { to.y } else { to.y + to.h - 1 };
    surface.put_layered(to_leg_cx, arrow_y, arrow_ch, Layer::ConnectorEnd);

    // Label: use pre-computed geometry (single source of truth).
    if let Some(text) = label {
        let route = RouteGeometry {
            forward,
            from_anchor,
            to_anchor,
            effective_route_y,
            left_x,
            right_x,
            from_leg_cx,
            to_leg_cx,
            corridor_left,
            corridor_right,
        };
        draw_transition_label(surface, tcx, text, row, &route);
    }
}
