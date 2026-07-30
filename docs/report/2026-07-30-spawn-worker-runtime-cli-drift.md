# SpawnWorker failed because the active Runtime CLI rejected its launch protocol

Date: 2026-07-30

## Context

While implementing Ticket `00001KYS7YMN5`, the parent Worker delegated two read-only code-mapping tasks through `SpawnWorker`.

Both launches failed before a child socket appeared. The spawned process reported:

```text
yoi-runtime: unexpected positional argument `worker`

Usage: yoi-runtime [OPTIONS]
```

The active `yoi-runtime` binary accepts only its HTTP Runtime server options and auth subcommands, while the Worker-management layer attempted to start a child through a legacy `worker` positional command.

## Impact

- Delegated read-only investigation was unavailable.
- The parent Worker had to perform the code mapping directly.
- This is protocol/version drift between the Worker-management launch path and the active Runtime binary, not a failure in the delegated task or scope.

## Suggested improvement

Make the child-launch protocol explicit and versioned rather than invoking a user-facing Runtime CLI subcommand implicitly. At minimum, Runtime registration/health should advertise the supported spawn protocol, and `SpawnWorker` should reject an incompatible Runtime with a bounded compatibility diagnostic before attempting to create a child socket.

The active backend/runtime should also be rebuilt and restarted together after CLI/launch-protocol changes so long-running Worker-management processes do not retain an obsolete invocation contract.
