# Dogfooding Yoi

This repository is developed with Yoi itself. Dogfooding is valuable because it exposes orchestration, memory, TUI, and workflow problems under real use.

## Pre-restart gate

Never use the live dogfood Server or Runtime as the first startup test for a new binary. A dogfood restart is allowed only after this sequence succeeds:

1. Build the production entrypoints: `cargo build -p worker-runtime --bin yoi-runtime -p yoi-workspace-server --bin yoi-server`.
2. Run the focused and dependent tests for the changed contracts, followed by workspace-root `cargo check`, `cargo fmt --all -- --check`, and `git diff --check HEAD`.
3. Exercise the provisioning and operational checks in [Workspace ↔ Runtime authentication](server-runtime-auth.md) against isolated Server DB, Runtime data, and ports. The Workspace owner must create the binding, the Runtime operator must install the Workspace issuer bundle, and the challenge proof must become verified.
4. Have an external supervisor or operator restart Server and Runtime at the same generation. A Worker hosted by the target Runtime must never terminate its own Runtime.
5. Verify post-restart readiness through the Workspace Runtime projection, ping, Worker list/create, protocol subscription, and a Runtime-to-Server source-proof operation before treating the environment as healthy.

The former isolated startup shell harness depended on removed Server-global trust commands and is intentionally not a fallback smoke path. New automated startup coverage must provision the same Workspace-scoped binding and challenge authority used by production rather than recreating global trust or seeding private authority directly.

## What to record

When a tool limitation, workflow obstacle, or model-facing policy problem blocks work, record it under `docs/report/` or a work item artifact. Do not turn every minor annoyance into a maintained design doc.

A report is useful when it explains:

- what the agent/user tried to do
- what made the work unsafe, confusing, or slow
- what design boundary was missing
- what evidence was observed

For a Remote Runtime rollout, also record the source commit, binary generation, Server schema version, Runtime binding revision, Workspace key generation, and typed HTTP/WebSocket outcomes. A successful document response does not outweigh visible UI, console, or API errors.

## Runtime command caveat

After rebuilding and restarting during dogfooding, `current_exe()` can point at a deleted binary path. Use typed runtime-command configuration and the development-only `YOI_POD_RUNTIME_COMMAND` executable override rather than reviving shell-command overrides.

## Multi-Worker work

Use child Workers for scoped tasks and reviews, but keep orchestration decisions in visible project records. Do not merge, close, or clean up merely because a child notification arrived.

## Secrets and logs

Do not put secrets, private prompts, or ignored secret-like file contents into diagnostics, work items, docs, session logs, or model context. During broad audits, existence/path checks are enough unless the user explicitly asks to inspect content.
