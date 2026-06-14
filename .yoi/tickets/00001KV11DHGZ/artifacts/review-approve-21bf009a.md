Approve implementation review for Ticket 00001KV11DHGZ.

Scope reviewed: implementation commit 21bf009a plus ticket report commit 77892b94 against base cdb12af9.

Evidence:
- Builtin role profile resources no longer contain `scope` or `delegation_scope`; reusable profile data retains role/model/prompt/feature/tool policy only.
- Fresh profile launch scope is applied in `crates/pod/src/entrypoint.rs` by launch policy after profile resolution. Default/Companion launches receive direct workspace write scope with `.worktree` write denied and delegation gets workspace read plus `.worktree` write. Orchestrator ticket-role launches receive direct root read and delegation root read plus `.worktree` write, with no root workspace write delegation.
- `SpawnPod` profile/inherit handling continues to replace child direct scope with the explicit delegated child scope and resets child delegation unless explicitly provided; profile/default scope does not leak into child direct authority.
- Pod metadata restore uses saved manifest snapshots when present, so saved scope/delegation are preserved instead of being overwritten by current profile/default launch policy.
- One-file manifest loading still rejects missing/empty concrete `scope.allow`; the retained user-profile scope compatibility path is separated from builtin role authority and is overwritten by launch/delegation policy on fresh role launches.

Validation performed:
- `cargo test -p manifest --quiet`
- Focused pod tests for normal startup launch policy, orchestrator launch policy, SpawnPod delegation scoping, and metadata snapshot restore.
- `cargo test -p client ticket_role --quiet`
- `cargo build -p yoi`
- `cargo fmt --check`
- `git diff --check cdb12af9..HEAD`

Result: approve. No blocking requirement or design-boundary concern found.
