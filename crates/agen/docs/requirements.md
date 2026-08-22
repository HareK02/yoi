# agen requirements

## R1: Turn execution and continuation

- `Engine::run` starts a turn and loops through provider output and tool calls.
- An `Interceptor` may continue, cancel, or pause work at defined orchestration boundaries.
- `Engine::resume` continues paused generation without fabricating another user message.
- Cancellation and provider errors are represented as typed `EngineError` values.

## R2: Explicit cache-preserving state

- `Engine<C, Mutable>` permits configuration and history edits.
- `Engine::run` or `Engine::lock` transitions to `Engine<C, Locked>` and records the committed prefix.
- A locked engine appends turns but cannot mutate that prefix through mutable-only APIs.
- `Engine::unlock` explicitly abandons the lock before configuration or history changes.

## R3: Tool declarations and execution

- `#[tool_registry]` generates a schema and `Tool` implementation for methods marked `#[tool]`.
- `#[description = "..."]` supplies argument descriptions in generated JSON Schema.
- Generated code resolves its runtime and helper dependencies through `::agen`.
- Invalid and duplicate marker attributes produce compile errors rather than panics.
- Tools execute through `ToolServer` with typed context, errors, and output limits.

## R4: Provider-neutral orchestration

- `LlmClient` is the boundary between the engine and provider-specific transport.
- Request/response schemes translate provider wire formats into shared request and event types.
- Interceptors, tool execution, timeline collection, and pruning stay above the provider transport.
- Provider-specific capabilities are optional features when they require additional policy or dependencies.

## R5: Publication quality

- crates.io metadata includes license, repository, documentation, README, categories, keywords, and MSRV.
- The default feature set and each optional feature compile and test independently.
- Macro expansion compiles in a downstream-style integration test without direct helper dependencies.
- rustdoc builds without dependency documentation.
- Package contents are explicitly bounded and exclude credentialed fixture-recording utilities.
- `cargo package` and `cargo publish --dry-run` are run for `agen-macros` before `agen` because the main package depends on its companion package.
