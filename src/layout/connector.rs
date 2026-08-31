//! Connector — orthogonal line routing between two nodes with an
//! optional arrowhead and a label.
//!
//! Connectors know the rectangles of both endpoints and any other nodes
//! the caller wants to avoid. The router walks in canonical patterns
//! (straight run, V-H-V, H-V-H, L-shape) and, when the path would
//! intersect an avoidance rectangle, picks a detour.
//!
//! Routing geometry helpers (`natural_mid_y`, `Segment`, path builders,
//! etc.) live in [`super::routing`] so this file stays under 250 lines.

use unicode_width::UnicodeWidthStr;

use crate::canvas::{Canvas, Layer};
use crate::style::{Charset, LineStyle};
use crate::text::wrap_label;

use super::geom::{Anchor, Rect};
use super::node::{horizontal_line_glyph, vertical_line_glyph};
use super::riding::{
    RidingLabel, beside_line_label_avail, corridor_label_avail, corridor_label_block_top,
};
use super::routing::{
    Segment, build_three_h_segment, build_three_segment, detoured_mid_x, detoured_mid_y,
    natural_mid_y, path_intersects_any, side_route_column, snap_outside, straight_vertical,
};

/// Pick the arrowhead glyph that points inward along the source's
/// dominant anchor direction.
///
/// - S → N: arrow points DOWN at target top
/// - N → S: arrow points UP at target bottom
/// - E → W: arrow points RIGHT at target left
/// - W → E: arrow points LEFT at target right
pub fn arrow_glyph_for_pair(src: Anchor, _tgt: Anchor, style: LineStyle, charset: Charset) -> char {
    let dir = match src {
        Anchor::South | Anchor::SouthEast | Anchor::SouthWest => Dir::Down,
        Anchor::North | Anchor::NorthEast | Anchor::NorthWest => Dir::Up,
        Anchor::West => Dir::Left,
        _ => Dir::Right,
    };
    glyph_for(dir, style, charset)
}

/// Derive the arrowhead from the geometric direction the connector
/// arrives at the target. Used by side-route rendering where the path
/// geometry (not the source anchor) determines the arrival direction.
pub fn arrow_from_path(last_dx: i32, last_dy: i32, style: LineStyle, charset: Charset) -> char {
    let dir = if last_dy > 0 {
        Dir::Down
    } else if last_dy < 0 {
        Dir::Up
    } else if last_dx > 0 {
        Dir::Right
    } else if last_dx < 0 {
        Dir::Left
    } else {
        Dir::Down
    };
    glyph_for(dir, style, charset)
}

#[derive(Clone, Copy, Debug)]
enum Dir {
    Up,
    Down,
    Left,
    Right,
}

fn glyph_for(dir: Dir, style: LineStyle, charset: Charset) -> char {
    match (dir, style, charset) {
        (Dir::Up, LineStyle::Bold, _) => '⇑',
        (Dir::Up, _, Charset::Unicode) => '↑',
        (Dir::Up, _, Charset::Ascii) => '^',
        (Dir::Down, LineStyle::Bold, _) => '⇓',
        (Dir::Down, _, Charset::Unicode) => '↓',
        (Dir::Down, _, Charset::Ascii) => 'v',
        (Dir::Left, LineStyle::Bold, _) => '⇐',
        (Dir::Left, _, Charset::Unicode) => '←',
        (Dir::Left, _, Charset::Ascii) => '<',
        (Dir::Right, LineStyle::Bold, _) => '⇒',
        (Dir::Right, _, Charset::Unicode) => '→',
        (Dir::Right, _, Charset::Ascii) => '>',
    }
}

/// Connector with source/target endpoints, optional arrowhead, label,
/// line style, and avoidance rectangles.
#[derive(Clone, Debug)]
pub struct Connector {
    pub source: (usize, usize),
    pub target: (usize, usize),
    pub source_anchor: Anchor,
    pub target_anchor: Anchor,
    pub source_rect: Rect,
    pub target_rect: Rect,
    pub avoid: Vec<Rect>,
    pub style: LineStyle,
    pub charset: Charset,
    pub arrow_tail: bool,
    pub arrow_head: char,
    pub label: Option<String>,
    /// User-specified canvas width — the hard upper limit for label wrapping.
    /// When `None`, falls back to the canvas width at draw time.
    pub user_width: Option<usize>,
    /// Riding directive for vertical-connector labels colliding with
    /// same-source corridor siblings (see [`RidingLabel`]).
    pub riding: Option<RidingLabel>,
}

