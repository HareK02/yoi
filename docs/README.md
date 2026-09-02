# Yoi documentation

This directory contains maintained developer documentation for Yoi. Its job is to preserve design intent that is hard to recover from code alone.

It is not a dumping ground for external research, old plans, API inventories, or ticket history. Those belong in local notes, work item artifacts, code, or git history.

## Reading order

1. [`design/overview.md`](design/overview.md) — the system map.
2. [`design/durable-operations.md`](design/durable-operations.md) — cross-domain operation identity, checkpoints, retries, child operations, and disposition.
3. [`design/context-history.md`](design/context-history.md) — the highest-risk invariant: inputs that affect the model must be committed to history before they enter context.
4. [`design/worker-session-state.md`](design/worker-session-state.md) — Worker identity, replayable session logs, current metadata, and live process hints.
5. [`design/session-observation.md`](design/session-observation.md) — common session captures, `SessionEntryRef`, Memory evidence, and host-authorized Worker observation.
6. [`design/flow-state-graph.md`](design/flow-state-graph.md) — Workspace Flow sources, immutable revisions, transition attempts, and bounded internal verification.
7. [`design/profiles-manifests-prompts.md`](design/profiles-manifests-prompts.md) — reusable Profiles, resolved Manifests, and prompt resources.
8. [`design/tool-permissions-scope.md`](design/tool-permissions-scope.md) — tool policy and filesystem scope.
9. [`design/plugin-packages.md`](design/plugin-packages.md) — plugin package distribution, discovery, and enablement boundaries.
10. [`development/plugin-development.md`](development/plugin-development.md) — how to build, package, enable, and inspect Yoi Plugins.
11. [`design/memory-knowledge.md`](design/memory-knowledge.md) — generated memory and audit records.
12. [`design/workspace-kanban-orchestrator-runtime.md`](design/workspace-kanban-orchestrator-runtime.md) — how Kanban operations become durable orchestration events and backend-internal routing decisions.
13. [`design/workspace-runtime-docker.md`](design/workspace-runtime-docker.md) — the WebUI / Backend / Runtime split, Docker image layout, worker launch path, and workdir materialization boundary.
14. [`development/server-runtime-auth.md`](development/server-runtime-auth.md) — manual Workspace Server / Runtime public-key exchange and authenticated Runtime startup checks.
15. [`development/work-items.md`](development/work-items.md) — how project work is recorded and reviewed.
16. [`development/rust-testing-strategy.md`](development/rust-testing-strategy.md) — what Yoi Rust tests should prove, where they belong, and how to name them.
17. [`development/validation.md`](development/validation.md) — how to check changes.
18. [`development/workspace-schema-migrations.md`](development/workspace-schema-migrations.md) — how to preflight, apply, verify, and roll back control-plane SQLite schema changes.
19. [`design/standalone-agent-host.md`](design/standalone-agent-host.md) — in-process standalone Worker host の依存方向、authority、lifecycle、非目標。

## What belongs here

Keep documentation when it records a stable design boundary, a non-obvious rationale, or a workflow that future changes must respect.

Examples that belong:

- Why Worker metadata is not the session log.
- Why `Profile` and resolved `Manifest` are different layers.
- Why context-only event injection is forbidden.
- Why child Worker notifications are hints rather than completion proof.

## What does not belong here

Do not keep material only because it once helped a ticket.

Examples that should be removed or moved to `docs/.local/` / work item artifacts:

- External project comparisons.
- Provider API snapshots, prices, or model tables.
- Old implementation plans that are no longer the design authority.
- Public type or method inventories that drift with code.
- Debug notes that are useful only for one investigation.

`docs/.local/` is intentionally outside the maintained documentation surface.
