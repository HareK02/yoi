<script lang="ts">
  import { untrack } from 'svelte';
  import WorkerDriveGrants from '#lib/workspace/drive-grants/WorkerDriveGrants.svelte';
  import { canRestoreWorker, createRestoreRequest, restoreWorkspaceWorker, restoreNotice, restoreErrorNotice, type RestoreRequest } from '#lib/workspace/sidebar/worker-actions.ts';
  import { workspaceWorkersStore, refreshWorkspaceWorkers } from '#lib/workspace/sidebar/worker-subscription.ts';
  import { pushWorkspaceAlert } from '#lib/workspace/alerts/store.ts';
  import { workspaceApiPath } from '#lib/workspace/api/http.ts';
  import {
    parseRuntimeCleanupExecution,
    parseRuntimeCleanupPlan,
    parseWorkerRetentionResponse,
  } from '#lib/workspace/api/runtime-workers.ts';
  import { workerHref } from '#lib/workspace/resource-links.ts';
  import { formatWorkdirPermissions } from '#lib/workspace/settings/workdir-permissions.ts';
  import { formatCurrentWorkdirRevision } from '#lib/workspace/settings/workdir-revision.ts';
  import { canOpenWorkerConsole } from '#lib/workspace/sidebar/workers.ts';
  import { liveWorkerState } from '#lib/workspace/sidebar/worker-state.ts';
  import type { CleanupWorkerCandidate, RuntimeCleanupPlanResponse, Worker } from '#lib/workspace/sidebar/types.ts';
  import type { PageProps } from './$types';

  type WorkerActionKind = 'restore' | 'pin' | 'delete';

  let { data }: PageProps = $props();
  let cleanupPlans = $state<Record<string, RuntimeCleanupPlanResponse>>({});
  let workers = $state<Worker[]>([]);
  let restoreRetries = $state<Record<string, RestoreRequest>>({});
  let busyAction = $state<{ workerKey: string; kind: WorkerActionKind } | null>(null);

  let lifetime = 0;
  let cleanupEpoch = 0;
  let catalogRefreshing = $state(true);
  let catalogReady = $state(false);

  function actionScope() {
    const id = data.workspaceId;
    const epoch = lifetime;
    return { workspaceId: id, isCurrent: () => lifetime === epoch && data.workspaceId === id };
  }

  $effect(() => {
    const id = data.workspaceId;
    lifetime++;
    cleanupEpoch++;
    busyAction = null;
    restoreRetries = {};
    catalogReady = false;
    const initial = untrack(() => data);
    workers = initial.workers?.items ?? [];
    cleanupPlans = initial.cleanupPlans;
    let previousCatalog: string | undefined;
    let previousObservation: number | undefined;
    let previousRefreshing: boolean | undefined;
    const scope = actionScope();
    const unsubscribe = workspaceWorkersStore(id).subscribe((state) => {
      if (!scope.isCurrent()) return;
      const observationChanged = previousObservation !== state.observationVersion;
      const refreshStarted = state.catalogRefreshing && previousRefreshing !== true;
      const refreshCompleted = !state.catalogRefreshing && previousRefreshing === true;
      const catalog = state.catalogWorkers === null ? undefined : JSON.stringify(state.catalogWorkers);
      const catalogChanged = catalog !== previousCatalog;
      previousObservation = state.observationVersion;
      previousRefreshing = state.catalogRefreshing;
      previousCatalog = catalog;
      catalogRefreshing = state.catalogRefreshing;
      if (observationChanged || catalogChanged || refreshStarted) {
        cleanupPlans = {};
        ++cleanupEpoch;
      }
      if (state.catalogWorkers === null) return;
      catalogReady = true;
      workers = state.catalogWorkers;
      // Subscription overlays stay visible during the GET, but cleanup must wait
      // for its authoritative catalog result, even if that result is unchanged.
      if (catalogRefreshing || (!catalogChanged && !observationChanged && !refreshCompleted)) return;
      cleanupPlans = {};
      const epoch = ++cleanupEpoch;
      for (const runtimeId of new Set(state.catalogWorkers.map((worker) => worker.runtime_id))) {
        void refreshCleanupPlan(runtimeId, scope, epoch).catch(() => {});
      }
    });
    return () => {
      lifetime++;
      cleanupEpoch++;
      unsubscribe();
    };
  });

  function workerKey(worker: Worker): string {
    return `${worker.runtime_id}/${worker.worker_id}`;
  }

  function isActionBusy(worker: Worker, kind: WorkerActionKind): boolean {
    return busyAction?.workerKey === workerKey(worker) && busyAction.kind === kind;
  }

  function actionsDisabled(): boolean {
    return busyAction !== null;
  }

  function errorMessage(payload: unknown, fallback: string): string {
    if (payload && typeof payload === 'object') {
      if ('message' in payload && typeof payload.message === 'string') return payload.message;
      if ('error' in payload) {
        const error = payload.error;
        if (typeof error === 'string') return error;
        if (error && typeof error === 'object' && 'message' in error && typeof error.message === 'string') return error.message;
      }
      if ('diagnostics' in payload && Array.isArray(payload.diagnostics)) {
        const diagnostic = payload.diagnostics.find(
          (entry): entry is { message: string } => Boolean(entry) && typeof entry === 'object' && 'message' in entry && typeof entry.message === 'string',
        );
        if (diagnostic) return diagnostic.message;
      }
    }
    return fallback;
  }

  async function refreshCleanupPlan(
    runtimeId: string,
    scope = actionScope(),
    epoch = cleanupEpoch,
  ): Promise<void> {
    if (!scope.isCurrent() || catalogRefreshing || cleanupEpoch !== epoch) return;
    const response = await fetch(
      workspaceApiPath(scope.workspaceId, `/runtimes/${encodeURIComponent(runtimeId)}/cleanup-plan`),
    );
    if (!response.ok) return;
    const plan = parseRuntimeCleanupPlan(await response.json());
    if (!scope.isCurrent() || catalogRefreshing || cleanupEpoch !== epoch) return;
    cleanupPlans = { ...cleanupPlans, [runtimeId]: plan };
  }

  async function restoreWorker(worker: Worker, retry = false): Promise<void> {
    const key = workerKey(worker);
    if (busyAction || (retry ? !restoreRetries[key] : restoreRetries[key] || !canRestoreWorker(worker))) return;
    const scope = actionScope();
    const request = retry ? restoreRetries[key] : createRestoreRequest(worker);
    busyAction = { workerKey: key, kind: 'restore' };
    cleanupPlans = {};
    ++cleanupEpoch;
    try {
      let notice;
      try {
        notice = restoreNotice(await restoreWorkspaceWorker(scope.workspaceId, worker, request));
      } catch (cause) {
        notice = restoreErrorNotice(cause);
      }
      if (!scope.isCurrent()) return;
      const next = { ...restoreRetries };
      if (notice.retry) next[key] = request;
      else delete next[key];
      restoreRetries = next;
      pushWorkspaceAlert(notice.level, notice.message, { title: notice.title });
      await refreshWorkspaceWorkers(scope.workspaceId).catch(() => {});
      if (!scope.isCurrent()) return;
      await refreshCleanupPlan(worker.runtime_id, scope).catch(() => {});
    } finally {
      if (scope.isCurrent()) busyAction = null;
    }
  }

  async function setPinned(worker: Worker, pinned: boolean): Promise<void> {
    if (busyAction) return;
    const scope = actionScope();
    busyAction = { workerKey: workerKey(worker), kind: 'pin' };
    cleanupPlans = {};
    ++cleanupEpoch;
    try {
      const response = await fetch(
        workspaceApiPath(
          scope.workspaceId,
          `/runtimes/${encodeURIComponent(worker.runtime_id)}/workers/${encodeURIComponent(worker.worker_id)}/pin`,
        ),
        { method: pinned ? 'PUT' : 'DELETE' },
      );
      const payload = await response.json().catch(() => null);
      if (!scope.isCurrent()) return;
      if (!response.ok) throw new Error(errorMessage(payload, response.statusText));
      parseWorkerRetentionResponse(payload);
      await refreshWorkspaceWorkers(scope.workspaceId);
      if (!scope.isCurrent()) return;
      await refreshCleanupPlan(worker.runtime_id, scope);
    } catch (error) {
      if (!scope.isCurrent()) return;
      pushWorkspaceAlert('error', error instanceof Error ? error.message : 'Worker pin failed', {
        title: 'Worker pin failed',
      });
    } finally {
      if (scope.isCurrent()) busyAction = null;
    }
  }

  function cleanupCandidate(worker: Worker): CleanupWorkerCandidate | undefined {
    return cleanupPlans?.[worker.runtime_id]?.workers.find(
      (candidate) => candidate.runtime_id === worker.runtime_id && candidate.runtime_worker_id === worker.worker_id,
    );
  }

  async function deleteWorker(worker: Worker, candidate: CleanupWorkerCandidate): Promise<void> {
    const plan = cleanupPlans?.[worker.runtime_id];
    if (!plan || busyAction || candidate.blocking_reason || cleanupCandidate(worker) !== candidate) return;
    const scope = actionScope();
    busyAction = { workerKey: workerKey(worker), kind: 'delete' };
    cleanupPlans = {};
    ++cleanupEpoch;
    try {
      const response = await fetch(
        workspaceApiPath(scope.workspaceId, `/runtimes/${encodeURIComponent(worker.runtime_id)}/cleanup-executions`),
        {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({
            expected_plan_revision: plan.revision,
            expected_plan_digest: plan.digest,
            worker_target_ids: [candidate.target_id],
            workdir_target_ids: [],
            confirm_dirty_discard_target_ids: [],
          }),
        },
      );
      const payload = await response.json().catch(() => null);
      if (!scope.isCurrent()) return;
      if (!response.ok) throw new Error(errorMessage(payload, response.statusText));
      const execution = parseRuntimeCleanupExecution(payload);
      const result = execution.results.find((entry) => entry.target_id === candidate.target_id);
      if (!result || result.status !== 'deleted') {
        throw new Error(result?.message ?? 'Runtime did not delete the selected Worker');
      }
      await refreshWorkspaceWorkers(scope.workspaceId);
      if (!scope.isCurrent()) return;
      await refreshCleanupPlan(worker.runtime_id, scope);
    } catch (error) {
      if (!scope.isCurrent()) return;
      pushWorkspaceAlert('error', error instanceof Error ? error.message : 'Worker deletion failed', {
        title: 'Worker deletion failed',
      });
    } finally {
      if (scope.isCurrent()) busyAction = null;
    }
  }

  function workerStatus(worker: Worker): string {
    return liveWorkerState(worker);
  }

  function workerProfile(worker: Worker): string {
    return worker.profile ?? 'unknown';
  }

  function workerDirectory(worker: Worker): string {
    const attachments = worker.workdir_attachments ?? [];
    if (attachments.length === 0) return '—';
    return attachments.map(({ alias, effective_permissions, working_directory: directory }) => {
      const repositoryKey = directory.source.kind === 'repository'
        ? directory.source.repository_key
        : null;
      const provider = repositoryKey
        ? data.repositories?.items.find((repository) => repository.repository_key === repositoryKey)?.provider
        : null;
      const label = directory.display_name ?? repositoryKey ?? 'External Workdir';
      if (directory.source.kind === 'external_grant') {
        return `${alias}: ${label} · ${formatWorkdirPermissions(effective_permissions)}`;
      }
      return `${alias}: ${label} · ${formatCurrentWorkdirRevision(directory, provider)}`;
    }).join(', ');
  }
