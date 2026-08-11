<script lang="ts">
  import { workerConsoleHref } from '$lib/workspace/console/model';
  import {
    workspaceWorkersStore,
    type SidebarWorker,
  } from './worker-subscription';
  import { canShowWorkerInSidebar } from './workers';

  const COLLAPSED_WORKER_COUNT = 6;

  type Props = {
    currentPath?: string;
    workspaceId: string;
  };

  let { currentPath = '/', workspaceId }: Props = $props();
  let loading = $state(true);
  let error = $state<string | null>(null);
  let workers = $state<SidebarWorker[]>([]);
  let expanded = $state(false);
  let visibleWorkers = $derived(
    expanded ? workers : workers.slice(0, COLLAPSED_WORKER_COUNT),
  );
  let hiddenWorkerCount = $derived(
    Math.max(0, workers.length - COLLAPSED_WORKER_COUNT),
  );

  $effect(() => {
    expanded = false;
    const subscription = workspaceWorkersStore(workspaceId);
    return subscription.subscribe((state) => {
      loading = state.loading;
      error = state.error;
      workers = state.workers.filter(canShowWorkerInSidebar);
    });
  });
</script>

<section class="nav-section" aria-labelledby="workers-heading">
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
        <li>
          <a
            href={href}
            class="worker-nav-link"
            class:active={currentPath === href}
            aria-current={currentPath === href ? 'page' : undefined}
          >
            <span class="worker-status-indicator">
              {#if worker.state === 'idle'}
                <span class="worker-status-dot" aria-label="Idle"></span>
              {:else if worker.state === 'running'}
                <span class="worker-status-spinner" aria-label="Running"></span>
              {/if}
            </span>
            <span class="worker-nav-label">{worker.display_name || worker.label}</span>
            <small class="worker-nav-meta">
              {worker.repository_id ?? '—'}・{worker.working_directory_id ?? '—'}
            </small>
          </a>
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
