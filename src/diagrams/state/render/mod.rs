//! FSM state diagram renderer.
//!
//! Renders states as rounded pills with centered labels. Accepting states
//! get a double rounded border. Transitions are routed orthogonally with
//! arrowheads and optional labels.
//!
//! The rendering stages live in sibling modules so each file stays under
//! the project's 250-line cap: `sizing` (canvas extents), `label_rows`
//! (row assignment for overlapping labels), `gap_expansion` (vertical gap
//! growth), `states` (state boxes and the initial arrow), `transition`
//! (routing orchestration), `same_layer` (horizontal arrows), `label`
//! (label placement), `avoidance` (collision avoidance helpers), and
//! `self_loop`.

mod avoidance;
mod gap_expansion;
mod label;
mod label_rows;
mod same_layer;
mod self_loop;
mod sizing;
mod states;
mod transition;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::fmt;

use crate::canvas::Canvas;
use crate::diagrams::state::layout::{LayoutParams, StateLayout, layout_states};
use crate::diagrams::state::sugiyama;
use crate::diagrams::state::types::{StateNode, Transition};
use crate::error::{FigoError, Result};
use crate::render::surface::Surface;
use crate::render::widget::{PaintContext, Rect};
use crate::style::{Charset, LineStyle};
use crate::text::wrap_label;

use gap_expansion::{apply_gap_expansion, compute_gap_expansion};
use label_rows::compute_label_rows;
use sizing::{compute_canvas_height, compute_canvas_width, self_loop_label_height};
use states::{draw_initial_arrow, draw_states};
use transition::{DrawStage, draw_transitions};

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

        // --- Single source of truth: compute all transition geometry once,
        // BEFORE the vertical gap expansion. Every TransGeom field derives
        // from x coordinates or RELATIVE y positions, which the expansion
        // (a uniform downward shift per layer) cannot change — and the
        // expansion needs these same avail values for its line-count
        // estimates, so both sides read one source.
        // layout_states returns declaration order, which
        // compute_trans_geoms indexes against.
        let id_to_idx: HashMap<&str, usize> =
            self.states.iter().enumerate().map(|(i, s)| (s.id.as_str(), i)).collect();
        let trans_geoms =
            sugiyama::compute_trans_geoms(&layouts, &self.transitions, &id_to_idx, self.width);

        let label_rows = compute_label_rows(&self.transitions, &layouts);

        // Compute per-gap max row and expand gaps accordingly.
        let gap_extra =
            compute_gap_expansion(&self.transitions, &layouts, &label_rows, &trans_geoms);
        apply_gap_expansion(&mut layouts, &gap_extra, &params);

        // Shift states down for top margin (room for labels above topmost state).
        let max_label_row = label_rows.values().copied().max().unwrap_or(0);
        if max_label_row > 0 {
            label_rows::shift_layouts(&mut layouts, max_label_row + 1);
        }

        let id_to_layout = build_id_map(&layouts);

        // apply_gap_expansion sorts by y; restore declaration order so
        // layouts[i] == states[i]. The geoms above are unaffected.
        layouts.sort_by_key(|l| id_to_idx[l.id.as_str()]);

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
            let lw = wrap_label(text, avail).max_width;
            let lx = base_x.saturating_sub(lw / 2);
            total_w = total_w.max(lx + lw);
        }

        let total_h = compute_canvas_height(&layouts, &params, max_label_row)
            + self_loop_label_height(&self.transitions, &layouts, total_w);

        let mut canvas = Canvas::new(total_w, total_h);

        {
            let ctx = PaintContext { charset: self.charset, color: self.color };
            let mut surface = Surface::new(&mut canvas);

            draw_initial_arrow(&mut surface, &id_to_layout, self.initial, self.charset);
            draw_states(&mut surface, &layouts, &ctx)?;
            draw_transitions(&mut DrawStage {
                surface: &mut surface,
                ctx: &ctx,
                canvas_width: total_w,
                transitions: &self.transitions,
                layouts: &layouts,
                trans_geoms: &trans_geoms,
                label_rows: &label_rows,
                id_to_layout: &id_to_layout,
            });
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
