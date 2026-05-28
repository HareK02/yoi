---
id: 20260527-000012-spawnpod-initial-run-confirmation
slug: spawnpod-initial-run-confirmation
title: SpawnPod: initial Run delivery confirmation
status: open
kind: task
priority: P2
labels: [migrated]
created_at: 2026-05-27T00:00:12Z
updated_at: 2026-05-28T13:05:40Z
assignee: null
legacy_ticket: tickets/spawnpod-initial-run-confirmation.md
---

## Migration reference

- legacy_ticket: tickets/spawnpod-initial-run-confirmation.md
- migrated_from: TODO.md / tickets directory migration on 2026-05-27

# SpawnPod: initial Run delivery confirmation

## 背景

`SpawnPod` は child Pod を起動し、初回 task を `Method::Run` として送る。しかし、実例として `impl-llm-worker-stream-continuation` を再作成した際、runtime registry / socket / process は生きている一方で、初回 task の session log が materialize されず、Pod は `idle` のままだった。

確認された状態:

- `<runtime-dir>/pods.json` に live allocation がある
- `<runtime-dir>/<pod>/status.json` は `state: "idle"` と runtime `segment_id` を持つ
- `<insomnia-sessions>/pods/<pod>/metadata.json` は pending segment のまま
- 対応する session / segment `.jsonl` が存在しない
- `ReadPodOutput` は no new assistant text

`SpawnPod` の送信側は `send_run` で `Method::Run` を write してすぐ切断し、`TurnStart` 等の ack を待っていない。一方 server 側は接続直後に `Snapshot` を書いてから method を読むため、client がすぐ close すると server が snapshot write で失敗し、method を読む前に connection handler が終了する race があり得る。

この場合 `SpawnPod` は成功を返すが、child Pod は初回 task を実行していない。

同種の問題は child Pod の通知経路でも既に踏んでおり、送信側が write 後にすぐ切断せず、receiver 側の acknowledgement / observable event を待つ形にして解消している。`SpawnPod` の初回 task delivery も同じ性質の race と見なす。

追加確認として、Pod socket server は接続直後に replayed `Alert` と connect-time `Snapshot` を送ってから client `Method` を読む。したがって one-shot / send-only client は初期 event を消化してから Method を送る必要がある。

- `send_run_and_confirm` は `Method::Run` を送った後に event を読む実装になっており、Snapshot が大きい場合や Run payload が大きい場合に双方向で詰まる余地がある。
- `connect_and_send` / `fetch_history` は既に Snapshot まで drain / read しており、この系統の問題は対策済み。
- `probe_socket` は最初の event だけを見て `Snapshot` でなければ status を取らないため、replayed `Alert` が先に来る live Pod で reachable だが status unknown になる可能性がある。
- `PodClient::connect` は background reader を起動するため、通常の TUI attach / interactive client では初期 Snapshot を詰まらせにくい。

## 方針

`SpawnPod` は child process / socket の起動だけでなく、初回 task が controller に受理され、少なくとも `UserMessage` または `TurnStart` が観測できるまで確認してから成功を返す。

既存の `SendToPod` / `SpawnPod` が使う run delivery confirmation ロジックを、接続直後の `Alert` / `Snapshot` drain を含む形へ共通化・安全化する。

## 要件

- `SpawnPod` の初回 task 送信は fire-and-forget にしない。
  - `Method::Run` 送信後、`UserMessage` / `TurnStart` / `InvokeStart` など、run が受理されたことを示す event を待つ。
  - timeout 時は `SpawnPod` を失敗扱いにする。
- 初回 task delivery に失敗した場合、process / registry / delegated scope の扱いを明確にする。
  - cleanup するか、attach 可能な idle Pod として残すかを実装で決める。
  - 少なくとも成功扱いで返さない。
- Server が connection 開始時に `Alert` / `Snapshot` を書く設計と競合しない。
  - client 側が `Alert` / `Snapshot` を読みながら `Method::Run` ack を待つ形にする。
  - `send_run_and_confirm` は connect-time `Snapshot` を消化してから `Method::Run` を送る。
- live Pod status probe は replayed `Alert` によって status 取得を落とさない。
  - `probe_socket` は first event だけで判断せず、`Snapshot` まで初期 event を読む。
- `SpawnPod` 成功後は、child Pod の metadata が pending でも、初回 run が開始済みであることを確認できる。
  - session log materialization のタイミングそのものは別設計でもよい。
- `SendToPod` と `SpawnPod` の run delivery confirmation ロジックを可能な範囲で共通化する。

## 完了条件

- `SpawnPod` が初回 task の受理確認を待つ。
- 初回 task が実行されない race を再現する test または regression test がある。
- connect-time `Alert` / `Snapshot` がある状態でも `send_run_and_confirm` が詰まらず、受理 event を観測する regression test がある。
- `probe_socket` が replayed `Alert` の後の `Snapshot` から status を取得できる regression test がある。
- `SpawnPod` が success を返した後、child Pod が idle pending のまま task 未実行になる状態が起きない。
- delivery timeout / failure 時の error message が人間に分かる。
- `cargo fmt --check` と関連 crate の test が通る。

## 範囲外

- `tui -r` picker に live pending Pod を表示する修正。
- session log の SegmentStart materialization 方針変更。
- spawned child Pod panel UI。