impl Connector {
    /// Create a new connector. The arrowhead glyph is derived from the
    /// source anchor so it points INTO the target.
    pub fn new(
        source_rect: Rect,
        target_rect: Rect,
        source_anchor: Anchor,
        target_anchor: Anchor,
        style: LineStyle,
        charset: Charset,
    ) -> Self {
        let head = arrow_glyph_for_pair(source_anchor, target_anchor, style, charset);
        Self {
            source: snap_outside(source_rect, source_anchor),
            target: snap_outside(target_rect, target_anchor),
            source_anchor,
            target_anchor,
            source_rect,
            target_rect,
            avoid: Vec::new(),
            style,
            charset,
            arrow_tail: false,
            arrow_head: head,
            label: None,
            user_width: None,
            riding: None,
        }
    }

    /// Set the user-specified canvas width as a hard limit for label wrapping.
    pub fn with_user_width(mut self, width: usize) -> Self {
        self.user_width = Some(width);
        self
    }

    /// Set the riding directive for this connector's label (see
    /// [`RidingLabel`]).
    pub fn with_riding_label(mut self, riding: RidingLabel) -> Self {
        self.riding = Some(riding);
        self
    }

    /// Add a single rectangle to the avoidance list.
    pub fn with_avoid(mut self, rect: Rect) -> Self {
        self.avoid.push(rect);
        self
    }

    /// Add many rectangles to the avoidance list at once.
    pub fn with_avoids(mut self, rects: impl IntoIterator<Item = Rect>) -> Self {
        self.avoid.extend(rects);
        self
    }

    /// Render the connector (forward edge) onto the canvas.
    pub fn render(&self, canvas: &mut Canvas) {
        let path = self.compute_path();
        self.render_segments(canvas, &path);
        let (tx, ty) = self.target;
        canvas.put_layered(tx, ty, self.arrow_head, Layer::ConnectorEnd, None);
        if let Some(label) = &self.label {
            self.draw_label(canvas, label, &path);
        }
    }

    /// Render a back-edge via a side corridor to the right of every
    /// node, so it does not punch through intermediates. The path is:
    /// H out from the source's right edge → V along the side corridor →
    /// H into the target's right edge. The arrowhead points LEFT into the
    /// target, avoiding the top of the target where forward edges land.
    pub fn render_side_route(&self, canvas: &mut Canvas, all_rects: &[Rect], canvas_w: usize) {
        let route_x = side_route_column(all_rects, canvas_w);
        let src_right = self.source_rect.right();
        let src_cy = self.source_rect.cy();
        let tgt_right = self.target_rect.right();
        let tgt_cy = self.target_rect.cy();
        let h_ch = horizontal_line_glyph(self.style, self.charset);
        let v_ch = vertical_line_glyph(self.style, self.charset);

        // H out from the source's right edge to the side corridor.
        if route_x > src_right {
            canvas.put_horizontal_layered(
                src_right,
                src_cy,
                route_x - src_right,
                h_ch,
                Layer::Connector,
            );
        }
        // V along the side corridor between source and target rows.
        let (lo, hi) = if src_cy < tgt_cy { (src_cy, tgt_cy) } else { (tgt_cy, src_cy) };
        canvas.put_vertical_layered(route_x, lo, hi - lo + 1, v_ch, Layer::Connector);
        // H from the side corridor into the target's right edge.
        if route_x > tgt_right {
            canvas.put_horizontal_layered(
                tgt_right,
                tgt_cy,
                route_x - tgt_right,
                h_ch,
                Layer::Connector,
            );
        }
        // Arrowhead pointing LEFT into the target's right edge.
        let head = arrow_from_path(-1, 0, self.style, self.charset);
        canvas.put_layered(tgt_right, tgt_cy, head, Layer::ConnectorEnd, None);
        if let Some(label) = &self.label {
            // Place the label in the side corridor, to the right of the
            // vertical line. Wrap to the remaining canvas width (not
            // user_width, because route_x may be past user_width when
            // the canvas was widened for node boxes).
            let lx = route_x + 1;
            let avail = canvas_w.saturating_sub(lx).max(10);
            let lines = wrap_label(label, avail).lines;
            for (i, line) in lines.iter().enumerate() {
                let line_w = UnicodeWidthStr::width(line.as_str());
                let clamped_lx = lx.min(canvas_w.saturating_sub(line_w));
                canvas.put_str_layered(clamped_lx, src_cy + i, line, Layer::Label, None);
            }
        }
    }

