# レビュー: Hook と Interceptor の責務分離

対象差分: `crates/pod/src/{hook,pod,lib,pod_interceptor}.rs`、削除: `compact_interceptor.rs` / `hook_interceptor.rs`（staged、未コミット）

## 要件達成状況

| 要件 | 状態 |
|---|---|
| Hook の Input が read-only サマリ型になる | ✅ `PromptSubmitInfo` / `PreRequestInfo` / `ToolCallSummary` / `ToolResultSummary` / `TurnEndInfo` / `AbortInfo` を新設。`Hook::call(&self, input: &E::Input)` で `&mut` が消えた |
| Hook から context / history の直接操作パスが型レベルで存在しない | ✅ `Vec<Item>` / `ToolCallInfo` / `ToolResultInfo` が Hook の Input から消え、サマリ型に置換 |
| Interceptor は context / history の直接操作が可能なまま | ✅ `PodInterceptor` が `Interceptor` を実装し `&mut Vec<Item>` / `&mut ToolCallInfo` 等を受け取る |
| Interceptor → Hook の実行順序（内部が先、公開が後） | ✅ `PodInterceptor` の各メソッドが: 内部ロジック（compaction check 等）→ サマリ構築 → Hook 呼び出し の順 |
| HookInterceptor ブリッジの削除 | ✅ `hook_interceptor.rs` deleted |
| CompactInterceptor の統合 | ✅ `compact_interceptor.rs` deleted。compaction check は `PodInterceptor::pre_llm_request` に統合 |
| 既存の compaction 動作が壊れない | ✅ `compact_state` を `PodInterceptor` に渡し、`exceeds_turn()` で Yield する同じロジック |
| 単体テストで Hook が context を操作できないことが検証される | ✅ テストの Hook が `&PreRequestInfo`（read-only）を受け取る形で書かれている。`&mut` パスは型レベルで不可能 |

## アーキテクチャ

### 良い点

**`PodInterceptor` が single composite interceptor として全責務を統合**:
- compaction check (内部) → サマリ構築 → Hook 呼び出しを1つの `pre_llm_request` 内で制御
- Worker 側は単一の `set_interceptor` で完結（`Vec<Box<dyn Interceptor>>` 不要）
- `compact_interceptor.rs` の decorator パターン（inner HookInterceptor をラップ）が消え、フラットな構造に

**Hook の `call` シグネチャが `&self, input: &E::Input` に**:
- `&mut E::Input` → `&E::Input` への変更で、Hook 実装者が context を書き換える経路が型レベルで消えた
- 将来スクリプトに公開するとき、`&T` だけ渡せば良い（sandbox の粒度が明確）

**サマリ型の設計**:
- `ToolCallSummary::arguments` が `serde_json::Value` clone — 構造的アクセスが可能で permission 判断に使える
- `ToolResultSummary::output` が `ToolOutput` clone — summary + content 両方にアクセス可能
- `PreRequestInfo` が item_count / estimated_tokens / turn_index / tool_calls_this_turn を集約 — compaction 判断に必要十分
- `TurnEndInfo::final_text_preview` が 512 byte 制限 + UTF-8 boundary — Pod orchestration で spawned Pod の結果要約に使える

**ターン追跡の統合**:
- `next_turn_index` / `tool_calls_this_turn` が `PodInterceptor` 内の `AtomicUsize` で管理
- `on_prompt_submit` でリセット、`pre_tool_call` でインクリメント
- Hook が受け取るサマリにこの情報が反映される

### テスト

4 ケース:
- `pre_llm_request_yields_and_skips_hooks_when_compact_threshold_exceeded` — 内部機構（compaction）が Hook より先に short-circuit することを検証
- `pre_llm_request_runs_hooks_when_under_threshold` — 通常時は Hook が走ることを検証
- `pre_llm_request_runs_hooks_when_no_compact_state` — compaction 無効時も Hook が走ることを検証
- `pre_llm_request_short_circuits_on_first_non_continue` — 複数 Hook の短絡評価を検証

特に 1 つ目が**設計の核心（Interceptor が先、Hook が後。内部が short-circuit したら Hook は走らない）**を lock-in している。

## 指摘事項

### 1. 🟢 `pod.rs` の `ensure_interceptor_installed` が少しすっきりした

旧: `CompactInterceptor::new(hook_interceptor, state)` で decorator をネスト → 新: `PodInterceptor::new(registry, compact_state)` でフラットに構築。`compact_state` の有無で分岐していた `set_interceptor` 呼び出しが1箇所に統合。

### 2. 🟢 `HookEventKind::Input` に `Send + Sync` バウンドが追加

```rust
type Input: Send + Sync;
```

`&E::Input` を async メソッドで受け取るために必要。正しい追加。

### 3. 🟢 `extract_message_text` / `preview` がユーティリティとして分離

`PodInterceptor` 内の private function として切り出し。`preview` は UTF-8 boundary を考慮した切断で、`truncate_content`（tool output truncation）と同じパターン。

## 結論

**無条件で受け入れ可**。チケットの要件を完全に達成。特に「型レベルで Hook から context 操作が不可能」という核心的な保証が `&E::Input` (not `&mut`) + サマリ型で実現されている。テストも設計意図を正確にカバー。
