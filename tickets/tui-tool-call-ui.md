# TUI: ツール呼び出しをアクティブ領域内で更新型表示する

## 背景

現在の TUI はツール呼び出しを append-only のテキスト行として扱っており、1 回の呼び出しごとに `[tool] Read` / `[tool] Read done (128 bytes)` / `[tool result] ...` の**3〜4 行**が別々に積まれる。プロバイダがストリーミングで流してくる `ToolCallArgsDelta` は `crates/tui/src/app.rs:167` で丸ごと無視されており、**ユーザーはツールがどんな引数で呼ばれようとしているかを完了まで見られない**。

この制約は単なる実装漏れではなく、現行の**レンダリングモデルから来る構造的なもの**である。

### 現行レンダリングモデル

TUI は ratatui の **inline viewport + `insert_before`** で動いている:

- `crates/tui/src/ui.rs` の `draw()` で描く inline viewport は **separator / status / input の 3 行固定**
- 履歴は `terminal.insert_before(height, ...)` で inline viewport の**上に押し出す**
- 一度 `insert_before` で流した行はターミナルのスクロールバックに入り、**アプリから書き換え不能**になる

つまり現行アーキテクチャには「update 可能な領域」という概念が inline viewport の 3 行以外に存在しない。`ToolCallArgsDelta` を delta ごとに行として積むと履歴が爆発するので暫定的に捨てている、というのが現状の正確な理解。

### 追跡して書き換えるような API は存在しない

スクロールバックに入った内容は terminal emulator の private buffer に属し、ANSI / crossterm の層でも編集できない。ratatui 側にも「過去に insert した行を更新する」API は無い。書き換えできる唯一の場所は **毎フレーム再描画される inline viewport の内部**。

## 方針: アクティブ領域として inline viewport を拡張する

「書き換えできる場所でだけ書き換える」という一点で現行モデルと両立させる:

1. **inline viewport を可変高さにする**。現行の 3 行固定（separator / status / input）に加え、`active` 領域を上部に確保する。
2. **進行中のツール呼び出しフレームを active 領域に保持**する。`ToolCallStart` でフレームを作り、`ToolCallArgsDelta` で毎フレーム再描画し、引数の流入をライブプレビューする。
3. **完了 + 引き継ぎのタイミングで `insert_before` に格下げ**する。ツールが終わり、次のブロック（assistant text / 次の tool / turn end）が始まった時点で、そのフレームの最終状態を `insert_before` でスクロールバックに追い出し、履歴として immutable にする。
4. **active 領域は常に最小限に保つ**。何も進行していないときは 0 行。ツールが複数並列で走っているときはそれぞれのフレーム分だけ膨らむ。

このモデルは現行のスクロールバック活用・コピペ親和性を保ったまま、進行中の部分だけを mutable にする最小侵襲のアプローチ。fullscreen TUI への移行（全履歴を state から毎フレーム描く大工事）は取らない。

## 要件

### レンダリングモデルの変更

- inline viewport を可変高さにし、active 領域 + 固定 3 行（separator / status / input）の構成にする。
- active 領域が画面高さを侵食しすぎないように**上限**を設ける（上限超過時は active 内をスクロールまたは省略）。上限値の決め方は設計時に判断。

### フレームのライフサイクル

ツール呼び出し 1 回 = フレーム 1 個。tool_use_id で同一フレームに集約する。

- `ToolCallStart`: フレームを作り active 領域に追加。状態 = **Pending**（args 未確定）
- `ToolCallArgsDelta`: フレーム内の argument preview に delta を連結し、次フレームで再描画。状態 = **Streaming**
- `ToolCallDone`: args 受信完了。状態 = **Executing**（実行中）
- `ToolResult`: 対応する `tool_use_id` のフレームに紐付ける。結果を取り込み、状態 = **Done** または **Error**
- フレームが Done/Error になり、次のブロック（text / tool / turn end 等）が来た時点で `insert_before` に flush し active 領域から削除する

