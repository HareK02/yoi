# Rust testing strategy for Yoi

Yoi tests protect behavior that Rust's type system cannot prove and deliberate compile-time API constraints that must not regress. A test must name the contract it protects and fail for one understandable reason.

This document adapts the general Rust distinction between unit and integration tests to Yoi's architecture: `yoi` CLI, Workspace Server, Worker Runtime, Worker tools, SQLite authorities, Web/API DTOs, and local/runtime process boundaries.

References used while writing this policy:

- Rust Book, "Test Organization": unit tests are focused and may test module-private behavior; integration tests use public APIs the way external code does.
- Cargo Book, `cargo test`: package, target, and feature selection determine which tests Cargo builds and runs.

## What Rust types already guarantee

Do not write tests that only re-prove these facts for values already constructed as Rust types:

- enum values are one of the declared variants;
- required struct fields exist;
- references obey Rust ownership and lifetime rules;
- `Option<T>` is present or absent explicitly;
- exhaustive `match` expressions handle every variant;
- ordinary function argument and return types agree;
- a `Box<dyn Target>` or trait object satisfies the trait at compile time;
- a typed DTO has fields of the declared Rust type after successful construction.

A typed value still needs a **boundary** test when it is converted from untrusted JSON, TOML, files, SQLite rows, WebSocket frames, HTTP requests, CLI argv, environment variables, model tool input, external process output, or platform paths. Serde representation, path containment, HTTP route shape, SQLite schema, and prompt/tool JSON schemas are external contracts, not compiler guarantees.

Compile-time behavior may itself be a public API contract. Compile-pass or compile-fail tests are appropriate when downstream code must continue to compile or be rejected, including macro expansion, typestate transitions, public trait bounds, and unavailable methods in a restricted state. Such a test protects the shape of the public API; it does not re-run a compiler guarantee as a runtime assertion.

Yoi currently uses `trybuild` for compile-fail API-shape tests in `agen`. Add new `trybuild` tests only when the compile-time accept/reject behavior is the product contract being protected. Keep fixtures minimal and name them after the API rule, not after the implementation helper that happens to trigger the compiler error.

## Test names

A test name must describe an observable rule. Prefer names of the form `<condition>_<expected_behavior>` or `<operation>_<expected_result>`. Include the input class or state when omitting it would make the rule ambiguous.

Good names:

- `backend_target_rejects_local_worker_operations`
- `cli_backend_url_overrides_config_file_value`
- `repository_path_rejects_symlink_escape`
- `second_enter_does_not_submit_rewind_twice`
- `workspace_server_returns_not_found_for_unknown_worker`

Bad names:

- `test_target`, `test_parse`, or `test_create`: names the subject but not the rule.
- `works`, `basic`, `happy_path`, or `success`: does not say what outcome is protected.
- `regression_1234`: records provenance but loses the lasting product behavior.
- `ticket_test` or `server_test_2`: adds no information beyond the module or file.
- a private helper name alone, such as `normalize_path`: couples the name to implementation without stating the externally relevant result.

No prefix is required. `contract_` is normally redundant because every test in this policy protects a contract. The following narrower labels are optional:

- `boundary_` when distinguishing external input handling from typed internal behavior is useful in that test target;
- `regression_` when the original failure symptom is itself the clearest durable rule;
- `smoke_` when the deliberately limited contract is successful wiring to a minimum state.

Examples such as `boundary_repository_path_rejects_symlink_escape`, `regression_second_enter_does_not_submit_rewind_twice`, and `smoke_workspace_server_reaches_health_route` remain meaningful after removing the prefix. A label does not rescue a vague name such as `boundary_path`, `regression_1234`, or `smoke_works`.

## Yoi contract map

