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

## Suggested investigation

Trace the credential lifecycle across Backend restart and Runtime Worker restore:

1. whether Backend rotates or recreates the credential record;
2. whether restored Worker execution receives the current plaintext credential rather than retaining an old environment/config bundle;
3. whether credential binding should remain stable across Backend restart or be explicitly refreshed before marking the Worker restored;
4. whether restore health should include a bounded Workspace API authentication probe.

Do not treat a current DB credential row alone as proof that the restored Worker possesses it.
