//! Segment-aware crossing glyphs and junction-arm validation.
//!
//! The raster junction repair cannot tell a crossing (two lines passing
//! through the same cell without connecting) from a real junction —
//! both render as `+`, which reads as a connection. This pass reads the
//! connector-line log (`Canvas::line_log`, recorded by the put_* hooks)
//! and rewrites both:
//!
//! - **Crossings** render as a gap: the vertical stays continuous and
//!   the horizontal yields one blank cell on each side of the crossing
//!   cell. A crossing is a horizontal span and a vertical span
//!   overlapping at a cell interior to BOTH spans; any endpoint on the
//!   cell means a real junction (a leg joining a corridor, a merge) and
//!   keeps its `+`. Orthogonal paths only ever meet their own segments
//!   at endpoints, so edge identity is not needed.
//! - **Unbacked junction arms** are stripped: a repair-made junction
//!   glyph keeps an arm only when same-direction line coverage spans
//!   the glyph cell AND the arm's neighbour — the phantom arms the
//!   raster repair welds onto adjacent lines (e.g. a horizontal run
//!   passing one row above another edge's corner) vanish.

use std::collections::HashSet;

use super::{Canvas, Charset, Directions, Layer, LineStyle, junction_char};
use crate::layout::routing::Segment;

impl Canvas {
    /// Rewrite crossings and unbacked junction arms from the recorded
    /// connector-line log. Call after `repair_connector_junctions`.
    pub fn apply_crossing_pass(&mut self, style: LineStyle, charset: Charset) {
        let log = std::mem::take(&mut self.line_log);
        let mut h_spans: Vec<(usize, usize, usize)> = Vec::new();
        let mut v_spans: Vec<(usize, usize, usize)> = Vec::new();
        let mut h_cover: HashSet<(usize, usize)> = HashSet::new();
        let mut v_cover: HashSet<(usize, usize)> = HashSet::new();
        for seg in &log {
            match *seg {
                Segment::H { x, y, len } => {
                    if len > 1 {
                        h_spans.push((x, y, len));
                    }
                    for c in x..x + len {
                        h_cover.insert((c, y));
                    }
                }
                Segment::V { x, y, len } => {
                    if len > 1 {
                        v_spans.push((x, y, len));
                    }
                    for r in y..y + len {
                        v_cover.insert((x, r));
                    }
                }
            }
        }
        // Crossings: the cell is interior to both spans.
        let mut crossings: HashSet<(usize, usize)> = HashSet::new();
        for &(hx, hy, hlen) in &h_spans {
            for &(vx, vy, vlen) in &v_spans {
                if vx > hx && vx < hx + hlen - 1 && hy > vy && hy < vy + vlen - 1 {
                    crossings.insert((vx, hy));
                }
            }
        }
        // Arm validation: a repair-made junction glyph keeps an arm only
        // when same-direction coverage spans the glyph cell and the arm's
        // neighbour. Pre-written junction glyphs (self-loop corners,
        // corridor junction marks) are authoritative and skipped.
        let touched = std::mem::take(&mut self.repair_touched);
        for (x, y) in touched {
            let Some(cell) = self.cell(x, y) else { continue };
            if cell.layer != Layer::Connector || !is_junction_glyph(cell.ch) {
                continue;
            }
            let dirs = Directions {
                n: y > 0 && v_cover.contains(&(x, y)) && v_cover.contains(&(x, y - 1)),
                s: v_cover.contains(&(x, y)) && v_cover.contains(&(x, y + 1)),
                w: x > 0 && h_cover.contains(&(x, y)) && h_cover.contains(&(x - 1, y)),
                e: h_cover.contains(&(x, y)) && h_cover.contains(&(x + 1, y)),
            };
            let want = desired_glyph(dirs, style, charset, cell.ch);
            if want != cell.ch {
                self.put_layered(x, y, want, Layer::Connector, None);
            }
        }
        // Crossing glyphs (after validation, which keeps their `+`): the
        // vertical stays continuous and the horizontal yields one blank
        // cell on each side — but only over plain horizontal glyphs
        // whose far side is not a junction (a corner or tee one cell
        // away needs its adjacent dash to stay connected).
        for &(x, y) in &crossings {
            let v = match charset {
                Charset::Ascii => '|',
                Charset::Unicode => '│',
            };
            self.put_layered(x, y, v, Layer::Connector, None);
            if x > 1 && is_plain_h(self.cell_char(x - 1, y)) && !is_junction_at(self, x - 2, y) {
                self.put_layered(x - 1, y, ' ', Layer::Connector, None);
            }
            if is_plain_h(self.cell_char(x + 1, y)) && !is_junction_at(self, x + 2, y) {
                self.put_layered(x + 1, y, ' ', Layer::Connector, None);
            }
        }
    }
}

