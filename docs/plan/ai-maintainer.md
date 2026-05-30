# AI Maintainer Workflow 設計

## 目的

AI maintainer は、insomnia リポジトリの開発を継続的に進めるための orchestration role である。単発の `/auto-maintain` より広く、設計相談、work item 整理、実装委譲、レビュー、運用課題の記録、改善提案を一つの maintainer loop として扱う。

`/auto-maintain` はこの設計の限定実行形であり、`tickets.sh` / `work-items/` から小さな実装作業を選んで実装・レビューを orchestration する Workflow に留まる。

## 正本と権限境界

現在の作業管理の正本は `tickets.sh` と `work-items/` である。古い `TODO.md` / legacy `tickets/` directory を前提にした運用は superseded。

- work item の作成・コメント・レビュー・状態変更・完了は原則 `./tickets.sh` 経由で行う
- `work-items/{open,pending,closed}/<id>/item.md` は要件・受け入れ条件の正本
- `thread.md` は計画・判断・レビュー・実装報告の時系列 thread
- `resolution.md` は close 時の完了記録
- 時系列と状態遷移の最終根拠は git history
- `docs/report/` は観測・所感・改善候補の記録であり、最新仕様の authority ではない
- `.insomnia/memory` は個人/生成 state であり、project record の正本ではない

AI maintainer は project record を勝手に膨らませない。明確な実装単位は work item 化し、小粒な所見は `KNOWN_ISSUES.md`、ドッグフーディング上の障壁やツール問題は `docs/report/` に記録する。

## Role model

### Maintainer Pod

人間と対話する親 Pod。設計判断、作業選定、scope 委譲、レビュー統括、merge / close の判断を担う。

主な責務:

- `work-items/`, `KNOWN_ISSUES.md`, `docs/`, git history, worktree 状態を読んで現在地を把握する
- 作業可能な work item と設計相談が必要な work item を分ける
- child worktree / child Pod に実装を委譲する
- 実装結果を work item の背景・要件・受け入れ条件に照らして review する
- review / close / merge の branch placement を守る
- 人間に判断が必要な境界で止まる

Maintainer Pod は「便利だから」project record を書き換えない。新規 work item 作成、既存 work item の大幅変更、close、merge、commit などは、ユーザー指示または会話上の合意に基づいて行う。

### Implementation Pod

狭い write scope を持つ作業者。対象 worktree と work item を渡され、その範囲で実装・検証・報告を行う。

制約:

- 指定 scope 外を編集しない
- `.insomnia` や main workspace の control-plane record を勝手に編集しない
- work item / review / close は maintainer の責務として扱う
- 実装報告には変更点、検証、未解決点を含める

### Reviewer Pod

原則 read-only。実装 diff と対象 work item を読み、妥当性を確認する。

見るべき点:

- work item の要件・受け入れ条件を満たしているか
- 設計を歪めていないか
- 不必要な後方互換性や局所最適な変更を作っていないか
- テストが適切か
- ドキュメント / work item / known issue の更新が必要か

## Workflow modes

### Mode 0: Design consultation

設計や方針を相談するだけの mode。コードや work item は編集しない。

使う場面:

- 要件がまだ曖昧
- work item 化する前に合意したい
- 複数案の設計判断が必要

### Mode 1: Project record maintenance

`work-items/`, `KNOWN_ISSUES.md`, `docs/`, `docs/report/` など control-plane record だけを編集する。実装コードは触らない。

例:

- work item の作成・詳細化
- review / implementation report / resolution の追記
- stale docs の修正
- known issue の追加・削除

新規 work item 作成や大幅変更は、人間の合意後に行う。

### Mode 2: Implementation orchestration

Maintainer Pod が child worktree と implementation Pod を作り、実装と review を回す。親は scoped delegation と project record authority を保持する。

基本手順:

