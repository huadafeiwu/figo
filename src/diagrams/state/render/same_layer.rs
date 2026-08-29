//! Same-layer transitions: the direct horizontal arrow drawn between two
//! boxes on the same row (no V-H-V detour), including its embedded label.
//!
//! NOTE: with the longest-path layering in `layout.rs`, transition
//! endpoints always land on different layers (see the invariant test in
//! `sugiyama.rs`), so this path is currently unreachable. It implements
//! the documented same-layer behavior and is kept in case the layering
//! ever gains support for same-layer transitions.

use unicode_width::UnicodeWidthStr;

use crate::canvas::Layer;
use crate::diagrams::state::render::transition::TransitionCtx;
use crate::render::surface::Surface;
use crate::style::{BorderStyle, Charset};
use crate::text::wrap_label;

use super::avoidance::avoid_box_x;

/// Draw a same-layer horizontal arrow with its label embedded in the
/// corridor line. Returns `true` if drawn, `false` when another box sits
/// between the endpoints (the caller falls back to the V-H-V path).
pub(super) fn draw_same_layer_transition(
    surface: &mut Surface<'_>,
    tcx: &TransitionCtx<'_>,
    label: Option<&str>,
) -> bool {
    let glyphs = BorderStyle::Single.glyphs(tcx.ctx.charset);
    let from_cx = tcx.geom.from_cx;
    let to_cx = tcx.geom.to_cx;
    let left_edge = tcx.geom.left_x;
    let right_edge = tcx.geom.right_x;
    let ly = tcx.from.y + tcx.from.h / 2;

    // Check if any other box in the same layer sits between from and to.
    let has_obstacle = tcx.all_layouts.iter().any(|layout| {
        let r = &layout.rect;
        r != &tcx.from
            && r != &tcx.to
            && r.y == tcx.from.y
            && r.right() > left_edge
            && r.x < right_edge
    });
    if has_obstacle {
        return false;
    }

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
        match tcx.ctx.charset {
            Charset::Ascii => '>',
            Charset::Unicode => '▶',
        }
    } else {
        match tcx.ctx.charset {
            Charset::Ascii => '<',
            Charset::Unicode => '◀',
        }
    };
    surface.put_layered(right_edge, ly, arrow, Layer::ConnectorEnd);

    if let Some(text) = label {
        let avail = tcx.geom.avail;
        let wrapped = wrap_label(text, avail);
        let lines = &wrapped.lines;
        let n = wrapped.line_count;
        let mid_x = tcx.geom.base_x;
        let block_top = ly.saturating_sub(n / 2);

        for (i, line) in lines.iter().enumerate() {
            let lw = UnicodeWidthStr::width(line.as_str());
            let mut label_x = mid_x.saturating_sub(lw / 2);
            label_x = label_x.max(left_edge).min(right_edge.saturating_sub(lw));
            let label_y = block_top + i;
            // Scene D: don't let label cover any box.
            label_x = avoid_box_x(
                label_x,
                lw,
                label_y,
                tcx.from,
                tcx.to,
                tcx.all_layouts,
                tcx.canvas_width,
            );
            surface.put_str_layered(label_x, label_y, line, Layer::Label);

            // Restore `---` on both sides of the label on the corridor row.
            if tcx.geom.corridor_w > 0 {
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
    true
}
