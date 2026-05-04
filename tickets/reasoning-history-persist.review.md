# Review: reasoning ブロックを history に永続化する

## 前提・要件の確認

- **streaming 層で受信した reasoning が `Item::Reasoning` として `worker.history` に append される**
  - 満たされている。`crates/llm-worker/src/llm_client/scheme/anthropic/events.rs:373-389` で `content_block_stop` 時に `pending_thinking` から `Event::ReasoningItem` を発火、`worker.rs:1006-1011` で `reasoning_item_collector.take_collected()` → `build_assistant_items` 経由で history に append。`tests/reasoning_round_trip_test.rs:25-62` が end-to-end で検証。
- **Anthropic `signature` の round-trip 保持**
  - 満たされている。受信側は `events.rs:350-358` で `signature_delta` を蓄積、`scheme_impl.rs:36-52` で `PendingThinking` に格納、`Item::Reasoning::signature` (`types.rs:100-105`) として保持。送信側は `request.rs:340-367` で `signature` を見て `AnthropicContentPart::Thinking` を投影。`signature` 無し + `encrypted_content` あり → `RedactedThinking`、両方無し → drop の分岐も妥当。
- **ツール使用ループ内の reasoning 引き継ぎ**
  - 達成されている。Item として history に乗っているので、次の `build_request` でそのまま items に含まれる。ただし下記「アーキテクチャ・スコープ」参照: 同一論理ターン内の thinking が wire 上で同一 assistant message に纏まらない既存のメッセージ束ね方の問題が残る。
- **通常のマルチターン (新しいユーザー入力をまたぐ) での引き継ぎ**
  - 同上。Item として残るので保持される。世代別 keep/strip はスコープ外として明記済みで意図通り。
- **`Worker::on_thinking_block` callback 互換性**
  - 維持されている。`Event::ReasoningItem` は `BlockStart/Delta/Stop(Thinking)` と並行発火する設計 (`event.rs:48-58`、`anthropic/events.rs:336-348`、`openai_responses/events.rs:458-471`)。`pod/src/controller.rs:216` の既存 thinking callback が引き続き機能することを確認。
- **OpenAI Responses の `encrypted_content` / `summary` を活用**
  - `openai_responses/events.rs:30-74` で `pending_reasoning` に蓄積、`output_item.done` (Reasoning) で `Event::ReasoningItem` 発火 (`events.rs:366-395`)。送信側は既存の `request.rs:286-309` が `Item::Reasoning` を `InputItem::Reasoning` に投影しており、`encrypted_content` も伝搬する。
- **resume 時の reasoning 再現**
  - `session-store/src/logged_item.rs:36-46, 95-108, 147-159` で `LoggedItem::Reasoning` に `signature` を追加し、永続化スキーマ側でも保持される。`session.rs:206-211` の assistant 群グルーピングにも `is_reasoning()` が含まれており LogEntry::AssistantItems に正しく束ねられる。`tests/reasoning_round_trip_test.rs:142-211` が injected reasoning が outgoing request に乗ることを確認。

## アーキテクチャ・スコープ

- **CLAUDE.md「LLM コンテキスト加工原則」遵守**
  - reasoning を context に差し込むのではなく、必ず `worker.history` に commit してから次リクエストの items に出す設計になっており、原則に沿う。
- **layer 分離**
  - llm-worker 内で完結しており低レベル基盤に閉じている。Pod / 上位層への漏れなし。
- **新規モジュールの妥当性**
  - `timeline/reasoning_item_collector.rs` 新設は `text_block_collector` / `tool_call_collector` と対称な構造で、責務分割が明確。`Event::ReasoningItem` を独立イベントにし、streaming 表示用 `BlockType::Thinking` と persist 用 `ReasoningItem` を別経路にした責務分離も合理的。
- **scheme 側の `parse_with_state` 導入**
  - Anthropic は `content_block_stop` が block_type を持たない都合で既に state が必要だったので、`pending_thinking` の蓄積を同じ場所に置く判断は自然。`signature_delta` をストリーム表示には流さず state にだけ蓄積する選択も合理的。
- **`build_assistant_items` の順序 (Reasoning → text → tool_call)**
  - Anthropic 仕様 (thinking はアシスタントメッセージの先頭) を意識した並びは妥当。ただし下記 Blocking 参照。
