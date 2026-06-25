# Tool permissions and scope

Yoi treats tools as explicit capabilities. Model-visible tool names are not permission by themselves; the resolved Manifest and Worker scope decide whether a call is allowed.

## Permission policy

Tool permissions are built into PreToolCall policy.

- `allow` permits the call to proceed.
- `deny` rejects only that call with a synthetic error result.
- `ask` fails closed until a real approval/resume protocol exists.

Failing closed matters because an unresolved approval state is not the same as permission. A future approval flow must be able to pause and resume the same unresolved call, not merely ask the model to retry later.

## Filesystem scope

Filesystem scope is separate from tool allow/deny. A file tool may be registered and permitted, but the concrete path still must be inside readable or writable scope.

Symlinks do not grant extra authority. Access decisions should be made on the canonical target when it exists, and broken or out-of-scope links should produce diagnostics rather than escaping scope.

Directory traversal tools should not follow symlink directories as a way around scope. If an external checkout is needed, add its real path to read scope.

## Delegation

Child Workers receive an explicit subset of the parent's scope. Delegation is a capability loan, not a copy of all parent authority.

When a child stops, shuts down, or is pruned as unreachable, delegated write permissions must be reclaimed. Explicit base denies remain in force.

## Tool output

Tool output should be bounded before it enters history/model context. The system may truncate or summarize mechanical output boundaries, but it should not hide the fact that a tool call happened or fabricate successful results.

Network tools such as WebSearch/WebFetch are disabled/unconfigured by default, fail closed, and need explicit manifest/profile configuration. Their outputs are untrusted content and must remain bounded.

## Why this design exists

LLM tool calls are suggestions from an untrusted planner. Yoi can let the model propose operations while keeping final authority in manifest policy, scope checks, and durable records.
