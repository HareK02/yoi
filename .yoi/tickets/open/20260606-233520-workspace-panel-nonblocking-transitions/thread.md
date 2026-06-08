<!-- event: create author: "yoi ticket" at: 2026-06-06T23:35:20Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: hare at: 2026-06-06T23:36:10Z -->

## Plan

Created from investigation of perceived delays when opening `yoi panel`, attaching to a Pod, and returning from an attached Pod.

Root cause recorded:
- the panel waits for full snapshot/Orchestrator work before first draw;
- attach waits for socket connect or restore/spawn before a visible transition;
- return from nested Pod awaits `app.reload_or_notice().await` before showing the panel again.

Desired direction:
- show the panel/progress state first;
- move reload/connect/ensure work into background or explicit progress states where practical;
- especially make return-from-attach redraw the previous panel immediately and refresh in the background.


---

<!-- event: intake_summary author: intake at: 2026-06-07T22:30:10Z -->

## Intake summary

Existing Ticket refined sufficiently for routing. Request is to make `yoi panel` transition paths non-blocking: return from nested Pod should redraw the previous panel immediately and refresh in background; Pod open/attach should show attaching/restoring progress before socket/restore waits; initial panel load should avoid blocking first draw on non-essential snapshot/Orchestrator observation where practical. Acceptance criteria and non-goals are already explicit. Readiness: implementation_ready. needs_preflight: true because this touches async TUI lifecycle, background reload guarding, Pod attach/restore UX, and Orchestrator observation timing while preserving Ticket workflow and Pod authority semantics. risk_flags: [tui-lifecycle, async-reload, pod-attach, orchestrator-observation].

---

<!-- event: state_changed author: intake at: 2026-06-07T22:30:10Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake found the existing Ticket specific enough for Orchestrator routing. No duplicate Ticket was created; implementation must still preserve Ticket workflow semantics and Pod restore/spawn authority.

---

<!-- event: decision author: hare/intake at: 2026-06-07T22:35:08Z -->

## Decision

Scope clarification requested by user:

Panel-originated Pod launch/restore/open dispatch is explicitly in scope for this Ticket when the issue is visible blocking before feedback. Pressing the panel action to launch, restore, open, or attach a Pod should enter a visible progress/transition state such as `launching`, `restoring`, or `attaching` before awaiting runtime socket connect, restore, or spawn work.

This clarification is about non-blocking UX and progress/error surfacing for the dispatch path. It does not expand the Ticket to redesign role launch policy, Ticket workflow semantics, Orchestrator scheduling, Pod authority, or automatic coder/reviewer startup.

---

<!-- event: state_changed author: workspace-panel at: 2026-06-07T22:36:07Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket queued for Orchestrator routing.


---

<!-- event: decision author: orchestrator at: 2026-06-07T22:36:33Z -->

## Decision

Routing decision: preflight_needed

Reason:
- The Ticket is specific and valuable, but it touches async TUI lifecycle boundaries: panel first draw, nested Pod return, background reload guarding, Pod attach/restore/spawn progress, and Orchestrator observation timing.
- Intake explicitly marked readiness as implementation-ready only with `needs_preflight: true` and risk flags `[tui-lifecycle, async-reload, pod-attach, orchestrator-observation]` in the intake summary.
- Several implementation choices can preserve or violate important invariants: no duplicate overlapping reload tasks, no dropped key input during progress states, no Ticket workflow semantic changes, and no Pod restore/spawn authority changes.

Evidence checked:
- Ticket body requirements, non-goals, acceptance criteria, and problem paths.
- Thread plan, intake summary, user scope clarification for panel-originated launch/restore/open dispatch, and `ready -> queued` event.
- Workspace state: `develop` is clean except this Ticket's queued item/thread updates; no existing worktree or branch for this slug.
- Visible Pods: intake peer for this Ticket is idle; no implementation/review Pods are active for this Ticket.

Next action:
- Run `ticket-preflight-workflow` before implementation delegation.
- Preflight should produce a concise design/authority note covering: which transition paths become background/progress states first, how `PendingReload` or equivalent prevents overlap, how selection/composer/input are preserved, what remains synchronous for correctness, and focused test strategy.
- Do not transition `queued -> inprogress`, create `.worktree/workspace-panel-nonblocking-transitions`, or spawn coder/reviewer Pods until preflight records that the Ticket is implementation-ready.

