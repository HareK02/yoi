<!-- event: migration author: tickets.sh-migration at: 2026-05-27T00:00:16Z -->

## Migrated

Migrated from tickets/tui-picker-live-pending-pods.md. No legacy review file was present at migration time.

---

<!-- event: plan author: hare at: 2026-05-30T04:54:03Z -->

## Plan

## Preflight implementation plan

Classification: implementation-ready.

No blocking preflight gap remains. The product rule is settled: reachable live Pods must be visible/attachable even if durable session-log metadata is incomplete, but missing session logs must not make them restorable.

Implementation detail to preserve:
- Treat “pending live” as a display/model condition, not persisted state.
- Use reachable `LivePodInfo` plus incomplete stored/session summary or runtime-only segment id to improve row order/preview/debug ids.
- Do not mark the Pod restorable unless stored metadata has a usable active segment/session under existing restore rules.

Current code map:
- `crates/tui/src/picker.rs`: picker construction, row rendering, live attach socket override.
- `crates/tui/src/pod_list.rs`: shared model merge/sort/truncation/actions; current sort is updated_at desc only; `merge_live` already supplements segment id from runtime.
- `crates/tui/src/main.rs`: selected live row attaches via socket override before restore fallback.
- `crates/tui/src/multi_pod.rs`: also uses `PodList`, so ordering effects should be checked.
- `crates/pod/src/discovery.rs`: List/Attach/Restore behavior is related but out of scope.
- `crates/pod-registry/src/table.rs`: runtime allocation segment id source.
- `crates/pod-store/src/lib.rs`: pending active segment metadata; do not persist runtime supplementation.

Implementation phases:
1. Change `PodList::from_sources` sorting to reachable-live first, then updated_at desc, then pod_name asc; truncation remains after sorting.
2. Make reachable live pending preview explicit, e.g. `[live, pending segment]`, when durable summary is incomplete.
3. Preserve and test runtime segment id supplementation for display/debug ids only.
4. Add focused `pod_list` tests for live-first-before-truncation, live pending runtime segment attach-only behavior, and live-only runtime segment attach-only behavior.
5. Adjust existing sort/multi-pod tests only as needed.
6. Keep `PodDiscovery::inspect` / `AttachOrRestorePod` behavior out of scope; record follow-up if needed.

Critical risks:
- Live attachability and restoreability must stay separate.
- Do not persist runtime segment supplementation to pod-store.
- Sort must happen before truncation.
- Do not duplicate picker-specific merge/sort logic; fix shared `PodList`.
- Rank reachable live rows, not unreachable registry allocations.
- Preview wording must not imply restoreability.
- Multi-Pod dashboard ordering may change; reviewer should check it remains intended.

Validation plan:
- `cargo test -p tui pod_list`
- `cargo test -p tui picker`
- `cargo test -p tui multi_pod`
- `cargo test -p tui`
- `cargo fmt --check`


---

<!-- event: review author: hare at: 2026-05-30T05:00:32Z status: approve -->

## Review: approve

Approve.

The change correctly moves the live-priority rule into shared `PodList` construction, so both the resume picker and multi-Pod dashboard consume the same merged/sorted model. Reachable live Pods now sort ahead of non-live/unreachable/stopped/corrupt rows before truncation, and live pending rows get display-only runtime segment supplementation plus clearer pending preview text without changing pod-store metadata or restore behavior.

Blocker findings: none.

Requirement coverage:
- Reachable live rows sort before stopped/corrupt/unreachable rows before truncation.
- Sorting remains deterministic inside groups: `updated_at` desc, then pod name asc.
- Live pending/runtime-only rows remain attachable/openable but not restorable.
- Runtime segment id supplementation is display/model-only; no pod-store write path is touched.
- Pending preview uses `[live, pending segment]` and does not imply restoreability.
- Shared `PodList` was fixed rather than duplicating picker-specific logic.
- Unreachable registry allocations are not promoted.
- PodDiscovery / AttachOrRestore behavior was not broadened.

Validation reviewed from coder report:
- `cargo test -p tui pod_list` — passed.
- `cargo test -p tui picker` — passed.
- `cargo test -p tui multi_pod` — passed.
- `cargo test -p tui` — passed.
- `cargo fmt --check` — passed.

Final verdict: approve.


---
