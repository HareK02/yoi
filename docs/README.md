# Yoi documentation

This directory contains maintained developer documentation for Yoi. Its job is to preserve design intent that is hard to recover from code alone.

It is not a dumping ground for external research, old plans, API inventories, or ticket history. Those belong in local notes, work item artifacts, code, or git history.

## Reading order

1. [`design/overview.md`](design/overview.md) — the system map.
2. [`design/context-history.md`](design/context-history.md) — the highest-risk invariant: inputs that affect the model must be committed to history before they enter context.
3. [`design/pod-session-state.md`](design/pod-session-state.md) — Pod identity, replayable session logs, current metadata, and live process hints.
4. [`design/profiles-manifests-prompts.md`](design/profiles-manifests-prompts.md) — reusable Profiles, resolved Manifests, and prompt resources.
5. [`design/tool-permissions-scope.md`](design/tool-permissions-scope.md) — tool policy and filesystem scope.
6. [`design/memory-knowledge.md`](design/memory-knowledge.md) — generated memory, Knowledge, and audit records.
7. [`development/work-items.md`](development/work-items.md) — how project work is recorded and reviewed.
8. [`development/validation.md`](development/validation.md) — how to check changes.

## What belongs here

Keep documentation when it records a stable design boundary, a non-obvious rationale, or a workflow that future changes must respect.

Examples that belong:

- Why Pod metadata is not the session log.
- Why `Profile` and resolved `Manifest` are different layers.
- Why context-only event injection is forbidden.
- Why child Pod notifications are hints rather than completion proof.

## What does not belong here

Do not keep material only because it once helped a ticket.

Examples that should be removed or moved to `docs/.local/` / work item artifacts:

- External project comparisons.
- Provider API snapshots, prices, or model tables.
- Old implementation plans that are no longer the design authority.
- Public type or method inventories that drift with code.
- Debug notes that are useful only for one investigation.

`docs/.local/` is intentionally outside the maintained documentation surface.
