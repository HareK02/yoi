# Web Console renders one Worker notification twice

Date: 2026-08-06

## Symptom

A single Workspace Orchestrator attention notification appears in Web Console as two adjacent system lines with identical content:

- `System · embedded worker notification`
- `System · notification`

The first label is also misleading for Workers running on a non-embedded Runtime.

## Confirmed cause

`worker-runtime` projects one accepted `WorkerInputKind::Notify` through two independent protocol-event paths.

1. `Runtime::interact_worker` calls `input_protocol_event(&input)` after the execution backend acknowledges the input and pushes the result to the Worker observation bus. For `Notify`, `input_protocol_event` constructs a synthetic `protocol::Event::SystemItem` with kind `embedded_worker_notification`.
2. The real Worker drains `NotifyBuffer`, commits `SystemItem::Notification` to Worker history, and the Runtime execution bridge republishes the committed log entry as another `protocol::Event::SystemItem` with kind `notification`.

The Workspace subscription forwards both events. `web/workspace/src/lib/workspace/console/model.ts` renders every `system_item` independently and has no semantic deduplication between these two different kinds/event identities.

The authoritative history item is the second event. The synthetic input-observation item is not the durable Worker-history authority, so the LLM should receive the notification once even though Web Console displays it twice.

## Relevant code

- `crates/worker-runtime/src/runtime.rs`
  - `input_protocol_event(WorkerInputKind::Notify)` created `embedded_worker_notification`.
  - `Runtime::interact_worker` unconditionally published that synthetic event after accepted input.
- `crates/worker-runtime/src/worker_backend.rs`
  - the controller bridge republishes committed `SegmentLogSink` entries through `live_log_entry_event`.
- `web/workspace/src/lib/workspace/console/model.ts`
  - both system-item variants are rendered as separate lines.

## Resolution

`worker-runtime` no longer converts accepted `Notify` input into a synthetic protocol event. `input_protocol_event` returns no event for Notify, while the normal interaction acknowledgement remains unchanged. The committed `SystemItem::Notification` is therefore the single Console-visible projection.

A caller-boundary regression test drives accepted user and Notify inputs, verifies that Notify adds no synthetic observation, then publishes the committed notification event and asserts that exactly one notification system item is present.
