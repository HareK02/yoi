<script lang="ts">
  import { untrack } from 'svelte';
  import Spinner from '#lib/workspace/console/Spinner.svelte';
  import { workerConsoleHref } from '#lib/workspace/resource-links.ts';
  import { pushWorkspaceAlert } from '#lib/workspace/alerts/store.ts';
  import {
    canRestoreWorker,
    createRestoreRequest,
    restoreWorkspaceWorker,
    restoreNotice,
    restoreErrorNotice,
    type RestoreRequest,
    canStopSidebarWorker,
    canDeleteSidebarWorker,
    deleteSidebarWorker,
    stopSidebarWorker,
  } from './worker-actions';
  import {
    workspaceWorkersStore,
    refreshWorkspaceWorkers,
    type SidebarWorker,
  } from './worker-subscription';
  import {
    canShowWorkerInSidebar,
    sidebarWorkerActivity,
    visibleWorkersForSidebar,
    workerOwnsSidebarPath,
  } from './workers';
  import { sidebarWorkdirMeta } from './worker-workdir-meta';

  const COLLAPSED_WORKER_COUNT = 6;
  type WorkerActionKind = 'restore' | 'stop' | 'delete';

  type Props = {
    currentPath?: string;
    workspaceId: string;
  };

  let { currentPath = '/', workspaceId }: Props = $props();
  let loading = $state(true);
  let workers = $state<SidebarWorker[]>([]);
  let expanded = $state(false);
  let openWorkerKey = $state<string | null>(null);
  let menuElement = $state<HTMLElement | null>(null);
  let menuTrigger = $state<HTMLButtonElement | null>(null);
  let restoreRetries = $state<Record<string, RestoreRequest>>({});
  let busyAction = $state<{ workerKey: string; kind: WorkerActionKind } | null>(null);
  let visibleWorkers = $derived(
    visibleWorkersForSidebar(workers, {
      workspaceId,
      currentPath,
      expanded,
      limit: COLLAPSED_WORKER_COUNT,
    }),
  );
  let hiddenWorkerCount = $derived(
    Math.max(0, workers.length - COLLAPSED_WORKER_COUNT),
  );

  function workerKey(worker: SidebarWorker): string {
    return `${worker.runtime_id}:${worker.worker_id}`;
  }

  function isBusy(worker: SidebarWorker, kind: WorkerActionKind): boolean {
    return busyAction?.workerKey === workerKey(worker) && busyAction.kind === kind;
  }

  function closeWorkerMenu(restoreFocus = false) {
    const trigger = menuTrigger;
    openWorkerKey = null;
    menuElement = null;
    menuTrigger = null;
    if (restoreFocus) queueMicrotask(() => trigger?.focus());
  }

  function toggleWorkerMenu(worker: SidebarWorker, trigger: HTMLButtonElement) {
    const key = workerKey(worker);
    if (openWorkerKey === key) {
      closeWorkerMenu();
      return;
    }
    openWorkerKey = key;
    menuTrigger = trigger;
    queueMicrotask(() => {
      menuElement?.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus();
    });
  }

  function handleWindowClick(event: MouseEvent) {
    if (!openWorkerKey) return;
    const target = event.target;
    const owner = target instanceof Element ? target.closest('[data-worker-actions]') : null;
    if (owner?.getAttribute('data-worker-actions') !== openWorkerKey) closeWorkerMenu();
  }

  function handleWindowKeydown(event: KeyboardEvent) {
    if (event.key !== 'Escape' || !openWorkerKey) return;
    event.preventDefault();
    closeWorkerMenu(true);
  }

  let lifetime = 0;

  function actionScope() {
    const id = workspaceId;
    const epoch = lifetime;
    return { workspaceId: id, isCurrent: () => lifetime === epoch && workspaceId === id };
  }

  async function restoreWorker(worker: SidebarWorker, retry = false) {
    const key = workerKey(worker);
    if (busyAction || (retry ? !restoreRetries[key] : restoreRetries[key] || !canRestoreWorker(worker))) return;
    const scope = actionScope();
    const request = retry ? restoreRetries[key] : createRestoreRequest(worker);
    closeWorkerMenu();
    busyAction = { workerKey: key, kind: 'restore' };
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
    } finally {
      if (scope.isCurrent()) busyAction = null;
    }
  }

  async function stopWorker(worker: SidebarWorker) {
    if (busyAction || !canStopSidebarWorker(worker)) return;
    const scope = actionScope();
    closeWorkerMenu();
    busyAction = { workerKey: workerKey(worker), kind: 'stop' };
    try {
      await stopSidebarWorker(scope.workspaceId, worker);
      if (!scope.isCurrent()) return;
      await refreshWorkspaceWorkers(scope.workspaceId);
      if (!scope.isCurrent()) return;
      pushWorkspaceAlert('info', `${worker.display_name || worker.label} stopped`, {
        title: 'Worker stopped',
      });
    } catch (cause) {
      if (!scope.isCurrent()) return;
      pushWorkspaceAlert('error', cause instanceof Error ? cause.message : 'Worker stop failed', {
        title: 'Worker stop failed',
      });
    } finally {
      if (scope.isCurrent()) busyAction = null;
    }
  }

  async function deleteWorker(worker: SidebarWorker) {
    if (busyAction || !canDeleteSidebarWorker(worker)) return;
    const scope = actionScope();
    closeWorkerMenu();
    busyAction = { workerKey: workerKey(worker), kind: 'delete' };
    try {
      await deleteSidebarWorker(scope.workspaceId, worker, (input, init) => {
        if (!scope.isCurrent()) throw new Error('Worker action is no longer current');
        return fetch(input, init);
      });
      if (!scope.isCurrent()) return;
      await refreshWorkspaceWorkers(scope.workspaceId);
      if (!scope.isCurrent()) return;
      pushWorkspaceAlert('info', `${worker.display_name || worker.label} deleted`, {
        title: 'Worker deleted',
      });
    } catch (cause) {
      if (!scope.isCurrent()) return;
      pushWorkspaceAlert('error', cause instanceof Error ? cause.message : 'Worker deletion failed', {
        title: 'Worker deletion failed',
      });
    } finally {
      if (scope.isCurrent()) busyAction = null;
    }
  }

  $effect(() => {
    const id = workspaceId;
    lifetime++;
    busyAction = null;
    restoreRetries = {};
    expanded = false;
    untrack(closeWorkerMenu);
    const unsubscribe = workspaceWorkersStore(id).subscribe((state) => {
      loading = state.loading;
      workers = state.workers.filter(canShowWorkerInSidebar);
    });
    return () => {
      lifetime++;
      unsubscribe();
    };
  });
