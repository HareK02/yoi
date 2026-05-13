# Pod: 状態と socket 配信を session log 正本に統合する

## 背景

Pod の状態は現在 3 つの形で同居している:

1. **session log** (`crates/session-store/src/session_log.rs:LogEntry`) — append-only の typed 正本。 `UserInput { segments }` / `AssistantItems` / `ToolResults` / `HookInjectedItems` / `TurnEnd` / `SessionStart` で会話全体を表現できる。
2. **worker.history** (`Vec<Item>`) — LLM に投げるために flatten / 加工された派生 view。 user_message は `Vec<Segment>` を flatten した String になっている。
3. **PodSharedState** の `history` + `user_segments` — worker.history を ipc 層に渡すための中継ミラー。 typed segments は parallel 配列で別途保持。

`Method::GetHistory` (`crates/pod/src/ipc/server.rs:132-182`) は (3) の中継から組み立てており、 平坦化された user_message に segments を後付けする overlay + skip-align ロジックが必要になっている。 broadcast (`event_tx`) はライブイベントだけを流し、 接続時 snapshot は別経路 + 別 Event 型 (`Event::History`) で返るため、 再アタッチ時に snapshot ↔ live が連続しない (`tickets/pod-socket-state-view.md` の問題)。

派生方向が逆転している: 正本は session log なのに (3) は (2) を経由した二次派生になっており、 (1) が既に持ってる typed 情報を flatten/復元で往復する歪んだ構造を生んでいる。 また `Method::GetHistory` が RPC 形を取っていることで、 同じ socket writer に「broadcast forwarder」 と「query handler」 の 2 経路が同居している。

## 方針

- session log を Pod 状態の単一正本として位置付け、 worker.history は LLM context 投影用の内部 view に格下げる。 ipc 経路には worker.history が現れない。
- 接続クライアントへの配信を 「session log の prefix (snapshot) + suffix (live)」 という同型ストリームに統合する。 query/reply 型の `Method::GetHistory` を廃止し、 接続自体が暗黙の subscribe-with-replay として動作する。
- ストリーミング系イベント (`TextDelta` / `ToolCallStart` / `ToolCallArgsDelta` 等) は progressive 描画用の best-effort hint に役割を限定する。 late attach で過去 delta が失われるのは仕様。 確定情報は session log entry の broadcast で別途到達する。
- entry commit の hook 点は worker 側の確定 callback に置く。 現状 `Pod::run()` 末尾で `persist_turn` が `history_before..` を一括 flush しているが (`crates/pod/src/pod.rs:1491-1502`)、 これを 「worker が assistant block / tool call / tool result を確定した瞬間に append_entry を呼ぶ」 形へ移す。 `wire_event_bridges_on_worker` で worker → event_tx を bridge しているのと同じ箇所に append_entry hook を追加する想定で、 worker 内部構造への介入は確定 callback 受け口の追加に限定する。
- atomicity の中身は「disk write が成功した entry のみ broadcast に乗る」 順序保証。 alerter は memory-only なので buffer lock + `broadcast::send` で完結するが、 session log は disk I/O が混じるため対称ではない。 `append_entry` は (1) disk write → (2) in-memory mirror 更新 → (3) `Event::Entry` broadcast の順で、 (1) 失敗時は (2)(3) を行わず error を上に返す。 (2)(3) は同一の subscribe lock 下で行い、 `subscribe_with_snapshot` が見る mirror と receiver 側のイベント列に重複・欠落・順序逆転が出ないようにする。
- `Event::SystemMessage` 廃止に伴い、 system_message を LogEntry に焼く責務は controller 側の `Event::SystemMessage` 送信点 (`crates/pod/src/controller.rs:372`) を `LogEntry::HookInjectedItems` の append_entry 呼び出しに置き換える形で取る。 「context に乗せる前に history に commit する」 という CLAUDE.md の加工原則に揃う。 notify 系の history 焼き込みは `tickets/notify-history-persist.md` が別途扱う領域で、 本チケットは system_message 経路の置換のみを範囲とする。

