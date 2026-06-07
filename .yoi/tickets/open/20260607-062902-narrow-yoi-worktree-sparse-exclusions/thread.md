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
