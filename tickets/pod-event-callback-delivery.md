# PodEvent callback delivery を確実化する

## 背景

Spawned Pod の完了通知は、child が parent の Unix socket へ `Method::PodEvent(PodEvent::TurnEnded)` を送ることで成立する。parent は受信した `PodEvent` を `NotifyBuffer` に積み、idle なら `RunForNotification(PodEvent)` を auto-kick して LLM に通知する。

しかし実運用で child が完了しているのに parent 側に完了通知が来ない事象が複数回発生した。`ReadPodOutput` の singular `LogEntry` 追従漏れは別途解消済みだが、完了通知自体の delivery path にも構造的な不安がある。

現行の callback 送信は `ipc::event::fire_and_forget` → `send_pod_event` → `connect_and_send` で、`connect_and_send` は socket に接続して Method を 1 行 write し、応答を読まずに close する。一方、受信側 `SocketServer::handle_connection` は新規接続に対して、Method を読む前に alert snapshot と `Event::Snapshot` を先に write する。このため、send-only client が read half を持たない / 読まない状態で接続した場合、受信側が snapshot write で失敗または詰まり、肝心の Method を読む前に接続処理が終わる可能性がある。

この経路は `PodEvent` だけでなく、`StopPod` が child に送る `Method::Shutdown` など `connect_and_send` を使う fire-and-forget Method 全般に影響しうる。

## 現行経路

### child completion

```text
child controller drive_turn
  -> Worker turn が Finished
  -> parent_originated なら PodEvent::TurnEnded を fire_and_forget
  -> child は実行継続、socket は alive
```

### transport

```text
fire_and_forget(parent_socket, PodEvent::TurnEnded)
  -> tokio::spawn
  -> send_pod_event
  -> connect_and_send(parent_socket, Method::PodEvent(...))
  -> Method を write して close（応答は読まない）
```

### parent receive

```text
SocketServer::handle_connection
  -> subscribe snapshot
  -> Event::Snapshot を client に write
  -> その後 select! で Method を read
  -> controller Method::PodEvent
  -> apply_event_side_effects
  -> NotifyBuffer.push_pod_event
  -> parent が Idle なら RunForNotification(PodEvent)
  -> interceptor が SystemItem::PodEvent として history に commit
  -> LLM request に通知が乗る
```

## 要件

- fire-and-forget Method 送信が、受信側の initial `Event::Snapshot` write に阻害されないようにする。
  - `Method::PodEvent` / `Method::Shutdown` など send-only 用 helper が確実に controller へ Method を届けること。
  - 解決策は実装時に選ぶが、少なくとも以下のどちらかを満たす。
    - send-only client 側が read half を保持し、必要なら snapshot を drain しつつ Method write を完了する。
    - server 側で method-only / callback 接続が initial snapshot を要求しない protocol にする。
- `PodEvent::TurnEnded` の delivery failure を観測可能にする。
  - fire-and-forget の warn log だけに閉じず、テスト可能な失敗経路を持つ。
  - 必要なら callback sender を fire-and-forget と awaited variant に分ける。
- parent 受信後の処理が期待通り行われることを test する。
  - `Method::PodEvent(TurnEnded)` 受信で `NotifyBuffer` に typed event が積まれる。
  - parent が Idle の場合、`RunForNotification(PodEvent)` が staged / 実行される。
  - parent が Running の場合、in-flight turn の次回 `pre_llm_request` で drain される。
- `connect_and_send` を使う既存経路への影響を確認する。
  - `StopPod` の `Method::Shutdown`
  - child の `PodEvent::ShutDown`
  - `ScopeSubDelegated` re-emit
- 大きな snapshot を持つ parent に対しても callback Method が届くことを regression test にする。

## 完了条件

- child completion 相当の `PodEvent::TurnEnded` が parent controller に確実に届く test を追加する。
- initial snapshot が大きい parent socket に対しても `connect_and_send` / callback delivery が詰まらない test を追加する。
- `StopPod` が child に `Method::Shutdown` を届けられることを test する。
- 通知が parent history に `SystemItem::PodEvent` として commit されることを test する。
- `cargo fmt --check`
- `cargo check --workspace`
- `cargo test -p pod`

## 範囲外

- PodEvent の永続 retry queue
- delivery ack を LLM-visible tool result として返す UX 変更
- 過去 Pod 探索 / restore tool
- delegated scope reclaim の実装

## 関連

- `crates/pod/src/controller.rs`: `drive_turn`, `Method::PodEvent` handling
- `crates/pod/src/ipc/event.rs`: `fire_and_forget`, `send_pod_event`, side effects
- `crates/pod/src/spawn/comm_tools.rs`: `connect_and_send`
- `crates/pod/src/ipc/server.rs`: connection start時の `Event::Snapshot` 送信
- `crates/pod/src/ipc/notify_buffer.rs`: parent LLM context への通知注入
