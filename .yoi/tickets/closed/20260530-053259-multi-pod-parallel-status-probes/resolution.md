---
id: 20260530-053259-multi-pod-parallel-status-probes
slug: multi-pod-parallel-status-probes
title: Parallelize multi-Pod live status probes
status: closed
kind: task
priority: P2
labels: [tui, pod-dashboard, performance]
created_at: 2026-05-30T05:32:59Z
updated_at: 2026-05-30T05:45:37Z
assignee: null
legacy_ticket: null
---

## Background

The `--multi` dashboard frequently shows `[live unknown]` for reachable Pods. Current code probes each runtime-registry socket with a very short `LIVE_STATUS_PROBE_TIMEOUT` of 25ms in `crates/tui/src/pod_list.rs`. A live row becomes `status = None` when the socket connects but no `Event::Snapshot` / `Event::Status` is read before that deadline.

That label is misleading: the Pod is reachable, but status probing timed out or did not receive a status event quickly enough. Raising the timeout alone risks making dashboard reload latency scale linearly with the number of live Pods, because status probes are currently performed sequentially.

## Requirements

- Increase the live status probe timeout to a more realistic value, likely in the 150ms–250ms range.
- Run live status probes concurrently so reload latency does not become the sum of all per-Pod timeouts.
- Keep reachable Pods with missing status as live/attachable; do not treat status timeout as unreachable.
- Keep restoreability separate from live attachability; this ticket must not make runtime-only Pods restorable.
- Replace or soften the `live unknown` label in `--multi` so it communicates reachable-live-with-unreported-status rather than broken state. Candidate labels: `live`, `live probing`, or similar.
- Keep the implementation in shared `PodList` / live probe code where possible; avoid duplicating dashboard-specific discovery logic.
- Preserve existing behavior for explicitly reported `Idle`, `Running`, and `Paused` statuses.

## Non-goals

- Do not redesign Pod notification or run completion delivery.
- Do not persist last-known status in pod-store.
- Do not change `AttachOrRestorePod` or restore semantics.
- Do not make unreachable registry allocations appear attachable.

## Acceptance criteria

- Multiple live Pod status probes wait concurrently, not strictly one after another.
- The per-Pod timeout is long enough to significantly reduce false `status = None` cases compared to 25ms.
- A reachable Pod whose status probe times out remains displayed as live and openable/attachable.
- The multi-Pod row label for `status = None` is less misleading than `live unknown`.
- Tests cover concurrent probing behavior, timeout/none-status handling, and label rendering.
- `cargo test -p tui pod_list`, `cargo test -p tui multi_pod`, `cargo test -p tui`, `cargo fmt --check`, and `./tickets.sh doctor` pass.
