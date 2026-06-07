<!-- event: create author: "yoi ticket" at: 2026-06-07T09:51:42Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: hare at: 2026-06-07T09:51:42Z -->

## Plan

## Background

`yoi panel` currently attempts to ensure the workspace Orchestrator on initial panel load. If Orchestrator spawn fails, the initial `Ensure` path can surface an `unavailable` diagnostic such as a scope conflict. A later panel reload runs in `Observe` mode and may replace the header/status with plain `orchestrator missing`, making the actual failure reason disappear within a few seconds.

This makes spawn failures look like a missing-state problem and prevents the user from reading the actionable error.

Observed example while dogfooding:

```text
error: failed to create pod: requested scope `/home/hare/Projects/yoi` conflicts with pod `yoi` rule `/home/hare/Projects/yoi`
```

The scope conflict itself is expected in this situation: the current Pod is being used as the Companion and already owns workspace write scope. The UI issue is that the panel loses the prior spawn failure diagnostic on refresh.

## Goal

Keep Orchestrator lifecycle diagnostics visible across panel refreshes until superseded by a successful Orchestrator state or a newer lifecycle diagnostic.

## Requirements

- Do not change Orchestrator scope policy in this ticket.
- Preserve the distinction between:
  - `Ensure` lifecycle attempts that can spawn/restore and produce actionable failure diagnostics;
  - `Observe` refreshes that should not retry spawn.
- When an `Ensure` spawn/restore fails, keep the failure reason visible after subsequent `Observe` reloads that only report `missing`.
- Do not let stale diagnostics remain after the Orchestrator becomes live/restored/spawned successfully.
- Prefer a bounded recent lifecycle diagnostic stored in the panel/app state over recomputing or retrying spawn on every refresh.
- Make the diagnostic readable enough that short-lived errors are not lost to periodic refresh.
- Ensure actionbar Ticket gate messages, such as `attention_required is set`, do not obscure the Orchestrator lifecycle failure reason.

## Acceptance criteria

- Reproduce by causing Orchestrator spawn failure, then waiting through at least one panel refresh: the actionable failure remains visible.
- When Orchestrator later becomes live/restorable/spawned, the stale failure diagnostic is cleared or superseded.
- Plain missing state still displays when no spawn has been attempted and no prior failure exists.
- No refresh path starts retrying Orchestrator spawn implicitly.
- Focused TUI model/lifecycle tests cover diagnostic persistence and clearing.
- `cargo test -p tui ... --lib`, `cargo fmt --check`, `git diff --check`, and `target/debug/yoi ticket doctor` pass.


---
