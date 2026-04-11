# コンテキスト圧縮: Prune + Compact

## 背景

長時間実行エージェントにとって、コンテキストウィンドウの管理はコア要件。
現状の Worker は history をそのまま保持し、オーバーフロー時の対策がない。

Claude Code の3層構造（MicroCompaction / AutoCompact / Full Compact）を参考に、
Insomnia では2層（条件付き Prune + Compact）で対処する。

参考: [docs/ref/claude-code-compaction.md](../docs/ref/claude-code-compaction.md)

---

## 前提: ToolOutput の再設計

Prune の設計は ToolOutput の構造に依存する。
現行の Inline/Stored enum を **summary + content** の2フィールド構造に改める。

詳細: ~~[tool-output-design.md](tool-output-design.md)~~ — **実装済み**

### 構造

```rust
pub struct ToolOutput {
    pub summary: String,          // 1-2行。常に残る
    pub content: Option<String>,  // 詳細。Prune で消える
}
```

```rust
Item::ToolResult {
    call_id: CallId,
    summary: String,
    content: Option<String>,
}
```

### Prune との関係

- summary: Prune 後も残る。「何をしたか」の最低限の情報
- content: Prune 対象。`None` に置換するだけ
- 巨大出力はツール側がファイルに退避し、content に見取り図を置く

### 削除対象

ToolOutput 再設計に伴い、以下を削除:

- `ToolOutput` enum（Inline/Stored）→ struct に置換
- `Content` enum, `auto_summarize`, `ToolOutputProcessor` trait
- `BlobStore` trait, `FsBlobStore`, `BlobOutputProcessor`
- `inspect_tool.rs`（汎用の read_file/grep で代替）
- Worker の `output_processor` フィールド

---

## Phase 1: 条件付き Prune

### 概要

Claude Code の `clear_at_least` パターンに倣い、**削れるトークン量が閾値を超える場合にのみ** Prune を実行する。キャッシュを無駄に壊さない。

### キャッシュの制約

全主要プロバイダ（Anthropic / OpenAI / Gemini）で KV キャッシュはプレフィクスベース。
プレフィクス中のアイテムを変更すると、**変更地点以降が全て再計算**になる。

```
キャッシュ済み: [A, B, C, D, E]
Prune:         [A', B, C, D, E]   ← A の content を消した
再計算:        [A', B, C, D, E]   ← A' 以降すべて
```

Prune で得られるトークン節約 vs キャッシュ再計算コスト。
`min_savings` 閾値で「削る価値がある場合だけ」実行する。

### コード配置

| 場所 | 内容 |
|------|------|
| `crates/llm-worker/src/prune.rs` | Prune アルゴリズム（集計 + 置換） |
| `crates/pod/src/prune_hook.rs` | `PruneHook`（`Hook<PreLlmRequest>` 実装） |

### アルゴリズム

```rust
pub struct PruneConfig {
    /// Prune 対象外とする直近ターン数
    pub protected_turns: usize,
    /// この推定トークン数以上削れる場合にのみ Prune を実行
    pub min_savings: usize,
}

pub fn prune(items: &mut Vec<Item>, config: &PruneConfig) -> bool {
    // 1. ターン境界の特定（UserMessage 出現位置）
    let turn_starts = find_turn_starts(items);
    if turn_starts.len() <= config.protected_turns {
        return false;
    }
    let boundary = turn_starts[turn_starts.len() - config.protected_turns];

    // 2. Prune 可能なトークン数を集計
    let mut total_savings: usize = 0;
    let mut prunable: Vec<usize> = Vec::new();

    for (i, item) in items[..boundary].iter().enumerate() {
        if let Item::ToolResult { content: Some(c), .. } = item {
            total_savings += c.len() / 4; // 粗い推定
            prunable.push(i);
        }
    }

    // 3. 閾値チェック
    if total_savings < config.min_savings {
        return false;
    }

    // 4. Prune: content を None にするだけ
    for &i in &prunable {
        if let Item::ToolResult { content, .. } = &mut items[i] {
            *content = None;
        }
    }
    true
}
```

### PruneHook

```rust
pub struct PruneHook {
    config: PruneConfig,
}

#[async_trait]
impl Hook<PreLlmRequest> for PruneHook {
    async fn call(&self, context: &mut Vec<Item>) -> PreRequestAction {
        prune(context, &self.config);
        PreRequestAction::Continue
    }
}
```

### 特性

- **条件付き**: 集計して閾値を超えた場合のみ実行
- **冪等**: `content: None` のアイテムはスキップ
- **非破壊**: history 本体は変更しない。Prune 状態（どこまで刈ったか）を Pod が保持し、LLM リクエスト構築時に反映する
- **単純**: Prune = `content = None`。blob 参照の解析やサマリ生成は不要

---

## Phase 2: Compact

### 概要

history 全体を要約で置き換える。
別の Worker（要約専用・ツールなし）で要約を生成する。

### トリガー

Controller が `input_tokens` を追跡し、run 完了後に閾値と比較。

```rust
let last_input_tokens = Arc::new(AtomicU64::new(0));
{
    let tracker = last_input_tokens.clone();
    worker.on_usage(move |event| {
        if let Some(tokens) = event.input_tokens {
            tracker.store(tokens, Ordering::Relaxed);
        }
    });
}
```

### サーキットブレーカー

```rust
const MAX_COMPACT_FAILURES: usize = 3;
// 3回連続失敗で compaction を無効化
```

### Compaction フロー

session-store-extraction 後の構造を前提とする。
Pod が Worker を直接保持し、session-store は save/restore の関数群。