1. 対象 work item を main workspace で作成・詳細化し commit する
2. orchestrator が child worktree を作る
3. implementation Pod に対象 worktree と task を渡す
4. implementation Pod が実装・検証・報告する
5. reviewer Pod が diff と work item を読む
6. 必要なら修正を戻す
7. maintainer が merge-ready dossier を作る

### Mode 3: Maintainer-managed completion

人間が明示的に許可した場合に、親 Pod が commit / merge / close まで行う。

条件:

- 対象 work item が明確
- scope と worktree が分離されている
- review 結果が approve または残件が明示されている
- validation が通っている
- close / merge してよいというユーザー指示または会話上の合意がある

push はしない。破壊的 git 操作は明示指示なしに行わない。

## Work item lifecycle

### 作成

```
./tickets.sh create --title "..." [--slug slug] [--kind task] [--priority P2] [--label a,b]
```

作成後、必要なら `item.md` を詳細化し、背景・要件・受け入れ条件を明確にする。worktree を使う場合は、対象 work item を作成・詳細化して commit してから branch/worktree を切る。

### コメント / 計画 / 判断

```
./tickets.sh comment <id-or-slug> --role plan --file path
./tickets.sh comment <id-or-slug> --role decision --file path
./tickets.sh comment <id-or-slug> --role implementation_report --file path
```

thread は後から読まれる project record なので、実装ログをだらだら貼るより、判断理由・検証結果・残件を簡潔に残す。

### レビュー

```
./tickets.sh review <id-or-slug> --approve --file path
./tickets.sh review <id-or-slug> --request-changes --file path
```

レビューは diff の確認だけではない。work item の前提・要件・受け入れ条件が実装で満たされているか、コードベースを歪めていないかを確認する。

### 完了

```
./tickets.sh close <id-or-slug> --resolution "..."
```

close は `work-items/closed/` へ移動し、`resolution.md` と thread entry を残す。close commit には resolution と関連 project record 更新を含める。

## Branch / worktree placement

main workspace は orchestration / docs / work item lifecycle / merge の control plane。実装差分は原則 child worktree に隔離する。

```
main/develop workspace:
  work item 作成・詳細化 commit
  child worktree 作成

child worktree feature branch:
  implementation commit(s)
  validation

main/develop workspace:
  reviewer Pod に read-only 依頼
  merge / close / cleanup commit
```

child Pod に write scope を渡している間、親 Pod は同じ path を編集しない。必要なら `StopPod` で scope を回収してから project record を更新する。

## Human approval boundaries

AI maintainer は以下で人間に戻す。

- 要件から複数の設計方針が自然に導ける
- public API / protocol / manifest / durable data format を変える
- 大規模 rename / directory 移動 / repository-wide format を伴う
- work item の作成・大幅変更・close について合意がない
- reviewer が request-changes しており、修正方針が自明でない
- validation が失敗し、原因が対象変更か既存問題か不明
- destructive git operation や push が必要

## Reports / Knowledge / Memory

- `docs/report/`: ドッグフーディングで感じた障壁、改善案、ツール問題の記録。明確な作業単位になったら work item 化する
- `.insomnia/knowledge`: curated project knowledge。正本ではなく補助 context
- `.insomnia/memory`: generated/personal state。project record の代替にしない
- `KNOWN_ISSUES.md`: ticket 化するほどではないが、次に近所を触る時に拾いたい小粒所見

## Future extension points

### WorkItemStore API

現在は `tickets.sh` + repo-managed `work-items/` を正本とする。将来、remote maintainer hub や issue tracker へ差し替えるなら、WorkItem / Thread / Event / Artifact を扱う store interface を追加し、Git/path layout を上位 workflow に漏らさない形にする。

### Maintainer doctor

merge 前 / close 前に以下を検査する command。

- 対象 work item が存在する
- acceptance criteria と implementation report が対応している
- review status が明示されている
- validation log がある
- work item lifecycle と branch placement が矛盾していない

### Multi-maintainer coordination

複数 maintainer Pod が同じ repository を扱う場合は、work item / branch / scope の lease が必要。現状は人間と git history が最終調停者。
