//! Label placement for V-H-V transitions.
//!
//! The label block anchors on the column from `TransGeom`
//! (`stacked_base_x` for stacked rows, `base_x` otherwise), clamps into
//! the corridor when embedded, shifts off sibling junction columns and
//! box overlaps, then restores the corridor `---` and the vertical leg
//! `|` around the block.
//!
//! Aligned edges that share their gap with corridor siblings take a
//! different anchor: their default mid-route position lands exactly on
//! the sibling corridor row (the fork), so the block moves to the
//! exclusive leg segment between the fork and the target box, centered
//! within it — see `own_leg_placement`.

use unicode_width::UnicodeWidthStr;

use crate::canvas::Layer;
use crate::diagrams::state::render::transition::TransitionCtx;
use crate::layout::riding_placement_cols;
use crate::render::surface::Surface;
use crate::style::{BorderStyle, Charset};
use crate::text::wrap_label;

use super::avoidance::{avoid_box_x, shift_off_junctions};

/// Geometry of the drawn V-H-V route, consumed by label placement.
pub(super) struct RouteGeometry {
    pub forward: bool,
    pub from_anchor: usize,
    pub to_anchor: usize,
    pub effective_route_y: usize,
    pub left_x: usize,
    pub right_x: usize,
    pub from_leg_cx: usize,
    pub to_leg_cx: usize,
    pub corridor_left: usize,
    pub corridor_right: usize,
}

/// Where and how a label block is laid out: the wrapped lines, the
/// column the block centers on, and the top row of the block.
struct LabelBlock {
    lines: Vec<String>,
    center_x: usize,
    block_top: usize,
}

/// Draw a transition's label block and restore the corridor `---` plus
/// the vertical leg `|` around it.
pub(super) fn draw_transition_label(
    surface: &mut Surface<'_>,
    tcx: &TransitionCtx<'_>,
    text: &str,
    row: usize,
    route: &RouteGeometry,
) {
    let glyphs = BorderStyle::Single.glyphs(tcx.ctx.charset);
    let geom = tcx.geom;
    let corridor_w = geom.corridor_w;
    let embed_in_corridor = geom.embed;
    let avail = geom.avail;

    // An aligned edge sharing its gap with corridor siblings rides the
    // exclusive leg segment below/above the fork instead of the default
    // mid-route position (which coincides with the sibling corridor row
    // and makes branch attribution ambiguous).
    let rides_own_leg = corridor_w == 0 && !tcx.avoid_junction_cols.is_empty();

    let block = if rides_own_leg {
        own_leg_placement(tcx, text, route)
    } else {
        default_placement(tcx, text, row, route, avail)
    };
    let lines = &block.lines;
    let num_lines = lines.len();
    let base_x = block.center_x;
    let block_top = block.block_top;

    // Draw all label lines first.
    let mut label_positions: Vec<(usize, usize, usize)> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let lw = UnicodeWidthStr::width(line.as_str());
        let mut label_x = base_x.saturating_sub(lw / 2);
        label_x = label_x.min(tcx.canvas_width.saturating_sub(lw));
        // Clamp label to corridor bounds when embedded.
        if embed_in_corridor && lw < corridor_w {
            label_x = label_x.max(route.left_x).min((route.right_x + 1).saturating_sub(lw));
        }
        let label_y = block_top + i;
        // Root cause 3 cleanup: shift the corridor-row line off any
        // sibling junction column so the sibling's `+` still renders.
        if label_y == route.effective_route_y && !tcx.avoid_junction_cols.is_empty() {
            label_x = shift_off_junctions(label_x, lw, tcx.avoid_junction_cols);
            label_x = label_x.min(tcx.canvas_width.saturating_sub(lw));
        }
        // Scene D: don't let label cover any box.
        label_x =
            avoid_box_x(label_x, lw, label_y, tcx.from, tcx.to, tcx.all_layouts, tcx.canvas_width);
        surface.put_str_layered(label_x, label_y, line, Layer::Label);
        label_positions.push((label_x, lw, label_y));
    }

    // Restore corridor `---` ONLY on the corridor row + write `+` at junctions.
    if corridor_w > 0
        && route.effective_route_y >= block_top
        && route.effective_route_y < block_top + num_lines
    {
        let idx = route.effective_route_y - block_top;
        if let Some(&(lx, lw, _)) = label_positions.get(idx) {
            let label_end = lx + lw;
            if lx > route.corridor_left {
                surface.put_horizontal(
                    route.corridor_left,
                    route.effective_route_y,
                    lx - route.corridor_left,
                    glyphs.horizontal,
                    Layer::Connector,
                );
            }
            if label_end <= route.corridor_right {
                surface.put_horizontal(
                    label_end,
                    route.effective_route_y,
                    route.corridor_right - label_end + 1,
                    glyphs.horizontal,
                    Layer::Connector,
                );
            }
            let junction_ch = if tcx.ctx.charset == Charset::Ascii { '+' } else { '┼' };
            if lx > route.corridor_left {
                surface.put_layered(
                    route.corridor_left,
                    route.effective_route_y,
                    junction_ch,
                    Layer::Connector,
                );
            }
            if label_end <= route.corridor_right {
                surface.put_layered(
                    route.corridor_right,
                    route.effective_route_y,
                    junction_ch,
                    Layer::Connector,
                );
            }
        }
    }

    // Restore vertical legs `|` around the label block.
    if row > 0 {
        let vcol = if route.forward { route.from_leg_cx } else { route.to_leg_cx };
        let top_y = if route.forward { route.from_anchor } else { route.to_anchor };
        let leg_top = top_y.min(route.effective_route_y);
        for ly in leg_top..block_top {
            surface.put_layered(vcol, ly, glyphs.vertical, Layer::Connector);
        }
        for ly in (block_top + num_lines)..=route.effective_route_y {
            surface.put_layered(vcol, ly, glyphs.vertical, Layer::Connector);
        }
    } else if corridor_w > 0 {
        let vcol = if route.forward { route.from_leg_cx } else { route.to_leg_cx };
        for ly in (block_top + num_lines)..=route.effective_route_y {
            surface.put_layered(vcol, ly, glyphs.vertical, Layer::Connector);
        }
    }
}