```
Run 完了 → input_tokens > threshold
  ↓
Pod: worker.history() + worker.request_config() を読み出す
  ↓
Pod: build_client(&manifest.provider) で要約用 Worker を生成（ツールなし、temperature=0）
  ↓
要約 Worker: history を要約プロンプトとして受け取り、構造化要約を生成
  ↓
Pod: [要約 Item, 直近 N ターン] で新 history を構築
  ↓
Pod: worker.set_history(新 history)
  ↓
Pod: session_store::save_compacted(store, new_id, compacted_from, ...) で新セッション開始
  ↓
旧セッション JSONL はそのまま保全（append-only 原則を維持）
```

```
旧セッション (abc-123):
  [entry0] → [entry1] → ... → [entryN]  ← そのまま残る

新セッション (def-456):
  [SessionStart { history: [要約 + 直近N], compacted_from: (abc-123, entryN.hash) }] → ...
```

### SessionStart の出自フィールド

```rust
LogEntry::SessionStart {
    ts: u64,
    system_prompt: Option<String>,
    config: RequestConfig,
    history: Vec<Item>,
    /// fork 由来の場合、元セッションと分岐点
    forked_from: Option<(SessionId, EntryHash)>,
    /// compact 由来の場合、元セッションと圧縮時点
    compacted_from: Option<(SessionId, EntryHash)>,
}
```

- 通常の新規セッション: 両方 `None`
- fork: `forked_from = Some(...)`
- compact: `compacted_from = Some(...)`
- EntryHash で元セッションのどの時点からの操作かを追跡可能

### 要約用 Worker

- `build_client(&manifest.provider, manifest_dir)` で新しい LlmClient を作る
  - reqwest::Client は内部 Arc。1回きりのリクエストなので新規プールで問題なし
- Pod が `manifest_dir` を保持する必要がある（現状 `from_manifest` では受け取るが保持していない）

### 要約プロンプト

TODO: system prompt の文面、history を文字列化する方法を詰める。

出力フォーマット:

```
## Original Task
（元のユーザー指示）

## Completed Work
- （完了した作業。ファイルパス・関数名等の具体情報）

## Key Discoveries
- （判明した事実・制約・エラー）

## Current State
- （変更されたファイル、残タスク）
```

### エラーハンドリング

- 要約 Worker エラー → 警告ログ、スキップ、consecutive_failures++
- 3回連続失敗 → セッション残りで compaction 無効化
- Thrash loop（compaction 直後に再び閾値超過）→ エラーで停止

---

## 設定

### マニフェスト

```toml
[compaction]
# Prune: 直近何ターンを保護するか（デフォルト: 3）
prune_protected_turns = 3

# Prune: この推定トークン数以上削れる場合にのみ実行（デフォルト: 4096）
prune_min_savings = 4096

# Compact: input_tokens がこの値を超えたら要約を実行（省略 = 無効）
compact_threshold = 80000

# Compact: 圧縮後に保持するターン数（デフォルト: 2）
compact_retained_turns = 2
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionConfig {
    #[serde(default = "default_prune_protected_turns")]
    pub prune_protected_turns: usize,       // default: 3
    #[serde(default = "default_prune_min_savings")]
    pub prune_min_savings: usize,           // default: 4096
    pub compact_threshold: Option<u64>,
    #[serde(default = "default_compact_retained_turns")]
    pub compact_retained_turns: usize,      // default: 2
}
```

### デフォルト動作

- `[compaction]` 省略: Prune も Compact も無効
- `[compaction]` あり・`compact_threshold` 省略: Prune のみ有効

---

## 設計判断

| 判断 | 理由 |
|------|------|
| ToolOutput を summary + content に | Prune が `content = None` で済む。blob/inspect の複雑さが消える |
| BlobStore / inspect を削除 | 巨大出力はツール側の責務。フレームワークは summary/content を受け取るだけ |
| Prune は条件付き（`min_savings`） | KV キャッシュ無効化コスト vs 節約量。Claude Code の `clear_at_least` に倣う |
| Prune は request context を操作 | history 本体を保全。session log の完全性を維持 |
| Compact は run 間で実行 | 要約は LLM 呼び出しを伴う。ターンループ内では Prune が対処 |
| サーキットブレーカー | 連続失敗の無限ループ防止。Claude Code の知見 |
| 新しい trait は不要 | 設計原則3: Hook + Controller 制御 + set_history() で完結 |

---

## 実装順序

1. **ToolOutput 再設計** — enum → struct（summary + content）。Item::ToolResult の変更。単体テスト
2. **旧モジュール削除** — BlobStore, BlobOutputProcessor, inspect_tool, ToolOutputProcessor, Content, auto_summarize。Worker から output_processor 除去
3. **`prune.rs`** — 条件付き Prune アルゴリズム。単体テスト
4. **`PruneHook`** — Pod に Hook 実装
5. **`CompactionConfig`** — manifest にセクション追加
6. **`LogEntry` に provenance フィールド追加** — SessionStart に `compacted_from` / `forked_from`
7. **`compact()` 関数** — Pod に compaction ロジック + サーキットブレーカー
8. **Protocol** — `CompactionStart` / `CompactionDone` イベント追加

ステップ 1-2 は ToolOutput 移行として独立実行可能。
ステップ 3-4（Prune）と 5-6（Compact 準備）は並行可能。
ステップ 5-8 は session-store-extraction 完了後に実装。

---

## 依存チケット

- ~~[remove-hook-module.md](remove-hook-module.md)~~ — 完了
- [session-store-extraction.md](session-store-extraction.md) — ステップ 5-8 の前提
