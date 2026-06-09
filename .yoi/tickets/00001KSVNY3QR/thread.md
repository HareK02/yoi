<!-- event: create author: tickets.sh at: 2026-05-30T05:32:59Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-05-30T05:33:43Z -->

## Plan

## Preflight

Classification: implementation-ready.

The issue is localized to TUI live status probing and dashboard labeling. The product decision is settled: reachable Pods with missing status remain live/attachable, while status probe timeout should be less frequent and less alarming. No restore semantics or Pod runtime protocol changes are required.

Current code map:
- `crates/tui/src/pod_list.rs`: `LIVE_STATUS_PROBE_TIMEOUT`, `load_live_pods`, `probe_live_status`, `LivePodInfo`, `PodList` merge/sort tests.
- `crates/tui/src/multi_pod.rs`: row label rendering for live status, currently `live unknown` when `LivePodInfo.status` is `None`.

Critical risks:
- Do not make reload latency scale as N * timeout.
- Do not treat `status = None` as unreachable.
- Do not change restoreability/attachability semantics.
- Avoid duplicating discovery logic outside shared `PodList`.


---

<!-- event: review author: hare at: 2026-05-30T05:45:14Z status: approve -->

## Review: approve

Approve.

The implementation addresses the ticket in the shared `PodList` live-probe path. Live status probes now run concurrently with a more realistic per-Pod timeout, reachable sockets without reported status remain live/attachable, and the multi-Pod label is softened from `live unknown` to `live` while explicit statuses remain unchanged.

Blocker findings: none.

Requirement coverage:
- Per-Pod timeout increased to 200ms.
- Probes are concurrent and bounded by the slowest bounded probe rather than `N * timeout`.
- Reachable sockets with no status remain reachable/live/attachable.
- Restoreability remains separate and unchanged.
- No last-known status is persisted to pod-store.
- Explicit `live idle`, `live running`, and `live paused` labels remain.
- `status = None` renders as `live`.
- The implementation changes shared `PodList` live-probe logic rather than duplicating dashboard-only logic.
- Tests cover concurrency, timeout/no-status reachable handling, and label rendering.

Validation reviewed:
- `cargo fmt --check` — passed.
- `cargo test -p tui pod_list` — passed.
- `cargo test -p tui multi_pod` — passed.
- `cargo test -p tui` — passed.
- Reviewer additionally ran `./tickets.sh doctor` — passed.

Final verdict: approve.


---

<!-- event: close author: hare at: 2026-05-30T05:45:37Z status: closed -->

## Closed

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


---
