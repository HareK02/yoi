# AI maintainer 用 WorkItem / Thread 抽象

## 背景

現在の開発運用は `TODO.md` と `tickets/*.md` を中心に回している。これは Git 履歴で要件と完了条件を追うには十分だが、AI maintainer が単なる Coding Agent を超えて運用を担うには弱い。

特に、設計相談、実装 Pod の作業報告、review 指摘、修正依頼、完了判断、Pod run、lease、artifact が ticket file / review file / 会話 / git log に分散し、thread として扱いづらい。

repository-managed issue directory example の `issues/` directory のように、repository 内に issue / work item を置く運用は参考になる。一方で、同 repository の owner も指摘している通り、中央の `SEQUENCE` ファイルによる連番採番は並列 branch / worktree で conflict しやすい。将来的な network 越し workspace / remote coordination も想定すると、最初から Git directory や中央採番前提で API を固めるべきではない。

本チケットでは、`tickets/` を直ちに置き換えるのではなく、AI maintainer が扱う上位の **WorkItem / Thread / Event / Lease / Artifact** 抽象を設計し、最小 file backend を導入できる状態にする。

## 方針

- WorkItem / Thread の正本は `.insomnia` ではなく、project-visible な repo-managed 領域に置く
- `.insomnia` は local runtime state / memory / workflow / Pod run / lease cache の領域として分離する
- API / domain model は Git に依存しない形にする
- WorkItem ID は中央 `SEQUENCE` 連番ではなく、作成時刻ベースの衝突しにくい ID にする
- 初期 backend は repo 内 directory（例: `work-items/` または `issues/`）でよい
- network 越し workspace / remote hub は後回しにするが、将来差し替え可能な interface を先に切る
- 既存 `tickets/` は当面維持し、WorkItem から link するか、file backend の一 view として扱えるようにする

## データ配置

初期設計では以下の分離を前提にする。

```text
repo/
  work-items/ or issues/        # project-visible, git-managed coordination data
  tickets/                      # 当面の既存 ticket
  docs/                         # 設計・report
  .insomnia/                    # local agent/runtime state
    memory/
    workflow/
    maintainer/
      leases/
      runs/
      inbox/
```

### repo-managed に置くもの

- WorkItem description
- acceptance criteria
- discussion thread
- design decision
- review comment
- status history
- linked branch / worktree / commit
- durable artifact metadata

### `.insomnia` に置くもの

- Pod run state
- lease の local cache
- SpawnedPod polling cursor
- temporary inbox
- local-only trial log
- model / role runtime state

## WorkItem ID

WorkItem ID は identity のためだけに使い、priority や処理順序を背負わせない。`SEQUENCE` のような中央連番ファイルは、複数 branch / worktree / Pod が同時に WorkItem を作ると conflict しやすいため採用しない。

初期 file backend では、directory name を immutable ID として扱う。

```text
YYYYMMDD-HHMMSS-<slug>
YYYYMMDD-HHMMSS-<short-rand>-<slug>  # 同一秒衝突を避けたい場合
```

例:

```text
20260510-184233-maintainer-work-items
20260510-184233-a1b2-maintainer-work-items
```

要件:

- lexical sort で概ね作成順になる
- 中央採番ファイルを更新しない
- collision 時は backend が短い random suffix や retry で解決する
- human-visible `slug` / `title` と immutable `id` を分ける
- priority / status / scheduling は `id` ではなく metadata で表す
- 将来 remote backend に移る場合も ID 生成責務は backend 側に閉じ込める

## WorkItem model

最低限、以下の概念を持つ。

```text
WorkItem
- id
- slug
- title
- status
- kind: feature | bug | refactor | design | ops | investigation
- priority / labels
- owner / assignee / current lease summary
- acceptance criteria
- linked ticket / docs / branches / worktrees / commits
- thread events
- artifacts
```

```text
ThreadEvent
- id
- work_item_id
- occurred_at
- author: human | orchestrator | pod:<name>
- role: comment | plan | decision | implementation_report | review | status_change | escalation | artifact
- reply_to?
- body
- links
```

```text
Lease
- id
- work_item_id
- holder
- scope hint
- worktree path
- expires_at
- status: active | released | expired
```

Lease はリアルタイム coordination に近いため、Git-managed thread の正本とは分ける。file backend 初期実装では `.insomnia/maintainer/leases/` か local DB を使ってよい。

## backend interface

AI maintainer / `/auto-maintain` は file path を直接前提にせず、概念上は store interface を通す。

```text
WorkItemStore
- list(filter)
- get(id)
- create(item)
- append_event(id, event)
- update_status(id, status)
- attach_artifact(id, artifact)
```

```text
LeaseStore
- acquire(work_item_id, holder, scope, ttl)
- refresh(lease_id)
- release(lease_id)
- list_active(filter)
```

初期 backend 候補:

- `file://<repo>/work-items`
- `file://<repo>/issues`
- `sqlite://<workspace>/.insomnia/maintainer/work-items.db`

将来 backend 候補:

- `http://maintainer-hub/...`
- `github://owner/repo/issues`

## 初期 file backend 案

```text
work-items/
  open/
    20260510-184233-maintainer-work-items/
      item.md
      thread.jsonl
      artifacts/
        review.md
        test-log.txt
  pending/
    20260510-190102-transport-parameter-api/
      item.md
      thread.jsonl
      artifacts/
  closed/
    20260510-201522-anthropic-burst-bundling/
      item.md
      thread.jsonl
      resolution.md
      artifacts/
```

`item.md` は human-readable な issue 本文（背景、根拠、完了条件、非目標など）を持つ。`thread.jsonl` は append-only を基本にし、AI maintainer が conversation / decision / review / status change を追いやすい形にする。`resolution.md` は close 時の解決方法や検証結果を、人間が読みやすい形でまとめる任意ファイルとする。

## `/auto-maintain` との関係

`/auto-maintain` は当面 `TODO.md` / `tickets/` を読むが、将来的には WorkItemStore を入口にする。

移行方針:

1. 既存 `tickets/` は維持
2. WorkItem 抽象と file backend schema を設計する
3. 新しい設計相談・並列作業・長い thread が必要な作業だけ WorkItem 化する
4. ticket は WorkItem の linked artifact として扱う
5. 十分に安定したら `tickets/` を WorkItem backend の view に寄せる

## 範囲外

- remote maintainer hub の実装
- GitHub Issues backend の実装
- 既存 `tickets/` の即時移行
- 常駐 scheduler
- Pod lifecycle / completion tracking の完全実装
- scope owner handoff の再設計

## 完了条件

- WorkItem / Thread / Event / Lease / Artifact の domain model が docs に定義されている
- repo-managed coordination data と `.insomnia` local runtime state の分担が明文化されている
- `WorkItemStore` / `LeaseStore` 相当の interface 方針が決まっている
- WorkItem ID scheme が中央連番ではなく timestamp-based になっている
- 初期 file backend の directory schema が決まっている
- `/auto-maintain` / AI maintainer が将来 WorkItemStore を入口にできる移行方針が書かれている
- network 越し workspace / remote hub は後回しにしつつ、backend 差し替え可能性を潰していない

## 参照

- `docs/plan/ai-maintainer.md`
- `tickets/auto-maintain-workflow.md`
- `docs/report/2026-05-10-ticket-lifecycle-branch-placement.md`
