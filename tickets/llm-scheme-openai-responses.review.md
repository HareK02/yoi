# OpenAI Responses scheme の新設 — レビュー

## 前提・要件の再確認

チケット本体の 9 要件 + 2 設計課題を前提に、実装が意図と整合しているかを確認した。`cargo check`（warning 1 / 旧由来）・`cargo test --workspace`（全 pass）通過。変更量: 新規 5 ファイル（1407 行）+ 既存 5 ファイル微修正。

## 要件達成度

| # | 要件 | 状況 | メモ |
|---|---|---|---|
| 1 | `scheme/openai_responses` 新設、`HttpTransport<S>` に差し込める | ✓ | `Scheme` trait 実装完了 |
| 2 | Request body: `tool_choice: "auto"` / `parallel_tool_calls: true` / `store: false` / `include` / `stream: true` 固定、`reasoning` 投影 | ✓ | `ResponsesRequest` 構造体に全項目、`build_request` で capability 照合して reasoning 投影 |
| 3 | SSE event パース (response.* 一式) | ✓ + α | ticket 列挙に加えて `response.content_part.done` / `response.reasoning_summary_part.added/done` / top-level `error` もカバー |
| 4 | BlockType / DeltaContent 対応（text は `content_part.added`、tool_use は `output_item.added` で BlockStart） | ✓ | `OpenAIResponsesState` の 3 種 SlotKey（OutputItem / ContentPart / Summary）で (output_index, content_index) → flat index を管理 |
| 5 | `Item::Reasoning` 拡張（text + summary + encrypted_content） | ✓ | `with_reasoning_summary` / `with_encrypted_content` ビルダー追加、既存コンストラクタは空値で互換 |
| 6 | `AuthRef::ApiKey` / `Authorization: Bearer` / base_url `https://api.openai.com` / パス `/v1/responses` | ✓ | scheme_impl.rs の `required_auth` / `default_base_url` / `path` |
| 7 | Usage 正規化 | ✓ | `response.completed` の `usage` を `UsageEvent` に変換、`total_tokens` 未提供時は input+output で補完 |
| 8 | capability 共通判定関数 | ✓ | `openai_chat/capability.rs::classify -> OpenAiFamily` を `pub(crate)` で公開、`openai_responses/capability.rs::lookup` が共有 |
| 9 | 完了時動作 | ✓ | `provider/lib.rs::build_client` の `OpenaiResponses` アームが `SchemeNotImplemented` から実装に差し替え済み |

## 設計決定への反映

| 決定 | 反映 |
|---|---|
| `store` / `include[]` を `OpenAIResponsesScheme` フィールドで override 可能、`ModelCapability` には入れない | ✓ `with_store` / `with_include_encrypted_content` ビルダー、デフォルトは stateless + ZDR 相当 |
| `ReasoningSupport::Effort / Both` 対応時のみ `reasoning` 送信 | ✓ `build_request` で capability と `request.config.reasoning.effort` の両方が揃う時のみ投影 |
| Responses 未使用パラメータ (`service_tier` / `prompt_cache_key` / `text.verbosity`) は予約のみ | ✓ `ResponsesRequest` 構造体には入れず、必要時に追加できる構造 |

## アーキテクチャ評価

### 良い点
- **`ensure_and_delta` による防御的設計**: `content_part.added` が欠落しても delta 単独で BlockStart + Delta を発行できる。`delta_without_prior_start_recovers` テストで確認済
- **`OpenAIResponsesState` の 3 種 SlotKey**: `OutputItem` (tool 全体) / `ContentPart { output, content }` (text/reasoning) / `Summary { output, summary }` (reasoning 要約) で Responses の 2 次元座標を flat index に自然にマップ。`parallel_output_items_get_distinct_indices` で並列 tool call の独立性を検証
- **テストの充実**: request 12 ケース (scheme defaults / tool_choice 固定 / role 別 content 型 / reasoning 有無 / round-trip / serialize shape)、events 10 ケース (text / function_call / custom_tool / reasoning_text / summary / 並列 / failed / unknown / 防御)
- **capability の一次情報共有**: `OpenAiFamily` enum を `openai_chat` に置いて両 scheme で共有、DRY と結合度のバランスが良い
- **`Item::Reasoning` 拡張の互換性**: `Vec::is_empty` / `Option::is_none` での skip_serializing、既存 `Item::reasoning()` コンストラクタは空値で互換。他 scheme の `request.rs` は `Item::Reasoning { text, .. }` で `..` 省略しており、追加フィールドで壊れない

## 指摘事項

### 優先度: 低

#### 1. tool 引数 delta 先行時の空メタデータ

`ensure_and_delta` が `function_call_arguments.delta` / `custom_tool_call_input.delta` で BlockStart 未発行の場合、`BlockMetadata::ToolUse { id: String::new(), name: String::new() }` を合成する。実運用では `output_item.added` が先行するはずで発動しないが、仮に発火すると後段で空 `call_id` を使うリスクがある。

対応案:
- `ToolUse` では `ensure_and_delta` を使わず、`output_item.added` 必須で、欠落時は warning ログ + Delta 破棄
- もしくは現状維持で「防御コードとして warning」出す

現状、`output_item.added` が保証されている Responses API の仕様に依存しており、実害は薄い。

#### 2. `tools[].strict: false` ハードコード

`ResponseTool::strict: false` を常時送信。Responses の `strict: true` は JSON Schema 準拠を強制する。`ModelCapability::structured_output == JsonSchema` のときに `strict: true` に昇格させる余地があるが、本チケットではスコープ外として許容範囲。

### 優先度: 極低（構造的なスコープ内）

#### 3. `summary` / `encrypted_content` が他 scheme で落ちる

`Item::Reasoning { text, .. }` で `..` 省略されている他 scheme（Anthropic / OpenAI Chat / Gemini）の `request.rs` は `summary` と `encrypted_content` を送信しない。ただし:
- Anthropic は独自の `signature` が別途必要で将来拡張
- OpenAI Chat と Gemini は reasoning を first-class では送らない
- scheme をまたいだ履歴引き継ぎは現状想定外

構造的には設計通り、問題なし。

#### 4. `OpenAIResponsesScheme` の override が `build_client` から届かない

`with_store` / `with_include_encrypted_content` は存在するが、`build_client` 経由では常に `OpenAIResponsesScheme::new()` デフォルト。ZDR 非対応環境で `store=true` にする場合は provider 側で新しい経路が必要。

チケットの設計課題 1 で「将来対応」と明示されており、スコープ通り。

## 総合判定

**close 可能**。9 要件すべて達成、設計決定も正確に反映、テストも充実。指摘事項はいずれも実害が薄いか、チケット設計課題として明示されたスコープ内の将来対応。

`tickets/llm-auth-codex-oauth.md` (次のチケット) を進められる状態。
