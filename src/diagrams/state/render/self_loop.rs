//! Self-loop drawing: the small loop attached to the right edge of a
//! state box, with its label to the right of the loop.

use unicode_width::UnicodeWidthStr;

use crate::canvas::Layer;
use crate::render::surface::Surface;
use crate::render::widget::{PaintContext, Rect};
use crate::style::{BorderStyle, Charset};
use crate::text::wrap_label;

/// Draw a self-loop on the right edge of `rect` with an optional label.
pub(super) fn draw_self_loop(
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
        let lines = wrap_label(text, avail).lines;
        for (i, line) in lines.iter().enumerate() {
            let lw = UnicodeWidthStr::width(line.as_str());
            let lx = (loop_x + 2).min(canvas_width.saturating_sub(lw));
            surface.put_str_layered(lx, top + i, line, Layer::Label);
        }
    }
}
