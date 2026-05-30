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
