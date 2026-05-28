# コンテキスト圧縮 (Compaction)

長時間実行エージェントのコンテキストウィンドウ管理。
Prune（段階的な出力除去）と Compact（履歴の要約置換）の2層で対処する。

---

## 全体像

```
[各リクエストの合間]
LLM リクエスト
  ↓
PruneHook (pre_llm_request)
  → 古い ToolResult の content を除去（summary は残す）
  → リクエストコンテキストのみ操作、history 本体は不変
  ↓
CompactInterceptor (pre_llm_request)  ← safety net
  → input_tokens > request_threshold なら Yield
  → Worker が WorkerResult::Yielded で正常終了
  ↓
Pod::handle_worker_result
  → persist_turn（旧セッションに記録）
  → compact() → resume()

[ターンの合間 — 次の Pod::run 冒頭]
Pod::try_pre_run_compact  ← proactive
  → input_tokens > post_run_threshold なら compact() (best-effort)
```

---

## Prune

古い ToolResult の `content` を `None` に置換し、`summary` だけ残す。

### 設計判断

- **条件付き実行**: 推定トークン節約量が `min_savings` を超えた場合のみ。KV キャッシュの無駄な無効化を避ける
- **リクエストコンテキストのみ操作**: history 本体は変更しない。Prune 状態を Pod が保持し、LLM リクエスト構築時に反映する
- **保護境界**: 直近 `prune_protected_tokens` 相当の suffix は残す。turn 数ではなく usage history 由来の token estimate で境界を引くため、単発の長い tool loop でも古い `ToolResult.content` が候補になる
- **冪等**: `content: None` のアイテムはスキップ

### ToolOutput の構造

```rust
pub struct ToolOutput {
    pub summary: String,          // 常に残る。「何をしたか」の最低限の情報
    pub content: Option<String>,  // Prune 対象。None に置換するだけ
}
```

`Tool::execute()` は `Result<ToolOutput, ToolError>` を返す。
`From<String>` で自動変換可能（小さい出力は summary のみ、大きい出力は summary + content）。
巨大な出力はフレームワークの責務外。ツール側がファイルに退避し、content に見取り図を置く。

---

## Compact

### 用語

- **run = turn**: 同じ概念。1 ユーザープロンプト → 完了までの単位
- **リクエスト**: 1 turn 内で投げる個別の LLM 呼び出し。ツール使用で 1 turn に複数リクエストが発生する
- **リクエストの合間**: 1 turn 内、次の LLM リクエストを投げる前の地点
- **ターンの合間**: turn が完了して次の turn を待つ状態

### トリガー（2段階の閾値）

1. **ターンの合間 (次 turn 冒頭)**: `try_pre_run_compact()` で `input_tokens > post_run_threshold` → best-effort
2. **リクエストの合間 (CompactInterceptor)**: `pre_llm_request` で `input_tokens > request_threshold` → `PreRequestAction::Yield`

**ターンの合間が proactive (小さい閾値)**:
turn が完了した地点はタスクの自然な区切り。ここで先を見越して早めに compact する。
マニフェストの `threshold` が対応。

**リクエストの合間は safety net (大きい閾値)**:
turn 内部でリクエストの合間にチェックするのは「暴走的に膨張した場合のみ止める」用途。
マニフェストの `request_threshold` が対応。通常は発動しない。

**両閾値は manifest で個別指定する**。過去の設計では 9/8 倍で自動導出していたが、
比率に根拠がなかったため廃止。両方が `Option<u64>` で、片方だけの設定も可能
（そちら側のチェックだけが有効になる）。

### Yield の仕組み

`PreRequestAction::Yield` は Worker のターンループを正常終了させる汎用プリミティブ。
Worker は Compact を知らない。「Interceptor が yield と言ったので抜ける」だけ。
Pod が Yielded を検知して compact → resume する。

```
PreRequestAction::Yield → WorkerResult::Yielded → Pod が compact → resume
PreRequestAction::Cancel → WorkerError::Aborted → エラー
```

### 安全機構

