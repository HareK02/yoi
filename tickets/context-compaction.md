# コンテキスト圧縮: Prune + Compact

## 背景

長時間実行エージェントにとって、コンテキストウィンドウの管理はコア要件。
現状の Worker は history をそのまま保持し、オーバーフロー時の対策がない。

2段階のアプローチで対処する:
1. **Prune**: リクエストごとに古いツール出力を削ぎ落とし、コンテキストを節約
2. **Compact**: 閾値超過時に要約を生成し、history 全体を圧縮

---

## Phase 1: Prune

### 概要

`PreLlmRequest` フックとして実装する。リクエストコンテキスト（history のクローン）上で動作し、実際の history は変更しない。セッションログの完全性を保ちつつ、LLM に送るコンテキストを軽量化する。

### コード配置

| 場所 | 内容 |
|------|------|
| `crates/llm-worker/src/prune.rs` | Prune アルゴリズム（純粋関数） |
| `crates/pod/src/prune_hook.rs` | `PruneHook`（`Hook<PreLlmRequest>` 実装） |

アルゴリズムは `Item` を操作する純粋関数なので llm-worker に置く。
フックの配線は Pod 層の責務。

### アルゴリズム

```rust
// crates/llm-worker/src/prune.rs

/// 古いターンのツール出力を刈り込む。
///
/// `items` はリクエストコンテキスト（history のクローン）。
/// 直近 `protected_turns` ターン以内のアイテムは保護される。
pub fn prune(items: &mut Vec<Item>, protected_turns: usize) {
    // 1. ターン境界の特定
    //    UserMessage の出現位置 = ターンの開始点
    let turn_starts: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.is_user_message())
        .map(|(i, _)| i)
        .collect();

    // 2. 保護境界の計算
    //    直近 N ターンの最初の UserMessage のインデックス
    let protection_boundary = if turn_starts.len() <= protected_turns {
        return; // 保護対象以内ならスキップ
    } else {
        turn_starts[turn_starts.len() - protected_turns]
    };

    // 3. 境界より前のアイテムを刈り込み
    for item in items[..protection_boundary].iter_mut() {
        prune_item(item);
    }
}

fn prune_item(item: &mut Item) {
    match item {
        Item::ToolResult { output, .. } => {
            if output == "[pruned]" || output.starts_with("[pruned]") {
                return; // 冪等性: 既に刈り込み済み
            }
            // blob 参照があれば保持し、サマリーだけ除去
            if let Some(blob_ref) = extract_blob_ref(output) {
                *output = format!("[pruned] {blob_ref}");
            } else {
                *output = "[pruned]".to_string();
            }
        }
        Item::Reasoning { text, .. } => {
            *text = "[pruned]".to_string();
        }
        // UserMessage, AssistantMessage, ToolCall は保持
        // （会話の流れとツール呼び出しの意図は残す）
        _ => {}
    }
}

/// "[blob:abc123] summary..." から "[blob:abc123]" を抽出
fn extract_blob_ref(output: &str) -> Option<String> {
    if output.starts_with("[blob:") {
        output.find(']').map(|end| output[..=end].to_string())
    } else {
        None
    }
}
```

### PruneHook

```rust
// crates/pod/src/prune_hook.rs

pub struct PruneHook {
    protected_turns: usize,
}

impl PruneHook {
    pub fn new(protected_turns: usize) -> Self {
        Self { protected_turns }
    }
}

#[async_trait]
impl Hook<PreLlmRequest> for PruneHook {
    async fn call(&self, context: &mut Vec<Item>) -> PreRequestAction {
        prune(context, self.protected_turns);
        PreRequestAction::Continue
    }
}
```

### 特性

- **冪等**: 既に `[pruned]` のアイテムは再処理しない
- **非破壊**: history 本体は変更せず、リクエストコンテキスト（クローン）のみ操作
- **blob 参照保持**: `[pruned] [blob:abc123]` の形式で blob 参照を残す。LLM は `inspect` ツールで必要に応じて内容を取得可能
- **対象**: `ToolResult`（最大の節約源）と `Reasoning`。`ToolCall` の arguments は残す（ツール操作の意図が消えるため）

### KV キャッシュへの影響

`pre_llm_request` はリクエストコンテキスト（クローン）を操作する。プロバイダ側の KV キャッシュは、送信内容が変わった部分で再計算が必要。ただし刈り込み対象は古いアイテムであり、キャッシュヒットしない領域なのでトレードオフとして許容。

---

## Phase 2: Compact

### 概要

Prune がアイテム単位の軽量な刈り込みであるのに対し、Compact は history 全体を要約で置き換える重量級の操作。別の Worker（要約専用・ツールなし）を使って要約を生成し、history を圧縮する。

