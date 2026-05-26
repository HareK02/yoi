# Ticket 管理: tickets.sh による WorkItem / Thread MVP

## 背景

現在の ticket 運用は `TODO.md` と `tickets/*.md`、必要に応じて `tickets/*.review.md` を Git 履歴で管理している。要件と完了条件を追うには機能しているが、multi-agent worktree workflow と組み合わせると review / 修正依頼 / 実装報告が扱いづらい。

特に `.review.md` は、review artifact を main workspace の ticket directory に作る必要がある。一方で実装 Pod は child worktree だけに write scope を持つため、review thread と実装 thread が分断されやすい。子 Pod を止めて scope を回収し、review file を作り、再度 restore / spawn するような運用になりがちで面倒である。

Git は履歴の保存層として有用だが、人間や AI maintainer が毎回 file move / delete / review file 作成 / git log 探索を直接操作するのは低級すぎる。repository 内の file backend を正本にしつつ、`tickets.sh` で create / list / show / comment / review / close などの意味的操作を提供する。

この ticket は `maintainer-work-items.md` の大きな WorkItemStore 設計の前段として、shell script による最小 MVP を実装する。

## 方針

- 既存 `tickets/` / `TODO.md` はすぐに置き換えない。
- 新しい thread 型作業だけ `work-items/` に置く。
- `tickets.sh` は Git を内部保存層として前提にしてよいが、操作単位は file path ではなく WorkItem 操作にする。
- 初期実装では自動 commit しない。
  - `tickets.sh` は file 操作まで。
  - `git add/commit` は利用者または workflow が明示的に行う。
- `--help` だけで基本操作が分かるようにする。
- shell script なので依存は POSIX shell + 基本 Unix tool に寄せる。`jq` 必須にはしない。

## 初期 backend schema

```text
work-items/
  open/
    20260526-123456-short-slug/
      item.md
      thread.md
      artifacts/
  pending/
    ...
  closed/
    ...
      resolution.md
      artifacts/
```

`item.md` は YAML frontmatter + Markdown body。

```yaml
---
id: 20260526-123456-short-slug
slug: short-slug
title: Human-readable title
status: open
kind: feature
priority: P2
labels: [maintainer, workflow]
created_at: 2026-05-26T12:34:56Z
updated_at: 2026-05-26T12:34:56Z
assignee: null
---

## Background

...

## Acceptance criteria

- ...
```

`thread.md` は append-only Markdown event log とする。JSONL より人間が読みやすいことを優先する。

```md
<!-- event: comment author: hare at: 2026-05-26T12:40:00Z -->

## Comment

...

---

<!-- event: review author: orchestrator at: 2026-05-26T13:00:00Z status: request_changes -->

## Review: request changes

...
```

`tickets.sh` が必ず event header と separator を付ける。機械 parse は初期実装では簡易でよい。

## コマンド MVP

```text
./tickets.sh help
./tickets.sh list [--status open|pending|closed|all]
./tickets.sh show <id-or-slug>
./tickets.sh create --title <title> [--slug <slug>] [--kind <kind>] [--priority P2] [--label a,b]
./tickets.sh comment <id-or-slug> [--role comment|plan|decision|implementation_report] [--author <name>] [--file <path>]
./tickets.sh review <id-or-slug> --approve|--request-changes [--author <name>] [--file <path>]
./tickets.sh status <id-or-slug> open|pending|closed
./tickets.sh close <id-or-slug> [--resolution <text>|--file <path>]
./tickets.sh doctor
```

`help` / `--help` は同じ内容を出す。

### list

- `work-items/{open,pending,closed}/*/item.md` を scan する。
- status / id / slug / title / kind / priority / updated_at を一行で表示する。
- 初期実装では frontmatter parser は簡易でよい。

### show

- `item.md` と `thread.md` の末尾を読みやすく表示する。
- 完全な thread 全体を出すか、初期は tail 表示でもよい。`--all` は後続でよい。

### create

- ID は `YYYYMMDD-HHMMSS-<slug>`。
- 同一 path が存在する場合は短い random suffix または pid suffix を付けて衝突回避する。
- `work-items/open/<id>/item.md`, `thread.md`, `artifacts/` を作る。
- central `SEQUENCE` は作らない。

### comment / review

- `thread.md` に append する。
- `item.md` の `updated_at` を更新する。
- review は role/comment の special case として、`approve` / `request_changes` が分かる event header を付ける。
- `.review.md` は作らない。

### status / close

- status directory を move する。
- `item.md` frontmatter の `status` と `updated_at` を更新する。
- `close` は `status closed` + optional `resolution.md` + close event append。
- 完了しても削除しない。

### doctor

- directory status と frontmatter `status` の一致を検査する。
- `item.md` / `thread.md` / `artifacts/` の存在を検査する。
- duplicate slug / duplicate id を検査する。
- error は非ゼロ exit。

## 要件

- `tickets.sh --help` で使い方が分かる。
- `create/list/show/comment/review/status/close/doctor` が動く。
- WorkItem ID は timestamp-based で、central sequence file を使わない。
- close しても削除せず `work-items/closed/` に移動する。
- review は `.review.md` ではなく thread event として append できる。
- `doctor` が directory status と frontmatter status の不一致を検出する。
- script は main repository の既存 `tickets/` を破壊しない。
- 初期実装では自動 git commit しない。
- README 相当の usage は `--help` に含める。

## 完了条件

- repo root に `tickets.sh` が追加される。
- `work-items/` schema の minimal docs が script help または `work-items/README.md` にある。
- `tickets.sh create` で WorkItem を作成できる。
- `tickets.sh comment` / `tickets.sh review` で thread event を append できる。
- `tickets.sh close` で closed に移動できる。
- `tickets.sh doctor` が正常ケースで 0、不整合ケースで非ゼロになる。
- shellcheck が利用可能なら通る。無い場合は少なくとも focused smoke test を実行する。

## 範囲外

- 既存 `tickets/` の移行。
- TODO.md の generated view 化。
- Rust crate / DB / remote backend 実装。
- LeaseStore / Pod run tracking の実装。
- Git commit の自動化。
- TUI 統合。
