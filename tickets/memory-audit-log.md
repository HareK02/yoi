# メモリ機構: extract / consolidation 監査ログ

## 背景

Memory extract と consolidation は、session log / staging / memory / knowledge をまたいで自律的に記録を作成・更新する。動作結果は最終的には `memory/*` / `knowledge/*` と git diff で確認できるが、実行中に何が起きているか、人間が `tail -f` 相当で追える観測面がない。

特に consolidation は rewrite / merge / trim / drop を許可するため、あとから最終 diff だけを見るよりも、run lifecycle と memory tool 操作の監査ログがある方が挙動を理解しやすい。

現状は、そもそも extract / consolidation worker が起動したのか、起動したならどの理由で完了・skip・失敗したのかをユーザーが把握しづらい。memory の価値評価をする前提として、worker lifecycle と outcome reason を記録し、TUI 上でも直近の稼働が見える必要がある。

## 方針

workspace の `.insomnia/memory/_logs/` に append-only な JSONL ログを出す。拡張子は `.log` とし、1 行 1 event で `tail -f` できる形式にする。

ログは source of truth ではなく監査・観測用である。正本は従来通り session log、staging、`memory/*`、`knowledge/*`、git diff とする。consolidation の入力や memory 採択判断がこのログに依存する設計にはしない。

TUI の actionbar は最新の memory worker event を短く表示する通知面として使う。actionbar 表示は補助的な観測面であり、正本や再処理入力にはしない。

## 要件

- `.insomnia/memory/_logs/` 配下に JSONL `.log` を append する仕組みを追加する。
  - 具体的なローテーション単位は実装で決めてよいが、`tail -f` しやすい最新ログ導線を用意する。
  - 例: 日次 `memory-YYYY-MM-DD.log`、または run 単位 log + `current.log`。
- extract / consolidation worker の run lifecycle event を共通形式で記録する。
  - `started` / `completed` / `skipped` / `failed` / `cancelled`
  - `run_id`
  - `worker`: `memory_extract` / `memory_consolidation`
  - `trigger`: `session_end` / `turn_threshold` / `token_threshold` / `staging_backlog` / `idle` / `manual` / `startup_recovery` / `unknown` 等
  - `reason`: 完了・skip・失敗理由。例: `candidates_found`, `no_new_session_range`, `no_candidates`, `no_staging`, `no_record_changes`, `records_updated`, `linter_failed`, `model_error`, `scope_error`, `interrupted`
  - 取得できる場合は model / usage
- extract の lifecycle event には処理対象を記録する。
  - session id と処理対象 range
  - staging に書いた件数・path / id の概要
  - 候補なしで正常終了した場合も `skipped` または `completed` + `reason` で明示する
- consolidation の lifecycle event には staging と書き込み結果を記録する。
  - consumed staging id list または count
  - write / edit / delete / drop / merge / trim の件数概要
  - staging が空、採択なし、差分なしの場合も `skipped` または `completed` + `reason` で明示する
- memory / knowledge 専用 write/edit/delete tool の操作を audit event として記録する。
  - `kind`, `slug`, `path`, `op`, `status`
  - 可能なら before / after hash
  - Linter failure も失敗 event として残す
- memory / knowledge read/query の明示利用も usage/audit event として追えるようにする。
  - `kind`, `slug`, `query`, `status` など、取得できる範囲でよい
  - 既存の `_usage/events.jsonl` と統合するか、audit log へ寄せるかは実装で選んでよい
- TUI actionbar に直近の memory worker event を短く表示する。
  - 例: `MEMORY extract running…`
  - 例: `MEMORY extract done: staged 3`
  - 例: `MEMORY consolidation done: wrote 1, merged 2, dropped 4`
  - 例: `MEMORY consolidation skipped: no staging`
  - 例: `MEMORY extract failed: model_error`
  - 表示は永続ログの補助であり、LLM context へ暗黙注入しない
- ログは通常の LLM context に暗黙注入しない。
  - 人間が `tail -f` するための観測面とする。
  - LLM が読む必要がある場合は通常の tool read 経由にし、history に残る経路を使う。
- `_staging` とは分離し、consolidation の処理対象にしない。

## 完了条件

- extract run の開始・終了・skip・失敗・cancel が `.insomnia/memory/_logs/*.log` に JSONL で追記される。
- consolidation run の開始・終了・skip・失敗・cancel が同ログに JSONL で追記される。
- 各 run event から `trigger` と outcome `reason` が分かる。
- memory / knowledge 専用 tool による write/edit/delete と Linter failure が同ログに JSONL で追記される。
- memory / knowledge read/query の明示利用が usage/audit event として確認できる。
- TUI actionbar に直近の memory worker event が短く表示される。
- 最新ログを `tail -f` する運用手順がドキュメントまたはコメントから分かる。
- ログが memory / knowledge の正本や consolidation 入力として扱われない。

## 範囲外

- ログ viewer UI。
- ログを使った自動 rollback。
- ログを使った Knowledge 採択 / 整理判定。
- session-store の正本イベント形式の変更。
