# Dogfooding Yoi

This repository is developed with Yoi itself. Dogfooding is valuable because it exposes orchestration, memory, TUI, and workflow problems under real use.

## Pre-restart gate

Never use the live dogfood Server or Runtime as the first startup test for a new
binary. A dogfood restart is allowed only after this sequence succeeds:

1. Build the production entrypoints:
   `cargo build -p worker-runtime --bin yoi-runtime -p yoi-workspace-server --bin yoi-server`.
2. Run the focused and dependent tests for the changed contracts, followed by
   `cargo fmt --all -- --check` and `git diff --check HEAD`.
3. Run `scripts/isolated-startup-smoke.sh` from an external shell/process.
4. Inspect any failed run's retained `/tmp/yoi-isolated-startup-smoke.*` logs;
   do not restart dogfood until the cause is fixed and the smoke passes.
5. Have an external supervisor or operator restart Server and Runtime. A Worker
   hosted by the target Runtime must never terminate its own Runtime.
6. Verify post-restart readiness through the Workspace Runtime projection and a
   real restored Worker operation before treating the environment as healthy.

The smoke harness runs the normal `yoi-server` and `yoi-runtime` binaries using
separate `HOME`, `XDG_DATA_HOME`, `XDG_CONFIG_HOME`, temporary Git repository,
Server database, Runtime fs store, identity/trust material, and non-dogfood
ports. It fails if either port is already occupied, if state escapes the
temporary root, if a process exits unexpectedly, if Runtime readiness is not
visible through Server, or if startup logs contain a panic, migration collision,
or Worker execution restore failure. It also proves that a listening Server
without its configured Runtime is not readiness and restarts the isolated
Runtime once to exercise persistence reopen.

Override `YOI_SMOKE_SERVER_BIN`, `YOI_SMOKE_RUNTIME_BIN`,
`YOI_SMOKE_SERVER_PORT`, or `YOI_SMOKE_RUNTIME_PORT` only when a separate build
or port is intentionally under test. Set `YOI_SMOKE_KEEP=1` to retain successful
artifacts. Failed artifacts are retained automatically.

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
