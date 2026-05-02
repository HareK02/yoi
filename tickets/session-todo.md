# セッション内 TODO ツール

## 背景

長めのタスクを LLM に進めさせる際、Claude Code / OpenCode が備える「セッション内 TODO リスト」相当の機構が無いため、エージェントが自分の作業計画を構造化された形で保持・更新できない。Reasoning や text 出力の中で擬似的に TODO を書くことはできるが、

- ターンを跨いだとき直近の TODO 状態が context から押し出される
- compact を跨ぐと完全に消える
- ツール結果ではないため、状態の上書き・部分更新の規約が決まらず、意図と乖離した「やったつもり」を引き起こす

この用途のために、セッション内に正規化された TODO リストを保持し、ターンごとに LLM へ最新状態を再提示（注意機構）し、compact を跨いで保存される専用ツールを導入する。

## 方針

- **保存先は `tools` 層の session-lifetime 状態**。`Tracker` と同じ生存スコープで `Pod` が所有。`Arc<Mutex<Vec<TodoItem>>>` ベースの `TodoStore` を tool に注入する
- **永続化は専用レーンを持たない**。`tool_call.arguments` がセッションログに既に乗っているため、resume 時には履歴 replay の中で最後の `todo_write` 引数を `TodoStore` に再適用すれば状態が復元される
- **注意機構は `pre_llm_request` Interceptor**。直近の user message に `<system-reminder>` ブロックを揮発的に append するだけ。履歴・ログには載せない
- **system-reminder 注入の汎用化はやらない**。利用者が TODO 1個しかない段階で抽象を立てない（CLAUDE.md「概念の追加は不在が問題になってから」）。ただし「タグ形式は `<system-reminder>...</system-reminder>` で揃える」「履歴は汚さない」の2点は本実装で確立し、将来の追加機構が同じ規約に乗れるようにする

## 要件

### `todo_write` ツール

- 入力は TODO リスト全体（全置換）。差分更新は受けない
- 各エントリは `id` / `content` / `status (pending | in_progress | completed)` の 3 フィールド
- `id` は LLM 側が一貫して採番できる文字列。同 id があれば置換、なければ新規。順序は配列順を信頼
- 戻り値は更新後のスナップショットを summary に含める（次ターンで再確認可能）
- 読み出し専用ツール（`todo_read`）は作らない。注意機構と tool result snapshot で代替

### Resume 時の復元

- `Pod::resume` の履歴 replay 中に `todo_write` の `tool_call.arguments` を観測したら、`TodoStore` を引数値で上書き
- 専用 LogEntry / Persistence 型は追加しない（`Tracker` と同じ方針）
- `tool_call.arguments` のフォーマットが `todo_write` の引数 schema と乖離した場合（旧バージョンのログ）は、その call を無視してよい

### Compact 跨ぎ

- compact 起動時、Pod は現在の `TodoStore` スナップショットを compact worker context に渡す
- compact worker は summary を書く際、未完了 TODO を summary 文に取り込める情報源として参照する（強制ではない）
- compact 後の新セッション開始時、Pod は **`mark_read_required` と同じ system message 注入レーン**に「未完了 TODO スナップショット」を 1 メッセージとして注入する
- 新セッションは空の `TodoStore` で始まる。次に LLM が `todo_write` を呼び出した時点で再構築される（system message に書かれたスナップショットがその拠り所）
- compact worker に TODO 編集権限は与えない（消去・縮約はしない）

### 注意機構（Interceptor）

- `pre_llm_request` で `Vec<Item>` を受け取り、未完了 TODO（`pending` または `in_progress`）が 1 件でも存在する場合に発動
- 直近の user message の content（または content[最終 text part]）の末尾に `<system-reminder>` ブロックを append
- ブロック内には現在の TODO リストを、status を含む簡潔な形式で列挙
- 履歴 (`Worker` の保持する `Vec<Item>`) は変更しない。リクエスト送信時の Vec のみ加工
- TODO が空の場合は何も差し込まない

## 完了条件

- `todo_write` ツールが builtin tool として登録され、Pod で利用できる
- LLM が `todo_write` を呼ぶと TodoStore が更新され、その後の `pre_llm_request` で system-reminder として LLM に再提示される
- セッションを resume すると、最後の `todo_write` の状態から再開される
- compact を跨いでも、未完了 TODO が新セッション冒頭の system message として残る
- system-reminder の注入は揮発的で、`get_history` / セッションログには現れない
- 単体テストで `todo_write` の更新挙動 / replay 復元 / Interceptor の差し込みがカバーされる

## 範囲外

- 差分更新 API（add / remove / patch）。全置換のみで十分
- TODO 階層・優先度・タグ
- TUI / GUI での TODO 状態の可視化（ツール呼び出しのイベントは既に流れているので、クライアント側で表示するかは別軸）
- system-reminder 注入機構の汎用化（`TODO.md` に立項済み、別途検討）
- TODO の永続化を専用 LogEntry に分離する設計（現方針は tool_call replay で復元、追加レーン不要）
- 複数 Pod 間で TODO を共有する仕組み

## 参照

- 設計指針: `CLAUDE.md`（最小の構造化 / 概念の追加は不在が問題になってから）
- 参考実装: Claude Code の TodoWrite、OpenCode の todo tool
- 関連: `crates/tools/src/tracker.rs`（session-lifetime 状態の前例）、`crates/pod/src/compact/worker.rs`（auto-injection レーン）
