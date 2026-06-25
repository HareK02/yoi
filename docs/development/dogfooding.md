# Dogfooding Yoi

This repository is developed with Yoi itself. Dogfooding is valuable because it exposes orchestration, memory, TUI, and workflow problems under real use.

## What to record

When a tool limitation, workflow obstacle, or model-facing policy problem blocks work, record it under `docs/report/` or a work item artifact. Do not turn every minor annoyance into a maintained design doc.

A report is useful when it explains:

- what the agent/user tried to do
- what made the work unsafe, confusing, or slow
- what design boundary was missing
- what evidence was observed

## Runtime command caveat

After rebuilding and restarting during dogfooding, `current_exe()` can point at a deleted binary path. Use typed runtime-command configuration and the development-only `YOI_POD_RUNTIME_COMMAND` executable override rather than reviving shell-command overrides.

## Multi-Worker work

Use child Workers for scoped tasks and reviews, but keep orchestration decisions in visible project records. Do not merge, close, or clean up merely because a child notification arrived.

## Secrets and logs

Do not put secrets, private prompts, or ignored secret-like file contents into diagnostics, work items, docs, session logs, or model context. During broad audits, existence/path checks are enough unless the user explicitly asks to inspect content.
