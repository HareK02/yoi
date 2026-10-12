# runtime-api

HTTP contract for Worker Runtime management. Owns REST DTOs and generated client/server adapters. Runtime persistence, authentication, body limits, and operation permissions are implemented in their owning crates, such as worker-runtime.

## Contract entry points

- [`RuntimeApi`] — Runtime management operations
- [`WorkerRestoreRequest`] / [`WorkerRestoreCoordinationRequest`] — Restore intent and Backend preparation contracts
- [`LogicalWorkdirAttachment`] — Logical Workdir attachments determined by Workspace
- [`BackendJobExecutionBinding`] — Job/attempt execution identity fixed by the Host
