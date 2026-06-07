Implemented, reviewed, merged, and validated.

Summary:
- Updated worktree workflow rules so child worktrees may include tracked `.yoi` project records instead of excluding all `.yoi/**`.
- Sparse-checkout guidance now excludes generated/personal/local/runtime state such as `.yoi/memory`, root and recursive `_logs`, logs, locks, local/runtime/pods/sessions/sockets/tmp/cache/secrets, local overrides, and secret-like files.
- Updated validation guidance to check excluded generated/local paths rather than requiring `.yoi` absence.
- Updated child-worktree Ticket edit policy: branch-local artifacts/dossiers may live in child worktrees; active orchestration progress and final review/approval/close remain main-workspace responsibilities.
- Updated multi-agent workflow and generated role/queue guidance to match the new boundary.
- Did not change memory root detection; `memory-root-uses-yoi-memory-marker` remains the follow-up for that.

Implementation:
- Child commits: `ee85b51 workflow: narrow yoi worktree sparse exclusions`, `6f33275 fixup! workflow: narrow yoi worktree sparse exclusions`
- Merge commit: `merge: narrow yoi worktree sparse exclusions`

Review:
- External reviewer initially requested recursive `.yoi/**/_logs/**` sparse exclusions.
- Fixup added recursive `_logs` exclusions.
- Reviewer approved after fixup.

Validation after merge:
- `cargo test -p client ticket_role --lib`
- `cargo test -p tui ticket_queue_notification --lib`
- `target/debug/yoi ticket doctor`
- `cargo fmt --check`
- `git diff --check`