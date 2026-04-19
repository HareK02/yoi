# Anthropic プロンプトキャッシュの有効化

レビュー中: [anthropic-prompt-cache.review.md](anthropic-prompt-cache.review.md)

## 背景

Anthropic プロバイダ経由のリクエストで prompt caching が一切機能していない。セッションログの `cache_read_tokens` / `cache_write_tokens` が常に 0、`AnthropicScheme::build_request` に `cache_control: ephemeral` の breakpoint が一つも入っていない。

結果：

- 毎ターン system prompt + tools + 全履歴が full-price / full-token で再送信される
- 30k ITPM 帯の組織では、数ターン debug を回しただけで `rate_limit_error` (429) に到達する。実例：`ListPods` → `SpawnPod`（失敗）→ `Glob`（大量出力）→ `Read` のデバッグシーケンスで 429
- 長期セッションのコストが本来の ~10 倍になっている（キャッシュなら cache read は通常 input の ~10%）

Anthropic の自動キャッシュ（モデル・時期依存）の有無に関わらず、明示的 breakpoint は自分で制御できる確実な手段。

## 依存

- なし。`crates/llm-worker/src/llm_client/scheme/anthropic/request.rs` 単独

## 設計

### Breakpoint 戦略

Anthropic の breakpoint は「その位置までの prefix をキャッシュする」ので後方が前方を subsume する。前方 breakpoint は後方キャッシュが TTL で失効した時の fallback として機能する。

安定度の異なる **3 層**に置く：

| 位置 | 前進するタイミング | 主な用途 |
|---|---|---|
| **Prune 位置** | compaction 走行時 | 超長期フォールバック |
| **最後のターン末** | ターン完了時 | 次ターン以降の read |
| **新規リクエスト Head** | 毎 LLM コール（= messages 末尾に追従） | 同ターン内の tool round での read |

### 期待される挙動

**1 ターン内で M 回の tool round（agent loop）:**

- Call 1: 最後のターン末 = 前ターンの終わり → read、新規 Head = Call 1 の messages 末尾に creation
- Call 2: 新規 Head（前 Call の末尾）→ read、新しい Head を messages 末尾に creation
- ...（K 回目も同様）

ターン全体の累積入力コスト: O(K²) → **O(K)** に改善。

**ターン N+1 開始時:**

- 最後のターン末 = ターン N 最終状態 → read
- Head = N+1 最初の Call の末尾に creation

**compaction 走行時:**

- Prune 位置が前進、以降 read

### TTL 耐性

5 分 TTL で最新が失効しても段階的に fallback：

- 新規 Head 失効 → 最後のターン末 で read
- 最後のターン末 失効 → Prune 位置 で read
- Prune 位置 失効 → compaction 境界から re-create

### 4 枠のうち 3 枠使用

残り 1 枠は将来の拡張用（tools 配列を別 TTL で管理したい場合など）に温存。

前方に system 単独 / tools 単独の breakpoint を打つ案は subsume されるだけで意味がないので採用しない。

### 実装方針

`AnthropicContentPart` に `cache_control` フィールドを追加。Anthropic の API 形式：

```json
{ "type": "text", "text": "...", "cache_control": { "type": "ephemeral" } }
```

`Option<CacheControl>` で持ち、値がある場合のみシリアライズ（`skip_serializing_if`）する。既存の非 Anthropic プロバイダ（OpenAI / Gemini / Ollama）には影響させない — Anthropic scheme 内部型のみ。

3 つの breakpoint 位置の決定：

- **Prune 位置**: 既存 compaction 機構（`pod::compact_state` か Pod 内で管理される prune 済みインデックス）から取得。`build_request` 経路で Anthropic scheme に prune インデックスを渡す API 拡張が必要
- **最後のターン末**: Pod / Worker 側が既にターン境界を記録しているならそれを使う。無ければ `messages` 逆走査で「直近 user メッセージの 1 つ前」を探す
- **新規リクエスト Head**: `messages.last()` に付ける（= 現行 request の末尾）

3 箇所が重なる場合（例: 初回 request で Prune 位置・ターン末・Head が全部同じ item）は重複を除去して実質 1 breakpoint にする。

### 自動テスト

- breakpoint が Prune 位置 / 最後のターン末 / 新規リクエスト Head の 3 箇所に付いたリクエスト JSON が生成されること
- Prune 位置が 0（compaction 未走行）のケースでは 2 箇所（ターン末 + Head）のみに付くこと
- ターン末と Head が重なる最終 request（= 新ターンの最初の Call）では 2 箇所に縮退すること
- 3 箇所が全て重なる初回 request では 1 箇所に縮退すること
- OpenAI / Gemini の request 生成が一切変わらないこと（Anthropic 専用だが回帰防止）

## 影響範囲

- `crates/llm-worker/src/llm_client/scheme/anthropic/request.rs`: 内部型 + breakpoint 配置ロジック
- `crates/llm-worker/src/llm_client`: Prune 境界インデックスを `build_request` 経路で渡す API 拡張（`Request` に prune hint を足すか、別経路で scheme に伝える）
- `crates/pod/src/pod.rs` or `compact_state.rs`: 現在の prune 済み件数を読み出せるようにする（既に内部で管理されているはず、公開 API 化）
- 既存の serde round-trip テスト: 追加フィールドを skip_serializing_if で出さないので差分なし

## 完了条件

- Anthropic リクエストの Prune 位置（compact 済みサマリ末尾）に `cache_control: ephemeral` が付く
- Anthropic リクエストの最後のターン末（直近 user メッセージの直前）に `cache_control: ephemeral` が付く
- Anthropic リクエストの新規リクエスト Head（messages 末尾）に `cache_control: ephemeral` が付く
- Prune 位置 / ターン末 / Head が重なる場合は重複除去される
- 実セッションで `cache_read_tokens` が 2 コール目以降に非 0 になる
- 特に同一ターン内の 2 回目以降の LLM コールで直前の tool_result 以前が cache read されること
- 既存の Anthropic / OpenAI / Gemini テストが全 pass
- cache_control が正しい位置に入ることを検証する新規ユニットテスト

## 範囲外

- OpenAI / Gemini の prompt caching（各プロバイダの API 設計が違うため別チケット）
- 動的な breakpoint 数の調整（4 枠目を状況により使い分ける、など）。まずは固定 3 箇所
- Cache hit 率の可観測化（TUI 表示など）。集計は `cache_read_tokens` として既に記録されるので、表示は別途
- Rate limit 429 を受けた際の retry-after honor（別課題）
