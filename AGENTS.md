# 開発時の入口

変更量を最小にするために設計を歪めたり、不必要な後方互換性を作ったりしない。長期的な保守性と型安全性を優先する。

## 作業条件に応じて読む規則

該当する作業の前に、次の文書を読み、その規則に従う。

- 文書・説明コメントを追加、変更、削除する → [文書管理規則](docs/README.md)
- 正本、権限、状態の競合制御、LLM入力・履歴を扱う → [正本・権限・実行の原則](docs/principle/authority-and-history.md)
- 永続操作、外部副作用、再試行・復旧を設計する → [永続操作の原則](docs/principle/durable-operations.md)
- テストを追加・変更する → [テスト設計規則](docs/development/rust-testing-strategy.md)
- 変更を検証する・作業を完了する → [検証規則](docs/development/validation.md)
- 保存schemaやmigrationを変更する → [保存schema変更の規則](docs/development/workspace-schema-migrations.md)
- Webの画面・操作を設計する → [UX設計](docs/principle/ux-design.md)・[Design Language](docs/principle/design-language.md)。描画を変更したら[目視確認規則](docs/development/ui-ux/visual-review.md)にも従う。
- Ticket・Objective・作業記録を扱う → [作業記録と判断の規則](docs/development/work-items.md)
- 稼働環境に関わる作業や、開発中の障壁を報告する → [ドッグフーディングの安全規則](docs/development/dogfooding.md)

## 常に守ること

- 自身を実行しているプロセスを停止しない。マージ後の稼働環境の更新は利用者が行う。
- Ticket操作は提供されたtyped toolsとsystem instructionsに従い、CLIや保存先の直接編集で代替しない。
