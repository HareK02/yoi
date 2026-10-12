# worker

`worker` builds an `agen` Engine from a resolved Manifest and connects Worker input handling, Feature registration, Session persistence, and protocol handling. It uses scoped capabilities supplied by the Host rather than owning Workspace business records or managing physical Workdirs.

## Main entry points

- [`Worker`](src/worker.rs): Input admission, execution, and Session restoration.
- [`controller`](src/controller.rs): Worker control and protocol intake.
- [`feature`](src/feature.rs): Feature declarations, registration, and structured invocation.
- [`wip`](src/wip.rs): WIP namespaces, native/compatibility projections, and the Client/Host boundary.
- [`spawn`](src/spawn/mod.rs): Delegation and control of parent-owned Internal SubWorkers.
