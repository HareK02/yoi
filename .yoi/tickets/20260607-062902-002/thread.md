<!-- event: create author: LocalTicketBackend at: 2026-06-07T06:29:02Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: hare at: 2026-06-07T07:14:24Z -->

## Plan

## Preflight / implementation intent

Classification: implementation-ready and should run before `memory-root-uses-yoi-memory-marker`.

Intent:
- Stop treating child worktrees as code-only surfaces that exclude all `.yoi/**`.
- Keep Yoi project records visible in child worktrees where useful: Tickets, workflows, curated Knowledge, and project config.
- Continue excluding generated/personal/local runtime state, especially `.yoi/memory`.

Updated decision from discussion:
- `.yoi` remains the project records/workspace marker.
- `.yoi/memory` is the repo-local memory marker and should not be copied into child worktrees.
- The worktree workflow should stop saying child worktrees must not contain `.yoi`; it should instead define which `.yoi` subpaths are included/excluded and how Ticket edits are split between branch-local artifacts and main-workspace final records.

Implementation direction:
- Update `.yoi/workflow/worktree-workflow.md`:
  - replace `!/.yoi/**` sparse rules with narrower exclusions for `.yoi/memory`, logs, locks, local/runtime/secret-like paths;
  - update validation from `test ! -e .../.yoi` to checks that memory/local paths are absent and project records are allowed;
  - update child-worktree prohibitions from “no `.yoi`” to “no generated memory/local/runtime files”;
  - define Ticket edit policy: branch-local artifacts/dossiers may live in child worktree; active orchestration progress and final review/close remain main-workspace responsibilities unless explicitly designed otherwise.
- Update `.yoi/workflow/multi-agent-workflow.md` where it still says `.yoi` is excluded or that Ticket/workflow/docs records are always main-workspace-only.
- Update generated role guidance/tests if code still says child worktrees exclude `.yoi` broadly.
- Keep the change focused on worktree sparse/workflow policy; memory root detection changes belong to `memory-root-uses-yoi-memory-marker`.

Validation:
- Run focused tests for any crate touched.
- Run `target/debug/yoi ticket doctor`, `cargo fmt --check`, and `git diff --check`.
- If only workflow/docs change, no broad cargo tests are required beyond relevant prompt/guidance tests if touched.

---

<!-- event: intake_summary author: INSOMNIA at: 2026-06-07T07:14:31Z -->

## Intake summary

Implementation-ready: keep `.yoi` as the project records marker, stop excluding all `.yoi/**` from child worktrees, include tracked project records, exclude `.yoi/memory`/logs/local/runtime files, and update workflow/role guidance plus Ticket edit policy. Memory root detection remains a separate follow-up ticket.

---

<!-- event: state_changed author: INSOMNIA at: 2026-06-07T07:14:31Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Ticket intake complete; workflow_state intake -> ready.


---

<!-- event: implementation_report author: hare at: 2026-06-07T07:15:31Z -->

## Implementation report

## Delegation

Implementation delegated to child Pod `narrow-yoi-worktree-coder-20260607` in worktree `.worktree/narrow-yoi-worktree-sparse-exclusions` on branch `work/narrow-yoi-worktree-sparse-exclusions`.

This worktree intentionally uses the proposed narrower sparse rules:
- includes `.yoi` project records;
- excludes `.yoi/memory`, memory logs, ticket backend lock, and local/runtime paths.

Scope:
- Update `worktree-workflow` and `multi-agent-workflow` away from `.yoi` whole-directory exclusion.
- Define project-record vs generated-memory/local-runtime boundaries.
- Update role guidance/tests if they still say child worktrees exclude all `.yoi`.
- Do not implement memory-root detection; `memory-root-uses-yoi-memory-marker` follows after this.

The child should commit implementation work in the child worktree and report diff/tests. Merge, review, Ticket closure, and cleanup remain with the parent/human workflow.

---

<!-- event: implementation_report author: hare at: 2026-06-07T07:28:48Z -->

## Implementation report

## Implementation report

Coder Pod `narrow-yoi-worktree-coder-20260607` completed implementation in `.worktree/narrow-yoi-worktree-sparse-exclusions`.

Commit:
- `ee85b51 workflow: narrow yoi worktree sparse exclusions`