Escalate if:
- Making initial panel first draw non-blocking requires a broader TUI runtime-loop rewrite.
- Pod launch/restore progress state would require changing spawn/restore authority semantics.
- Background reload cancellation/ordering needs a new durable scheduler or lease concept rather than a local UI guard.

---

<!-- event: intake_summary author: intake at: 2026-06-07T23:57:14Z -->

## Intake summary

Existing Ticket is already sufficiently clarified for routing. Scope covers non-blocking `yoi panel` transition UX: immediate redraw on nested-Pod return with background refresh, visible progress before panel-originated Pod launch/restore/open/attach waits, and practical reduction of first-draw blocking while preserving Ticket workflow semantics, Pod restore/spawn authority, input handling, and reload correctness. Readiness: implementation_ready. needs_preflight: true. risk_flags: [tui-lifecycle, async-reload, pod-attach, orchestrator-observation].

---

<!-- event: state_changed author: intake at: 2026-06-07T23:57:14Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake reconfirmed the existing Ticket as ready for Orchestrator routing. No duplicate Ticket was created. Prior thread context already records the queued attempt and Orchestrator preflight-needed decision; implementation must not start from Intake.

---

<!-- event: state_changed author: workspace-panel at: 2026-06-08T00:02:21Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket queued for Orchestrator routing.


---

<!-- event: decision author: orchestrator at: 2026-06-08T00:02:48Z -->

## Decision

Routing decision: preflight_needed

Reason:
- This Ticket is still queued for routing, but the recorded scope touches async TUI lifecycle, background reload ordering/guarding, Pod launch/restore/attach progress states, and Orchestrator observation timing.
- The thread explicitly records `needs_preflight: true` risk context and an earlier Orchestrator decision that preflight must cover transition paths, reload overlap prevention, input/selection preservation, synchronous correctness boundaries, and test strategy.
- No subsequent preflight decision or design note is present that converts those risks into binding implementation invariants.
- Current workspace state also contains unrelated dirty/untracked Ticket records, so implementation side effects should not start from this routing pass.

Evidence checked:
- Ticket body: background, requirements, non-goals, and acceptance criteria.
- Thread: intake summaries, user scope clarification, prior `preflight_needed` routing decision, and latest `ready -> queued` event.
- Workspace state: `develop` has modified Ticket files for this queued Ticket plus unrelated untracked Ticket records; no matching worktree or branch exists for this slug.
- Visible Pods: only the Intake peer for this Ticket and this Orchestrator are live; no coder/reviewer implementation Pods are active.

Next action:
- Run `ticket-preflight-workflow` and record the concise preflight/design note before implementation delegation.
- Leave this Ticket queued for now; do not transition `queued -> inprogress`, create `.worktree/workspace-panel-nonblocking-transitions`, or spawn coder/reviewer Pods until preflight records that implementation may proceed.

Escalate if:
- The non-blocking first-draw or attach-return path requires a broad TUI runtime-loop rewrite.
- Pod launch/restore progress states would require changing spawn/restore authority semantics.
- Background reload ordering requires a durable scheduler/lease rather than a local UI guard.

---

<!-- event: decision author: hare at: 2026-06-08T01:36:56Z -->

## Decision

## Binding decision: non-blocking transition scope

This Ticket should implement non-blocking UX for three panel-local wait points:

1. Process/Pod transition waits
   - Panel-originated Pod spawn / restore / attach / open should show a visible progress state before awaiting runtime socket connect, restore, or spawn completion.
   - Example states: `launching`, `restoring`, `attaching`.

2. Composer submit/send confirmation waits
   - Companion send, existing Ticket clarify/intake launch, and Ticket action dispatch should give immediate visible feedback that submit/dispatch started.
   - On busy/rejected/failed send, preserve the draft where appropriate and surface a bounded diagnostic.
   - On success, clear/update only according to the existing submit semantics.

3. Initial panel first draw
   - `yoi panel` should show an initial/loading/minimal panel before non-essential full snapshot, Orchestrator ensure/observe, or slow local runtime checks complete where practical.
   - Background completion should update the panel state/diagnostics once ready.

Implementation boundaries:

