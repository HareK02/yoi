# rig 参照設計の導入実装方針

## 目的

`llm_worker_rs` の既存方針（Timeline中心、Worker主導）を維持しながら、`rig` 由来で有効だった設計を段階導入する。

対象は以下の5項目。

1. Tool 実行を `ToolServer` 的に分離する
2. Hook をストリーミング粒度まで拡張する
3. Typed output（構造化出力）の第一級 API 化
4. Provider 固有最適化を provider 層で吸収する
5. GenAI telemetry を標準実装にする

---

## 1. Tool 実行を `ToolServer` 的に分離する ✅

### 目的

Worker から「ツール登録・呼び出し・定義解決・動的選択」の責務を分離し、Worker は turn orchestration と hook 制御に集中させる。

### 実装状況: ✅ 完了

- `tool_server.rs` に `ToolServer` / `ToolServerHandle` を実装済み。
- `Worker` は `ToolServerHandle` を保持し、直接 `Tool` マップを持たない設計に移行済み。
- `Worker::register_tool()` は内部で `ToolServerHandle::register_tool()` を呼ぶ形に統一済み。
- ツール実行は `ToolServerHandle::call_tool(name, args)` 経由に統一済み。
- `tool_definitions_sorted()` で決定的順序のツール定義送信を実装済み。
- 単体テスト（重複登録・未登録呼び出し・ソート順）を `tool_server.rs` 内に実装済み。

### 計画との差分

- `ToolExecutor` trait 抽象は未導入。現状 `ToolServerHandle` が具象型として直接使われている。remote tool / MCP 対応時に trait 化する余地あり。

---

## 2. Hook をストリーミング粒度まで拡張する ✅

### 目的

Text/Tool delta を受けた時点でフック介入できるようにし、監査・UI 連携・早期停止を改善する。

### 実装状況: ✅ 完了

- `hook.rs` に以下の streaming 系イベント種別を追加済み:
  - `OnTextDelta` (入力: `TextDeltaContext`)
  - `OnToolCallDelta` (入力: `ToolCallDeltaContext`)
  - `OnStreamChunk` (入力: `StreamChunkContext`)
  - `OnStreamComplete` (入力: `StreamCompleteContext`)
- 戻り値は `StreamHookResult { Continue | Abort(String) | Pause }` で統一済み。
- `Worker` に個別登録 API を実装済み:
  - `add_on_text_delta_hook()`
  - `add_on_tool_call_delta_hook()`
  - `add_on_stream_chunk_hook()`
  - `add_on_stream_complete_hook()`
- `HookRegistry` に全 streaming hook のストレージを追加済み。
- `worker.rs` の stream dispatch ループ内で `BlockDelta` 受信時に各 hook を順序保証付きで呼び出し済み。
- `Abort` / `Pause` の制御線が Worker の run ループに接続済み。

### 計画との差分

- 高レベル統合 API（subscriber/DSL）は `worker_api_plan` 側で別途進行中。
- 実行順序仕様の文書化は未完了（コード上は登録順実行を実装済み）。

---

## 3. Typed output（構造化出力）の第一級 API 化 ⬚

### 目的

「テキストを返して呼び出し側で JSON parse」から脱却し、型付きレスポンスを API レベルで保証する。

### 実装状況: ⬚ 未着手

- `run_typed<T>()` API は未実装。
- `output_schema` の request builder 統合は未実装。
- エラー種別（`StructuredOutputUnsupported` / `Deserialize` / `Empty`）は未追加。

### 次のステップ

1. `output_schema` を Worker request builder に追加
2. typed 実行 API と decode ロジックを実装
3. provider capability 判定を `llm_client` 層に追加
4. エラー型とユーザー向けメッセージを整理

---

## 4. Provider 固有最適化を provider 層で吸収する 🔄

### 目的

キャッシュ、beta header、細粒度 stream 差分などの provider 特有処理を Worker から排除する。

### 実装状況: 🔄 部分的に進行

- `llm_client/providers/*` に Anthropic / OpenAI / Gemini / Ollama の provider 実装が存在する。
- `llm_client/scheme/*` に各 provider の request/events 変換が分離済み。
- Worker は provider 非依存の `Event` を Timeline 経由で処理する設計に移行済み。
- ただし `ProviderOptions` 型や capability trait は未導入。

### 次のステップ

1. provider capability trait を追加
2. 既存 provider 実装を capability 駆動へ移行
3. Worker 側に残る provider 分岐があれば削除

---

## 5. GenAI telemetry を標準実装にする ⬚

### 目的

運用時に必要な追跡情報（turn、tool call、usage、latency、abort reason）を一貫して記録可能にする。

### 実装状況: ⬚ 未着手

- `tracing` クレートは依存に含まれており、`worker.rs` 内で `debug!` / `info!` / `trace!` / `warn!` マクロが使用されている。
- ただし GenAI semantic conventions に沿った構造化 span は未実装。
- `TelemetryConfig` / telemetry モジュールは未追加。

