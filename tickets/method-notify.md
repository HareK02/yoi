# Method::Notify: システム起点のコンテキスト注入と自動ターン開始

## 背景

現状の Pod の実行サイクルは `Method::Run { input }` が唯一のターン開始手段であり、これは人間（または外部クライアント）が明示的にテキストを送ることを前提としている。

Pod オーケストレーション（`tickets/pod-orchestration.md`）では、子 Pod が親 Pod にコールバックで通知を送り、親 Pod が**人間の入力を待たずに**その通知を処理して次のアクションを起こす必要がある。現状のアーキテクチャにはこの経路が無い。

また、RUNNING 中の Pod に対しても、リクエストの合間に情報を注入して LLM に認知させたいケースがある（子 Pod からの非同期通知など）。現状は RUNNING 中にコンテキストを追加する手段が無い。

## ゴール

`Method::Notify` を新設し、Pod の状態（IDLE / RUNNING）に応じて適切にコンテキストへの注入とターン制御を行う。

## 仕様

### `Method::Notify { source: String, message: String }`

protocol に追加する新しい Method。`Run` とは異なり、ユーザーメッセージではなく**システム通知**としてコンテキストに注入される。

### Pod が IDLE のとき

1. 通知メッセージを system message としてコンテキストに注入
2. **自動でターンを開始**する（人間の入力を待たない）
3. LLM は通知を見て次のアクションを判断する

### Pod が RUNNING のとき

1. 通知メッセージを**次の LLM リクエストの直前**に注入する（tool call の応答を送った後、次のリクエストを組み立てる前）
2. ターンは中断しない。LLM は現在のタスクを続行しつつ、次のレスポンスで通知を認知する
3. LLM は今やっていることを優先し、切りの良いタイミングで通知に対処するかを自分で判断する

### 注入されるメッセージのフォーマット

通知は素のテキストではなく、以下の構造で注入される:

```
[Notification from {source}]
{message}

This is a notification, not a blocking request. If you are in the middle of a task, continue your current work and address this at a natural stopping point.
```

- `[Notification]` prefix で LLM にこれが通知であることを明示
- 「ブロッカーではないので直ちに対処しなくてよい」という指示を付与
- LLM が通知を見て即座にタスクを放棄する（指示追従性の暴走）を防ぐ

### 複数通知のバッファリング

- RUNNING 中に複数の `Notify` が到着した場合、バッファに溜めて次の LLM リクエスト直前にまとめて注入する
- 個別の `[Notification]` ブロックとして並べる（1つにマージしない）
- IDLE 中に複数到着した場合、1つの system message にまとめて注入し、ターンを1回だけ開始する

## 実装に必要な変更

### protocol crate

- `Method::Notify { source: String, message: String }` を `Method` enum に追加
- 対応する `Event` の追加が必要かは設計時に判断

### Worker

- RUNNING 中に外部からメッセージを注入する仕組みが必要
- 現状の Worker は turn 実行中にコンテキストの追加手段を持たない
- tool call → tool result → **notification 注入** → 次の LLM リクエスト、というフローを追加
- 注入ポイントは `execute_tools` 完了後、次の LLM リクエスト組み立て前

### Controller

- `Method::Notify` のハンドリングを追加
- IDLE 時: 通知を注入 → 内部的に `run()` を開始（`Method::Run` と似た経路だがメッセージ種別が異なる）
- RUNNING 時: Worker の notification buffer に push

### Pod

- notification buffer を保持するフィールドを追加
- `ensure_system_prompt_materialized` 的な「ターン開始前に notification を flush する」フックが要るかもしれない

## `Method::Run` との対比

| | `Method::Run` | `Method::Notify` |
|---|---|---|
| 対象状態 | IDLE のみ（RUNNING 中は AlreadyRunning エラー） | IDLE でも RUNNING でも受け付ける |
| コンテキスト上の見え方 | user message | system message（`[Notification]` prefix 付き） |
| ターン制御 | 新ターンを開始 | IDLE: 自動でターン開始。RUNNING: 現ターンに注入 |
| LLM の期待挙動 | 指示に従って即座に行動 | 現タスク優先、切りの良いタイミングで対処を判断 |
| 送信元 | 人間 / クライアント | システム / 子 Pod のコールバック / Hook |

## 設計で決めること

- **notification の注入はどの message type で行うか**: 既存の `Role::User` / `Role::Assistant` とは別の `Role::System` (mid-conversation) を追加するか、`Role::User` に `[Notification]` prefix を付けて実質的に区別するか
- **IDLE 時の自動ターン開始と人間の `Run` の競合**: 通知が到着して自動ターンが始まった直後に人間が `Run` を送った場合の挙動
- **notification buffer のサイズ上限**: RUNNING が長時間の場合にバッファが無制限に溜まるリスク
- **通知メッセージの prefix / suffix テンプレートの置き場**: ハードコードか、instruction 側でカスタマイズ可能にするか
- **Event の対応**: `Event::NotificationInjected` のような確認イベントを返すか

## 完了条件

- `Method::Notify` が protocol に追加され、Controller が IDLE / RUNNING で適切にハンドリングする
- IDLE 時: 通知が system message として注入され、自動でターンが開始される
- RUNNING 時: 通知が次の LLM リクエスト直前に注入され、LLM が認知できる
- 複数通知のバッファリングが動作する（RUNNING 中に溜まった通知がまとめて注入される）
- `[Notification]` prefix と non-blocking 指示が付与される
- 単体テストで IDLE / RUNNING 両パスが検証される

## 他チケットとの関係

- **tickets/pod-orchestration.md**: 本チケットの主要消費者。子 Pod の `Notify` ツール → 親の callback → 親の `Method::Notify` というフローでオーケストレーションの非同期通知が成立する
- **tickets/protocol-design.md**: protocol への Method 追加。既存の Method 設計パターン（Run / Resume / Cancel / Shutdown）と整合させる
- **tickets/compact-improvements.md**: notification がコンテキストに蓄積した場合の compaction 挙動は別途検討

## 範囲外

- **通知の routing / addressing**: 誰が誰に通知を送るかはオーケストレーション側の責務。本チケットは「Pod が Notify を受けたときの挙動」だけを扱う
- **通知の優先度 / フィルタリング**: 全通知を等しく注入する。重要度に応じた選別は LLM に任せる
- **通知の永続化**: 通知は session-store に永続化しない。コンテキスト上の message としてのみ存在する
