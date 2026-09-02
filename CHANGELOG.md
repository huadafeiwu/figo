# Changelog

All notable changes to figo will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.1] — 2026-09-02

A comprehensive label-layout overhaul: no label character is ever silently
dropped or drawn over another element, in any diagram type.

### Fixed

- **CJK display width** — double-width characters advance the cursor by two
  columns everywhere (banner, flowchart, state, sequence, table, tree),
  fixing misaligned borders and lost characters on Chinese text.
- **Label layout system ("three rules")** — structural minimums (names,
  headers, bit widths) size the canvas unconditionally; labels widen their
  geometry up to the display budget; past the budget they wrap greedily and
  are never truncated. Replaces the scattered per-diagram clamping that
  silently dropped characters (table columns, arrow text, box titles,
  gantt name columns, banner fallback for non-ASCII lines).
- **Sequence diagrams** — self-loop labels no longer cover lifelines
  (unified lane model); last-lane self-loops reserve structural space so
  wrapping never spills past the canvas.
- **State diagrams** — Sugiyama crossing reduction and coordinate
  assignment align vertical legs; labels of aligned edges ride their own
  exclusive leg segment below the fork (clear branch attribution instead
  of sitting on the sibling corridor row); same-gap labels stack with
  height awareness; vertical legs reroute around intermediate boxes.
- **Flowcharts** — fork-riding labels never interleave with corridor
  sibling labels; converging corridor labels no longer overwrite each other
  when a long detoured edge lands on a lower fork's row (each shifts to
  the nearest free row).
- **Geometry unification** — one shared geometry model (TransGeom) and
  shared wrap-width helpers between the sizing and drawing passes, closing
  the estimate/draw drifts that caused rows to be under-reserved.

### Changed

- `width` is optional in every subcommand's JSON (defaults to detected
  terminal width); the CLI raises the label budget to
  `max(user width, detected width)`.
- CONTRIBUTING.md documents the label layout rules for future changes.

## [0.2.0] — 2026-07-18

### Changed

- **State Diagram** (`figo state`) — complete rewrite from UML state machine to
  **FSM (Finite State Machine)** style. States are rendered as rounded pills;
  accepting states use a double rounded border. Automatic layered layout replaces
  the manual grid-based positioning. Transitions route from source bottom to
  target top through a gap-based corridor, with direction-aware arrowheads
  (▼ forward, ▲ back).
  - **Breaking:** Removed composite, initial (node type), final, and history
    state types. Use `"simple"` and `"accepting"` instead.
  - **Breaking:** Removed `row`, `col`, and `children` fields from state nodes.
    Layout is now fully automatic.
  - **Breaking:** `StateType::Final` renamed to `StateType::Accepting`.

## [0.1.0] — 2026-07-16

### Added

- **Box Art** (`figo box`) — bordered containers with title, content, word-wrap,
  padding, alignment, and 5 border styles (single, double, rounded, dashed, bold)
- **Table** (`figo table`) — grid/table layouts with headers, rows, configurable
  columns, per-column alignment, padding, and header separators
- **Flowchart** (`figo flowchart`) — rectangular/rounded/diamond nodes with
  auto-layout (Sugiyama-style) or manual positioning, orthogonally-routed edges
- **Packet Header** (`figo packet`) — IETF/RFC-style packet header diagrams with
  32-bit word scale and bordered field cells
- **Tree** (`figo tree`) — hierarchical tree diagrams with Unicode/ASCII branch
  characters and arbitrary nesting depth
- **Arrow** (`figo arrow`) — standalone arrows/connectors (horizontal, vertical,
  bidirectional) with configurable line styles and labels
- **Sequence Diagram** (`figo sequence`) — timeline-based message sequence
  diagrams with participant lanes, message arrows, and self-messages
- **Banner** (`figo banner`) — FIGlet text banners using the bundled "standard" font
- **Gantt Chart** (`figo gantt`) — project management Gantt charts with sections,
  tasks, progress bars, milestones, dependencies, and today markers
- **State Diagram** (`figo state`) — UML state machine diagrams with simple,
  composite, initial, final, and choice states, plus labeled transitions
- ASCII and Unicode character set support for all diagram types
- ANSI color support (opt-in via `--color` / `color: true`)
- CLI with inline JSON, `--file`, and stdin input; `--output`, `--clipboard` output
- Public library API with free functions and builder patterns for all diagram types
- 2D canvas rendering engine with word-wrapping and text alignment utilities

### Dependencies

- `clap` 4.5 for CLI argument parsing
- `serde` / `serde_json` for JSON deserialization
- `arboard` 3.4 for clipboard access
- `thiserror` 2.0 for error handling
- `unicode-width` 0.2 for accurate text measurement
- `insta` 1.42 (dev) for snapshot testing
