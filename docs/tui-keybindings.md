# TUI キーバインド

`crates/tui` の対話画面で効くキー一覧。`main.rs:handle_key` を単一情報源とし、このドキュメントは人間向けの目次。

## 入力編集

| キー | 動作 |
|---|---|
| 文字キー | カーソル位置に挿入（未割り当ての `Ctrl`+文字キーは無視） |
| `Ctrl-A` | 入力欄全体の先頭へ |
| `Backspace` | カーソル直前を削除（ペーストプレースホルダは 1 回で全削除） |
| `Delete` | カーソル直後を削除（同上） |
| `Left` / `Right` | カーソル移動 |
| `Up` / `Down` | 論理行単位の上下移動（桁位置を保持） |
| `Home` / `End` | 現在行の行頭 / 行末へ |
| `Alt-Enter` | 改行を挿入（Input を複数行化） |

### ペーストプレースホルダ

ブラケットペーストで入力されたテキストは入力バッファ上では不可分な
プレースホルダ `[Clipboard #N | X chars, Y lines]` として表示される。
実テキストは裏で保持され、送信時に `#N` ラベル無しで展開されて Pod に渡る。
カーソルはプレースホルダ内部には入れず、`Backspace` / `Delete` 1 回で
プレースホルダ全体が消える。`#N` の通し番号は TUI プロセス起動中で連番。

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

## 履歴ビューのナビゲーション

履歴ビューは全画面 TUI の上段にあり、TUI 内部で全ブロックを state として
保持してスクロール可能。スクロールバック（端末側の履歴）ではなく TUI の
自前バッファを動かす点に注意。

| キー | 動作 |
|---|---|
| `Shift-Up` / `Shift-Down` | 1 論理行スクロール |
| `PageUp` / `PageDown` | 1 ページスクロール（`area_height - 1` 行） |
| `Ctrl-[` / `Ctrl-]` | 前 / 次のターン先頭へジャンプ |
| `Ctrl-Home` | 履歴の先頭へ |
| `Ctrl-End` | 履歴の末尾へ（末尾追従モードを再開） |

### 末尾追従

デフォルトは「末尾追従」モード：新しいイベント到着で自動的に最新行が
画面下に固定される。ユーザーが上方向にスクロールした瞬間に追従は解除され、
その位置で固定される（新着イベントが来ても画面は動かない）。再び追従に
戻すには `Ctrl-End`。追従解除中はステータスバー右に `↑ scrolled` が点く。

### ターン単位ジャンプ

`Ctrl-[` / `Ctrl-]` は TurnHeader ブロックの位置にスクロールオフセットを
合わせる。多数のツール呼び出しが挟まった長いターンでも前後ターンの先頭に
1 発で戻れる。末尾ターンから `Ctrl-]` を押すと末尾追従に復帰する。

## 表示モード

履歴各ブロックの密度を 3 段階で切り替える。現在のモードはステータスバー
右端に `[mode]` として表示される。

| キー | 動作 |
|---|---|
| `Ctrl-O` | `detail` → `normal` → `overview` → `detail` の順に循環 |

- **detail**: 全ブロック完全表示。ツールブロックは結果本体も全量
- **normal**: 完了ブロックは概ね 5–6 行に圧縮、実行中のツールブロックは
  detail と同じ扱い
- **overview**: 各ブロック 1 行に畳む（ツールブロックは
  `ToolResult.summary` 1 行、長文 AssistantText は先頭 1 行 + 省略記）

モード切替は全体に一括適用。個別ブロックの開閉は持たない。

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
- 旧 inline viewport 時代は履歴スクロールを端末側スクロールバックに
  任せていたため TUI 内のスクロールキーは存在しなかった。全画面 alt screen
  への移行（`tickets/tui-fullscreen-overhaul.md`）で `Shift-Up/Down` ほか
  のナビゲーションキーが追加された
