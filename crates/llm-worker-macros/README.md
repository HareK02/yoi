# llm-worker-macros

## Role

`llm-worker-macros` provides procedural macros for declaring Rust methods as LLM-callable tools.

## Boundaries

Owns:

- compile-time generation of tool argument structures and definitions
- small macro conveniences around tool descriptions and schemas

Does not own:

- runtime permission decisions
- filesystem scope checks
- tool execution policy
- model/tool-loop orchestration

## Design notes

Macros reduce boilerplate, but they must not imply capability. A generated tool definition is still subject to manifest permissions, Pod scope, and runtime policy.

## See also

- [`../../docs/design/tool-permissions-scope.md`](../../docs/design/tool-permissions-scope.md)
