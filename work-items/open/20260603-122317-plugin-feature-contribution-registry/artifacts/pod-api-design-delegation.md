# Delegation intent: Plugin base Pod API design

## Intent

Design the public Pod-side API that will serve as the base for Plugin / Feature contributions. The result should make Plugin-provided or built-in extension modules easy to register cleanly without adding ad hoc Pod processing paths.

This is a design task, not an implementation task. The output should be a concise but concrete design document suitable for turning into implementation tickets or acceptance criteria for `plugin-feature-contribution-registry` and `hook-public-surface-hardening`.

## Background

The current direction is that feature state remains owned by the feature/extension module, while interaction with Pod happens through existing durable host surfaces:

- Tools
- Hooks
- notifications / events / durable history append paths

The concern is that adding WorkItem, MCP, memory, plugin, and other capabilities without a common registry will create many unrelated Pod-specific insertion points. The Plugin system should establish a common contribution and authority boundary, even for built-in features.

`hook-public-surface-hardening` is being implemented separately to make public Hook actions safe before plugin exposure.

## Design question

What should the clean public API look like for a feature/plugin module that wants to contribute capabilities to a Pod?

The design should answer:

- What API types should extension modules use to declare/register capabilities?
- What belongs in a pure descriptor vs a runtime install callback?
- How should Tools, Hooks, and notifications be represented in the same public surface?
- How should capability request / host grant / diagnostics be expressed?
- What state should the feature keep itself, and what state may Pod keep?
- What must be impossible through this API?
- Where should the API live initially, and what parts should be movable to a future `plugin`/`extension` crate?

## Required constraints

- Public API must not let features/plugins mutate prompt context or session history invisibly.
- Model-visible additions must go through durable host paths: tool result, committed history append, explicit notification/history append, or user-visible event path.
- Public Hook contribution must depend on the safe Hook surface after `hook-public-surface-hardening`.
- Tool contributions must use the normal ToolRegistry / PreToolCall permission / history result path.
- Feature registry must install into existing Pod/Worker surfaces; it must not create a parallel Pod runtime path.
- Capability grant is host-controlled. A feature may request capabilities but must not assume them.
- Built-in features and future external plugins should fit the same shape.
- Avoid designing package distribution, WASM execution, or MCP implementation details beyond the minimal runtime-kind placeholders needed for the API.
- Avoid broad refactors of Pod/Worker crate boundaries unless needed to explain a clean API boundary.

## Files / records to read

Tickets:

- `/home/hare/Projects/yoi/work-items/open/20260603-122317-plugin-feature-contribution-registry/item.md`
- `/home/hare/Projects/yoi/work-items/open/20260603-122317-hook-public-surface-hardening/item.md`
- `/home/hare/Projects/yoi/work-items/open/20260531-010005-plugin-extension-surface/item.md`
- `/home/hare/Projects/yoi/work-items/open/20260601-031252-builtin-work-item-intake-routing/item.md`

Code:

- `crates/pod/src/hook.rs`
- `crates/pod/src/ipc/interceptor.rs`
- `crates/pod/src/controller.rs`
- `crates/pod/src/pod.rs`
- `crates/pod/src/permission.rs`
- `crates/llm-worker/src/tool.rs`
- `crates/llm-worker/src/interceptor.rs`
- `crates/tools/src/lib.rs`
- `crates/pod/src/workflow/mod.rs`

## Expected output

Write a design document to:

`/home/hare/Projects/yoi/work-items/open/20260603-122317-plugin-feature-contribution-registry/artifacts/pod-api-design.md`

Use this structure:

1. Summary recommendation
2. Current relevant Pod/Worker surfaces
3. Proposed public API shape
   - types/modules
   - example registration snippet
   - Tool contribution
   - Hook contribution
   - notification/event contribution
   - capability request/grant/diagnostics
4. State ownership model
5. Safety invariants / forbidden operations
6. Placement and crate-boundary recommendation
7. Migration path from current built-in registrations
8. Impact on WorkItem / MCP / plugin distribution follow-ups
9. Open questions / risks

## Non-goals

- Do not edit source code.
- Do not implement tests.
- Do not create a worktree.
- Do not close or modify tickets except writing the requested design artifact.

## Completion report

Report:

- whether the artifact was written
- the recommended API placement
- the highest-risk API decision
- any blockers that require parent/user decision
