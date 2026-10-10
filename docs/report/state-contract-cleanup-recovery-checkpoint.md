# State-contract cleanup completion and recovery record

The interrupted cleanup has been restored, integrated with `develop` at
`72ce817de4592e8c9703e0725dd705f46cba63aa`, completed, and validated. This record
supersedes the earlier incomplete restart checkpoint. No live Server, Runtime,
or user database was stopped or migrated as part of this work.

## Recovery and integration

- Workdir attachment: `yoi-origin`; branch: `hare/develop`.
- Preserved the Workdir delegation fix (`7321d6c7`), asynchronous cleanup fix
  (`dc8e586a`), and the T-731/T-732 changes integrated by `72ce817d`.
- Applied stash `dd31c0ecd41d2aca1ef29889a5194fb4499cb149` once after merging
  develop. It remains as recovery evidence; do not apply it again over this work.
- Older stash `a386a9e2943d53f331a555a8c255741bc004c419` also remains untouched.
- Direct implementation/review SubWorkers were collected after their committed
  sessions and concrete changes/test evidence were inspected.

## Completed contracts

State/update ordinals have been replaced by content digests, key fingerprints,
binding/trust/connection IDs, immutable mutation IDs, and observation tokens.
Schema/protocol versions, ordered events, command IDs, and local asynchronous
invalidation fences remain distinct contracts and were not indiscriminately removed.

- Config source/history preserves separate source/toolchain/projection evaluations.
  Cache identities occupy a namespace forbidden to real virtual paths, including
  JSON-deserialized paths. Native and generated WASM regressions cover the boundary.
- Job/content migration retains completed results and cleanup/replay evidence.
  Proven reserved-only work can fail definitively; uncertain execution is not
  silently admitted again. Historical Flow definitions are validated, not recompiled.
- Drive preserves old receipt fingerprints and validates old Update/Relocate/Delete
  replays against frozen evidence rather than granting authority from old ordinals.
- SSH migration reseals envelopes under actual mutation receipts. Terminal Workdir
  history whose credentials were deleted remains non-authorizing archival evidence.
  INSERT/UPDATE guards prevent that archive from acquiring candidate/lease authority.
- Runtime binding verification ties together binding/trust identities and actual
  public-key fingerprints. Worker command-ID replay is separate from admission-time
  state. Ticket checker intent IDs include the complete immutable request.

## Migration safety and review corrections

- Uncertain Worker removals (`executing`/`failed`) block **before any retained
  migration runs**, including ordinary Server open and migration CLI entrypoints.
  Tests start at schema84, verify unchanged schema/receipt after rejection, then
  reconcile with the old contract and upgrade successfully.
- SSH dry-run uses the original database's secret context while validating an
  in-memory backup. Actual encrypted-envelope tests prove dry-run non-mutation,
  formal migration, new-AAD decryption, and unchanged master-key material.
- Interrupted signing identity provisioning resumes the same reserved key/material.
  Frozen old receipt validation precedes operation-key/fingerprint conversion;
  activation and migration failures roll back. Completed and create receipts are
  also covered.
- The old reservation CHECK constraint is removed through a transactional rebuild
  retaining other columns, constraints, indexes, triggers, and incoming/outgoing
  foreign keys. Unknown dependencies are not broadly rewritten.
- Fresh/upgraded metadata comparison covers 36 state-contract tables: columns,
  defaults, PK/FK structure, index columns, and triggers. It caught and repaired a
  missing history DEFAULT and retained-chain reservation lookup index. This is not
  a general SQL-equivalence proof of all CHECK expressions or index predicates;
  separate behavior/rollback tests cover the relevant authority constraints.
- Independent data and Server reviews found the above defects; follow-up reviews
  confirmed their resolution without additional blockers in the reviewed scope.

## Final validation

- Root `cargo check`: PASS.
- All 21 changed Rust crates tested together: **3,795 passed**, **3 ignored**,
  zero failures, including doctests. The Server contributes 809 library tests and
  11 CLI tests (one library test ignored).
- `server-api --features typescript`: **96 passed**.
- Additional T-731/T-732 integration coverage: yoi, workdir, tools, fs-operation
  tests passed separately.
- Web standard suite: **589 passed**; component suite: **315 passed / 33 files**.
- Web type check: zero errors/warnings; production build passed (chunk-size warning).
- Config WASM parity includes 18 passing cases, including literal-cache-key collision
  and JSON URI-path refusal. Generated JS/types/WASM were compared with the pinned
  wasm-bindgen 0.2.117 toolchain. OpenAPI/API/protocol generated checks passed.
- `cargo fmt --all -- --check` and `git diff --check HEAD`: PASS. Included frozen
  Rust migration/test files were formatted explicitly as well.

Local evidence logs (ignored build artifacts, not part of stash/history):
`target/revision-final-combined-rust-tests.log`,
`target/revision-final-typescript-tests.log`,
`target/revision-final-root-check.log`,
`target/revision-final-web-{tests,component,check,build}.log`,
`target/revision-virtual-path-web-{config-tests,final-check}.log`,
`target/revision-web-final-{artifacts,normalized-artifacts}.log`, and
`target/revision-schema-equivalence.log`.

## Deployment and separate findings

Deployment/restart remains user-operated. Take the normal database/key-material
backup before migration. Resolve uncertain old Worker removals with the old Server
before attempting cutover. Existing migrated Runtime bindings become configured;
re-enrollment/verification requires the actual Runtime-issued trust ID from
`trust-workspace add/show`, not an invented value or public-key fingerprint.

Previously identified Server-only capability revocation and missing enrollment
identity in Runtime-to-Server source proofs predate this cleanup. They are not
claimed fixed here. The separately recorded Runtime HTTP-only feature dependency
issue also remains outside this cleanup.

Workdir operations intermittently returned HTTP 401 during the work. Failed calls
were not treated as successful edits/tests; subsequent reads and final combined
validation established the recorded result. No authorization or live-process
workaround was used. The default wasm-bindgen CLI had a schema-version mismatch;
a matching CLI was installed only under ignored `target/revision-wasm-tools`, with
no global CLI change or additional change to the restored Cargo.lock.