</script>

<svelte:window onclick={handleWindowClick} onkeydown={handleWindowKeydown} />

<section class="sidebar-nav-section" aria-labelledby="workers-heading">
  <div class="section-heading-row">
    <h2 id="workers-heading">
      <a
        class="section-heading-link"
        class:active={currentPath === `/w/${workspaceId}/workers`}
        href={`/w/${workspaceId}/workers`}
        aria-current={currentPath === `/w/${workspaceId}/workers` ? 'page' : undefined}
      >workers</a>
    </h2>
    <a
      class="section-action"
      class:active={currentPath === `/w/${workspaceId}/workers/new`}
      href={`/w/${workspaceId}/workers/new`}
      aria-current={currentPath === `/w/${workspaceId}/workers/new` ? 'page' : undefined}
    >
      New
    </a>
    {#if !loading && workers.length > 0}
      <span class="section-count">{workers.length}</span>
    {/if}
  </div>

  {#if loading}
    <p class="section-state">Checking workers…</p>
  {:else if workers.length === 0}
    <p class="section-state">No Workers are active.</p>
  {:else}
    <ul class="nav-list" aria-label="Workers">
      {#each visibleWorkers as worker (`${worker.runtime_id}:${worker.worker_id}`)}
        {@const href = workerConsoleHref(workspaceId, worker)}
        {@const activity = sidebarWorkerActivity(worker)}
        {@const key = workerKey(worker)}
        {@const label = worker.display_name || worker.label}
        {@const active = workerOwnsSidebarPath(workspaceId, worker, currentPath)}
        {@const workdir = sidebarWorkdirMeta(worker.workdir_attachments)}
        <li class="worker-nav-item" data-worker-actions={key}>
          <a
            href={href}
            class="worker-nav-link"
            class:active
            aria-current={active ? 'page' : undefined}
          >
            <span class="worker-status-indicator">
              {#if activity === 'worker-running'}
                <span class="worker-status-spinner"><Spinner label="Running" /></span>
              {:else if activity === 'subworker-running'}
                <span class="worker-status-spinner is-subworker"><Spinner label="SubWorker running" /></span>
              {:else if activity === 'idle'}
                <span class="worker-status-dot" aria-label="Idle"></span>
              {/if}
            </span>
            <span class="worker-nav-label">{label}</span>
            <small class="worker-nav-meta" title={workdir.details} aria-label={workdir.details}>
              {workdir.text}
            </small>
          </a>
          <button
            class="worker-actions-trigger"
            class:open={openWorkerKey === key}
            type="button"
            aria-label={`Actions for ${label}`}
            aria-haspopup="menu"
            aria-expanded={openWorkerKey === key}
            onclick={(event) => toggleWorkerMenu(worker, event.currentTarget)}
          >
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <circle cx="5" cy="12" r="1.5"></circle>
              <circle cx="12" cy="12" r="1.5"></circle>
              <circle cx="19" cy="12" r="1.5"></circle>
            </svg>
          </button>
          {#if openWorkerKey === key}
            <div class="worker-actions-menu" role="menu" aria-label={`Actions for ${label}`} bind:this={menuElement}>
              <button
                type="button"
                role="menuitem"
                disabled={busyAction !== null || !canRestoreWorker(worker) || Boolean(restoreRetries[key])}
                onclick={() => restoreWorker(worker)}
              >
                {isBusy(worker, 'restore') ? 'Restoring…' : 'Restore'}
              </button>
              {#if restoreRetries[key]}
                <button
                  type="button"
                  role="menuitem"
                  disabled={busyAction !== null}
                  onclick={() => restoreWorker(worker, true)}
                >Retry Restore</button>
                <p role="status">Restore outcome unresolved. Retry the same request to reconcile.</p>
              {/if}
              <button
                type="button"
                role="menuitem"
                disabled={busyAction !== null || !canStopSidebarWorker(worker)}
                onclick={() => stopWorker(worker)}
              >
                {isBusy(worker, 'stop') ? 'Stopping…' : 'Stop'}
              </button>
              <button
                class="danger"
                type="button"
                role="menuitem"
                disabled={busyAction !== null || !canDeleteSidebarWorker(worker)}
                onclick={() => deleteWorker(worker)}
              >
                {isBusy(worker, 'delete') ? 'Deleting…' : 'Delete'}
              </button>
            </div>
          {/if}
        </li>
      {/each}
    </ul>
    {#if workers.length > COLLAPSED_WORKER_COUNT}
      <button
        class="worker-overflow-toggle"
        type="button"
        aria-expanded={expanded}
        aria-label={expanded
          ? 'Collapse Worker list'
          : `Show ${hiddenWorkerCount} more Workers`}
        title={expanded
          ? 'Collapse Worker list'
          : `Show ${hiddenWorkerCount} more Workers`}
        onclick={() => (expanded = !expanded)}
      >
        <span class="worker-overflow-line" aria-hidden="true"></span>
        <svg
          class="worker-overflow-chevron"
          viewBox="0 0 24 24"
          aria-hidden="true"
        >
          <path d="m6 9 6 6 6-6"></path>
        </svg>
        <span class="worker-overflow-line" aria-hidden="true"></span>
      </button>
    {/if}
  {/if}
</section>
