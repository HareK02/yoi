# AI maintainer 用 WorkItem / Thread 抽象メモ

> Status: mostly implemented / historical design memo.
>
> この文書は `tickets.sh` + `work-items/` MVP の前に書かれた設計メモ。現在の運用上の正本は `AGENTS.md` の Work item / Ticket 運用と、実際の `tickets.sh` / `work-items/{open,pending,closed}`。古い `TODO.md` / `tickets/` 前提は superseded。

## 現在実現済みの部分

- WorkItem 相当: `work-items/{open,pending,closed}/<id>/item.md`
- Thread 相当: `thread.md`
- Event 相当: `tickets.sh comment/review/status/close` が append する thread entry
- Artifact 相当: `artifacts/` directory
- Resolution 相当: close 時の `resolution.md`
- ID: timestamp + slug 形式
- Doctor: `./tickets.sh doctor`

`work-items/` は repo-managed な project coordination record であり、`.insomnia/memory` はその代替ではない。

## 現在も残る設計余地

- Rust crate / Store interface としての WorkItemStore
- LeaseStore / Run tracking / maintainer inbox
- remote maintainer hub / GitHub Issues などへの backend 差し替え
- TUI 統合

これらは必要になった時点で改めて work item 化する。

## 現在の運用参照

- `AGENTS.md` — Work item / Ticket の運用ルール
- `tickets.sh` — 実際の操作コマンド
- `work-items/` — 現在の project-visible coordination data
- `docs/plan/ai-maintainer.md` — AI maintainer workflow の現行設計
