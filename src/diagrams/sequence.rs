//! Sequence diagrams with participants and messages.
//!
//! Each participant occupies a "lane" of `LaneLayout::lane_width` cells.
//! The participant's box is inset within the lane so adjacent boxes show
//! a `LANE_GAP` (2-cell) horizontal gap.
//!
//! Lane sizing follows the global label rules: participant names are
//! structural (lanes always fit them), and message labels widen the
//! lane-to-lane distance — or the right margin for the last lane — so
//! they fit on one line, bounded by the display budget. Labels that
//! would exceed the budget wrap inside their `LaneLayout::label_region`
//! (private by design — the lane model owns it), which never covers a
//! lifeline column.

use std::collections::HashMap;
use std::fmt;

use crate::canvas::{Canvas, Layer};
use crate::error::{FigoError, Result};
use crate::layout::{Anchor, Connector, Rect};
use crate::style::{BorderStyle, Charset, LineStyle};
use crate::text::wrap_label;
use unicode_width::UnicodeWidthStr;

/// Horizontal gap (in cells) between adjacent participant boxes.
const LANE_GAP: usize = 2;

/// One-sided inset within a lane (cells reserved on the left of every box).
const LANE_GAP_HALF: usize = LANE_GAP / 2;

/// Columns a self-message loop occupies right of its lifeline
/// (`draw_self_message` draws the loop over `lifeline .. lifeline + 3`).
const SELF_LOOP_COLS: usize = 4;

/// Cap on the ideal lane spread, so a wide canvas with few participants
/// does not produce absurdly wide boxes. Names and labels may exceed it.
const IDEAL_LANE_CAP: usize = 40;

/// A message in a sequence diagram.
#[derive(Debug, Clone)]
pub struct SequenceMessage {
    /// The sending participant's name.
    pub from: String,
    /// The receiving participant's name.
    pub to: String,
    /// The label displayed on the message arrow.
    pub label: String,
}

/// Builder for sequence diagrams.
pub struct SequenceDiagram<'a> {
    width: usize,
    /// Display budget for label-driven lane growth: the canvas may grow
    /// beyond `width` for labels, but never beyond this. Defaults to
    /// `width` (no growth) so library renders stay deterministic; the
    /// CLI raises it to the detected terminal width.
    label_budget: usize,
    charset: Charset,
    participants: Vec<&'a str>,
    messages: Vec<SequenceMessage>,
    color: bool,
}

impl<'a> SequenceDiagram<'a> {
    /// Create a new sequence diagram builder.
    pub fn new(width: usize, charset: Charset) -> Self {
        Self {
            width,
            label_budget: width,
            charset,
            participants: Vec::new(),
            messages: Vec::new(),
            color: false,
        }
    }

    /// Set the display budget for label-driven lane growth.
    pub fn label_budget(mut self, budget: usize) -> Self {
        self.label_budget = budget;
        self
    }

    /// Add a participant.
    pub fn add_participant(mut self, name: &'a str) -> Self {
        self.participants.push(name);
        self
    }

