---
title: 'Redesign Plugin WASM runtime API and execution model'
state: 'closed'
created_at: '2026-06-24T19:31:28Z'
updated_at: '2026-06-24T19:55:18Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-24T19:39:36Z'
---

## 背景

Yoi の Plugin API はまだ public release されていないにもかかわらず、実装上 `LegacyToolAdapter` / legacy raw `wasm` runtime と新しい `wasm-component` runtime が併存している。外部互換性を守る必要がない段階で legacy layer を残すと、Plugin runtime の責務境界、権限設計、service / ingress 実行モデル、host API の将来設計が曖昧になる。

現状の Plugin 実行モデルは、Pod process 内の Feature として Plugin を有効化し、Tool call / explicit ingress dispatch のタイミングで Wasm component の exported function を同期的に呼ぶ形である。Tool Plugin には使えるが、Discord Gateway / Slack Socket Mode のような長寿命 WebSocket integration、host-managed background service、非同期 ingress delivery、heartbeat / reconnect / backpressure を必要とする用途には足りない。

また、`host_api.websocket` は `open` / `send-text` / `recv(timeout)` / `close` の pull 型 API であり、Plugin service が WebSocket frame arrival を非同期 event として受け取る実行モデルになっていない。Service / Ingress の型と下地はあるが、実運用可能な event-driven Plugin runtime としては未完成である。

この Ticket では、未公開機能としての整理を前提に legacy runtime を削除し、要件を満たす WASM Plugin runtime API / execution model を設計し直す。

## 目的

- 未公開の legacy Plugin runtime / adapter を active design から削除する。
- Plugin runtime を `wasm-component` / Component Model 中心に一本化する。
- Tool Plugin、Service Plugin、Ingress Plugin の責務と実行モデルを明確化する。
- 長寿命 integration に必要な host-managed event runtime を設計する。
- WebSocket を `recv(timeout)` pull primitive ではなく、host-owned connection / event delivery / outbound command model として扱えるようにする。
- Plugin が hidden context injection や ambient authority を持たず、明示 grant / explicit history / domain operation の原則を保つ。

## 要件

### Legacy runtime cleanup

- public release 前であるため、`LegacyToolAdapter` / raw `wasm` runtime / `yoi-plugin-wasm-1` 互換 layer は active runtime から削除する方針にする。
- 新規 Plugin API は `wasm-component` runtime のみを正とする。
- manifest schema から legacy runtime を残す必要があるかを確認し、不要なら reject する。
- compatibility alias / fallback を作らない。
- tests / templates / docs から legacy wording を取り除く。
- migration は不要。既存 private fixture が必要なら fixture を更新する。

### Runtime model

- Plugin は Pod process 内の untrusted extension であり、ambient process / filesystem / network authority を持たない。
- Component call は bounded execution とし、fuel / memory / input / output limits を維持する。
- Tool Plugin は request-response 型の bounded operation として扱う。
- Service Plugin は host-managed lifecycle を持つ。
  - start
  - status
  - stop
  - failure / restart / disable semantics
- Ingress Plugin は host-managed event queue から配送される event を処理する。
- 同一 Plugin instance の concurrency policy を明確にする。
  - v0 は per-plugin serial dispatch でよい。
  - queue / backpressure / timeout / failure handling を定義する。
- `start()` が無限 loop / long polling / recv loop を実行するモデルにしない。

### Service / Ingress event runtime

- Plugin service 用の event queue を導入する方針にする。
- Host が event source を管理し、Plugin の `handle-ingress` 相当へ配送する。
- event delivery は bounded / observable / retry-safe にする。
- event は source / ingress name / payload / created_at / delivery attempt / correlation id を持つ。
- Plugin output は raw side effect ではなく command / result として返す。
  - websocket send
  - request dispatch
  - domain event append
  - diagnostic / status update
- output command の許可は manifest declaration + enablement grant で制御する。

### WebSocket runtime

