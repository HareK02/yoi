Implemented, reviewed, merged, and validated.

Summary:
- Added panel/app state to retain the latest actionable workspace Orchestrator lifecycle failure.
- Subsequent `Observe` refreshes that only report plain `missing` / `stopped` keep the prior actionable diagnostic visible.
- Stale diagnostics are cleared when Orchestrator becomes `live`, `spawned`, or `restored`.
- New `Unavailable` lifecycle diagnostics supersede older failures.
- Orchestrator scope policy was unchanged.
- `Observe` refresh still does not retry spawn.
- Ticket actionbar gate messages remain separate from Orchestrator lifecycle diagnostics.

Implementation:
- Child commit: `e15e9b7 fix: persist orchestrator lifecycle diagnostics`
- Merge commit: `merge: orchestrator diagnostic persistence`

Review:
- External reviewer `orchestrator-diagnostic-reviewer-20260607` approved with no blockers.

Validation after merge:
- `cargo test -p tui multi_orchestrator --lib`
- `cargo test -p tui --lib`
- `cargo fmt --check`
- `git diff --check`
- `target/debug/yoi ticket doctor`
