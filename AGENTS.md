# 開発時の入口

変更量を最小にするために設計を歪めたり、不必要な後方互換性を作ったりしない。長期的な保守性と型安全性を優先する。

## 作業条件に応じて読む規則

該当する作業の前に、次の文書を読み、その規則に従う。

- 文書・説明コメントを追加、変更、削除する → [文書管理規則](docs/README.md)
- LLM向けPromptを追加・変更する → [Prompt記述規則](docs/development/llm-prompting.md)
- 正本、権限、状態の競合制御、LLM入力・履歴を扱う → [正本・権限・実行の原則](docs/principle/authority-and-history.md)
- 永続操作、外部副作用、再試行・復旧を設計する → [永続操作の原則](docs/principle/durable-operations.md)
- テストを追加・変更する → [テスト設計規則](docs/development/rust-testing-strategy.md)
- 変更を検証する・作業を完了する → [検証規則](docs/development/validation.md)
- 保存schemaやmigrationを変更する → [保存schema変更の規則](docs/development/workspace-schema-migrations.md)
- Webの画面・操作を設計する → [UX設計](docs/principle/ux-design.md)・[Design Language](docs/principle/design-language.md)。描画を変更したら[目視確認規則](docs/development/ui-ux/visual-review.md)にも従う。
- Ticket・Objective・作業記録を扱う → [作業記録と判断の規則](docs/development/work-items.md)
- 稼働環境に関わる作業や、開発中の障壁を報告する → [ドッグフーディングの安全規則](docs/development/dogfooding.md)

## MRの検証記録

検証の選択・実行は上記の[検証規則](docs/development/validation.md)に従い、MergeRequestには次の証拠を残す。文書だけの変更では文書向けの確認とその範囲を明記し、この記録規則の変更自体を理由に全テストスイートの実行を要求しない。

- 検証したsource commit/tree（未commitならdirty snapshotの識別）、実行コマンド・対象スイート、主要な環境・toolchain・features・並列数・キャッシュ状態を記録する。
- 件数とpass / fail / skip / timeout / 未実行を区別する。スイート別と全体のwall-clockを記録し、ビルド時間とテスト実行時間も可能な範囲で分ける。並列実行の個別時間の合計を全体経過時間とせず、測定・分離できない値はその旨を記す。
- 変更前の比較sourceを示し、同等条件での時間・結果の差を記録する。比較できなければ理由を、キャッシュ・負荷などの条件が異なればその差も明記する。failure / slowdownの原因（未確定ならその旨）、対処・残件とアクセス可能なログ参照を残す。
- 修正途中は影響範囲のテストを使い、コード変更のMR最終候補ではリポジトリで定義した標準スイート全体を計測する。標準対象と実行対象を列挙し、一部を全体と称さない。skip・未到達を可視化し、全体を実行できなければ理由と未実行範囲を記す。標準対象・コマンドは現行の定義を確認し、推測で固定しない。
- 同じsource/tree・対象・環境/条件のアクセス可能な検証証拠は参照して再利用できる。再利用と新規実行を区別し、依存・features・生成物・lockfile・shared init等の変更影響を評価して必要な再検証を選ぶ。記録のためだけに同じテストを反復しない。
- 失敗はbaselineで実際に再現 / 回帰 / 環境依存 / 未分類を区別する。未実行・timeout・ビルド失敗をpassやbaselineで実再現した証拠として扱わない。この記録方針だけを理由に速度閾値の自動gateやCIを追加しない。

## 常に守ること

- 自身を実行しているプロセスを停止しない。マージ後の稼働環境の更新は利用者が行う。
- Ticket操作は提供されたtyped toolsとsystem instructionsに従い、CLIや保存先の直接編集で代替しない。
