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
- FS indexable boundaries and directory.list are provider policy. T-714 now
  implements entrance-only trees and bounded typed discovery (details below).
  Transport/commit tests are `checkout_wip_tests.rs`, `checkout_http_tests.rs`,
  and `checkout_race_tests.rs`.
- Worldspace state is runtime-only. Historical opaque references in append-only
  session/tool history are not migrated into cache or parsed for a guessed scope;
  a restored Client starts with fresh observations and direct path Inspect.

## T-714 checkout discovery validation

Based on integrated T-713 at `f9d4c97f`, still using the exact released crates.io
0.2.0 SDK and official renderer. No alternate wire, renderer or upstream version
was introduced. The checkout projection design documents provider live pagination,
its ordering/cursor/byte ceilings, special-entry policy and the distinction between
indexable emptiness and empty filesystem directories.

Focused regressions:

```sh
cargo test -p worker --lib checkout_wip
cargo test -p worker --lib checkout::race_tests
cargo test -p worker --lib checkout_http_tests
cargo test -p worker --lib ancestor_tests
cargo test -p tools --lib checkout_list
cargo test -p fs-operation --test list_pagination
cargo test -p workdir --test list_pagination
```

Host depth-zero omission / positive-depth `children: []`, empty/small/1100-entry
folders, direct deep roots and zero provider content-List calls during observation
are tested independently of Client output. Registered model tools exercise entrance
self-scope and deep target ancestor-scope, paginated List/Glob/Grep typed entries,
lazy selected Inspect with official signatures/path context, and Read without
COMMAND. Acquired paths never become tree edges; a stalled result observation is
never awaited and disappearance only fails a later explicit Inspect. Remote HTTP
checks verify List continuation on the wire with no ordinary List fallback or
response credentials/host-path leakage. Provider tests cover scoped External and
nested cwd/output_root rebasing, nonrecursive enumeration/read authority, ignore
policy, live mutations, byte bounds, cancellation and malicious cursor/result DTOs.

`WipSubtreeProvider::interface_target` is a routing hint for target-qualified local
names under ancestor scopes. The default retains self/shared-scope behavior. Host
checks provider ownership and resolves **one** coherent current target/scope
publication; it never treats the hint as authorization or fetches a scope in an
independent mutable lookup. Focused Host tests reject forged/outside/prefix/missing
hints, ref-less deletion and absent membership while preserving existing ancestor
ref/validator and in-flight snapshots. Actual checkout tests reject sibling/prefix
scopes, supplied refs when current publication has none and stale alias generations;
ordinary content updates do not terminate the entrance Interface lifetime.

Completion validation:

- Root `cargo check` passed.
- `cargo test -p fs-operation -p workdir -p tools -p worker` passed: FS **58 unit +
  6 pagination**; Workdir **125 unit + 5 pagination**; Tools **84 unit + 39
  integration + 1 doctest**; Worker **901 unit + 110 integration**.
- `cargo test -p workdir --features http-client` passed (**125 + 5**).
- `cargo test -p yoi-workspace-server` passed (**691 unit + 11 CLI**).
- `cargo test -p server-api` passed (**83**, including checked-in OpenAPI/TypeScript
  freshness). No generated API artifact changes were needed.
- `node scripts/verify-wip-dependencies.mjs` confirmed one crates.io 0.2.0 Protocol
  source and all four exact SDK packages. `cargo fmt --all -- --check` and
  `git diff --check HEAD` passed.

The first dependent Server test build exposed predecessor fixtures still using
removed `discover`/opaque strings/private `call`. Those tests now select structured
references from actual path Inspect and use public Tree/Inspect/Invoke; the complete
Server suite above includes configuration integration and the typed Workdir proxy.
Its dev-only Protocol dependency reuses the same locked SDK source; the resolved
package/version set is unchanged.

No full workspace test, full Nix/image build, deployment, target integration or
live dogfood update is claimed. Target movement remains Orchestrator authority.

### Post-success connection-loss review regression

The first independent source review identified `POST_COMMIT_CONNECTION_FENCE`:
lazy result publication had propagated a connection mismatch as a pre-effect
rejection even after successful Write/Edit/Create. The production result
publication boundary now maps post-success connection loss for mutations to
`OutcomeUnknown`, while Read/List/Glob/Grep keep their validator mismatch. Lazy
entry mapping is unchanged; there is still no result-wide acquisition.

`committed_checkout_mutation_losing_connection_is_unknown_not_rejected_or_replayed`
executes real checked Tools/provider mutations, deterministically detaches after
the routed execution guard finishes but before the same production publication
boundary, then passes the error through WIP Invoke/Client. It checks committed
file contents, unknown audit (not rejection), and exactly one dispatch for each
Write/Edit/Create. `readonly_checkout_result_losing_connection_keeps_validator_mismatch`
protects the non-mutating error policy. No sleep or production test hook is used.

After this fix, all **9** checkout race tests and the complete Worker suite
(**903 unit + 110 integration**) passed, as did root Cargo check, formatting,
diff checks and released SDK resolution verification. The earlier **901** Worker
count above describes the pre-review combined-crate run.
