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

<!-- event: decision author: orchestrator at: 2026-06-08T07:06:17Z -->

## Decision

Routing decision: implementation_ready

Correction:
- The previous `preflight_needed` routing decision was too conservative. The Ticket body already fixes the core authority boundary: `cwd` is intentionally separate from runtime workspace root, Profile discovery, project/Ticket/memory roots, and Pod identity.
- The remaining choices are bounded implementation details rather than preflight blockers.

Reason:
- The Ticket explicitly states that `SpawnPod.cwd` is optional child process/tool working-directory context, not workspace context and not an authority grant.
- It already records that `cwd` does not replace delegated scope and does not change profile/manifest/workspace-root semantics.
- The recently merged direct/delegation authority split strengthens the surrounding invariant: child scope delegation remains explicit and independent from process cwd.
- Requirements and acceptance criteria are observable: schema/input support, validation/diagnostics, child process current_dir/default Bash cwd, workflow guidance, and tests.

Evidence checked:
- Ticket body and thread, including the user clarification that the Ticket exists specifically to separate workspace and cwd.
- Workspace state: no existing matching worktree/branch; main workspace clean before this routing update.
- Existing workflow pain observed earlier: spawned child Pods currently validate/run from parent cwd and require non-recursive parent read grants even when implementation work should be rooted in a child worktree.
- Related completed Ticket: `split-direct-and-delegation-authority` has now separated direct scope from delegation authority, so `cwd` can remain a process context without implying child authority.

IntentPacket:

Intent:
- Add an optional `cwd` field to `SpawnPod` so the parent can choose the child process/tool default working directory independently from runtime workspace context and delegated scope.

Binding decisions / invariants:
- `cwd` means child process/tool working directory only.
- `cwd` is not runtime workspace root.
- `cwd` does not affect Profile discovery, project record root, Ticket config root, workflow registry, memory root discovery, Pod name/default identity, or role launch workspace context.
- `cwd` grants no read/write authority. Child filesystem access remains controlled by explicit delegated `scope` and, after the direct/delegation split, by the parent's delegation authority.
- Omitted `cwd` preserves existing behavior as closely as possible.
- Provided `cwd` must be absolute, exist, and be a directory.
- Provided `cwd` must be readable/usable under the child effective direct scope, or launch must fail clearly. This prevents starting a child in a directory it cannot inspect/use.
- Worktree/multi-agent workflows should set coder `cwd` to the child worktree while still delegating explicit read/write scope to that worktree.
- Reviewer `cwd` is a workflow convenience, not an authority signal.

Requirements / acceptance criteria:
- Extend `SpawnPod` tool input/schema with optional `cwd`.
- Validate `cwd` before child launch and return clear errors for relative, missing, non-directory, or not-in-child-scope paths.
- Start the spawned Pod process with `cwd` as its process current directory when provided.
- Ensure the child Bash/tool default cwd is the provided `cwd` so commands no longer need mandatory `cd <worktree> && ...` wrappers.
- Preserve existing behavior for callers that omit `cwd`.
- Keep delegated scope validation independent from `cwd`.
- Update multi-agent/worktree workflow guidance and/or generated launch prompt wording where maintained guidance currently tells coders to `cd` into worktrees because SpawnPod cannot set cwd.
- Add focused tests for schema/validation and child launch cwd behavior where practical.

Implementation latitude:
- Coder may choose exact Rust field names/types and validation helper placement.
- Coder may update only maintained workflow/prompt guidance that directly references child worktree `cd` workarounds.
- If runtime process cwd is easier to validate via child direct scope than parent authority, prefer child-scope validation because `cwd` is for the child's usable environment, not a parent capability grant.
- Coder may leave role launcher/Profile workspace-root behavior untouched unless a test proves `cwd` currently contaminates it.

Escalate if:
- Adding `cwd` requires changing workspace-root/Profile/memory-root semantics.
- Child process current_dir cannot be changed without broad runtime command redesign.
- A safe validation rule cannot be expressed without granting authority via `cwd`.
- Existing spawned Pod registry/scope accounting assumes child process cwd is always the parent cwd in a way that cannot be localized.

Validation:
- Focused tests for `SpawnPod` input/schema validation.
- Tests or probes proving provided `cwd` becomes the child process/tool default cwd while omitted `cwd` preserves current behavior.
- SpawnPod scope/delegation tests to ensure `cwd` does not bypass delegated scope.
- `cargo test -p pod spawn_pod --test spawn_pod_test` or focused equivalent.
- `cargo test -p protocol` / schema tests if tool input types live there.
- `cargo fmt --check`.
- `git diff --check`.
- `cargo run -q -p yoi -- ticket doctor`.
- Because tool schema/runtime/workflow guidance may change, final merge-completion should include `nix build .#yoi`.

Current code map:
- `crates/pod/src/tools/pod_management.rs` or current SpawnPod tool input/handler path.
- `crates/pod/src/spawn/tool.rs` and related runtime spawn implementation after the authority split.
- `crates/protocol/src/lib.rs` if tool schemas/input structs are protocol-owned.
- `crates/pod/tests/spawn_pod_test.rs` for SpawnPod behavior tests.
- Workflow guidance files for worktree/multi-agent coder instructions.

Critical risks / reviewer focus:
- `cwd` must not become a hidden workspace-root or authority source.
- Relative/missing/out-of-scope cwd must fail clearly before launch.
- Omitted `cwd` must preserve existing launch behavior.
- Child direct tools must run from `cwd` by default when provided.
- Delegated scope and delegation authority validation must remain independent and stricter than cwd convenience.

---

<!-- event: state_changed author: orchestrator at: 2026-06-08T07:06:29Z from: queued to: inprogress reason: orchestrator_acceptance field: workflow_state -->

## State changed

Accepted queued implementation after correcting the prior over-conservative preflight routing and recording an implementation-ready IntentPacket. This acceptance precedes worktree creation and coder/reviewer Pod spawning.

---
