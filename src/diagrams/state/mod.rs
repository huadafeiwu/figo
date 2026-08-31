//! FSM state diagrams with automatic layered layout.

pub mod layout;
pub mod render;
pub mod sugiyama;
pub mod types;

pub use render::StateDiagram;
pub use types::{StateNode, StateType, Transition};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Charset;

    #[test]
    fn test_simple_fsm() {
        let sd = StateDiagram::new(80, Charset::Unicode)
            .add_state(StateNode {
                id: "idle".into(),
                label: "Idle".into(),
                state_type: StateType::Simple,
            })
            .add_state(StateNode {
                id: "done".into(),
                label: "Done".into(),
                state_type: StateType::Accepting,
            })
            .initial("idle")
            .add_transition("idle", "done", Some("finish"));
        let out = sd.build().unwrap();
        assert!(out.contains("Idle"));
        assert!(out.contains("Done"));
        assert!(out.contains("finish"));
        // Accepting state should have double border (two sets of rounded corners).
        assert!(
            out.matches('╭').count() >= 2,
            "accepting state needs outer and inner top-left corners"
        );
    }

    #[test]
    fn test_serial_fsm_unicode() {
        let out = StateDiagram::new(80, Charset::Unicode)
            .add_state(StateNode {
                id: "idle".into(),
                label: "Idle".into(),
                state_type: StateType::Simple,
            })
            .add_state(StateNode {
                id: "running".into(),
                label: "Running".into(),
                state_type: StateType::Simple,
            })
            .add_state(StateNode {
                id: "done".into(),
                label: "Done".into(),
                state_type: StateType::Accepting,
            })
            .initial("idle")
            .add_transition("idle", "running", Some("start"))
            .add_transition("running", "done", Some("finish"))
            .build()
            .unwrap();
        insta::assert_snapshot!(out);
    }

    #[test]
    fn test_serial_fsm_ascii() {
        let out = StateDiagram::new(80, Charset::Ascii)
            .add_state(StateNode {
                id: "idle".into(),
                label: "Idle".into(),
                state_type: StateType::Simple,
            })
            .add_state(StateNode {
                id: "running".into(),
                label: "Running".into(),
                state_type: StateType::Simple,
            })
            .add_state(StateNode {
                id: "done".into(),
                label: "Done".into(),
                state_type: StateType::Accepting,
            })
            .initial("idle")
            .add_transition("idle", "running", Some("start"))
            .add_transition("running", "done", Some("finish"))
            .build()
            .unwrap();
        insta::assert_snapshot!(out);
    }

    #[test]
    fn test_self_loop_fsm_unicode() {
        let out = StateDiagram::new(40, Charset::Unicode)
            .add_state(StateNode {
                id: "idle".into(),
                label: "Idle".into(),
                state_type: StateType::Simple,
            })
            .initial("idle")
            .add_transition("idle", "idle", Some("tick"))
            .build()
            .unwrap();
        insta::assert_snapshot!(out);
    }

    #[test]
    fn test_two_independent_states_unicode() {
        let out = StateDiagram::new(80, Charset::Unicode)
            .add_state(StateNode {
                id: "a".into(),
                label: "A".into(),
                state_type: StateType::Simple,
            })
            .add_state(StateNode {
                id: "b".into(),
                label: "B".into(),
                state_type: StateType::Accepting,
            })
            .initial("a")
            .build()
            .unwrap();
        insta::assert_snapshot!(out);
    }

    #[test]
    fn test_fork_fsm_unicode() {
        // One state transitions to two independent states — they
        // should appear in the same layer, side by side.
        let out = StateDiagram::new(80, Charset::Unicode)
            .add_state(StateNode {
                id: "idle".into(),
                label: "Idle".into(),
                state_type: StateType::Simple,
            })
            .add_state(StateNode {
                id: "a".into(),
                label: "A".into(),
                state_type: StateType::Simple,
            })
            .add_state(StateNode {
                id: "b".into(),
                label: "B".into(),
                state_type: StateType::Accepting,
            })
            .initial("idle")
            .add_transition("idle", "a", Some("go_a"))
            .add_transition("idle", "b", Some("go_b"))
            .build()
            .unwrap();
        insta::assert_snapshot!(out);
    }

    #[test]
    fn test_overlapping_labels_unicode() {
        // Transitions with long labels must be placed on different
        // rows to avoid collisions.
        let out = StateDiagram::new(120, Charset::Unicode)
            .add_state(StateNode {
                id: "a".into(),
                label: "A".into(),
                state_type: StateType::Simple,
            })
            .add_state(StateNode {
                id: "b".into(),
                label: "B".into(),
                state_type: StateType::Simple,
            })
            .add_state(StateNode {
                id: "c".into(),
                label: "C".into(),
                state_type: StateType::Simple,
            })
            .add_state(StateNode {
                id: "d".into(),
                label: "D".into(),
                state_type: StateType::Accepting,
            })
            .initial("a")
            .add_transition("a", "b", Some("very_long_label_one"))
            .add_transition("b", "c", Some("very_long_label_two"))
            .add_transition("c", "d", Some("very_long_label_three"))
            .build()
            .unwrap();
        insta::assert_snapshot!(out);
    }

    #[test]
    fn test_empty_states_is_error() {
        let result = StateDiagram::new(80, Charset::Unicode).build();
        assert!(result.is_err());
    }

    #[test]
    fn test_accepting_state_has_double_border() {
        let out = StateDiagram::new(40, Charset::Unicode)
            .add_state(StateNode {
                id: "done".into(),
                label: "Done".into(),
                state_type: StateType::Accepting,
            })
            .initial("done")
            .build()
            .unwrap();
        // Accepting state must have both outer and inner borders.
        assert!(out.contains("Done"));
        // There should be at least 2 top-left rounded corners (outer + inner).
        let tl_count = out.chars().filter(|&c| c == '╭').count();
        assert!(
            tl_count >= 2,
            "accepting state needs double border, got {tl_count} top-left corners"
        );
    }

    #[test]
    fn test_long_transition_label_wraps() {
        let long_label = "初始化完成且所有校验检查均通过后进入正常运行状态";
        let out = StateDiagram::new(60, Charset::Unicode)
            .add_state(StateNode {
                id: "init".into(),
                label: "初始化".into(),
                state_type: StateType::Simple,
            })
            .add_state(StateNode {
                id: "running".into(),
                label: "运行中".into(),
                state_type: StateType::Simple,
            })
            .initial("init")
            .add_transition("init", "running", Some(long_label))
            .build()
            .unwrap();
        for ch in long_label.chars() {
            assert!(out.contains(ch), "label char '{ch}' missing:\n{out}");
        }
    }

    #[test]
    fn test_trans_geom_matches_layout() {
        // Multi-layer state graph similar to Netfilter structure:
        // fork + cross-layer + independent path. This exercises the
        // layout_states → apply_gap_expansion (sorts by y) →
        // compute_trans_geoms (indexes by declaration order) chain.
        // If layouts are not re-sorted to declaration order before
        // compute_trans_geoms, the geom cx values will be wrong.
        use crate::diagrams::state::layout::{LayoutParams, layout_states};
        use crate::diagrams::state::sugiyama;
        use std::collections::HashMap;

        let states = vec![
            StateNode {
                id: "pre".into(),
                label: "PRE_ROUTING".into(),
                state_type: StateType::Simple,
            },
            StateNode {
                id: "local".into(),
                label: "LOCAL_IN".into(),
                state_type: StateType::Simple,
            },
            StateNode { id: "fwd".into(), label: "FORWARD".into(), state_type: StateType::Simple },
            StateNode {
                id: "post".into(),
                label: "POST_ROUTING".into(),
                state_type: StateType::Simple,
            },
            StateNode {
                id: "out".into(),
                label: "LOCAL_OUT".into(),
                state_type: StateType::Simple,
            },
            StateNode {
                id: "post2".into(),
                label: "POST_ROUTING_OUT".into(),
                state_type: StateType::Simple,
            },
        ];
        let transitions = vec![
            Transition {
                from: "pre".into(),
                to: "local".into(),
                label: Some("local_delivery".into()),
            },
            Transition { from: "pre".into(), to: "fwd".into(), label: Some("forward".into()) },
            Transition {
                from: "fwd".into(),
                to: "post".into(),
                label: Some("post_forward".into()),
            },
            Transition {
                from: "out".into(),
                to: "post2".into(),
                label: Some("output_post".into()),
            },
        ];
        let params = LayoutParams::default();
        let layouts = layout_states(&states, &transitions, Some("pre"), 120, &params);

        // Build id_to_idx matching self.states declaration order.
        let id_to_idx: HashMap<&str, usize> =
            states.iter().enumerate().map(|(i, s)| (s.id.as_str(), i)).collect();

        let geoms = sugiyama::compute_trans_geoms(&layouts, &transitions, &id_to_idx, 120);

        // Verify each transition's geom cx matches the actual layout rect.
        for (i, t) in transitions.iter().enumerate() {
            let from_i = id_to_idx[t.from.as_str()];
            let to_i = id_to_idx[t.to.as_str()];
            let expected_from_cx = layouts[from_i].rect.x + layouts[from_i].rect.w / 2;
            let expected_to_cx = layouts[to_i].rect.x + layouts[to_i].rect.w / 2;
            assert_eq!(
                geoms[i].from_cx, expected_from_cx,
                "transition {}→{}: geom.from_cx={} but layout says cx={}",
                t.from, t.to, geoms[i].from_cx, expected_from_cx
            );
            assert_eq!(
                geoms[i].to_cx, expected_to_cx,
                "transition {}→{}: geom.to_cx={} but layout says cx={}",
                t.from, t.to, geoms[i].to_cx, expected_to_cx
            );
        }
    }

    #[test]
    fn test_label_not_unnecessarily_wrapped() {
        // Verify that labels are not wrapped when the corridor is wide
        // enough. This catches bugs where compute_column_gaps fails to
        // widen the corridor (e.g., due to x-vs-cx mismatch or canvas cap).
        use crate::diagrams::state::layout::{LayoutParams, layout_states};
        use crate::diagrams::state::sugiyama;
        use std::collections::HashMap;
        use unicode_width::UnicodeWidthStr;

        let states = vec![
            StateNode {
                id: "pre".into(),
                label: "PRE_ROUTING".into(),
                state_type: StateType::Simple,
            },
            StateNode {
                id: "local".into(),
                label: "LOCAL_IN".into(),
                state_type: StateType::Simple,
            },
            StateNode { id: "fwd".into(), label: "FORWARD".into(), state_type: StateType::Simple },
            StateNode {
                id: "post".into(),
                label: "POST_ROUTING".into(),
                state_type: StateType::Simple,
            },
            StateNode {
                id: "out".into(),
                label: "LOCAL_OUT".into(),
                state_type: StateType::Simple,
            },
            StateNode {
                id: "post2".into(),
                label: "POST_ROUTING_OUT".into(),
                state_type: StateType::Simple,
            },
        ];
        let transitions = vec![
            Transition {
                from: "pre".into(),
                to: "local".into(),
                label: Some("local_delivery".into()),
            },
            Transition { from: "pre".into(), to: "fwd".into(), label: Some("forward".into()) },
            Transition {
                from: "fwd".into(),
                to: "post".into(),
                label: Some("post_forward".into()),
            },
            Transition {
                from: "out".into(),
                to: "post2".into(),
                label: Some("output_post".into()),
            },
        ];
        let params = LayoutParams::default();
        let layouts = layout_states(&states, &transitions, Some("pre"), 120, &params);

        let id_to_idx: HashMap<&str, usize> =
            states.iter().enumerate().map(|(i, s)| (s.id.as_str(), i)).collect();

        let geoms = sugiyama::compute_trans_geoms(&layouts, &transitions, &id_to_idx, 120);

        for (i, t) in transitions.iter().enumerate() {
            let Some(label) = &t.label else { continue };
            let label_w = UnicodeWidthStr::width(label.as_str());
            let geom = &geoms[i];

            // Build the full output to check if the label appears unsplit.
            let out = StateDiagram::new(120, Charset::Ascii)
                .add_state(states[0].clone())
                .add_state(states[1].clone())
                .add_state(states[2].clone())
                .add_state(states[3].clone())
                .add_state(states[4].clone())
                .add_state(states[5].clone())
                .initial("pre")
                .add_transition("pre", "local", Some(label))
                .build()
                .unwrap();

            // The label should appear as a single unbroken line in the output.
            // If it's wrapped, the label text won't appear as a contiguous string.
            assert!(
                out.contains(label.as_str()),
                "label '{}' should appear unsplit in output but was wrapped.\n\
                 geom: corridor_w={}, embed={}, avail={}, label_w={}\n{}",
                label,
                geom.corridor_w,
                geom.embed,
                geom.avail,
                label_w,
                out
            );
        }
    }

    #[test]
    fn test_stacked_upward_labels_not_lost() {
        // Two upward (bottom → top) transitions whose long labels overlap
        // horizontally get stacked on different rows (row 0 / row 1).
        // Stacked labels anchor on the `to` leg (TransGeom::stacked_base_x);
        // the canvas must be sized with that same anchor, otherwise the
        // label is clamped or shifted at draw time and can lose characters.
        let out = StateDiagram::new(120, Charset::Unicode)
            .add_state(StateNode {
                id: "top".into(),
                label: "TOP".into(),
                state_type: StateType::Simple,
            })
            .add_state(StateNode {
                id: "left".into(),
                label: "LEFT".into(),
                state_type: StateType::Simple,
            })
            .add_state(StateNode {
                id: "right".into(),
                label: "RIGHT".into(),
                state_type: StateType::Simple,
            })
            .initial("left")
            .add_transition("left", "top", Some("back_label_alpha_return"))
            .add_transition("right", "top", Some("back_label_beta_return"))
            .build()
            .unwrap();
        assert!(out.contains("back_label_alpha_return"), "full label lost or wrapped:\n{out}");
        assert!(out.contains("back_label_beta_return"), "full label lost or wrapped:\n{out}");
    }

    #[test]
    fn test_aligned_label_avoids_sibling_junction() {
        // The left branch is an aligned edge (same column) whose label
        // used to sit centered on the shared leg column at the fork,
        // where it either covered the right branch's corridor junction
        // (Label layer > Connector layer, the `+` was dropped) or abutted
        // it so closely the label's branch attribution was ambiguous.
        // The label now rides the exclusive leg segment below the fork,
        // keeping the corridor row clean.
        let out = StateDiagram::new(100, Charset::Ascii)
            .add_state(StateNode {
                id: "a".into(),
                label: "TOP".into(),
                state_type: StateType::Simple,
            })
            .add_state(StateNode {
                id: "b".into(),
                label: "LEFT".into(),
                state_type: StateType::Simple,
            })
            .add_state(StateNode {
                id: "c".into(),
                label: "RIGHT".into(),
                state_type: StateType::Simple,
            })
            .initial("a")
            .add_transition("a", "b", Some("left_branch_label"))
            .add_transition("a", "c", Some("right_branch_label"))
            .build()
            .unwrap();
        // The corridor row (the long `---` run) carries both junctions.
        let corridor_row =
            out.lines().max_by_key(|l| l.matches('-').count()).expect("corridor row with dashes");
        assert!(
            corridor_row.matches('+').count() >= 2,
            "both corridor junctions must render on the corridor row:\n{corridor_row}"
        );
        // The riding label sits on its own leg below the fork, clear of
        // the corridor row and both junction columns.
        let label_row =
            out.lines().find(|l| l.contains("left_branch_label")).expect("label missing");
        assert!(
            !label_row.contains('+') && label_row != corridor_row,
            "riding label must stay clear of the fork:\n{label_row}"
        );
        insta::assert_snapshot!(out);
    }

    #[test]
    fn test_riding_label_rides_leg_with_rails() {
        // The riding convention: an aligned edge's label centers on its
        // leg column below the fork with one `|` rail above and below
        // the block (`| label |`), mirroring the corridor embed's
        // `---label---` padding — and the fork junction stays a T, not a
        // corner (the leg continues behind the label).
        let out = StateDiagram::new(80, Charset::Unicode)
            .add_state(StateNode {
                id: "idle".into(),
                label: "Idle".into(),
                state_type: StateType::Simple,
            })
            .add_state(StateNode {
                id: "a".into(),
                label: "A".into(),
                state_type: StateType::Simple,
            })
            .add_state(StateNode {
                id: "b".into(),
                label: "B".into(),
                state_type: StateType::Simple,
            })
            .initial("idle")
            .add_transition("idle", "a", Some("go_a"))
            .add_transition("idle", "b", Some("go_b"))
            .build()
            .unwrap();
        let lines: Vec<&str> = out.lines().collect();
        let label_idx =
            lines.iter().position(|l| l.contains("go_a")).expect("riding label missing");
        // Rails above and below the riding block on its leg column.
        let above = lines[label_idx - 1];
        let below = lines[label_idx + 1];
        assert!(above.contains('│'), "no `|` rail above the riding label:\n{above}");
        assert!(below.contains('│'), "no `|` rail below the riding label:\n{below}");
        // The fork junction is a T (leg continues through the label),
        // never a corner glyph.
        let junction_row = lines[label_idx - 2];
        assert!(
            junction_row.contains('├') || junction_row.contains('┼') || junction_row.contains('+'),
            "fork junction must stay a T/cross, not a corner:\n{junction_row}"
        );
        insta::assert_snapshot!(out);
    }

    #[test]
    fn test_riding_label_wraps_clear_of_sibling_leg() {
        // Wrap ladder: when the riding label is wider than the measured
        // distance to the sibling's descending leg allows, it wraps to
        // that width (fewest lines) instead of overlapping the leg —
        // never loses characters.
        let long_label = "a_very_long_riding_branch_label_here";
        let out = StateDiagram::new(60, Charset::Ascii)
            .add_state(StateNode {
                id: "top".into(),
                label: "TOP".into(),
                state_type: StateType::Simple,
            })
            .add_state(StateNode {
                id: "mid".into(),
                label: "MID".into(),
                state_type: StateType::Simple,
            })
            .add_state(StateNode {
                id: "side".into(),
                label: "SIDE".into(),
                state_type: StateType::Simple,
            })
            .initial("top")
            .add_transition("top", "mid", Some(long_label))
            .add_transition("top", "side", Some("sib"))
            .build()
            .unwrap();
        // No characters lost: every char of the label appears somewhere
        // (the wrap ladder may reflow, never drop).
        for ch in long_label.chars() {
            assert!(out.contains(ch), "label char '{ch}' missing:\n{out}");
        }
        // The sibling corridor label stays intact too.
        assert!(out.contains("sib"), "sibling corridor label missing:\n{out}");
        insta::assert_snapshot!(out);
    }

    #[test]
    fn test_upward_riding_label() {
        // Upward aligned edge (back-edge) with a corridor sibling in the
        // same gap: the riding label centers on the leg segment ABOVE
        // the fork (toward its target box).
        let out = StateDiagram::new(80, Charset::Ascii)
            .add_state(StateNode {
                id: "top".into(),
                label: "TOP".into(),
                state_type: StateType::Simple,
            })
            .add_state(StateNode {
                id: "mid".into(),
                label: "MID".into(),
                state_type: StateType::Simple,
            })
            .add_state(StateNode {
                id: "side".into(),
                label: "SIDE".into(),
                state_type: StateType::Simple,
            })
            .initial("top")
            .add_transition("top", "mid", None)
            .add_transition("top", "side", Some("branch"))
            .add_transition("mid", "top", Some("back_edge"))
            .build()
            .unwrap();
        assert!(out.contains("back_edge"), "upward riding label missing:\n{out}");
        // The label must not sit on the corridor row (the long `---` run).
        let corridor_row =
            out.lines().max_by_key(|l| l.matches('-').count()).expect("corridor row");
        assert!(
            !corridor_row.contains("back_edge"),
            "upward riding label must not sit on the corridor row:\n{corridor_row}"
        );
        insta::assert_snapshot!(out);
    }

    #[test]
    fn test_gap_riders_stack_instead_of_overlapping() {
        // Two aligned edges (A→B, E→D) in one gap, both labeled, whose
        // riding blocks overlap horizontally: without mutual avoidance
        // the later-drawn block overwrites the earlier one's characters
        // in the overlap zone (exe-verified: 8 of 24 L's lost, and the
        // survivor flips with declaration order). The riders must stack
        // on successive rows instead.
        let out = StateDiagram::new(80, Charset::Ascii)
            .add_state(StateNode {
                id: "a".into(),
                label: "A".into(),
                state_type: StateType::Simple,
            })
            .add_state(StateNode {
                id: "e".into(),
                label: "E".into(),
                state_type: StateType::Simple,
            })
            .add_state(StateNode {
                id: "b".into(),
                label: "B".into(),
                state_type: StateType::Simple,
            })
            .add_state(StateNode {
                id: "d".into(),
                label: "D".into(),
                state_type: StateType::Simple,
            })
            .add_transition("a", "b", Some("LLLL LLLL LLLL LLLL LLLL LLLL"))
            .add_transition("e", "d", Some("RRRR RRRR RRRR RRRR RRRR RRRR"))
            .add_transition("a", "d", None)
            .build()
            .unwrap();
        assert_eq!(out.matches('L').count(), 24, "first rider lost characters:\n{out}");
        assert_eq!(out.matches('R').count(), 24, "second rider lost characters:\n{out}");
        for line in out.lines() {
            assert!(
                !(line.contains('L') && line.contains('R')),
                "riding labels must not share a row:\n{line}\n{out}"
            );
        }
        insta::assert_snapshot!(out);
    }
}
