# tools

## Role

`tools` implements built-in tools and shared tool execution helpers used by Workers.

## Boundaries

Owns:

- built-in filesystem, web, memory, and Worker-management tool implementations where applicable
- bounded tool output formatting
- scope-aware file operation helpers
- tool-facing diagnostics suitable for history/model consumption

Does not own:

- manifest permission policy definition (`manifest`)
- Engine tool-loop semantics (`agen`)
- Worker lifecycle decisions (`worker`)
- UI presentation (`tui`)

## Design notes

A tool implementation must assume model input is untrusted. Permission policy, scope checks, output bounding, and redaction are part of the safety boundary, not optional UI behavior.

## Checkout-native processing

`execute_checkout_tool` (also exported through `tools::checkout`) accepts the
original `WorkdirSessionRouter`, shared `Tracker`, route-bound alias/generation,
logical target and provider validator. It does not create an attachment or router.
The Host owns native publication and permission identity; provider scope and
capabilities remain authoritative. `Create` uses the `Write` capability/identity.

| Operation | Arguments (only these keys accepted) | Target |
| --- | --- | --- |
| Read | `offset?`, `limit?` | Existing file; same numbered lines, defaults and byte bound as Tool Read |
| Edit | `old_string`, `new_string`, `replace_all?` | Existing file; prior read/hash and provider validator required |
| Write | `content` | Existing file only; prior read/hash and provider validator required |
| Create | `path`, `content` | Bound parent directory; relative destination strictly below it; create-new, missing parents allowed |
| List | `limit?` (1..1000, default 100), `after?` (`{kind, path}`) | Bound directory; Read capability; all typed `EntryKind` cursor values accepted, path is a direct child in checkout-root-relative provider coordinates |
| Glob | `pattern`, `path?` | Bound directory; optional path relative beneath it |
| Grep | `pattern`, `path?`, `glob?`, `type?`, `case_insensitive?`, `-B?`, `-A?`, `-C?`, `multiline?`, `output_mode?`, `head_limit?`, `offset?` | Bound directory; same provider search/options/rendering as Tool Grep |

Native arguments cannot override `file_path`, `target_workdir`, generation or
validator. Absolute/parent-traversing directory-relative paths are rejected.
Checkout file calls share Tool processing through an optional checked `FileTarget`
and use `checkout_execute`, never a separate Worker-side filesystem engine.
Read records the provider's full content hash **and total line count**, including
partial reads, in the original alias/generation namespace. Normal and native
observations/mutations interoperate. Only successful mutations update tracking.

Glob/Grep share normal request defaults/options and rendering through `SearchTarget`.
Only native dispatch uses `CheckoutSearchRequest::new` and `checkout_search`.
The constructor's empty scope layers/root output are refined by provider wrappers;
Tools does not synthesize authority or parse/rebase rendered filenames. Ordinary
Tool dispatch remains `session.glob`/`session.grep`, with its existing error mapping.

List dispatches one bounded `checkout_search` page without Bash or entry prefetch.
`CheckoutToolOutput.listing` preserves the typed `ListResult`, including
`next_after`, for Host projection into canonical Worldspace entries and cursors.
Its normal Tool output is a summary only, and `paths` is empty; every other
Operation returns `listing: None`. The Host decodes canonical cursor paths before
calling Tools, mapping the public `{kind, entry}` cursor into `{kind, path}`.
Both returned entries and the continuation must stay directly inside the bound
directory. Cursor kinds retain all typed values (`directory`, `file`, `symlink`,
`other`); ordering groups directories first and all non-directories together,
then compares paths. A truncated page must supply `next_after` matching its last
entry's ordering key; an untruncated page has no continuation. Special typed
listing entries are path coordinates, not checkout observations: direct checkout
observation remains provider File/Directory-only. Pagination is live, not a
cross-page snapshot; the same before/after bound-directory validator checks
used for search apply to List.

`CheckoutToolOutput.paths` contains typed checkout-root-relative search paths
(from `GlobResult.paths` / `GrepResult.paths`) and the created path for Create.
Rendered Grep text is never parsed to construct links. The optional validator is
the provider's returned observation validator, unchanged. Create's result
observation describes the bound parent target, while returned paths and tracker
updates describe the destination. Search checks the bound observation before and
after the bounded checkout provider search, but does **not** guarantee an atomic tree snapshot
or prevent external edits to descendants. File validator/hash checks are inside
the provider's checked execution boundary. Checked native provider conflicts and
native generation/search fences use `ToolError::StructuredConflict` with code
`checkout_stale`; ordinary Tool conflicts retain `ExecutionFailed`. Checked-only
known provider refusals use `checkout_denied` (scope/capability/read-only policy),
`checkout_invalid` (invalid arguments/paths/target kind/search syntax), or
`checkout_unavailable` (closed session/unavailable capacity). A disappeared bound
target is `checkout_stale`. These codes preserve known no-effect rejection for the
Host; ordinary Tool error mappings are unchanged. Ambiguous transport/IO and
operation failures remain `ExecutionFailed`, as does explicit provider
`OutcomeUnknown`; a failure after mutation must not
be automatically retried. A malformed post-mutation provider result is likewise
reported explicitly as outcome unknown. Explicit mismatched Read/Glob/Grep results
are readonly failures classified as `checkout_unavailable`, not stale or unknown.

## See also

- [`../../docs/design/tool-permissions-scope.md`](../../docs/design/tool-permissions-scope.md)
- [`../../docs/design/memory-knowledge.md`](../../docs/design/memory-knowledge.md)
