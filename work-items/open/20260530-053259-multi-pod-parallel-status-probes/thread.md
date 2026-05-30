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