### トリガー

Controller が `input_tokens` を追跡し、run 完了後に閾値と比較する。

```rust
// controller.rs 内の actor ループ

// 使用量トラッカー（セットアップ時に Worker コールバックに登録）
let last_input_tokens = Arc::new(AtomicU64::new(0));
{
    let tracker = last_input_tokens.clone();
    worker.on_usage(move |event| {
        if let Some(tokens) = event.input_tokens {
            tracker.store(tokens, Ordering::Relaxed);
        }
    });
}

// run 完了後のチェック（actor ループ内）
let input_tokens = last_input_tokens.load(Ordering::Relaxed);
if let Some(threshold) = compact_threshold {
    if input_tokens > threshold {
        // → compaction 実行
    }
}
```

### Compaction フロー

```
Run 完了
  ↓
Controller: input_tokens > threshold?
  ↓ yes
Controller: history 全体を要約プロンプトに変換
  ↓
Controller: 要約用 Worker を生成（ツールなし、専用 system prompt）
  ↓
要約 Worker: 要約テキストを生成
  ↓
Controller: 要約 + 直近 N ターンで新しい history を構築
  ↓
Controller: pod.session_mut().worker_mut().set_history(compacted)
  ↓
Controller: セッションログに Compacted エントリを記録
  ↓
次の run/resume で圧縮済み history を使用
```

### 要約用 Worker

```rust
// controller.rs 内、compaction 実行部分

async fn compact<C, St>(
    pod: &mut Pod<C, St>,
    retained_turns: usize,
) -> Result<(), PodError>
where
    C: LlmClient + 'static,
    St: Store + 'static,
{
    let manifest = pod.manifest().clone();
    let history = pod.session_mut().worker_mut().history().to_vec();

    // 1. 直近 N ターンのアイテムを分離
    let (old_items, recent_items) = split_at_turn_boundary(&history, retained_turns);

    if old_items.is_empty() {
        return Ok(()); // 圧縮対象なし
    }

    // 2. 要約用 Worker を構築
    let client = provider::build_client(&manifest.provider, None)?;
    let mut summary_worker = Worker::new(client);
    summary_worker.set_system_prompt(COMPACTION_SYSTEM_PROMPT);
    summary_worker.set_request_config(
        RequestConfig::new()
            .with_max_tokens(2048)
            .with_temperature(0.0),
    );

    // 3. 会話履歴を要約対象テキストとして入力
    let summary_input = format_history_for_summary(&old_items);
    let locked = summary_worker.lock();
    let output = locked.run(summary_input).await;
    let summary_worker = output.worker.unlock();

    // 4. 要約テキストを取得
    let summary_text = extract_last_assistant_text(summary_worker.history())
        .unwrap_or_else(|| "[compaction failed]".to_string());

    // 5. 新しい history を構築
    let summary_item = Item::user_message(format!(
        "[Compaction Summary — previous conversation condensed]\n\n{summary_text}"
    ));
    let mut compacted = vec![summary_item];
    compacted.extend(recent_items);

    // 6. 適用
    pod.session_mut().worker_mut().set_history(compacted);

    Ok(())
}
```

### 要約フォーマット

要約用 Worker の system prompt:

```
You are a conversation summarizer for an AI coding assistant.

Given a conversation history between a user and an assistant, produce a structured
summary. The summary will replace the conversation history, so include all
information the assistant needs to continue working effectively.

Format:

## Original Task
(The user's original goal or instruction)

## Completed Work
- (Bullet list of what was accomplished, with specific file paths and changes)

## Key Discoveries
- (Important facts, constraints, decisions, or errors encountered)

## Current State
- (What files were modified, what remains to be done)

Be precise about file paths, function names, and technical details.
Omit pleasantries and conversational filler.
```

### 直近ターンの分離

```rust
/// history を「古い部分」と「直近 N ターン」に分割する。
/// ターン境界は UserMessage の出現で判定。
fn split_at_turn_boundary(
    items: &[Item],
    retained_turns: usize,
) -> (Vec<Item>, Vec<Item>) {
    let turn_starts: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.is_user_message())
        .map(|(i, _)| i)
        .collect();

    if turn_starts.len() <= retained_turns {
        return (vec![], items.to_vec()); // 全て保護
    }

    let split_at = turn_starts[turn_starts.len() - retained_turns];
    let old = items[..split_at].to_vec();
    let recent = items[split_at..].to_vec();
    (old, recent)
}
```

### セッションログ

新しい `LogEntry` variant を追加:

```rust
// session_log.rs

pub enum LogEntry {
    // ... existing variants ...

    /// Context compaction: history was replaced with a summary + recent items.
    Compacted {
        ts: u64,
        /// The new compacted history.
        history: Vec<Item>,
    },
}
```

