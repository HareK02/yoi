# Standalone subjektiv (T-709)

## Implementation design, recorded before implementation

Base: `13055d7ee18f9d29edc641d0bd0e57c3d034dfa0`, including T-704 and T-708. No Backend database, fake Workspace, local fallback, new daemon, or subjektiv-specific Internal Worker runner is introduced.

The operator-managed standalone state directory (existing CLI/XDG standalone state setting, not cwd or a repository file) is the local sharing boundary. `<state_dir>/subjektiv/` is a private directory. A persisted random storage-scope identity is issued here; Subject IDs use the common domain UUID issuer, never cwd/Profile/Worker name. The common Feature SQLite manager opens the subjektiv database under this root; it owns connection options, migration execution, close/reopen and transaction locking. The common subjektiv crate owns the existing schema/migrations/domain/CAS/receipt/surface rules. Existing Backend layout and physical schema remain unchanged. Job domain state is not embedded into Memory tables or Session text.

Subjects are explicitly created/listed in that state scope and selected at normal launch; enabled policy alone does not create or attach one. Each selected Subject has a separate OS advisory lock file under `<state_dir>/subjektiv/leases/`. The Host holds the exclusive lock for body execution and all delegated Jobs until confirmed shutdown. A second process cannot acquire the same lock. Process death releases the kernel lock, not a PID/Profile guess; uncertain in-process cleanup retains ownership. A separate Subject can run concurrently, with SQLite serializing common database transactions. Symlinks and non-private state nodes must be rejected at this boundary. Knowledge of an ID/path is not a model capability.

The selected Subject and local storage-scope identity are persisted in the standalone Worker record. Restore verifies them and reacquires the original Subject lock; resume never silently changes Subjects. Subject-free and disabled-policy launches retain the old path with no subjektiv persistence side effects. Ordinary SubWorkers/Reviewers receive no independent Subject/extraction connection.

The Feature receives an explicit Host-scoped subjektiv capability, separate from WorkspaceClient and Profile policy. Backend adapts its existing authenticated API operations; standalone binds one local store, Subject, Worker and committed Session authority. Session attribution is established by the Host, including Sessions without candidates. Reads use the existing committed public Session projection and standalone Session store, not a copy in the Memory database. No system prompt, hidden reasoning, uncommitted history or arbitrary filesystem/foreign-Subject access is granted.

Consolidation uses the existing generic standalone Jobs service and common request/result contracts. For a selected Subject the Host owns `<state_dir>/subjektiv/jobs/<SubjectId>/jobs.sqlite3` under that Subject lease, allowing durable intents/unknown outcomes/results to follow it across Workers. Subject-free Hosts retain the T-708 Worker-local location. Subjektiv selects the consolidation Profile. Job capabilities are explicitly attenuated to the immutable candidate batch, recall/decision/surface operations and public Session references; they never confer the Subject body's execution lease or ambient parent filesystem/Workspace tools. Common receipt/CAS rules handle partial apply and response loss; unknown execution is never automatically replayed. Surface failure leaves confirmed Memory intact. The generic runner owns cancellation, timeout, result binding, resource cleanup and shutdown; processing stops with the standalone process and pending durable work is recovered only on a subsequent explicit Host connection.

Validation must distinguish scripted model/Host and real test-owned process/lock runs from live-provider model quality. Root `cargo check`, changed-crate tests and narrow Backend/Feature/Job regression tests are required, plus format/diff checks. Concrete CLI/UI usage and recovery are documented in [standalone.md](../standalone.md).

## Implemented boundaries and validation

The validation counts and former API terminology below record T-709 at the cited source, not new executions. Current Memory changes and Job input-digest bindings follow [the store contract](subjektiv-store.md) and [the Job contract](standalone-jobs.md).

- `feature-storage` owns the shared SQLite connection policy/Feature manager; `subjektiv` owns the unchanged domain schema/migrations, Host-neutral API dispatch, and T-704 consolidation request/result/surface validation. Server modules remain thin adapters; Worker/standalone do not depend on Workspace Server.
- `worker::subjektiv` exposes a nonserializable live Host capability plus a separate attenuated consolidation capability. Backend creation still requires the actual authenticated client and bound snapshot. Local creation is explicit Host injection; unavailable Backend never falls back locally.
- The local catalog keeps private scope/language settings and common Feature storage under `subjektiv/`, separately from generic Subject Job ledgers. The common attribution table's existing `runtime_id` lookup key is the local storage namespace for local rows, not a running Runtime identity; public local evidence leaves `runtime_id`, Workspace, and Account fields absent. Worker IDs, Subject binding, and operator-managed scope must all match before Session content is read.
- The shared strict FsStore public index is also used for committed evidence admission. Stable references are intersected with visible committed entries; numeric hints accept the two existing capture producers. Open-Run evidence is deferred only before a request, never exposed by Session tools or recorded before commit. Persisted local human provenance is distinct from Backend Account provenance and maps to the existing public `HumanInput` class.
- Standalone integration tests use real Hosts/SQLite, scripted extraction/consolidation/surface models, and test-owned second processes. Nine cases cover inert/disabled policy, reopen, saved-binding restore, cross-Worker recall/source discovery, exact explicit commit staging, normal extract/pointer → generic Job → CAS apply → resident surface, same-Subject cross-process exclusion/abnormal-exit recovery, and transport loss after apply followed by surface-only repair without duplicate immutable Memory change.
- Generic Job tests additionally cover immutable grant persistence, missing grants, foreign grants, mismatched input bindings and wrong-consumer grants, per-attempt/deadline revocation, cancellation, timeout, cleanup failure, and stale capabilities. Session adapter tests cover no-candidate discovery, foreign Subject/scope/Worker denial, bounded UTF-8 and JSON output, filter/generation-bound cursors, missing/corrupt storage, and hidden/uncommitted/partial-tail exclusion without observation writes.

Verified commands:

```sh
cargo test -p standalone --test subjektiv -- --test-threads=1
cargo test -p protocol -p feature-storage -p subjektiv -p manifest -p session-store -p standalone -p worker -p tui -p yoi -p yoi-workspace-server
cargo test -p worker-runtime --lib protocol_
cargo check
cargo check -p protocol --features typescript,json-schema
cargo check -p tui --features e2e-test
cargo fmt --all -- --check
git diff --check HEAD
```

The changed-crate run passed, including 836 Worker unit tests, 603 Workspace Server unit tests (authorization/singleton/T-704 regression), 48 standalone unit tests, and nine standalone Subject integration tests. Existing ignored tests remain ignored. TypeScript generation check reports existing unsupported serde-attribute warnings but succeeds. These commands prove mechanism/authorization/storage contracts, not live-model quality or full interactive terminal E2E behavior.
