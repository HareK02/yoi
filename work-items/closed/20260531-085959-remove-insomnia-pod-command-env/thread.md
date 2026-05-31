<!-- event: create author: tickets.sh at: 2026-05-31T08:59:59Z -->

## Created

Created by tickets.sh create.

---

<!-- event: review author: hare at: 2026-05-31T10:09:52Z status: approve -->

## Review: approve

External reviewer: `remove-insomnia-pod-command-reviewer-20260531`
Reviewed implementation commit: `c618fa6` (`cli: remove pod command env override`)
Verdict: approve

Summary:
- `INSOMNIA_POD_COMMAND` is removed as an active runtime/config/test override.
- Pod runtime launch remains typed through `PodRuntimeCommand`, defaulting to current executable plus the `pod` prefix argument.
- SpawnPod tests now use typed injection rather than process-wide env mutation.
- Production spawn/restore paths still use `PodRuntimeCommand::resolve()`, preserve detached process handling, and retain the ready/socket delivery behavior.

Requirements mapping:
- No active non-work-item `INSOMNIA_POD_COMMAND` references remain in the implementation branch.
- No replacement env var was introduced.
- No `insomnia-pod` binary/alias, Pod protocol/flag/profile semantic changes, or `tui` rename were introduced.

Blockers: none.

Non-blocking follow-up:
- Merge conflict in `docs/environment.md` is expected after `eliminate-test-only-env-vars`; integration should keep the newer generic test-only wording while preserving removal of explicit `INSOMNIA_POD_COMMAND` references.

Validation adequacy:
- Coder validation covered fmt, helper/client/pod tests, cargo check, doctor, diff check, and active-reference grep.
- Reviewer additionally reran targeted tests/checks in the implementation worktree and confirmed no active refs for `INSOMNIA_POD_COMMAND`, `POD_COMMAND_OVERRIDE_ENV`, `from_override_env`, or `executable_only` outside work items.


---

<!-- event: close author: hare at: 2026-05-31T10:12:03Z status: closed -->

## Closed

Removed `INSOMNIA_POD_COMMAND` as an active runtime/config/test override.

Implementation:
- `PodRuntimeCommand::resolve()` now always uses the typed default: current executable plus the `pod` prefix argument.
- Removed override/env parsing and related tests from the `insomnia` helper crate.
- Updated SpawnPod tests to use typed `PodRuntimeCommand` injection instead of process-wide env mutation.
- Preserved production spawn/restore behavior, detached process handling, and ready/socket delivery behavior.
- Updated docs so no active supported surface documents the removed env override.

Review:
- External reviewer `remove-insomnia-pod-command-reviewer-20260531` approved implementation commit `c618fa6`.

Validation after merge:
- `cargo fmt --check`
- `cargo test -p insomnia`
- `cargo test -p client -p insomnia`
- `cargo test -p pod --lib discovery::tests`
- `cargo test -p pod --test spawn_pod_test`
- `cargo check -p tui -p pod -p client -p insomnia` (passed with unrelated existing warnings)
- `./tickets.sh doctor`
- `git diff --check`
- `git grep -n "INSOMNIA_POD_COMMAND" -- ':!work-items' || true` produced no active references.


---