    /// Draw the segments onto the canvas.
    fn render_segments(&self, canvas: &mut Canvas, path: &[Segment]) {
        let h_ch = horizontal_line_glyph(self.style, self.charset);
        let v_ch = vertical_line_glyph(self.style, self.charset);
        for segment in path {
            match segment {
                Segment::H { x, y, len } => {
                    canvas.put_horizontal_layered(*x, *y, *len, h_ch, Layer::Connector);
                }
                Segment::V { x, y, len } => {
                    canvas.put_vertical_layered(*x, *y, *len, v_ch, Layer::Connector);
                }
            }
        }
    }

    /// Compute the orthogonal path from source to target.
    fn compute_path(&self) -> Vec<Segment> {
        let (sx, sy) = self.source;
        let (tx, ty) = self.target;
        let from_south = matches!(self.source_anchor, Anchor::South);
        let from_north = matches!(self.source_anchor, Anchor::North);
        let to_north = matches!(self.target_anchor, Anchor::North);
        let to_south = matches!(self.target_anchor, Anchor::South);
        let same_x = sx == tx;
        let same_y = sy == ty;

        // Vertical-axis flow (south → north, or north → south).
        if (from_south && to_north) || (from_north && to_south) {
            if same_x {
                return straight_vertical(sx, sy, ty);
            }
            let mid_y = natural_mid_y(sy, ty, &self.source_rect, &self.target_rect);
            let path = build_three_segment(sx, sy, tx, ty, mid_y);
            if !path_intersects_any(&path, &self.avoid) {
                return path;
            }
            let safe_y = detoured_mid_y(sy, ty, &self.avoid, &self.source_rect, &self.target_rect);
            return build_three_segment(sx, sy, tx, ty, safe_y);
        }

        // Horizontal-axis flow (east ↔ west).
        let target_west = matches!(self.target_anchor, Anchor::West);
        let target_east = matches!(self.target_anchor, Anchor::East);
        let from_east = matches!(self.source_anchor, Anchor::East);
        let from_west = matches!(self.source_anchor, Anchor::West);
        if (from_east && target_west) || (from_west && target_east) {
            if same_y {
                return if sx < tx {
                    vec![Segment::H { x: sx, y: sy, len: tx - sx + 1 }]
                } else {
                    vec![Segment::H { x: tx, y: sy, len: sx - tx + 1 }]
                };
            }
            let mid_x = (sx + tx) / 2;
            let path = build_three_h_segment(sx, sy, tx, ty, mid_x);
            if !path_intersects_any(&path, &self.avoid) {
                return path;
            }
            return build_three_h_segment(sx, sy, tx, ty, detoured_mid_x(&self.avoid));
        }

        // Fallback: simple L-shape.
        if same_x {
            return straight_vertical(sx, sy, ty);
        }
        let path = vec![
            Segment::H { x: sx.min(tx), y: sy, len: sx.abs_diff(tx) },
            Segment::V { x: tx, y: sy.min(ty), len: sy.abs_diff(ty) },
        ];
        if !path_intersects_any(&path, &self.avoid) {
            return path;
        }
        let safe_y = detoured_mid_y(sy, ty, &self.avoid, &self.source_rect, &self.target_rect);
        build_three_segment(sx, sy, tx, ty, safe_y)
    }

