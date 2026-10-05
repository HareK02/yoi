# Worker WIP mode

## Status and dependencies

Workers support two explicit model-facing modes:

```toml
[worker]
name = "example"
mode = "tools" # default
```

```toml
[worker]
name = "example"
mode = "wip"
```

Profiles select the same policy without owning the runtime Worker name:

```toml
worker = { mode = "wip" }
```

Omission resolves to `tools`, so existing manifests, profiles, persisted snapshots, and restored Workers retain the ordinary individual-tool surface. A direct Internal SubWorker selected with `profile = "inherit"` inherits the parent mode as reusable behavior. `default` and named profile selections use that profile's own mode and do not implicitly inherit WIP.

The implementation uses the crates.io `wip-client`, `wip-protocol`, and `wip-http` **0.1.0** releases. It does not use Git/path dependencies or the old `wip-https` name. Those releases require Rust 1.88; the workspace declares `rust-version = "1.88"`, and the changed crates inherit it. WIP Host/interface crates are not needed for the async compatibility adapter: the published client/protocol/HTTP state machines remain authoritative while Yoi supplies an in-process async Host transport.

## LLM surface and route contract

A WIP Worker exposes exactly these generic LLM tools instead of exposing every enabled ordinary tool twice:

- `WipDiscover`: observe an object path and bounded descendants, beginning at `/`.
- `WipInspect`: fetch the descriptor for an opaque interface reference returned by discovery.
- `WipCall`: call an operation using fresh object and interface observations.

The compatibility projection mounts enabled ordinary tools at:

```text
/tools/<exact-tool-registration-name>
```

Each object has one opaque interface reference:

```text
yoi.tool/<exact-tool-registration-name>/v1
```

Its single operation is `call`. For this compatibility route, `WipCall.arguments` is the original ordinary tool argument object. Native projections instead receive a JSON object keyed by the descriptor's declared parameter names. Native input is decoded with the published `wip-http` descriptor-bound codec, so named and composite types are resolved, an integral JSON number remains a WIP `Number` when declared as such, and WIP `Bytes` use the codec's canonical RFC 4648 base64 JSON representation. Tool argument IDs remain JSON values; the adapter never infers a domain object or Worldspace route from them.

The interface descriptor uses WIP `Json` for compatibility input and includes the exact original JSON Schema in descriptor documentation. Before execution, the Host validates the JSON value with that original schema and then delegates to the original async `Tool`. Constraints are therefore neither approximated nor silently dropped. A schema that cannot be compiled prevents WIP startup instead of creating a weaker projection.

## Native projection extension and collisions

`worker::wip::WipMountRegistry` is the Host-owned namespace, Object, and operation registry. A provider requests a root namespace with `allocate_namespace(owner, namespace)`; the owner is retained as Host metadata and is not inserted into the public path. It then mounts the collection Object directly at `/<namespace>` and may install a bounded direct-child resolver. Features do not appear as intermediate Objects.

Object resolution and operations are separate contracts. One mounted `WipProjection` owns a static Object route and its initial operations. Another Feature may add operations with `contribute_operations` without replacing that Object. Dynamic families likewise keep one `WipDynamicItemResolver`, while `contribute_dynamic_operations` supplies disjoint handlers for every resolved item without a second resolver. The Host merges only contributions with the same interface format, documentation, and type declarations; duplicate operation names, inconsistent interfaces/validators, missing targets, duplicate namespace owners, and unrelated route claims fail deterministically at registration. The merged interface validator changes with the effective operation set, so stale Known Space cannot authorize a newly contributed operation.

