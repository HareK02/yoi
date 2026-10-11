# Rust testing strategy for Yoi

Yoi tests protect behavior that Rust's type system cannot prove and deliberate compile-time API constraints that must not regress. A test must name the contract it protects and fail for one understandable reason.

This document defines how to choose test scope for Yoi's architecture: `yoi` CLI, Workspace Server, Worker Runtime, Worker tools/WIP adapters, SQLite authorities, Web/API DTOs, and local/runtime process boundaries. Cargo target classification and architectural test scope are separate concepts. Test the product logic and the connections that carry it; neither a directory name nor a full-stack fixture is evidence of coverage.

References used while writing this policy:

- Rust Book, "Test Organization": unit tests are focused and may test module-private behavior; integration tests use public APIs the way external code does.
- Cargo Book, `cargo test`: package, target, and feature selection determine which tests Cargo builds and runs.

## Test scope is not Cargo target classification

Cargo integration-test targets, normally under `tests/`, compile as separate crates and exercise the library's public API. In-crate tests can access private implementation details. This distinction determines compilation and visibility, not how many production components a test connects.

Use the following scopes when explaining what a test proves:

| Scope | What it proves |
| --- | --- |
| Unit / module-local | One local rule without composing the surrounding subsystem |
| Component | One named subsystem's behavior through its boundary, with external dependencies substituted where appropriate |
| Integration / composition | Agreement between connected production components, including propagation of data, authority, and effects |
| E2E | An explicitly selected product journey through the deployed interfaces and execution boundaries relevant to that journey |

These scopes are relative to the named subject. Describe the production path and substitutions rather than relying on the label. A public-API test against a fake dependency does not prove agreement with that dependency's production implementation. Yoi's placement and execution rules below make these scopes operational rather than leaving each author to choose a layout.

Process boundaries are an execution concern, not a separate test level. An isolated child process can be part of an ordinary integration test when the connection under test requires it. It does not require an E2E exception merely because it is a real product binary. Assembling the product to exercise a user journey belongs to the E2E suite, not a crate-local process harness.

## Choose scope from risk and observable behavior

Start with the observable behavior and the failure that matters to the user. Select the smallest scope that can expose that failure, considering its likelihood, impact, and the cost of maintaining the test. Smallest means sufficient, not merely easiest to mock.

- Keep local policy matrices in unit/component tests. Use integration tests when the risk lies in agreement or handoff between components.
- Test domain logic directly. UI interaction and rendering tests protect UI behavior; they do not replace proof of state transitions, authority decisions, or durable effects.
- Select retry, concurrency, cancellation, and recovery cases from the product's guarantees and failure impact. Do not expand every change into all failure modes or all component combinations.
- Prefer deterministic boundary failures for logical recovery tests. Use process termination only when process death itself is material to the guarantee and narrower tests cannot prove it. Logical recovery tests do not establish arbitrary OS-crash durability.
- Reserve E2E for behavior that requires the wider product path. Explain the additional risk it covers beyond component and integration tests.

## Production connections and test doubles

For integration tests, make the entry point, real components, substituted boundaries, and observable result clear in the test or a short fixture comment.

1. Keep the connection under test real. Independently testing both components against fakes does not prove they work together.
2. Substitute dependencies outside the behavior being claimed. Do not reproduce the product rule in a fake and then assert that it succeeds.
3. Exercise the production operation and observe its result through the boundary that owns the guarantee. Use real temporary storage for persistence and transaction claims; captured calls alone do not prove durable effects.
4. Preserve the identity and authority context relevant to the claim. A fixture that bypasses an authorization boundary cannot prove that boundary.
5. Inject failures without replacing the recovery logic being tested. Synchronize through observable events or conditions, not timing guesses.

## Coverage responsibility and maintenance

Before adding tests, inspect existing coverage for the changed contract. Identify which tests own the local rules and which prove the relevant connections. Keep this explanation proportional to the change; it does not require a new coverage registry or a centralized integration suite.

