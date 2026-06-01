# 複数ツール動的読み込み機構の設計

## Context

Yoi はエージェントが扱うツール数の増加 (built-in tools + MCP サーバ + ユーザ定義) を想定する必要がある。すべてを upfront に context へ展開すると以下が問題になる:

- **入力トークン消費**: 30-50 ツールで 10-20K tokens を消費しうる (Anthropic 公式ガイド)
- **ツール選択精度の低下**: 数十個を超えるとモデルの tool selection accuracy が落ちる
- **KV cache 効率**: ローカル推論では prefill コストが重く、prefix が動くと再計算が走る

Claude Code が採用する deferred tools 機構 (`docs/ref/claude-code-deferred-tools.md`) と OpenAI Harmony のアプローチ (`docs/ref/tool_approach_comparison.md`) を比較すると、**全モデルで同じ deferred 方式は通用しない**。モデルファミリごとに戦略を切り替えられる抽象が必要。

## 決定事項

### 二層分離: Registry / ContextRenderer

ツール抽象を以下の二層に分ける。両者の責務を厳密に切り離す。

| 層 | 責務 | 単一の真実 |
|---|---|---|
| Registry | ツール実装・schema・名前解決・引数バリデーション | 全ツール常時登録 |
| ContextRenderer | モデルへ渡す prompt にどの tool 定義を、どの形式で、どこに置くか | モデル戦略ごとに差し替え |

**重要**: バリデーションは **Registry 側で実引数 vs 登録 schema の照合**だけで行う。「context にスキーマテキストが現れているか」は検証条件にしない (Claude Code 実演で確認済み: `claude-code-deferred-tools.md` §10)。これにより、ContextRenderer がどんな戦略で schema を見せていようと、registry の真実性が一本化される。

### 戦略 (RenderStrategy) のモデル系統別マッピング

| モデル系統 | 戦略 | 根拠 |
|---|---|---|
| Claude 系 (Anthropic API + ローカル Anthropic 互換) | **Deferred** + ToolSearch 相当 | XML+JSON のテキスト表現で tool_result からも schema 注入可。prefix 安定 |
| OpenAI 系 (gpt-oss / Harmony / Responses) | **Upfront** + MCP-style dispatcher | namespace ブロックが構造化されており、後追い注入が訓練分布外。汎用 dispatcher で外部解決 |
| Hermes / Qwen / Llama 等独自系 | **Upfront** または **Rolling Developer Message** | モデル個別 chat template に従い、必要なら境界で書き換え |

### 戦略を決める軸

`RenderStrategy` は以下の組み合わせで表現:

- **配置**: `SystemPrompt` (固定) / `DeveloperMessage` (cache 境界) / `ToolResultStream` (Claude 流注入)
- **発見手段**: `AlwaysVisible` / `MetaTool { search, describe }` / `Static`
- **変更時の cache 影響**: `PrefixStable` / `RewindToBoundary`

Claude 系 = `(ToolResultStream, MetaTool, PrefixStable)`、OpenAI 系 = `(DeveloperMessage, Static, RewindToBoundary)` または `(SystemPrompt + dispatcher, AlwaysVisible, PrefixStable)`。

## 設計詳細

### Registry インターフェース

```rust
pub trait ToolRegistry {
    fn list(&self) -> Vec<ToolMeta>;            // 名前+description のみ
    fn schema(&self, name: &str) -> Option<&ToolSchema>;
    fn dispatch(&self, name: &str, args: Value) -> Result<ToolResult, DispatchError>;
}
```

- `list()` は常に全ツール返す (戦略は ContextRenderer 側の責務)
- `schema()` は ToolSearch 相当の動線で使用
- `dispatch()` は schema 照合+実行。**context に schema text があるかは見ない**

### ContextRenderer インターフェース

```rust
pub trait ContextRenderer {
    fn initial_render(&self, registry: &dyn ToolRegistry) -> InitialContext;
    fn on_tool_load(&self, name: &str, registry: &dyn ToolRegistry) -> Option<ContextDelta>;
    fn parse_call(&self, raw_output: &str) -> Result<ToolCall, ParseError>;
    fn format_result(&self, name: &str, result: &ToolResult) -> String;
}
```

戦略ごとに実装を差し替える:

- `ClaudeDeferredRenderer`: 初期 prompt に core tools のみ展開、`tool_search` メタツールを常設、ロード時は tool_result として `<function>{schema}</function>` を流す
- `HarmonyUpfrontRenderer`: developer メッセージに namespace で全 tool 展開、ロード概念なし
- `HarmonyDispatcherRenderer`: namespace は `call_mcp(server, tool, args)` だけ、サブツール解決は外部 MCP
- `RollingDeveloperRenderer`: 一定境界 (compaction 等) で developer メッセージを再描画。cache 損失は境界で吸収

### Validation / Retry レイヤ

ツール呼び出しの失敗ハンドリングは ContextRenderer / Registry の上に置く独立層:

```rust
pub struct ToolDispatcher<R: ToolRegistry, C: ContextRenderer> {
    registry: R,
    renderer: C,
    retry_policy: RetryPolicy,
}
```

責務:

1. モデル出力をパース (`renderer.parse_call`)
2. registry で schema 照合 → invalid なら error tool_result を返す
3. dispatch → 結果を `renderer.format_result` で整形してモデルへ
4. malformed 出力時は error フィードバックして同一ターン内修正を促す

これは `tool_approach_comparison.md` §4 で議論した「プロバイダ側がやっている (フォーマット規約 / バリデーション / リトライ / 訓練投資)」のうち、**ローカルモデル向けには (4) が効かないため (1)-(3) を自前で組む**ことに対応する。

### KV cache / prompt cache 整合

戦略の選択は cache 効率に直結する:

- **PrefixStable 戦略 (Claude Deferred / Dispatcher パターン)**: 初期 prefix が固定。ToolSearch 結果や dispatcher 経由の動的解決は **会話末尾の tool_result** に積まれるため、前方プレフィックスが揺らがない
- **RewindToBoundary 戦略 (Rolling Developer)**: tool セット変更が cache 全消し。compaction 境界に同期させて損失を抑える

Anthropic API の `prompt caching` は Explicit (cache_control) で、`llm_providers.md` §Prompt caching の `CacheStrategy::Explicit { max_breakpoints }` と整合する。ローカル推論の KV cache は基本 prefix-only のため `Auto` 相当。両方とも「安定 prefix 設計」に効く。

## 根拠

- **Registry vs Context 分離**: Claude Code の実演で「context に schema があるかは validation に無関係」と判明 (`claude-code-deferred-tools.md` §10)。同じ抽象で Claude / OpenAI / ローカル系を統一できる
- **戦略の差し替え可能性**: Harmony は構造化トークン+namespace 前提で、Claude の deferred 方式が直接通用しない (`tool_approach_comparison.md` §1, §2)。モデル系統ごとの戦略切り替えは避けられない
- **MCP-style dispatcher**: OpenAI が MCP 統合で採用している方向。namespace に汎用 entry point だけ置き、サブツール解決を外部化する。upfront にせず、訓練分布も逸脱しない
- **検証レイヤを別層に**: モデル側のフォーマットの強さ (Harmony の特殊トークン) と弱さ (Claude の正規表現パース) で fail mode が違うため、ContextRenderer に閉じ込めずに上位で統一的に扱う

## 実装原則

- **registry は llm-worker の上位層に置く** (低レベル基盤に留める方針: `feedback_llm_worker_scope.md`)
- **MCP サーバ統合は registry のバックエンドの一つ**として扱い、Harmony 側 dispatcher と内部実装を共有
- **ContextRenderer の選択は ProviderScheme と一対多**: `scheme/anthropic` → ClaudeDeferred、`scheme/openai_responses` → HarmonyUpfront 等。`llm_providers.md` のプロバイダカタログにレンダラ指定を載せる
- **ToolSearch 相当は MetaTool として実装**: registry 側に `__tool_search` / `__tool_describe` を登録し、ContextRenderer の戦略によって core tools に含めるか否かを決める
- **テスト**: registry のバリデーションが context schema 有無に依存しないことを property test で保証する

## Scope 外

- 個別の MCP プロトコル実装 / サーバ連携の詳細
- 各モデル固有の chat template レンダリング (Hermes / Qwen / Llama 等の差分は別 ticket)
- Tool 結果の構造化出力 (citation / file reference 等) のスキーマ
- Tool 並列実行・依存解決・cancellation
- ユーザ定義ツールの permission 管理 (sandbox.md と別)

## 参考

- `docs/ref/claude-code-deferred-tools.md` — Claude Code の deferred tools 機構と実演による検証
- `docs/ref/tool_approach_comparison.md` — Anthropic / OpenAI のツール呼び出しアプローチ比較
- `docs/plan/llm_providers.md` — プロバイダ抽象とスキーム / capability 設計
