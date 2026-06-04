# Decision: Task tools as trial built-in feature module

Use the Task tool group as a trial of the Feature API boundary, but keep the scope to an internal built-in module.

Rationale:

- `TaskCreate` / `TaskUpdate` / `TaskGet` / `TaskList` are already small and state-bounded around `TaskStore`.
- They do not need external plugin loading, network, filesystem, secrets, model notification authority, or custom UI.
- The feature registry slice already routes them through the registry; the remaining useful trial is to extract the concrete Task feature into a clean module boundary that future built-in modules can copy.

This ticket should not introduce a package loader, sandbox, or external-plugin permission model. It should validate the public API shape by making Task tools look like a normal built-in feature contribution: descriptor-declared tools, no host authorities, install through registry, normal ToolRegistry/PreToolCall behavior preserved.
