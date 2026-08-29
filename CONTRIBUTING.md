# Contributing to figo

Thank you for your interest in contributing! This document outlines the
process for contributing to figo.

## Code of Conduct

This project adheres to the [Contributor Covenant Code of Conduct](CODE_OF_CONDUCT.md).
By participating, you are expected to uphold this code. Please report
unacceptable behavior to the project maintainers.

## Getting Started

1. Fork the repository on GitHub.
2. Clone your fork:
   ```sh
   git clone https://github.com/<your-username>/figo.git
   cd figo
   ```
3. Ensure tests pass on your machine:
   ```sh
   cargo test
   cargo clippy --all-targets
   cargo fmt --check
   ```

## Development Workflow

1. Create a branch for your changes:
   ```sh
   git checkout -b my-feature
   ```
2. Make your changes, following the [coding conventions](AGENTS.md).
3. Add tests for new functionality.
4. Run the full verification suite:
   ```sh
   cargo clippy --all-targets
   cargo fmt --check
   cargo test
   ```
5. Commit your changes with a clear, descriptive message.
6. Push your branch and open a pull request against `main`.

## Coding Conventions

See [AGENTS.md](AGENTS.md) for the full coding conventions. Key points:

- **MSRV**: Rust 1.85, Edition 2024
- **No unsafe code or panics** in production code
- **All public items must have doc comments**
- **Max 250 lines per file** (tests excluded)
- **Use imports** — no fully qualified paths inline
- **Run clippy, fmt, and tests** before committing

## Label Layout Rules

Every diagram type follows the same three rules when placing labels.
Derive all bounds from geometry, content, or the display budget — never
from magic numbers:

1. **Structural minimums always win.** Content that cannot wrap
   (participant names, table headers, packet field bit widths) sizes
   the canvas unconditionally; the canvas grows to fit it.
2. **Labels widen their geometry up to the display budget.** A label's
   carrier (corridor, lane, column, box) widens so the label fits on
   one line. The budget is `max(user width, detected terminal width)`;
   library builders default it to the design width so renders stay
   deterministic, and the CLI raises it to the detected width.
3. **Past the budget, wrap greedily.** Labels that cannot fit wrap
   inside their region using the full region width (fewest lines), and
   are never truncated or drawn over other elements — a label region is
   always the free cells between its flanking obstacles (lifelines,
   borders, other structures).

Regression tests must cover the loss cases: no label character may
ever be silently dropped, and no label may cover another element's
glyphs.

## Pull Request Process

1. Update documentation if your changes affect the public API.
2. Ensure all CI checks pass (build, test, clippy, format).
3. A maintainer will review your PR. Please be responsive to feedback.
4. Once approved, a maintainer will merge your changes.

## Reporting Bugs

Please open an issue on GitHub with:

- A clear, descriptive title
- Steps to reproduce the bug
- Expected vs actual behavior
- Rust version (`rustc --version`) and OS details

## Feature Requests

Feature requests are welcome! Please open an issue describing:

- The problem you'd like to solve
- Your proposed solution (if you have one)
- Any relevant use cases or examples

Please keep in mind the project's [design principles](figo-spec.md#11-design-principles):
KISS and YAGNI — build only what is needed.

## License

By contributing to figo, you agree that your contributions will be
dual-licensed under [Apache 2.0](LICENSE-APACHE-2.0) and [MIT](LICENSE-MIT).
