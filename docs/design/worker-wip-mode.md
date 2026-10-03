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

Its single operation is `call`. For this compatibility route, `WipCall.arguments` is the original ordinary tool argument object. Native projections instead receive a JSON object keyed by the descriptor's declared parameter names. Tool argument IDs remain JSON values; the adapter never infers a domain object or Worldspace route from them.

The interface descriptor uses WIP `Json` for compatibility input and includes the exact original JSON Schema in descriptor documentation. Before execution, the Host validates the JSON value with that original schema and then delegates to the original async `Tool`. Constraints are therefore neither approximated nor silently dropped. A schema that cannot be compiled prevents WIP startup instead of creating a weaker projection.

## Native projection extension and collisions

`worker::wip::WipMountRegistry` is the Host-owned mount allocator. A Feature can independently construct a `WipProjection` with an async `WipOperationHandler`, then pass its aggregate registry through `install_wip_mode_with_mounts` before compatibility finalization.

- Routes must be canonical absolute WIP paths.
- Object names must match the final route segment.
- A projected object exposes exactly its declared interface.
- Unrelated duplicate routes and conflicting interface descriptors fail startup.
- A native projection replaces a compatibility projection only when both declare the same semantic capability (`tool:<registration-name>` for compatibility tools).
- When native is already selected, the compatibility tool is hidden rather than exposed through a second entry.

The unit samples `mount_collision_and_native_replacement_are_explicit` and `native_projection_accepts_descriptor_named_arguments_end_to_end` demonstrate independent native registration, deterministic replacement, collision rejection, and a complete native invocation.

## Authority and execution

Discovery is not authority. The Worldspace contains only the tools already registered from enabled Features and the Worker's current execution context. Calling a compatibility operation:

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

## Errors, cancellation, and retries

The adapter keeps these outcomes distinct:

- **rejected/not dispatched**: route, membership, validator, descriptor, schema, or permission failure; the original operation does not run;
- **stale**: object/interface validator mismatch; refresh before reconstructing a call;
- **cancelled**: the original provider confirms cooperative cancellation and its bounded cancelled output is retained;
- **interrupted**: provider interruption is retained separately from cancellation;
- **disconnected/timeout after dispatch**: `wip-client` records an unknown outcome;
- **operation outcome unknown**: an executing provider fails without a rollback/non-execution guarantee, a response cannot be completed after dispatch, or the call future is force-closed.

An unknown mutating outcome must never be retried automatically. The model must inspect external state or ask for operator guidance. Retrieval failures may be explicitly refreshed.

## Measurement

The runtime tracks bounded counters for:

- ordinary individual-tool schema bytes hidden by WIP mode;
- the three WIP gateway schema bytes;
- discover, descriptor-inspection, and operation round trips;
- successful operations.

These values are returned with discovery/inspection output and are intended for comparing prompt input volume, exploration round trips, and representative operation success with ordinary Tool mode. No fixed reduction percentage is promised.