- Routes must be canonical absolute WIP paths, and allocated namespaces are one lowercase root segment.
- Object names must match the final route segment.
- Object existence and resolution have one owner; operation contributors cannot remount or substitute the Object.
- Multiple Features may contribute disjoint operations to the same Object interface.
- Unrelated duplicate routes, duplicate operation names, and conflicting interface shapes fail startup.
- A bounded dynamic mount may resolve exactly one item segment below an already mounted native collection; deeper paths never fall through to the resolver.
- A native projection replaces a compatibility projection either by the original same-route semantic capability or by an explicit registry capability claim owned by a mounted native route.
- Conflicting native claims fail startup. When native is selected, the claimed compatibility tool is hidden rather than exposed through a second entry. Normal Tool mode is unaffected because it does not install the WIP registry.

`WipInspect` renders the complete effective descriptor, including contributed operations, descriptor-local named declarations, and recursive record, list, enum, and union shapes with required flags and declaration documentation. The tests `host_namespace_object_resolution_and_operation_contributions_are_independent`, `operation_contributions_reject_interface_and_target_conflicts`, and `native_projection_preserves_descriptor_and_decodes_typed_arguments_end_to_end` demonstrate root allocation, unique resolution, contribution merging, collision rejection, current call-time authorization, complete descriptor inspection, and descriptor-typed invocation.

The built-in native route migration is:

| Historical route | Canonical route |
| --- | --- |
| `/features/ticket/tickets[/<reference>]` | `/tickets[/<reference>]` |
| `/features/objective/objectives[/<reference>]` | `/objectives[/<reference>]` |
| `/features/merge-request/merge-requests[/<reference>]` | `/merge-requests[/<reference>]` |

The built-in Repository/Workdir reference provider allocates `/repositories`, `/workdirs`, and `/workdir-attachments` using this contract; `manage-workdir` contributes operations rather than owning another Object. See [Repository, Workdir and attachment WIP projection](workdir-wip-projection.md) for identity encoding, reference/management permissions, attachment lifetime safety, and first-Workspace discovery. `/checkouts` content projection remains a separate provider.

### Registering a subsequent provider

1. Allocate `allocate_namespace("repository", "repositories")`. The returned `WipNamespaceRoute::root()` is `/repositories`; the owner is independent of any Feature registration name.
2. Mount a native collection `WipProjection` at that root with capability `repository:collection`, matching Object name and one declared interface. Static Objects underneath the root use the same capability owner prefix. A mounted collection retains its own identity, interfaces, and validator even when it has static children.
3. For dynamic items, register one `WipDynamicMount` on the collection with capability `repository:item`. A foreign capability owner is rejected. Its `WipDynamicItemResolver` owns reference acceptance, Object identity, and current Object validator. It returns only a bounded direct child; it must not reinterpret a reference as another namespace or grant permission from existence.
4. The reference provider supplies its read operations and handlers independently of management Feature enablement. An Object-only provider may use an empty operation descriptor. Enabled management Features register `WipOperationContribution` for static Objects or `WipDynamicOperationContribution` for a dynamic family; they do not allocate the namespace again or replace its resolver.
5. Contributions use the existing interface reference and identical format, documentation, and type declarations, supplying only their new operations. Interface references are global: extending a descriptor that is also mounted elsewhere must not create different descriptors/validators under the same reference (registration fails). Use separate references for collection and item contracts. Failed contributions leave the registry unchanged.
6. Handlers are responsible for checking current target capability and current subject authorization before dispatch. Registration/discovery is metadata, not a grant. A resolver/handler must retain the original permission identity and domain audit/failure contracts. Revocation after discovery must fail before provider dispatch. Compatibility mounts are restricted to `/tools/<tool-name>` and cannot bypass namespace ownership or republish legacy routes.

Tests `management_enablement_is_independent_of_read_and_operation_permissions` and `contributed_interface_invalidates_observations_and_restore_requires_discovery` cover all eight enablement/read/manage combinations, direct-call rejection, effective descriptor changes, fresh runtime isolation under the same Worker identity, reset, and post-observation revocation.

## Authority and execution

Discovery is not authority. Object publication, operation implementation, target capability, and subject permission are independent checks. An Object may remain discoverable through an enabled read provider when a management Feature contributes no operations; conversely, publishing an operation never bypasses the target's current capability or the subject's current permission. Every handler re-evaluates permission at call time, so revocation after discovery is rejected before provider dispatch. Feature enablement selects implementations and does not itself grant read or mutation authority.