## 要件

- session log entry の commit は単一経路 (`Pod::append_entry` 相当) を通り、 「永続書き込み + in-memory mirror 更新 + `Event::Entry(LogEntry)` broadcast」 を atomic に行う。 atomicity は alerter と同じパターンの `subscribe_with_snapshot` 用ロックで保証される。
- entry commit は **per-item / per-block 粒度** で行う。 現在の turn 末尾一括の `persist_turn` / `save_delta` を分解し、 mid-turn 接続で進行中の tool call / 確定済み assistant block / user input すべてが snapshot から見える状態にする。
- 接続クライアントは接続時に `Event::Snapshot { entries: Vec<LogEntry>, greeting, status }` を受信し、 続けて live `Event::Entry(LogEntry)` を時系列で受信する。 prefix と suffix の境目に重複・欠落が無い。
- typed user input (`Vec<Segment>`) は flatten/復元の往復なく client に届く。 `PodSharedState.user_segments` と GetHistory の overlay+skip-align ロジックを廃止する。
- ストリーミング hint は変更なし継続。 ただし「確定情報は entry にあり、 hint は描画進捗のみ」 という分担を protocol 上のドキュメントで明記する。
- TUI は `Event::Snapshot` / `Event::Entry` 駆動で view を組み立てる。 既存ブロック描画と等価な LogEntry → Block mapping を実装する。
- inter-pod query (`crates/pod/src/spawn/comm_tools.rs` の GetHistory 経路) は新 snapshot 形式に追従する。

## 完了条件

- session log entry 1 件の commit が、 永続書き込みと `Event::Entry` broadcast を atomic に同期させる経路で行われる。 mid-turn の任意の瞬間で、 session log と Event::Entry の到達順が常に整合する。
- 接続時に `Event::Snapshot` が必ず流れ、 直後から live `Event::Entry` が同型で連続する。 mid-turn 再アタッチで進行中の user input / 確定済み assistant 出力 / 進行中の tool call / 確定済み tool result が view に再現される。
- `Method::GetHistory` / `Event::History` / `Event::SystemMessage` が protocol から削除されている。 後者 2 つは `Event::Entry` (`HookInjectedItems` バリアント等) で代替される。
- `PodSharedState.history` / `PodSharedState.user_segments` が削除されている。 `PodSharedState` は status / greeting / fs_view / workflow / knowledge の lookup ハブとして残る。
- `crates/pod/src/runtime/dir.rs` の `history.json` write は廃止または用途縮小される (session log が正本)。
- 既存テスト (`crates/pod/tests/controller_test.rs`、 `crates/session-store/tests/`、 TUI 関連) が通る。 ターン中再アタッチで in-flight turn の user_input が view に含まれることを示すテストが新規追加されている。

## 範囲外

- `LogEntry` スキーマの変更 (バリアントは現状維持)。
- compaction / fork 動作の変更 (既存の `SessionStart.{compacted_from, forked_from}` がそのまま使われる)。
- TUI rendering の機能拡張。 LogEntry → 既存 Block の mapping は等価再構成に留め、 装飾追加は別チケット。
- `PodSharedState` の完全廃止と Pod 借用構造の分解。 controller が `&mut Pod` を握る構造は変えない。
- broadcast cap (256) の最適化、 ストリーミング hint の replay buffer 化。
- `Method::ListCompletions` の subscribe 化 (これは真の query なので RPC 形のまま残す)。

## 関連

- `tickets/pod-persistent-state.md` の「session log は引き続き会話状態の唯一の復元ソース」方針と整合する。 Pod identity 永続化は引き続き別チケット領域。
- `tickets/notify-history-persist.md` の「context に乗せる前に history に commit」 原則と同根。 本チケットは system_message 経路の置換まで、 notify 経路は当該チケットで扱う。
