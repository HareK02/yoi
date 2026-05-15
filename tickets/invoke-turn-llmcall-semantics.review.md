# Review: Invoke / Turn / LlmCall セマンティクス整理

レビュー対象実装: `d0dbac1 feat: Invoke marker と LlmCall callback を導入し AgentTurn セマンティクスを明確化` (worktree `invoke-turn-llmcall-semantics`)。

## 前提・要件の確認

決定事項に明記されたスコープ B (コード反映) を順に対応付ける。

- **`Event::InvokeStart { kind: InvokeKind }` / `Event::LlmCallStart` / `Event::LlmCallEnd` 追加**: `crates/protocol/src/lib.rs:248-296`。serde tag は既存の `event` 名前空間に乗り、新 variant のみで既存 variant は無傷。round-trip テスト `event_invoke_start_roundtrip` / `event_llm_call_start_end_roundtrip` (`crates/protocol/src/lib.rs:751-805`) で 5 種すべての kind と llm_call カウンタを担保。
- **`InvokeKind` enum**: `crates/protocol/src/lib.rs:521-540`。`UserSend` / `Notify` / `PodEvent` / `SystemReminder` / `Wakeup` の 5 variants が `snake_case` で wire 化されている (チケット決定通り)。配置も `protocol` crate (`session-store` が import する向き) で正しい。
- **`Worker.llm_call_count` + `on_llm_call_start` / `on_llm_call_end` callback**: `crates/llm-worker/src/worker.rs:166-175`, `:343-355`, `:1050-1065`。
  - 増分位置は `stream_response` 直前後で 1 ペアずつ発火 → callback の対称性が保たれる (interceptor の Cancel/Yield 早期 return では fire しないため pair-balance OK)。
  - `Mutable → Locked` / `Locked → Mutable` の遷移時に新 field をすべて移送している (`:1241-1252`, `:1503-1517`, `:1589-1603`) — state machine の整合性は維持。
- **`Worker::turn_count` の doc 更新**: `crates/llm-worker/src/worker.rs:153-160`, `:498-503`。AgentTurn 数として再定義 + 「today retry 未実装で LlmCall と 1:1」明記。実装上の増分点は `:1070` で `llm_call_count += 1` と同じ場所を共有する形で、retry が入ったら増分点を分離する旨が doc で示されている。
- **`LogEntry::Invoke { ts, trigger }` 追加 + 開始時 commit**: `crates/session-store/src/session_log.rs:121-136`。`field 名は trigger` (serde tag `kind` との衝突回避) の判断は doc コメントに明記されている (`:130-131`)。`replay_invoke_marker_does_not_mutate_state` (`:706-735`) で「replay は marker のみで state 不変」が担保。pod 側 commit は `crates/pod/src/pod.rs:1147-1153` (`run`) と `:1508-1515` (`run_for_notification`)。順序は `prepare_for_run → Invoke marker → UserInput / SystemItem` で、ticket 「直後の Turn entry に payload を書く」に整合。
- **controller の wiring + broadcast**: `crates/pod/src/controller.rs:306-313` で `on_llm_call_start/end` を `event_tx` に橋渡し。`PendingRun::RunForNotification(InvokeKind)` (`:97-103`, `:565-579`) で `Method::Notify` (kind=`Notify`) と `Method::PodEvent` (kind=`PodEvent`) からそれぞれ正しい kind が伝搬している (`:660`, `:730`)。`crates/pod/src/ipc/server.rs:118-120` で `LogEntry::Invoke → Event::InvokeStart` 変換、`crates/pod/src/session_log_sink.rs:124` で live broadcast filter に追加 — 新 client の snapshot 復元時には mirror 経由、live 接続中の client には broadcast 経由で同じ event が届く既存パターンを正しく踏襲。

完了条件のうち文書化系 (定義・retry 境界・SystemTurn 範囲・案 A 採択・persistence-semantics 整合) は元チケット本体に明記されており、コード側のコメント (`session_log.rs:121-136`, `protocol/src/lib.rs:248-296`, `worker.rs:153-175`) でも繰り返し引用されている。

## アーキテクチャ・スコープ

