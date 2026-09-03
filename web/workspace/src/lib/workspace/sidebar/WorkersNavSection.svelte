<script lang="ts">
  import Spinner from '$lib/workspace/console/Spinner.svelte';
  import { workerConsoleHref } from '$lib/workspace/console/model';
  import { pushWorkspaceAlert } from '$lib/workspace/alerts/store';
  import {
    canDeleteSidebarWorker,
    deleteSidebarWorker,
    stopSidebarWorker,
  } from './worker-actions';
  import {
    workspaceWorkersStore,
    type SidebarWorker,
  } from './worker-subscription';
  import { canShowWorkerInSidebar, sidebarWorkerActivity } from './workers';

  const COLLAPSED_WORKER_COUNT = 6;
  type WorkerActionKind = 'stop' | 'delete';

  type Props = {
    currentPath?: string;
    workspaceId: string;
  };

  let { currentPath = '/', workspaceId }: Props = $props();
  let loading = $state(true);
  let error = $state<string | null>(null);
  let workers = $state<SidebarWorker[]>([]);
  let expanded = $state(false);
  let openWorkerKey = $state<string | null>(null);
  let menuElement = $state<HTMLElement | null>(null);
  let menuTrigger = $state<HTMLButtonElement | null>(null);
  let busyAction = $state<{ workerKey: string; kind: WorkerActionKind } | null>(null);
  let visibleWorkers = $derived(
    expanded ? workers : workers.slice(0, COLLAPSED_WORKER_COUNT),
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

  async function stopWorker(worker: SidebarWorker) {
    if (busyAction || !worker.capabilities.can_stop) return;
    closeWorkerMenu();
    busyAction = { workerKey: workerKey(worker), kind: 'stop' };
    try {
      await stopSidebarWorker(workspaceId, worker);
      workers = workers.map((item) =>
        workerKey(item) === workerKey(worker)
          ? { ...item, state: 'stopped', capabilities: { ...item.capabilities, can_stop: false } }
          : item
      );
      pushWorkspaceAlert('info', `${worker.display_name || worker.label} stopped`, {
        title: 'Worker stopped',
      });
    } catch (cause) {
      pushWorkspaceAlert('error', cause instanceof Error ? cause.message : 'Worker stop failed', {
        title: 'Worker stop failed',
      });
    } finally {
      busyAction = null;
    }
  }

  async function deleteWorker(worker: SidebarWorker) {
    if (busyAction || !canDeleteSidebarWorker(worker)) return;
    closeWorkerMenu();
    busyAction = { workerKey: workerKey(worker), kind: 'delete' };
    try {
      await deleteSidebarWorker(workspaceId, worker);
      workers = workers.filter((item) => workerKey(item) !== workerKey(worker));
      pushWorkspaceAlert('info', `${worker.display_name || worker.label} deleted`, {
        title: 'Worker deleted',
      });
    } catch (cause) {
      pushWorkspaceAlert('error', cause instanceof Error ? cause.message : 'Worker deletion failed', {
        title: 'Worker deletion failed',
      });
    } finally {
      busyAction = null;
    }
  }

  $effect(() => {
    expanded = false;
    openWorkerKey = null;
    menuElement = null;
    menuTrigger = null;
    const subscription = workspaceWorkersStore(workspaceId);
    return subscription.subscribe((state) => {
      loading = state.loading;
      error = state.error;
      workers = state.workers.filter(canShowWorkerInSidebar);
    });
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
    {#if !loading && !error && workers.length > 0}
      <span class="section-count">{workers.length}</span>
    {/if}
  </div>

  {#if loading}
    <p class="section-state">Checking workers…</p>
  {:else if workers.length === 0}
    <p class="section-state" class:error={Boolean(error)}>{error ?? 'No Workers are active.'}</p>
  {:else}
    {#if error}<p class="section-state error">{error}</p>{/if}
    <ul class="nav-list" aria-label="Workers">
      {#each visibleWorkers as worker (`${worker.runtime_id}:${worker.worker_id}`)}
        {@const href = workerConsoleHref(worker, workspaceId)}
        {@const activity = sidebarWorkerActivity(worker)}
        {@const key = workerKey(worker)}
        {@const label = worker.display_name || worker.label}
        <li class="worker-nav-item" data-worker-actions={key}>
          <a
            href={href}
            class="worker-nav-link"
            class:active={currentPath === href}
            aria-current={currentPath === href ? 'page' : undefined}
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
            <small class="worker-nav-meta">
              {worker.repository_key ?? '—'}・{worker.working_directory_id ?? '—'}
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
                disabled={busyAction !== null || !worker.capabilities.can_stop}
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
