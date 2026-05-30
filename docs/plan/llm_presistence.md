# LLM 履歴永続化 plan（superseded）

> Status: superseded / historical note.
>
> この文書は、`session-store` / segment log / Pod metadata 分離が入る前の検討メモ。現在の実装仕様として読まないこと。現行仕様は `docs/architecture.md` の「セッション永続化」と `crates/session-store` / `crates/pod-store` / `crates/pod` を正とする。

## 現在の要点

- 会話・worker replay の authority は `session-store` の append-only JSONL segment log。
- fresh conversation は新しい `SessionId` を作る。
- compact / fork / rewind は同じ `SessionId` 配下の `SegmentId` を切り替える。
- segment 出自は `LogEntry::SegmentStart` 内の `SegmentOrigin`（`compacted_from` / `forked_from`）で表す。
- Pod 名からの current state は `pod-store` metadata が持つ。
  - active `(SessionId, SegmentId)` pointer
  - `resolved_manifest_snapshot`
  - spawned child delegation / reclaim state
- runtime socket path / registry mirror は live/derived state であり、conversation history や Pod-name current state の durable authority ではない。

## Superseded な旧前提

この旧 plan は以下の前提を含んでいたため、現在の仕様とは一致しない。

- session を単一ファイル・単一系列として扱う前提。
- compact/fork で新 SessionId を作る前提。
- entry hash を replay lineage の主参照にする前提。
- Pod 名 current state と conversation log authority を分離しない前提。

履歴としては有用だが、実装時の根拠には使わない。新しい変更を検討する場合は、まず current code と work item を読むこと。