### 並列ツール呼び出し

- 複数の tool_use_id が同時に active 領域に存在できる。
- イベントが interleave した順序で届いても tool_use_id で正しく振り分ける。
- 並列実行の並び順は active 領域内で安定（開始順が望ましい）。

### 引数のライブプレビュー

- delta を**生の文字列として連結**し、そのまま表示する。途中の JSON を整形しようとしない（破綻する）。
- プレビュー長に上限を設ける。1 フレームが active 領域を占有しないように折り畳みまたは省略。
- 非常に長い引数（Write の `content` 等）は最小実装では省略でよい。展開キーは任意。

### 状態による視覚差異

- Pending / Streaming / Executing / Done / Error を色・枠線・アイコン等で区別する。
- 完了フレームが `insert_before` で格下げされた後も、Done/Error のスタイルは履歴側に残る（insert_before に渡す時点で最終スタイルを焼き込む）。

### 結果の取り込み

- `ToolResult` はフレームに統合され、**別行として insert_before されない**。
- 結果のサマリ（`ToolResult.summary`）はフレーム内にインライン表示。本体（`content`）は最小実装では省略または簡易表示。リッチレンダリングは別チケット。

### 履歴再生との共存

- セッション再開で `Event::History` を受けたときも、過去のツール呼び出しはフレーム化された最終状態で `insert_before` に流れる（active 領域には入らない）。
- 現行の `App::restore_history`（`app.rs:240`）にも手が入る。tool_call + tool_result を 1 フレームに集約する経路が必要。

### イベントの欠落耐性

- プロバイダ起因でイベントが欠落したり順序が逆転しても panic しない。
- `ToolResult` が来ないまま `TurnEnd` になった場合、active 領域のフレームは **Incomplete** 状態で flush する（握りつぶさず視覚化する）。

## 設計で決めること

- **active 領域の高さ上限**: 画面の N% か絶対行数か、超過時は active 内スクロールか自動折り畳みか
- **フレームのデータモデル**: 既存 `OutputItem` に `ToolFrame` バリアントを足すか、active 領域専用の別コレクションとして持つか（後者の方が flush タイミングの制御が素直）
- **flush のトリガー**: 「次のブロックが来たら flush」で十分か、時間経過やユーザー操作でも flush する必要があるか
- **折り畳み / 展開**: 最小実装に含めるか、別チケットに切り出すか
- **並列フレームの並び順**: 開始順 / 終了順 / 画面上の発生順
- **Error と通知チャネルの使い分け**: ツール失敗はフレームの Error 状態で済ませるか、`tickets/tui-notification-channel.md` の通知にも流すか

## 完了条件

- `ToolCallArgsDelta` の内容が active 領域のフレーム内でライブに表示される。
- 1 回のツール呼び出しが 1 フレームとして表示され、start → args streaming → done → result の遷移が同じ領域の更新として見える。
- 完了フレームは次のブロック到着時に自動で `insert_before` に格下げされ、スクロールバックで immutable な履歴として残る。
- 並列ツール呼び出しでフレームが混ざらない（tool_use_id で正しく振り分けられる）。
- `ToolResult` が欠落したまま turn が終わった場合でもフレームが Incomplete として履歴に残り、panic しない。
- セッション再開時に過去のツール呼び出しがフレーム形式で履歴に復元される。

## 範囲外

- **フルスクリーン TUI への移行**。inline viewport + insert_before の組合せを維持する。
- **ツール結果そのもののリッチレンダリング**（Markdown 整形・シンタックスハイライト・diff 表示等）。本チケットはコンテナと状態遷移に集中する。
- **ツール実行のキャンセル / 介入 UI**。ユーザーがフレームから操作を起こす仕組みは別チケット。
- **プロバイダ側イベントスキーマの変更**。既存の `ToolCallStart` / `ToolCallArgsDelta` / `ToolCallDone` / `ToolResult` をそのまま使う。
