# メモリ機構: extract / consolidation 監査ログ

## 背景

Memory extract と consolidation は、session log / staging / memory / knowledge をまたいで自律的に記録を作成・更新する。動作結果は最終的には `memory/*` / `knowledge/*` と git diff で確認できるが、実行中に何が起きているか、人間が `tail -f` 相当で追える観測面がない。

特に consolidation は rewrite / merge / trim / drop を許可するため、あとから最終 diff だけを見るよりも、run lifecycle と memory tool 操作の監査ログがある方が挙動を理解しやすい。

## 方針

workspace の `.insomnia/memory/_logs/` に append-only な JSONL ログを出す。拡張子は `.log` とし、1 行 1 event で `tail -f` できる形式にする。

ログは source of truth ではなく監査・観測用である。正本は従来通り session log、staging、`memory/*`、`knowledge/*`、git diff とする。consolidation の入力や memory 採択判断がこのログに依存する設計にはしない。

## 要件

- `.insomnia/memory/_logs/` 配下に JSONL `.log` を append する仕組みを追加する。
  - 具体的なローテーション単位は実装で決めてよいが、`tail -f` しやすい最新ログ導線を用意する。
  - 例: 日次 `memory-YYYY-MM-DD.log`、または run 単位 log + `current.log`。
- extract の lifecycle event を記録する。
  - started / completed / failed
  - run id
  - session id と処理対象 range
  - staging に書いた件数・path / id の概要
  - 取得できる場合は model / usage
- consolidation の lifecycle event を記録する。
  - started / completed / failed
  - run id
  - consumed staging id list または count
  - 書き込み概要
  - 取得できる場合は model / usage
- memory / knowledge 専用 write/edit/delete tool の操作を audit event として記録する。
  - `kind`, `slug`, `path`, `op`, `status`
  - 可能なら before / after hash
  - Linter failure も失敗 event として残す
- ログは通常の LLM context に暗黙注入しない。
  - 人間が `tail -f` するための観測面とする。
  - LLM が読む必要がある場合は通常の tool read 経由にし、history に残る経路を使う。
- `_staging` とは分離し、consolidation の処理対象にしない。

## 完了条件

- extract run の開始・終了・失敗が `.insomnia/memory/_logs/*.log` に JSONL で追記される。
- consolidation run の開始・終了・失敗が同ログに JSONL で追記される。
- memory / knowledge 専用 tool による write/edit/delete と Linter failure が同ログに JSONL で追記される。
- 最新ログを `tail -f` する運用手順がドキュメントまたはコメントから分かる。
- ログが memory / knowledge の正本や consolidation 入力として扱われない。

## 範囲外

- ログ viewer UI。
- ログを使った自動 rollback。
- ログを使った Knowledge 採択 / 整理判定。
- session-store の正本イベント形式の変更。
