---
title: 'Panel startup で Pod status probe を重複実行せず初回一覧表示を高速化する'
state: 'closed'
created_at: '2026-06-19T04:07:17Z'
updated_at: '2026-06-19T04:19:09Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['panel', 'startup-latency', 'pod-status-probe', 'live-path', 'performance']
---

## Background

Live workspace で `yoi panel` を起動すると、first frame は約 50ms で出る一方、実際の Ticket / Pod rows が表示されるまで約 8 秒かかっている。実測 breakdown では `pod_metadata_status_probe.initial`、`companion.presence`、`orchestrator.presence` がそれぞれ約 2.5 秒かかり、同じ Pod metadata / live status scan が初回 dashboard render 前に直列で重複実行されている。

この Ticket では初回一覧表示前の重複 Pod status probe をなくし、live Pod summary の重い session log scan を避け、ユーザー目線の「一覧が表示されるまで」を短縮する。

## Requirements

- `load_multi_pod_snapshot` で初回 `load_pod_list` の結果を Companion / Orchestrator presence 判定に再利用する。
- 初回 render 前に `load_exact_companion_pod_presence` / `load_exact_pod_presence` 相当の追加 full probe を直列実行しない。
- Live status probe は session log 全読みの preview/summary 作成を初回 path で行わない。
  - stored metadata summary を優先して使う。
  - live-only row は minimal live summary でよい。
- Companion / Orchestrator spawn/restore が必要な場合の reload は維持する。
- Existing Panel behavior を壊さない。
  - Companion / Orchestrator live status 表示
  - Queue action
  - Pod rows open/attach
  - E2E dashboard readiness
- Live workspace に近い例外的計測で、rows 表示までの時間が改善していることを確認する。

## Acceptance criteria

- Panel startup source breakdown で `companion.presence` / `orchestrator.presence` が追加 full Pod probe として秒単位で出ない。
- Live workspace 計測で first non-empty rows 表示が従来約 8 秒から明確に短縮する。
- `cargo test -p yoi-e2e --features e2e --test panel` が通る。
- `cargo check -p yoi-e2e -p yoi -p tui --features tui/e2e-test` が通る。
- `cargo fmt --check` / `git diff --check` / `target/debug/yoi ticket doctor` が通る。

## Related work

- `00001KVDETSN6` — Panel startup latency をユーザー目線の dashboard content ready 基準で計測・改善する。
- `00001KVDQH839` — Panel E2E に shell Enter 起動経路の dashboard readiness 計測を追加する。
