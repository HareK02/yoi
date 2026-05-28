# Crate boundary audit

Date: 2026-05-28

## summary

The workspace dependency graph is broadly acyclic and mostly layered in the expected direction: `protocol` / `lint-common` / proc-macros sit at the bottom, `llm-worker` / `manifest` / `tools` / `provider` / `session-store` provide shared infrastructure, and `pod` / `tui` are orchestration or UI layers. I did not find a hard Cargo-level cycle or an obvious UI crate being depended on by a lower crate.

The main boundary problems are subtler:

1. `protocol` exposes several public wire payloads as `serde_json::Value` while documenting them as the JSON form of `session_store::*` types. This avoids a Rust dependency edge but creates a hidden schema dependency from `protocol`/clients to `session-store`.
2. `workflow` depends on `memory` for `WorkspaceLayout`, and `memory::WorkspaceLayout` owns workflow paths. This makes `memory` a cross-domain workspace-layout hub rather than only the memory subsystem.
3. Several lower/shared crates have comments/doc-comments explaining `Pod`, `TUI`, controller, prompt-catalog, or downstream orchestration behavior. Most are acceptable integration-contract notes, but a few are implementation knowledge that should move upward or be generalized.

No code, formatting, commits, merges, or project-record files outside `artifacts/` were changed.

## inspected commands / files

### Commands run

- `cargo metadata --no-deps --format-version 1 | jq ... > artifacts/deps.txt`
  - Extracted workspace-internal normal/dev dependency edges.
- `cargo metadata --no-deps --format-version 1 | jq ... > artifacts/reverse-deps.txt`
  - Extracted reverse dependency summary.
- `rg -n 'pub (struct|enum|fn|mod|trait|type|use) ...' crates --glob '*.rs' > artifacts/public-concept-hits.txt`
  - Searched public APIs for boundary-relevant terms (`Pod`, `TUI`, `Workflow`, `Manifest`, `Memory`, `Session`, etc.).
- `rg -n '(^\s*(//!|///|//)\s?.*(...))' crates --glob '*.rs' > artifacts/comment-concept-hits.txt`
  - Searched comments/doc-comments for crate names and upper-layer concepts.
- `rg -n 'TUI / GUI|session_store::|parent Controller|Pod treats|Pod side|...' ... > artifacts/suspicious-excerpts.txt`
  - Narrowed suspicious comment excerpts.
- `rg -n 'use (session_store|pod_registry|llm_worker|manifest)::|...' crates/tui/src`
  - Checked why TUI depends on lower internal crates.
- `rg -n 'WorkspaceLayout|memory::' crates/workflow/src`
  - Checked `workflow -> memory` dependency use.

Failed exploratory commands:

- `python` / `python3` parse attempts failed because Python was not available in the environment; switched to `cargo metadata` + `jq`.

Supplemental raw outputs left in the artifact directory:

- `deps.txt`, `reverse-deps.txt`
- `deps-numbered.txt`, `reverse-deps-numbered.txt`
- `public-concept-hits.txt`
- `comment-concept-hits.txt`
- `suspicious-excerpts.txt`

### Main files inspected directly

- Root/workspace:
  - `Cargo.toml`
  - `work-items/open/20260528-131317-crate-boundary-audit/item.md`
- Cargo manifests:
  - `crates/protocol/Cargo.toml`
  - `crates/manifest/Cargo.toml`
  - `crates/llm-worker/Cargo.toml`
  - `crates/pod/Cargo.toml`
  - `crates/client/Cargo.toml`
  - `crates/tui/Cargo.toml`
  - `crates/memory/Cargo.toml`
  - `crates/workflow/Cargo.toml`
  - `crates/provider/Cargo.toml`
  - `crates/session-store/Cargo.toml`
  - `crates/pod-registry/Cargo.toml`
  - `crates/session-metrics/Cargo.toml`
  - `crates/tools/Cargo.toml`
  - `crates/daemon/Cargo.toml`
  - `crates/lint-common/Cargo.toml`
  - `crates/llm-worker-macros/Cargo.toml`
