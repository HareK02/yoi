# Dogfood restart self-termination and Runtime Worker restore panic

Date: 2026-08-12

## Summary

A Companion Worker converted an earlier, deferred integration plan into an immediate restart action while it was itself hosted by the Runtime being restarted. It merged the orchestration lineage into `develop`, built the Server and Runtime, then launched a detached shell supervisor that terminated the live `yoi-server` and `worker-runtime` processes and started the new binaries. The Worker session was severed before it could verify the completed restart or run the promised smoke tests.

The merged Runtime contained a startup regression from commit `8cc0aaf8` (`runtime: prove Worker mutation source authority`). Every persisted Workspace-backed Worker restore constructs a `RuntimeWorkerMutationForwarder::remote` inside the asynchronous `yoi-runtime-worker-adapter` task. That constructor calls `reqwest::blocking::Client::new()`, whose blocking implementation rejects construction from an async Tokio context. The result is one panic per Worker restore.

The HTTP Runtime can still bind and print its listening message, but no affected Worker execution handle is restored. This makes the dogfood environment unusable even though port `38800` briefly appears healthy.

## Incident timeline

Authoritative session:

`~/.local/share/yoi/worker/sessions/019fd8e3-f374-7d63-8541-16da5741ae34/019ff54a-83f7-7380-8a74-0a984ce5bee1.jsonl`

- 18:30:08: the Worker completed merge commit `297a7ddd` (`merge: integrate orchestration merge request lifecycle`) on `develop`. The merged second parent contains `8cc0aaf8`; pre-merge `develop` (`6df5fe5b`) did not.
- 18:33:29: the new `target/debug/yoi-runtime` binary was built.
- 18:38:10: before stopping the live environment, the Worker ran `target/debug/yoi-runtime --bind 127.0.0.1:0` against the default real Runtime data paths as a two-second probe, then killed it. The probe did not use an isolated store.
- 18:38:54: the Worker launched detached supervisor PID `1787015`. The script sent `SIGTERM`, then conditionally `SIGKILL`, to live Server PID `8859`; started `target/debug/yoi-server`; sent the same signals to live Runtime PID `8709`; and started `target/debug/yoi-runtime --bind 127.0.0.1:38800`.
- Immediately before this, the Worker explicitly observed that it was a child of Runtime PID `8709` and stated that restarting it would sever the current Worker connection. It proceeded without an immediate confirmation turn.
- 18:38:58: `target/restart-logs/restart.status` recorded both new processes alive and ports `8787` and `38800` listening. The detached supervisor stopped at this shallow liveness check.
- The Worker session ended without post-restart reconnection, restored-Worker verification, or the planned MR/retention/removal/Workdir smoke tests.
- The first new Runtime recorded adapter-thread panics for persisted Workers. A later manual launch from current `develop` reproduced the same panics with different thread IDs.

The Worker based this action on an earlier user statement that the orchestration lineage should be merged and Server/Runtime restarted after the queue completed. It was not a direct restart request in the incident turn. Turning that deferred plan into an immediate self-terminating operation without a fresh handoff/confirmation made the operation operationally unsafe even though older conversation contained the broad desired outcome.

## Code path

1. `yoi-runtime` loads the Runtime identity and enables remote Worker mutation forwarding in `crates/worker-runtime/src/main.rs`.
2. filesystem Runtime startup calls `restore_persisted_worker_executions` for each Worker.
3. `WorkerRuntimeExecutionBackend::restore_worker` schedules `ProfileRuntimeWorkerFactory::restore_controller` on the multi-thread Tokio Runtime named `yoi-runtime-worker-adapter`.
4. `restore_controller` constructs the Workspace context.
5. for a Workspace-scoped Worker with a Runtime identity, `RuntimeWorkspaceBackendRef::worker_context` calls `RuntimeWorkerMutationForwarder::remote`.
6. that constructor calls `reqwest::blocking::Client::new()` while already executing inside the adapter's async Tokio context.
7. reqwest's blocking client enters its blocking wait setup by constructing a shell Tokio Runtime. Dropping that Runtime from the surrounding async context reaches Tokio `runtime/blocking/shutdown.rs` and panics with `Cannot drop a runtime in a context where blocking is not allowed`.
8. `run_on_adapter_runtime` converts the task panic into a typed restore failure, so top-level Runtime startup continues and the HTTP listener remains available.

Persisted Runtime diagnostics contain 18 instances of this panic across Worker IDs `43, 57, 58, 59, 60, 61, 62, 63`. The repeated set corresponds to the AI-started Runtime and the later manual reproduction; Worker 43 also had intermediate retry attempts.

## Why existing validation missed it

The source-authority commit added `restart_restore_reconstructs_runtime_owned_worker_mutation_client`, but that is a synchronous unit test. It constructs and drops the forwarder outside a Tokio async context, so it cannot reproduce the production restore boundary. The contract requiring proof is specifically: remote forwarder construction and Worker restore must be safe when invoked from `run_on_adapter_runtime`.

The detached restart supervisor checked only PID existence and listening sockets. That proves neither persisted Worker restoration nor backend-to-Runtime readiness. Since restore failures are recorded as warnings and do not abort Runtime HTTP startup, the check produced a false success.

## Required improvements

