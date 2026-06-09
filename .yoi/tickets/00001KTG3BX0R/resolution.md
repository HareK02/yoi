Implemented, reviewed, merged, and validated.

Summary:
- Added Orchestrator merge-completion guidance for reviewed in-progress Tickets with merge-ready dossiers.
- Merge authority is explicit: dogfooding/workspace policy may authorize merge/cleanup/close; conservative/missing authorization stops at the dossier.
- Dossier requirements are explicit: Ticket id/slug, branch/worktree, commits, intent/invariant check, implementation summary, coder/reviewer Pods, blockers fixed or rejected findings with reasons, validation performed, residual risks, dirty state, and parent/human decision needs.
- Post-merge validation baseline is explicit: focused Ticket/dossier tests, `cargo fmt --check`, `git diff --check`, `target/debug/yoi ticket doctor` where applicable, and broader validation such as `cargo check --workspace --all-targets` / `nix build .#yoi` when risk/touched files warrant it.
- Branch-local reviewer verdicts remain dossier evidence before merge; final main Ticket review/approval/completion records are written during merge-completion after confirming the reviewed branch is the branch being merged.
- Guidance covers merge, post-merge validation, Ticket `done`/close handling, Pod scope reclaim, and worktree/branch cleanup.
- Queue routing and worktree/coder/reviewer routing were not reimplemented in this ticket.

Implementation:
- Child commits: `cb17728 orchestrator: add merge completion guidance`, `33abf3f fixup! orchestrator: add merge completion guidance`
- Merge commit: `merge: orchestrator merge completion`

Review:
- External reviewer `orchestrator-merge-reviewer-20260607` requested changes for explicit dossier/validation requirements.
- Fixup addressed blockers.
- Reviewer approved after fixup.

Validation after merge:
- `cargo test -p client ticket_role --lib`
- `cargo fmt --check`
- `git diff --check`
- `cargo build -p yoi`
- `target/debug/yoi ticket doctor`