- Reuse or extend existing local pending-task guards such as `PendingReload` where practical.
- Prevent duplicate overlapping reload/transition tasks.
- Preserve selected row and composer draft across background refreshes unless the submitted operation succeeds and intentionally clears the draft.
- Keep enough synchronous checks to avoid presenting actions that would immediately violate known local state invariants.

Non-goals are limited to adjacent changes that could naturally be mixed into this work:

- Do not change Pod spawn/restore/attach authority, identity, or metadata semantics.
- Do not change Ticket workflow-state/action semantics.
- Do not introduce a durable scheduler/lease/background-job system.
- Do not rewrite the entire TUI runtime loop.

Everything else should not be listed as a non-goal unless the implementation discovers it is an actual adjacent risk. Unrelated exclusions add noise and should be avoided.

---

<!-- event: decision author: orchestrator at: 2026-06-08T02:08:16Z -->

## Decision

Routing decision: implementation_ready

Reason:
- The new human binding decision supplies the missing preflight boundary for the previously recorded `preflight_needed` routing decision.
- The implementation scope is now fixed to three panel-local wait points: process/Pod transition waits, composer submit/send confirmation waits, and practical initial panel first-draw work.
- Binding boundaries explicitly preserve Pod spawn/restore/attach authority, identity, metadata semantics, Ticket workflow-state/action semantics, local-only pending guards, input/selection/draft preservation, and no durable scheduler/runtime-loop rewrite.
- Remaining uncertainty is bounded TUI implementation investigation: exact local progress-state representation, `PendingReload` extension points, and test seams.

Evidence checked:
- Ticket body requirements and acceptance criteria.
- Thread decisions, including the latest `Binding decision: non-blocking transition scope`.
- Code map search for `PendingReload`, `reload_or_notice`, `prepare_open`, `finish_open`, `load_multi_pod_snapshot`, `OrchestratorLifecycleMode`, `run_pod_name_nested`, composer submit paths, and existing progress strings.
- Workspace state: no branch/worktree exists for this Ticket; unrelated dirty Ticket records exist in the main workspace, but the user explicitly asked to proceed if ready and this work will be isolated in a child worktree.
- Active work: `remove-non-goals-from-workflow-templates` is in separate worktree/review and touches workflow/docs/client role text, not the panel TUI implementation paths for this Ticket.
- Ticket doctor: 0 errors; existing warnings are unrelated legacy closed-Ticket diagnostics.

IntentPacket:

Intent:
- Make `yoi panel` transitions feel immediate by showing the next panel/progress state before slow local/runtime work completes, while preserving the existing authority and workflow semantics.

Binding decisions / invariants:
- Scope is limited to three panel-local wait points:
  1. Panel-originated Pod spawn / restore / attach / open should show visible `launching` / `restoring` / `attaching` style progress before awaiting runtime socket connect, restore, or spawn completion.
  2. Companion send, existing Ticket clarify/intake launch, and Ticket action dispatch should give immediate visible feedback that submit/dispatch started.
  3. Initial `yoi panel` should show an initial/loading/minimal panel before non-essential full snapshot, Orchestrator ensure/observe, or slow local runtime checks complete where practical.
- Preserve Pod spawn/restore/attach authority, identity, and metadata semantics.
- Preserve Ticket workflow-state/action semantics and role-launch policy.
- Do not introduce a durable scheduler, lease, or background-job system.
- Do not rewrite the entire TUI runtime loop or replace the single-Pod TUI.
- Reuse or extend local pending-task guards such as `PendingReload` where practical.
- Prevent duplicate overlapping reload/transition tasks.
- Preserve selected row and composer draft across background refreshes unless the submitted operation succeeds and intentionally clears the draft.
- Keep enough synchronous checks to avoid presenting actions that would immediately violate known local state invariants.

Requirements / acceptance criteria:
- Returning from nested Pod should redraw the previous panel immediately with a concise refreshing notice, then perform snapshot reload in the background.
- Avoid awaiting `app.reload_or_notice().await` on the attach-return path before the panel is visible.
- Opening/attaching/restoring/launching a Pod from the panel should visibly enter a progress state before slow socket/restore/spawn waits.
- Composer submit/dispatch paths should show immediate sending/launching/dispatching feedback and preserve drafts on busy/rejected/failed sends where existing semantics require it.
- Initial panel open should avoid blocking first draw on non-essential snapshot/Orchestrator work where practical, using a minimal/loading state or equivalent.
- Background completion must update Pod state, Ticket rows, Orchestrator state, diagnostics, selection, and composer target consistently.
- Preserve no-Ticket Pod-centric behavior.
- Tests should cover non-blocking return/reload and practical progress-state behavior.

