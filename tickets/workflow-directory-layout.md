# Workflow の物理配置を `.insomnia/workflow` に分離する

## 背景

現行の Workflow は `<workspace>/.insomnia/memory/workflow/<slug>.md` に配置される。これは実装上 memory subsystem の loader / linter に載せていた名残だが、概念上は不自然になっている。

Workflow は session-derived な記憶ではなく、人間が管理する手順・操作ポリシーである。一方で `.insomnia/memory/summary.md`、`decisions/`、`requests/`、`_staging/` は自動抽出・統合される memory state であり、ignore や書き込み禁止の扱いも異なる。

この混在により、`.insomnia/.gitignore` で `memory` を ignore すると project-authored Workflow まで Git 管理から外れる。また、memory write tool が Workflow 書き込みを禁止するなど、「memory 配下にあるが memory ではない」ものとして扱う歪みが出ている。

Knowledge は既に `.insomnia/knowledge/<slug>.md` として `memory/` の外にある。Workflow も同様に `.insomnia/workflow/<slug>.md` を canonical path とし、memory state から分離する。

## 要件

### canonical path の変更

Workflow の標準配置を以下に変更する。

```text
<workspace>/.insomnia/workflow/<slug>.md
```

旧配置は以下。

```text
<workspace>/.insomnia/memory/workflow/<slug>.md
```

新規作成・ドキュメント・補完・resolver の説明は新配置を使う。旧配置の互換読み取りは行わない（このリポジトリ内で完結する移行であり、外部利用者の互換配慮は不要との判断）。旧配置にファイルが残っていても loader は読まないし、linter / scope deny も新配置のみを対象にする。

### linter / registry / completion

- Workflow linter は新配置を検査できる
- `requires` の Knowledge 参照検査は従来通り維持する
- `WorkflowRegistry` は新配置の source path を保持する
- `/<slug>` completion / invocation は新配置から読める
- resident workflow (`model_invokation: true`) の広告も新配置を対象にする

### memory state との分離

`.insomnia/memory/` は generated / session-derived state として扱い、Workflow は含めない。

推奨される layout:

```text
.insomnia/
  manifest.toml
  workflow/
    auto-maintain.md
    worktree-workflow.md
  knowledge/
    <slug>.md
  memory/
    summary.md
    decisions/
    requests/
    _staging/
```

`.insomnia/.gitignore` は `memory` 丸ごと ignore ではなく、generated memory state のみを ignore する形にする。

例:

```gitignore
/memory/_staging/
/memory/summary.md
/memory/decisions/
/memory/requests/
```

### 既存ファイルの移行

workspace に既存 Workflow がある場合、新配置へ移す。

対象例:

```text
.insomnia/memory/workflow/auto-maintain.md
.insomnia/memory/workflow/worktree-workflow.md
```

移行後:

```text
.insomnia/workflow/auto-maintain.md
.insomnia/workflow/worktree-workflow.md
```

移行は Git 管理対象として扱えるようにする。

## 範囲外

- Workflow の自動生成ポリシー変更
- Workflow frontmatter schema の大幅変更
- Workflow DSL 化
- memory tool で Workflow を書けるようにする変更
- 内部 Worker Workflow 化そのもの（`tickets/internal-worker-workflow.md` の対象）

## 完了条件

- Workflow の canonical path が `.insomnia/workflow/<slug>.md` として実装・文書化されている
- loader / linter / scope deny / completion / invocation が新配置のみを対象にしている
- Workflow linter / loader / completion / invocation のテストが新配置をカバーしている
- `.insomnia/.gitignore` が generated memory state のみを ignore し、`.insomnia/workflow/*.md` を Git 管理できる
- この workspace の既存 Workflow が `.insomnia/workflow/` に移されている
- `docs/plan/workflow.md` や関連コメントが新配置に更新されている

## 参照

- `docs/plan/workflow.md`
- `crates/memory/src/workspace.rs`
- `crates/memory/src/workflow.rs`
- `crates/pod/src/workflow/mod.rs`
- `tickets/auto-maintain-workflow.md`
- `tickets/internal-worker-workflow.md`

## Review

- 状態: Approve
- レビュー詳細: [./workflow-directory-layout.review.md](./workflow-directory-layout.review.md)
- 日付: 2026-05-08
