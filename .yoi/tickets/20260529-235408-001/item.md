---
title: "Preserve provider stream trace recording across profile-spawned Pods"
state: "closed"
created_at: "2026-05-29T23:54:08Z"
updated_at: "2026-05-30T00:38:28Z"
---

## Background

`SessionConfig::record_event_trace` controls the debug sidecar `{segment_id}.trace.jsonl`, which records normalized provider stream events and lifecycle markers outside the durable segment log. It is intentionally separate from session replay, but it is important when diagnosing provider failures that are visible only as live events.

During the `session-pod-state-boundary` implementation handoff, a child Pod showed `[ProviderError]` with `error_code: context_length_exceeded` in TUI for a `ping` request. The child's session JSONL had no `run_errored`, and there was no `.trace.jsonl` sidecar. Inspecting the child's runtime manifest showed:

```toml
[session]
record_event_trace = false
```

This suggests the profile/spawn configuration path may have dropped or reset a previously enabled debug trace setting when profiles were introduced or when `SpawnPod` materializes its child manifest. Even if the immediate provider-error persistence bug is fixed separately, disabling the trace sidecar for child Pods makes diagnosis of live-only provider events much harder.

The current trace sidecar is not fully raw SSE; it records parsed `llm_worker::llm_client::event::Event` values plus lifecycle/transport diagnostics. Unknown OpenAI Responses SSE events are surfaced as `Event::UnhandledSse` with bounded previews, while known terminal events such as `response.failed` become `Event::Error`.

## Goal

Make provider stream/event trace configuration survive profile resolution and spawned-Pod manifest construction so debugging settings remain effective for child Pods, and make the resulting behavior explicit.

## Acceptance criteria

- Identify where `session.record_event_trace` is lost or defaulted during profile resolution, manifestization, `SpawnPod`, or hidden spawn-config construction.
- Preserve the intended trace setting for spawned Pods when the parent/profile configuration enables it, unless an explicit child override disables it.
- Add focused tests covering profile-resolved manifests and spawned-Pod manifest construction with `record_event_trace = true`.
- Clarify in docs or comments that `.trace.jsonl` stores normalized stream events/lifecycle diagnostics, not necessarily byte-for-byte raw SSE.
- Do not make event trace globally enabled by default; it remains an opt-in debugging feature.
- Validate with focused manifest/pod tests and `./tickets.sh doctor`.