- Public/API and suspicious source files:
  - `crates/protocol/src/lib.rs`
  - `crates/manifest/src/lib.rs`
  - `crates/manifest/src/model.rs`
  - `crates/llm-worker/src/lib.rs`
  - `crates/llm-worker/src/interceptor.rs`
  - `crates/llm-worker/src/llm_client/types.rs`
  - `crates/pod/src/lib.rs`
  - `crates/pod/src/pod.rs` (grep/read excerpts)
  - `crates/pod/src/spawn/comm_tools.rs` (grep excerpts)
  - `crates/client/src/lib.rs`
  - `crates/client/src/spawn.rs`
  - `crates/tui/src/app.rs` (grep excerpts)
  - `crates/tui/src/spawn.rs` (grep excerpts)
  - `crates/tui/src/picker.rs` (grep excerpts)
  - `crates/memory/src/lib.rs`
  - `crates/memory/src/scope.rs`
  - `crates/memory/src/workspace.rs`
  - `crates/memory/src/extract/mod.rs` (grep excerpts)
  - `crates/memory/src/consolidate/mod.rs` (grep excerpts)
  - `crates/memory/src/resident.rs` (grep excerpts)
  - `crates/workflow/src/lib.rs`
  - `crates/workflow/src/linter.rs` (grep excerpts)
  - `crates/workflow/src/scope.rs` (grep excerpts)
  - `crates/workflow/src/workflow.rs` (grep excerpts)
  - `crates/session-store/src/lib.rs`
  - `crates/session-store/src/segment.rs`
  - `crates/session-store/src/segment_log.rs` (grep excerpts)
  - `crates/session-store/src/system_item.rs`
  - `crates/session-store/src/pod_metadata.rs`
  - `crates/pod-registry/src/lib.rs`
  - `crates/provider/src/lib.rs`
  - `crates/tools/src/lib.rs`

## dependency graph overview

Internal dependency edges from `cargo metadata --no-deps`:

```text
client -> manifest, protocol
daemon -> manifest, protocol
lint-common -> (none)
llm-worker -> llm-worker-macros
llm-worker-macros -> (none)
manifest -> llm-worker, protocol
memory -> lint-common, llm-worker, manifest
pod -> llm-worker, manifest, memory, pod-registry, protocol, provider, session-metrics, session-store, tools, workflow
pod-registry -> manifest, session-store
protocol -> (none)
provider -> llm-worker, manifest
session-metrics -> session-store
session-store -> llm-worker, protocol
tools -> llm-worker, manifest
tui -> client, llm-worker, manifest, pod-registry, protocol, session-store; dev-dep tools
workflow -> lint-common, manifest, memory
```

Reverse summary:

```text
client <- tui
lint-common <- memory, workflow
llm-worker <- manifest, memory, pod, provider, session-store, tools, tui
manifest <- client, daemon, memory, pod, pod-registry, provider, tools, tui, workflow
memory <- pod, workflow
pod-registry <- pod, tui
protocol <- client, daemon, manifest, pod, session-store, tui
provider <- pod
session-metrics <- pod
session-store <- pod, pod-registry, session-metrics, tui
tools <- pod, tui
workflow <- pod
```

This is directionally reasonable for orchestration-heavy code: `pod` is the main integrator; `tui` sits above `client` but also reads lower schemas; `protocol` has no Rust workspace dependencies.

## dependency/interface findings grouped by severity

### Severity: actual problem / should ticket

#### 1. `protocol` public API has hidden `session-store` schema coupling through `serde_json::Value`

Evidence:

- `crates/protocol/src/lib.rs:237` documents `Event::SystemItem.item` as the JSON form of `session_store::SystemItem`.
- `crates/protocol/src/lib.rs:394` documents `Event::Snapshot.entries` as the JSON form of `session_store::LogEntry`.
- `crates/protocol/src/lib.rs:419` documents `Event::SegmentRotated.entry` as the JSON form of `session_store::LogEntry::SegmentStart`.
- `crates/tui/src/app.rs:1236` and nearby lines deserialize snapshot entries back into `session_store::LogEntry`.
- `crates/tui/src/app.rs:1277` and nearby lines deserialize `Event::SystemItem.item` into `session_store::SystemItem`.

Why this is a boundary issue:

- `protocol` is dependency-free at the Cargo level, but its wire contract is not actually self-owned: clients must know `session-store` schemas to reconstruct state correctly.
- The type system cannot enforce compatibility between `protocol` and `session-store` because the public protocol type is only `serde_json::Value`.
- This explains why `tui` depends directly on `session-store` despite also depending on `client`/`protocol`.

Recommended direction:

- Extract the wire-stable log/system-item DTOs into a neutral crate, or move protocol-facing DTOs into `protocol` and have `session-store` convert to/from them.
- Avoid public protocol docs that say “this is `session_store::X` JSON” unless `session-store` is intentionally part of the protocol contract and typed as such.

#### 2. `workflow -> memory` dependency exists for shared workspace layout

