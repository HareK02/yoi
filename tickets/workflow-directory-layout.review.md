# Review: Workflow の物理配置を `.insomnia/workflow` に分離する

対象 commit: `04f1837 feat: Workflowの読み取り位置変更の実装`

## 前提・要件の確認

- **canonical path 変更**: `crates/memory/src/workspace.rs:120-122` で `workflow_dir()` が `insomnia_dir().join("workflow")` を返すようになっており、`.insomnia/workflow/<slug>.md` が canonical。`crates/memory/src/workspace.rs:163` の `classify` も新配置を `RecordKind::Workflow` として返す。OK
- **旧配置は読まない / linter / scope deny も新配置のみ**: `load_workflows` は `layout.workflow_dir()` のみを開く（`crates/memory/src/workflow.rs:184-205`）。`scan_existing` も同様（`crates/memory/src/linter/existing.rs:85-88`）。`deny_write_rules` は memory / knowledge / workflow の 3 ディレクトリ（`crates/memory/src/scope.rs:23-29`）。`workspace::workflow_under_memory_is_invalid_path` と `workflow::workflow_under_memory_is_ignored` で「旧配置は無視される」ことを test 化（`crates/memory/src/workspace.rs:280-286`、`crates/memory/src/workflow.rs:374-389`）。OK
- **linter / WorkflowRegistry / completion / resident workflow**: `lint_workflow` は `existing::scan_existing` 経由で新配置の `requires` 検査を維持。`Linter::lint` が `RecordKind::Workflow` を即座に `WorkflowWriteForbidden` で弾く（`crates/memory/src/linter/mod.rs:103-106`）。`WorkflowRegistry` は新配置の path のみ保持（`crates/memory/src/workflow.rs:246-256`）。pod 側の resolver / resident 広告も `layout.workflow_dir()` 経由で同様に流用される（`crates/pod/src/workflow/mod.rs:150,174,187`、`crates/pod/src/prompt/system.rs:157`）。OK
- **`.insomnia/.gitignore`**: `/memory/_staging/` `/memory/summary.md` `/memory/decisions/` `/memory/requests/` のみを ignore に変更（`.insomnia/.gitignore`）。`git ls-files .insomnia/` で `manifest.toml` / `workflow/auto-maintain.md` / `workflow/worktree-workflow.md` が tracked、`git check-ignore` で generated state のみが ignore されているのを確認した。OK
- **既存 workflow ファイルの移行**: `.insomnia/workflow/auto-maintain.md` と `.insomnia/workflow/worktree-workflow.md` が新配置に存在し、旧 `.insomnia/memory/workflow/` ディレクトリは現存しない。OK
- **ドキュメント / チケット本文の更新**: `docs/plan/workflow.md`、`docs/plan/memory.md`、`crates/memory/src/{schema/workflow.rs,skill.rs,tool/query.rs}` の doc コメント、`crates/pod/src/{pod.rs,prompt/system.rs}` のコメントが新配置に更新済み。チケット本文も「互換読み取り」要件削除と完了条件の更新が反映されている。OK

`cargo test --workspace` も全パス（memory crate 122 件含む）。

## アーキテクチャ・スコープ

- **Workflow の memory subsystem からの分離**: 物理配置のみ memory tree から外し、loader / linter / scope は引き続き memory crate 内で管理する形。これは「Workflow は session-derived state ではないが、project-managed なリソースとして memory 周辺の一貫した検査機構（frontmatter validator、`requires` reference check、scope deny）を共用する」という現状設計とずれていない。新規 crate 切り出しなどの過剰な構造変更を避けており妥当。
- **scope deny**: `deny_write_rules` に `workflow_dir()` を追加するのは memory tool の generic CRUD パスを Workflow path に到達させない目的。doc コメント（`crates/memory/src/scope.rs:14-18`）で「Workflow files are human-edited on the host side」と明示しており、想定ポリシーと整合。
- **classify の対称性**: `classify` の if 分岐順序は knowledge → workflow → memory（`crates/memory/src/workspace.rs:157-167`）の順で、`memory_dir()` 配下の旧 `memory/workflow/` を `RecordKind::Workflow` に分岐させる経路を完全に削っている。`workflow_under_memory_is_invalid_path` test がこの挙動を保護している。
- **互換コードの除去**: 旧 fallback 読み取りの実装が一切残っていないことを確認した。merge 関数や priority 比較などの互換複雑度を抱え込まず、要件の更新（後方互換不要）に沿って最小実装。コードベースを歪めていない。
- **不必要な実装は混入していない**。差分は path 定数 1 行の移動、`classify` 分岐 1 つ追加、`deny_write_rules` 1 行追加、loader / linter から `WORKFLOW_DIR` segment の削除、テスト 3 件追加に留まる。

## 指摘事項

### Non-blocking / Follow-up

- **`crates/memory/src/tool/query.rs` の test fixture が legacy path を使っている** — `setup` で `.insomnia/memory/workflow` を作成（line 446）し、`memory_query_excludes_workflow_and_staging` でも legacy path にファイルを書く（line 559）。`MemoryQuery` は `decisions_dir` / `requests_dir` / `summary` のみを scan するので、test の主張（query から workflow が見えない）自体は今でも成立しているが、canonical path が `.insomnia/workflow/` に移った今では「workflow を memory tree に置いて除外を確認する」という構図が古い。`.insomnia/workflow/wf.md` に書き換えるか、test の意図を「memory tree 配下の異物を query が拾わない」と捉え直してコメントを添えると正確。
- **`tickets/internal-worker-workflow.md` の path 表記が古い** — 別ティケットだが、line 11 / 25 / 62 に `<workspace_root>/.insomnia/memory/workflow/<slug>.md` `memory/workflow/<slug>.md` が残っている。本チケットの完了条件には含まれないが、次にそのチケットへ着手する人が誤った前提で実装を組まないよう、近い将来の更新が望ましい。
- **TODO.md の該当行** — `TODO.md:4` に本チケットへのリンクが残っている。完了 commit でユーザーが削除する想定（CLAUDE.md のライフサイクル d）なので report しておくのみ。

### Nits

- `.insomnia/.gitignore` の末尾に `# Project-authored workflows and knowledge are intentionally tracked.` というコメントが入っているが、`knowledge/` は対象 ticket の範囲外（既に追跡されていた）。意図は分かるが、本ファイルがこの ticket の範囲では「workflow が tracked になる」ことを表明しているとみなせるので問題なし。

## 判断

**Approve（完了可）** — 要件はすべて満たされており、後方互換不要に切り替えた更新方針に沿って最小・整合性のある実装になっている。Non-blocking で挙げた `tool/query.rs` test の legacy path 残置は別途整理可能。
