# Flow state graph and verifier

Flow is a Workspace-scoped declarative state graph. A Worker operating under an active Flow can request evaluation of its current state's outgoing conditions, but cannot select or write a target state directly.

## Source authority

A Workspace-authored Flow source is one DCDL document stored under a virtual path such as `flows/coder-review.dcdl`. Built-in Flow sources are compiled resources under `resources/flows/*.dcdl` and use read-only virtual paths such as `builtin/flows/coder-review.dcdl`. The source document is the graph authority; states and transitions are not normalized into independently editable relational records.

Every invocation uses a source-qualified typed selector:

```text
builtin:<slug>
workspace:<slug>
```

Unqualified selectors and implicit override precedence are rejected. `builtin:coder-review` resolves from the embedded resource catalog. `workspace:coder-review` resolves from the current Workspace DB source. Built-in and Workspace sources with the same slug coexist as distinct logical records.

The compiler pipeline is:

```text
DCDL source
  -> decodal evaluation
  -> private Serde-compatible value
  -> typed Flow source schema
  -> graph validation
  -> CompiledFlowDefinition
```

The Serde-compatible intermediate is private compiler infrastructure. Public APIs and persisted Flow runtime records use typed Flow domain values.

`flow_sources` stores one current logical Workspace-authored source per Workspace/slug. Every changed source creates an immutable `flow_source_revisions` row containing the original content, content digest, and compiled definition. Built-ins remain read-only embedded resources with an explicit monotonic resource revision; resolving one compiles and returns that resource snapshot without writing it into Workspace DB. Runtime pins source identity, revision, digest, and compiled definition in Worker state, so editing a Workspace source or updating a built-in resource never changes an existing instance.

The compiler rejects unknown fields, unsupported schema versions, invalid/reserved identifiers, unknown transition targets, authored `$cancelled` state/targets, terminal states with outgoing transitions, non-terminal states without transitions, unreachable states, and reachable closed paths that cannot reach a user-declared terminal state. It injects the synthetic exceptional-cancellation transition and `$cancelled` terminal state after validation.

## Runtime-owned instance and event authority

Flow source authority and Flow execution authority are split at the immutable source snapshot boundary.

The Workspace Backend stores only current Flow sources and immutable source revisions. Resolving a source-qualified selector returns the Workspace id, Flow id, revision, digest, and compiled definition. Resolution is read-only with respect to Flow execution: it never creates an instance, attempt, or event.

One Runtime Worker durably owns:

- the pinned source snapshot and compiled definition;
- its active Flow instance, current state, revision, and lifecycle status;
- its active transition attempt;
- its ordered Flow events.

The complete `FlowRuntimeState` is persisted as a typed `flow.runtime.v1` Worker session extension. Initial Flow state is committed in the same `UserInput` log record as the entered-state instructions and remaining Submit segments. Backend therefore cannot contain an active instance that Worker history has never observed.

Transition mutations clone the current Runtime state, append ordered events, persist the new session extension, and only then replace the in-memory projection. A persisted verifying attempt survives Runtime restart and same-Worker restore; the next transition request recovers it instead of creating a competing attempt.

Worker stop retains the Flow with the Worker session. Restoring the same Worker reconstructs the latest state from session extensions and the saved Profile still determines `feature.flow` eligibility. Worker deletion removes the owning Worker/session; Flow state is not implicitly handed to another Worker.

Workspace Server schema migration v26 removes the legacy `flow_instances`, `flow_transition_attempts`, and `flow_events` tables. Backend/Web visibility, when needed, is a bounded Runtime Worker projection rather than a second instance authority.

## Worker boundary

Flow invocation uses the normal Submit/Run segment vector rather than a Worker-create field:

```json
{
  "method": "run",
  "input": [
    { "kind": "flow", "selector": "builtin:coder-review" },
    { "kind": "text", "content": "Ticket 00001... implementation" }
  ]
}
```

Runtime accepts exactly one Flow segment only when the resolved Profile enables `feature.flow` and a Workspace client is available. The Worker asks Workspace authority only for an immutable source snapshot, creates the instance locally, replaces the Flow segment with the entered state's instructions, and commits that runtime state atomically with the remaining Submit segments before LLM execution. A Worker with an active Flow rejects the duplicate input without changing its local state or events.

The generic model-facing `WorkerSpawn` accepts `initial_submit: Vec<Segment>` and routes them unchanged through the shared Workspace spawn request into Runtime `CreateWorkerRequest.initial_input`. It does not have a parallel `initial_text` or a role-specific `SpawnCoder` wrapper. Backend derives the flat content projection from the canonical segment vector, validates Flow shape before spawn, and includes the segment vector in lifecycle idempotency fingerprints. Restoring the same Worker never replays spawn initial segments.

`RequestFlowTransition` accepts only:

```json
{ "reason": "bounded explanation" }
```

It does not accept Workspace, Flow, instance, Runtime, Worker, state, transition, or target identifiers. `RuntimeFlowCoordinatorClient` reads and persists only the current Worker's local `FlowRuntimeState`; no Workspace mutation client participates in transitions. Profile enablement makes the capability eligible but does not create or select an instance; without an active instance the transition request fails closed.

The transition tool begins or recovers a locally persisted attempt, runs one internal verifier, and resolves the typed verifier outcome into the Worker-owned state. The resulting state and next-state instructions are committed in the normal tool result history.

## Internal verifier authority

The internal verifier receives:

- one immutable snapshot of committed parent-session entries through `session-explore`;
- the captured current state, request reason, and complete outgoing condition list;
- `FinishFlowVerification`;
- when the parent has a Workdir session, only `Read`, `Glob`, and `Grep` backed by a `ReadOnlyWorkdirSession` capability-reducing wrapper.

It does not receive Workspace, Ticket, Memory, Worker-management, write/edit, Bash, or command authority. The read-only wrapper reports only read capabilities, rejects mutation/command operations, and closing it does not close the parent's source session.

`FinishFlowVerification` accepts exactly one `met | not_met | indeterminate` result and a bounded rationale for every transition id in the attempt. Missing, unknown, or duplicate ids are rejected before a result is recorded. A prose-only internal Worker completion is a failed verifier outcome, not a successful transition.

## Separation from the role-owned loop

This state graph does not implement Coder/Reviewer lifecycle, state-entry side effects, deterministic condition providers, or arbitrary Flow state data. The downstream role-owned loop submits the built-in Coder Flow segment, follows entered-state instructions, and uses Flow events plus typed review/repository evidence for its higher-level completion decisions.
