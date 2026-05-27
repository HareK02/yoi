---
id: 20260527-000016-tui-picker-live-pending-pods
slug: tui-picker-live-pending-pods
title: TUI picker: live pending Pod の表示優先と状態補完
status: open
kind: task
priority: P2
labels: [migrated]
created_at: 2026-05-27T00:00:16Z
updated_at: 2026-05-27T00:00:16Z
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
