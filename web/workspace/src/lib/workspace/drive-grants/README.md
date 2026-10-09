# Worker Drive grant controls

`WorkerDriveGrants.svelte` is embedded in the existing Workers page. The page supplies its current `workspaceWorkersStore().catalogWorkers` projection; this component neither discovers Workers nor maintains a registry.

## Integration interface

```ts
{
  workspaceId: string;
  workers: Pick<Worker, "runtime_id" | "worker_id" | "display_name" | "label">[];
  canManage: boolean;
  workersReady?: boolean; // defaults to true for standalone use
}
```

The page maps `canManage` to `workspace.permissions.manage_runtimes` (the current Backend owner settings permission) and `workersReady` to `catalogReady && !catalogRefreshing`. Non-owners have no grant surface or grant requests. Backend owner authorization remains authoritative on every request. Profile Drive tool enablement is separate and is not edited here.

`api.ts` is the owner-controlled grant transport adapter using generated `drive-api.ts` DTOs. It keeps grant administration separate from ordinary content access:

- `listDriveGrants(workspaceId): Promise<DriveGrantResponse[]>` — fetch **all** grant pages (200/page, bounded to 100 pages); reject incomplete, foreign-Workspace, duplicate, unordered or cyclic results instead of silently truncating.
- `createDriveGrant(workspaceId, DriveGrantCreateRequest): Promise<DriveGrantResponse>`.
- `revokeDriveGrant(workspaceId, DriveGrantResponse): Promise<DriveGrantResponse>`.
- `DriveGrantError` with `outcome: "not_committed" | "unknown"`.
- Generated `DriveAccess` / `DriveGrantResponse` type exports.

Mutation success must validate Workspace/Runtime/Worker identity and the requested resulting state. HTTP bodies are bounded; transport loss, malformed success and unclassified failures are unknown mutation outcomes. No Worker credentials or transport identity headers are sent; requests use current browser same-origin authentication and bypass caching.

Workspace, selected Worker, catalog membership/readiness, permission and component lifetime fence asynchronous results. Workspace switches clear selection; unchanged catalog identities do not clear pending mutations. Active grants match **both** Runtime and Worker IDs. An existing active grant must be revoked before another access level is created. Unknown outcomes remove actionable state until an explicit authority refresh, never automatic mutation retry.

## Narrow validation

From `web/workspace`:

```sh
deno task test:component src/lib/workspace/drive-grants/WorkerDriveGrants.browser.spec.ts 'src/routes/w/[workspaceId]/workers/workers-page.browser.spec.ts' 'src/routes/w/[workspaceId]/workers/workers-restore-integration.browser.spec.ts'
deno run -A npm:svelte-check@4.7.6 --tsconfig ./src/lib/workspace/drive-grants/tsconfig.check.json
# Full integration check (also includes sibling-owned work):
deno task check
```

Recorded result: 21 grant tests plus 29 existing Workers tests passed. Final parent integration `deno task check` reported zero errors/warnings, and the complete component suite passed 301 tests in 32 files. Intermediate test-only `ByRoleOptions` errors were corrected before final validation.

The component tests exercise actual rendered controls with HTTP-boundary fixtures, including pagination, same Worker ID on different Runtimes, owner gating, scope switches, removal, single-submit behavior, read-only/read-write creation, revocation, denial and unknown outcomes. The existing Workers tests cover retention/restore/catalog regressions.

## Visual evidence

The development-only `tools/web-ux/drive-grants/visual-review.ts` runner uses the real Workers route and application shell with loopback browser API fixtures. It creates no real Backend, Runtime, Worker, account or grant state. It lives outside production Web source and is independently Deno-checked:

```sh
# From repository root, in the dev shell:
deno run --check --config tools/web-ux/deno.json -A tools/web-ux/drive-grants/visual-review.ts after
```

`before` mode must run against the original Workers page source. For the recorded baseline, the original page was temporarily restored from the read-only Git source and the implemented page restored in `finally`; Git/index/history were not changed.

Evidence: `target/web-ux/t724-drive-grants/{before,after}/review-context.json` and sibling PNGs. Contexts record screenshot hashes, Workers page source hash, browser version, accessibility, visible/browser/request errors and document overflow. The after context also records component/API source hashes and the runner checks grant controls for viewport clipping.

Reviewed owner/non-owner, light/dark, 1440/768/390 px; active, empty/create form, loading, deliberately failed list, long labels/identifiers/errors. The before bundle has 24 captures and the after bundle has 42. `compare-{theme}-{width}.png` and `states-{theme}-{width}.png` contact sheets in the same artifact root were opened by the implementer, along with individual captures.

Finding fixed: the pre-existing Worker table widened the page's grid track and clipped new grant controls. The page now constrains its grid and gives the table a named keyboard-accessible local scroll region. Long native-select labels have a full identity disclosure. No unexpected browser errors or document overflow were observed; intentional HTTP 503 console errors belong to the error fixture (and remain in the cumulative diagnostics of its subsequent loading capture). Other pre-existing Worker table presentation is unchanged.
