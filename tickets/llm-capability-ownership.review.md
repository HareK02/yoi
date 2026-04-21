# Review: モデル capability の責務を llm-worker 外へ移す

## 前提・要件の確認

1. **`Scheme::capability_for` の廃止**: 満たされている。
   - `crates/llm-worker/src/llm_client/scheme/mod.rs:79-83` では trait メソッドが削除され、doc コメントも「`provider::capability::lookup` 側（高レベル構築層）の責務」と明記。
   - 各 `scheme_impl.rs`（anthropic / openai_chat / openai_responses / gemini）から `capability_for` impl が消えていることを diff で確認。
   - 念のため grep で `capability_for` 残骸を全 crate 検索 → チケット本文以外の参照なし。

2. **モデル知識を `crates/provider` へ移す**: 満たされている。
   - `crates/provider/src/capability.rs:1-153` に Anthropic / OpenAI Chat / OpenAI Responses / Gemini の 4 テーブルと、openai_chat と openai_responses で共有していた `OpenAiFamily` / `openai_classify` を統合。
   - 公開 API は要件通り `pub fn lookup(scheme: SchemeKind, model_id: &str) -> Option<ModelCapability>`（`crates/provider/src/capability.rs:20-27`）。
   - `crates/provider/src/lib.rs:13` で `pub mod capability;` としてモジュール公開。

3. **`build_client` の capability 解決順**: 満たされている。
   - `crates/provider/src/lib.rs:119-123` で `config.capability.clone().or_else(|| capability::lookup(...)).unwrap_or_else(|| scheme.default_capability())` の 3 段階フォールバックが明示されており、コメントも要件の 3 階層とそれぞれの責務を正しく対応付けている。

4. **examples の追従（provider に依存させない）**: 満たされている。
   - `crates/llm-worker/examples/worker_cli.rs:341-349`: `scheme.default_capability()` 直接利用。Ollama 分岐（`worker_cli.rs:378`）だけは従前通りローカル `default_capability()` 関数を使用 → これは「明示 `ModelCapability` に置換」の要件に合致。
   - `crates/llm-worker/examples/worker_cancel_demo.rs:26-28`: `scheme.default_capability()` のみ。ローカル fallback 定義は削除済。
   - `crates/llm-worker/examples/record_test_fixtures/main.rs:22-36`: `scheme.default_capability()` + Ollama 分岐で `AnthropicScheme::new().default_capability()`。provider クレート import なし。
   - examples の `Cargo.toml` に provider 依存が追加されていないことも間接的に確認（worker_cli / worker_cancel_demo は `llm_worker::` のみ import）。

5. **テストの移設**: 満たされている。
   - 旧 `scheme/openai_responses/capability.rs` の `#[cfg(test)] mod tests` は削除。
   - `crates/provider/src/capability.rs:155-211` に 8 件の unit test（Anthropic known/unknown, OpenAI chat gpt5, OpenAI responses codex / gpt-5-codex, Gemini 2.5 / 1.5, unknown across schemes）を配置。旧来 `gpt_5_codex_is_reasoning` / `codex_mini_latest_is_reasoning` / `unknown_model_returns_none` の振る舞いは保存されている。
   - scheme 側の capability.rs には `#[cfg(test)]` 残留なしを grep で確認。

6. **完了時の動作**: 満たされている。
   - `cargo check --workspace --examples` 成功（warning は既存の `end_scope` 未使用のみ、今回の変更と無関係）。
   - `cargo test -p provider --lib` 33/33 pass（capability テスト 8 件込み）。
   - `cargo test -p llm-worker --lib` 100/100 pass。
   - 疎通テストの実行ログは diff に含まれていないが、静的テーブルは 1:1 で移植されており挙動差は出ない（下記「挙動保存の確認」参照）。

## アーキテクチャ・スコープ

### レイヤ境界
`llm-worker` から `claude-*` / `gpt-5` / `codex-` / `gemini-2.5` 等のモデル ID 固有知識が完全に撤退し、`provider` に集約された。CLAUDE.md の「llm-worker は wire scheme の低レベル基盤に留める」方針と feedback memory `llm_worker_scope` に厳密に合致する、このチケットの趣旨通りの成果。

### モジュール粒度
`provider/src/capability.rs` を 1 ファイルに統合した判断は本件では妥当。理由:
- 統合前の 4 ファイル合計 ~180 行で、既に 1 ファイルに収まるサイズ。
- `OpenAiFamily` / `openai_classify` が chat / responses 両方から参照される「一次情報」だったので、同一ファイルに閉じ込めた方が `pub(crate)` スコープだけで済み、サブモジュール間の `pub(super)` 公開戦略を考えなくて良い。
- 将来テーブルが肥大化した場合の `capability/{anthropic,openai,gemini}.rs` サブモジュール分割は容易（今は不要）。

判断として Approve。将来新プロバイダを追加したときに 1 ファイル 400-500 行を超えるようなら改めて分割検討で十分。

### scheme 側 `default_capability()` の存置
ticket 設計判断の通り、「この wire で安全に送れる最小共通項」は wire 知識であり scheme 層に残す意味がある。Ollama（Anthropic scheme + 認証なし + `default_capability`）経路でも wire 既定が有効に機能する（`crates/llm-worker/examples/record_test_fixtures/main.rs:126` など）。Approve。

