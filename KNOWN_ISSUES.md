# Known Issues

Ticket を切るほどではないが、次に近所を触るときに合わせて拾いたい小粒な所見の置き場。

## 運用

- 1 項目 = 出典 (file:line) + 症状 (一文) + トリガー (いつ拾うか、一文)
- 関連 ticket があれば `→ [tickets/foo.md]` でリンク
- 修正したら同じコミットで該当エントリを削除する (履歴は git)
- ここに溜める基準: 「ticket は重い」「だが忘れたら次の触り手が踏む」もの。明確に作業すべきものは ticket 化する

## エントリ

- `crates/tui/src/app.rs:478-485` — bad workflow slug を含む `Method::Run` 送信時、`Event::UserMessage` の早期 broadcast で `turn_index += 1` されターンヘッダだけ残る ("ghost turn header")。次に TUI のターンヘッダ / エラー表示周りを触るときに整理。→ [tickets/pod-input-validate-internalize.md] の review 由来。
- `crates/pod/src/controller.rs:944` — `worker_error_code` で `PodError::WorkflowResolve(_) => InvalidRequest` が post-commit な resolve エラー (`KnowledgeNotFound` 等) にも適用される。意味論的には妥当方向だが、resolve 系のエラー粒度を分けたくなったタイミングで再評価。