    /// Add a message between participants.
    pub fn add_message(mut self, from: &str, to: &str, label: &str) -> Self {
        self.messages.push(SequenceMessage {
            from: from.to_string(),
            to: to.to_string(),
            label: label.to_string(),
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
        if self.participants.is_empty() {
            return Err(FigoError::MissingFields("no participants specified".into()));
        }
        if self.messages.is_empty() {
            return Err(FigoError::MissingFields("no messages specified".into()));
        }

        let rounded = BorderStyle::Rounded.glyphs(self.charset);
        let v_ch = rounded.vertical;
        let tee_up = rounded.tee_up;

        let n = self.participants.len();
        let name_to_idx: HashMap<&str, usize> =
            self.participants.iter().enumerate().map(|(i, &p)| (p, i)).collect();

        let layout = LaneLayout::new(
            &self.participants,
            &self.messages,
            &name_to_idx,
            self.width,
            self.label_budget,
        );
        let box_width = layout.box_width();

        if box_width < 6 {
            return Err(FigoError::InvalidDimensions(
                "width too small for participant names".into(),
            ));
        }

        // Vertical layout:
        //   row 0                header top border
        //   row 1                header name row
        //   row 2                header bottom border + tee-junction into lifeline
        //   rows 3..total_height lifelines (Layer::Connector) interleaved with messages
        let header_height: usize = 3;

        // Pre-compute label line counts per message from the lane
        // regions — the same widths the draw pass wraps at — so canvas
        // height always matches what gets drawn.
        let msg_heights: Vec<usize> = self
            .messages
            .iter()
            .map(|msg| {
                let from_idx = name_to_idx.get(msg.from.as_str()).copied().unwrap_or(0);
                let to_idx = name_to_idx.get(msg.to.as_str()).copied().unwrap_or(0);
                let (left, right) = layout.label_region(from_idx, to_idx);
                let avail = right.saturating_sub(left) + 1;
                let label_lines = wrap_label(&msg.label, avail).line_count;
                let base = if from_idx == to_idx { 3 } else { 2 };
                label_lines.max(1) + base
            })
            .collect();
        let msg_rows: usize = msg_heights.iter().sum();
        let total_height = header_height + msg_rows + 1;

        let mut canvas = Canvas::new(layout.total_width, total_height);

        // Paint pass 1: lifelines at Layer::Connector (low) drawn FIRST so
        // they extend from the header bottom down to the canvas bottom.
        // The header box drawn next will cleanly cover the lifeline start.
        let lifeline_start = header_height;
        let lifeline_end = total_height.saturating_sub(1);
        for i in 0..n {
            let lifeline_x = layout.lifeline(i);
            canvas.put_vertical_layered(
                lifeline_x,
                lifeline_start,
                lifeline_end.saturating_sub(lifeline_start) + 1,
                v_ch,
                Layer::Connector,
            );
        }

        // Paint pass 2: header boxes at Layer::NodeBorder, names at
        // Layer::NodeContent, and the tee-junction where lifeline meets box.
        for (i, name) in self.participants.iter().enumerate() {
            let hx = i * layout.lane_width + LANE_GAP_HALF;
            canvas.draw_rect(hx, 0, box_width, header_height, &rounded)?;
            let name_x = hx + (box_width.saturating_sub(name.width())) / 2;
            canvas.put_str_layered(name_x, 1, name, Layer::NodeContent, None);
            // Tee-junction glyph anchors the lifeline visually to the header.
            let lifeline_x = layout.lifeline(i);
            canvas.put_layered(lifeline_x, header_height - 1, tee_up, Layer::NodeBorder, None);
        }

        // Paint pass 3: messages rendered through Connector. Each message
        // sees TWO 1×1 invisible rects at the lifeline columns; the
        // straight horizontal path is the arrow body and the arrowhead sits
        // one cell outside the target lifeline (at Layer::ConnectorEnd).
        let mut msg_y = header_height + 1;
        for (mi, msg) in self.messages.iter().enumerate() {
            let from_idx = name_to_idx.get(msg.from.as_str()).copied().unwrap_or(0);
            let to_idx = name_to_idx.get(msg.to.as_str()).copied().unwrap_or(0);
            let from_x = layout.lifeline(from_idx);
            let to_x = layout.lifeline(to_idx);
            let msg_h = msg_heights[mi];
            let (region_left, region_right) = layout.label_region(from_idx, to_idx);
            let region_w = region_right.saturating_sub(region_left) + 1;
            let label_lines = wrap_label(&msg.label, region_w).lines;
            let num_lines = label_lines.len().max(1);
            // Place the arrow below the label block so labels never overlap
            // the header or previous message's arrow.
            let arrow_y = msg_y + num_lines;

            if from_x == to_x {
                // Self-message: small loop to the right of the lifeline.
                Self::draw_self_message(&mut canvas, from_x, arrow_y, v_ch, &label_lines);
            } else {
                let left_to_right = from_x < to_x;

                // 1×1 invisible endpoint rects on the arrow row. Connector
                // computes a single horizontal segment between them.
                let source_rect = Rect::new(from_x, arrow_y, 1, 1);
                let target_rect = Rect::new(to_x, arrow_y, 1, 1);
                let (src_anchor, tgt_anchor, arrow_glyph) = if left_to_right {
                    (Anchor::East, Anchor::West, Self::east_glyph(self.charset))
                } else {
                    (Anchor::West, Anchor::East, Self::west_glyph(self.charset))
                };

                let mut c = Connector::new(
                    source_rect,
                    target_rect,
                    src_anchor,
                    tgt_anchor,
                    LineStyle::Simple,
                    self.charset,
                );
                c.arrow_head = arrow_glyph;
                c.render(&mut canvas);

                // Activation emphasis: draw a single activation cell on each
                // lifeline at the arrow row. This sits at Layer::Connector,
                // same level as the lifeline, so visual continuity is kept.
                canvas.put_layered(from_x, arrow_y, v_ch, Layer::Connector, None);
                canvas.put_layered(to_x, arrow_y, v_ch, Layer::Connector, None);

                // Label centered in its region — the free columns between
                // the two lifelines — so it never covers a lifeline.
                for (i, line) in label_lines.iter().enumerate() {
                    let lw = UnicodeWidthStr::width(line.as_str());
                    let label_x = if lw <= region_w {
                        region_left + (region_w - lw) / 2
                    } else {
                        region_left
                    };
                    let ly = arrow_y.saturating_sub(num_lines).saturating_add(i);
                    canvas.put_str_layered(label_x, ly, line, Layer::Label, None);
                }
            }

            msg_y += msg_h;
        }

        // Repair connector junctions so corners and crossings use proper
        // Unicode box-drawing glyphs.
        canvas.repair_connector_junctions(LineStyle::Simple, self.charset);

        Ok(canvas.render(self.color))
    }

    /// Render and return as a `String`. Equivalent to [`Self::build`].
    pub fn render(&self) -> Result<String> {
        self.build()
    }

    /// Draw a 2-row self-message loop to the right of the lifeline. The
    /// label lines (already wrapped to the lane region) start right of
    /// the loop, above the arrow row.
    fn draw_self_message(
        canvas: &mut Canvas,
        from_x: usize,
        arrow_y: usize,
        v_ch: char,
        label_lines: &[String],
    ) {
        let is_unicode = v_ch == '│';
        let corner_top_right = if is_unicode { '┐' } else { '+' };
        let corner_bot_right = if is_unicode { '┘' } else { '+' };
        let h_ch = if is_unicode { '─' } else { '-' };
        let loop_top_x = from_x + 2;
        let loop_bot_x = from_x + 2;
        canvas.put_layered(from_x, arrow_y, h_ch, Layer::Connector, None);
        canvas.put_layered(from_x + 1, arrow_y, corner_top_right, Layer::Connector, None);
        canvas.put_vertical_layered(loop_top_x, arrow_y + 1, 2, v_ch, Layer::Connector);
        canvas.put_layered(loop_bot_x, arrow_y + 2, corner_bot_right, Layer::Connector, None);
        canvas.put_layered(from_x, arrow_y + 2, '<', Layer::ConnectorEnd, None);
        canvas.put_horizontal_layered(from_x + 1, arrow_y + 2, 2, h_ch, Layer::Connector);
        // Multi-line label to the right of the loop, starting above the
        // arrow row so it never overlaps the loop glyphs. Lines are
        // pre-wrapped to the lane region, which starts right of the loop.
        let num_lines = label_lines.len().max(1);
        let label_start_y = arrow_y.saturating_sub(num_lines);
        let label_x = from_x + SELF_LOOP_COLS;
        for (i, line) in label_lines.iter().enumerate() {
            canvas.put_str_layered(label_x, label_start_y + i, line, Layer::Label, None);
        }
    }

    /// Eastward arrowhead glyph for the given charset.
    fn east_glyph(charset: Charset) -> char {
        match charset {
            Charset::Unicode => '▶',
            Charset::Ascii => '>',
        }
    }

    /// Westward arrowhead glyph for the given charset.
    fn west_glyph(charset: Charset) -> char {
        match charset {
            Charset::Unicode => '◀',
            Charset::Ascii => '<',
        }
    }
}
impl fmt::Display for SequenceDiagram<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.build() {
            Ok(s) => write!(f, "{s}"),
            Err(e) => write!(f, "[figo error: {e}]"),
        }
    }
}