Give each invariant a primary test at the boundary that owns it. Integration tests should prove connections rather than repeat every local input matrix. Strengthen existing tests where possible; if they already prove the contract, report that evidence instead of adding redundant tests. Test counts, directory placement, and a passing smoke test are not proof of coverage.

When a product contract changes, update affected fixtures and assertions, including opt-in suites. Preserve obsolete behavior only for an explicitly supported compatibility boundary. A skipped or unexecuted test is not evidence that the current behavior works.

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

This map identifies responsibility areas, not an exhaustive scenario checklist or a snapshot of implementation details. Select tests from the current product contract and the risks of the change; do not turn an old fixture or state name into a new product requirement.

| Area | Guaranteed by types | Must be tested |
| --- | --- | --- |
| CLI parser and Target selection | parsed `Mode`, `LaunchMode`, `TargetKind`, and command enum variants exist | top-level target option precedence, mutually exclusive options, default config merge, local/backend-only rejection, help/usage text for public command surface |
| Client config | typed config structs after TOML/environment parse | data-dir and cwd config locations, environment snapshot to startup config, property-wise overlay, missing/empty backend URL diagnostics, workspace-to-backend resolution, explicit CLI override over config |
| Workspace Server HTTP API | handler function signatures and response DTO field types | route shape, scoped workspace authority, status-code mapping, request validation, bounded list/detail bodies, mutation round trips, compatibility routes only where explicitly adopted as product contracts |
| Runtime HTTP/WebSocket protocol | protocol enum variants and typed frames after decode | JSON wire compatibility, frame ordering, snapshot/backlog/live semantics, bounded output, cancellation/restore lifecycle, unknown id/status errors |
| Client crate Target operations | trait method signatures and result DTO types | unsupported operation errors, Local/Backend behavior parity where promised, stopped-worker semantics, URL/workspace/routing normalization |
| Worker tool definitions | `Tool` trait implementation and JSON value construction compile | tool name/schema stability, input validation, Backend authority requirement, bounded output, no direct local authority fallback, side effects/audit records |
| Tickets | typed Ticket inputs, state values, and assignment references after parse | authority mutation/readback, lifecycle and resolution semantics, assignment identity and authority, relations and audit history, separation of lifecycle, work progress, and delivery |
| Objectives | typed Objective summaries/details after authority read | SQLite authority mutation round trip, canonical id allocation, state/title/body validation, ticket link/unlink, audit/event rows, scoped API/tool behavior |
| Memory | typed memory operation inputs after parse | single-document edit semantics, old_string uniqueness, staging close lifecycle, bounded reads, Backend Workspace authority use, no filesystem fallback in Worker tools |
| Worker lifecycle and workdirs | Rust structs for worker/workdir records | restore/dry-restore decisions, stopped/corrupted state transitions, occupancy projection, repository path containment, cleanup idempotence |
| Workspace authority and SQLite stores | Rust records and trait methods compile | schema migrations, foreign-key/unique constraints, transaction rollback behavior, authority boundary selection, import-only legacy paths |
| Nix/Docker/dev scripts | Nix attr names and shell strings are strings | binary names, package/bin target selectors, image entrypoints, fixed-output hashes, smoke help checks |
| Web frontend TypeScript/Rust DTO boundary | generated/declared TS types after decode | API representation and route agreement, projection logic, validation and error handling; source-text assertions do not prove runtime or rendered behavior |
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
20. Tests must not depend on live external services or unrestricted network access. Use in-process components or loopback transport; substitute only dependencies outside the connection being proved.
21. Keep boundary integration in the ordinary test suite, including isolated process boundaries where needed. Add or extend E2E only for an explicitly selected product journey that narrower tests cannot prove, using the shared `tests/e2e` harness rather than assembling the product independently in a crate-local suite.

## Placement, fixtures, and execution

The following are Yoi's default operating rules, not interchangeable layout suggestions. The crate that owns a boundary also owns its test suite. Moving a test into another file does not change its scope.

