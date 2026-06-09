Implemented, reviewed, merged, and validated.

Summary:
- Added `ToolExecutionContext` with `call_id`, `batch_id`, and `call_index`.
- Changed Tool execution APIs so `Tool::execute` / `ToolServer::call_tool` receive execution context.
- Worker assigns one response-local batch id per approved tool-call batch and preserves LLM-returned call order with `call_index`.
- Approved tool calls still execute in parallel; no scheduler, same-file mutex, mutation queue, conflict solver, or `parallel_tool_calls=false` behavior was added.
- `ToolCallInfo` / `ToolResultInfo` expose context metadata.
- Macro-generated tools can receive `ToolExecutionContext` as a non-schema parameter; context is not exposed as an LLM JSON argument.
- Built-in tools and tests were migrated to the new API.

Implementation:
- Coder commit: `d8aed7b tool: add execution context`
- Reviewer approved with no blocking findings.
- Merge commit: `4744548 merge: add tool execution context`

Validation after merge:
- `cargo test -p llm-worker --test parallel_execution_test test_tool_execution_context -- --nocapture`
- `cargo test -p llm-worker --test parallel_execution_test test_parallel_tool_execution -- --nocapture`
- `cargo test -p llm-worker --test tool_macro_test -- --nocapture`
- `cargo check -p llm-worker -p tools -p ticket`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `cargo check --workspace`
- `nix build .#yoi`

Residual note:
- `batch_id` is Worker-local and not a durable/global external artifact key.