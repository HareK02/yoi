---
title: 'Workspace Worker Consoleを任意Worker attach前提で再設計する'
state: 'done'
created_at: '2026-06-26T17:42:10Z'
updated_at: '2026-06-26T18:22:51Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-26T17:45:40Z'
---

## 背景

現在の Web Console は Companion MVP の延長として作られており、Workspace の正規 Console としては UX / 情報設計 / theme 統一 / attach model が不足している。Companion 専用 API と簡易チャット風 UI に寄っており、任意 Worker を `runtime_id + worker_id` で開く導線、TUI Console 相当の protocol event 表示、Workspace 全体と統一された visual design が弱い。

Runtime / Backend / Client の observation proxy と、Backend Runtime Worker API は既に任意 Worker attach の基盤になり得る。次は Companion 固定の Console を正規経路として延命せず、Workspace top の Worker list から任意 Worker Console を開ける UI として再設計する。

## 目的

- Workspace top の Worker list から任意 Worker の Console を開ける。
- Sidebar に standalone `Console` category / navigation entry を置かない。
- Console target は `runtime_id + worker_id` を authority とする。
- Companion は特別な Console 実装ではなく、通常 Worker として表示・attach できる。
- Companion は Backend 起動時の自動起動 Worker として Worker list に現れ、必要なら role badge / pinned display で識別する。
- 現行 Web Console UX は置き換え前提とし、Workspace theme と情報設計に合わせて新規設計する。
- TUI Console の見た目ではなく、`protocol::Event` を transcript / live UI state に落とす意味論を Web に移植する。

## 要件

### Navigation / route model

- 正規導線は Workspace top の Worker list から Console を開く形にする。
- Sidebar に standalone `Console` category / navigation entry を作らない。
- 既存の Companion 固定 `/console` route がある場合は正規 route として残さず、削除または Worker Console route へ置き換える。
- 旧 `/console` からの redirect / fallback は残さない。古い導線の互換維持より、正規 navigation と route authority を明確にする。
- Companion 専用の quick action / drawer / global composer が必要になった場合は、この Ticket ではなく後続の UX 設計として扱う。

### Attach model / routing

- Workspace top に Worker list を表示し、Worker row から Console を開ける。
- Console route / state は `runtime_id + worker_id` を target identity として持つ。
- Browser は Runtime endpoint / credential / socket path / session path を扱わない。
- Console は Backend Worker API を使う。
  - Worker detail / capabilities。
  - bounded transcript / Snapshot 相当。
  - Backend client-facing observation WS。
  - input API。
- Companion Worker は role / badge / pinned row などで識別してよいが、Console 実装は通常 Worker と共有する。
- Companion 専用の route / sidebar entry / Console implementation は作らない。

### UX redesign

- 現行 Companion Console UI を正規 Console として継続しない。
- Console の主表示は conversation / run stream と composer に集中する。
- runtime id、worker id、transport、capability、diagnostic などの metadata は常時幅を取らない。
  - 必要な情報は compact header、details drawer、inspector、collapsible diagnostics へ退避する。
- Workspace 全体の theme / spacing / typography / color token と統一する。
- narrow width / wide width の両方で transcript が読みやすい layout にする。
- busy / unavailable / rejected / reconnecting / stale cursor などの状態は主導線を邪魔しない形で表示する。

### Protocol rendering model

- Web Console は `protocol::Event` を UI state に変換して表示する。
- TUI Console から移植する対象は rendering semantics と event handling であり、TUI の見た目や keybinding ではない。
- 少なくとも以下を表示対象にする。
  - user message。
  - assistant text delta / done。
  - thinking block。
  - tool call lifecycle。
  - tool result。
  - status / run start / run end / error。
  - usage。
  - snapshot / transcript reconstruction。
  - in-flight output。
- 既に protocol / Backend WS で届く情報を、簡易 chat message へ潰しすぎない。
- 未実装の Runtime/permission/advanced control のための UI は作らない。

### Input / observation

- Composer input は Backend Worker input API へ送る。
- Live update は Backend client-facing observation WS から受ける。
- Initial render は bounded transcript / Snapshot 相当から構築する。
- WS reconnect / duplicate event / cursor resume は最低限破綻しない動作にするか、typed diagnostic として表示する。
- Worker が event streaming 非対応の場合は、polling / manual refresh / transcript-only などの degrade path を明示する。

## Non-goals

- Full permission / auth / redaction policy implementation。
- Runtime endpoint / credential を Browser に渡す direct Runtime attach。
- TUI の見た目そのものの Web 再現。
- 未実装 command / advanced control UI。
- Multi-worker split view。
- Full log viewer / raw provider trace viewer。
- Companion 専用 Console の新規拡張。
- Companion 専用 route / sidebar entry / quick action の設計。
- 旧 `/console` route の redirect / fallback 互換維持。

## 受け入れ条件

- Workspace top の Worker list から任意 Worker Console を開ける。
- Sidebar に standalone `Console` category / navigation entry が残っていない。
- Console target が `runtime_id + worker_id` で表現される。
- Companion Worker を通常 Worker として Console で開ける。
- Companion 専用 route / sidebar entry / Console implementation が正規導線に残っていない。
- 旧 `/console` route の redirect / fallback 互換が残っていない。
- 現行 Companion Console 固定 UI が正規 Console 導線から置き換えられている。
- Console UI が Workspace theme と統一され、metadata / diagnostics が主表示の幅を過剰に占有しない。
- Backend transcript / input / observation WS を使って Console が動作する。
- `protocol::Event` 由来の user / assistant / thinking / tool / status / error / usage が表示される。
- Streaming 非対応 Worker の degrade path が明示されている。
- Focused Web UI tests or component tests が追加されている。
- `cd web/workspace && deno task check` が通る。
- `cd web/workspace && deno task build` が通る。
- `cargo check -p yoi` が通る。
- `git diff --check` が通る。
- `nix build .#yoi --no-link` が通る。