Reported summary:
- Updated `.yoi/workflow/worktree-workflow.md` so child worktrees may include tracked `.yoi` project records.
- Replaced broad `.yoi/**` sparse exclusion guidance with narrower exclusions for `.yoi/memory`, logs, locks, local/runtime/cache/tmp/session/socket/pod state, and secret-like paths.
- Updated validation guidance to allow `.yoi` while checking `.yoi/memory` and local/runtime/log/lock/secret-like paths are absent.
- Added explicit Ticket edit policy: branch-local artifacts/dossiers may live in child worktrees; active orchestration progress and final review/approval/close stay main-workspace responsibilities.
- Updated `.yoi/workflow/multi-agent-workflow.md` to stop describing `.yoi` as broadly excluded and to clarify child-worktree project-record boundaries.
- Updated generated role guidance/tests in `crates/client/src/ticket_role.rs` and `crates/tui/src/multi_pod.rs`.
- Did not implement memory root detection and did not recreate `.yoi/memory`.

Reported validation:
- `cargo test -p client ticket_role --lib`
- `cargo test -p tui ticket_queue_notification --lib`
- `target/debug/yoi ticket doctor`
- `cargo fmt --check`
- `git diff --check`
- `nix build .#yoi`

External review will be delegated before merge.

---

<!-- event: review author: narrow-yoi-worktree-reviewer-20260607 at: 2026-06-07T07:34:37Z status: request_changes -->

## Review: request changes

Request changes.

Blocker:
- `worktree-workflow` sparse-checkout rules do not exclude recursive `.yoi/**/_logs/**`, even though the Ticket explicitly requires generated log paths under `.yoi/**/_logs/**` to remain out of child worktrees.

Evidence:
- `.yoi/workflow/worktree-workflow.md` only excludes root log dirs:
  - `!/.yoi/logs/`
  - `!/.yoi/logs/**`
  - `!/.yoi/_logs/`
  - `!/.yoi/_logs/**`
- There is no committed recursive `_logs` sparse rule.
- The validation block checks for any `*/_logs` directory, so a tracked nested `.yoi/<project-record>/_logs/...` path would be allowed by sparse-checkout and then fail validation rather than being excluded by the rules.

Suggested minimal fix:
- Add recursive sparse exclusions such as:
  - `!/.yoi/**/_logs/`
  - `!/.yoi/**/_logs/**`
- Consider whether the same recursive treatment is intended for `logs`, `locks`, `local`, `runtime`, `pods`, `sessions`, `sockets`, `tmp`, `cache`, and `secrets`; the required blocker is strongest for `_logs` because the Ticket names `.yoi/**/_logs/**` explicitly.

Other checks passed:
- Workflow docs no longer require whole-directory `.yoi` exclusion.
- Project records are allowed in child worktrees.
- Ticket edit policy is clear enough and keeps final review/approval/close as main-workspace responsibilities.
- Generated role guidance/tests align with the new boundary.
- Memory root detection was not changed.

Reviewer validation:
- `cargo test -p client ticket_role --lib`
- `cargo test -p tui ticket_queue_notification --lib`
- `target/debug/yoi ticket doctor`
- `cargo fmt --check`
- `git diff --check develop...HEAD`
- `nix build .#yoi`
- `git merge-tree --write-tree develop HEAD`

Merge readiness: not ready until recursive `_logs` sparse exclusion is fixed.

---

<!-- event: review author: narrow-yoi-worktree-reviewer-20260607 at: 2026-06-07T07:37:53Z status: approve -->

## Review: approve

Approved after fixup commit `6f33275`.

Evidence:
- Previous blocker is fixed in `.yoi/workflow/worktree-workflow.md`: sparse rules now include both `!/.yoi/**/_logs/` and `!/.yoi/**/_logs/**`.
- Nearby guidance explicitly says generated log trees exclude root `_logs` and recursive `.yoi/**/_logs/**`.
- No broad `.yoi/**` exclusion was reintroduced.
- `.yoi` project records remain allowed in child worktrees.
- Generated/local state remains excluded: `.yoi/memory`, logs, locks, local/runtime/pods/sessions/sockets/tmp/cache/secrets, local overrides, and secret-like files remain listed in sparse rules.
- Ticket edit policy remains intact: active orchestration progress and final review/approval/close remain main-workspace responsibilities.
- Memory root detection was not changed; diff remains limited to workflow/guidance files.

Reviewer validation:
- `cargo test -p client ticket_role --lib`
- `cargo test -p tui ticket_queue_notification --lib`
- `target/debug/yoi ticket doctor`
- `cargo fmt --check`
- `git diff --check develop...HEAD`
- `nix build .#yoi`
- `git merge-tree --write-tree develop HEAD`

Merge readiness: approved; worktree clean at `6f33275`.

---

<!-- event: state_changed author: hare at: 2026-06-07T07:38:29Z from: ready to: done reason: closed field: workflow_state -->

## State changed

Ticket closed; workflow_state set to done.


---

<!-- event: close author: hare at: 2026-06-07T07:38:29Z status: closed -->

## Closed

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

---