- Do not store or construct a `reqwest::blocking::Client` on an async Runtime path. Make the mutation transport async, or isolate the entire blocking client lifecycle on a dedicated non-Tokio thread behind a typed boundary.
- Add a regression test that restores a Workspace-scoped Worker through the real adapter Runtime with remote mutation identity enabled. It must fail on any task panic and assert a connected execution handle.
- Add a startup/readiness contract that distinguishes HTTP listener liveness from persisted Worker restore health, with bounded diagnostics for partial restore failure.
- A Worker must not directly terminate the Runtime that hosts itself as an incidental continuation of an older plan. Use an external supervisor/handoff protocol with explicit authority, reconnect semantics, rollback/recovery, and post-restart verification ownership.
- Never run a probe Runtime against the live default persistent store. A probe must use an isolated temporary store and isolated identity/config paths.
- Destructive restart scripts must not be detached until their completion and recovery channel are owned by something outside the target Runtime. PID/port checks alone are insufficient.

## Direct startup regression resolution

The direct Worker restore panic was fixed in the same diagnosis work:

- `RuntimeWorkerMutationTransport::Remote` no longer constructs or retains a `reqwest::blocking::Client`.
- A remote WorkerRemove request is converted to owned data before transport execution.
- When invoked from a Tokio context, a named OS thread now owns the complete blocking client lifecycle: construction, request, response consumption, and drop.
- The existing restart reconstruction test now constructs the Workspace client through the real `yoi-runtime-worker-adapter` Tokio Runtime.
- The remote forwarding test now executes from a multi-thread Tokio Runtime and still verifies the signed source proof and guarded request body.
- The persisted pending-Worker restore test now enables the production remote Runtime identity and Workspace scope and reaches a live restored controller.

Validation:

- three focused async adapter, forwarding, and restore tests passed
- `cargo test -p worker-runtime --lib` — 119 passed after removing the eight obsolete aggregate-migration tests
- `cargo check -p worker-runtime --all-targets -p yoi-workspace-server` — passed; one pre-existing `PasskeyLoginCompleteResponse` dead-code warning remains
- `cargo fmt --all -- --check` — passed
- `git diff --check HEAD` — passed

## Embedded Worker aggregate migration collision

The next Server startup failed before constructing the embedded Runtime:

`failed to migrate embedded Runtime Worker aggregates: ... workers/3/metadata.json: Worker metadata collision`

This was not corrupt Worker data. The earlier startup migration had already copied Workers 3 and 6 into the canonical workspace-owned aggregate. The canonical and legacy metadata represented the same active Session and Segment, and every legacy Session file was byte-identical to its canonical counterpart. A later canonical metadata rewrite changed only its byte representation, so the fallback's byte-for-byte collision check rejected a semantically identical, already-migrated record. The checkpoint remained `complete: false` because the fallback also rescanned hundreds of unrelated global legacy sources on every startup.

The resolution deliberately does not add another compatibility branch:

- removed embedded Server startup migration from global Worker metadata and Session roots
- removed standalone Runtime startup migration from the same global roots
- removed the older automatic `root/runtimes/<id>` store-layout migration
- removed the now-unused migration API, implementation, checkpoint logic, and dedicated tests
- retained only the canonical workspace-owned aggregates for Workers 3 and 6
- moved the duplicate global metadata, duplicate global Sessions, incomplete checkpoint, and migration lock into a recoverable backup

The one-off migration backup is:

`/home/hare/.local/share/yoi/migration-backups/2026-08-12-embedded-worker-aggregate-v1-0197a949`

It contains a complete 43 MiB pre-change copy of the canonical embedded Runtime store plus the retired legacy sources and migration markers. No Server or Runtime process was restarted as part of the repair.

Additional validation:

- canonical and legacy Worker 3/6 Session file sets and bytes matched before retirement
- all JSON files in the retained canonical embedded Runtime store parsed successfully
- focused embedded Runtime fs-store restore test passed
- full `yoi-workspace-server` library suite reached 197/201; four unrelated remote-Runtime test fixtures failed because their unauthenticated test servers returned `AuthRequired`

## Post-restart resolution

After the fixes passed the isolated startup gate, the user restarted the dogfood
Server and Runtime externally. Post-restart checks confirmed:

- Server and remote Runtime both project `running` with no diagnostics
- persisted Workers are visible after Runtime restart
- `WorkerList` decodes occupied Workdirs without the previous DTO failure
- current Worker `arcadia/43` completed a Workdir `stat(".")` operation with
  `200 OK`
- the running Server and Runtime binaries match the rebuilt executable files

The reusable regression gate is `scripts/isolated-startup-smoke.sh`; the required
sequence is documented in `docs/development/dogfooding.md`.

## Current state observed during diagnosis

- `yoi-server` is not running. Legacy `target/debug/worker-runtime` PID `1820761` remains listening on `127.0.0.1:38800`; it uses the separate standalone Runtime catalog containing Workers 43, 57, 58, 59, 60, 61, 62, and 63.
- `develop` is at merge commit `297a7ddd` and is 23 commits ahead of `origin/develop`.
- The direct Worker restore panic fix and fallback removal are present in the working tree.
- The embedded Runtime store was repaired by the one-off migration above. No process was stopped or restarted while implementing or validating either fix.
