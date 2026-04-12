# Bash ツール

## 背景

builtin-tools チケットで Read/Write/Edit/Glob/Grep の5ツールは実装済み。
Bash ツールは子プロセスが直接 fs を触るため ScopedFs では保護できず、
Permission 層（deny/allow ルール）との統合が前提。

## 実装内容

- コマンド実行（`tokio::process::Command`）
- タイムアウト（`timeout` パラメータ、デフォルト 120秒、最大 600秒）
- 作業ディレクトリの永続（ツール内部で `pwd` 状態を保持、`cd` で変更可能）
- stdout/stderr の結合出力
- ToolOutput の summary（コマンド + exit code）+ content（出力テキスト）

## Scope との関係

Bash の子プロセスは ScopedFs を経由しない。Scope による保護は不可能。

代わりに:
- `PreToolCall` Hook + Permission ルール（`[permission]` マニフェストセクション）で制御
- Permission 未実装の間は制約なしで動作

## 依存チケット

- [permission-extension-point.md](permission-extension-point.md) — deny/allow ルールによる Bash コマンド制御
