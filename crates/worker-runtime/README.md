# worker-runtime

Manages Worker creation, stopping, and Restore, execution connections within the Runtime, and Workdir materialization. The Backend owns Workspace records and grants; the Worker controller owns model execution and input acceptance. Optional features provide HTTP/WebSocket and filesystem persistence.

## Reference index

- [`Runtime`]: lifecycle operation acceptance, Workspace boundaries, and execution connections.
- [`execution`] — execution backends and operation-specific results.
- [`working_directory`] — repository materialization, session resources, and safe deletion.
- [`workspace_issuer`] — Workspace signature verification and replay prevention.

## Offline stopped-Worker fixtures

The nondefault `test-support` feature exposes `test_support::StoppedWorkerTemplate`.
It is intentionally limited to current-schema, stopped, attachment-free artificial
conversations with one Segment, text history, and no pending execution/activation.
It does not add a product endpoint or change create/restore authorization.

1. With the source Runtime offline, call `StoppedWorkerTemplate::extract(worker_dir)`.
   The source must have its existing Runtime owner lock and identity. Extraction is
   read-only; the returned serde template excludes source auth, saved Manifest,
   request/config references, run directories, and Runtime operation history.
2. Initialize **new** Runtime identity and Workspace trust in a fresh destination
   using normal setup. Preserve the template's Runtime and Workspace IDs.
3. Call `template.install(&options, request, &resolved_manifest)` before starting
   the Runtime. Supply a fresh typed `CreateWorkerRequest` (including config/profile
   references and trusted memory settings) and `manifest::WorkerManifest` resolved
   for that environment. Its Worker name must equal `template.worker_name()`.
4. Start normally and use the ordinary explicit restore operation. Installation
   itself creates no execution and leaves the Worker stopped with a fresh guard.

The template preserves the original Session manifest/JSONL bytes, including stable
entry IDs, timestamps, and settled submission receipts. It rejects resumable queues
and Feature invocation history. Portability/secret-marker checks do not replace
review of the supplied artificial conversation or its independently verified hashes.
The installer refuses any destination content other than initialized `auth.toml`;
interrupted publication is fail-closed, not an overwrite/recovery facility.
