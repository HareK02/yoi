# Usage 履歴の永続化

## 背景

LLM レスポンスから得られる Usage は **プロバイダがリクエスト送信時の history prefix
に対して実測したトークン数**であり、手元にある最も正確なトークン情報。

現状は `CompactState::last_input_tokens` に最新の input_tokens だけを `AtomicU64`
で上書き保存しており、過去履歴も prefix との対応関係も失われている。

これを session-store の append-only ログに積めば、ハッシュチェーンに乗った
tamper-evident な実測値の時系列が得られ、履歴上の任意位置の占有量を逆算できる。

## 動機

- ターンより細かい粒度で「履歴のここまでが N トークン」と言いたい
  （長く自走するエージェントは1ターンが長いため、ターン境界では粗すぎる）
- セッションを restore した直後でも、過去の実測値が手元にある状態にしたい
- Compact / Prune の判断、UI のトークン表示、コスト集計、デバッグ用の
  時系列分析、すべての基盤になる

## データモデル

求めているのは **「history のある位置における占有量スナップショット」** であって
LLM 呼び出しの input/output 統計ではない。ただし占有量を実測するための測定値が
そのまま 1 リクエスト = 1 entry の形に乗るので、測定値を素直に保存する。

### `LogEntry::LlmUsage` 追加

```rust
// crates/session-store/src/session_log.rs

LlmUsage {
    ts: u64,
    /// 送信時の history.len()。以下の測定値はこの prefix に対するもの
    history_len: usize,
    /// history[..history_len] をプロバイダが実測した占有量（プロンプト全長）。
    /// このリクエストで新たに追加したトークン数ではなく、折り返しを想定した prefix 全体。
    /// 各プロバイダの正規化:
    ///   - Anthropic: input_tokens + cache_read + cache_creation
    ///   - OpenAI:    prompt_tokens
    ///   - Gemini:    promptTokenCount
    ///   - Ollama:    prompt_eval_count
    input_total_tokens: u64,
    /// 上記のうちキャッシュから読み出された分。料金会計用
    ///   - Anthropic: cache_read_input_tokens
    ///   - OpenAI:    prompt_tokens_details.cached_tokens
    ///   - Gemini:    cachedContentTokenCount
    ///   - Ollama:    0
    cache_read_tokens: u64,
    /// 上記のうちこのリクエストでキャッシュに書かれた分。Anthropic のみ非ゼロ
    cache_write_tokens: u64,
    /// このリクエストで生成された出力トークン数
    output_tokens: u64,
}
```

- ハッシュチェーンに乗る (`HashedEntry` の通常 variant として)
- `collect_state` の replay で `RestoredState.usage_history` に積まれる
- 1 リクエスト = 1 entry。Anthropic は message_start と message_delta の2回 Usage
  を出すが、llm-worker 側で集約して **完了時の最終値だけ** を pod に渡す

### `RestoredState` 拡張

```rust
pub struct RestoredState {
    // ... 既存
    pub usage_history: Vec<UsageRecord>,
}

#[derive(Debug, Clone)]
pub struct UsageRecord {
    pub history_len: usize,
    pub input_total_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
}
```

`collect_state` の replay で `LlmUsage` entry を見たら `usage_history` に push。
他の variant のように `history` を変化させることはない（独立した時系列）。

### `save_usage` 関数

```rust
// crates/session-store/src/session.rs

pub async fn save_usage(
    store: &impl Store,
    session_id: SessionId,
    head_hash: &mut Option<EntryHash>,
    history_len: usize,
    input_total_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    output_tokens: u64,
) -> Result<(), StoreError>;
```

## タイミングと結線

**append のタイミングはリクエスト完了時に 1 回**。送信時には storage に触らない。

### llm-worker 側

- 各プロバイダの scheme で 1 リクエスト内の複数 Usage event（Anthropic の
  message_start + message_delta）を集約し、**完了時の最終値だけを 1 つの
  `UsageEvent` として外に発火する**。pod 側では暫定値を見ない
- 占有量への正規化（Anthropic: `input_tokens + cache_read + cache_creation`）は
  各 scheme の `convert_usage` で行い、`UsageEvent.input_tokens` には正規化済みの
  占有量（プロンプト全長）が入る。consumer 側（pod / UsageTracker）は
  `event.input_tokens` をそのまま使う
  - 動機: 正規化ロジックを scheme に閉じ込めることで、consumer が provider 差異を
    意識する必要がなくなる
