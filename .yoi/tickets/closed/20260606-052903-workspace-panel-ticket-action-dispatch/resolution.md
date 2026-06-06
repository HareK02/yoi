Implemented workspace panel Ticket action dispatch.

Changes:
- Added dispatch for selected Ticket action rows with current authority re-checks.
- Mutation is gated by the same Ticket config availability logic used by panel display; absent/unusable config rejects before backend load/mutation.
- `Go` records a `decision` event authorizing Orchestrator routing/preflight, explicitly without starting implementation, and attempts to notify the workspace Orchestrator when reachable.
- `Defer` records a `decision` event and moves open Tickets to pending where applicable.
- `Review` does not silently approve/request changes; it emits a diagnostic requiring the explicit review flow.
- `Close` does not close without a resolution; it emits a diagnostic requiring an explicit resolution path.
- No-Ticket workspaces expose no Ticket action dispatch and stay Pod-centric.
- Existing selected-Pod open/direct-send and composer Intake paths are preserved.
- No scheduler/queue/automatic coder/reviewer spawning was introduced.
- `--multi` was not reintroduced.

Validation after merge:
- `cargo test -p tui workspace_panel`
- `cargo test -p tui multi_pod`
- `cargo test -p ticket`
- `cargo test -p yoi panel`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check HEAD~1..HEAD`
- `cargo build -p yoi`
- `target/debug/yoi ticket doctor`
- `nix build .#yoi --no-link --print-out-paths`

External review approved after one requested-changes cycle.

Known follow-up:
- Final layout/display tuning remains the next pass.
- Close/review could later gain richer inline workflows, but the first pass is intentionally safe and non-destructive.
