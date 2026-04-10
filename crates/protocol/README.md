# protocol

クライアントとPod間の通信プロトコルを定義するクレート。Unix ソケット上で JSON Lines として送受信されるメッセージ型を提供する。

## 公開型

- `Method` — クライアント→Pod のコマンド（`Run`, `Resume`, `Cancel`）
- `Event` — Pod→クライアント のイベント（`TurnStart`, `TextDelta`, `ToolCallStart`, `Usage`, `Error` など）
- `TurnResult` — ターン完了状態（`Finished`, `Paused`）
- `ErrorCode` — エラー分類（`AlreadyRunning`, `ProviderError`, `ToolError` など）
