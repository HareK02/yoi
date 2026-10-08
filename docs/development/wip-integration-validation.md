# Scoped WIP integration validation

Adoption: wip-rs `1cbe03b49e48dd7e0be28b76fbc3c932f8920a36`, canonical
wip-reference `6086f3c5ef10aa1464ce9c824750d7f667217bfd`. See
[`vendor/wip-rs/README.md`](../../vendor/wip-rs/README.md) for provenance and
regeneration. All four WIP packages are committed path dependencies; Cargo
metadata resolves exactly one Protocol type source and no old registry WIP package.

## Executed checks

- `cargo test -p worker --locked --offline`: **888 unit + 110 integration tests
  passed**, no ignored/failed tests. This includes ordinary Tools mode, Feature
  permission/revocation, restore, session/history and controller integration.
- Narrow development checks: `wip::binding` (25), `wip::provider_tests` (6), existing
  `wip::tests` and workdir projection tests (43), checkout transports/providers
  (20), workspace-config production/authority tests (19). These are also included
  in the complete worker run.
- Official packages, using `cargo test --offline --manifest-path
  vendor/wip-rs/<package>/Cargo.toml --target-dir target/wip-upstream`:
  Text View **16 public signature/token/golden tests + 4 doctests**;
  Client **63 adapter/runtime tests**;
  HTTP **38 tests + 1 doctest**;
  Protocol **22 tests + 1 doctest**. Vendored source and tests are unmodified.
- Root `cargo check --locked --offline` passed, including normal TUI, CLI,
  Runtime and Workspace Server compile closure.
- `cargo fmt --all -- --check` and `git diff --check HEAD` passed.
- `node scripts/verify-wip-vendor.mjs`: all **43** Cargo-checksummed upstream files
  passed. No custom renderer or lexer replaces official signatures.
- `nix build --no-link .#yoi.cargoDeps` passed with the updated registry dependency
  hash. This validates credential-free dependency acquisition, not a full Nix
  binary/image build or deployment update.

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

Existing checkout tests retain commit-time captured Workdir/connection/validator
checks, alias reuse/restore fencing, inode replacement rejection, authorization,
shared prior-read tracking, exact post-operation validators, cooperative
cancellation and unknown/lost/malformed responses without retries. Existing native
Ticket/Objective/Merge Request tests retain scoped Workspace authority and Feature
permission separation; compatibility Invoke retains original schema validation,
permission identity and ToolOutput/attachments.

## Downstream provider seams

- Public projection/contribution Interface fields now use
  `wip_protocol::InterfaceReference { scope, name }`.
- Contextual/self-scope references are explicit paths, not the old encoded `/@/`
  suffix. Their fetch uses one live Object/descriptor/validator snapshot and its
  optional Object ref. Checkout's ref-less publication remains legitimate; its
  existing captured connection and provider validators still fence execution.
- `WipRuntime::{tree, inspect, invoke}` is the object-centered integration seam.
  Inspect uses official summary-only signatures and typed rendering failures.
- `WipMountRegistry::mount_interface` adds a separate Interface namespace without
  replacing Object ownership or flattening same-named operations.
- FS indexable boundaries and directory.list remain provider policy. This change
  deliberately does not implement the separate T-714 boundary/list migration.
  The relevant transport/commit tests are `checkout_wip_tests.rs`,
  `checkout_http_tests.rs`, and `checkout_race_tests.rs`.
- Worldspace state is runtime-only. Historical opaque references in append-only
  session/tool history are not migrated into cache or parsed for a guessed scope;
  a restored Client starts with fresh observations and direct path Inspect.