Evidence:

- `crates/workflow/Cargo.toml:11` depends on `memory`.
- `crates/workflow/src/linter.rs:5`, `crates/workflow/src/scope.rs:6`, `crates/workflow/src/workflow.rs:17` use `memory::WorkspaceLayout`.
- `crates/memory/src/workspace.rs:8` includes `<root>/.insomnia/workflow/<slug>.md` in memory's layout documentation.
- `crates/memory/src/workspace.rs:16-18` says workflows are human-managed and live one level up under `.insomnia/workflow/`.
- `crates/memory/src/workspace.rs:127-165` exposes `workflow_dir()` / `workflow_path()` from the memory crate.

Why this is a boundary issue:

- `workflow` is conceptually a sibling subsystem, not a consumer of generated memory state.
- The current dependency is only for path layout. That makes `memory` own cross-subsystem workspace conventions and forces workflow to import a memory-domain crate for non-memory concerns.
- This is not severe yet, but it will make future workflow growth pull against crate ownership.

Recommended direction:

- Extract `WorkspaceLayout` / `.insomnia` path conventions into a neutral crate or a neutral module under `manifest`/new `workspace-layout` crate.
- Then make `memory` and `workflow` both depend on that neutral layout instead of depending on each other.

### Severity: suspicious but currently acceptable

#### 3. `session-store` owns Pod metadata and spawned-child metadata

Evidence:

- `crates/session-store/src/pod_metadata.rs:1-6` defines “Pod metadata persistence API”.
- `crates/session-store/src/pod_metadata.rs:42-60` defines `PodSpawnedScopeRule` / `PodSpawnedChild`, including delegated scope and `callback_address`.
- `crates/session-store/src/pod_metadata.rs:62-88` exposes `PodMetadata` and `PodMetadataStore` publicly.

Assessment:

- This is Pod/orchestration-specific state inside a crate named `session-store`.
- It is acceptable if `session-store` is intentionally “insomnia persistence primitives”, not a generic conversation-log crate. Current project decisions appear to lean that way.
- If the intended boundary is “session-store only stores sessions/segments/logs”, this should be split or renamed. If the intended boundary is “session-store stores all durable Pod state”, the naming/docs should say that explicitly.

Recommended direction:

- No immediate refactor unless the ownership goal changes.
- Clarify crate-level docs: either broaden `session-store`'s stated responsibility to durable Pod/session persistence, or split Pod metadata into a `pod-state`/`pod-metadata` crate.

#### 4. TUI directly depends on persistence/registry crates

Evidence:

- `crates/tui/Cargo.toml` depends on `session-store`, `pod-registry`, `manifest`, `llm-worker`, and `protocol` in addition to `client`.
- `crates/tui/src/picker.rs` uses `pod_registry::{LockFileGuard, default_registry_path}` and `session_store::{...}`.
- `crates/tui/src/app.rs:1236-1298` parses `session_store::LogEntry` / `session_store::SystemItem`.
- `crates/tui/src/spawn.rs:408-409` uses `session_store::FsStore` and `restore_by_segment` for resume-related paths.

Assessment:

- TUI is a top-level crate, so dependency direction is allowed.
- The direct `session-store` parse dependency is largely a symptom of finding #1: protocol sends untyped JSON whose real schema lives in `session-store`.
- Direct `pod-registry` access for picker/runtime discovery may be acceptable for a local-first TUI, but it bypasses a cleaner “TUI talks protocol/client only” boundary.

Recommended direction:

- Fix protocol DTO ownership first.
- After that, re-evaluate whether TUI still needs direct `session-store` and `pod-registry` dependencies or whether picker/discovery can move behind `client`/`protocol` APIs.

#### 5. `manifest -> llm-worker` dependency is acceptable but should remain one-way

Evidence:

- `crates/manifest/Cargo.toml` depends on `llm-worker` and `protocol`.
- `crates/manifest/src/model.rs:17-19` re-exports `llm_worker::llm_client::capability::{ModelCapability, ReasoningControl, ReasoningEffort}`.

Assessment:

- This is a reasonable tradeoff to avoid duplicate model-capability types.
- It does mean `manifest` is not a pure data crate independent of worker runtime types.
- The boundary remains acceptable as long as `llm-worker` does not depend back on `manifest`, and provider-level resolution stays in `provider`.

### Severity: no issue found

