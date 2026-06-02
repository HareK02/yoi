# Investigation summary: peer Pod handshake

Date: 2026-06-02

## Current authority map

- `ListPods` / `RestorePod` are implemented in `crates/pod/src/discovery.rs` and are served both as LLM tools and protocol methods from the currently attached Pod's controller. They intentionally start from the caller Pod's visibility set rather than a host-wide Pod universe. Today the visibility set is the caller itself plus spawned children from durable Pod metadata and the live spawned-child registry.
- Pod metadata is in `crates/pod-store/src/lib.rs` as name-keyed current state (`PodMetadata`) under the Pod-state root. `spawned_children` is durable current parent/child visibility and delegation state; session JSONL remains the durable explanation for delivered context/history.
- The spawned-child registry (`SpawnedPodRegistry`) is runtime/current ownership state for `SpawnPod`, `SendToPod`, `ReadPodOutput`, and `StopPod`. It carries output cursors and delegated scope; it should not be reused for peer visibility.
- `SendToPod` in `crates/pod/src/spawn/comm_tools.rs` is explicitly spawned-child scoped: it looks up only `SpawnedPodRegistry`, sends `Method::Run`, and is paired with `ReadPodOutput` cursor semantics. Broadening it would blur child vs peer semantics, so a distinct peer-safe send surface is preferable.
- Protocol methods are in `crates/protocol/src/lib.rs`; TUI command dispatch is local parsing in `crates/tui/src/command.rs`, returning typed `Method`s that `single_pod.rs` sends to the currently attached Pod. This is suitable for a TUI command that asks the current Pod to perform authoritative metadata changes.
- Delivered notifications already have an explainable durable path: `Method::Notify` is queued through `NotifyBuffer`, rendered as a `SystemItem`, committed as `LogEntry::SystemItem`, and then appended to model history. A peer message can reuse that receive-side path without hidden context injection.

## Implementation direction

No escalation blocker found. The minimal safe design is:

- Add a `peers` field to `PodMetadata`, separate from `spawned_children`, with serde defaults so no broad migration is needed.
- Add a controller-handled `Method::RegisterPeer { name }` that validates the target Pod state exists, rejects self, and writes reciprocal peer entries to both Pod metadata records. Normal write failures roll back the first side so successful replies do not leave one-sided visibility.
- Extend `ListPods` visibility with a `peer` reason sourced from `PodMetadata.peers`; keep spawned-child comm registry and child output cursors unchanged.
- Add a separate `SendToPeerPod` tool backed by `PodDiscovery` visibility. It only sends to visible peers, uses the target's live/restored socket from discovery, and delivers a `Method::Notify` message labeled with the sender Pod name. It does not read output, stop Pods, grant scope, or use `SpawnedPodRegistry`.
- Add TUI `:peer <name>` command that emits `Method::RegisterPeer { name }` to the attached Pod and shows local diagnostics; the controller returns a typed result event for success and normal `Event::Error` for failure.

Crash-level atomicity for two metadata files is not provided by the existing store, but the implementation can avoid misleading partial state for ordinary validation/write failures and does not require broad schema migration or hidden history changes.
