<!-- event: create author: LocalTicketBackend at: 2026-06-08T01:10:36Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: decision author: ticket-intake at: 2026-06-08T02:46:39Z -->

## Decision

## Intake readiness classification

- readiness: implementation_ready
- needs_preflight: true
- risk_flags: [authority-boundary, scope-delegation, pod-runtime, workspace-context, workflow-guidance]

The Ticket is specific enough for Orchestrator routing: it fixes the public tool input name (`cwd`), default behavior when omitted, the distinction between process/tool cwd and inherited workspace context, and the invariant that `cwd` grants no authority by itself. Preflight remains required because the change touches SpawnPod delegation authority, scope validation, runtime cwd behavior, and workflow guidance.

---

<!-- event: intake_summary author: ticket-intake at: 2026-06-08T02:46:46Z -->

## Intake summary

Existing Ticket updated rather than duplicated. Scope is to add an optional `cwd` field to `SpawnPod` so child Pods can run process/tool commands from a child worktree while inheriting the parent workspace context. Required invariants: omitted `cwd` preserves current spawner pwd behavior; provided `cwd` is absolute, existing directory, and within parent read authority; cwd does not grant read/write permission, change workspace root, alter Profile discovery, or affect Pod naming; delegated scope remains explicit. Worktree and multi-agent workflow guidance should prefer coder Pods with child-worktree cwd. Ticket is implementation-ready for routing, with preflight recommended because it touches authority/scope/runtime/workflow boundaries.

---

<!-- event: state_changed author: ticket-intake at: 2026-06-08T02:46:46Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake classified the existing Ticket as ready for Orchestrator routing. The user or panel may now queue it; implementation must still wait for Orchestrator acceptance and any preflight/routing decision.

---

<!-- event: state_changed author: workspace-panel at: 2026-06-08T03:06:04Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket queued for Orchestrator routing.


---

<!-- event: decision author: orchestrator at: 2026-06-08T03:07:33Z -->

## Decision

Routing decision: preflight_needed

Reason:
- The Ticket is well specified, but Intake explicitly records that preflight remains required because this touches SpawnPod delegation authority, scope validation, runtime cwd behavior, and workflow guidance.
- The proposed `cwd` field must preserve a strict authority boundary: cwd changes process/tool working directory only, does not grant read/write permission, does not change workspace context, and does not affect Profile discovery or Pod naming.
- This needs a short binding design note before coder delegation so reviewer can judge schema, validation, runtime launch, and workflow-guidance changes against recorded authority invariants rather than implementation convenience.

Evidence checked:
- Ticket body requirements, design notes, and acceptance criteria.
- Thread: Intake classification, risk flags, and latest `ready -> queued` event.
- Workspace state: no matching branch/worktree exists; main workspace has unrelated Ticket-record edits.
- Code map search for SpawnPod input/launch, cwd/current_dir, scope validation, and workflow guidance paths.
- Ticket doctor: 0 errors; existing warnings are unrelated legacy closed-Ticket diagnostics.

Next action:
- Run `ticket-preflight-workflow` before implementation delegation.
- Preflight should record: `cwd` schema/name, required validation (`absolute`, existing directory, within parent read authority), whether cwd must also be readable by delegated child scope or only by parent authority at launch, exact relationship to child process `current_dir` and Bash default cwd, unchanged workspace/profile/Pod-name semantics, diagnostics, and workflow guidance updates.
- Leave this Ticket queued for now; do not transition `queued -> inprogress`, create `.worktree/allow-spawnpod-child-workspace-cwd`, or spawn coder/reviewer Pods until preflight records implementation readiness.

Escalate if:
- cwd validation would require broad capability model changes.
- Setting child process cwd cannot be separated from workspace-root/Profile discovery.
- Tool default cwd cannot be made consistent without changing Bash/tool execution semantics more broadly.

---
