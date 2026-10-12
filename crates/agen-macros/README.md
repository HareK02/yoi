# agen-macros

`agen-macros` generates `agen` tool definitions and implementations from Rust
method declarations. It isolates procedural macro expansion from `agen`, which
owns tool registration and execution.

## Design entry points

- [`tool_registry`]: declaration validation, argument schemas, and execution wrappers