</script>

<svelte:head>
  <title>Workers · Yoi Workspace</title>
  <meta name="description" content="Workspace Workers" />
</svelte:head>

<section class="workers-page" aria-labelledby="workers-heading">
  <header class="workers-page-header">
    <div>
      <h1 id="workers-heading">Workers</h1>
      <p>Workers running or persisted for this workspace. Pinning updates Backend retention.</p>
    </div>
    <a class="section-action" href={`/w/${data.workspaceId}/workers/new`}>New Worker</a>
  </header>

  {#if !catalogReady && data.workersError}
    <p class="section-state error">{data.workersError}</p>
  {:else if !catalogReady && !data.workers}
    <p class="section-state">Loading Workers…</p>
  {:else if workers.length === 0}
    <p class="section-state">No Workers are visible.</p>
  {:else}
    <!-- svelte-ignore a11y_no_noninteractive_tabindex (Horizontal scroll regions must be keyboard-focusable.) -->
    <div class="table-wrap workers-table-wrap" role="region" aria-label="Workers" tabindex="0">
      <table class="workers-table">
        <thead>
          <tr>
            <th>Worker</th>
            <th>Runtime</th>
            <th>Profile</th>
            <th>Status</th>
            <th>Retention</th>
            <th>Workdir</th>
            <th>Action</th>
          </tr>
        </thead>
        <tbody>
          {#each workers as worker}
            {@const cleanup = cleanupCandidate(worker)}
            {@const canDelete = cleanup && !cleanup.blocking_reason}
            {@const anyActionDisabled = actionsDisabled()}
            {@const workerDisplayName = worker.display_name || worker.label}
            <tr>
              <td>
                {#if canOpenWorkerConsole(worker) && worker.resource_key}
                  <a class="worker-title-link" href={workerHref(data.workspaceId, { ...worker, resource_key: worker.resource_key })}><strong>{workerDisplayName}</strong></a>
                {:else}
                  <strong>{workerDisplayName}</strong>
                {/if}
                <small>worker <code>{worker.resource_key}</code></small>
              </td>
              <td><code>{worker.runtime_id}</code></td>
              <td>{workerProfile(worker)}</td>
              <td>{workerStatus(worker)}</td>
              <td><span class="pill {worker.pinned ? 'success' : 'muted'}">{worker.retention_state ?? 'normal'}</span></td>
              <td>{workerDirectory(worker)}</td>
              <td>
                <div class="worker-actions" aria-label={`Actions for ${workerDisplayName}`}>
                  <button
                    class="icon-action"
                    type="button"
                    disabled={anyActionDisabled || !canRestoreWorker(worker) || Boolean(restoreRetries[workerKey(worker)])}
                    aria-label={`Restore ${workerDisplayName}`}
                    title="Restore"
                    onclick={() => restoreWorker(worker)}
                  >
                    {#if isActionBusy(worker, 'restore')}
                      <span class="spinner" aria-hidden="true"></span>
                    {:else}
                      <svg class="action-icon" aria-hidden="true" viewBox="0 0 24 24"><path d="M3 11a9 9 0 1 1 2.6 6.4" /><path d="M3 3v8h8" /></svg>
                    {/if}
                  </button>
                  {#if restoreRetries[workerKey(worker)]}
                    <button type="button" disabled={anyActionDisabled} onclick={() => restoreWorker(worker, true)} aria-label={`Retry Restore ${workerDisplayName}`}>Retry Restore</button>
                    <span role="status">Restore outcome unresolved. Retry the same request to reconcile.</span>
                  {/if}
                  <button
                    class="icon-action"
                    type="button"
                    disabled={anyActionDisabled}
                    aria-label={worker.pinned ? `Unpin ${workerDisplayName}` : `Pin ${workerDisplayName}`}
                    title={worker.pinned ? 'Unpin' : 'Pin'}
                    onclick={() => setPinned(worker, !worker.pinned)}
                  >
                    {#if isActionBusy(worker, 'pin')}
                      <span class="spinner" aria-hidden="true"></span>
                    {:else if worker.pinned}
                      <svg class="action-icon" aria-hidden="true" viewBox="0 0 24 24"><path d="M12 17v5" /><path d="M15 9.34V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H7.89" /><path d="m2 2 20 20" /><path d="M9 9v1.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h11" /></svg>
                    {:else}
                      <svg class="action-icon" aria-hidden="true" viewBox="0 0 24 24"><path d="M12 17v5" /><path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z" /></svg>
                    {/if}
                  </button>
                  {#if cleanup}
                    <button
                      class="icon-action danger"
                      type="button"
                      disabled={!canDelete || anyActionDisabled}
                      aria-label={`Delete ${workerDisplayName}`}
                      title={cleanup.blocking_reason ?? cleanup.reason}
                      onclick={() => deleteWorker(worker, cleanup)}
                    >
                      {#if isActionBusy(worker, 'delete')}
                        <span class="spinner" aria-hidden="true"></span>
                      {:else}
                        <svg class="action-icon" aria-hidden="true" viewBox="0 0 24 24"><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6" /><path d="M3 6h18" /><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" /></svg>
                      {/if}
                    </button>
                  {/if}
                </div>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}

  <WorkerDriveGrants
    workspaceId={data.workspaceId}
    {workers}
    workersReady={catalogReady && !catalogRefreshing}
    canManage={data.workspace?.permissions.manage_runtimes === true}
  />
</section>

<style>
  .workers-page {
    min-width: 0;
    grid-template-columns: minmax(0, 1fr);
  }

  .workers-table-wrap {
    min-width: 0;
    overflow-x: auto;
  }

  .workers-table-wrap:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }

  .worker-title-link {
    color: inherit;
    text-decoration: none;
  }

  .worker-title-link:hover,
  .worker-title-link:focus-visible {
    color: var(--accent);
    text-decoration: underline;
  }

  .worker-actions {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
  }

  .icon-action {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 2rem;
    height: 2rem;
    padding: 0;
    border: 1px solid var(--line);
    border-radius: 0.5rem;
    background: var(--bg-raised);
    color: var(--text);
    cursor: pointer;
  }

  .icon-action:hover:not(:disabled),
  .icon-action:focus-visible:not(:disabled) {
    border-color: var(--accent);
    color: var(--accent);
  }

  .icon-action.danger:hover:not(:disabled),
  .icon-action.danger:focus-visible:not(:disabled) {
    border-color: var(--danger, oklch(60% 0.18 30));
    color: var(--danger, oklch(60% 0.18 30));
  }

  .icon-action:disabled {
    cursor: not-allowed;
    opacity: 0.45;
  }

  .action-icon {
    width: 1rem;
    height: 1rem;
    fill: none;
    stroke: currentColor;
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }

  .spinner {
    width: 1rem;
    height: 1rem;
    border: 2px solid currentColor;
    border-right-color: transparent;
    border-radius: 999px;
    animation: worker-action-spin 0.8s linear infinite;
  }

  @keyframes worker-action-spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
