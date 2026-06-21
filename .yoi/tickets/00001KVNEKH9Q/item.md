---
title: 'Workspace backend: expose local host and worker list'
state: 'ready'
created_at: '2026-06-21T16:00:49Z'
updated_at: '2026-06-21T16:01:33Z'
assignee: null
---

## 背景

Workspace web control plane は read-only API / static SPA / SQLite skeleton まで立ち上がっているが、実行環境の表示はまだ `runner` placeholder のままで、実際に稼働中の local Pod / session を表示できない。

当面の設計語は `Host` / `Worker` とする。

- Host: 実行環境。最初の実装では workspace backend が動いている local machine を表す。
- Worker: Host 上で動いている agent/runtime session。現行 local runtime では Pod に対応する。
- Pod: 現行 local implementation detail。API では `implementation.kind = "local_pod"` のように表現する。

初期実装では、現行 TUI dashboard / panel が local state を見るのと同じ感覚で、backend process が動いている host 上の local Pod metadata / socket/session state を read-only に検出し、Workspace API から host / worker 一覧を返す。これは hosted/multi-host runner protocol の完成を待たずに、Web UI で現在の local execution state を見られるようにするための bridge である。

## 要件

- Workspace backend API に host / worker list endpoint を追加する。
  - 例: `GET /api/hosts`
  - 例: `GET /api/hosts/{host_id}/workers` または `GET /api/workers`
  - 既存 `runner` placeholder は `host` naming に寄せる。破壊的変更でよい。
- 初期 Host は backend process が動いている local machine として検出する。
  - stable-ish `host_id`、label、kind/status、last_seen/observed_at、capability summary を返す。
  - capability は詳細実装しすぎず、local pod inspection available / workspace_root / os など bounded summary でよい。
- Worker list は現行 local Pod state から read-only に生成する。
  - `~/.yoi/pods/<pod_name>/metadata.json` などの現在の Pod metadata authority を使う。
  - active socket/session/runtime hints は利用してよいが、metadata と session logs を壊さない。
  - local Pod は Worker の implementation detail として返す。
- API response は Web/control-plane domain を優先する。
  - `worker_id`
  - `host_id`
  - `label` / `pod_name`
  - `role` or `profile` if known
  - `workspace_root`
  - `state` / `status` if known
  - `implementation: { kind: "local_pod", pod_name: ... }`
  - bounded diagnostics
- Secrets / prompt contents / session transcript contents / hidden metadata は返さない。
- backend が local Pod data dir を読めない場合は fail closed ではなく、empty list + diagnostic または host capability unavailable として返す。
- Web UI skeleton で host / worker list を表示できる範囲までつなぐ。
- これは remote/self-hosted/cloud runner protocol の実装ではない。将来の Host/Worker protocol に置き換えられる local bridge として実装する。

## Non-goals

- Worker start/stop/attach/notify 操作。
- remote runner / cloud runner registration protocol。
- Run と Worker の完全な紐付け。
- Pod metadata schema migration。
- session transcript / model context / tool result content の表示。
- Host resource scheduling / quota / billing。

## 受け入れ条件

- `GET /api/hosts` が backend-local Host を bounded JSON で返す。
- `GET /api/workers` または `GET /api/hosts/{host_id}/workers` が local Pod 由来の Worker 一覧を bounded JSON で返す。
- response naming は `host` / `worker` を使い、`pod` は implementation detail に閉じる。
- 既存 `/api/runners` placeholder は削除または `hosts` へ移行され、frontend / tests も追従している。
- Web UI に Host / Worker 一覧が表示される。
- local Pod metadata が存在しない環境でも API は安全に empty/diagnostic response を返す。
- secrets、session transcript、prompt contents は response に含まれない。
- Focused tests cover host list, worker list, missing local pod data dir, and response bounds/redaction.
- Validation before completion includes `cargo fmt --check`, `cargo test -p yoi-workspace-server`, frontend check/build, `cargo check`, `git diff --check`, `yoi ticket doctor`, and `nix build .#yoi --no-link`.