    /// Place the label embedded in the corridor line. Long labels are
    /// wrapped to the corridor width and the corridor `---` is restored
    /// on both sides. For purely vertical connectors, the label sits
    /// beside the line and `|` is restored above/below.
    fn draw_label(&self, canvas: &mut Canvas, label: &str, path: &[Segment]) {
        let (sx, sy) = self.source;
        let (tx, ty) = self.target;
        let mid_x = (sx + tx) / 2;
        let w_limit = self.user_width.unwrap_or_else(|| canvas.width());
        let h_ch = horizontal_line_glyph(self.style, self.charset);
        let v_ch = vertical_line_glyph(self.style, self.charset);

        let has_h = path.iter().any(|s| matches!(s, Segment::H { .. }));
        if !has_h {
            // Purely vertical: label embedded IN the vertical line. The
            // label text covers `|` on its rows; `|` is restored above and
            // below the label block so the connector stays continuous.
            let downward = sy <= ty;
            let (avail, center_x, block_top) = match self.riding {
                // Same-source corridor siblings place their corridor row
                // at the default anchor (sy+1): the caller (which sees
                // every node's geometry) has already picked the exclusive
                // leg-segment stretch — clear of sibling blocks and
                // intermediate boxes — the wrap width, and the column to
                // center on (the leg, or the beside-span center when the
                // ladder stepped off the line).
                Some(riding) => (riding.wrap_w, riding.center_x, riding.block_top),
                None => {
                    let avail = beside_line_label_avail(sx, w_limit);
                    let n = wrap_label(label, avail).line_count;
                    let top = if downward { sy + 1 } else { sy.saturating_sub(n) };
                    (avail, sx, top)
                }
            };
            let wrapped = wrap_label(label, avail);
            let lines = &wrapped.lines;
            let n = wrapped.line_count;

            // Draw label lines centered on the block's center column.
            // When it equals the leg column the block rides the line
            // (covering it on its rows); when the ladder stepped off the
            // line the block sits beside it and the leg stays visible.
            for (i, line) in lines.iter().enumerate() {
                let line_w = UnicodeWidthStr::width(line.as_str());
                let lx = center_x.saturating_sub(line_w / 2);
                canvas.put_str_layered(lx, block_top + i, line, Layer::Label, None);
            }

            // Restore `|` above and below the label block.
            let (lo, hi) = if downward { (sy, ty) } else { (ty, sy) };
            for ly in lo..block_top {
                canvas.put_layered(sx, ly, v_ch, Layer::Connector, None);
            }
            for ly in (block_top + n)..=hi {
                canvas.put_layered(sx, ly, v_ch, Layer::Connector, None);
            }
            return;
        }
        if let Some(&Segment::H { x, y, len }) =
            path.iter().find(|s| matches!(s, Segment::H { .. }))
        {
            // Horizontal corridor: label embedded IN the corridor line (y),
            // with `---` restored on both sides.
            let center = if sx != tx { mid_x } else { x + len / 2 };
            // Wrap to corridor width minus 4 (left/right `---` padding).
            let avail = corridor_label_avail(len, w_limit);
            let wrapped = wrap_label(label, avail);
            let lines = &wrapped.lines;
            let n = wrapped.line_count;
            // Keep the whole label block inside the connector's vertical
            // span instead of clamping each line. Centering on the
            // corridor row can start the block above the source anchor;
            // the old per-line clamp then mapped two lines onto the same
            // row, where one silently overwrote the other and label
            // characters were lost. Shifting the block keeps every line.
            let block_top = corridor_label_block_top(y, n, sy, ty);

            // Draw all label lines first.
            let mut label_positions: Vec<(usize, usize, usize)> = Vec::new();
            for (i, line) in lines.iter().enumerate() {
                let line_w = UnicodeWidthStr::width(line.as_str());
                let base_x = center.saturating_sub(line_w / 2).max(x);
                let lx = base_x.min((x + len).saturating_sub(line_w)).max(x);
                let label_y = block_top + i;
                canvas.put_str_layered(lx, label_y, line, Layer::Label, None);
                label_positions.push((lx, line_w, label_y));
            }

            // Restore `---` ONLY on the corridor row (segment y), not on
            // every label row. Also write `+` at junction points where
            // vertical legs meet the corridor.
            if y >= block_top && y < block_top + n {
                let idx = y - block_top;
                if let Some(&(lx, line_w, label_y)) = label_positions.get(idx) {
                    let label_end = lx + line_w;
                    if lx > x {
                        canvas.put_horizontal_layered(x, label_y, lx - x, h_ch, Layer::Connector);
                    }
                    if label_end <= x + len {
                        canvas.put_horizontal_layered(
                            label_end,
                            label_y,
                            (x + len).saturating_sub(label_end),
                            h_ch,
                            Layer::Connector,
                        );
                    }
                    // Write `+` at corridor junction points.
                    let junction_ch = match self.charset {
                        Charset::Ascii => '+',
                        Charset::Unicode => '┼',
                    };
                    if lx > x {
                        canvas.put_layered(x, label_y, junction_ch, Layer::Connector, None);
                    }
                    if label_end < x + len {
                        canvas.put_layered(
                            x + len - 1,
                            label_y,
                            junction_ch,
                            Layer::Connector,
                            None,
                        );
                    }
                }
            }
            return;
        }
        // Same-row horizontal fallback.
        let avail = w_limit.saturating_sub(mid_x).max(2).min(w_limit);
        let lines = wrap_label(label, avail).lines;
        for (i, line) in lines.iter().enumerate() {
            let line_w = UnicodeWidthStr::width(line.as_str());
            let label_y = sy.saturating_sub(1 + i);
            let lx = mid_x.saturating_sub(line_w / 2);
            canvas.put_str_layered(lx, label_y, line, Layer::Label, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_vertical_line() {
        let src = Rect::new(0, 0, 4, 3);
        let tgt = Rect::new(0, 5, 4, 3);
        let c = Connector::new(
            src,
            tgt,
            Anchor::South,
            Anchor::North,
            LineStyle::Simple,
            Charset::Ascii,
        );
        let segs = c.compute_path();
        assert!(matches!(segs[0], Segment::V { .. }));
    }

    #[test]
    fn avoids_obstacle_in_path() {
        let src = Rect::new(0, 0, 4, 3);
        let tgt = Rect::new(10, 8, 4, 3);
        let obstacle = Rect::new(4, 3, 6, 3);
        let c = Connector::new(
            src,
            tgt,
            Anchor::South,
            Anchor::North,
            LineStyle::Simple,
            Charset::Ascii,
        )
        .with_avoid(obstacle);
        let path = c.compute_path();
        let intersects = path.iter().any(|s| {
            let r = match *s {
                Segment::H { x, y, len } => Rect::new(x, y, len.max(1), 1),
                Segment::V { x, y, len } => Rect::new(x, y, 1, len.max(1)),
            };
            r.overlaps(&obstacle)
        });
        assert!(!intersects, "path {path:?} intersects obstacle {obstacle:?}");
    }

    #[test]
    fn arrowhead_points_down_for_south_source() {
        let c = Connector::new(
            Rect::new(0, 0, 4, 3),
            Rect::new(0, 5, 4, 3),
            Anchor::South,
            Anchor::North,
            LineStyle::Simple,
            Charset::Unicode,
        );
        assert_eq!(c.arrow_head, '↓');
    }

    #[test]
    fn arrowhead_points_up_for_north_source() {
        let c = Connector::new(
            Rect::new(0, 8, 4, 3),
            Rect::new(0, 0, 4, 3),
            Anchor::North,
            Anchor::South,
            LineStyle::Simple,
            Charset::Unicode,
        );
        assert_eq!(c.arrow_head, '↑');
    }

    #[test]
    fn vertical_label_upward_flow_stays_on_line() {
        // Source below target: upward flow (sy > ty). The label must
        // appear beside the line segment (between ty and sy), not below
        // the source.
        let src = Rect::new(0, 8, 4, 3);
        let tgt = Rect::new(0, 0, 4, 3);
        let mut c = Connector::new(
            src,
            tgt,
            Anchor::North,
            Anchor::South,
            LineStyle::Simple,
            Charset::Ascii,
        );
        c.label = Some("test".to_string());
        c.user_width = Some(40);

        let mut canvas = Canvas::new(40, 12);
        c.render(&mut canvas);
        let out = canvas.render(false);

        assert!(out.contains("test"), "label should be visible:\n{out}");
        assert!(out.contains('|'), "vertical line should be visible:\n{out}");

        // The label should not appear below the source row (y >= 9).
        let below: String = out.lines().skip(9).collect::<Vec<_>>().join("\n");
        assert!(!below.contains("test"), "label should not appear below source:\n{out}");
    }
}
