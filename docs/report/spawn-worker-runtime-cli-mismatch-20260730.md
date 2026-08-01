# SpawnWorker fails against current `yoi-runtime` CLI

Date: 2026-07-30

## Symptom

A `SpawnWorker` tool call failed before creating its socket. The child process stderr was:

```text
yoi-runtime: unexpected positional argument `worker`

Usage: yoi-runtime [OPTIONS]
```

The Worker-management layer appears to invoke the configured Runtime executable with a legacy positional `worker` subcommand, while the current `yoi-runtime` binary starts the Runtime HTTP service directly and no longer accepts that subcommand.

## Impact

Read-only review delegation was unavailable during the Workspace credential repair implementation. Work had to continue in the parent Worker without a spawned reviewer.

## Suggested investigation

- Check the SpawnWorker process launcher command construction against the current `yoi-runtime` CLI contract.
- Ensure runtime executable discovery does not select the server binary when a dedicated Worker child executable/protocol entrypoint is required.
- Add an integration test that starts a spawned Worker through the same command used by the Worker-management tool and waits for its socket.