Implementation latitude:
- Coder may choose the exact local state names/types, whether to extend `PendingReload` or introduce adjacent local guards, and which paths can safely remain synchronous.
- Coder may split implementation if initial first-draw requires a smaller practical subset, but must report any acceptance criterion not fully achieved.
- Coder may add focused unit tests around app state transitions rather than full E2E.

Escalate if:
- Initial first draw cannot be improved without a broad TUI runtime-loop rewrite.
- Progress states require changing Pod spawn/restore/attach authority or metadata semantics.
- Background reload ordering requires durable scheduler/lease semantics rather than local UI guards.
- The implementation would drop composer text/selection unexpectedly or allow duplicate overlapping reloads.

Validation:
- Focused TUI tests for `multi_pod` / workspace panel transition behavior.
- Existing no-Ticket and Ticket-enabled panel tests.
- `cargo test -q -p tui multi_` or more focused equivalent selected by coder.
- `cargo test -q -p tui workspace_panel`.
- `cargo fmt --check`.
- `git diff --check`.
- `cargo run -q -p yoi -- ticket doctor`.
- Because TUI/runtime behavior is touched, final validation should include `nix build .#yoi` before merge-completion unless explicitly waived.

Current code map:
- `crates/tui/src/multi_pod.rs`: `PendingReload`, `run_multi_pod_tui`, `MultiPodApp::load`, `reload_or_notice`, `prepare_open`, `finish_open`, `load_multi_pod_snapshot`, `OrchestratorLifecycleMode`, composer submit/action handling, existing tests.
- `crates/tui/src/spawn.rs`: nested Pod open/connect/restore path and existing `attaching` strings.
- `crates/tui/src/workspace_panel.rs`: panel model/action/progress/status rendering.
- `crates/tui/src/lib.rs` and `crates/tui/src/single_pod.rs`: nested single-Pod return boundary as needed.

Critical risks / reviewer focus:
- Return-from-nested-Pod path must not block on snapshot reload before drawing the panel.
- Progress states must not imply authority changes or successful launch/restore before acceptance/evidence exists.
- Duplicate reload/transition tasks must be guarded.
- Selection/composer draft must survive background refresh unless success semantics intentionally clear it.
- Initial first draw should improve only where practical without a large runtime-loop rewrite.

---

<!-- event: state_changed author: orchestrator at: 2026-06-08T02:08:22Z from: queued to: inprogress reason: orchestrator_acceptance field: workflow_state -->

## State changed

Accepted queued implementation after human preflight supplementation and routing IntentPacket were recorded. This acceptance precedes worktree creation and coder/reviewer Pod spawning.

---

<!-- event: plan author: orchestrator at: 2026-06-08T02:08:45Z -->

## Plan

Implementation routing accepted and worktree created.

Worktree plan:
- Branch: `workspace-panel-nonblocking-transitions`
- Worktree: `.worktree/workspace-panel-nonblocking-transitions`
- Base: `e3df223 ticket: route workspace panel nonblocking transitions`
- Scope: implement non-blocking workspace panel transition UX for panel-local wait points recorded in the binding decision.

Delegation plan:
- Spawn sibling coder Pod with write scope limited to `.worktree/workspace-panel-nonblocking-transitions`.
- Coder must run Bash commands from the child worktree, not from the main workspace.
- Coder must not edit main-workspace `.yoi` / Ticket / workflow / docs records, and must not create generated memory/local/runtime/secret-like files.
- Reviewer will be spawned read-only after coder reports diff/commits and validation evidence.

Concurrency note:
- `remove-non-goals-from-workflow-templates` remains active in a separate worktree/review loop. This panel implementation must remain isolated and should not edit that worktree or depend on unmerged workflow-template changes.

Stop/completion policy:
- Because the human authorized cleanup after completion, this Orchestrator will proceed through reviewer approval, merge-completion validation, Ticket close, Pod stop, worktree removal, and branch deletion if the branch is approved and merge authority remains clear.

---
