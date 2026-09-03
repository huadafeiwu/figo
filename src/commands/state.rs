//! Command handler for FSM state diagrams.

use super::{resolve_width, terminal_width};
use figo::diagrams::state::{StateDiagram, StateNode, StateType};
use figo::error::Result;
use serde::Deserialize;

#[derive(Deserialize)]
struct StateInput {
    #[serde(default)]
    width: usize,
    charset: super::JsonCharset,
    states: Vec<StateJson>,
    #[serde(default)]
    initial: Option<String>,
    #[serde(default)]
    transitions: Vec<TransitionJson>,
    #[serde(default)]
    color: bool,
}

#[derive(Deserialize)]
struct StateJson {
    id: String,
    label: String,
    #[serde(default = "default_state_type")]
    #[serde(rename = "type")]
    stype: String,
}

fn default_state_type() -> String {
    "simple".into()
}

#[derive(Deserialize)]
struct TransitionJson {
    from: String,
    to: String,
    #[serde(default)]
    label: Option<String>,
}

pub fn run_state(input: &str) -> Result<String> {
    let inp: StateInput = serde_json::from_str(input)?;
    let initial_state = inp.initial.clone();
    let charset = inp.charset;

    let mut states = Vec::with_capacity(inp.states.len());
    for s in inp.states {
        let stype = StateType::parse(&s.id, &s.stype)?;
        states.push(StateNode { id: s.id, label: s.label, state_type: stype });
    }

    let width = resolve_width(inp.width);
    // Display budget: labels may grow the canvas beyond the user width
    // (labels prefer one line) but never what the display can show.
    let mut sd = StateDiagram::new(width, charset.into())
        .label_budget(width.max(terminal_width()))
        .color(inp.color);
    for s in states {
        sd = sd.add_state(s);
    }
    if let Some(ref init_id) = initial_state {
        sd = sd.initial(init_id);
    }
    for t in &inp.transitions {
        sd = sd.add_transition(&t.from, &t.to, t.label.as_deref());
    }
    sd.build()
}
