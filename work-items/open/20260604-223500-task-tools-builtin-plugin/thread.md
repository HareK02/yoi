<!-- event: create author: tickets.sh at: 2026-06-04T22:35:00Z -->

## Created

Created by tickets.sh create.

---

<!-- event: decision author: hare at: 2026-06-04T22:35:48Z -->

## Decision

# Decision: Task tools as trial built-in plugin

Use the Task tool group as a trial of the Plugin/Feature boundary, but keep the scope to a built-in plugin module.

Rationale:

- `TaskCreate` / `TaskUpdate` / `TaskGet` / `TaskList` are already small and state-bounded around `TaskStore`.
- They do not need external plugin loading, network, filesystem, secrets, model notification authority, or custom UI.
- The feature registry slice already routes them through the registry; the remaining useful trial is to extract the concrete Task feature into a clean module boundary that future built-in plugins can copy.

This ticket should not introduce a package loader or sandbox. It should validate the public API shape by making Task tools look like a normal built-in plugin/feature contribution: descriptor-declared tools, no host authorities, install through registry, normal ToolRegistry/PreToolCall behavior preserved.


---
