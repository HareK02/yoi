# T-740 implementation and validation

## Result

Classification foundation is implemented independently of case-state migration and the final Web UI. Contract/design and T-739/T-743 integration notes: `docs/design/workspace-ticket-tags.md`.

Changed boundaries:

- `ticket`: stable Workspace tag identity, normalized display-name uniqueness, tag catalog mutation/list, transactional multi-membership replacement, observation comparisons, identity-bound exact receipts, historical snapshots, schema v8 migration/verification, and projections.
- `server-api` / `client`: typed scoped routes and client methods, list/detail/query tag fields, AND/OR/NOT ID filters, OpenAPI and browser type generation.
- `workspace-server`: generated route registration, authenticated source attribution, SQL filtering before bounded candidate scans, authoritative projection recheck, suppression of execution/routing notifications, retained pre-classification schema verification and fixture correction, Workspace deletion purge inventory.
- `worker`: normal classification tools, native collection/item WIP operations, query filters, current identities/observation keys, and historical before/after snapshots in model-visible tag-change events.

## Executed checks

Tests were designed against `docs/development/rust-testing-strategy.md`. Narrow development checks included:

- `cargo test -p ticket --lib tags`: 14 tag authority tests passed, including name policy, duplicate/foreign/deleted references, stale observation, exact/incorrect replay, changed actor, assignment-context changes, multi-attach/remove, rollback, concurrent delete/attach and replay, no-op audit semantics, bounded lists/sets, and control-word tags across all existing states.
- `cargo test -p server-api --lib ticket_tag`: 4 passed; with `--features typescript`, 5 passed.
- `cargo test -p worker --lib feature::builtin::ticket::tags`: 7 passed, covering model-visible schemas, permission/authority errors, scoped HTTP operations and WIP binding.
- `cargo test -p worker --lib ticket_tag_history_projection`: passed.
- `cargo test -p yoi-workspace-server --lib ticket_tags_tests`: 3 passed, covering real router mutation/projection/filter, authentication/forged source/other Workspace rejection, actor-bound receipts, no state/content/assignment/grant/execution projection changes, and query pagination/filter-bound cursors.
- `cargo test -p yoi-workspace-server --lib fresh_and_upgraded_state_contract_schemas`: passed, including tag tables/constraints.
- `cargo test -p yoi-workspace-server --lib schema_v65_migrates`: passed after distinguishing retained intermediate schema from current classification schema.
- `cargo test -p yoi-workspace-server --lib explicit_purge_inventory`: passed.

Full changed-crate command passed:

```sh
cargo test -p ticket -p server-api -p client -p worker -p yoi-workspace-server --quiet
```

Unit suites at that point: client 57, server-api 92, ticket 120, worker 968, workspace-server 814 passed / 1 ignored. All included integration and doc-test targets also passed. Worker was subsequently rerun after adding the historical tag projection test (969 unit tests); its included integration/doc tests passed.

The one ignored Server test is the existing repositoryless Drive real-Web-adapter roundtrip, unrelated to classification. It was not executed.

- Root `cargo check`: passed, including normal TUI/Server compile closure.
- `cargo test -p server-api --features typescript`: 101 passed; doc tests passed. Existing ts-rs warnings about unsupported serde attributes remain.
- Canonical `export_openapi -- --check`: passed.
- `generate_repository_openapi_types`, `generate_runtime_api_types`, `generate_ticket_api_types`, `generate_workdir_api_types`, `generate_worker_launch_api_types`, `generate_companion_api_types` with `-- --check`: all passed. Other feature-gated type catalogs are protected by the full server-api TypeScript tests.
- `cargo fmt --all -- --check`, `git diff --check HEAD`: passed.
- `env -u LD_LIBRARY_PATH deno run --allow-read --allow-run=git tools/check-state-contract-names.ts`: passed, 0 unapproved / 0 stale exceptions; exception inventory unchanged.

Initial development/full-suite failures concerned in-flight trait implementations, stale generated artifacts, status-code expectations, and a retained schema fixture/intermediate verifier. They were corrected and the commands above rerun; the final changed-crate run had no failures.

## Not claimed / follow-up

No final Web tag UI, full browser/TUI/runtime E2E, real-model judgment test, workspace-wide test run, Nix/Docker build, or live production-data migration was performed. T-739 branch integration is not tested here: schema composition and contract regeneration must preserve the catalog, memberships, tombstones, receipts and tag audit/thread events. This branch is independently buildable on the existing Ticket-state model.

MR publication and trusted independent review are separate handoff evidence. This implementation does not merge the MR or complete/close T-740.
