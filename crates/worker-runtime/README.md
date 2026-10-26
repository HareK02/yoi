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
and Feature invocation history. A saved system prompt is required for restore;
checkpoint receipts must match the Worker's typed receipt schema. Preserved records
must use canonical writer field shapes: unknown/duplicate fields (including nested
fields), discarded optional fields, and opaque Feature history payloads are rejected,
not stripped or rewritten. Portability/secret-marker checks do not replace
review of the supplied artificial conversation or its independently verified hashes.
The installer refuses any destination content other than initialized `auth.toml`;
interrupted publication is fail-closed, not an overwrite/recovery facility.

## Owned-process Restore checkpoints

The independent, nondefault `restore-checkpoints` feature exists only for explicit
process tests in the shared `tests/e2e` harness. Offline `test-support` does not enable
it. Normal binaries reject `--test-restore-checkpoint`; no environment variable or
HTTP endpoint activates this facility. Do not deploy a checkpoint-enabled binary
as a replacement for the default product.

A test-enabled Runtime accepts `--test-restore-checkpoint <config.json>` with a
bounded regular JSON file selecting a loopback socket, Worker ID, one stage and a
diagnostic nonce. The nonce is only a handshake label, never execution identity,
request authority or routing state. The one-shot stages are real prepared candidate
with `NotCommitted`, and durable `Committed` before activation/terminal receipt.
Before acknowledging, the existing bounded canonical reader validates the exact
Worker/operation/decision, syncs its file/directories and requires no saved receipt.
No pending JSON is fabricated and no lifecycle work is skipped.

The Runtime sends bounded newline-delimited JSON evidence, then waits at most eight
seconds for byte `R`. EOF, invalid reply, unavailable peer or deadline releases the
wait into the original operation. The Runtime-state mutex is not held across that
wait; existing per-Worker ownership still covers the actual candidate and cleanup.
The harness verifies the ack, kills/reaps only its owned child and inspects the
canonical aggregate offline before restarting the same roots/identity without the
checkpoint flag or another fixture install. These tests are a separate cfg/binary
lane from featureless product regression; they do not certify arbitrary OS failure,
external power loss or a real model call.