- 現行 `host_api.websocket.open/send/recv/close` pull API を、長寿命 integration の正 API として扱わない。
- Discord Gateway などを実装できる host-owned WebSocket driver を設計する。
- Host が connection lifecycle を管理する。
  - connect
  - reader task
  - incoming frame queue
  - close / error detection
  - reconnect / resume hook
  - heartbeat support
  - backpressure
- Incoming WS frame は Plugin service の ingress event として配送する。
- Plugin は必要な返信を output command として返し、Host が WebSocket send を実行する。
- binary / text / close / ping / pong の扱いを明確化する。
- guest-supplied handshake headers / auth injection は grant / secret ref / host policy 経由で扱う。

### Host API and authority

- Host API は explicit capability として扱う。
  - request
  - websocket service driver
  - scoped fs
  - future domain operations
- Plugin package declaration と enablement grant の両方が必要であることを維持する。
- Workspace filesystem scope / Pod tool authority を Plugin が自動継承しない。
- raw session / raw socket path / local runtime path を Plugin authority にしない。
- Plugin が prompt/context に隠し injection する API は作らない。
- MCP bridge は Plugin API と混同しない。

### Manifest / PDK / WIT

- `plugin.toml` の runtime / surface / service / ingress / host_api declaration を再整理する。
- `world tool` と `world instance` の責務を見直す。
- Service / Ingress 用の WIT / PDK API を、event-driven runtime に合う形へ更新する。
- WebSocket は low-level recv loop ではなく subscription / event / command model を表現できる API にする。
- PDK が長寿命 loop を推奨しない形にする。
- templates は新 API のみを生成する。

### Observability / status

- Plugin instance / service の status を backend / Pod から確認できるようにする。
- WebSocket connection status、last event、last error、queue depth、restart count、dropped event count などの diagnostic shape を設計する。
- Plugin service failure は fail closed とし、LLM-visible tool surface に曖昧な成功として出さない。

## Non-goals

- Discord Plugin の実装。
- Public plugin registry / install / update / signature policy の完成。
- Remote plugin execution runtime。
- MCP を Plugin runtime に統合すること。
- Plugin に unrestricted filesystem / shell / network authority を与えること。
- Legacy runtime 互換性の維持。

## 受け入れ条件

- `LegacyToolAdapter` / raw `wasm` runtime / `yoi-plugin-wasm-1` 互換 layer が active runtime から削除されている。
- `plugin.toml` / manifest validation は legacy runtime を新規 Plugin runtime として受け付けない。
- Plugin runtime は `wasm-component` / Component Model を正とする実装に一本化されている。
- Tool Plugin は bounded request-response operation として動作し、既存 Tool registration / execution tests が新 runtime model で通る。
- Service Plugin は host-managed lifecycle として `start` / `status` / `stop` / failure state を扱える。
- `start()` が long-running loop / polling loop / WebSocket recv loop を担わない実行モデルになっている。
- Ingress event は host-managed queue / dispatcher から Plugin instance に配送できる。
- Ingress dispatch には bounded queue、serial dispatch、backpressure、timeout、failure handling の実装または型境界がある。
- WebSocket は host-owned connection driver として扱われ、incoming frame を ingress event に変換できる。
- Plugin は WebSocket 返信・request dispatch・diagnostic update などの side effect を output command として返し、Host が grant 検査後に実行する。
- 現行 `open` / `send-text` / `recv(timeout)` / `close` pull API は、長寿命 integration の正 API から外れている、または互換目的で残す場合も public/recommended API として露出していない。
- Manifest / WIT / PDK / templates が新しい Component Model runtime、Service lifecycle、Ingress event、WebSocket command model に合わせて更新されている。
- Plugin package declaration と enablement grant の両方が必要な authority 境界が維持されている。
- Plugin は workspace filesystem scope、Pod tool authority、raw session、raw socket path、local runtime path を自動継承しない。
- Plugin instance / service / WebSocket connection の status と diagnostics を Pod/backend から確認できる。
- 関連 docs と tests から legacy runtime 前提の記述・fixture が削除または更新されている。
- `cargo test -p pod`、`cargo test -p yoi-plugin-pdk`、`cargo check -p yoi`、`git diff --check`、`nix build .#yoi --no-link` が通る。
