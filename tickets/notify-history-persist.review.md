# Review: 注入される system message をワーカー履歴に永続化する

対象コミット: `e804577 feat: notify-history-persist実装`

## 前提・要件の確認

### Notify / PodEvent 経路の挙動変更
- **`NotifyBuffer::drain` 由来の Item を `worker.history` に append**: 達成。`crates/llm-worker/src/worker.rs:859-867` で turn loop の先頭、per-request clone の前に `interceptor.pending_history_appends().await` を `self.history.extend(...)` で本体側に commit する。`crates/pod/src/ipc/interceptor.rs:127-145` で旧 `pre_llm_request` の注入ロジックを新メソッドに移送。
- **次の LLM リクエスト直前に1回だけ、複数 notify は順序保持で並ぶ**: 達成。`drain()` は破壊的、ループ各 iteration で1回呼ばれる。`pending_history_appends_drains_buffer_into_items` テストで複数 entry が順序保持で返ること、再呼出で空が返ることを検証。
- **`history.json` への永続化**: 達成。`worker.history` への通常の mutation と同じ経路（既存の `PodSharedState::history_json` → `RuntimeDir::write_history`）に乗る。実装上の追加配線は不要で、設計意図通り。
- **resume 時に読み戻される**: 達成（同上、既存パスを通る）。

### `<system-reminder>` 注入経路（session-todo-reminder の前提変更）
- 本コミットでは reminder 実装そのものは扱わず、`tickets/session-todo.md` の方針記述のみ書き換え。`pending_history_appends` のドキュメント (`crates/llm-worker/src/interceptor.rs:121-148`) が新規 system message を history に commit する責務であることを明示し、reminder もこのレーンに乗ることが構造上保証される。チケットが許容する後者パターン（session-todo-reminder 実装時に同原則に従う）を取っており、整合済み。

### 注入レーンの統一
- **`notify_buffer.rs` モジュール doc**: 達成。`crates/pod/src/ipc/notify_buffer.rs:1-19` で「single lane for system messages produced by Pod state」「no transient, history-skipping lane」を明記し、`tickets/notify-history-persist.md` と `AGENTS.md` を参照。
- **trait レベルの規約**: `crates/llm-worker/src/interceptor.rs:121-148` に `pending_history_appends` と `pre_llm_request` の役割を doc で対比。`pre_llm_request` を「決定的・履歴非依存変換」のみに使い、外部 input は前者に乗せる旨を明記。CLAUDE.md / AGENTS.md の「LLM コンテキストの加工原則」の体現として適切。

### 既存テスト・ドキュメント更新
- `pre_llm_request_drains_pending_notifies_into_context` → `pending_history_appends_drains_buffer_into_items` にリネームし、新レーン側の挙動検証に置換。
- `pre_llm_request_skips_notification_injection_when_yielding` → `pre_llm_request_does_not_touch_pending_notifies` に変更。新方針上、yield 経路で buffer を保持する責務は不要（history に既に commit 済みであれば resume 後に再取得不要）なので、合理的な置換。
- `controller_test.rs` の `notify_while_idle...` および `pod_event_turn_ended_while_idle...` に `handle.shared_state.history()` への assertion を追加し、request 側だけでなく history 側にも反映されることを検証。
- `tickets/session-todo.md` / `TODO.md` 末尾の「履歴を汚さない」記述を撤回・上書き済み。

## アーキテクチャ・スコープ

- **責務分離**: `Interceptor` trait に default 実装付きの新メソッドを1個追加し、Worker が turn loop で1回呼ぶだけ。llm-worker 層は「外部 input の中身」を知らず、低レベル基盤として hook されるだけ、という layer 区分を維持している。
- **抽象の規模**: 新規概念は `pending_history_appends` 1個のみ。NotifyBuffer の改名や別キュー新設には踏み込まず、ticket の「実装裁量」のうち最小コストの選択（既存 buffer をそのまま流用、レーンの解釈を doc で更新）を取った。歪みなし。
- **挙動の対称性**: 旧来 yield 時に buffer を保持していた設計が消えるが、新原則では「外部 input は受信時点で history 化されるべき」なので、turn loop 先頭での無条件 append は正しい。compaction 後 resume では buffer は既に空・履歴に entry あり、という状態に揃うのが意図通り。
- **LLM コンテキストの加工原則との整合**: 本ticketは原則そのものの適用であり、doc 側でも `pre_llm_request` と `pending_history_appends` の責務境界を明示している。原則に正面から沿った変更で、コードベースを歪めるどころか、これまでの破綻を正している。

## 指摘事項

### Blocking
- なし。

### Non-blocking / Follow-up
- **ticket が名指しした test ファイル位置との乖離**: ticket 要件に「`crates/pod/tests/pod_events_test.rs` で `PodEvent` 受信後に history に対応 Item が現れることを E2E に近い粒度で確認するケースを追加する」と明記されているが、本コミットは既存 `controller_test.rs::pod_event_turn_ended_while_idle_auto_starts_turn_and_injects_system_message` に history assertion を追加する形で代替している。機能カバレッジは同等以上（既存テストを重複させない方が健全）だが、文字通りの要件には合わない。今後の維持で問題は出ない見込みだが、必要なら ticket 側を「カバレッジは controller_test.rs 側で確保済み」と注記しておくと将来の読み手が混乱しない。
- **compaction yield 経路の挙動が暗黙化**: 旧 `pre_llm_request_skips_notification_injection_when_yielding` は「yield 中は buffer に積み残す」を保証していた。新設計では「turn loop 冒頭で必ず history に commit、yield しても history は残る」だが、これを直接検証するテスト（compaction 発火 → resume 後に history へ notify が残ること）は追加されていない。既存 compaction テストが history 側で間接的に保護してくれる範囲だが、回帰検出力としては薄い。session-todo-reminder で reminder を載せる際に同種の検証を入れる前提なら follow-up で十分。

### Nits
- `crates/pod/src/ipc/interceptor.rs:14-22` の use 順序: `tracing::warn`（line 20）が `llm_worker::interceptor::...`（line 16-19）と `llm_worker::tool::ToolOutput`（line 21）の間に入り、`llm_worker::` グループが分断されている。`tracing::warn` を `tracing::info` の隣（line 22）に置けば従来のグルーピングが保てる。`cargo fmt` がこれを揃えているなら無視可。

## 判断

**Approve（完了可）** — 要件は実質すべて満たされており、CLAUDE.md / AGENTS.md の「LLM コンテキストの加工原則」を体現する模範的な変更。ticket が名指しした test ファイル位置との微妙な不一致は機能カバレッジ等価のため非ブロッキング。
