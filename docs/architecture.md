# Insomnia アーキテクチャ

## プロジェクトの目的

複数の LLM エージェント（Pod）が独立プロセスとして並行動作し、自律的に分業・統括できるエンジン。シングルエージェントの LLM リグとの差別化は、タスクを別 Pod に委譲し結果を集約する**オーケストレーション**を LLM 自身に行わせる点にある。

## 設計原則

### 層の分離と方向性

各クレートは下位層に依存し、上位層を知らない。上位層は下位層の API を丸ごと隠蔽せず、制約が必要な部分だけをラップして提供し、それ以外は下位層を直接使わせる。

### 宣言した層が解決する

ある層が構成を宣言として受け取ったなら、その解決もその層の責務。マニフェストに `[model] ref = "anthropic/claude-sonnet-4-6"`（あるいは `scheme = "anthropic"` + `model_id = ...` の inline 形式）と書いた以上、`ModelManifest` → `LlmClient` の変換は insomnia 側（`crates/provider`）が行う。逆に llm-worker が `LlmClient` trait だけを受け取るのは正しい — llm-worker はプロバイダの選択を宣言として受け取っていないから。

### 概念の追加は不在が問題になってから

概念を先に作らない。「これがないと今の問題が解けない」と言えるまで追加しない。拡張ポイントはドキュメントに記録するが、実装はしない。

### 最小の構造化で最大の自由度

insomnia は環境再現・コンテナ管理・VCS 統合などを自身の責務としない。Pod が動くホストの fs 上で活動する主体を提供し、それにコンテキストを与えてスポーンさせられる仕組みを付与する。構築する環境はユーザー次第。

## Pod

独立したエージェントの実行単位。llm-worker の Worker をラップし、マニフェストによる宣言的構成、ディレクトリスコープ、Pod 名に紐づく永続状態を加える。

- 1 live Pod process は 1 Pod name/current state を所有する
- 会話の永続化は `session-store` の `session_id` / `segment_id` ログ、Pod 名の current state は `pod-store` metadata が担う
- マニフェストまたは profile から完結構築できる
- scope で読み取り・書き込み可能なパスを制限する
- 独立した socket サーバーを持ち、Client (TUI / GUI) が接続して操作する

### 実行ループ

```
Client → Method::Run { input }
  → Worker: user message 追加 → LLM リクエスト
    → LLM 応答: text → Client に Event::TextDelta で配信
    → LLM 応答: tool_use → Interceptor (pre_tool_call) → Tool 実行 → Interceptor (post_tool_call) → Hook
    → tool result を履歴に追加 → 次の LLM リクエスト
  → ターン終了: Hook (on_turn_end) → Client に Event::RunEnd
```

### Interceptor と Hook の分離

- **Interceptor**: Worker の内部制御フロー。context / history の直接操作が可能。Pod の内部機構（compaction トリガー、notification 注入）が使う。外部に公開しない
- **Hook**: Pod の公開 API。read-only のサマリ情報のみを受け取り、制御フロー判断（continue / skip / abort / pause）を返す。将来スクリプト言語に安全に公開可能
- 実行順序: Interceptor → Hook。Interceptor が context を整えた結果を Hook が観測する

### コンテキスト管理

- **Prune**: 古い tool result の content を除去（summary は残す）。LLM request context だけを加工し、永続 history 本体は変更しない
- **Compact**: 履歴 prefix を要約し、同じ `session_id` 配下の新 `segment_id` へ rotate する。`SegmentStart.compacted_from` が元 segment を参照し、Pod metadata の active pointer は新 segment を指す
- サーキットブレーカー: compact が3回連続失敗したら無効化

## Protocol

Pod の制御・監視に使う JSONL ベースのメッセージプロトコル。トランスポートに依存しない。正確な wire enum は `crates/protocol/src/lib.rs::{Method, Event}` を正とし、この節はカテゴリの目次として扱う。

- **Client → Pod (`Method`)**
  - turn 制御: `Run`, `Resume`, `Cancel`, `Pause`, `Shutdown`
  - context/session 制御: `Compact`, `ListRewindTargets`, `RewindTo`
  - typed injection / child lifecycle: `Notify`, `PodEvent`
  - client 補助: `ListCompletions`
  - Pod visibility / attach: `ListVisiblePods`, `InspectPod`, `AttachOrRestorePod`
- **Pod → Client (`Event`)**
  - accepted input / history seed: `Snapshot`, `UserMessage`, `SystemItem`, `SegmentRotated`
  - generation stream: `TurnStart`, `TurnEnd`, `LlmCallStart`, `LlmCallEnd`, retry/continuation events, `Text*`, `Thinking*`, `ToolCall*`, `ToolResult`, `Usage`, `RunEnd`
  - control replies: completions, rewind, visible Pod / inspect / attach results
  - operational status: `Status`, `Alert`, `MemoryWorker`, `Compact*`, `Error`, `Shutdown`
- リクエストとレスポンスの紐付けを一般化した RPC にはしない。多くの状態は broadcast event と Pod status で観測する
- 一部の reply（例: completions）は要求 socket にだけ返る。broadcast event と request-local reply の違いは enum variant のコメントを正とする
- 操作の競合は先勝ち（run 中に別の run → `AlreadyRunning` エラー）

## マニフェストとファクトリ

### PodManifest

