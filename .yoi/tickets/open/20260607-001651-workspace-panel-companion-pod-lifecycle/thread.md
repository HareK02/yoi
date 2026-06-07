<!-- event: create author: "yoi ticket" at: 2026-06-07T00:16:51Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: comment author: hare at: 2026-06-07T01:21:43Z -->

## Comment

## Dependency note: local role/session registry

Companion lifecycle should remain separate from Ticket/role Pod claim authority. Local role session and Ticket claim data belongs in the user-data workspace overlay planned by `workspace-panel-local-role-session-registry`, not in git-tracked Ticket metadata.

The Companion may eventually read/display this derived status, but it should not own the registry or gain mutation authority by default.

---

<!-- event: plan author: hare at: 2026-06-07T10:17:33Z -->

## Plan

## Preflight / implementation intent

Decision:
- The workspace Companion Pod id/name is the workspace directory basename.
- For this repository, that id/name is `yoi`.
- Do not introduce a separate `yoi-companion` identity for the normal workspace Companion.
- `yoi panel` should treat an existing live/restorable default workspace Pod named `yoi` as the Companion.
- The workspace Orchestrator remains a distinct identity such as `yoi-orchestrator`.

Readiness:
- This ticket is implementation-ready after `workspace-orchestrator-spawn-diagnostic-persistence` landed.
- Orchestrator scope conflict diagnostics are now persistent enough to distinguish expected scope conflicts from plain missing state during panel dogfooding.

Current code map:
- `crates/tui/src/multi_pod.rs`
  - owns panel load/reload, `MultiPodApp`, composer target dispatch, pending reloads, and current Orchestrator lifecycle glue.
  - current Companion composer target is not backed by a real Companion lifecycle and should be connected here or through a small helper module.
- `crates/tui/src/workspace_panel.rs`
  - owns panel model/header rows/composer target model and diagnostics.
- `crates/tui/src/pod_list.rs`
  - Pod list/read/restore/open helpers may be useful for determining live/restorable Companion presence.
- `crates/client/src/ticket_role.rs`
  - Ticket role launcher is separate; Companion is not a Ticket role and should not be implemented as one.
- `crates/client/src/runtime_command.rs` and existing Pod spawn/restore client helpers
  - likely source for typed runtime command and one-shot socket interaction.

Implementation intent:
- Add a workspace Companion lifecycle alongside, but not inside, the Ticket Orchestrator lifecycle.
- On `yoi panel` open, resolve the Companion Pod name from the workspace directory basename (`yoi` here).
- If that Pod is live, use it; if restorable, restore it; if missing, spawn it with the configured/default Companion profile path as appropriate.
- Panel close must not stop it.
- Route the panel `Companion` composer target to this Companion Pod's user history.
- Preserve Ticket Intake as a separate target; Intake text must not be appended to Companion history.
- Keep no-Ticket workspaces functional: Companion management chat plus Pod inspect/open should work without `.yoi/ticket.config.toml`.
- Do not reintroduce selected-Pod direct send, `--multi`, or `:ticket`.

Risks / boundaries:
- Companion should not be treated as a Ticket role and should not claim Ticket role session authority.
- Existing live `yoi` Pod may already own workspace write scope; the panel should reuse/attach/route to it rather than trying to spawn another `yoi` and failing with a scope conflict.
- If `yoi` is live but busy, message send behavior should follow existing Pod client semantics and report bounded diagnostics rather than inventing queueing in this ticket.
- Avoid broad UI restructuring; keep lifecycle state and diagnostics explicit and testable.
- If nonblocking spawn/restore requires more work, keep the implementation compatible with `workspace-panel-nonblocking-transitions` rather than solving that entire ticket here.

Validation expected:
- Focused TUI tests for Companion naming (`yoi` from repo dir), live/restorable/missing lifecycle decisions, no-Ticket availability, and composer routing separation from Ticket Intake.
- Existing panel/Orchestrator tests still pass.
- `cargo test -p tui ... --lib` for affected tests.
- `cargo fmt --check`.
- `git diff --check`.
- `target/debug/yoi ticket doctor`.


---

<!-- event: plan author: hare at: 2026-06-07T10:18:01Z -->

## Plan

## Delegation intent

Implement `workspace-panel-companion-pod-lifecycle` in a dedicated worktree.

Worktree:
- `.worktree/workspace-panel-companion-pod-lifecycle`
- branch `work/workspace-panel-companion-pod-lifecycle`

Coder Pod:
- `companion-lifecycle-coder-20260607`

Key decision to preserve:
- Companion Pod id/name is the workspace directory basename.
- In this repository, the Companion Pod id/name is `yoi`.
- Do not use `yoi-companion` for the normal workspace Companion.
- `yoi panel` should treat an existing live/restorable default workspace Pod named `yoi` as the Companion.
- Workspace Orchestrator remains distinct, e.g. `yoi-orchestrator`.

Scope and boundaries:
- Companion is not a Ticket role; do not route this through `TicketRole` or `.yoi/ticket.config.toml` role slots.
- Preserve Ticket Intake as a separate composer target and separate history path.
- Do not reintroduce selected-Pod direct send, `--multi`, or `:ticket`.
- Panel close must not stop Companion.
- Existing live `yoi` Pod may own workspace write scope; lifecycle should reuse/route to it instead of spawning a duplicate with a conflicting scope.
- Keep diagnostics bounded and visible.
- Avoid broad UI restructuring.

Requested validation:
- Focused TUI tests for Companion naming/lifecycle decisions and composer routing separation.
- Existing panel/orchestrator tests affected by lifecycle changes.
- `cargo test -p tui ... --lib` for affected tests.
- `cargo fmt --check`.
- `git diff --check`.
- `target/debug/yoi ticket doctor`.


---

<!-- event: implementation_report author: hare at: 2026-06-07T10:40:36Z -->

## Implementation report

## Implementation report

Coder Pod `companion-lifecycle-coder-20260607` completed implementation in `.worktree/workspace-panel-companion-pod-lifecycle`.

Commit:
- `ba0c9d5 feat: wire panel companion lifecycle`

Reported summary:
- Added real workspace Companion Pod lifecycle for `yoi panel`.
  - Companion Pod name is resolved from workspace basename, with no `-companion` suffix.
  - Live Pods are reused, restorable Pods are restored, missing Pods are spawned through normal default/Companion profile startup.
  - Panel close does not stop Companion.
- Wired `Companion` composer submit to the workspace Companion Pod via `Method::Run`.
  - It no longer routes to arbitrary selected Pods.
  - Empty composer behavior still preserves selected-Pod open/attach behavior.
  - Busy/unavailable Companion states keep the draft and surface bounded diagnostics.
- Kept Ticket Intake separate.
  - Ticket Intake target still launches Intake handoff and does not append to Companion history.
- Added focused tests for Companion naming, lifecycle decisions, no-Ticket Companion availability, composer routing, and Intake separation.

Changed files:
- `crates/tui/src/multi_pod.rs`
- `crates/tui/src/workspace_panel.rs`

Reported validation:
- `cargo test -p tui companion --lib`
- `cargo test -p tui multi_pod --lib`
- `cargo test -p tui workspace_panel --lib`
- `cargo test -p tui --lib`
- `cargo fmt --check`
- `git diff --check`
- `nix build .#yoi`
- `result/bin/yoi ticket doctor`

Caveat:
- `target/debug/yoi` was not used for `ticket doctor`; coder used the Nix-built `result/bin/yoi` after `nix build .#yoi`.

External review will be delegated before merge.


---
