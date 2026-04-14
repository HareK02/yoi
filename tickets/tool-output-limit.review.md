# レビュー: ツール実行結果のサイズ上限

対象コミット: 未コミット（作業ツリー）
対象差分: `crates/manifest/src/lib.rs`, `crates/llm-worker/src/{lib,tool,worker}.rs`, `crates/pod/src/pod.rs`

## 要件達成状況

| 要件 | 状態 |
|---|---|
| 単一チョークポイントで全ツールに効く | ✅ `worker::execute_tools` 内に配置 |
| マニフェストで設定可能（デフォルト 16KB） | 🚨 省略時にデフォルトが効かない（下記参照） |
| 切り詰めマーカーで LLM が検知できる | ✅ `[truncated: N bytes dropped, refine your query]` |
| `summary` 非改変 | ✅ `ToolResult::content` のみを書き換え |
| `content: None` スキップ | ✅ `as_mut()` の else で continue |
| 未知ツール名を許容 | ✅ `HashMap::get` なので自然にフォールバック |
| UTF-8 安全な切断 | ✅ `is_char_boundary` ループで実装 |
| 切り詰め関数の単体テスト | ✅ ASCII / UTF-8 / 境界 / per_tool |

## 指摘

### 1. 🚨 デフォルト 16KB が `[worker.tool_output]` 省略時に効かない

チケット本文:
> `[worker.tool_output]` セクション自体は省略可能。省略時はデフォルト 16KB が全ツールに適用。

現状の実装:

- `manifest::WorkerManifest.tool_output: Option<ToolOutputLimits>` + `#[serde(default)]`
  → TOML で省略すると `None`
- `pod::apply_worker_manifest`:
  ```rust
  worker.set_tool_output_limits(wm.tool_output.as_ref().map(|l| ...));
  ```
  → `None` がそのまま worker に伝播
- `Worker::execute_tools`:
  ```rust
  if let Some(limits) = self.tool_output_limits.as_ref() { ... }
  ```
  → `None` なら truncate ブロック丸ごとスキップ

結果、マニフェストに `[worker.tool_output]` を書き忘れると truncate 自体が無効化される。事故再発防止が目的のチケットで、**ユーザーの明示的なオプトイン**に縮退しており要件違反。

**判断**: 要修正（必須）。

**修正方針**: `manifest::WorkerManifest.tool_output` を非 Option にして `#[serde(default)]` + `impl Default for ToolOutputLimits` を利用する。Pod 側の `as_ref().map(...)` も `Some(...)` を常に渡す形に直す。

---

### 2. 🟡 truncate が post_tool_call interceptor の **前** に挿入されている

現状の順序:

```
Phase 2: ツール実行
  → truncate
Phase 3: post_tool_call interceptor
  → 履歴に積む
```

この順だと post_tool_call interceptor が切り詰め済みの content しか見られず、ログ・監査・分類などで full content を使いたいケースで情報ロスが起きる。
また interceptor が content を差し替えた場合、その差し替え後の巨大データは truncate を経由せず履歴に入る（防御が迂回される）。

**判断**: 修正する。truncate を Phase 3 の直後、履歴コミット直前に移動する。

**修正方針**: `worker::execute_tools` 内の truncate ループを Phase 3 の for ループ後に移動。それ以外の構造は変えない。

---

### 3. 🟢 `ToolOutputLimits` 型が manifest と llm-worker で二重定義

`manifest::ToolOutputLimits`（serde 付き）と `llm_worker::ToolOutputLimits`（serde なし）が同構造で、`pod::apply_worker_manifest` で手変換している。

manifest クレートが llm-worker に依存しない方針は正しく、他の manifest 型（`CompactionConfig` など）も Pod 境界で類似の変換を行っているため、**既存パターンと整合**している。

**判断**: 不問。現行のまま。

---

### 4. 🟢 極小 limit でのオーバーラン

`truncate_content` は limit が suffix のリザーブ分（約 70 bytes）を下回ると、body_budget が 0 になっても suffix 自体が 50〜70 bytes あるため結果サイズが limit を超える。実運用で limit < 128 を設定するケースは無く、実害は無い。

**判断**: 不問。必要なら将来 `debug_assert!(limit >= 128)` を足す程度で十分。

## 結論

**指摘1・2の修正を条件に受け入れ可**。3・4 はそのまま。