fn is_junction_glyph(ch: char) -> bool {
    matches!(
        ch,
        '+' | '┌'
            | '┐'
            | '└'
            | '┘'
            | '├'
            | '┤'
            | '┬'
            | '┴'
            | '┼'
            | '┏'
            | '┓'
            | '┗'
            | '┛'
            | '┣'
            | '┫'
            | '┳'
            | '┻'
            | '╋'
    )
}

fn is_plain_h(ch: Option<char>) -> bool {
    matches!(ch, Some('-') | Some('─') | Some('━'))
}

fn is_junction_at(canvas: &Canvas, x: usize, y: usize) -> bool {
    canvas.cell(x, y).is_some_and(|c| c.layer == Layer::Connector && is_junction_glyph(c.ch))
}

/// The glyph a junction cell should carry for the backed arm set.
fn desired_glyph(dirs: Directions, style: LineStyle, charset: Charset, current: char) -> char {
    match charset {
        Charset::Ascii => {
            if (dirs.n || dirs.s) && (dirs.e || dirs.w) {
                '+'
            } else if dirs.e || dirs.w {
                '-'
            } else if dirs.n || dirs.s {
                '|'
            } else {
                current
            }
        }
        Charset::Unicode => junction_char(dirs, style).unwrap_or(if dirs.e || dirs.w {
            match style {
                LineStyle::Bold => '━',
                LineStyle::Simple | LineStyle::BoxDrawing => '─',
            }
        } else if dirs.n || dirs.s {
            match style {
                LineStyle::Bold => '┃',
                LineStyle::Simple | LineStyle::BoxDrawing => '│',
            }
        } else {
            current
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::{Charset, LineStyle};

    #[test]
    fn crossing_renders_as_gap() {
        // A horizontal span and a vertical span cross at a cell interior
        // to both: the vertical stays continuous and the horizontal
        // yields a blank cell on each side — no `+`, which would read as
        // a connection.
        let mut c = Canvas::new(20, 12);
        c.put_vertical_layered(10, 0, 12, '|', Layer::Connector);
        c.put_horizontal_layered(0, 6, 20, '-', Layer::Connector);
        c.repair_connector_junctions(LineStyle::Simple, Charset::Ascii);
        c.apply_crossing_pass(LineStyle::Simple, Charset::Ascii);
        let row = c.render(false).lines().nth(6).unwrap().to_string();
        let cells: Vec<char> = row.chars().collect();
        assert_eq!(cells[10], '|', "crossing cell keeps the vertical:\n{row}");
        assert_eq!(cells[9], ' ', "horizontal yields west of the crossing:\n{row}");
        assert_eq!(cells[11], ' ', "horizontal yields east of the crossing:\n{row}");
    }

    #[test]
    fn endpoint_on_the_span_stays_a_junction() {
        // The horizontal ENDS on the vertical's column: a real junction
        // (a leg joining a corridor), not a crossing — it keeps its `+`.
        let mut c = Canvas::new(20, 12);
        c.put_vertical_layered(10, 0, 12, '|', Layer::Connector);
        c.put_horizontal_layered(0, 6, 11, '-', Layer::Connector);
        c.repair_connector_junctions(LineStyle::Simple, Charset::Ascii);
        c.apply_crossing_pass(LineStyle::Simple, Charset::Ascii);
        let row = c.render(false).lines().nth(6).unwrap().to_string();
        let cells: Vec<char> = row.chars().collect();
        assert_eq!(cells[10], '+', "an endpoint landing is a junction:\n{row}");
    }

    #[test]
    fn unbacked_arm_downgrades_to_a_line() {
        // A vertical starting one row below a horizontal's endpoint: the
        // raster repair welds a `+` with a south arm, but no vertical
        // covers the glyph cell, so the arm is unbacked and the cell
        // falls back to the plain horizontal glyph.
        let mut c = Canvas::new(20, 12);
        c.put_horizontal_layered(0, 6, 7, '-', Layer::Connector);
        c.put_vertical_layered(6, 7, 3, '|', Layer::Connector);
        c.repair_connector_junctions(LineStyle::Simple, Charset::Ascii);
        let row = c.render(false).lines().nth(6).unwrap().to_string();
        assert!(row.contains('+'), "repair welds the phantom arm first:\n{row}");
        c.apply_crossing_pass(LineStyle::Simple, Charset::Ascii);
        let row = c.render(false).lines().nth(6).unwrap().to_string();
        let cells: Vec<char> = row.chars().collect();
        assert_eq!(cells[6], '-', "unbacked arm downgrades to the line:\n{row}");
    }
}
