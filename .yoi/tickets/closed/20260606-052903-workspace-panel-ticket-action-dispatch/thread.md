<!-- event: create author: yoi ticket at: 2026-06-06T05:29:03Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: hare at: 2026-06-06T05:29:48Z -->

## Plan

Created this follow-up because the first panel slices now provide Ticket/action rows, Orchestrator lifecycle, Intake launch, and Intake handoff, but Ticket row actions are still mostly display affordances.

Before layout/display tuning, the panel should support a minimal safe action dispatch path for the human decision points it already displays, especially Go/Defer. The implementation should re-check Ticket authority before mutation, use Rust Ticket APIs, and notify Orchestrator for Go/routing actions when feasible.


---

<!-- event: plan author: hare at: 2026-06-06T05:30:26Z -->

## Plan

Preflight result: `implementation-ready` as the final first-pass panel slice before layout/display tuning.

The first panel slices now provide display, Orchestrator lifecycle, Intake launch, and handoff. This ticket should replace blanket display-only Ticket actions with minimal safe dispatch, especially Go and Defer, while keeping Review/Close safe if a full inline flow is not yet available.

Detailed delegation intent is recorded in `artifacts/delegation-intent.md`.


---

<!-- event: review author: hare at: 2026-06-06T06:04:40Z status: approve -->

## Review: approve

External reviewer approved current HEAD after requested changes were addressed.

Review summary:
- Ticket action dispatch now re-checks Ticket config availability before backend mutation, matching panel display gating.
- Stale/no-config action dispatch is rejected before mutation and covered by tests.
- Go records a decision and attempts Orchestrator notification without starting implementation.
- Defer safely records a decision and moves open Tickets to pending.
- Review and Close remain safe diagnostics rather than silent approval/closure.
- No-Ticket workspaces remain Pod-centric.
- Existing Pod open/direct-send and composer Intake behavior remain covered.


---

<!-- event: close author: hare at: 2026-06-06T06:04:40Z status: closed -->

## Closed

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


---
