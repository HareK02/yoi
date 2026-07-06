---
title: 'Scope Workspace Browser routes and API by workspace id'
state: 'inprogress'
created_at: '2026-07-06T19:25:08Z'
updated_at: '2026-07-06T19:54:30Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-07-06T19:53:14Z'
---

## 背景

現在の Workspace Browser は 1 Backend = 1 Workspace を前提に、Frontend route と Browser-facing API のどちらも暗黙の current Workspace を参照している。

```text
/
/repositories/<repository-id>
/objectives
/settings
/runtimes/<runtime-id>/workers/<worker-id>/console

/api/workspace
/api/repositories
/api/objectives
/api/settings/runtime-connections
/api/workers
```

この形では、将来 1 Backend process が複数 Workspace を扱う設計に進んだとき、URL / API authority / Sidebar data / Worker console context がどの Workspace に属するのか曖昧になる。1 Backend = 1 Workspace の間でも、Browser URL と API contract は Workspace id を含む形にしておく。

Workspace URL は slug/renameable handle ではなく immutable Workspace id を使う。多少長い id は許容し、URL の安定性を優先する。Route prefix は短く `/w/<workspace-id>/...` とする。Backend 管理 UI や global settings は将来 `/admin` / `/settings` / `/backend` など別 top-level に置ける。

同時に、Browser から叩く SvelteKit/Backend product API も Workspace id scope を持たせる。UI route だけ Workspace scoped にして API が `/api/...` のままだと、暗黙 current Workspace が残るため、Frontend は scoped API を使う。

## 要件

- Browser routes を `/w/<workspace-id>/...` に移す。
- Browser-facing API に `/api/w/<workspace-id>/...` を追加する。
- 1 Backend = 1 Workspace の現状でも、request path の `workspace-id` と Backend が serving している `workspace_id` を照合する。
- workspace id mismatch は typed sanitized error として返す。Browser-facing には raw path / internal store / authority-bearing internals を漏らさない。
- Frontend は route params の `workspaceId` を使う workspace-scoped API client/helper から API を呼ぶ。
- Sidebar / settings / repository / objectives / worker console の links は workspace id を含む。
- 既存 unscoped API は互換 alias として残してよいが、Frontend からは scoped API を使う。
- 既存 unscoped routes は redirect するか、互換を残す場合も canonical route は `/w/<workspace-id>/...` とする。
- Runtime API (`/v1/runtime`, `/v1/workers`) とは混同しない。この Ticket の API scope は Browser-facing Workspace Backend API の話である。

## 受け入れ条件

- `/w/<workspace-id>` が現行 overview を表示する。
- `/w/<workspace-id>/repositories/<repository-id>` が Repository detail を表示する。
- `/w/<workspace-id>/objectives` と `/w/<workspace-id>/objectives/<objective-id>` が動く。
- `/w/<workspace-id>/settings` が Runtime/Workspace settings を表示する。
- `/w/<workspace-id>/runtimes/<runtime-id>/workers/<worker-id>/console` が Worker console を表示する。
- Frontend の API 呼び出しは `/api/w/<workspace-id>/...` を使う。
- `/api/w/<workspace-id>/workspace`, `/repositories`, `/objectives`, `/workers`, `/settings/runtime-connections` など主要 Browser-facing API が scoped path で動く。
- `workspace-id` が Backend の current `workspace_id` と違う場合は typed 404 または equivalent mismatch diagnostic を返す。
- Sidebar navigation links と Worker console links は workspace id を含む。
- 旧 unscoped route/API を互換として残す場合でも、Frontend test は scoped route/API を期待する。
- `cd web/workspace && deno task check && deno task test` が通る。
- `cargo test -p yoi-workspace-server` が通る。

## 非目標

- Backend の canonical store を multi-workspace DB に移行すること。
- `/api/workspaces/<workspace-id>/...` 形式の long form API を同時に実装すること。
- Workspace slug / handle / rename / alias 機能を実装すること。
- Runtime `/v1/...` API に workspace id を path segment として入れること。
- Backend global admin UI を完成させること。
