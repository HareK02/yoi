# Compact の改善

## 背景

`Pod::compact()` とその周辺機構（CompactInterceptor, CompactState, Controller 統合）は
実装済み。挙動の詳細に未決定事項が残っており、要約品質にも改善余地がある。

---

## 1. 要約入力の改善

### 現状の問題

`build_summary_prompt()` が全 Item をフラットにテキスト化して LLM に投げている。

1. **データが多すぎる**: ToolResult の content（ファイル内容、grep 結果等）を含めている
2. **単一関心事の前提**: "Original Task" が1つだけ。タスク切り替わりに対応できない

### Phase 1: 入力データの削減

`build_summary_prompt` で渡すデータを絞る:

```rust
fn build_summary_prompt(items: &[Item]) -> String {
    for item in items {
        match item {
            Item::ToolResult { summary, .. } => {
                // content を含めない。summary だけ
                lines.push(format!("[ToolResult] {summary}"));
            }
            Item::ToolCall { name, .. } => {
                // arguments を含めない。ツール名だけ
                lines.push(format!("[ToolCall] {name}"));
            }
            Item::Reasoning { .. } => {
                // skip（内部思考は要約に不要）
            }
            // User/Assistant のテキストはそのまま
        }
    }
}
```

### Phase 2: 要約フォーマットの改善

タスク切り替わりを反映する:

```
## Tasks
### Task 1: (最初のユーザー指示)
- 完了した作業
- 判明した事実

### Task 2: (次のユーザー指示)
- 完了した作業
- 判明した事実

## Current State
- (変更されたファイル、残タスク)
```

### Phase 3: マルチターン要約 Worker

1リクエストで全部読ませるのではなく、要約 Worker にツールを持たせて自律的に要約させる。

```
要約 Worker:
  system: 「セッションログを読んで構造化要約を生成せよ」
  ツール: read_session_segment(offset, limit)
```

利点:
- 巨大セッションでもコンテキストに収まる
- Worker が自分で「重要/不要」を判断できる
- タスク切り替わりを検出し、関心事ごとに要約できる

builtin-tools チケットとの依存あり。

---

## 2. 挙動の未決定事項

### 現在の挙動

**トリガー（2段階）:**
1. ターン間 (CompactInterceptor): `input_tokens > turn_threshold` → Yield → compact + resume
2. run 後 (Controller): `input_tokens > post_run_threshold (×9/8)` → best-effort compact

**安全機構:**
- サーキットブレーカー: 3回連続失敗で無効化
- Thrash 検出: compact 直後に再び閾値超過 → CompactThrash エラー
- Yield 前の永続化: persist_turn を compact の前に実行

### 2-1. Yield のタイミング精度

現状: `pre_llm_request` でチェック = ターンの切れ目でしか発火しない。
1ターン内でツール呼び出しが多く、途中でコンテキストが膨らむケースは次のターンまで待つ。

検討:
- ツール実行後にもチェックする？（`post_tool_call` で Yield 相当の処理）
- 現状の「ターン切れ目のみ」で十分か？

### 2-2. 閾値の導出

- `turn_threshold` = マニフェストの `compact_threshold` そのまま
- `post_run_threshold` = `turn_threshold * 9 / 8`（≈ 112.5%）

9/8 の根拠はない（安全マージン）。マニフェストで個別指定可能にする？

### 2-3. Prune と Compact の相互作用

```
pre_llm_request:
  1. PruneHook（content を除去）
  2. CompactInterceptor（トークン数チェック）
```

Prune はリクエストコンテキストのみ操作し、`last_input_tokens` は前回の LLM レスポンスの値。
Prune の効果は `last_input_tokens` に反映されず、Compact の判断には影響しない。
→ Prune で十分に縮んでも Compact が走る可能性がある。保守的で実害は小さい。

### 2-4. compact 中のクライアント通知

compact は LLM 呼び出しを伴う。この間 Controller は Pod を占有。
`AlreadyRunning` エラーで弾かれる。Protocol チケットの `CompactStart`/`CompactDone` で対応。

### 2-5. 復元時の挙動

`Outcome::Yielded` で記録されたセッションを restore した場合:
- `last_run_interrupted = true` で復元
- Pod は resume 可能（通常の interrupted セッションと同じ）
- compact 後の新セッションが存在する場合、どちらを restore するかは呼び出し側の責任
- `compacted_from` で辿れる

---

## 実装順序

1. Phase 1（content/arguments/reasoning 除去）→ `build_summary_prompt` の変更のみ
2. 挙動の未決定事項 → 実運用でのフィードバックを元に判断
3. Phase 2（フォーマット改善）→ チューニング
4. Phase 3（マルチターン要約）→ builtin-tools 後