/// Lane geometry for a sequence diagram — the single source for where
/// every lifeline sits and how much horizontal room each message label
/// has.
struct LaneLayout {
    /// Number of participant lanes.
    n: usize,
    /// Uniform width of every participant lane.
    lane_width: usize,
    /// Total canvas width (lanes plus the last lane's label margin).
    total_width: usize,
}

impl LaneLayout {
    fn new(
        participants: &[&str],
        messages: &[SequenceMessage],
        name_to_idx: &HashMap<&str, usize>,
        width: usize,
        budget: usize,
    ) -> Self {
        let n = participants.len().max(1);
        let max_name_w = participants.iter().map(|p| p.width()).max().unwrap_or(5);
        // Structural: the box must hold the widest name (box interior is
        // name + 4 cells of padding; the lane adds the inter-box gap).
        let name_lane = max_name_w + 4 + LANE_GAP;
        // Ideal spread across the canvas, capped so a wide canvas with
        // few participants doesn't produce absurdly wide boxes.
        let base_lane = (width / n).min(IDEAL_LANE_CAP).max(name_lane);

        // Label-driven requirements: a label needs its flanking lifelines
        // far enough apart to hold the unwrapped label. Self labels
        // additionally reserve the loop columns right of their lifeline.
        let mut label_lane = 0usize;
        let mut label_margin = 0usize;
        for msg in messages {
            let label = &msg.label;
            if label.is_empty() {
                continue;
            }
            let Some(&fi) = name_to_idx.get(msg.from.as_str()) else { continue };
            let Some(&ti) = name_to_idx.get(msg.to.as_str()) else { continue };
            let lw = label.width();
            if fi == ti {
                // Self label: loop columns, the label, one clear column
                // before the next obstacle.
                let need = SELF_LOOP_COLS + lw + 1;
                if fi + 1 < n {
                    label_lane = label_lane.max(need);
                } else {
                    label_margin = label_margin.max(need);
                }
            } else {
                // One clear column on each side of the label; a label
                // spanning several lanes divides its requirement.
                let span = fi.abs_diff(ti).max(1);
                label_lane = label_lane.max((lw + 2).div_ceil(span));
            }
        }

        // Budget bound: label-driven growth may push the canvas beyond
        // the design width but never beyond the display budget. The
        // structural (name) floor always wins — if names already exceed
        // the budget, the canvas grows anyway because names cannot wrap.
        let lane_cap = (budget / n).max(base_lane);
        let lane_width = base_lane.max(label_lane).min(lane_cap);
        let margin_cap = budget.saturating_sub(lane_width * n);
        let right_margin = label_margin.min(margin_cap);

        // Structural floor for a last-lane self label: even when the
        // budget leaves no margin, the region right of the loop keeps
        // the wrap minimum (2 columns) so no line can spill past the
        // canvas edge and get dropped.
        let last_lifeline = (n - 1) * lane_width + LANE_GAP_HALF + (lane_width - LANE_GAP - 1) / 2;
        let last_self_floor = if label_margin > 0 { last_lifeline + SELF_LOOP_COLS + 2 } else { 0 };
        let total_width = (lane_width * n + right_margin).max(width).max(last_self_floor);
        LaneLayout { n, lane_width, total_width }
    }