- **層の境界**: 新 callback は `Worker` の純然たる observable として置かれ、Pod / controller 側で broadcast に橋渡しする既存パターンと一致。`llm-worker` は低レベル基盤に留めるという memory の方針 (`feedback_llm_worker_scope`) を尊重している。
- **Provider policy**: 触らない。
- **Cargo.toml**: 依存追加なし — `cargo add` 規則は無関係。
- **互換性**: protocol の既存 variant は無変更、新 variant のみ追加。range 外と明示された `Hook::OnTurnEnd` rename / `LogEntry::TurnEnd` commit 位置移動 / retry 本実装 / TUI UI 設計のいずれにも踏み込んでいない。
- **`run_for_notification(kind)` の API 拡張**: 引数追加は最小だが、`debug_assert!` で `UserSend` を弾く (`crates/pod/src/pod.rs:1496-1505`) — `UserSend` は `pod.run()` 専用という呼び出し規約を実行時に強制している。型レベル分離 (例: `NotifyInvokeKind` newtype) より軽量で、本チケット範囲では妥当な選択。
- **`PendingRun::RunForNotification(InvokeKind)`** はタプル variant に最小限 1 引数だけ持たせており、`is_parent_originated()` の判定にも影響しない (`controller.rs:115`)。Notify と PodEvent の kind 違いを controller 入口で確定させる方針は読み手にとっても自然。
- **Resume を Invoke 対象から除外**: `Method::Resume` は IDLE → active ではなく Paused → active なので Invoke marker を打たない、という判断は明示されていないが、ticket の「IDLE → active 遷移時に追記」と一貫しており、判定として正しい。doc に注記があると親切だが blocking ではない。

## 指摘事項

### Blocking

(なし)

### Non-blocking / Follow-up

- **`Event::UserMessage` の doc コメントが実際の発火順序と食い違う**: `crates/protocol/src/lib.rs:212-214` の "Fires exactly once per accepted `Method::Run`, after `InvokeStart { kind: UserSend }` and before the first `TurnStart`" は、実装上 controller が `Method::Run` 受信直後 (`controller.rs:636-638`) に `event_tx.send(UserMessage)` を fire し、`InvokeStart` (entry broadcast 由来) は `pod.run()` 内の `prepare_for_run → commit_entry(Invoke)` を経て後発で出る。同じ socket 上では IPC handler の `select!` で multiplex されるため client から見た順序保証は弱いが、実態として `UserMessage → InvokeStart` の順で届く。doc 側を実態に合わせる (順序は逆) か、controller 側を `event_tx.send(UserMessage)` を Invoke commit の後に動かすかのどちらか。前者で十分 (TUI は今回 Invoke を no-op で受けるため UI 影響なし)。
- **`Resume` 経路に Invoke を打たない判断の doc 化**: `pod.resume()` が IDLE → active ではない (Paused → active) ため Invoke marker 不要、を `pod.rs` の `resume()` doc または `LogEntry::Invoke` doc に一行注記すると後続の persistence/fork 実装者が迷わない。
- **End-to-end の Invoke commit を担保するテストが薄い**: protocol/session-store の round-trip + replay 不変性は単体テストで担保されているが、`pod.run()` を実走させて session log mirror に `LogEntry::Invoke` がちゃんと並ぶ統合テストはない (`controller_test.rs:history_from_sink` は Invoke を `_ => {}` で無視している)。`pod-session-fork` / `at_turn_index` 整合の後続チケットで初めて触れる前に、ここで一段階アサーションを入れておく価値はある (今回の範囲外なら別チケットで OK)。

### Nits

- `crates/pod/src/pod.rs:1147-1153` と `:1508-1515` で `self.session_id = self.session_head.lock().session_id;` を Invoke 直前にも書いているが、この再代入は `commit_entry` 自体が `session_head.lock()` から `session_id` を取り直すため動作には不要 (元コードと同じく単なる bookkeeping)。意図的なら問題なしだが、`pod.rs:1149` のコメントには触れられていない点、Invoke commit を前置したことで「`session_id` 同期が必要なタイミングが2回ある」ように読めるので、`prepare_for_run` 内に1回まとめて寄せるリファクタも検討価値あり (本チケット範囲外)。
- `crates/llm-worker/tests/worker_state_test.rs:355-366` は AgentTurn:LlmCall 1:1 の現状を担保するが、tool_use を含む単一 `run()` で複数 LlmCall が立つケース (= 同 run 内で `llm_call_count` が複数増える) のテストはない。retry 実装後に「retry が turn_count を増やさない」アサーションを追加するため、ベースとして tool_use 連鎖テストを近いうちに足しておくとリグレッション検出力が上がる。

## 判断

**Approve** — スコープ B の決定事項はすべて反映されており、protocol の既存互換も保たれ、Worker / Pod / IPC / TUI の各層に対する変更は最小かつ既存パターンを踏襲している。`UserMessage` doc の順序記述が実態と逆な点は文言修正で済む follow-up であり、機能面・型安全性・ビルド整合に blocking はない。`cargo build --workspace` および `cargo test --workspace` がいずれもクリーンに通ることを確認済み。
