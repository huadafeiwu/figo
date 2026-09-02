//! State box drawing: the initial-state arrow, simple rounded pills, and
//! accepting states with a double rounded border.

use std::collections::HashMap;

use unicode_width::UnicodeWidthStr;

use crate::canvas::Layer;
use crate::diagrams::state::layout::StateLayout;
use crate::diagrams::state::render::StateLayoutRef;
use crate::diagrams::state::types::StateType;
use crate::error::Result;
use crate::render::node::Node;
use crate::render::surface::Surface;
use crate::render::widget::{LayoutContext, MeasureContext, PaintContext, Rect, Widget};
use crate::style::{BorderStyle, Charset, HAlign, VAlign};
use crate::text::wrap_label;

/// The state's label wrapped to its box interior. Derived from the same
/// `wrap_label` call `state_size` sized the box with, so the drawn
/// lines and the box dimensions cannot drift apart.
fn label_lines(layout: &StateLayout) -> Vec<String> {
    wrap_label(&layout.label, layout.rect.w.saturating_sub(2).max(2)).lines
}

/// Draw the initial-state pseudo arrow (`*────>`) into the first state.
pub(super) fn draw_initial_arrow(
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

/// Draw every state box.
pub(super) fn draw_states(
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
        .content(label_lines(layout))
        .align(HAlign::Center, VAlign::Middle);
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
        // Re-draw the label lines at the center.
        let lines = label_lines(layout);
        let top = layout.rect.y + (layout.rect.h.saturating_sub(lines.len().max(1))) / 2;
        for (i, line) in lines.iter().enumerate() {
            let lw = UnicodeWidthStr::width(line.as_str());
            let lx = layout.rect.x + (layout.rect.w.saturating_sub(lw)) / 2;
            surface.put_str_layered(lx, top + i, line, Layer::NodeContent);
        }
    }
}