| Scope | Placement | Allowed dependencies | Execution class |
| --- | --- | --- | --- |
| Unit | Owning module's inline `#[cfg(test)] mod tests` or extracted child test module under `src/` | Local logic and minimal fixtures | Ordinary crate tests |
| Component | Owning crate's `src/<component>/tests.rs` or equivalent named test module | Component implementation, temporary resources, doubles outside the component boundary | Ordinary crate tests |
| Integration | A named test target under the owning crate's `tests/`; a named in-crate integration module when crate-internal access is required | Connected production implementations, temporary databases, loopback/IPC, isolated child processes when relevant to the boundary | Ordinary crate tests, with applicable product features |
| E2E | Workspace `tests/e2e` package, with journey tests using its shared harness | Product assembly and actual execution interfaces needed by the selected journey, with isolated data and resources | Dedicated opt-in E2E target/job |

### Placement details and exceptions

- A component test whose purpose is external public-API use belongs under the owning crate's `tests/`. State its component scope in the target/module documentation; being a Cargo integration-test target does not expand its guarantee.
- In-crate integration modules must be identified as such and document the production connection they cover. Do not expose private implementation solely to move tests under `tests/`.
- Share Cargo test-target helpers under `tests/common/mod.rs`; use a crate-local test-support module for in-crate suites. Group related integration cases into modules of one target instead of creating a separate test executable for every case.
- Cross-crate integration belongs to the crate that owns the composed boundary. Introduce a separate test-support or integration package only when dependency constraints or genuinely shared use justify it; do not place ordinary composition tests in `tests/e2e` for convenience.
- Parser/help policy tests and Worker tool policy tests stay with their owning modules. Store tests prove persistence constraints; authority tests prove product policy. Route tests that connect router and authority implementations belong to the owning API's integration suite.
- Keep product-assembly fixtures in `tests/e2e`. Do not create parallel E2E launch stacks under individual crates.

### Shared fixture responsibilities

Each integration or E2E suite must use shared support for the resources it owns, rather than repeating setup and cleanup in individual cases. A short suite/module description must identify its scope, production components, substitutions, execution target, and prerequisites.

- Allocate isolated directories, databases, identities, and communication endpoints. Never borrow a running developer instance or live user data.
- Resolve binaries explicitly and construct each child's working directory and environment without modifying the parent test process environment. Keep product environment inputs separate from test-control hooks.
- Own child processes from launch through termination and reaping, including assertion failure and timeout. Synchronize startup and work completion on observable readiness/events, not fixed sleeps.
- Bound waits and collect useful failure diagnostics. Reuse the suite's cleanup and artifact policy rather than making each case invent one.

Ordinary integration tests must remain deterministic and parallel-safe. A need for a database, a socket, or a child process alone is not a reason to make a test opt-in. Tests needing special tools, privileges, or platform support require an explicit prerequisite and execution selection; that exception does not automatically make them E2E.

Execution commands and CI selection belong in [validation.md](validation.md). A suite's default/feature/opt-in classification must be reflected in its Cargo target and runner/CI configuration, not just its prose label.

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

Opt-in, ignored, privileged, platform-specific, and E2E execution must be explicit design decisions. Their targets, selection conditions, prerequisites, timeouts, and diagnostics must be documented and configured. Ordinary process-spawning integration tests use the same shared-fixture isolation and cleanup rules; process spawning alone is not an opt-in condition.

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
10. Does the test use the prescribed owning suite and shared fixtures? If it assembles the product, is it in `tests/e2e` with a selected journey that narrower tests cannot prove?
11. Are the connection under test and its production implementations real, with substitutions outside the claim?
12. Does the test scope match the failure's likelihood and impact, including any recovery or fault-injection cases?
13. Does existing coverage already prove this rule, and have affected fixtures and assertions been updated for the current contract?
14. Are prerequisites, process/resource cleanup, execution selection, and CI responsibility explicit rather than inferred from a test label?
