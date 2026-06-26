---
title: 'TUIをRuntime API/WebSocket接続へ移行する'
state: 'done'
created_at: '2026-06-25T19:32:38Z'
updated_at: '2026-06-26T08:54:39Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-25T20:34:42Z'
---

## 背景

現在の TUI は旧 Pod / Worker process の Unix socket に直接 attach し、connect-time `Snapshot`、`Append`、`Status` などの protocol event を読んで Console 表示を構築している。一方、Worker/Runtime 移行後は、Worker は Runtime 内の実行単位であり、Runtime は REST command API と WebSocket observation stream を公開する。Backend は RuntimeRegistry を通じて embedded / remote / compatibility runtime を束ね、Browser や将来の TUI は Runtime endpoint / socket path / session path を直接 authority として扱わない方向にする。

この Ticket では、Runtime API / WebSocket observation がある程度整った後、TUI の接続 backend を旧 Pod socket 直結から Runtime API / Backend proxy / WebSocket stream に移行する。既存 TUI の Console rendering / composer / status UI は活かしつつ、入力送信と出力購読の transport を Runtime model に合わせる。

## 目的

- TUI が旧 Pod socket path を直接 authority としない接続経路へ移行する。
- TUI が `runtime_id + worker_id` を対象 identity として扱える。
- TUI の input は Runtime command API 経由で送る。
- TUI の output / status / transcript update は Runtime WebSocket observation stream 由来の event projection で受ける。
- 旧 Pod attach path は移行期間の compatibility / debug path として扱い、正規 path ではなくす。

## 要件

### Connection model

- TUI は Runtime / Worker target を `runtime_id + worker_id` で指定できる。
- TUI は Backend RuntimeRegistry / Runtime routing / Runtime endpoint credential 管理を内部実装しない。
- TUI は Browser と同じく、原則として Backend API に接続する client として振る舞う。
- Backend が RuntimeRegistry / policy / visibility / audit / runtime routing を担う。
  - TUI -> Backend -> embedded Runtime direct call。
  - TUI -> Backend -> remote Runtime REST/WS client。
  - TUI -> Backend -> local compatibility adapter。
- TUI が remote Runtime endpoint / credential / raw socket path / raw session path を直接受け取らない。
- Local-only debug mode として direct Runtime endpoint / legacy socket attach を残す場合は、明示的な debug/compat mode とする。

### Input path

- Composer input は Runtime command API へ送る。
- v0 input は user message を最小単位とする。
- Busy / rejected / worker not found / runtime unavailable / provider error を TUI の status/error 表示に map する。
- 既存 `UserMessage` acceptance evidence 相当を、Runtime command accepted / run started event に置き換える。

### Output / observation path

- TUI は Runtime WebSocket event stream または Backend-proxied observation stream を購読する。
- Runtime event を TUI Console model へ変換する adapter を実装する。
- 少なくとも以下を扱う。
  - worker snapshot / initial transcript projection。
  - user input accepted / transcript append。
  - assistant text delta / final。
  - reasoning / thinking visibility policy に基づく event。
  - tool call lifecycle。
  - run started / completed / errored。
  - usage。
  - status / diagnostics。
- Event cursor / reconnect / replay / duplicate event handling を TUI 側で扱う。
- Raw provider trace / raw full session log は TUI event path の authority にしない。

### Existing TUI modelとの関係

- 既存 TUI の rendering component、composer、scrollback、status/actionbar 表示は可能な限り維持する。
- 旧 Pod protocol event から直接 UI block を作る path と、新 Runtime event から UI block を作る path を分ける。
- 旧 `Event::Snapshot` / `Event::Append` / `Event::Status` と Runtime event の対応を明示する。
- 連続 Thinking block grouping、in-flight snapshot、tool call rendering など既存 UI semantics が regress しないようにする。

### Compatibility / migration

- 移行途中で HEAD の `cargo run --bin yoi -- panel` / TUI が旧 Pod list 取得に失敗する可能性は許容するが、最終的には RuntimeRegistry / Worker projection を正とする。
- 旧 Pod socket attach は完全互換を目標にしない。
- ただし dogfooding に必要な範囲で、legacy local Worker compatibility adapter 経由の attach/read はできるようにする。
- Compatibility path を残す場合も、新規設計の正規 API と混同しない naming / CLI flag / diagnostics にする。

## Non-goals

- `worker-runtime` core crate の作成。
- Runtime REST command server の作成。
- Runtime WebSocket event stream server の作成。
- Backend RuntimeRegistry の embedded / remote 接続実装。
- Web Console UI の完成。
- 旧 Pod socket protocol の完全互換維持。
- Raw session storage migration。
- Full multi-user auth / permission model。

## 受け入れ条件

- TUI が `runtime_id + worker_id` を対象として接続できる。
- TUI が Backend API client として実装され、Backend RuntimeRegistry / Runtime routing / Runtime endpoint credential 管理を内部実装していない。
- TUI input が Backend/Runtime command API 経由で Worker に届く。
- TUI が Runtime WebSocket observation stream または Backend-proxied stream から Worker output/status events を受け取る。
- Runtime events が既存 TUI Console model に変換され、user message / assistant output / status / error が表示される。
- Initial transcript / snapshot 相当の表示ができる。
- Reconnect / cursor resume / duplicate event の基本動作が実装されている、または明確な typed diagnostic になる。
- Browser/remote Runtime credential/socket/session path を TUI が authority として扱わない。
- 旧 Pod socket attach path は正規 path ではなく compatibility/debug path として分離されている。
- Focused TUI tests or adapter tests が追加されている。
- `cargo test -p tui` または該当 TUI crate test が通る。
- `cargo check -p yoi` が通る。
- `git diff --check` が通る。
- `nix build .#yoi --no-link` が通る。
