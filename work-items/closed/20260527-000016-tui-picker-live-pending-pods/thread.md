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

<!-- event: close author: hare at: 2026-05-30T05:00:56Z status: closed -->

## Closed

---
id: 20260527-000016-tui-picker-live-pending-pods
slug: tui-picker-live-pending-pods
title: TUI picker: live pending Pod の表示優先と状態補完
status: closed
kind: task
priority: P2
labels: [migrated]
created_at: 2026-05-27T00:00:16Z
updated_at: 2026-05-30T05:00:56Z
assignee: null
legacy_ticket: tickets/tui-picker-live-pending-pods.md
---

## Migration reference

- legacy_ticket: tickets/tui-picker-live-pending-pods.md
- migrated_from: TODO.md / tickets directory migration on 2026-05-27

# TUI picker: live pending Pod の表示優先と状態補完

## 背景

`tui -r` の Pod picker は session store の name-keyed Pod metadata と runtime registry の live allocation を合わせて表示している。しかし、spawned child Pod がまだ最初の user turn / SegmentStart を materialize していない場合、Pod metadata は pending segment のままになり、session log も存在しない。

実例として、`impl-llm-worker-stream-continuation` は live socket と runtime registry 上の segment_id を持っていたが、metadata は以下のように `session_id` のみだった。

```json
{
  "pod_name": "impl-llm-worker-stream-continuation",
  "active": {
    "session_id": "019e5bc6-c3f3-7193-98a1-d64c635f86a1"
  }
}
```

一方で runtime 側には segment_id が存在する。

```json
{
  "pod_name": "impl-llm-worker-stream-continuation",
  "segment_id": "019e5bc6-c3f3-7193-98a1-d6559bdc9cd6",
  "state": "idle"
}
```

この状態の Pod は attach 可能だが、session log がないため `updated_at = 0` になり、picker の `updated_at desc` sort と `MAX_ROWS = 10` truncate によって一覧から漏れやすい。

## 方針

Live socket が reachable な Pod は、session log / metadata active segment が未確定でも attach 可能な対象として picker に表示する。restore 可能性と attach 可能性を分け、live pending Pod は restore 不能でも live attach 対象として扱う。

## 要件

- `tui -r` picker は reachable live Pod を stopped Pod より優先して表示する。
  - `updated_at = 0` でも live row が `MAX_ROWS` truncate で落ちない。
  - sort key は少なくとも live first, updated_at desc, pod_name になる。
- Live Pod の metadata が pending segment の場合でも picker row に表示する。
  - preview は `[live, pending segment]` など、人間が状態を理解できる文言にする。
  - debug id 表示では runtime registry の segment_id を可能なら表示する。
- Runtime registry / live status に segment_id があり、metadata に segment_id が無い場合、表示上は runtime segment_id を補完できるようにする。
  - ただし session log が存在しない限り restore 可能とは扱わない。
  - attach は live socket に対して行う。
- Existing stopped / corrupt Pod metadata rows の表示を壊さない。
- `ListVisiblePods` / discovery 側にも同様の pending live 表示不整合がある場合、必要なら後続 ticket に切り出す。
  - この ticket の主対象は `tui -r` picker。

## 完了条件

- live pending Pod が `tui -r` に表示される。
- live pending Pod を選択すると live socket に attach する。
- live pending Pod が多数の stopped Pod によって `MAX_ROWS` truncate から漏れない。
- picker の sort / row build の unit test が追加または更新されている。
- `cargo fmt --check` と `cargo test -p tui picker` あるいは関連 TUI test が通る。

## 範囲外

- pending Pod metadata を runtime segment_id で永続的に書き換えること。
- session log が無い Pod を restore 可能にすること。
- spawned child Pod の first turn / SegmentStart materialization 方針の変更。
- 汎用 spawned Pod panel UI。


---
