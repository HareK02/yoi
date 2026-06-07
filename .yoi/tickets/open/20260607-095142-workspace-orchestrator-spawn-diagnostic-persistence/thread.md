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

<!-- event: plan author: hare at: 2026-06-07T10:00:33Z -->

## Plan

## Delegation intent

Implement `workspace-orchestrator-spawn-diagnostic-persistence` in a dedicated worktree.

Worktree:
- `.worktree/workspace-orchestrator-spawn-diagnostic-persistence`
- branch `work/workspace-orchestrator-spawn-diagnostic-persistence`

Coder Pod:
- `orchestrator-diagnostic-coder-20260607`

Scope and boundaries:
- Keep Orchestrator scope policy unchanged. The observed scope conflict with the current `yoi` Pod is expected for dogfooding and should remain a visible diagnostic, not be solved by loosening scope.
- Preserve the current `Ensure` vs `Observe` lifecycle distinction. `Observe` refresh must not retry spawn.
- Persist or carry forward the most recent actionable Orchestrator lifecycle failure from an `Ensure` attempt so a later `Observe` result of plain `missing` does not erase it.
- Clear/supersede stale lifecycle failures when Orchestrator becomes live/restored/spawned or when a newer lifecycle diagnostic is produced.
- Keep Ticket actionbar gate messages separate from Orchestrator lifecycle diagnostics.
- Prefer a small panel/app-state model change plus focused tests over broad UI restructuring.

Relevant code map:
- `crates/tui/src/multi_pod.rs`: panel load/reload, `OrchestratorLifecycleMode::{Ensure, Observe}`, `orchestrator_lifecycle`, `spawn_orchestrator_pod`, snapshot assembly.
- `crates/tui/src/workspace_panel.rs`: panel model/header diagnostics rendering and actionbar behavior.
- `crates/client/src/ticket_role.rs`: Ticket role Pod launch path; errors from spawn/connect/initial Run should remain available to the TUI lifecycle layer.

Requested validation:
- Focused TUI tests covering diagnostic persistence and clearing.
- `cargo test -p tui ... --lib` for affected tests.
- `cargo fmt --check`.
- `git diff --check`.
- `target/debug/yoi ticket doctor`.


---

<!-- event: implementation_report author: hare at: 2026-06-07T10:10:37Z -->

## Implementation report

## Implementation report

Coder Pod `orchestrator-diagnostic-coder-20260607` completed implementation in `.worktree/workspace-orchestrator-spawn-diagnostic-persistence`.

Commit:
- `e15e9b7 fix: persist orchestrator lifecycle diagnostics`

Reported summary:
- Added `MultiPodApp` state to remember the latest actionable workspace Orchestrator lifecycle failure.
- Later `Observe` refreshes that only report plain `missing` / `stopped` keep the prior actionable diagnostic visible.
- Stored diagnostic is cleared when the Orchestrator becomes `live`, `spawned`, or `restored`.
- A newer `Unavailable` lifecycle diagnostic supersedes the older one.
- Actionbar behavior and Ticket gate messages were not changed.

Reported validation:
- `cargo test -p tui multi_orchestrator --lib`
- `cargo test -p tui --lib`
- `cargo fmt --check`
- `git diff --check`
- `nix build .#yoi`
- `./result/bin/yoi ticket doctor`

Caveat:
- `target/debug/yoi ticket doctor` was not available inside the child worktree; coder used the Nix-built binary after `nix build .#yoi`.

External review will be delegated before merge.


---

<!-- event: review author: hare at: 2026-06-07T10:15:40Z status: approve -->

## Review: approve

Approved by `orchestrator-diagnostic-reviewer-20260607`.

No blocking findings. The implementation matches the ticket intent and is merge-ready.

Evidence:
- `Ensure` vs `Observe` is preserved. Initial load uses `Ensure`; refresh paths use `Observe`; observe does not restore/spawn.
- Prior actionable failure is retained across plain observe `missing` / `stopped` for the same Pod name.
- Plain missing without prior failure remains plain missing.
- Stale failures are cleared when Orchestrator becomes `live`, `spawned`, or `restored`.
- New `Unavailable` diagnostics supersede older ones.
- Ticket actionbar gating remains separate; lifecycle diagnostics remain in `panel.header.diagnostics`.
- Worktree is clean and merge-tree against current `develop` reports no conflict.

Reviewer validation:
- `CARGO_TARGET_DIR=/tmp/yoi-review-target cargo test -p tui multi_orchestrator --lib`
- `CARGO_TARGET_DIR=/tmp/yoi-review-target cargo test -p tui --lib`
- `cargo fmt --check`
- `git diff --check develop...HEAD`
- `nix build .#yoi --no-link`
- `CARGO_TARGET_DIR=/tmp/yoi-review-target cargo run -p yoi -- ticket doctor`


---