/// Default placement: wrap at the geom's `avail`, center on the geom
/// anchor column, and stack above the corridor row by `stack_offset`.
fn default_placement(
    tcx: &TransitionCtx<'_>,
    text: &str,
    row: usize,
    route: &RouteGeometry,
    avail: usize,
) -> LabelBlock {
    let wrapped = wrap_label(text, avail);
    let center_x = if row > 0 { tcx.geom.stacked_base_x } else { tcx.geom.base_x };
    let block_top = route.effective_route_y.saturating_sub(tcx.stack_offset);
    LabelBlock { lines: wrapped.lines, center_x, block_top }
}

/// Own-leg placement for an aligned edge with corridor siblings: the
/// block rides the leg segment that belongs exclusively to this
/// transition (between the corridor row and the target box), centered
/// within it. All values are measured from the route geometry — no
/// fixed offsets.
fn own_leg_placement(tcx: &TransitionCtx<'_>, text: &str, route: &RouteGeometry) -> LabelBlock {
    let ride_col = route.to_leg_cx;
    let (wrap_w, center_x) =
        riding_placement_cols(tcx.avoid_junction_cols, ride_col, tcx.canvas_width, tcx.geom.avail);

    let wrapped = wrap_label(text, wrap_w);

    // Rows the gap's row-0 label block overhangs the corridor row
    // (odd-height blocks straddle it): the exclusive segment starts
    // below that dip for forward edges, ends above the overhang for
    // upward ones, so the riding block never lands on the sibling's
    // straddling rows.
    let skip_below = tcx.gap_row0_lines.div_ceil(2).saturating_sub(1);
    let skip_above = tcx.gap_row0_lines / 2;

    // The exclusive leg segment between the corridor row and the target
    // box, exclusive of both; the block centers within it. When the
    // segment is tight the block fills it from the segment start.
    let (seg_start, seg_len) = if route.forward {
        (
            route.effective_route_y + 1 + skip_below,
            route.to_anchor.saturating_sub(route.effective_route_y + 1 + skip_below),
        )
    } else {
        (
            route.to_anchor + 1,
            route.effective_route_y.saturating_sub(route.to_anchor + 1).saturating_sub(skip_above),
        )
    };
    let block_top = seg_start + seg_len.saturating_sub(wrapped.line_count) / 2;

    LabelBlock { lines: wrapped.lines, center_x, block_top }
}
