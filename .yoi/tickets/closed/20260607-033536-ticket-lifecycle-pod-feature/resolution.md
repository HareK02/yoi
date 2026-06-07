Implemented, reviewed, merged, and validated.

Summary:
- Refined the existing Ticket-domain `builtin:ticket` feature rather than introducing an Orchestrator-specific feature.
- Added `TicketFeatureAccess::{ReadOnly, Lifecycle}`.
- Added explicit Ticket tool partitions:
  - read-only/status tools: `TicketList`, `TicketShow`, `TicketDoctor`;
  - mutating/lifecycle tools: create/comment/review/intake-ready/workflow-state/status/close.
- Feature descriptors and installation now expose only the tools allowed by the configured access level.
- Preserved the existing full lifecycle/default behavior for existing call sites.
- Updated `TicketWorkflowState` tool description with the queued acceptance sequencing contract.
- Preserved typed backend/tool transition enforcement.

Implementation:
- Child commit: `3d662bc pod: split ticket feature access levels`
- Merge commit: `merge: ticket lifecycle feature`

Review:
- External reviewer `ticket-lifecycle-feature-reviewer-20260607` approved with no blockers.

Validation after merge:
- `cargo test -p pod ticket --lib`
- `cargo test -p ticket tool --lib`
- `cargo fmt --check`
- `git diff --check`
- `cargo check --workspace --all-targets`