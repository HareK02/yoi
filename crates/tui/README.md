# tui

Pod と対話するためのターミナル UI クライアント。Unix ソケット経由で Pod に接続し、チャット形式でユーザー入力の送信・アシスタント応答の表示・ツール実行の監視を行う。

## 公開型

- `App` — アプリケーション状態（メッセージ履歴、入力バッファ、スクロール位置）
- `Message` / `MessageKind` — 表示メッセージ（User, Assistant, Tool, Error, Status）
- `PodClient` — Pod との Unix ソケット通信クライアント（`connect()`, `send()`, `next_event()`）
- `draw()` — ratatui によるUI描画関数
