# reasoning ブロックを history に永続化する

## 背景

`docs/ref/model-reasoning-context.md` でまとめた通り、近年の Reasoning 対応モデルは「assistant 応答に含まれた thinking / reasoning ブロックを次のリクエストに戻す」ことを前提に設計が進んでいる。特に:

- Anthropic: ツール使用ループ (`tool_use` → `tool_result` → 続きの assistant) では同一論理ターン内で thinking ブロックを **必ず** 返送する必要がある。新世代 (Opus 4.5+/Sonnet 4.6+) ではマルチターンでも保持がデフォルト。thinking ブロックには `signature` が付与され、改ざん検知に使われる
- OpenAI Responses API: reasoning items を `previous_response_id` または `output` 再送で引き継ぐ設計。o3/o4-mini 以降は function call 隣接の reasoning items が連続性に効く
- Ollama: client 側で thinking フィールドの再送有無を制御する責務

現状の `llm-worker` は streaming イベントとして thinking デルタを観測でき、`Worker::on_thinking_block` callback まで到達する一方で、**`worker.history` 上の `Item` には一切 commit されず、ターン境界で蒸発する**。`worker.rs:976` で `text_block_collector` のみが `take_collected()` され、`build_assistant_items` (`worker.rs:591-610`) は `assistant_message` と `tool_call` しか生成しない。

加えて、Anthropic 応答中の `SignatureDelta` は `anthropic/events.rs:256` で明示的に捨てられている。`Item::Reasoning` (`types.rs:84-102`) にも `signature` フィールドがない。このまま新世代 Claude に thinking を送り返そうとすると signature 不整合で 400 を返される可能性が高い。

CLAUDE.md の「LLM コンテキスト加工原則」に照らしても、thinking を history に commit せず context にだけ載せる解決は禁止側に該当する。最初から history 上の Item として扱うのが正解。

## 要件

- llm-worker のストリーミング層で受信した thinking / reasoning ブロックが、ターン終了時に `Item::Reasoning` として `worker.history` に append され、`history.json` に永続化される
- Anthropic の `signature` が round-trip で保持される (受信→Item に格納→次リクエストの assistant message として再送)。`Item::Reasoning` に必要なフィールドを追加する
- ツール使用ループ内 (同一論理ターン: assistant → tool_use → tool_result → 次の assistant) で、直前 assistant ターンの reasoning が次のリクエストに含まれる
- 通常のマルチターン (新しいユーザー入力をまたぐ) でも reasoning が引き継がれる。世代別 keep/strip のデフォルト分岐は本チケットでは扱わず、保持側の挙動を実装する
- 既存の `Worker::on_thinking_block` callback の発火タイミング・ペイロードは互換維持
- OpenAI Responses scheme で既に部分対応している `encrypted_content` / `summary` の経路は活用しつつ、history への commit ルートを統一する
- resume (`history.json` から再開) 時にも reasoning が再現できる (これは history に乗ってさえいれば自動)

## スコープ外 (フォローアップ候補)

以下は本チケット完了後に別途検討する。本チケットでは触らない:

- モデル世代別の keep/strip デフォルト分岐 (Opus 4.5+/Sonnet 4.6+ vs それ以前、OpenAI Responses vs Chat Completions)
- Anthropic `clear_thinking_20251015` context-editing 戦略の実装
- `prune.rs` の reasoning aware 化 (古い reasoning の選択的剥離)
- Ollama scheme の `think` パラメータ対応 (そもそも Ollama scheme 自体が未実装)