### examples の provider 非依存
worker_cli の Ollama 分岐が、provider 層に依存せずローカル `default_capability()` 関数で手組み `ModelCapability` を作っている実装は、チケット設計判断「examples を provider に依存させない」に一致。循環依存の予防としても正しい。Approve。

## 挙動保存の確認（旧 → 新のテーブル）

旧 4 ファイルの `lookup` / `classify` / `OpenAiFamily` と `provider/src/capability.rs` を突き合わせた結果、全フィールド一致:

| モデル family | 旧位置 | 新位置 | 差分 |
| --- | --- | --- | --- |
| Anthropic `claude-*` | `scheme/anthropic/capability.rs` | `provider::capability::anthropic_lookup` | 無し（`Explicit { max_breakpoints: 4 }`, `BudgetTokens`, `vision: true`） |
| OpenAI `gpt-5` / `o1` / `o3` / `o4` | `scheme/openai_chat/capability.rs` | `openai_classify` + `openai_chat_lookup` | 無し（Reasoning: `Effort`） |
| OpenAI `gpt-4` | 同上 | 同上 | 無し |
| OpenAI `gpt-3.5` | 同上 | 同上 | 無し（`JsonObject`, `vision: false`） |
| OpenAI Responses `codex-*` prefix | `scheme/openai_responses/capability.rs` | `openai_responses_lookup` の `or_else` | 無し（Reasoning にフォールバック） |
| Gemini `gemini-*` (一般) | `scheme/gemini/capability.rs` | `gemini_lookup` | 無し |
| Gemini `gemini-2.5` / `gemini-3` | 同上 | 同上 | 無し（`BudgetTokens`） |

`default_capability()` 値も全 scheme で保存（anthropic: vision=false, openai_chat: vision=false, openai_responses: vision=false, gemini: vision=true）。**特に gemini の `vision: true` が保たれているか**というレビュー観点は OK。

## 指摘事項

### Blocking
なし。

### Non-blocking / Follow-up

- **[major][スコープ外変更の混入]** チケット diff にスコープ外の Codex OAuth 疎通修正が同居している（`crates/llm-worker/src/llm_client/scheme/openai_responses/scheme_impl.rs` の `default_base_url` / `path` 変更、`crates/provider/src/lib.rs::effective_base_url`、`crates/provider/src/codex_oauth/mod.rs::build_headers`、`crates/manifest/src/model.rs` の `#[serde(rename = "codex_oauth")]`）。ユーザー自身も「別コミット候補」と認識しているので後続コミット分割で対処できるが、レビュー観点としては:
  - 本チケットの review が「capability 移設」単体の妥当性判定を越えて Codex 疎通修正の可否まで巻き込むことは避けるべき。
  - 少なくともコミット段階で `llm-capability-ownership` コミットと「Codex OAuth 疎通修正」コミットを分けた方が後から git log で挙動回帰を追える。
  - `path` を `/v1/responses` → `/responses` にしつつ `default_base_url` に `/v1` を含める変更は OpenAI 公式経路と ChatGPT backend 経路の URL 組立てを統合する話で、本チケットで動作させた疎通確認が「capability 移設後の回帰」なのか「URL 変更の検証」なのかを混同しやすい。次回以降、本性質の「ついで修正」は別チケット/別コミットに切ることを推奨。

- **[minor][公開 API の可視性]** `provider::capability::lookup` は Pub だが、`OpenAiFamily` / `openai_classify` / `*_lookup` 系 helper が `pub(crate)` ですら宣言されておらず、module-private 関数として閉じている。現状 `build_client` は `lookup` 1 点経由で十分だが、将来 `llm-provider-catalog` から family を参照したいユースケースが出た時点で公開化を検討すればよい。今は Approve。

- **[minor][テスト粒度]** 旧 `openai_responses` の `gpt_5_codex_is_reasoning` / `codex_mini_latest_is_reasoning` / `unknown_model_returns_none` が `provider/src/capability.rs` tests に移植されたのは確認済だが、もとの `openai_chat::lookup` 側には個別 test が無かったので、新テーブルでも gpt-4 / gpt-3.5 の classify 成否テストは追加されていない。挙動保存の観点では現状で十分（ticket 要件は「移設」なので）。新モデル追加時に境界テストを足す運用で Approve。

### Nits

- `provider/src/capability.rs` の section コメント `// --- Anthropic ---` 等、既存 codebase 他所ではあまり見かけないスタイル。統一上は `mod anthropic { ... }` サブモジュール化でも良かったが、短い関数群の可読性補助としては問題なし。
- `lib.rs:119-123` の capability 解決コメントが丁寧で、将来の保守者にとって助かる。good keep.

## 判断

**Approve** — 要件 1-6 すべてを満たし、テーブルの移設は旧挙動を完全保存、レイヤ境界を `llm-worker / provider` 間で正しく引き直した。scheme 側 `default_capability()` の存置判断、examples を provider 非依存に保った判断とも妥当。

ただし「スコープ外 Codex 疎通修正」の同居は、このチケットのコミット外に切り出すのが望ましい（コミットは user の責務なので、分割方針を user 判断で）。
