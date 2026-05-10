# Review: Memory prompts: project-specific ticket shadow policy を外す

## 前提・要件の確認

- bundled memory prompts から ticket / TODO 固有の shadow 禁止を削除: OK。Phase 1 prompt は ticket / TODO / worktree / branch / commit 等の列挙を消し、authoritative project records 一般へ置き換えている（`.worktree/memory-prompts-remove-ticket-policy/resources/prompts/internal/memory_extract_system.md:27-32`）。Phase 2 prompt も `git is authoritative` と ticket/TODO 固有列挙を削除し、issue trackers / task boards / planning documents 等の一般表現へ置換している（`.worktree/memory-prompts-remove-ticket-policy/resources/prompts/internal/memory_consolidation_system.md:21-22`）。
- `git is authoritative` の扱いを一般ユーザー向けに弱める: OK。正確な内容の authoritative record を memory が mirror しないという原則は残しつつ、durable project-management facts / workflow constraints / prioritisation rationale / process decisions / cross-item lessons は保存可能と明示している（`memory_extract_system.md:27-32`, `memory_consolidation_system.md:22`）。
- Phase 1 / Phase 2 の整合: OK。Phase 1 の抽出除外条件と Phase 2 の統合時ルールがどちらも「raw status ledger は作らないが、持続的な抽象は採用可」という同じ方針になっている（`memory_extract_system.md:29-32`, `memory_consolidation_system.md:22`）。
- default prompt が特定 project-management 手法を前提にしないことの文書化: OK。`docs/plan/memory-prompts.md` の共通原則に「特定の project-management 手法を前提にしない」が追加され、ticket / TODO 固有の旧記述が一般化されている（`.worktree/memory-prompts-remove-ticket-policy/docs/plan/memory-prompts.md:19-20`）。
- 既存テスト / prompt catalog 検査: OK。`cargo test -p pod prompt::catalog` を実行し、11 tests passed。

## アーキテクチャ・スコープ

- 変更は bundled prompt 2 件と plan doc 1 件のみで、memory の file format / linter / Phase 1 / Phase 2 実行機構には触れていない。ticket の範囲外を守っている。
- prompt 文言の責務分離は妥当。INSOMNIA 固有の ticket shadow 回避を default prompt から外し、workspace / user prompt override 側へ逃がせる状態にしている。
- `docs/plan/memory-prompts.md` も同時に更新しており、実装 prompt と計画文書が乖離していない。

## 指摘事項

### Blocking

なし。

### Non-blocking / Follow-up

なし。

### Nits

- `docs/plan/memory-prompts.md:68` に「git で可逆」という表現が残っているが、これは tidy phase の drop 判断に関する一般的な VCS 可逆性の話で、ticket / TODO 固有の shadow policy ではないため本チケットの完了を妨げない。

## 判断

Approve — ticket / TODO 固有の bundled prompt 方針は削除され、一般的な authoritative record 非重複方針へ置き換わっている。変更範囲も prompt / plan に限定され、既存 prompt catalog tests も通っている。
