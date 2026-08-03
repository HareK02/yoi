# Restored Worker retained unusable Workspace credential after Backend restart

## Observed

After restarting the Runtime and Server, Worker 30 restored and continued executing normal turns, but every typed Ticket operation failed with:

```text
Worker Workspace authentication failed: missing Runtime Workspace credential
```

The Server control-plane DB contained a current active `worker_workspace_credentials` row for the same Workspace, Runtime, and Worker identity, while the restored Worker/tool request did not authenticate with it. The failure prevented the required ticket-first workflow for an auth/storage regression even though the Worker itself remained live.

## Impact

- Restore can appear healthy because model turns still execute while Workspace-authority tools are unusable.
- A Worker cannot report or ticket the restore regression through the intended typed authority.
- The failure is easy to misattribute to the Browser multiplexer; in this incident Browser authentication/bootstrap was a separate issue.

## Resolution

The per-Worker bearer credential was removed rather than adding restore-time secret rotation and reinjection. Worker Workspace requests now carry the Runtime/Worker identity binding established by Runtime, and Server verifies that identity against the current Runtime catalog before applying Ticket role/assignment gates.

The removal also deletes token mint/rotate/revoke/refresh behavior, live Worker token replacement, and the control-plane credential table. Runtime/Server trust remains the security boundary; Worker role checks remain the accidental-misuse gate.
