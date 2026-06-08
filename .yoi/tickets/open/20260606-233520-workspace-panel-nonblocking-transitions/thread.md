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
