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
