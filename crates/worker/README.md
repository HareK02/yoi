# worker

## Role

`worker` turns an `agen` Engine into a named runtime entity with manifest configuration, scoped tools, session persistence, protocol handling, and Worker metadata integration.

## Boundaries

Owns:

- Worker lifecycle and socket protocol serving
- Engine construction around a resolved Manifest
- session-store and session-store worker metadata coordination
- built-in tool registration under scope/policy
- spawned-child orchestration hooks

Does not own:

- provider-specific wire formats (`provider` / `agen` clients)
- product CLI parsing (`yoi`)
- TUI display authority (`tui`)
- current-state storage schema outside Worker metadata (`session-store` worker metadata)

## Design notes

A Worker is runtime authority, not UI state. It should commit model-visible events through history/session paths and keep current Worker-name state in Worker metadata rather than in transient runtime files.

## Workspace config WIP Feature

Enable the opt-in surface in a Worker profile/manifest:

```toml
[feature.workspace_config]
enabled = true
```

This setting accepts **only** `enabled` and defaults to false. Controller
registration additionally requires Workspace identity, an available injected
Workspace Client and WIP mode. It creates no grant. An owner must explicitly
create a Backend grant bound to Workspace/Runtime/Worker and read-only or
read-write access; standalone/Tools-mode config compatibility tools do not exist.

`feature::builtin::workspace_config` registers `/workspace-config` through the
common structured invocation registry (T696). Its optional `access` argument is
`effective` (default), `read_only`, or `read_write`; explicit write never silently
downgrades. Feature preparation attaches before the request reaches the assistant
and reports the logical target and effective access. Native self-attach uses the
same `WorkspaceConfigFeature::attach` path. Backend alias attachment is idempotent
under the session lock; neither entry point blindly retries uncertain outcomes.

The canonical alias is `workspace-config`; the content entrance is WIP
`/workspace-config`. Attachment identity and conditional detach use the existing
T697 Backend ledger. No OS mount, Workdir session, Bash capability, local config
copy or second attachment inventory is created. The registry source is logical,
not a Runtime filesystem target.

The request-time provider maps `/workspace-config` to the Backend's empty root
path and descendants to canonical relative source paths, for example
`/workspace-config/main.dcdl` -> `main.dcdl`. Observations return structure,
Backend validators and current operations, **not bodies**. Files publish `read`;
read-write grants additionally publish `write`, unique-substring `edit`, and
`delete`. Directories and explicit absence observations publish `create` where
Backend metadata permits it. The root publishes `apply_changes` for atomic
create/update/delete/rename sets, allowing new imported
sources and their references to be saved together. Empty directory creation,
commands and generic POSIX operations are not exposed. The adapter uses the typed
`workers/self/workspace-config` GET/attach/observe/read/commit API; it never calls
an owner config API as a bypass.

Client-managed PATH validators are compared against freshly resolved Host
metadata. Each mutation captures the ROOT validator, whole-tree revision/digest,
canonical entrypoints and node state from that same observation. Backend resolves
the live grant/link and validates canonical changes/CAS under its execution lock.
No operation accepts LLM-supplied validators or `expected_digest` fields. Returned
post-call path validators are refreshed from the exact committed revision, not
substituted with root validators or later unrelated state. UI/Worker edits and
same-bytes replacement revisions reject stale calls. Old descriptors cannot
bypass read-only restrictions, detach, revocation or Workspace boundaries.

Bounds: depth 0..8, 256 observed nodes/changes, 512-byte relative paths, 256 KiB
aggregate changed text/read content, and 30-second retrieval/operation deadlines.
Path traversal and malformed parameters fail before content calls. Typed
`NotCommitted` failures remain distinct from `Unknown`; timeouts/lost mutation
responses are unknown and never automatically retried. Raw transport/provider
errors are not copied into model/UI output. Retrieval protocol failures are
reported as failures after recording their Client cache state, not as fresh
successful observations. Saving commits the canonical Workspace configuration
and existing projection/activation pipeline; it does **not** rewrite the resolved
manifests of already-running Workers.

Public integration seams: `WorkspaceConfigFeature::for_workspace`,
`workspace_config::wip::mount_workspace_config_wip`, and
`WipRuntime::from_mounts` plus `discover`/`inspect`/`call`. Worker tests exercise the
real Feature registry, native mount and WIP Client/Host through a typed in-process
contract router: deep discovery, slash/native attach, read/edit/write,
create/delete, atomic multi-source updates, grant restrictions/revocation,
detach, cross-Workspace isolation, concurrent/same-bytes stale observations,
invalid/oversize/save/unknown failures and no retries. Those contract-router tests
are not a substitute for Backend canonical persistence, semantic validation,
projection activation or full Web-to-Backend acceptance tests.

## See also

- [`../../docs/design/worker-session-state.md`](../../docs/design/worker-session-state.md)
- [`../../docs/design/context-history.md`](../../docs/design/context-history.md)
- [`../../docs/design/tool-permissions-scope.md`](../../docs/design/tool-permissions-scope.md)