- `cache_read_input_tokens` / `cache_creation_input_tokens` は内訳として
  別フィールドに保持。料金会計用

### pod 側

- LLM リクエスト送信**直前**に `history.len()` を捕捉して stash
  （`Arc<Mutex<Option<usize>>>` などで on_usage callback と共有）
- `on_usage` callback で stash された `history_len` と Usage の最終値を組にして
  `save_usage` を呼ぶ
- 既存の `CompactState::update_input_tokens` 経路はそのまま残してよい
  （閾値判定は最新値だけで足りる）。`save_usage` はそれと並列に呼ぶ

結線箇所はおそらく `crates/pod/` 内の既存 `compact_state.rs` の `on_usage`
callback と同じ場所。

## 任意位置のトークン数を割り出せること

このデータがあれば、後段（token-counter）が `history[..M]` の占有量を逆算できる:

各 `UsageRecord` から得られるデータ点:
1. **送信時点**: `(history_len, input_total_tokens)` — 完全な実測
2. **応答完了時点**: `(history_len + k, input_total_tokens + output_tokens)` —
   k は応答で追加された item 数。次の `LlmUsage` の history_len と log 上の
   AssistantItems entry から復元できる

`history[..M]` のトークン数を求める手順:
- 上記のデータ点を時系列に並べる
- M がデータ点と一致 → 実測値そのまま
- 2つのデータ点の間 → 区間内 items のバイト数で按分
- 最後のデータ点より新しい → 最後の rate で外挿

途中で user input や hook injected items が入った分は、次の LlmUsage 時点で
再測定されてキャリブレートされるので誤差は永久に蓄積しない。

詳細アルゴリズムと API は [token-counter.md](token-counter.md) で扱う。

## 設計ポイント

- **最細粒度はリクエスト単位**: provider API がそれ以上細かく返さない。
  ターンより遥かに細かいのでこれで十分
- **append-only + ハッシュチェーン**: 改ざん検知や fork 検出に乗る
- **collect_state の replay コストはほぼゼロ**: usage_history に push するだけで
  history の構築には影響しない
- **既存ログとの互換性**: 古いログには `LlmUsage` entry が存在しないだけ。
  `RestoredState.usage_history` が空になる以外の挙動変化は無い
- **占有量とコストの両立**: input_total と cache 内訳を別フィールドに持つので、
  Compact/Prune 用の占有量も将来のコスト集計も同じ entry から取れる

## 実装対象

- `crates/llm-worker/`
  - 各 provider scheme で 1 リクエスト内の複数 Usage event を集約し、
    完了時に 1 度だけ最終値を発火する仕組み
  - Anthropic の `cache_read_input_tokens` / `cache_creation_input_tokens` を
    `UsageEvent` 経由で正しく外に出せていることの確認（既に出している）
- `crates/session-store/src/session_log.rs`
  - `LogEntry::LlmUsage` variant 追加
  - `UsageRecord` 型追加
  - `RestoredState.usage_history: Vec<UsageRecord>` 追加
  - `collect_state` で `LlmUsage` を replay
- `crates/session-store/src/session.rs`
  - `save_usage` 関数追加
  - `lib.rs` から re-export
- `crates/pod/`
  - LLM リクエスト送信直前に `history.len()` を stash する仕組み
    （`Arc<Mutex<Option<usize>>>` を on_usage callback と共有）
  - `on_usage` callback から `save_usage` 呼び出し
  - provider 別の占有量正規化（Anthropic は cache_read + cache_creation を
    足す）をどこに置くか実装時判断
- テスト
  - `LlmUsage` を含むログの round-trip
  - 複数 entry を replay して `usage_history` が時系列順に積まれる
  - 既存ログ（`LlmUsage` 無し）を読んでも壊れない
  - Anthropic の cache hit ありレスポンスで input_total が正しく計算される

## レビュー状態

Reviewed — [usage-history.review.md](usage-history.review.md)

## 依存

- なし（前提チケット）

## ブロックする後続

- [token-counter.md](token-counter.md) — この履歴を消費して計算する側
- [compact-improvements.md](compact-improvements.md) — 上記経由で依存