- No Rust workspace dependency cycle was found in the inspected graph.
- I did not find lower crates depending on `tui` or `client` implementation crates.
- `client -> protocol/manifest` and `pod -> provider/tools/session-store/memory/workflow` are directionally appropriate.
- `provider -> llm-worker/manifest` is appropriate: provider constructs concrete `LlmClient` implementations from resolved model configuration.
- `tools -> llm-worker/manifest` is appropriate: tools expose `ToolDefinition`s and enforce manifest scopes.
- `pod-registry -> session-store` is acceptable if registry entries need session/segment identity and durable state coordination.

## comment/doc-comment findings

### Problematic or should be generalized

#### `protocol` describes parent controller and pod-registry side effects

- `crates/protocol/src/lib.rs:65-70`
  - `PodEvent` docs say the “parent Controller applies variant-specific side effects (registry / pod-registry updates)”.
  - This is implementation knowledge from the `pod` crate inside a dependency-free protocol crate.
  - Better: state the wire contract (“event is delivered to the parent; receiver is responsible for handling lifecycle effects”) and keep registry-specific behavior in `pod` docs.

#### `protocol` documents `session_store::*` JSON shapes as protocol payloads

- `crates/protocol/src/lib.rs:237`
- `crates/protocol/src/lib.rs:394`
- `crates/protocol/src/lib.rs:419`

This is the comment-level manifestation of the public-interface issue in finding #1.

#### `llm-worker` public request docs mention Pod-specific cache-key choice

- `crates/llm-worker/src/llm_client/types.rs:523-526`
  - `Request::cache_key` doc says pod side is expected to pass `SegmentId`.
  - `llm-worker` should expose the generic concept: a stable caller-provided conversation/cache namespace key.
  - Pod's choice of `SegmentId` belongs in `pod` docs/tests, not in the generic request type.

#### `memory` docs prescribe Pod assembly details

- `crates/memory/src/lib.rs:3-7`
  - Says generic CRUD tools must not touch memory/knowledge and Pod is responsible for denying them.
- `crates/memory/src/scope.rs:4-8`
  - Says Pod is expected to call `deny_write_rules` and pass the result to `tools::ScopedFs`.
- `crates/memory/src/extract/mod.rs:3-14`
  - Explains Pod post-run hook, `PromptCatalog`, `PodPrompt::MemoryExtractSystem`, and pointer persistence responsibility.
- `crates/memory/src/consolidate/mod.rs:5-15`
  - Explains Pod assembling a disposable Worker and using `PodPrompt::MemoryConsolidationSystem`.
- `crates/memory/src/resident.rs:3-11`
  - Says surfaces are used by the Pod system-prompt assembler and Pod IPC layer for TUI `#` completion.

Assessment:

- These are understandable because `memory` is currently a helper subsystem consumed by `pod`.
- They nevertheless make `memory` read like it is documenting Pod orchestration rather than memory-owned contracts.
- Prefer caller-neutral wording: “the orchestrator/caller registers these tools”, “the caller persists the pointer”, “completion consumers may use ...”. Keep Pod-specific sequence docs in `pod`.

### Suspicious but acceptable integration-contract comments

#### `protocol::Segment` docs mention TUI/GUI and Pod parsing behavior

- `crates/protocol/src/lib.rs:116-126`
  - Mentions richer clients (TUI/GUI) producing typed atoms and Pod not re-parsing flattened strings.
- `crates/protocol/src/lib.rs:143-153`
  - Mentions Pod resolving `FileRef` and treating unknown variants as unresolved input.
- `crates/protocol/src/lib.rs:222-231`
  - Mentions additional TUI/GUI instances rendering user messages.

Assessment:

- Mentioning client classes can be acceptable in protocol docs when it explains wire semantics.
- The Pod behavior details are more debatable; they should be limited to required protocol semantics, not specific controller implementation.

#### `session-store::SystemItem` mentions TUI typed rendering

- `crates/session-store/src/system_item.rs:27-35`
- `crates/session-store/src/system_item.rs:49-52`

Assessment:

- It is valid to document why typed payload exists.
- “so the TUI can render” should probably be generalized to “so clients can render” because `session-store` is lower than `tui`.

#### `session-store::segment` mentions Pod as typical caller

- `crates/session-store/src/segment.rs:3-5`
- `crates/session-store/src/segment.rs:38-40`
- `crates/session-store/src/segment.rs:175-180`
- `crates/session-store/src/segment.rs:252-254`

Assessment:

- Mostly acceptable because Pod is currently the primary writer.
- Better wording would say “caller/orchestrator” first and optionally “e.g. Pod” only where it clarifies current integration.