- **サーキットブレーカー**: 3回連続 compact 失敗で無効化 (`CompactState::disabled`)
- **Thrash 検出**: compact 直後に再び閾値超過 → `PodError::CompactThrash`
- **Yield 前の永続化**: `persist_turn` を compact の前に実行。失敗しても旧セッションにデータが残る

### セッション管理

compact は fork と同じ構造。旧セッションを保全し、新 SessionId で圧縮後のセッションを開始。

```
旧セッション (abc-123):
  [...entries...] → Outcome::Yielded (interrupted=true)  ← そのまま残る

新セッション (def-456):
  [SessionStart { compacted_from: (abc-123, entryN.hash), history: [要約 + 直近] }] → ...
```

`SessionStart` に `forked_from` / `compacted_from` フィールドで出自を追跡可能。

---

## compact 後の history 構造

全て system message（`Item::Message { role: System }`）として注入。

```
[system prompt]                              ← 不変
[system: 構造化要約]                          ← compact worker の出力
[system: auto-read ファイル群]                ← read_required の結果
[system: リファレンス一覧]                     ← reference の結果
[直近 N トークン分の生の会話]                  ← pruned 状態で保持
```

### 直近の保護: トークンベース

ターン単位ではなくトークン数ベースで直近の会話を保護する。

ターン単位の問題: 自走エージェントは1ターン内で多数のリクエストを回す。
1ターンが膨大に長くなり、保護量がターン長に依存してしまう。
トークン数ベースなら、ターンの長さに関わらず一定量の直近コンテキストが保持される。

```toml
[compaction]
threshold = 80000                  # ターンの合間 (proactive)
request_threshold = 90000          # リクエストの合間 (safety net)
prune_protected_tokens = 8000      # prune から保護する末尾 token budget
retained_tokens = 8000             # compact 後に生のまま残す末尾 token budget

overview_target_tokens = 8000      # compact worker 初期 overview の通常目標
overview_warning_tokens = 16000    # 超えたら警告・trace、compact は続行
overview_deadline_tokens = 40000   # 超えたら粗い overview へ fallback

worker_context_max_tokens = 50000          # compact worker session 全体の hard limit
finish_warning_remaining_tokens = 8000     # 残りが少ないため write_summary へ進める勧告
final_reserve_tokens = 4000                # 最終 summary/closing turn 用 reserve
worker_max_turns = 20                      # compact worker 自身の tool loop 上限

summary_target_tokens = 1500       # write_summary の目標サイズ
summary_max_tokens = 3000          # write_summary の hard validation
auto_read_budget_tokens = 8000     # compact 後に注入する file content 合計上限
result_context_max_tokens = 24000  # 新 session 初期 context の dry-run validation
```

`compact_*` prefix の旧 key は互換 alias として読み取るが、`[compaction]` 内の新規 key は prefix なしを正とする。
初期 overview の target/warning は効率のための目安で、通常は hard error にしない。deadline 超過時も、可能なら deterministic に粗い overview へ fallback して compact の完走を優先する。

### Auto-Read とリファレンス

2段階のファイル参照:

- **Read** (`mark_read_required`): 全文/範囲指定でコンテキストに注入
- **Reference** (`add_reference`): 「読んだことがある」とだけ伝える。必要なら再読要求

auto-read の system message:
```
[Auto-read file: src/main.rs:42-142]
fn main() {
    let config = Config::load();
    ...
}
```

リファレンスの system message:
```
[Referenced files — read before compaction, contents not included]
- src/config.rs (read during task setup)
- tests/integration_test.rs (read during test implementation)
Use read_file to access current contents if needed.
```

auto-read も通常の history 内 system message なので、将来の Prune/Compact 対象になる。

---

## compact worker

要約生成とファイル選定を行う使い捨て Worker。Pod は compact 対象 prefix を全文投入せず、User / Assistant / System を優先した bounded overview と tool index を初期 input として渡す。Tool call arguments、tool result full content、reasoning body は初期 input には載せない。

