# TUI キーバインド

`crates/tui` の対話画面で効くキー一覧。`main.rs:handle_key` を単一情報源とし、このドキュメントは人間向けの目次。

## 入力編集

| キー | 動作 |
|---|---|
| 文字キー | カーソル位置に挿入 |
| `Backspace` | カーソル直前を削除 |
| `Delete` | カーソル直後を削除 |
| `Left` / `Right` | カーソル移動 |
| `Home` / `End` | 行頭 / 行末へ |

## 送信（Enter）

| 状態 | 入力あり | 入力空 |
|---|---|---|
| Idle | `Method::Run` で新ターン開始 | no-op |
| Paused | `Method::Run`（前ターンは割り込み終了として扱い、新ターンとして開始） | `Method::Resume`（前ターンの続きを再開） |
| Running | 入力は TUI 側でバッファのみ、送信はしない | 同左 |

### Paused からの挙動

Paused 中に Enter すると、入力の有無で 2 通り：

- **空**: 前ターンを中断した地点から **Resume**。LLM はそのまま続きを書く（partial text は破棄済み）、未実行の tool があれば実行して続行
- **入力あり**: 前ターンは「割り込み終了」扱いとなり、新ターンとして **Run**。Pod 側では
  1. 未応答 `tool_use` があれば synthetic `tool_result("[Interrupted by user]")` で閉じる
  2. `[The previous turn was interrupted by the user]` system note を履歴に挿入
  3. 入力を新しい user メッセージとして append
  4. ターン開始

## Pod 制御

| キー | Running 中 | Idle / Paused |
|---|---|---|
| `Ctrl-X` | `Method::Cancel`（進行中ターンを破棄 → Idle） | no-op（エラー表示のみ） |
| `Ctrl-C` | `Method::Pause`（進行中ターンを中断 → Paused） | 1 回目 warn、3 秒以内の 2 回目で TUI 終了（Pod は残る） |
| `Ctrl-D` | 1 回目 warn、3 秒以内の 2 回目で `Method::Shutdown` | `Method::Shutdown`（Pod を終了） |

### Cancel と Pause の違い

- **Cancel** は「ターンを捨てる」: 進行中の LLM リクエスト・未完了 tool を打ち切り、状態は Idle。続きは Resume できない
- **Pause** は「止めるけど続けられるように」: 同じく打ち切るが状態は Paused、空 Enter で Resume 可能

Running 中に割り込みたい場合、ほとんどのケースで `Ctrl-C`（Pause）が自然。Ctrl-X（Cancel）は明示的に破棄したい時（LLM が暴走した時など）用。

### Ctrl-C と Ctrl-D の 2 段階 UX

どちらも「破壊的に見える操作」は確認を挟む：

- Ctrl-C Running 中: 1 回目で即 Pause（破壊的ではない）
- Ctrl-C Idle / Paused: 1 回目で warn メッセージ、3 秒以内の 2 回目で TUI 終了（Pod は残る）
- Ctrl-D Running 中: 1 回目で warn、3 秒以内の 2 回目で Shutdown
- Ctrl-D Idle / Paused: 1 回目で即 Shutdown

`Ctrl-C` は Pod は落とさず TUI プロセスだけ抜ける。`Ctrl-D` は Pod 自体に `Method::Shutdown` を送って終了させる（Pod プロセスが消える）。

## 履歴メモ

- かつて存在した `Ctrl-R`（Resume 専用）は、空 Enter での Resume に統合されたため廃止
- かつて存在した `Esc`（TUI 終了）は、`Ctrl-C` の 2 連打 UX に統合されたため廃止
