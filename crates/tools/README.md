# tools

## Role

`tools` implements built-in tools and shared tool execution helpers used by Pods.

## Boundaries

Owns:

- built-in filesystem, web, memory, and Worker-management tool implementations where applicable
- bounded tool output formatting
- scope-aware file operation helpers
- tool-facing diagnostics suitable for history/model consumption

Does not own:

- manifest permission policy definition (`manifest`)
- Engine tool-loop semantics (`llm-engine`)
- Worker lifecycle decisions (`worker`)
- UI presentation (`tui`)

## Design notes

A tool implementation must assume model input is untrusted. Permission policy, scope checks, output bounding, and redaction are part of the safety boundary, not optional UI behavior.

## See also

- [`../../docs/design/tool-permissions-scope.md`](../../docs/design/tool-permissions-scope.md)
- [`../../docs/design/memory-knowledge.md`](../../docs/design/memory-knowledge.md)
