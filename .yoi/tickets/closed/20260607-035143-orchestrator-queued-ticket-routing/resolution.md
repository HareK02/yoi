Implemented, reviewed, merged, and validated.

Summary:
- Updated Panel Queue notice so `ready -> queued` reports that Orchestrator routing is authorized and implementation side effects still require `queued -> inprogress` acceptance.
- Updated Orchestrator notification content to include Ticket slug/id/title and instruct the Orchestrator to read the Ticket, inspect workspace state, accept with `queued -> inprogress` before worktree/SpawnPod side effects when unblocked, and record a concise blocked/defer reason when blocked.
- Updated `.yoi/workflow/ticket-orchestrator-routing.md` so queued notifications are routing authorization, not passive no-op and not blind spawn permission.
- Kept worktree/coder/reviewer routing, merge completion, and orchestration plan tooling out of this ticket.
- Added focused tests for Queue notice and Orchestrator notification contract.

Implementation:
- Child commit: `ccf43f8 tui: update queued ticket routing notification`
- Parent workflow/report commit: `df6d7ee ticket: record queued routing implementation`
- Merge commit: `merge: queued ticket routing`

Review:
- External reviewer `orchestrator-routing-reviewer-20260607` approved with no blockers.

Validation after merge:
- `cargo test -p tui multi_pod --lib`
- `cargo fmt --check`
- `git diff --check`
- `target/debug/yoi ticket doctor`