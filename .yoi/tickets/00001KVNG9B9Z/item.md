---
title: 'Workspace web UI: add sidebar navigation panel'
state: 'closed'
created_at: '2026-06-21T16:30:12Z'
updated_at: '2026-06-21T17:01:13Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-21T16:35:06Z'
---

## 背景

Workspace web UI は static SPA skeleton と read-only API まで立ち上がっている。次に、Workspace control plane の基本情報へ移動するための左サイドパネルを作り、Workspace / Repository / Objective / Worker を一覧できる最初の navigation surface を用意する。

初期イメージ:

```text
my-workspace                         ⚙
---
repositories
- yoi

objectives
---
<workers list>
```

この Ticket は UX skeleton の実装を対象にする。深い編集機能や complex layout はまだ扱わず、Web UI の基本構造を早めに固定する。

## 要件

- Workspace web SPA に左サイドパネルを追加する。
- サイドパネル上部に workspace name / label を表示する。
  - 例: `my-workspace`
  - 右側に settings entry point として `⚙` または同等の icon/button を置く。
  - settings は初期実装では disabled / placeholder / diagnostics panel でもよい。
- `repositories` section を表示する。
  - 初期は API 由来の Repository list が未実装なら placeholder / local workspace repository summary でよい。
  - 既存 API に合わせて、後続で Repository API に接続しやすい component boundary にする。
- `objectives` section を表示する。
  - 既存 `/api/objectives` から objective list を取得して表示する。
  - title と state が分かる最小表示でよい。
- `workers` section を表示する。
  - `00001KVNEKH9Q` の Host/Worker API が入る前は placeholder でよい。
  - API が存在する場合は `GET /api/workers` または `GET /api/hosts/.../workers` に接続できるよう component boundary を分ける。
  - 表示名は `workers` とし、Pod は implementation detail として出さない。
- main content とは layout を分け、サイドパネルが常時表示される基本構成にする。
- narrow viewport では壊れない最低限の responsive behavior を持つ。
  - 初期は横スクロール回避 / fixed min-width 程度でよい。
- API failure は sidebar 全体を落とさず、section ごとに bounded error / empty state を表示する。
- style は現行 skeleton に合わせ、過剰な design system 化はしない。

## Non-goals

- settings 画面の本実装。
- Repository CRUD / Repository API の本実装。
- Objective detail/edit UI。
- Worker start/stop/attach 操作。
- drag-and-drop / collapsible tree / complex navigation。
- auth / multi-workspace switcher。

## 受け入れ条件

- Workspace web UI に左サイドパネルが表示される。
- sidebar header に workspace name と settings placeholder が表示される。
- `repositories` / `objectives` / `workers` の section が表示される。
- Objectives section は existing `/api/objectives` 由来の data を表示する。
- Workers section は Host/Worker API 未実装でも placeholder として安全に表示され、API 接続を後で差し替えやすい component boundary になっている。
- API failure / empty state が section 単位で表示される。
- `deno task check` と `deno task build` が通る。
- `cargo test -p yoi-workspace-server` または backend 変更がある場合の relevant tests が通る。
- `git diff --check`、`yoi ticket doctor`、`nix build .#yoi --no-link` が通る。
