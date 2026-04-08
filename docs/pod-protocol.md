# Pod Protocol 仕様

## 概要

Pod の制御・監視に使う JSONL ベースのメッセージプロトコル。

トランスポートに依存しない。CLI は Pod を直接制御し、daemon は Unix socket 上でこのプロトコルを中継する。

- **フレーミング**: 1行 = 1 JSON オブジェクト（`\n` 区切り）
- **方向**: 双方向。クライアントはメソッドを送信し、Pod はイベントを emit する

```
CLI        → Pod Protocol (直接呼び出し)
Native App → Pod Protocol (直接呼び出し)
Web        → 中央バックエンド → daemon (Unix socket) → Pod Protocol
```

## 設計原則

- リクエストとレスポンスの紐付けはしない。Pod は1つであり、Pod の状態遷移（イベント）を見れば何が起きているか分かる
- イベントは全リスナーに broadcast される。読み取り専用の監視も、操作側も同じストリームを受け取る
- 操作の競合は先勝ち。run 中に別の run が来たらエラーイベントを返す

## メッセージ形式

### クライアント → Pod（メソッド）

```json
{"method": "<name>", "params": {<...>}}
```

`params` はメソッドごとに異なる。省略可能な場合は `params` フィールド自体を省略できる。

### Pod → クライアント（イベント）

```json
{"event": "<name>", "data": {<...>}}
```

全リスナーに broadcast される。

## メソッド一覧

### `run`

ユーザー入力を送信し、LLM ターンを開始する。

```json
{"method": "run", "params": {"input": "What is the capital of France?"}}
```

Pod が既に実行中の場合、エラーイベントが返る。

### `resume`

Paused 状態から再開する。

```json
{"method": "resume"}
```

### `cancel`

実行中のターンをキャンセルする。

```json
{"method": "cancel"}
```

### `get_status`

Pod の現在の状態を要求する。応答は `status` イベントとして返る。

```json
{"method": "get_status"}
```

### `get_history`

会話履歴を要求する。応答は `history` イベントとして返る。

```json
{"method": "get_history"}
```

## イベント一覧

### ターン制御

#### `turn_start`

LLM ターンの開始。

```json
{"event": "turn_start", "data": {"turn": 1}}
```

#### `turn_end`

LLM ターンの完了。

```json
{"event": "turn_end", "data": {"turn": 1, "result": "finished"}}
```

`result`: `"finished"` | `"paused"`

### ストリーミング

#### `text_delta`

テキスト応答の差分。

```json
{"event": "text_delta", "data": {"text": "The capital"}}
```

#### `text_done`

テキストブロックの完了。全文を含む。

```json
{"event": "text_done", "data": {"text": "The capital of France is Paris."}}
```

#### `thinking_delta`

思考プロセスの差分（extended thinking 対応モデル）。

```json
{"event": "thinking_delta", "data": {"text": "Let me consider..."}}
```

#### `thinking_done`

思考ブロックの完了。

```json
{"event": "thinking_done", "data": {"text": "..."}}
```

### ツール

#### `tool_call_start`

ツール呼び出しの開始。

```json
{"event": "tool_call_start", "data": {"id": "call_123", "name": "search"}}
```

#### `tool_call_args_delta`

ツール引数の JSON 差分（ストリーミング中）。

```json
{"event": "tool_call_args_delta", "data": {"id": "call_123", "json": "{\"query\":"}}
```

#### `tool_call_done`

ツール呼び出しの引数確定。

```json
{"event": "tool_call_done", "data": {"id": "call_123", "name": "search", "arguments": "{\"query\": \"Paris\"}"}}
```

#### `tool_result`

ツール実行結果。

```json
{"event": "tool_result", "data": {"id": "call_123", "output": "Paris is the capital...", "is_error": false}}
```

### 状態

#### `status`

`get_status` への応答、または状態変化時に送信。

```json
{"event": "status", "data": {"state": "idle", "session_id": "019d6e91-...", "pod_name": "hello-pod"}}
```

`state`: `"idle"` | `"running"` | `"paused"`

#### `history`

`get_history` への応答。

```json
{"event": "history", "data": {"items": [...]}}
```

`items` は llm-worker の `Item` 配列をそのまま JSON シリアライズしたもの。

### メタ

#### `usage`

トークン使用量。

```json
{"event": "usage", "data": {"input_tokens": 25, "output_tokens": 150}}
```

#### `error`

エラー通知。

```json
{"event": "error", "data": {"code": "already_running", "message": "Pod is already executing a turn"}}
```

エラーコード:
- `already_running` — run 中に run が来た
- `not_running` — run していないのに resume/cancel が来た
- `not_paused` — paused でないのに resume が来た
- `provider_error` — LLM プロバイダからのエラー
- `tool_error` — ツール実行エラー
- `internal` — 内部エラー

## リスナーのライフサイクル

1. リスナーが登録される（直接呼び出しなら関数登録、daemon 経由なら socket 接続）
2. 登録直後から Pod のイベントが流れ始める（購読手続き不要）
3. クライアントはメソッドを任意のタイミングで送信できる
4. リスナーの解除は登録解除または接続切断で行う

## トランスポート: daemon (Unix socket)

daemon は Pod Protocol を Unix domain socket 上で中継する薄い層。

- クライアントが socket に接続するとリスナーとして登録される
- メソッドは socket 経由で Pod に転送される
- 切断時にリスナーリストから除外するだけでクリーンアップ完了

## イベントと llm-worker の対応

| イベント | llm-worker ソース |
|---------------|-----------------|
| `turn_start` | Subscriber `on_turn_start` |
| `turn_end` | Subscriber `on_turn_end` + `WorkerResult` |
| `text_delta` | `TextBlockEvent::Delta` |
| `text_done` | Subscriber `on_text_complete` |
| `thinking_delta` | `ThinkingBlockEvent::Delta` |
| `thinking_done` | `ThinkingBlockEvent::Stop` |
| `tool_call_start` | `ToolUseBlockEvent::Start` |
| `tool_call_args_delta` | `ToolUseBlockEvent::InputJsonDelta` |
| `tool_call_done` | Subscriber `on_tool_call_complete` |
| `tool_result` | `PostToolCall` hook |
| `usage` | `UsageEvent` |
| `error` | `ErrorEvent` / `WorkerError` |
| `status` | Pod 状態（Pod 層が管理） |
| `history` | `Worker::history()` |
