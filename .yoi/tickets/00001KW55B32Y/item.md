---
title: 'worker-runtimeにWorker実行Backend境界を追加する'
state: 'inprogress'
created_at: '2026-06-27T18:26:46Z'
updated_at: '2026-06-27T19:41:12Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-27T19:06:28Z'
---

## 背景

現在の `worker-runtime::Runtime` は Runtime / Worker identity、metadata、transcript projection、observation stream を持つが、既存 `worker` crate の実 LLM 実行に接続されていない。その結果、Backend / Web Console から見ると Worker として存在するが input を処理できない Worker が発生し得る。

これは Runtime/Worker model として不十分である。Worker として公開されるものは transcript / observation を読める必要があり、input を受け付ける Worker は実行 backend に接続されている必要がある。`can_stream_events` / `can_read_bounded_transcript` のような「中身を読めない Worker を表示するための capability」は正規 model に置かない。

この Ticket では、`worker-runtime` に実行 backend を差し込むための型境界と lifecycle contract を追加する。ここでは既存 `worker` crate への具体接続は後続 Ticket に残し、Runtime が「実行可能 Worker」と「実行 backend 未接続の placeholder」を混同しないための土台を作る。

## 目的

- Runtime が Worker execution backend を明示的に持てる。
- input を受け付ける Worker は execution backend に接続されていることを型・状態・テストで保証する。
- Worker transcript / observation は Worker の正規 contract として扱い、capability 扱いしない。
- fake / providerless assistant response を Runtime が生成しない。
- 後続で `worker` crate 実行 adapter を接続できる境界を作る。

## 要件

### Execution backend boundary

- `worker-runtime` に Worker execution backend の trait / handle / enum を追加する。
- execution backend は少なくとも以下を扱える形にする。
  - Worker spawn / initialize 時の execution handle 作成。
  - user input の実行 backend への dispatch。
  - run state / busy / rejected / errored の typed 結果。
  - stop / cancel が未実装の場合の typed rejection。
  - protocol event を Runtime observation bus へ publish するための hook。
- execution backend 未接続の Worker に対して user input を accepted にしない。
- Runtime 自身が providerless text / canned assistant message を生成しない。

### Worker model invariant

- Worker として公開されるものは transcript projection / observation stream を読める。
- `can_stream_events` / `can_read_bounded_transcript` を public Worker capability として復活させない。
- input acceptance は capability 表示でごまかさず、execution backend 接続状態と runtime state に基づく typed operation result とする。
- execution backend がない Worker を UI の通常 input-capable Worker として見せない。

### API / compatibility

- RuntimeRegistry / Backend API が raw execution backend handle、socket path、credential、session path を Browser に露出しない。
- 既存 remote Runtime WS / Backend proxy model と矛盾しない。
- placeholder / not-connected 状態が必要な場合は diagnostic / state として表現し、fake assistant response で埋めない。

## Non-goals

- 既存 `worker` crate の実行 loop への具体接続。
- Workspace Companion を実 LLM 実行で動かすこと。
- Web Console UX の再設計。
- Full permission / auth / redaction policy。
- remote Runtime process の lifecycle 実装。

## 受け入れ条件

- `worker-runtime` に execution backend 境界がある。
- execution backend 未接続 Worker への input が accepted にならない。
- Runtime が fake / providerless assistant response を生成しない。
- Worker transcript / observation が正規 contract として扱われ、public `can_stream_events` / `can_read_bounded_transcript` capability が存在しない。
- RuntimeRegistry / Backend projection に raw backend handle / path / credential が漏れない。
- Focused tests が、backend 未接続 input rejection、backend 接続 Worker の input dispatch 境界、observation publish hook を確認する。
- `cargo test -p worker-runtime --features ws-server` が通る。
- `cargo test -p yoi-workspace-server` が通る。
- `cargo check -p yoi` が通る。
- `git diff --check` が通る。
- `nix build .#yoi --no-link` が通る。