| Area | Guaranteed by types | Must be tested |
| --- | --- | --- |
| CLI parser and Target selection | parsed `Mode`, `LaunchMode`, `TargetKind`, and command enum variants exist | top-level target option precedence, mutually exclusive options, default config merge, local/backend-only rejection, help/usage text for public command surface |
| Client config | typed config structs after TOML/environment parse | data-dir and cwd config locations, environment snapshot to startup config, property-wise overlay, missing/empty backend URL diagnostics, workspace-to-backend resolution, explicit CLI override over config |
| Workspace Server HTTP API | handler function signatures and response DTO field types | route shape, scoped workspace authority, status-code mapping, request validation, bounded list/detail bodies, mutation round trips, compatibility routes only where explicitly adopted as product contracts |
| Runtime HTTP/WebSocket protocol | protocol enum variants and typed frames after decode | JSON wire compatibility, frame ordering, snapshot/backlog/live semantics, bounded output, cancellation/restore lifecycle, unknown id/status errors |
| Client crate Target operations | trait method signatures and result DTO types | unsupported operation errors, Local/Backend behavior parity where promised, stopped-worker semantics, URL/workspace/routing normalization |
| Worker tool definitions | `Tool` trait implementation and JSON value construction compile | tool name/schema stability, input validation, Backend authority requirement, bounded output, no direct local authority fallback, side effects/audit records |
| Tickets | typed Ticket backend API and workflow-state enum after parse | SQLite authority round trip, lifecycle transitions, dependency/queue gates, relation metadata, event/audit append, local import compatibility only where explicit |
| Objectives | typed Objective summaries/details after authority read | SQLite authority mutation round trip, canonical id allocation, state/title/body validation, ticket link/unlink, audit/event rows, scoped API/tool behavior |
| Memory | typed memory operation inputs after parse | single-document edit semantics, old_string uniqueness, staging close lifecycle, bounded reads, Backend Workspace authority use, no filesystem fallback in Worker tools |
| Worker lifecycle and workdirs | Rust structs for worker/workdir records | restore/dry-restore decisions, stopped/corrupted state transitions, occupancy projection, repository path containment, cleanup idempotence |
| Workspace authority and SQLite stores | Rust records and trait methods compile | schema migrations, foreign-key/unique constraints, transaction rollback behavior, authority boundary selection, import-only legacy paths |
| Nix/Docker/dev scripts | Nix attr names and shell strings are strings | binary names, package/bin target selectors, image entrypoints, fixed-output hashes, smoke help checks |
| Web frontend TypeScript/Rust DTO boundary | generated/declared TS types after decode | API route compatibility, URL construction, projection ordering, text truncation, auth/session redirect exceptions, source-level UI policy tests |
| Prompts and tool schema resources | resource files exist as strings | prompt inclusion policy, feature-driven instruction contribution, schema names/descriptions visible to the model, no stale command/tool references |

## Test design rules

1. Arrange only the state relevant to the named contract.
2. Exercise public or module-boundary behavior where possible. Test a helper directly only when that helper is the policy boundary.
3. Assert outcomes, rejected inputs, API responses, durable records, or state transitions rather than private call order and incidental ids.
4. Use a table in one test when several inputs describe one policy matrix. Use separate tests when failures would represent different product rules.
5. Boundary tests must include the boundary input in the fixture: argv slice, JSON body, TOML text, explicit environment snapshot, SQLite row/migration, path, HTTP request, protocol frame, or tool input JSON.
6. Filesystem tests use an RAII temporary directory and leave no files behind, including after panic. Do not write tests against the user's real Yoi data dir.
7. SQLite tests use temporary databases or in-memory stores and explicitly seed the authority records they need. Do not depend on the developer's live `server.db`.
8. Tool tests assert the model-visible contract: tool name, schema, validation, bounded content, authority errors, and durable side effects. Do not only assert that a Rust helper was called.
9. API tests should assert status codes and response bodies for both success and rejection. If a helper hides response bodies, include the body in assertion failure output.
10. Serde/DTO tests should exist only for external representation contracts: renamed fields, wire enums, bounded truncation, or explicitly adopted compatibility with existing persisted data. Do not add aliases, legacy variants, routes, or tests for them without an explicit compatibility decision.
11. Snapshot-like assertions are allowed only when the exact text/wire format is the contract. Prefer semantic assertions for large JSON/API responses.
12. Built-in resources and prompts should be tested by deriving expected references from manifests or feature config, not by duplicating mutable inventory counts in Rust assertions.
13. Do not test private helpers just to improve coverage numbers. If a private helper carries a policy boundary, name the contract in the test and keep the fixture minimal.
14. Every bug fix either strengthens an existing test or adds one focused test whose name states the lasting rule or the original failure mode. A `regression_` prefix is not required.
15. When a test needs a production resource, state which compatibility contract that resource represents. Generic fixtures should use minimal generated definitions.
16. Do not introduce environment variables whose purpose is test control, test fixtures, or test observation.
17. Code that consumes environment variables must snapshot them into typed startup configuration once. Test the environment-to-configuration mapping as a boundary over an explicit snapshot; all downstream logic tests pass typed configuration and do not read, set, remove, or depend on process-global environment variables.
18. Tests must not depend on execution order. Ordinary tests must be safe under Cargo's default parallel execution and must not coordinate through process-global state.
19. Synchronize on observable events or conditions. Do not use a fixed sleep to guess that asynchronous work has completed; use a timeout only as an upper bound that turns a hang into a diagnosable failure.
20. Tests must not depend on live external services or unrestricted network access. Use an in-process adapter or loopback fake at the narrow protocol boundary.
21. Do not implement a test that spawns real product processes or crosses a complete browser/TUI/client/backend/runtime flow unless the work explicitly requires the test to be designed and implemented as E2E. Otherwise protect the narrower parser, API, protocol, authority, or runtime contract.

