# セッション内 Task ツール

## 背景

長めのタスクを LLM に進めさせる際、Claude Code / OpenCode が備える「セッション内 TODO / Task リスト」相当の機構が無いため、エージェントが自分の作業計画を構造化された形で保持・更新できない。Reasoning や text 出力の中で擬似的に TODO を書くことはできるが、

- compact を跨ぐと完全に消える
- ツール結果ではないため、状態の作成・更新・削除の規約が決まらず、意図と乖離した「やったつもり」を起こしやすい
- 安定した識別子が無いため、後から特定 entry を更新できない

この用途のために、セッション内に正規化された Task リストを保持し、compact を跨いで保存される専用ツールを導入する。

「LLM がツールを使わなくなった場合のサボり防止」を目的とした注意機構（`<system-reminder>` の自動注入）は本チケットの範囲外。`tickets/session-todo-reminder.md` で別途扱う。

## 方針

- **保存先は `tools` 層の session-lifetime 状態**。`Tracker` と同じ生存スコープで `Pod` が所有。`Arc<Mutex<TaskStore>>` ベースの store を tool に注入する
- **Task は差分操作**。全置換ではなく `TaskCreate` / `TaskUpdate` によって TaskStore を逐次更新する
- **永続化は専用レーンを持たない**。`tool_call.arguments` がセッションログに既に乗っているため、resume 時には履歴 replay の中で `TaskCreate` / `TaskUpdate` を順に再適用すれば状態が復元される

## Task データモデル

各 Task entry は以下の4フィールドを持つ。

- `taskid`: store が自動採番する整数 ID。`TaskCreate` ごとに単調増加する
- `status`: `pending | inprogress | completed | deleted`
- `subject`: 1行程度の短い件名
- `description`: 詳細説明

状態遷移の通常ルートは `pending -> inprogress -> completed` とする。完了ではなく一覧から取り下げたい場合は、物理削除ではなく `TaskUpdate` で `status = deleted` を設定する。

## 要件

### `TaskCreate` ツール

- 入力は `subject` / `description` のみ
- TaskStore の末尾に新規 entry を push する
- `taskid` は TaskStore が自動で割り当てる。LLM は指定できない
- 初期 `status` は `pending`
- 戻り値は作成された Task と、更新後の TaskStore スナップショット概要を summary に含める

### `TaskList` ツール

- 入力なし
- 現在の TaskStore スナップショットを返す
- `taskid` / `status` / `subject` / `description` を含める
- `completed` / `deleted` も含め、store に残っている Task をすべて列挙する

### `TaskGet` ツール

- 入力は `taskid`
- 指定された Task を1件返す
- 存在しない `taskid` はエラーにする

### `TaskUpdate` ツール

- 入力は `taskid` と、更新する `status` / `subject` / `description` のいずれか
- 少なくとも1フィールドは更新指定が必要
- `taskid` は変更できない
- `status` は `pending | inprogress | completed | deleted` のいずれか
- 存在しない `taskid` はエラーにする
- 削除は `status = deleted` への更新として表現し、entry は store から取り除かない
- 戻り値は更新された Task と、更新後の TaskStore スナップショット概要を summary に含める

### Resume 時の復元

- `Pod::resume` / restore の履歴 replay 中に `TaskCreate` / `TaskUpdate` の `tool_call.arguments` を観測したら、TaskStore に順に再適用する
- `TaskCreate` は replay 時も自動採番を行う。ログ上の call 順序が保持されるため、通常の replay では元の `taskid` が再現される
- `TaskUpdate` は replay 済みの TaskStore に対して適用する
- 専用 LogEntry / Persistence 型は追加しない（`Tracker` と同じ方針）
- `tool_call.arguments` のフォーマットが現在の schema と乖離した場合（旧バージョンのログ）は、その call を無視してよい

### Compact 跨ぎ

- compact 起動時、Pod は現在の TaskStore **全スナップショット**を compact worker context に渡す
- compact worker は summary を書く際、TaskStore を参照できる。未完了 Task を summary 文に取り込める情報源として参照する（強制ではない）
- compact worker に Task 編集権限は与えない（作成・更新・削除はしない）
- compact 後の新セッションでも TaskStore は空にせず、compact 前の TaskStore **全件**を維持する。`completed` / `deleted` も含めて保持する
- compact 後の新セッション開始時、Pod は **`mark_read_required` と同じ system message 注入レーン**に TaskStore 全スナップショットを1メッセージとして注入する
- compact 完了後、Pod は新セッションの継続前に必ず `TaskList` を発火させ、LLM が compact 後の TaskStore 状態を tool result として再確認できるようにする

## 完了条件

- `TaskCreate` / `TaskList` / `TaskGet` / `TaskUpdate` が builtin tool として登録され、Pod で利用できる
- LLM が `TaskCreate` を呼ぶと TaskStore に pending Task が末尾追加され、自動採番された `taskid` が返る
- LLM が `TaskUpdate` を呼ぶと既存 Task の `status` / `subject` / `description` が更新される
- `TaskList` / `TaskGet` で TaskStore の状態を確認できる
- セッションを resume すると、履歴中の `TaskCreate` / `TaskUpdate` replay により TaskStore が復元される
- compact を跨いでも、TaskStore 全件が維持され、新セッション冒頭の system message と `TaskList` 発火により再提示される
- 単体テストで TaskCreate / TaskUpdate / TaskList / TaskGet の挙動、replay 復元がカバーされる

## 範囲外

- 注意機構（`<system-reminder>` の自動注入による「サボり防止」）→ `tickets/session-todo-reminder.md`
- Task の階層・優先度・タグ
- TUI / GUI での Task 状態の可視化（ツール呼び出しのイベントは既に流れているので、クライアント側で表示するかは別軸）
- Task の永続化を専用 LogEntry に分離する設計（現方針は tool_call replay と compact 時 snapshot 維持で復元、追加レーン不要）
- 複数 Pod 間で Task を共有する仕組み

## 参照

- 設計指針: `CLAUDE.md`（最小の構造化 / 概念の追加は不在が問題になってから）
- 参考実装: Claude Code の TodoWrite、OpenCode の todo tool
- 関連: `crates/tools/src/tracker.rs`（session-lifetime 状態の前例）、`crates/pod/src/compact/worker.rs`（auto-injection レーン）
- 後続: `tickets/session-todo-reminder.md`（注意機構）