`collect_state` での処理:

```rust
LogEntry::Compacted { history, .. } => {
    state.history = history.clone();
}
```

append-only のログ整合性を維持。圧縮前の全履歴はログの過去エントリに残る。

### Controller の変更

Controller の actor ループに compaction ロジックを追加:

```rust
// controller.rs (actor ループ内、run 完了後)

Method::Run { input } => {
    // ... existing run logic ...

    // Compaction check
    let input_tokens = last_input_tokens.load(Ordering::Relaxed);
    if let Some(threshold) = compaction_config.compact_threshold {
        if input_tokens > threshold {
            info!(input_tokens, threshold, "Triggering context compaction");
            let _ = event_tx.send(Event::CompactionStart);
            match compact(&mut pod, compaction_config.retained_turns).await {
                Ok(()) => {
                    let _ = event_tx.send(Event::CompactionDone);
                    // セッションログに記録
                    // ...
                }
                Err(e) => {
                    warn!(error = %e, "Compaction failed, continuing without");
                }
            }
        }
    }
}
```

### エラーハンドリング

Compaction は best-effort。失敗してもデータは失われない:
- 要約 Worker がエラー → ログに警告を出して続行。次の run 完了後に再試行
- 要約テキストの抽出に失敗 → フォールバック: 古い history をそのまま保持

---

## 設定

### マニフェスト拡張

```toml
[pod]
name = "code-agent"

[provider]
kind = "anthropic"
model = "claude-sonnet-4-20250514"

[worker]
system_prompt = "..."
max_tokens = 8192

[compaction]
# Prune: 直近何ターンを保護するか（デフォルト: 3）
prune_protected_turns = 3

# Compact: input_tokens がこの値を超えたら要約を実行（省略 = 無効）
compact_threshold = 80000

# Compact: 圧縮後に保持するターン数（デフォルト: 2）
compact_retained_turns = 2
```

```rust
// manifest/src/lib.rs

pub struct PodManifest {
    pub pod: PodMeta,
    pub provider: ProviderConfig,
    pub worker: WorkerManifest,
    #[serde(default)]
    pub scope: Option<ScopeConfig>,
    #[serde(default)]
    pub compaction: Option<CompactionConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionConfig {
    #[serde(default = "default_prune_protected_turns")]
    pub prune_protected_turns: usize,  // default: 3
    pub compact_threshold: Option<u64>,
    #[serde(default = "default_compact_retained_turns")]
    pub compact_retained_turns: usize, // default: 2
}
```

### デフォルト動作

- `[compaction]` セクション省略時: Prune も Compact も無効
- `[compaction]` セクションあり・`compact_threshold` 省略時: Prune のみ有効

---

## Protocol 拡張

Compact イベントをクライアントに通知:

```rust
// protocol/src/lib.rs

pub enum Event {
    // ... existing ...
    CompactionStart,
    CompactionDone,
}
```

---

## 設計判断

| 判断 | 理由 |
|------|------|
| Prune は request context（クローン）を操作 | history 本体を保全。セッションログに完全な履歴が残る |
| Compact は run 間で実行（mid-loop ではない） | 要約生成は LLM 呼び出しを伴う重い処理。ターンループ内で中断すると複雑性が増す。Prune がループ内のコンテキスト膨張を抑制するので十分 |
| 要約は UserMessage として挿入 | LLM がコンテキストとして自然に参照できる。system prompt とは分離 |
| `LogEntry::Compacted` で新 history を記録 | append-only チェーンを破らず、`collect_state` で正しく復元可能 |
| Compact 失敗は best-effort | データ喪失リスクをゼロにする。失敗しても次回の run 後に再試行可能 |
| 新しい trait は不要 | 設計原則3: `Hook<PreLlmRequest>` + Controller 制御 + `set_history()` の組み合わせで完結 |

---

## 実装順序

1. **`prune.rs`** — llm-worker にアルゴリズムを追加。単体テスト
2. **`PruneHook`** — pod に Hook 実装。`Pod::add_pre_llm_request_hook` で登録
3. **`CompactionConfig`** — manifest にセクション追加。パースのテスト
4. **`LogEntry::Compacted`** — session_log に variant 追加。`collect_state` テスト
5. **`compact()` 関数** — Controller に compaction ロジック。統合テスト
6. **Protocol** — `CompactionStart` / `CompactionDone` イベント追加

Phase 1（ステップ 1-2）と Phase 2 の準備（ステップ 3-4）は並行可能。

---

## 依存チケット

- ~~[remove-hook-module.md](remove-hook-module.md)~~ — 完了。`PreLlmRequest` は Pod 層の `hook::Hook<PreLlmRequest>` として利用可能