## Placement rules

- Put unit tests next to the code when the policy boundary is module-local and does not require public API composition.
- Put integration tests under `tests/` when the contract is the public API of a library or binary-facing behavior. Shared integration-test helpers should live under `tests/common/mod.rs` so Cargo does not treat helper-only files as separate test crates.
- Binary parser/help tests may live in the binary crate when the parser is intentionally internal. Extract parser and startup-configuration boundaries for direct testing where practical. A real product-process invocation is added only when process-level behavior is explicitly required as an E2E contract.
- Workspace Server route tests belong near the router/API code unless they require a cross-crate external client contract.
- Store and authority tests should distinguish pure store constraints from Workspace authority behavior. A store test proves persistence constraints; an authority test proves product policy.
- Worker tool tests belong with the feature/tool module when they assert model-visible names, schemas, validation, authority selection, or bounded output.

## Boundary-specific guidance

### CLI

Parser tests must focus on user-visible command contracts: accepted argv shapes, target precedence, config defaults, and clear rejection. Do not assert that a parser took a particular private branch.

Help tests should prevent stale public surface: command names, binary names, config locations, and removed shims. They should not duplicate the entire help text unless the exact text is the compatibility contract.

### Environment-backed startup configuration

Environment lookup belongs at startup, before operational logic begins. Materialize an explicit environment snapshot, resolve it into typed configuration once, and pass that configuration inward.

Test parsing, precedence, missing values, and invalid values against the explicit snapshot. Do not repeatedly exercise the same environment lookup through unrelated logic tests, and do not mutate the test process environment.

### Workspace Server and Runtime APIs

Route tests should use HTTP-like requests against the router and assert status plus JSON body. A direct handler test is appropriate only when route wiring is irrelevant and the handler is the boundary.

Protocol tests should assert observable event/order/state semantics, not internal channel choreography.

### SQLite authority

Migration tests must assert the current schema version and the persisted behavior introduced by the migration. A schema version bump without a behavior test is insufficient when the migration adds a table, index, constraint, or compatibility transform.

Authority mutation tests should prove durable state after re-read and audit/event side effects where the authority promises them.

### Worker tools

A Worker tool exposes an LLM-facing contract. Tests should cover:

- exact tool name when the name is model-visible;
- required/optional schema fields and bounds;
- rejected malformed input;
- authority errors when Backend Workspace authority is required;
- bounded and useful output content;
- durable side effects for mutation tools.

Do not expose local filesystem fallback in a tool test unless that fallback is the intended product contract.

### Prompts and resources

Prompt tests should assert inclusion/exclusion and feature ownership, not incidental whitespace. If exact wording is a product policy, keep the asserted snippet short and name the policy.

## Execution assumptions for test authors

An ordinary test is part of the normal Cargo test suite. It must be deterministic, order-independent, parallel-safe, hermetic, and fast enough for repeated local execution.

If a contract exists only under a Cargo feature, gate the test with the same feature and place it in a target that Cargo actually builds for that feature. Do not hide ordinary product tests behind an unrelated test-only feature or environment variable.

An opt-in, ignored, process-spawning, privileged, platform-specific, or E2E test must be an explicit design decision. Its target, feature gate, runtime prerequisites, cleanup behavior, timeout, and diagnostic artifacts are part of the test design rather than assumptions left to the person running it.

Commands and validation scope used while developing are repository workflow rules and belong in `AGENTS.md` or `development/validation.md`, not in this test-authoring policy.

## Review checklist

When reviewing or adding a test, ask:

1. What product contract does this test protect?
2. Could Rust types alone already prove this?
3. Is this the right boundary: parser/API/store/tool/protocol/public library?
4. Would one failure point to one understandable rule?
5. Does the test avoid live user data, real data dirs, and incidental ids?
6. If this is a bug fix, did we encode the lasting rule or the original regression symptom?
7. Does the test avoid test-only environment variables and process-global environment access?
8. Is the test order-independent, parallel-safe, and synchronized without guessed sleeps?
9. Is any compatibility behavior backed by an explicit product decision?
10. If this spawns a product process or crosses the full stack, was E2E implementation explicitly required?
