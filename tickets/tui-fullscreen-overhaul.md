# TUI: フルスクリーン化によるオーバーホール

## 背景

現在の TUI は **ratatui の inline viewport + `insert_before`** で動いている:

- `draw()` が描くのは画面下部の 3 行固定 (separator / status / input)
- 履歴は `terminal.insert_before()` でその上に押し出し、ターミナル側のスクロールバックに残す
- 一度 `insert_before` した行はアプリから書き換え不可

このモデルが以下の構造的課題を生んでいる:

1. **Input UX が貧弱**: Input は 1 行固定、複数行入力不可、ペーストすると長文が 1 行として流れる
2. **リサイズでセパレーターが増殖**: ターミナル横幅が変わるたびに separator 行が再生成されてスクロールバックに流れ、履歴が汚れる
3. **読み返し辛い**: TUI アプリ自身にスクロール機能がなく、ターミナル側スクロールバックに完全依存。折りたたみもフィルタリングもできない
4. **ツール呼び出しが断片化**: `ToolCallStart` / `ToolCallDone` / `ToolResult` がそれぞれ別行として積まれ、1 呼び出しを連続した 1 ブロックとして見られない。`ToolCallArgsDelta` は **破棄されている** (`crates/tui/src/app.rs`)

個別対症療法では直しきれないため、**レンダリングモデルを alternate screen buffer + 全描画保持に切り替えるオーバーホール**として扱う。旧 `tui-tool-call-ui.md` は inline viewport 維持を前提にした設計だったため、本チケットに要件を吸収して削除する。

## 方針

- ratatui を alternate screen buffer で初期化し、inline viewport を捨てる
- 全履歴を TUI アプリ内の state として保持、毎フレーム再描画
- ターミナル側のスクロールバックには何も流さない（リサイズ時の汚染と永続履歴依存を同時に解消）
- 履歴の見せ方をツール呼び出しやターン単位で集約可能にし、スクロール + 折りたたみ 3 段階で密度を変えられるようにする
- ツール呼び出しは 1 呼び出し = 1 ブロック。ツール名で dispatch する **ツール毎レンダラ** のフレームワークを持つ
- Input は複数行 / CJK / ペースト プレースホルダに対応

## 要件

### レンダリングモデル

- ratatui を alternate screen buffer で起動する。`insert_before` は使わない
- アプリ終了時はスクロールバックに戻る（TUI が描いたものは残らない）
- 再接続時は既存の `Event::History` で state を再構築する
- ウィンドウリサイズは state を保ったまま再レイアウトする（セパレーター増殖等は発生しない）

### レイアウト

- 画面全体を以下で構成する:
  - 上部: 履歴ビュー（可変高さ、スクロール可）
  - 下部: ステータス行 (1 行) + Input エリア (可変高さ)
- Input エリアが画面下部を占有しすぎないように上限を設ける（設計で決めること）

### 履歴モデル

- 最小単位は **ブロック**。種類: GreetingCard / TurnHeader / UserMessage / AssistantText / ToolCall / Notification / CompactEvent / TurnStats
- ブロックは **ターン** にグループ化される。TurnHeader / UserMessage 〜 その turn の TurnStats までが 1 ターン
- 履歴全体が state に保持され、モードに応じて各ブロックの見た目が変わる

### スクロール

- 行単位 / ページ単位のスクロール
- ターン単位のジャンプ（前のターン / 次のターン / 先頭 / 末尾）
- 末尾追従（常に最新を表示）は新規イベント到着でトリガ。ユーザーが手動で上にスクロールしている間は追従を停止

### 折りたたみモード

3 段階、全体トグルで切替。

- **detail**: 全ブロックを完全表示。ツールブロックは引数ストリーミング + 結果全体
- **normal**: 実行中のツールブロックは detail と同じ。完了後は各ブロックが概ね 5〜6 行に収まるよう圧縮
- **overview**: 各ブロックが 1 行。ツールブロックは `ToolResult.summary` をそのまま使う

### Input エリア

- 複数行、自動折り返し
- CJK 含む Unicode 表示幅に基づく正しいカーソル位置 (`unicode-width`)
- **ペースト プレースホルダ**: クリップボードから貼り付けられた文字列はプレースホルダ `[Clipboard #N | X chars, Y lines]` として入力バッファに挿入される。実テキストは裏で保持
  - プレースホルダは不可分（Backspace 1 回で全体が削除される。途中カーソルでの文字単位削除は不可）
  - 送信時にプレースホルダが実テキストに展開されて Pod に送られる（Pod 側には `#N` は見せない）
  - 番号付けの規則は設計で決めること
- 既存キー操作（カーソル移動・削除）は複数行対応に拡張

### ツール UI フレームワーク

- ツール呼び出し 1 回 = 1 ブロック。`tool_use_id` で同一ブロックに集約
- 各ブロックは以下の状態を遷移する:
  - **Pending**: `ToolCallStart` 受信、args 未確定
  - **Streaming**: `ToolCallArgsDelta` を連結中。ライブ反映
  - **Executing**: `ToolCallDone` 受信、args 確定、結果待ち
  - **Done / Error**: `ToolResult` 受信
  - **Incomplete**: `ToolResult` が来ないまま turn が終わった場合
