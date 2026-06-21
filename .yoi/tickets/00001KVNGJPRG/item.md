---
title: 'Workspace web: repository and objective pages'
state: 'queued'
created_at: '2026-06-21T16:35:19Z'
updated_at: '2026-06-21T16:41:09Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-21T16:40:35Z'
---

## 背景

Workspace web control plane の初期段階では、Ticket / Objective の操作系が Web で十分に実装され、移行できる状態になるまでは、既存 `.yoi/tickets` / `.yoi/objectives` の filesystem record を read-through authority として扱う。

SQLite は Workspace server の runtime/projection/store seam として使うが、Ticket / Objective の canonical write path を中途半端に DB へ移さない。少なくとも Ticket 作成・コメント・状態遷移・close、Objective 作成/更新、validation/audit が Web/API 側で成立するまでは、Web UI は filesystem records を読む方向で進める。

次の UI slice として、Workspace sidebar から遷移できる Repository page と Objective list page を追加する。

Repository page では、当面は backend が動いている Workspace root の Git repository を primary Repository として扱う。Git repository である場合は、簡易的な Git 情報、直近 log、Repository に関係する Ticket の Kanban view を表示する。Repository target metadata がまだ Ticket schema に十分無い場合は、初期実装では workspace-local tickets 全体または既存 metadata から安全に導ける範囲を表示し、target selector 対応は follow-up にできる。

## 方針

- Ticket / Objective は当面 filesystem read-through で表示する。
- Web UI からの mutation / DB migration は、この Ticket の主目的にしない。
- Repository は Git 専売の概念ではないが、初期 page は Git Repository の read-only summary から始める。
- Repository page は将来の Repository provider / RepositoryPoint / target selector model に繋がる形にする。
- Ticket Kanban は Ticket state を column として表示する。
- Objective list は existing `/api/objectives` を使い、Objective の title/state/summary を一覧できるようにする。

## 要件

### Backend / API

- Repository list/detail の read-only API を追加する、または既存 `/api/workspace` に必要最小限の Repository summary を追加する。
  - 初期は current workspace root を 1 つの local Repository として返してよい。
  - Git repository の場合は branch/head/root/dirty status/remote URL summary などを bounded に返す。
  - Git でない場合は `kind = "local"` / `git = unavailable` 相当の diagnostic を返す。
- Git log summary API を追加する。
  - 直近 N 件だけ返す。
  - commit hash、subject、author name/email の扱い、timestamp を bounded にする。
  - full diff / patch / file contents は返さない。
- Repository Ticket Kanban 用の read model を追加する。
  - 初期は Ticket state ごとに group した bounded list でよい。
  - Ticket target Repository metadata がない場合の fallback を明記する。
  - 将来 target selector が入ったら Repository ごとの filter に差し替えられる形にする。
- Objective list/detail は既存 filesystem read-through API を継続利用する。

### Frontend

- Sidebar の `repositories` section から Repository page に遷移できる。
- Repository page を追加する。
  - Repository summary。
  - Git summary / recent log。
  - Ticket Kanban columns。
  - API failure / non-Git / empty tickets を section 単位で表示。
- Objective list page を追加する。
  - Objective title/state/updated_at などの一覧。
  - Objective detail への遷移は可能なら行う。無理なら placeholder でよい。
- UI は static SPA のまま実装し、frontend に authority logic を持たせない。

## Non-goals

- Ticket / Objective の DB canonical migration。
- Web からの Ticket mutation / Objective mutation。
- Repository CRUD / remote Git hosting integration。
- Full Git diff viewer / file browser / blame。
- Ticket target selector schema の完成。
- Multi-repository selection UI の完成。
- Kanban drag-and-drop / state mutation。

## 受け入れ条件

- Ticket / Objective は引き続き filesystem read-through authority から表示される。
- Repository page が表示できる。
- Git repository の場合、Repository summary と recent log が bounded に表示される。
- Repository Ticket Kanban が state columns で表示される。
- Ticket target metadata が未整備でも安全な fallback 表示になる。
- Objective list page が表示できる。
- Sidebar から repositories / objectives に遷移できる。
- API failure、non-Git repository、empty state が UI で壊れず表示される。
- `deno task check` と `deno task build` が通る。
- backend 変更がある場合は `cargo test -p yoi-workspace-server` が通る。
- `cargo fmt --check`、`cargo check`、`git diff --check`、`yoi ticket doctor`、`nix build .#yoi --no-link` が通る。
