<script lang="ts">
  import { workerConsoleHref } from '$lib/workspace/console/model';
  import { workspaceWorkersStore } from './worker-subscription';
  import { canShowWorkerInSidebar } from './workers';
  import type { Worker } from './types';

  const MAX_VISIBLE_WORKERS = 6;

  type Props = {
    currentPath?: string;
    workspaceId: string;
  };

  let { currentPath = '/', workspaceId }: Props = $props();
  let loading = $state(true);
  let error = $state<string | null>(null);
  let workers = $state<Worker[]>([]);

  $effect(() => {
    const subscription = workspaceWorkersStore(workspaceId);
    return subscription.subscribe((state) => {
      loading = state.loading;
      error = state.error;
      workers = state.workers.filter(canShowWorkerInSidebar).slice(0, MAX_VISIBLE_WORKERS);
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
      {#each workers as worker (`${worker.runtime_id}:${worker.worker_id}`)}
        {@const href = workerConsoleHref(worker, workspaceId)}
        <li>
          <a href={href} class="nav-item worker-nav-item" class:active={currentPath === href} aria-current={currentPath === href ? 'page' : undefined}>
            <span class="worker-title-row">
              <span class="item-title">{worker.display_name || worker.label}</span>
              <span class="worker-task-title">-</span>
            </span>
            <span class="item-meta">
              worker {worker.worker_id} · {worker.profile ? `${worker.profile} · ` : ''}{worker.state} · 🖥 {worker.host_id}
              {worker.working_directory?.current_ref ? ` · wd:${worker.working_directory.repository_id}@${worker.working_directory.current_ref.slice(0, 8)}` : ''}
            </span>
          </a>
        </li>
      {/each}
    </ul>
  {/if}
</section>
