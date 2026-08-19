# SubWorker spawn failed because the inherited profile required an unavailable control service

Date: 2026-08-19

## Observed behavior

While splitting a migration investigation into read-only Runtime and Server analysis, both `SubWorkerSpawn` calls failed before the child session started:

```text
install Internal Worker features: Worker feature installation failed:
builtin:worker-observation: required service requirement is not available:
builtin:worker.control
```

The requested children used `builtin:coder` with read-only scopes. No child was created and no delegated work ran.

## Impact

A parent Worker with the SubWorker tools available cannot necessarily spawn a catalog profile whose transitive features require `builtin:worker.control`. The failure occurs at profile feature installation rather than being rejected when the profile is selected or omitted from the available SubWorker profile choices. The parent must continue the investigation without context splitting.

## Expected behavior

The SubWorker spawn layer should either install the parent-owned `worker.control` service before resolving dependent child features, provide a SubWorker-compatible profile projection that does not require unavailable Workspace Worker control, or reject the profile choice up front with an actionable capability diagnostic. A read-only delegated scope must remain read-only; satisfying the service dependency must not widen filesystem or Workspace authority.