### 次のステップ

1. telemetry モジュール追加（field key 定義）
2. Worker/timeline/tool/hook に span 計装
3. usage と stop reason を終了イベントで集約
4. ログとメトリクス出力の最小 adapter を実装

---

## 導入順序（更新版）

1. ~~ToolServer 分離~~ ✅
2. ~~Streaming Hook 拡張~~ ✅
3. Telemetry 標準化 ⬚
4. Typed output API ⬚
5. Provider capability 移行の仕上げ 🔄

---

## チケット分解（実装順・進捗付き）

### Epic A: ToolServer 分離 ✅

| ID | タイトル | 状態 | 備考 |
| --- | --- | --- | --- |
| A1 | ToolServer 最小骨格の追加 | ✅ | `tool_server.rs` に `ToolServer` / `ToolServerHandle` 実装済み |
| A2 | Worker の tool map を ToolServerHandle に置換 | ✅ | `Worker` は `ToolServerHandle` を保持。`register_tool` / `call_tool` / `tool_definitions_sorted` 全て経由 |
| A3 | Hook context を ToolServer 参照へ接続 | ✅ | `pre_tool_call` / `post_tool_call` で `ToolCallContext` に `meta` / `tool` を `ToolServerHandle::get_tool()` から取得 |
| A4 | KV キャッシュ保護ルールの実装 | ✅ | `CacheLocked` 型状態パターンで実装。`lock()` 後は prefix の変更が型レベルで防止される |
| A5 | ToolServer 単体/統合テスト追加 | ✅ | `tool_server.rs` 内に重複登録・未登録呼び出し・ソート順のテスト実装済み |

### Epic B: Streaming Hook 拡張 ✅

| ID | タイトル | 状態 | 備考 |
| --- | --- | --- | --- |
| B1 | Hook event 種別追加（delta/stream） | ✅ | `OnTextDelta` / `OnToolCallDelta` / `OnStreamChunk` / `OnStreamComplete` 定義済み |
| B2 | Timeline dispatch に hook 呼び出し点追加 | ✅ | `BlockDelta` 受信時に hook 実行。stream dispatch ループ内に組み込み済み |
| B3 | Abort/Pause 制御線の接続 | ✅ | `StreamHookResult` の `Abort` / `Pause` が Worker の run ループに接続済み |
| B4 | 実行順序仕様と回帰テスト | 🔄 | コード上は登録順実行を実装。仕様文書化・専用回帰テストは未完了 |

### Epic C: Telemetry 標準化 ⬚

| ID | タイトル | 状態 | 備考 |
| --- | --- | --- | --- |
| C1 | telemetry モジュールと semantic key 定義 | ⬚ | 未着手 |
| C2 | worker/turn/llm/tool/hook span 計装 | ⬚ | 未着手。`tracing` の基本利用はあるが構造化 span は未実装 |
| C3 | マスキング設定と no-op 運用 | ⬚ | 未着手 |
| C4 | telemetry テスト/サンプル整備 | ⬚ | 未着手 |

### Epic D: Typed Output API ⬚

| ID | タイトル | 状態 | 備考 |
| --- | --- | --- | --- |
| D1 | Request builder に output_schema 追加 | ⬚ | 未着手 |
| D2 | `run_typed<T>` / `run_typed_with_history<T>` 実装 | ⬚ | 未着手 |
| D3 | 非対応 provider のフォールバックとエラー分離 | ⬚ | 未着手 |
| D4 | typed 出力の回帰テスト | ⬚ | 未着手 |

### Epic E: Provider Capability 移行 🔄

| ID | タイトル | 状態 | 備考 |
| --- | --- | --- | --- |
| E1 | provider capability trait 導入 | ⬚ | 未着手 |
| E2 | Anthropic/OpenAI/Gemini の capability 移行 | 🔄 | scheme 層での request/events 分離は完了。capability trait は未導入 |
| E3 | Worker 側 provider 分岐の削除 | 🔄 | Worker は Event ベースで provider 非依存に近いが、完全な分離は未検証 |
| E4 | snapshot テストと責務ドキュメント更新 | ⬚ | 未着手 |

---

## チケット運用ルール

- 各 ticket は `feature/<ID>-...` ブランチで実装する。
- 依存 ticket が `main` に入るまで、後続 ticket は着手しない。
- 各 ticket で最低 1 つ回帰テストを追加する（ドキュメント専用 ticket を除く）。
- `CacheLocked` の prefix 不変性を崩す変更は、A4 と同時に防止テストを追加する。
- tool 定義のリクエスト化は決定的順序にする（内部保持が `HashMap` でも送信順は固定）。

## 非目標

- 既存 Worker API の全面破壊的変更
- 1回の PR で5項目を同時に実装
- Open Responses 完全準拠の即時達成

## 備考

`llm_worker_rs` は Timeline 駆動設計が中核であり、この方針は維持する。`rig` の設計は部品として参照し、全体アーキテクチャは置換しない。