    /// Interior width of a participant box.
    fn box_width(&self) -> usize {
        self.lane_width - LANE_GAP
    }

    /// Lifeline column of participant `i` — the exact center column of
    /// its header box.
    fn lifeline(&self, i: usize) -> usize {
        i * self.lane_width + LANE_GAP_HALF + (self.box_width() - 1) / 2
    }

    /// Free columns a message label may occupy (inclusive bounds),
    /// between its flanking obstacles: the two lifelines for a regular
    /// message, or the self-loop's right edge and the next lifeline (or
    /// canvas edge for the last lane) for a self message. A label
    /// wrapped to this region never covers a lifeline column.
    fn label_region(&self, from_idx: usize, to_idx: usize) -> (usize, usize) {
        if from_idx == to_idx {
            let lx = self.lifeline(from_idx);
            let right = if from_idx + 1 < self.n {
                self.lifeline(from_idx + 1).saturating_sub(1)
            } else {
                self.total_width.saturating_sub(1)
            };
            (lx + SELF_LOOP_COLS, right)
        } else {
            let a = self.lifeline(from_idx);
            let b = self.lifeline(to_idx);
            (a.min(b) + 1, a.max(b).saturating_sub(1))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_sequence() {
        let sd = SequenceDiagram::new(100, Charset::Unicode)
            .add_participant("Client")
            .add_participant("Server")
            .add_message("Client", "Server", "GET /api");
        let out = sd.build().unwrap();
        assert!(out.contains("Client"));
        assert!(out.contains("Server"));
        assert!(out.contains("GET /api"));
    }

    #[test]
    fn test_long_message_label_wraps() {
        let long_label = "write系统调用写入数据到文件描述符并等待缓冲区刷新完成";
        let sd = SequenceDiagram::new(60, Charset::Ascii)
            .add_participant("用户进程")
            .add_participant("内核VFS")
            .add_message("用户进程", "内核VFS", long_label);
        let out = sd.build().unwrap();
        for ch in long_label.chars() {
            assert!(out.contains(ch), "label char '{ch}' missing:\n{out}");
        }
    }

    #[test]
    fn test_no_participants() {
        assert!(SequenceDiagram::new(80, Charset::Unicode).build().is_err());
    }

    #[test]
    fn test_no_messages() {
        assert!(SequenceDiagram::new(80, Charset::Unicode).add_participant("A").build().is_err());
    }

    #[test]
    fn test_lane_gap_between_adjacent_boxes() {
        // Two participants of equal name length — verify there is a 2-cell
        // gap between the two rendered boxes (count consecutive spaces).
        let sd = SequenceDiagram::new(40, Charset::Unicode)
            .add_participant("A")
            .add_participant("B")
            .add_message("A", "B", "m");
        let out = sd.build().unwrap();
        let first_line = out.lines().next().unwrap();
        // The top row contains both box top borders; between the two `╭`
        // characters there must be at least one space (border end to start),
        // and the middle row has the participant names. Check the row with
        // the *top border* (`╭...─...╮╭...─...╮`) contains a 2-cell gap.
        let chars: Vec<char> = first_line.chars().collect();
        let first_close = chars.iter().position(|&c| c == '╮').expect("first box right border");
        let second_open = chars
            .iter()
            .enumerate()
            .skip(first_close + 1)
            .find(|&(_, &c)| c == '╭')
            .map(|(i, _)| i)
            .expect("second box left border");
        let gap = second_open.saturating_sub(first_close + 1);
        // Box left-border starts one cell into the lane (LANE_GAP_HALF = 1),
        // so the actual boxed border-to-border gap is `LANE_GAP`.
        assert!(gap >= LANE_GAP, "expected ≥ {LANE_GAP}-cell gap, got {gap}: {first_line:?}");
    }
    #[test]
    fn test_arrow_does_not_pierce_lifeline() {
        // Regression: the arrowhead is the corridor cell that visually
        // touches the target lifeline. A incorrect router would either
        // (a) leave a body glyph on top of the lifeline or (b) fail to
        // reach the lifeline at all. Both directions and both charsets
        // are exercised:
        //   * A → B : arrowhead (▶ / >) is the right corridor cell, the
        //              target lifeline must be IMMEDIATELY after it.
        //   * B → A : arrowhead (◀ / <) is the left corridor cell, the
        //              target lifeline must be IMMEDIATELY before it.
        for charset in [Charset::Unicode, Charset::Ascii] {
            let (lifeline, gt_arrow, lt_arrow) = match charset {
                Charset::Unicode => ('│', '▶', '◀'),
                Charset::Ascii => ('|', '>', '<'),
            };

            // A → B: arrow flows rightward; arrowhead sits in the right
            // corridor cell and is followed by the target lifeline.
            let out_a_to_b = SequenceDiagram::new(60, charset)
                .add_participant("A")
                .add_participant("B")
                .add_message("A", "B", "hi")
                .build()
                .unwrap();
            assert_arrow_touches_target_lifeline(
                &out_a_to_b,
                gt_arrow,
                lifeline,
                /*reversed=*/ false,
            );

            // B → A: arrow flows leftward; arrowhead sits in the left
            // corridor cell and is preceded by the target lifeline.
            let out_b_to_a = SequenceDiagram::new(60, charset)
                .add_participant("A")
                .add_participant("B")
                .add_message("B", "A", "hi")
                .build()
                .unwrap();
            assert_arrow_touches_target_lifeline(
                &out_b_to_a,
                lt_arrow,
                lifeline,
                /*reversed=*/ true,
            );
        }
    }

    #[test]
    fn test_self_label_never_covers_lifeline() {
        // Regression: self-message labels used a fixed wrap width
        // unrelated to lane geometry, spilling across neighbouring
        // lifelines (Label layer > Connector layer — the covered `|`
        // vanished). Lanes now widen for the label (within the budget),
        // and labels wrap to their lane region.
        let label = "一个很长很长的自环标签测试用例";
        let out = SequenceDiagram::new(120, Charset::Ascii)
            .add_participant("A1")
            .add_participant("A2")
            .add_participant("A3")
            .add_participant("A4")
            .add_participant("A5")
            .add_message("A1", "A1", label)
            .build()
            .unwrap();
        // Every label row must still show every other lifeline.
        let lifeline_rows: Vec<&str> = out.lines().skip(3).filter(|l| l.contains('|')).collect();
        for row in lifeline_rows {
            if row.contains("自环") || row.contains("标签") {
                let pipes = row.matches('|').count();
                assert!(pipes >= 4, "lifeline covered by label ({pipes} pipes):\n{out}");
            }
        }
        for ch in label.chars() {
            assert!(out.contains(ch), "label char '{ch}' lost:\n{out}");
        }
    }

    #[test]
    fn test_last_lane_self_label_stays_right() {
        // Regression: the last lane's self label used to be clamped
        // against the canvas edge and pushed LEFT across its own
        // lifeline. The lane model now reserves a right margin for it
        // (within the display budget).
        let label = "ibv_post_recv 预挂RQ";
        let out = SequenceDiagram::new(72, Charset::Ascii)
            .label_budget(160)
            .add_participant("App 发送方")
            .add_participant("NIC 发送方")
            .add_participant("NIC 接收方")
            .add_participant("App 接收方")
            .add_message("App 接收方", "App 接收方", label)
            .build()
            .unwrap();
        // With budget for the margin the label fits on one line, right
        // of its own lifeline, and no lifeline is covered.
        let label_row = out.lines().find(|l| l.contains("ibv_post_recv")).unwrap();
        assert_eq!(label_row.matches('|').count(), 4, "lifeline covered:\n{out}");

        // Without budget headroom the label wraps inside its region —
        // tight, but every character survives.
        let tight = SequenceDiagram::new(72, Charset::Ascii)
            .add_participant("App 发送方")
            .add_participant("NIC 发送方")
            .add_participant("NIC 接收方")
            .add_participant("App 接收方")
            .add_message("App 接收方", "App 接收方", label)
            .build()
            .unwrap();
        for ch in label.chars() {
            assert!(tight.contains(ch), "label char '{ch}' lost:\n{tight}");
        }
    }

    #[test]
    fn test_long_participant_name_does_not_cover_neighbor() {
        // Regression: lanes were capped below the structural name width,
        // so a 44-column name overflowed its box onto the neighbour's
        // border. Names are structural: the lane grows to fit.
        let long_name: String = "n".repeat(44);
        let out = SequenceDiagram::new(120, Charset::Ascii)
            .add_participant(&long_name)
            .add_participant("B")
            .add_message(&long_name, "B", "m")
            .build()
            .unwrap();
        assert_eq!(out.matches('n').count(), 44, "name chars lost:\n{out}");
        // The name must stay inside its own box: every row of the header
        // still shows both boxes' borders.
        for (i, line) in out.lines().take(3).enumerate() {
            let trimmed = line.trim_end();
            assert!(
                trimmed.ends_with('+') || trimmed.ends_with('|'),
                "header row {i} right border eaten:\n{out}"
            );
        }
    }

    #[test]
    fn test_label_beyond_budget_wraps_without_loss() {
        // Labels that cannot fit within the display budget wrap inside
        // their lane region — never lost, never covering a lifeline.
        let label: String = "x".repeat(200);
        let out = SequenceDiagram::new(60, Charset::Ascii)
            .label_budget(60)
            .add_participant("A")
            .add_participant("B")
            .add_message("A", "B", &label)
            .build()
            .unwrap();
        assert_eq!(out.matches('x').count(), 200, "label chars lost:\n{out}");
    }

    #[test]
    fn test_last_lane_self_label_never_spills_past_canvas() {
        // Regression (found in review): with a very narrow width the
        // last lane's self-label region can shrink to one column; the
        // wrap minimum (2) would then push a line's last character past
        // the canvas edge, where put_layered silently drops it. The
        // canvas structurally reserves loop columns + wrap minimum for
        // a last-lane self label, so every character survives.
        let label = "ab";
        let out = SequenceDiagram::new(18, Charset::Ascii)
            .add_participant("P1")
            .add_participant("P2")
            .add_message("P2", "P2", label)
            .build()
            .unwrap();
        assert!(out.contains("ab"), "label lost:\n{out}");
    }

    /// Asserts the arrowhead glyph in the first message's arrow row is
    /// exactly adjacent to the target lifeline glyph. The corridor flow
    /// direction flips the perspective: a left-to-right arrow places
    /// the arrowhead in the right corridor cell with the target
    /// lifeline cell next to it on the right; a right-to-left arrow
    /// places the arrowhead cell against the target lifeline on the
    /// left.
    fn assert_arrow_touches_target_lifeline(
        rendered: &str,
        arrow_glyph: char,
        target_lifeline: char,
        reversed: bool,
    ) {
        // Arrow row index = header_height (3) + 1 (gap) + num_lines (1 for "hi")
        // = 5 for the first message (label is 1 line, drawn above the arrow).
        let arrow_row =
            rendered.lines().nth(5).unwrap_or_else(|| panic!("missing row 5: {rendered:?}"));
        assert!(
            arrow_row.contains(arrow_glyph),
            "arrowhead {arrow_glyph:?} missing from {arrow_row:?}",
        );
        let pos = arrow_row.find(arrow_glyph).unwrap();
        if reversed {
            let prev = arrow_row[..pos].chars().last();
            assert_eq!(
                prev,
                Some(target_lifeline),
                "expected {target_lifeline:?} immediately before {arrow_glyph:?}; \
                 row={arrow_row:?}",
            );
        } else {
            let next = arrow_row[pos + arrow_glyph.len_utf8()..].chars().next();
            assert_eq!(
                next,
                Some(target_lifeline),
                "expected {target_lifeline:?} immediately after {arrow_glyph:?}; \
                 row={arrow_row:?}",
            );
        }
    }
}
