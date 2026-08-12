# Flow verifier uses a stale committed-session capture

Date: 2026-08-11
Ticket: `00001KZPQW4GJ`
Flow instance: `019ff243-d4df-71f2-beb3-cbc360f58c34`

## Symptom

`RequestFlowTransition` repeatedly evaluated the `implement -> review` condition against a parent-session capture that ended immediately after branch creation and Ticket/plan reads. It did not observe later committed session entries containing implementation work, post-commit validation, clean-tree checks, commits, or independent Reviewer approval.

The verifier therefore returned `indeterminate` even though the Workdir contained the implementation and the current Worker session had already recorded the required evidence.

## Repository-visible evidence

Named branch and commits:

- `work/00001KZPQW4GJ-worker-remove-v3`
- `8ae930c5fc81acb2c60de15add07e016a1552edd` — `worker: add guarded WorkerRemove lifecycle`
- `f60c2d583485572697f7ec42d8cf3c8015e7c179` — `worker: resume failed removal operation`

Post-commit validation:

- `cargo test -p worker --lib`: 520 passed.
- `cargo test -p worker-runtime --lib`: 127 passed.
- `cargo test -p yoi-workspace-server --lib retention::tests`: 12 passed.
- `cargo test -p yoi-workspace-server --lib worker_remove`: 5 passed.
- `cargo test -p yoi-workspace-server --lib stale_policy_and_failed_retry_restore_fence`: passed.
- `cargo check -p yoi-workspace-server -p worker-runtime -p worker`: passed.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.

Independent read-only Reviewer verdict:

> APPROVE — I found no blocker/high security or correctness issue in commits `8ae930c5` + `f60c2d58`.

The Reviewer explicitly confirmed the constrained four-field tool input, proof-only destructive boundary, exact Runtime-result Worker revision binding, successful recovery after registry purge, and failed-operation re-entry through the authoritative prepare/executing fence.

## Impact

A correct, tested, independently approved implementation cannot advance from `implement` to `review` because the Flow verifier does not see newly committed Worker history. Repeating validation or review inside the same live session does not repair the verifier input.

## Suggested fix

Before evaluating a transition, refresh the verifier's session capture from the latest committed Worker history revision and include stable references to:

- the current branch and commit,
- bounded validation command results,
- current clean-tree evidence,
- independent Reviewer verdicts,
- current Ticket review/evidence events.

The refreshed evidence must be committed to Worker history before verifier context construction, following the project context-injection invariant. Do not use an unrecorded transient reminder or mutate earlier history.
