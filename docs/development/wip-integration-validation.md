# Scoped WIP integration validation

## Released dependency and resolution

The adopted Client, HTTP, Protocol and official Text View are **crates.io 0.2.0**,
with exact `=0.2.0` workspace declarations and registry checksums in Cargo.lock.
`cargo info <package>@0.2.0` confirmed all four published packages (Rust 1.88).
`node scripts/verify-wip-dependencies.mjs` checks actual locked/offline Cargo
resolution: all WIP packages use registry 0.2.0, each required package occurs
once, and Protocol has one source. No path/Git override or attached checkout is
used. The earlier unreleased 0.1.0 source snapshot and vendor verifier were
removed when T-713 thread sequence 26 specified 0.2.0.

Canonical contract reference: wip-reference
`6086f3c5ef10aa1464ce9c824750d7f667217bfd`. This reference documents the contract;
Cargo resolves the release, not that checkout or an unreleased Rust revision.

## Executed checks on registry 0.2.0

- `cargo test -p worker --locked --offline`: **894 unit + 110 integration tests
  passed**, no ignored/failed tests. This includes ordinary Tools mode, Feature
  permission/revocation, restore, session/history and controller integration.
- Narrow development checks: `wip::binding` (25), `wip::provider_tests` (12), existing
  `wip::tests` and workdir projection tests (43), checkout transports/providers
  (20), workspace-config production/authority tests (19). These are included
  in the complete worker run.
- Published SDK tests via `cargo test -p wip-client -p wip-http -p wip-protocol
  -p wip-text-view --locked --offline`: Text View **16 public signature/token/golden
  tests + 4 doctests**; Client **63 adapter/runtime tests**; HTTP **38 tests + 1
  doctest**; Protocol **22 tests + 1 doctest**. Tests run from the registry packages,
  not substituted source or removed vendor directories.
- Root `cargo check --locked --offline` passed, including normal TUI, CLI,
  Runtime and Workspace Server compile closure.
- `cargo fmt --all -- --check` and `git diff --check HEAD` passed.
- `nix build --no-link .#yoi.cargoDeps` passed with the updated registry dependency
  hash. This validates credential-free dependency acquisition, not a full Nix
  binary/image build or deployment update.

## Current target compatibility (preview, not integration)

Source runtime at `23cf6c4f882ce78985ba4bd673751cebe64e2d87` and current develop
`b5ea162f838fc36e364edec9ab6689292b59cffd` produce the conflict-free `git merge-tree`
preview tree `7ef3545b635cbdfc5e24658f6d1532e0f8b0947c`. An archive of this tree,
without a branch/ref change, passed root locked/offline Cargo check, all **91**
Server API TypeScript tests (including OpenAPI/checked-in generator freshness),
registry WIP resolution checks, and Nix dependency acquisition with the same hash.
T-722's API macros, Server API/Drive/Server source, OpenAPI and generated TS blobs
are unchanged from develop in the combined tree; no artifact overwrite or
regeneration is necessary for this source change. The later added same-name scope
regression changes tests only, not the validated runtime or shared contracts.

The initial archive check reused a Cargo target directory; returning to the older
main checkout exposed stale preview Server API artifacts. All local workspace
artifacts were invalidated, and a fresh main root check and complete **894 + 110**
Worker suite passed. Use an isolated target directory for future cross-tree checks.
No target push, integration branch, deployment, or live dogfood update was made.
Orchestrator readiness/integration must still resolve the then-current target.

## Contract evidence

`crates/worker/src/wip/binding.rs` tests exercise fresh/loading/stale/failed and
capacity-blocked acquisition, explicit refresh, superseded responses, retrieval
ownership cleanup, output limits, direct unseen-path Inspect, official unmodified
signature/path envelopes, structured Interface input and complete edge-only trees.
They invoke the registered three-tool surface for a search returning typed Entry
paths, inspect only a selected nonindexable path, dispatch the selected operation,
and verify that a later Inspect failure neither fails nor replays an earlier
successful operation. Interface acquisition failures are not empty operation lists.

`provider_tests.rs` exercises the actual Yoi adapter for root/self scope, optional
refs, copied scope_ref, different scopes with identical names/operations, shape vs
membership errors, validator/ref absence/mismatch and protocol failure precedence.
It replaces a dynamic scope Object identity, observes both stale sides after
InterfaceMismatch, reacquires without replay, and confirms direct NotFound ends
safe Interface reuse. An in-flight operation survives descriptor refresh using
its exact frozen Host and Client descriptors rather than the newer return type.

`ancestor_tests.rs` covers the independent review finding P1_CURRENT_ANCESTOR_SCOPE:
valid live ancestor refs that differ from registration placeholders, root `/` and
intermediate provider-owned ancestors, ref-less and ref-bearing scope deletion
without a prior scope Inspect, rejected cached dispatch and orphan observation,
replacement/republication without restoring old Interface registrations,
same-name scope replacement or loss of public ref rejecting copied old metadata
without replay (and later explicit Inspect/Invoke copying only the new optional ref),
shape/target/Object-validator/Interface-mismatch precedence, and frozen in-flight
ancestor Descriptor/handler/result validation after replacement. Valid Invoke
makes one provider publication call for the target, not a separate scope lookup.

Existing checkout tests retain commit-time captured Workdir/connection/validator
checks, alias reuse/restore fencing, inode replacement rejection, authorization,
shared prior-read tracking, exact post-operation validators, cooperative
cancellation and unknown/lost/malformed responses without retries. Existing native
Ticket/Objective/Merge Request tests retain scoped Workspace authority and Feature
permission separation; compatibility Invoke retains original schema validation,
permission identity and ToolOutput/attachments.

## Downstream provider seams

- Public projection/contribution Interface fields use
  `wip_protocol::InterfaceReference { scope, name }` from registry 0.2.0.
- `WipSubtreeProvider::publication` returns a `WipPublication` that captures target,
  current scope Object, Descriptor/validator and handler at one provider boundary.
  Mutable ancestors must not be resolved independently after selecting a target.
  `scope: None` ends usable publication, even without a public ref; it is not a
  ref-less published Object. Host checks and observation never substitute the
  root registration placeholder, and Interface fetch never falls back to retained
  provider registrations. Old registrations must end when their scope lifetime
  ends. Published ancestor scopes outside the subtree remain Host-owned.
- Contextual/self-scope publishers use `WipPublication::self_scoped` on their exact
  provider snapshot; references are explicit paths, not an encoded `/@/` suffix.
  Checkout and Workspace Config retain their captured provider/Backend execution
  and commit fences. Ref-less publication remains legitimate.
- `WipRuntime::{tree, inspect, invoke}` is the object-centered integration seam.
  Inspect uses official summary-only signatures and typed rendering failures.
- `WipMountRegistry::mount_interface` adds a separate static Interface namespace
  without replacing Object ownership or flattening same-named operations. It does
  not overlay stale registration metadata onto a live subtree provider.
- FS indexable boundaries and directory.list remain provider policy. This change
  deliberately does not implement the separate T-714 boundary/list migration.
  Relevant transport/commit tests are `checkout_wip_tests.rs`,
  `checkout_http_tests.rs`, and `checkout_race_tests.rs`.
- Worldspace state is runtime-only. Historical opaque references in append-only
  session/tool history are not migrated into cache or parsed for a guessed scope;
  a restored Client starts with fresh observations and direct path Inspect.