The Worldspace contains only the operations registered from enabled Features and the Worker's current execution context. Calling a compatibility operation:

1. resolves the exact mounted route;
2. verifies object and interface membership and validators;
3. validates the WIP descriptor call;
4. validates the original JSON Schema;
5. evaluates manifest tool permission rules against the **original tool name and original arguments**;
6. invokes the original async `Tool` without blocking the Tokio runtime.

Filesystem scope, provider Workdir capability checks, Backend Workspace authority, Feature enablement, and input-specific restrictions stay in the original tool implementation and are checked again at invocation. `ask` permissions remain denied fail-closed because the runtime has no approval protocol. The gateway does not mint authority or expose authentication material.

Pre/post Engine history still records one bounded `WipCall` result, while the compatibility result retains the original `ToolOutput`, including attachments and normal Engine output pruning. WIP call audit records retain the route, operation, request identity, and terminal classification.

## Client lifetime, restoration, and compaction

The stateful `wip-client::Client` is owned by one Worker WIP runtime. Sessions are keyed by the canonical endpoint and an opaque security-context identity derived by the Host from Workspace, Worker, and session identity. Authentication credentials are not included in that identity and are never emitted to descriptors, tool output, or history.

Known Space and interface observations are therefore not shared across Workers, endpoints, or security contexts. A restored Worker creates a new runtime/client and must explore again. `WipDiscover(reset = true)` drops all observations before reconnect/authority-change exploration; `refresh = true` explicitly supersedes one cached observation. A normal compaction keeps the live runtime but the three gateway schemas are supplied again on every LLM request, including the instruction to begin discovery at `/`; the compacted transcript is not treated as cache authority.

Saved Session/history entries remain append-only evidence and are never rewritten or replayed as calls. In particular, historical `/features/<feature>/...` text stays displayable but is not a current route alias. Passing such a legacy path to discovery or call returns `NotFound` with guidance to rediscover from `/`; the Host does not translate it, resolve it under another namespace, or expose old and new routes in parallel.

## Errors, cancellation, and retries

The adapter keeps these outcomes distinct:

- **rejected/not dispatched**: route, membership, validator, descriptor, schema, or permission failure; the original operation does not run;
- **stale**: object/interface validator mismatch; refresh before reconstructing a call;
- **cancelled**: the original provider confirms cooperative cancellation and its bounded cancelled output is retained;
- **interrupted**: provider interruption is retained separately from cancellation;
- **disconnected/timeout after dispatch**: `wip-client` records an unknown outcome;
- **operation outcome unknown**: an executing provider fails without a rollback/non-execution guarantee, a response cannot be completed after dispatch, or the call future is force-closed.

Cooperative cancellation is scoped to the active execution's selected operation and exact resolved handler instance. Static/dynamic dispatchers forward `cancel_operation` only to that operation's provider, once per cancellation request, never to unrelated contributors or every alias backed by a shared handler. The runtime pins the dispatcher used for execution rather than resolving the Object again on cancellation, since dynamic resolvers may return stateful instances. Providers may implement `cancel_operation` for operation-specific control or the shared `cancel` default. Terminal completion or a dropped execution future releases the active binding; unknown/finished execution IDs do not invoke any provider. A cancellation request is not itself terminal confirmation.

An unknown mutating outcome must never be retried automatically. The model must inspect external state or ask for operator guidance. Retrieval failures may be explicitly refreshed.

## Measurement

The runtime tracks bounded counters for:

- ordinary individual-tool schema bytes hidden by WIP mode;
- the three WIP gateway schema bytes;
- discover, descriptor-inspection, and operation round trips;
- successful operations.

These values are returned with discovery/inspection output and are intended for comparing prompt input volume, exploration round trips, and representative operation success with ordinary Tool mode. No fixed reduction percentage is promised.
