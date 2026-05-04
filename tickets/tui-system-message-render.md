# TUI で role:system の system message を表示する

## 背景

Pod は user 入力の `@<path>` chip / `/<slug>` chip を submit 時に展開し、`Item::system_message` を `pending_attachments` 経由で worker.history に commit している:

- `@<path>` ライブ submit: `Pod::run` → `resolve_file_refs` (crates/pod/src/pod.rs:825,852) → `PodFsView::resolve_file_ref` (crates/pod/src/fs_view.rs:119) で `[File: <path>]\n<body>` を生成
- compact worker の auto-read: `mark_read_required` で nominate された再読対象が `PodFsView::render_auto_read` (crates/pod/src/fs_view.rs:84) で `[Auto-read file: <path>:<range>]\n<body>` として乗る
- `/<slug>` ライブ submit: `Pod::run` → `resolve_workflow_invocations` (crates/pod/src/pod.rs:826,876) → `crate::workflow::resolve_workflow_invocation` (crates/pod/src/workflow/mod.rs:74) で `[Workflow /<slug>]\n<body>` と requires Knowledge 毎の `[Workflow /<slug> requires Knowledge #<req>]\n<body>` を生成

いずれも `PodInterceptor::on_prompt_submit` (crates/pod/src/ipc/interceptor.rs:114) で `PromptAction::ContinueWith(extras)` 経由で worker.history に commit され、history.json に永続化されている (CLAUDE.md「許される加工」原則に整合)。

## 問題

LLM のコンテキストには乗っているが TUI には何も出ない。TUI 側で role:system の Item が表示経路に乗っていないため:

1. **ライブ側**: 解決済み system message を運ぶ broadcast event が `protocol::Event` に存在しない (crates/protocol/src/lib.rs:204–326)。`UserMessage` で `@<path>` / `/<slug>` chip 自体は表示されるが、解決された本体は出ない。失敗時のみ `Alert` で出るが、`Alert` はユーザー向け一過性通知で永続化されない (crates/protocol/src/lib.rs:328–348) ため表示経路として不適切。
2. **履歴復元側**: `App::restore_history` (crates/tui/src/app.rs:650–702) の match が `role: "user"` / `"assistant"` 以外を `_ => {}` で握り潰す。history.json に system role で残っているのに resume 時に消える。

結果として「`@<path>` や `/<slug>` を submit したのに、本当に読まれたのか / 何が context に乗ったのか TUI からは判別できない」状態になっている。

## 要件

- `Item::system_message` (role:system) を user / assistant メッセージと並列のログ要素として TUI に表示する**一般的な仕組み**を入れる。種別ごとの個別パッチではなく、role:system が来たら一律で表示経路に乗せる形にする。
- 仕組みとして最小限カバーすべき system message:
  - `[File: <path>]` (ライブ `@<path>`)
  - `[Auto-read file: <path>:<range>]` (compact 後の auto-read)
  - `[Workflow /<slug>]`
  - `[Workflow /<slug> requires Knowledge #<req>]`
- 表示の単一の情報源は永続化された history (= history.json の role:system Item)。ライブ submit 時 / 履歴復元時 / 別 client subscribe 時 の三経路で同じ Block バリアントを通る。
- 表示は本文プレビュー（数行 + 残行数 / 切り詰め注記）程度で良い。`[Auto-read file: ...:<range>]` の range ラベルは可視化する。workflow 本体と requires Knowledge は同じ workflow 起動に紐づく一連と分かる粒度で識別できる。
- 解決失敗時の従来経路は維持: `@<path>` は `Alert`、workflow は user-invocation エラーとして即座に Pod から返る (`Pod::validate_workflow_invocations`)。

## 範囲外 / 非目標

- `<system-reminder>` 注入機構そのものの汎用化や、notify_wrapper 適用後の本文表示。これらは別所（`session-todo-reminder` 等）。本チケットは**表示側の仕組みのみ**で、将来 `<system-reminder>` 系が role:system Item として history に乗るようになれば、同じ表示経路にそのまま流れる前提。
- 表示形式の凝った装飾（シンタックスハイライト / 折り畳み UI）。最初は素のテキストプレビューで十分。
- `model_invokation: true` のみの workflow（user_invocable=false）の表示は対象外。

## 完了条件

- `@<path>` を submit すると、user message ブロックに続けて自動読み取り結果が TUI に出る。本文プレビューが視認できる。
- `/<slug>` を submit すると、workflow 起動結果のログ要素が TUI に出る。`requires` がある場合は Knowledge 注入もそれと分かる形で出る。
- compact 後の resume で `[Auto-read file: ...]` が同じログ要素として復元・表示される。
- 別 client が後から subscribe して `Event::History` を受けた場合も、同じログ要素として描画される。
- ライブ event と history 復元の表示が一致する（同じ Block バリアントを通る）。
- 解決失敗時の従来経路（`Alert` / user-invocation エラー）は維持される。

## Review
- 状態: Approve
- レビュー詳細: [./tui-system-message-render.review.md](./tui-system-message-render.review.md)
- 日付: 2026-05-04
