# TUI から任意タイミングで Compact を実行する system command

## 背景

Compact は現在、manifest の compaction 設定と token 閾値に基づいて Pod 側で自動実行される。TUI は `CompactStart` / `CompactDone` / `CompactFailed` のイベントを受けて履歴上に表示できるが、ユーザーが TUI から任意のタイミングで Compact を明示実行する導線はない。

一方、TUI の入力欄にはすでに以下の sigil がある。

- `@`: FileRef / clipboard などの添付
- `#`: KnowledgeRef
- `/`: WorkflowRef

そのため、Compact のような Pod / TUI の制御操作を `/compact` や `#compact` として扱うと、既存の参照・補完体系と衝突する。通常の user message として LLM に送る入力とも明確に区別する必要がある。

## ゴール

TUI から、会話本文を送らずに任意タイミングで Compact を発火できるようにする。あわせて、Compact だけの特例ではなく、将来の TUI / Pod 制御操作にも使える system command の入口を用意する。

## 要件

### System command の入力体系

TUI に、通常の user message / `@` / `#` / `/` とは衝突しない system command の入力体系を追加する。

- `/` と `#` は使わない
- `@` FileRef とも衝突しない
- 入力された system command は LLM への user message として送られない
- command の発火は UI 上で見分けられる
- 未知 command や引数不正は、Pod に送らず TUI 側でユーザーに診断を出す

記法にするか、専用モードにするか、keybinding から command palette 的に起動するかは本チケット内で確定してよい。ただし既存の submit / completion / paste / chip 化の入力体験を壊さないこと。

### Manual Compact

System command から Compact を明示実行できるようにする。

- Idle 中に実行できる
- Run / Pause / spawn dialog / session picker など、実行できない状態では明確に拒否または無効化する
- 実行時に通常の user message は追加しない
- 既存の Compact lifecycle 表示（`CompactStart` / `CompactDone` / `CompactFailed`）と整合する
- Compact 完了後の session rotation / history restore / status 表示が、auto compact と同じ前提で動く
- compaction 設定が無い、または compactor model が解決できない場合の診断を TUI に出す

### Protocol / Pod 側

必要であれば、client → Pod の typed control method を追加する。

- `Method::Run` に特殊文字列を流す形にはしない
- Compact 実行は Pod 側の既存 compact 経路を使い、auto compact と履歴・store・broadcast のセマンティクスを分岐させない
- concurrent run 中の compact 要求、重複 compact 要求、shutdown 中の要求などの状態遷移を明確に扱う

## 完了条件

- TUI から任意タイミングで Compact を明示実行できる
- Compact 発火に使う system command の記法またはモードが、`@` / `#` / `/` と衝突していない
- system command が LLM への user message として history に混入しない
- 実行不可状態や失敗時の診断が TUI に表示される
- auto compact と同じ lifecycle event / history rotation 前提で表示が更新される
- protocol / Pod / TUI の必要なテストが追加されている

## 範囲外

- Compact の要約品質や prompt の変更
- compaction 閾値・retained token など manifest 設定の再設計
- slash command と WorkflowRef の意味論変更
- system command の豊富なコマンド群追加（本チケットでは Compact を最初の利用者として入口を作る）