#### `client` docs mention TUI/GUI/E2E

- `crates/client/src/lib.rs:9`
- `crates/client/src/spawn.rs:4-10`
- `crates/client/src/spawn.rs:92-96`

Assessment:

- Acceptable: `client` is explicitly a library for UI/GUI/E2E callers to speak Pod protocol. These are consumer examples rather than lower-layer implementation leakage.

#### `llm-worker::Interceptor` docs mention Pod as an upper layer

- `crates/llm-worker/src/interceptor.rs:3-6`
- `crates/llm-worker/src/interceptor.rs:122-126`
- `crates/llm-worker/src/interceptor.rs:140-146`

Assessment:

- Mostly acceptable: the docs explicitly say Worker does not know higher-level concepts and Pod is only an example upper layer.
- For stricter boundary hygiene, prefer “upper layers/orchestrators” and avoid naming Pod except in examples.

## acceptable dependency-aware comments criteria

I treated a comment as acceptable when it met at least one of these criteria:

1. It explains a public wire or file-format contract that consumers must honor, without prescribing one consumer's private implementation.
2. It names a higher layer only as an example (`e.g. Pod`) while the API remains generic and caller-owned.
3. It documents an intentional direction-of-control boundary, such as “the lower crate exposes a hook; upper layers implement policy”.
4. It references another crate that the current crate actually depends on and whose type or function is part of the local API.
5. It appears in tests/examples whose purpose is cross-crate contract verification.

I treated a comment as problematic when it did any of the following:

1. A lower crate explains what a dependent higher crate currently does internally.
2. A lower crate's public docs define a payload as another higher crate's private or semi-private JSON schema.
3. A shared subsystem describes its API mainly as a sequence of Pod/TUI orchestration steps, rather than a caller-neutral contract.
4. The comment reveals a hidden dependency that Cargo cannot type-check.

## recommended follow-up tickets

1. **Typed protocol snapshot/system-item payloads**
   - Goal: remove `protocol` public `serde_json::Value` payloads whose real schemas are `session_store::*`.
   - Candidate implementation directions:
     - Move wire DTOs for log entries/system items into `protocol`, with `session-store` converting to/from them; or
     - Extract a neutral `session-log-schema` / `wire-log` crate used by both `protocol` and `session-store`.
   - Success condition: TUI/client code can parse snapshots/system items using protocol-owned typed structures, not `session_store::LogEntry` hidden behind JSON.

2. **Extract neutral workspace layout from `memory`**
   - Goal: remove `workflow -> memory` when the only need is `.insomnia` path layout.
   - Candidate implementation directions:
     - New neutral crate/module for `WorkspaceLayout`; or
     - Move `.insomnia` path layout into `manifest` if that crate is intended to own workspace configuration.
   - Success condition: `workflow` and `memory` are siblings depending on a neutral layout owner.

3. **Boundary-comment hygiene pass**
   - Goal: replace reverse-knowledge comments in lower/shared crates with caller-neutral wording.
   - Scope:
     - `protocol/src/lib.rs` controller/session-store JSON wording.
     - `llm-worker/src/llm_client/types.rs` Pod `SegmentId` cache-key wording.
     - `memory/src/{scope,extract,consolidate,resident}.rs` Pod/TUI orchestration wording.
     - `session-store/src/{system_item,segment}.rs` TUI/Pod-specific wording where not required.
   - Success condition: comments explain local contracts and extension points; dependent-crate implementation details live in the dependent crate.

4. **Clarify `session-store` crate responsibility**
   - Goal: decide whether `session-store` is only session/segment log storage or the broader durable Pod-state persistence crate.
   - If broader: update crate docs/naming comments to say so.
   - If narrower: split `pod_metadata` into a Pod-owned persistence crate/module.

## unresolved questions

1. Is `protocol` intended to be the sole owner of all stable wire DTOs, or is `session-store` intentionally part of the protocol contract despite the current `serde_json::Value` indirection?
2. Is `session-store` deliberately the durable state crate for all Pod metadata, or should it be constrained to conversation/session logs?
3. Should `WorkspaceLayout` be considered a memory-domain concept, or a repository/workspace-domain concept shared by memory, knowledge, and workflow?
4. Should TUI remain allowed to inspect local registry/session files directly for picker and restore UX, or should those capabilities move behind `client`/`protocol` APIs?
5. Are comments allowed to name the primary current consumer (`Pod`) when documenting a generic lower-layer extension point, or should comments avoid such names unless the type itself is Pod-specific?