Pod の宣言的構成。TOML で記述。

```toml
[pod]
name = "agent"

[model]
ref = "anthropic/claude-sonnet-4-6"

[worker]
instruction = "$insomnia/default"
max_tokens = 4096
temperature = 0.3

[[scope.allow]]
target = "/abs/path"
permission = "write"
```

`[model]` は `ref = "<provider>/<model_id>"` でプロバイダ / モデルカタログを引く短縮形と、`scheme` / `model_id` / `auth` を直書きする inline 形式の両方を受ける。カタログは `resources/{providers,models}/builtin.toml` を builtin、`<config_dir>/{providers,models}.toml` を user override として解決する（`<config_dir>` の解決ルールは `manifest::paths` 参照）。詳細は `docs/pod-factory.md` と `crates/provider/README.md`。

### Manifest / profile 入力

通常の Pod 起動は Lua profile discovery/default から `PodManifest` を生成する。bundled `builtin:default` が fallback default で、user/project `profiles.toml` は profile registry と default selection だけを担う。user/project `manifest.toml` の ambient cascade は通常起動では使わない。

`insomnia-pod --manifest <PATH>` は explicit one-file compatibility/debug input で、指定 TOML 1 枚だけに builtin defaults を merge し、`PodManifestConfig -> PodManifest` の required validation を通す。

`PodFactory` の user/project/overlay API は低レベル構成部品として残るが、CLI の通常起動 path では generic TOML overlay を公開しない。

### Instruction とプロンプト資産

`worker.instruction` はファイル参照。3 層の prefix addressing でプロンプト資産を解決:

- `$insomnia/...` — バイナリ同梱（`resources/prompts/`、`include_dir!` で埋め込み）
- `$user/...` — `<config_dir>/prompts/`（`manifest::paths` で解決）
- `$workspace/...` — `<project>/.insomnia/prompts/`

テンプレートは minijinja で評価。`{% include "$insomnia/common/tool-usage" %}` のようにプロンプト間で参照可能（prefix なしの include は現在のファイルからの相対解決）。

レンダリング結果の末尾に scope summary と AGENTS.md（あれば）がコード側で固定付加される。ユーザーテンプレートからはこれらに触れない。

## Scope

Pod が操作できるファイルパスの制御。

- `allow` ルールで読み取り・書き込みを許可、`deny` ルールで制限
- effective permission = allow - deny
- `recursive = false` で直下のみに制限可能（summary に `[non-recursive]` マーカー）
- scope 排他: Pod 間の write 衝突は runtime registry / scope lock で検出する。child Pod へ委譲した write scope は親の effective scope から delegated-out deny として差し引かれ、child 停止・prune 時に reclaim される

## セッション永続化

- `session-store` は append-only JSONL segment log。1 `LogEntry` = 1 行
- `SessionId` は論理会話の fork-tree root、`SegmentId` はその中の現在の書き込み先 segment
- fresh conversation だけが新 `SessionId` を作る。compact / fork は同じ `SessionId` 配下に新 `SegmentId` を作り、`SegmentStart.{compacted_from,forked_from}: SegmentOrigin` で出自を持つ
- segment log の先頭は `LogEntry::SegmentStart`。以降に `Invoke`, `UserInput`, `AssistantItem`, `ToolResult`, `SystemItem`, `TurnEnd`, `RunCompleted` / `RunErrored`, `ConfigChanged`, `LlmUsage`, `Extension` などを append する
- replay は segment log から Worker state を再構成する。lineage は entry hash ではなく `SegmentOrigin.at_turn_index` と segment id で参照する
- Pod 名の durable current state（active pointer、resolved manifest snapshot、spawned child delegation/reclaim）は `pod-store` metadata が担う。socket path や runtime mirror は liveness authority ではない

## 組み込みツール

正確な callable set と description は ToolRegistry / manifest permission / scope / profile に依存する。高レベルには以下のカテゴリを持つ。

| カテゴリ | 例 | 概要 |
|---|---|---|
| File / shell | `Read`, `Write`, `Edit`, `Glob`, `Grep`, `Bash` | workspace ファイル操作と shell 実行。file tools は `ScopedFs` と read-before-edit tracker を通る。`Bash` は permission policy と出力退避で制御する |
| Task | `TaskCreate`, `TaskUpdate`, `TaskList`, `TaskGet` | セッション内の短期 task 状態管理 |
| Memory / Knowledge | `MemoryQuery`, `MemoryRead`, `MemoryWrite`, `MemoryEdit`, `MemoryDelete`, `KnowledgeQuery` | manifest の memory 設定が有効な時に登録される durable memory / knowledge 操作 |
| Pod orchestration | `SpawnPod`, `SendToPod`, `ReadPodOutput`, `StopPod`, `ListPods` | child Pod の起動・通信・停止・一覧 |
| Visible Pod state | `ListVisiblePods`, `InspectPod`, `AttachOrRestorePod` | durable Pod state と visibility に基づく Pod inspection / attach / restore |
| Web | `WebSearch`, `WebFetch` | manifest/env で明示設定された provider 経由の bounded web access |

すべての tool call は manifest tool permission と scope/policy のチェックを通る。ファイル write scope、Pod delegation、memory layout、web provider 設定はそれぞれ別の authority を持ち、UI 表示だけで権限を広げない。