初期 overview は `overview_target_tokens` を目標にする。`overview_warning_tokens` を超えた場合は警告・trace を記録して続行し、`overview_deadline_tokens` を超えた場合は粗い deterministic overview へ fallback する。Compact の目的は完走なので、初期 input が少し大きいだけでは hard error にしない。

### ツール

```
read_file(path, offset?, limit?)          — ファイルを読んで判断する
mark_read_required(path, offset?, limit?) — auto-read 対象として指定
add_reference(path)                       — リファレンスとして追加
write_summary(text)                       — 構造化要約を出力/上書き（最後の呼び出しが採用）
```

### フロー

1. Pod が `tools::Tracker::recent_files(5)` で最近触られたファイルを抽出（デフォルトリファレンス）
2. compact worker にプロンプトとして渡す:
   - bounded overview / index（User / Assistant / System 優先）
   - デフォルトリファレンスの一覧
   - TaskStore snapshot
3. compact worker が自律的に:
   - read_file で各ファイルを読み、必要性を判断
   - mark_read_required / add_reference で指定
   - write_summary で構造化要約を出力（呼び直し可）
4. CompactWorkerInterceptor が worker session 全体の context occupancy を監視する:
   - `finish_warning_remaining_tokens` 到達時に「探索を切り上げて write_summary へ進め」と Worker history に永続化される warning を挿入し、人間向け warning も出す
   - `final_reserve_tokens` を割った後は `write_summary` 以外の探索 tool call に synthetic error を返し、最終 summary の余白を守る
   - `worker_context_max_tokens` 超過は最後の hard stop
5. ターン終了時に write_summary 未呼び出し or read_required 空（かつファイル操作履歴がある場合）→ 追加プロンプトで促す
6. `summary_max_tokens` と `result_context_max_tokens` で compact 結果を検証してから新 session を作る

### 構造化要約の要件

**目的**: auto-read でコードは別途載せるので、要約は意思決定・コンテキスト・方向性の記録に特化する。

**含めるべき内容:**
1. 何を、なぜやったか — 意思決定の記録。具体的な型名・関数名で言及
2. ユーザーの指示・フィードバックの原文 — ニュアンス保持。重要なもののみ
3. 発生した問題と解決策 — 同じ轍を踏まない
4. 今どこにいて次に何をするか — compact 前後の一貫性

**含めないもの:**
- コードの全文（auto-read が担う）
- 変更の diff（git がある）
- 中間のやりとりの詳細（最終結論だけ）

### 要約フォーマット（5セクション、1000-2000 トークン目安）

```
## Completed Tasks
### (タスク名)
- 完了した作業（具体的な型名・ファイル名で）
- 注意点 / 発覚した事実

## Active Task
### (タスク名)
- 目標
- 現状（何が済んで何が未着手か）
- 次のステップ

## Key Decisions
- (判断内容) — (理由)

## User Directives
- 「（ユーザー発言の原文）」 — 重要な指示・フィードバックのみ

## Current Work
（直前に何をしていたか。2-3行）
```

セッション中に複数タスクが切り替わっている前提で設計。
完了タスクは簡潔に（注意点・事実のみ）、進行中タスクは十分な詳細で。

---

## 設計の背景

### Claude Code からの知見

Claude Code の compaction 実装（`docs/ref/claude-code-compaction.md`）を参考に、以下を取り入れた:

- **条件付き Prune** (`min_savings`): KV キャッシュ無効化コストとのトレードオフ。`clear_at_least` パターン
- **サーキットブレーカー**: 連続失敗の無限ループ防止
- **2段階のファイル参照** (Read vs Referenced): Claude Code は compact 後に「Referenced file」（内容省略）と「Read」（内容保持）を区別する。サイズと必要性で判断
- **要約の具体性**: 型名・関数名・ユーザー発言原文を保持。抽象的な要約は役に立たない

### 設計原則との対応

- Prune は Worker 層の拡張。新しい trait は不要（設計原則3）
- Compact は Pod 層の制御。Worker は Yield を知るが Compact は知らない
- CompactInterceptor は Decorator パターンで HookInterceptor をラップ。既存の Hook 機構を壊さない
