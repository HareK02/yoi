# Review: OpenAI Responses prompt_cache_key 送出によるキャッシュ有効化

## 前提・要件の確認

### llm-worker 側
- `Request` に `cache_key: Option<String>` を追加し doc を整備:
  `crates/llm-worker/src/llm_client/types.rs:458-465`。
  `cache_anchor` の直下、要件通りの位置。doc にも「会話単位の安定キー」「`prompt_cache_key` として送られる」「持たない provider は無視」が明記されている。
- `Request::cache_key(impl Into<String>)` builder:
  `crates/llm-worker/src/llm_client/types.rs:546-552`。要件通り。
- `OpenAIResponsesScheme::build_request` で `request.cache_key.clone()` を `ResponsesRequest::prompt_cache_key` に投影:
  `crates/llm-worker/src/llm_client/scheme/openai_responses/request.rs:228`。
- `ResponsesRequest::prompt_cache_key: Option<String>` を `#[serde(skip_serializing_if = "Option::is_none")]` 付きで追加:
  同 `request.rs:51-57`。
- 他 scheme (`anthropic`, `gemini`, `openai_chat`) は touch されておらず、`Request::cache_key` を未参照のまま無視している（grep 確認済）。要件通り。

### pod 側
- 主 Worker の構築 3 経路すべてで `set_cache_key(Some(session_id.to_string()))` を実施:
  - `from_manifest`: `crates/pod/src/pod.rs:1915`
  - `from_manifest_spawned`: 同 `:1976`
  - `restore_from_manifest`: 同 `:2064`
- compactor (`summary_worker`) と memory extract (`extract_worker`) も session_id でキー付け済み:
  `pod.rs:1198`, `pod.rs:1553`。要件で明示された 3 経路（主 Run / compactor / extract）すべてカバーされている。
- `Worker::set_cache_key` の追加と、`build_request` 時の `request.cache_key = self.cache_key.clone()` 投影、`lock()`/`unlock()` 越しの引き継ぎ:
  `crates/llm-worker/src/worker.rs:193, 399-405, 600, 1338, 1418`。状態遷移で落ちないことが確認できる。

### docs
- `docs/research/openai_responses_prompt_cache_key.md` を新規作成。
  「ChatGPT backend では prompt_cache_key 必須」「codex-rs の挙動」「insomnia での Fork / Compaction 方針」「公式 API での挙動」「URL」がカバーされている。`openai_responses_max_output_tokens.md` と同じ並び。要件通り。

### 完了条件
- ユニットテスト 2 件:
  `prompt_cache_key_passed_through_when_set` / `prompt_cache_key_omitted_when_none`
  (`crates/llm-worker/src/llm_client/scheme/openai_responses/request.rs:540-560`)。
  body にキーが乗る／省略される、両方を JSON 値で確認している。完了条件を満たす。
- `cargo check --workspace` 通過確認済（手元再実行で確認）。
- `cargo test -p llm-worker --lib` 121 件パス、`-p provider --lib` 41 件、`-p pod --lib` 133 件パスを確認。
- 「pod の Run で codex-oauth + Responses を使ったとき、2 turn 目以降の `cache_read_tokens` が 0 でない」については実セッションログ要観測（実装上は events.rs のパース修正で `cache_read_input_tokens` を埋める経路が出来ており、`prompt_cache_key` も乗ることがテストで確認できているので、計測経路としては揃っている）。これはコード単体では検証できない要件で、実走行に委ねるのが妥当。

## アーキテクチャ・スコープ

- llm-worker は provider-agnostic な `cache_key` を持ち、scheme ごとの解釈は scheme 配下で完結。`cache_anchor` (Anthropic 用 prefix index) と並立して別概念として扱う方針が doc とコードの両方で明確。低レベル基盤を歪めず、`cache_anchor` の規約パターンに素直に乗っている。
- 他 scheme (`anthropic`, `gemini`, `openai_chat`) はフィールドを未参照のまま残しており、不要な実装拡散がない。
- 範囲外項目への踏み込み確認:
  - **memory consolidate worker への適用** (`pod.rs:1750`): ticket の要件節で明示されているのは Run/compactor/extract の 3 件だが、同節末尾に「compactor / extract のように pod の中で派生する worker でも 同じ session_id を使う。これにより pod 内のすべての LLM 呼び出しが同一 cache_key 名前空間で動き、prefix が共有されるところでヒットが期待できる」というポリシーが明記されている。consolidate も pod 内派生 worker なので、明示列挙されていなくとも policy の対象に当然含まれる解釈で妥当。漏れて consolidate だけ別 namespace になる方が不自然。
  - **compact 後の re-key** (`pod.rs:1363`): compact 中に `self.session_id = new_session_id` (1333) で session_id 自体が入れ替わる。worker 側のキャッシュキーを古い session_id のまま放置すると、post-compact turn と extract/consolidate (これらは `self.session_id` = 新) で namespace が分裂する。範囲外の「compaction の cache_key 戦略」は「明示キーを別系統に切り替える等の最適化」を指しており、ここは「session_id 一本で動かす」という ticket 末尾の方針 (110行) を素直に維持しているだけ。むしろ re-key しない方がポリシー違反になる。
- Worker の構築・状態遷移箇所すべてで `cache_key` をハンドリング (`Worker::new` 初期化、`lock`/`unlock` 引き継ぎ) しており、後段で見落としによる空キー問題が起きない。
- `events.rs` の `input_tokens_details.cached_tokens` 取り込みは ticket 本文では「直近のパース修正で復旧した」前提として記述されているが、実際には未コミットだった分が今回まとめて入っている。これは本 ticket 完了の前提として必要な計測経路であり (= cache 効果が実環境で確認可能になる)、ticket の精神を満たすために必要。範囲外項目ではない。

## 指摘事項

### Blocking
なし。

### Non-blocking / Follow-up
- 完了条件の最後（codex-oauth で実走行して `cache_read_tokens` が 0 でないことを実セッションログで確認）は実装変更だけでは取り込めない。ticket クローズ前に 1 セッション流して `cache_read_input_tokens` がログに出ることを確認してほしい。
- ticket 本文では `cache_anchor` の隣としていたが、`Request::cache_anchor` フィールドの doc コメントが英語、`cache_key` は日本語になっている。プロジェクト方針として混在は許容されているように見えるが、両者揃える価値はある。優先度は低い。

### Nits
- `docs/research/openai_responses_prompt_cache_key.md:79-83`「Compaction との関係」セクションは ticket 範囲外項目を補足する形になっているが、現時点の実装方針 (post-compact で new session_id に re-key) と完全整合しているので有用。残してよい。

## 判断
**Approve** — ticket の前提・要件・完了条件はコード上満たされており、ticket 範囲外と明記された fork 越し継承や compaction 戦略への踏み込みも無い。consolidate worker への展開と post-compact re-key は ticket の policy 文 (「pod 内のすべての LLM 呼び出しが同一 cache_key 名前空間で動き」「session_id 一本で動かす」) に沿った最小拡張で、コードベースを歪めていない。残るは実セッションでの `cache_read_tokens > 0` 観測のみで、これは実走確認に委ねるのが妥当。
