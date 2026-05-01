# Bash ツール

## 背景

builtin-tools チケットで Read/Write/Edit/Glob/Grep の5ツールは実装済み。
Bash ツールは子プロセスが直接 fs を触るため ScopedFs では保護できず、
Permission 層（deny/allow ルール）との統合が前提。

## 実装内容

- コマンド実行（`tokio::process::Command`）
- タイムアウト（`timeout` パラメータ、デフォルト 120秒、最大 600秒）
- stateless: 各呼び出しは workspace root から fresh start。pwd は session を跨いで保持しない（`cd <dir> && cmd` のように chain させる運用。Claude Code と同方針）
- stdout/stderr の結合出力
- ToolOutput の summary（コマンド + exit code）+ content（出力テキスト）
- 出力ハンドリング: 短い場合（≤ 80行 & ≤ 12 KiB）はインラインで返す。それを超えたら full output を `<runtime_dir>/<pod_name>/bash-output/` 配下のファイルに退避し、tail 80行 + ファイルパスを返却。Worker の `ToolOutputLimits` (default 16 KiB) が末尾を切り捨てる挙動を回避するため
- 退避ファイルは scope に `allow(Read)` で追加するので `Read` ツールで読める。Pod 終了で `RuntimeDir::Drop` がまとめて掃除、session 終了でも `BashTool::Drop` が個別削除

## Scope との関係

Bash の子プロセスは ScopedFs を経由しない。Scope による保護は不可能。

代わりに:
- `PreToolCall` Hook + Permission ルール（`[permission]` マニフェストセクション）で制御
- Permission 未実装の間は制約なしで動作

## 依存チケット

- [permission-extension-point.md](permission-extension-point.md) — deny/allow ルールによる Bash コマンド制御

## Review
- 状態: Approve with follow-up（Round 2）
- レビュー詳細: [./bash-tool.review.md](./bash-tool.review.md)
- 日付: 2026-05-01（Round 2: 2026-05-01）
- Round 2 残課題:
  - ~~チケット本文「作業ディレクトリの永続」の記述を stateless 仕様に更新~~（解消済み）
  - `crates/tools/src/lib.rs:51` の broken intra-doc link 修正（別コミットで対応）
  - TUI `render_default` 回帰テストは別チケットへ切り出し
