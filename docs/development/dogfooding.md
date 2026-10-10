# Dogfooding Yoi

This repository is developed with Yoi itself. Dogfooding is valuable because it exposes orchestration, memory, TUI, and workflow problems under real use.

## Pre-restart gate

Never use the live dogfood Server or Runtime as the first startup test for a new binary. A dogfood restart is allowed only after this sequence succeeds:

1. Build the production entrypoints: `cargo build -p worker-runtime --bin yoi-runtime -p yoi-workspace-server --bin yoi-server`.
2. Run the focused and dependent tests for the changed contracts, followed by workspace-root `cargo check`, `cargo fmt --all -- --check`, and `git diff --check HEAD`.
3. Exercise the provisioning and operational checks in [Workspace ↔ Runtime authentication](server-runtime-auth.md) against isolated Server DB, Runtime data, and ports. The Workspace owner must configure the Runtime endpoint and pinned Runtime key, the Runtime operator must install the Workspace issuer bundle, and actual signed requests must authenticate. A connection-test result describes only that attempt; no stored verification state is a readiness prerequisite.
4. Have an external supervisor or operator restart Server and Runtime from the same validated source commit/build artifacts. A Worker hosted by the target Runtime must never terminate its own Runtime.
5. Verify post-restart readiness through the Workspace Runtime projection, ping, Worker list/create, protocol subscription, and a Runtime-to-Server source-proof operation before treating the environment as healthy.

The former isolated startup shell harness depended on removed Server-global trust commands and is intentionally not a fallback smoke path. New automated startup coverage must configure the same Workspace-scoped connection and explicit Runtime-side Workspace trust used by production, then exercise per-request authentication rather than recreating global trust or seeding a past verification result.

## What to record

When a tool limitation, workflow obstacle, or model-facing policy problem blocks work, record it under `docs/report/` or a work item artifact. Do not turn every minor annoyance into a maintained design doc.

A report is useful when it explains:

- what the agent/user tried to do
- what made the work unsafe, confusing, or slow
- what design boundary was missing
- what evidence was observed

For a Remote Runtime rollout, also record the source commit, binary artifact digest, Server schema version, Runtime `binding_id` and pinned key fingerprint, the current Workspace `key_id` and public fingerprint, the Runtime-side trust registration/revocation, and typed HTTP/WebSocket outcomes. Do not copy a Runtime trust ID into the Server connection configuration or treat a successful connection test as persisted authentication. A successful document response does not outweigh visible UI, console, or API errors.

## Runtime command caveat

After rebuilding and restarting during dogfooding, `current_exe()` can point at a deleted binary path. Use typed runtime-command configuration and the development-only `YOI_POD_RUNTIME_COMMAND` executable override rather than reviving shell-command overrides.

## Multi-Worker work

Use child Workers for scoped tasks and reviews, but keep orchestration decisions in visible project records. Do not merge, close, or clean up merely because a child notification arrived.

## Secrets and logs

Do not put secrets, private prompts, or ignored secret-like file contents into diagnostics, work items, docs, session logs, or model context. During broad audits, existence/path checks are enough unless the user explicitly asks to inspect content.