- **不必要な実装は見当たらない**
  - 各変更点はチケット要件と直結しており、scope creep なし。スコープ外 3 件 (世代別 keep/strip、`clear_thinking_20251015`、prune-aware) は明示されており触られていない。

## 指摘事項

### Blocking

なし。Anthropic 側の wire 表現に懸念 (Non-blocking 参照) があるが、本チケットで「ある reasoning item が history に commit され、次リクエストの `Request::items` に signature 付きで載る」という最小要件は達成されており、先行マージしてフォローアップで対応する判断は妥当。

### Non-blocking / Follow-up

- **Anthropic で同一論理ターンの content blocks が複数の assistant messages に分割される**
  `crates/llm-worker/src/llm_client/scheme/anthropic/request.rs:242-301` の `convert_items_to_messages` は、`Item::Reasoning` (assistant pending) と `Item::Message{Assistant}` (text) が連続するとき、前者を一度 flush して別 message にしてから text を別 message として追加する。結果として `messages = [..., assistant[Thinking], assistant("text"), assistant[ToolUse], ...]` のように同一論理ターンが3つの assistant message に割れる。Anthropic 仕様上、thinking blocks は最終 text/tool_use と**同一 assistant message の content 配列の先頭**に並ぶ必要があるので、新世代モデル (Opus 4.5+/Sonnet 4.6+) でツール使用ループに入ったとき signature 検証や thinking continuity が壊れる可能性がある。
  - これは pre-existing な分割パターン (text + tool_call の時点で既に複数 assistant message を生む) を Reasoning が継承した形であり、現行 production が動いている以上 Anthropic 側で許容されている可能性もある。ただし thinking signature の round-trip という本チケットの中核要件にとっては実 wire 上のリスクが残る。
  - 対応案: `convert_items_to_messages` で「連続する assistant 系 Item (Reasoning + assistant_message + ToolCall) は一つの assistant message に統合する」事後マージを入れる、もしくは `Item::Message{Assistant}` 受信時に `pending_assistant` を flush せず content_part を pending に追加してから flush する形に変える。

- **wire 構造を検証するテストが薄い**
  Anthropic 側の `reasoning_with_signature_projects_thinking_part` (`request.rs:817-838`) は thinking part の存在のみ検証し、message 配列内の位置・隣接 message との関係をチェックしていない。上記 follow-up と合わせて「Reasoning + assistant_message + ToolCall を含む history が単一 assistant message にまとまる」アサーションを足すと回帰検出に有効。

- **session-store: `signature` の round-trip テストが無い**
  `logged_item.rs:267-287` の `round_trip_reasoning_preserves_encrypted_content` は signature を検証していない。フィールドを追加した以上、JSON シリアライズ → デシリアライズで `signature` が保持されることを 1 ケース足しておくと安全 (現状実装は通るはずだが、将来 `LoggedItem::Reasoning` をいじったときの保険)。

- **`flush_usage` 直後の `Event::ReasoningItem` 順序**
  Anthropic では `content_block_stop` で BlockStop と同時に ReasoningItem が発火する。Worker 側は streaming 完了後にまとめて collector を `take_collected()` するので順序問題はないが、scheme 側で 1 回の `parse_with_state` から 2 イベントが返るパスは新しいので、念のため将来的に `Vec<Event>` のサイズ前提で組まれた呼び出し側 (テスト含め) が無いか軽く確認しておくと安心。

### Nits

- `crates/llm-worker/src/llm_client/event.rs:54` のコメント「上位層（Worker / ReasoningItemCollector）はこれを `Item::Reasoning` として `worker.history` に append する。」 — これは「Worker が ReasoningItemCollector 経由で取り出して」の方が実装と整合する (collector 自体は append しない)。
- `worker.rs:144-145` で `reasoning_item_collector` 自体は `Worker` に保持されているが、外部公開メソッドが無い (collect は内部のみ)。`text_block_collector` / `tool_call_collector` も同パターンなので踏襲としては妥当。

## 判断

**Approve with follow-up** — 本チケットの要件 (signature を含む reasoning material を `Item::Reasoning` として history に commit し、scheme の送受信両方向で round-trip させる) は満たされており、tests・既存テストともパス。Anthropic wire 上で同一論理ターンが複数 assistant message に分割される件は要 follow-up だが、これは pre-existing なメッセージ束ね方の問題を Reasoning が継承した形で、本チケット範囲を超える。別チケット (世代別 keep/strip と合わせるのが自然) で対応するのが適切。
