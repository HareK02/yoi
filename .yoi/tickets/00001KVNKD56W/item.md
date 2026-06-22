---
title: 'Workspace DB canonical schema design'
state: 'inprogress'
created_at: '2026-06-21T17:24:43Z'
updated_at: '2026-06-22T09:18:29Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-22T08:48:39Z'
---

## 背景

Workspace web control plane は、Rust backend / SQLite store / static SPA / read-only filesystem bridge まで立ち上がっている。ただし現在の SQLite schema は起動・API skeleton 用の足場であり、Ticket / Objective / Repository / Host / Worker / Artifact を将来 DB 正本にするための canonical schema はまだ固まっていない。

DB 設計は過度に難しく扱わず、まずは既存 filesystem Ticket/Objective model と現在の orchestration 運用を素直に relational/event model へ写す。重要なのは、`.yoi` filesystem record をすぐ捨てることではなく、Web/API/Orchestrator が将来 DB authority に移れるだけの record 境界を決めること。

長期方針:

- Ticket / Objective は Workspace 配下に平たく存在する。
- Repository は Workspace に接続される source/storage であり、Git Repository はその一種。
- Ticket は必要に応じて Repository target selector を持つ。
- Ticket thread/events が実行履歴の authority であり、Ticket state は current snapshot として持つ。
- Worker は Ticket に関連づく logical agent/session だが、v0 では DB 正本として永続化しない。Host/Worker 一覧は runtime inspection / future Host protocol から逐次取得する live view とする。
- Worker の一元管理、データ永続化、アーカイブは将来必要になるが、Host/Worker protocol と lifecycle requirements が固まるまで v0 schema には入れない。
- Host は Worker が観測される実行環境または capacity を表すが、v0 では canonical table ではなく live view とする。
- CI/check information should be represented as Ticket events plus Artifact links in v0. If richer CI status is needed, design it later as a separate actions-like subsystem rather than a generic validation table.
- Orchestrator は将来的に fs/Bash/Git を直接持たず、Ticket / TicketEvent / TicketWorkerLink / live Host/Worker view / Artifact / Review の DB/API surface だけを見る。
- Memory / Knowledge の本格再設計はこの Ticket では扱わず、保存先を Workspace backend へ移す時に回収する。

## 要件

### Canonical records / tables

DB design は `artifacts/schema-v0.md` を主たる設計記録とする。Ticket 本文では scope と要求だけを示す。

`schema-v0.md` では以下を明確に定義する。

- record / event / reference の区別。
- ID / timestamp / no-catch-all-payload column conventions。
- `AuthorRef` v0。
  - event author/source を記録する typed snapshot。
  - 初期実装では full `actors` table を必須にしない。
  - auth / permission / assignment / team membership が必要になった時点で `actors` table へ昇格できる。
- table candidates and required columns:
  - `workspaces`
  - `tickets`
  - `ticket_events`
  - `ticket_relations`
  - `objectives`
  - `objective_ticket_links`
  - `repositories`
  - `ticket_targets`
  - `ticket_target_paths`
  - `ticket_worker_links`
  - `artifacts`
  - `audit_events`
- Orchestrator no-fs/no-bash の read/write surfaces。
- filesystem read-through から DB authority への migration modes。

初期設計で曖昧な `actors` entity を置かない。必要なのは event authorship であり、v0 では `AuthorRef` fields として扱う。

### Authority / migration stance

- 当面は `.yoi/tickets` / `.yoi/objectives` filesystem read-through を維持する。
- DB schema は canonical target として設計するが、この Ticket だけで full migration はしない。
- import/projection/export の方針を決める。
  - filesystem -> DB import。
  - DB -> filesystem export / compatibility snapshot。
  - read-through bridge と DB authority の切り替え条件。
- 二重正本を避けるため、write path をいつ DB に切り替えるかを明記する。
- SQLite first でよいが、将来 Postgres/multi-tenant へ進めるよう、`workspace_id` を全主要 record に含める。

### Orchestrator no-fs/no-bash surface

DB/API だけを見て Orchestrator が判断できるように、以下の read/write surface を設計する。

Orchestrator が読むもの:

- Ticket state/thread/relations/targets。
- Ticket-associated WorkerRef links and worker status events。
- Objective context。
- Live Host/Worker view from runtime inspection or future Host protocol。
- Repository target and artifact source revision fields。
- Artifact summaries/diff metadata/log summaries。
- Review evidence。

Orchestrator が作るもの:

- Ticket comment/decision/state transition request。
- Ticket execution request / WorkerRef assignment / worker status events。
- Worker job request / assignment request。
- Review/check request。
- Close/done decision with evidence references。

fs/Bash/Git 操作は Host/Worker に閉じ込める。Orchestrator は raw repository filesystem や shell access を authority として持たない。

### Initial implementation slice

この Ticket は design-only でもよいが、可能なら最小 schema migration まで含める。

- `crates/workspace-server` の SQLite schema を canonical design に合わせて整理する。
- 既存 placeholder `runs` / `runners` naming を、structured ticket events / live Host/Worker API / `ticket_worker_links` へ寄せる。
- Migration versioning の方針を明記する。
- Existing read API を壊さない。

## Non-goals

- Full DB migration of existing `.yoi/tickets` / `.yoi/objectives`。
- Web UI からの Ticket/Objective mutation 実装。
- Memory / Knowledge の本格再設計。
- Multi-tenant production SaaS schema の完全設計。
- Auth/billing/quota/security の完全実装。
- Git hosting service の実装。
- Orchestrator profile から fs/Bash を実際に剥がすこと。

## 受け入れ条件

- Workspace DB canonical schema design が `artifacts/schema-v0.md` または同等の design record として記録されている。
- `schema-v0.md` が record / event / reference の区別を定義している。
- Table/record 境界として Workspace / Ticket / TicketEvent / TicketRelation / Objective / ObjectiveTicketLink / Repository / TicketTarget / TicketTargetPath / TicketWorkerLink / Artifact / AuditEvent が定義されている。
- Separate top-level Run entity/table を v0 では作らず、structured Ticket events と TicketWorkerLink relation を execution history/management surface とする方針が明記されている。
- Host/Worker は v0 DB canonical table ではなく live runtime view とし、Ticket には WorkerRef snapshot/link を保存する方針が明記されている。
- Event/request/audit authorship は `AuthorRef` v0 として required fields まで定義されている。
- 初期実装で full Actor entity/table を必須にしない方針と、将来 actors table へ昇格できる条件が明記されている。
- CI/check status は v0 core schema では Ticket events + Artifact links として扱い、actions-like subsystem は future work とする方針が明記されている。
- `.yoi` filesystem read-through から DB authority へ移る migration/export/import 方針が明記されている。
- Orchestrator no-fs/no-bash を可能にする DB/API read/write surface が明記されている。
- Memory / Knowledge は deferred として扱われ、この schema design の必須 scope から外れている。
- SQLite first だが、全主要 record に `workspace_id` を持たせる方針が明記されている。
- 既存 `runner` placeholder naming を Host/Worker に移す方針が明記されている。
- 実装まで含める場合は `cargo fmt --check`、`cargo test -p yoi-workspace-server`、`cargo check`、`git diff --check`、`yoi ticket doctor`、`nix build .#yoi --no-link` が通る。