- ブロックの見た目は **ツール毎のレンダラ** が決める。ツール名で dispatch、マッチしなければデフォルトレンダラ
- 並列ツール呼び出しは複数ブロックが同時に存在する。`tool_use_id` で振り分け
- Streaming 中の args は生の文字列として連結し、途中の JSON を整形しようとしない（破綻する）

### 組み込みツールのレンダラ

実装対象: Read / Write / Edit / Glob / Grep / default。

- **Read**: 同一 turn 内で連続する Read 呼び出しを **1 ブロックに集約**する。normal / detail とも「読んだファイル数 + ファイルパスのリスト」を表示、中身は出さない。集約中のライブ表示は実行順に最大 3 行のスクロールウィンドウで読んだファイルを下から追加
- **Write**: summary (Created / Overwrote をラベル色で区別) + 書き込まれた content の先頭 5 行。detail では content 全体
- **Edit**: TUI 側 **ファイル content キャッシュ** から該当ファイルを引き、`args.old_string` / `args.new_string` の置換箇所に対して **前後 3 行の unified diff** を赤 / 緑で表示
- **Glob**: `ToolResult.output` の先頭数行をそのまま表示
- **Grep**: 同上
- **default (未知ツール)**: normal で pretty JSON 化した args の先頭 3 行 + `ToolResult.output` の先頭 3 行。detail では全体

### ファイル content キャッシュ

Edit レンダラが diff を出すために TUI 側に持つ。責務は表示用で、ツール実装層の policy (`Tracker`) とは独立。

- Read レンダラが `ToolResult.output` からキャッシュに content を保存
- Write レンダラが `args.content` でキャッシュを更新
- Edit レンダラが `args.old_string` / `args.new_string` でローカル置換してキャッシュを更新
- `Event::History` 再生時も同じ順序でキャッシュを再構築する

### 既存機能の移植

以下は現行 TUI に既にある機能で、新アーキテクチャでも保持する:

- Greeting カード表示
- TurnHeader / UserMessage / AssistantText
- Notification (Warn / Error レベル)
- Compact 開始 / 完了 / 失敗
- ターン終了時の統計 (requests / tokens)
- Ctrl-C 2-tap でのアプリ終了 (Pod 自体は存続)
- shutdown_confirm 挙動
- Paused 状態での空 Enter → Resume
- `Event::History` によるセッション再接続時の履歴復元

### 履歴再生

- `Event::History` を受けたとき、過去のツール呼び出しは **最終状態のブロック** として履歴モデルに組み直される
- tool_call と tool_result の対応付け、および content キャッシュの再構築もこの経路で行う

### イベント欠落耐性

- プロバイダ起因でイベントが欠落したり順序が逆転しても panic しない
- `ToolResult` が来ないまま `TurnEnd` が来た場合、該当ブロックは Incomplete として残す（握りつぶさず視覚化する）

## 設計で決めること

- **キーバインド**: スクロール / ターン移動 / モード切替 のキー割り当て
- **折りたたみの粒度**: 全体トグルのみか、ターン単位の個別開閉も持たせるか
- **Input エリアの高さ上限**: 画面の N% か絶対行数か
- **ペースト番号付け**: TUI 起動中で通しか turn 毎にリセットするか
- **normal モードの圧縮基準**: 5〜6 行は目安。ブロック種別毎の実装上の判断基準
- **末尾追従の解除条件**: 上にスクロールした瞬間に解除するか、数行離れたら解除するか
- **Read 集約の切れ目**: 「連続」の定義（別ツール呼び出しや assistant text が挟まったら切る / `ToolResult` の受信順で切る 等）

## 前提

- `tickets/protocol-tool-result-shape.md`: `Event::ToolResult` に `summary: String` が追加されていること

## 完了条件

- TUI が alternate screen buffer で起動し、アプリ終了でスクロールバックに何も残らない
- ウィンドウリサイズで separator の増殖が発生しない
- 既存機能 (greeting / turn / user / assistant / tool / notification / compact / stats / ctrl-c / paused / history) がすべて保持されている
- 3 段モード (detail / normal / overview) で履歴の密度を切り替えられる
- Input が複数行 / CJK / ペースト プレースホルダに対応している
- ツール呼び出しが 1 ブロックとして表示され、start → args streaming → done → result の遷移が同じブロックの更新として見える
- 組み込み 5 ツール (Read / Write / Edit / Glob / Grep) がそれぞれ専用レンダラで表示される
- 未知ツールがデフォルトレンダラで表示される
- 並列ツール呼び出しで `tool_use_id` が正しく振り分けられる
- `ToolResult` が欠落したまま turn が終わった場合に該当ブロックが Incomplete として履歴に残り、panic しない
- `Event::History` 再生で過去のツール呼び出しがブロックとして復元され、Edit レンダラ用の content キャッシュも再構築される

## 範囲外

- ツール実行のキャンセル / 介入 UI
- 複数 Pod の spawn / 切替 UI (`tickets/tui-pod-spawn-ui.md`)
- Markdown レンダリング / シンタックスハイライト / リッチ diff レンダリング
- スクロールバック保存（仕様上出さない）
- TUI 独自の履歴永続化（再接続時は Pod 側の `Event::History` に任せる）
- 組み込みツールの `ToolOutput.content` 構造化 (protocol 拡張はスコープ外。必要になったら別チケット)
